//! hl-bsp -- a host-side inspector for GoldSrc (Half-Life) BSP v30 maps.
//!
//! Parses the lump table and reports the geometry + texture budget that
//! decides PS1 feasibility (Milestone 1): vertex/face counts, texture count
//! and total texels, embedded-vs-WAD split, lightmap/vis sizes, and the map
//! bounding box. Seed of the eventual map extractor; for now it only reads and
//! reports (no cooking).
//!
//! Usage:  hl-bsp <path/to/map.bsp>
//!     cargo run --release --manifest-path host/hl-bsp/Cargo.toml -- valve/maps/c1a0.bsp

mod entity_support;
mod quad_grid;
mod seated_placement;
mod testmap;

use hl_format::logic::*;
use hl_format::map as cooked;
use psx_render_contract::CookedDrawSurfaceCommand;
use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::process::exit;

// GoldSrc BSP v30 lump indices.
const LUMP_ENTITIES: usize = 0;
const LUMP_PLANES: usize = 1;
const LUMP_TEXTURES: usize = 2;
const LUMP_VERTEXES: usize = 3;
const LUMP_VISIBILITY: usize = 4;
const LUMP_NODES: usize = 5;
const LUMP_TEXINFO: usize = 6;
const LUMP_FACES: usize = 7;
const LUMP_LIGHTING: usize = 8;
const LUMP_CLIPNODES: usize = 9;
const LUMP_LEAVES: usize = 10;
const LUMP_MARKSURFACES: usize = 11;
const LUMP_EDGES: usize = 12;
const LUMP_SURFEDGES: usize = 13;
const LUMP_MODELS: usize = 14;
const NUM_LUMPS: usize = 15;
const HEADER_LEN: usize = 4 + NUM_LUMPS * 8;

// On-disk struct sizes (bytes).
const SZ_VERTEX: usize = 12; // 3 × f32
const SZ_EDGE: usize = 4; // 2 × u16
const SZ_SURFEDGE: usize = 4; // i32
const SZ_FACE: usize = 20;
const SZ_TEXINFO: usize = 40;
const SZ_NODE: usize = 24;
const SZ_LEAF: usize = 28;
const SZ_MODEL: usize = 64;
const SZ_MARKSURFACE: usize = 2;
const SZ_PLANE: usize = 20; // f32 normal[3] + f32 dist + i32 type
const SZ_CLIPNODE: usize = 8; // i32 planenum + i16 children[2]
const SZ_LEAF_VISOFS: usize = 4; // dleaf_t.visofs at byte 4
const SZ_LEAF_MARK0: usize = 20; // dleaf_t.firstmarksurface at byte 20
const SZ_MODEL_HEADNODE0: usize = 36; // dmodel_t.headnode[0], followed by hulls 1..3
const SZ_MODEL_VISLEAFS: usize = 52; // dmodel_t.visleafs in world model 0

fn u16le(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn u32le(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn i32le(b: &[u8], o: usize) -> Option<i32> {
    u32le(b, o).map(|v| v as i32)
}

fn model_headnode(models: &[u8], model: usize, hull: usize) -> Option<i32> {
    if hull >= 4 {
        return None;
    }
    i32le(
        models,
        model * SZ_MODEL + SZ_MODEL_HEADNODE0 + hull * core::mem::size_of::<i32>(),
    )
}
fn f32le(b: &[u8], o: usize) -> Option<f32> {
    u32le(b, o).map(f32::from_bits)
}

/// GoldSrc's PVS rows cover `dmodel[0].visleafs`, not every record in the
/// leaf lump. BSP compilers append submodel-only leaves after the world set.
fn world_visleaf_count(models: &[u8], n_leaves: usize) -> Result<usize, String> {
    let raw = i32le(models, SZ_MODEL_VISLEAFS)
        .ok_or_else(|| "BSP model lump has no world dmodel visleaf count".to_string())?;
    if raw < 0 || raw as usize > n_leaves.saturating_sub(1) {
        return Err(format!(
            "world dmodel visleaf count {raw} exceeds {} non-solid leaf records",
            n_leaves.saturating_sub(1)
        ));
    }
    Ok(raw as usize)
}

/// HLMD keeps the BSP header at 24 bytes: low 16 bits are total leaf records,
/// high 16 bits are world PVS clusters. GoldSrc's map limits fit both fields.
fn pack_leaf_counts(n_leaves: usize, n_visleaves: usize) -> Result<u32, String> {
    if n_leaves > u16::MAX as usize || n_visleaves > u16::MAX as usize {
        return Err(format!(
            "HLM leaf counts exceed u16 (records {n_leaves}, visleafs {n_visleaves})"
        ));
    }
    Ok(n_leaves as u32 | ((n_visleaves as u32) << 16))
}

fn pack_leaf_mark_count(mark_count: u16, contents: i32) -> Result<u16, String> {
    let liquid = match contents {
        -3 => cooked::LEAF_LIQUID_WATER,
        -4 => cooked::LEAF_LIQUID_SLIME,
        -5 => cooked::LEAF_LIQUID_LAVA,
        _ => cooked::LEAF_LIQUID_NONE,
    };
    cooked::pack_leaf_mark_count(mark_count, liquid).ok_or_else(|| {
        format!(
            "cooked leaf has {mark_count} marks; liquid-tagged format supports at most {}",
            cooked::LEAF_MARK_COUNT_MASK
        )
    })
}

#[derive(Clone, Copy)]
struct Lump {
    ofs: usize,
    len: usize,
}

struct Bsp<'a> {
    bytes: &'a [u8],
    lumps: [Lump; NUM_LUMPS],
}

impl<'a> Bsp<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Bsp<'a>, String> {
        if bytes.len() < HEADER_LEN {
            return Err(format!(
                "file too small ({} bytes) to be a BSP",
                bytes.len()
            ));
        }
        let version = i32le(bytes, 0).unwrap();
        if version != 30 {
            return Err(format!(
                "BSP version {} (expected 30 = GoldSrc/Half-Life; 29 = Quake)",
                version
            ));
        }
        let mut lumps = [Lump { ofs: 0, len: 0 }; NUM_LUMPS];
        for i in 0..NUM_LUMPS {
            let base = 4 + i * 8;
            let ofs = i32le(bytes, base).unwrap();
            let len = i32le(bytes, base + 4).unwrap();
            if ofs < 0 || len < 0 {
                return Err(format!("lump {} has negative offset/length", i));
            }
            let (ofs, len) = (ofs as usize, len as usize);
            if ofs.checked_add(len).map_or(true, |end| end > bytes.len()) {
                return Err(format!(
                    "lump {} (ofs {} len {}) runs past end of file ({})",
                    i,
                    ofs,
                    len,
                    bytes.len()
                ));
            }
            lumps[i] = Lump { ofs, len };
        }
        Ok(Bsp { bytes, lumps })
    }

    fn lump(&self, i: usize) -> &[u8] {
        let l = self.lumps[i];
        &self.bytes[l.ofs..l.ofs + l.len]
    }
}

struct TexStats {
    count: usize,
    embedded: usize,
    external: usize,
    texels: u64, // sum of width*height at mip0
    largest: Vec<(String, u32, u32)>,
}

fn read_name(b: &[u8], o: usize) -> String {
    let mut s = String::new();
    for k in 0..16 {
        match b.get(o + k) {
            Some(&0) | None => break,
            Some(&c) => s.push(c as char),
        }
    }
    s
}

#[derive(Clone, Copy)]
struct WadEntry {
    file: usize,
    ofs: usize,
    len: usize,
}

struct WadIndex {
    files: Vec<Vec<u8>>,
    entries: HashMap<String, WadEntry>,
}

impl WadIndex {
    fn load_for_bsp(path: &str) -> WadIndex {
        let mut idx = WadIndex {
            files: Vec::new(),
            entries: HashMap::new(),
        };
        let Some(wad_dir) = Path::new(path).parent().and_then(|maps| maps.parent()) else {
            return idx;
        };
        let Ok(read_dir) = std::fs::read_dir(wad_dir) else {
            return idx;
        };
        let mut paths: Vec<_> = read_dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("wad"))
            })
            .collect();
        paths.sort();
        for path in paths {
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            if data.len() < 12 || &data[0..4] != b"WAD3" {
                continue;
            }
            let Some(n) = i32le(&data, 4).filter(|&n| n >= 0).map(|n| n as usize) else {
                continue;
            };
            let Some(dir_ofs) = i32le(&data, 8).filter(|&o| o >= 0).map(|o| o as usize) else {
                continue;
            };
            let file_idx = idx.files.len();
            for i in 0..n {
                let o = dir_ofs + i * 32;
                let Some(filepos) = i32le(&data, o).filter(|&p| p >= 0).map(|p| p as usize) else {
                    continue;
                };
                let Some(disksize) = i32le(&data, o + 4).filter(|&s| s >= 0).map(|s| s as usize)
                else {
                    continue;
                };
                let compression = *data.get(o + 13).unwrap_or(&1);
                let name = read_name(&data, o + 16).to_ascii_lowercase();
                if name.is_empty()
                    || compression != 0
                    || filepos
                        .checked_add(disksize)
                        .is_none_or(|end| end > data.len())
                {
                    continue;
                }
                idx.entries.entry(name).or_insert(WadEntry {
                    file: file_idx,
                    ofs: filepos,
                    len: disksize,
                });
            }
            idx.files.push(data);
        }
        idx
    }

    fn miptex(&self, name: &str) -> Option<&[u8]> {
        let e = *self.entries.get(&name.to_ascii_lowercase())?;
        self.files.get(e.file)?.get(e.ofs..e.ofs + e.len)
    }
}

fn texture_stats(bsp: &Bsp) -> TexStats {
    let l = bsp.lump(LUMP_TEXTURES);
    let mut s = TexStats {
        count: 0,
        embedded: 0,
        external: 0,
        texels: 0,
        largest: Vec::new(),
    };
    let nummip = match i32le(l, 0) {
        Some(n) if n >= 0 => n as usize,
        _ => return s,
    };
    s.count = nummip;
    let mut all: Vec<(String, u32, u32)> = Vec::new();
    for i in 0..nummip {
        let dofs = match i32le(l, 4 + i * 4) {
            Some(d) => d,
            None => break,
        };
        if dofs < 0 {
            // -1 placeholder: texture not present here at all.
            s.external += 1;
            continue;
        }
        let mo = dofs as usize;
        let name = read_name(l, mo);
        let w = u32le(l, mo + 16).unwrap_or(0);
        let h = u32le(l, mo + 20).unwrap_or(0);
        // offsets[0] == 0 => pixels are NOT in the BSP (live in an external WAD).
        let embedded = u32le(l, mo + 24).unwrap_or(0) != 0;
        if embedded {
            s.embedded += 1;
        } else {
            s.external += 1;
        }
        s.texels += (w as u64) * (h as u64);
        all.push((name, w, h));
    }
    all.sort_by_key(|t| Reverse((t.1 as u64) * (t.2 as u64)));
    s.largest = all.into_iter().take(8).collect();
    s
}

fn report(path: &str, bsp: &Bsp) {
    let cnt = |lump: usize, sz: usize| bsp.lump(lump).len() / sz;
    let verts = cnt(LUMP_VERTEXES, SZ_VERTEX);
    let faces = cnt(LUMP_FACES, SZ_FACE);
    let edges = cnt(LUMP_EDGES, SZ_EDGE);
    let surfedges = cnt(LUMP_SURFEDGES, SZ_SURFEDGE);
    let texinfo = cnt(LUMP_TEXINFO, SZ_TEXINFO);
    let nodes = cnt(LUMP_NODES, SZ_NODE);
    let leaves = cnt(LUMP_LEAVES, SZ_LEAF);
    let models = cnt(LUMP_MODELS, SZ_MODEL);
    let marksurf = cnt(LUMP_MARKSURFACES, SZ_MARKSURFACE);
    let lighting = bsp.lump(LUMP_LIGHTING).len();
    let vis = bsp.lump(LUMP_VISIBILITY).len();
    let ents = bsp.lump(LUMP_ENTITIES);
    let ent_count = ents.iter().filter(|&&c| c == b'{').count();
    let ts = texture_stats(bsp);

    println!("== {} ==", path);
    println!("BSP v30 (GoldSrc) | {} KB on disk", bsp.bytes.len() / 1024);

    println!("\n[geometry]");
    println!("  vertices     {}", verts);
    println!("  faces        {}", faces);
    println!("  edges        {}  surfedges {}", edges, surfedges);
    println!(
        "  nodes        {}  leaves {}  models {}",
        nodes, leaves, models
    );
    println!("  marksurfaces {}  texinfo {}", marksurf, texinfo);
    let m = bsp.lump(LUMP_MODELS);
    if m.len() >= SZ_MODEL {
        let g = |o: usize| f32le(m, o).unwrap_or(0.0);
        println!(
            "  world bbox   [{:.0},{:.0},{:.0}]..[{:.0},{:.0},{:.0}]  ({:.0} x {:.0} x {:.0} units)",
            g(0), g(4), g(8), g(12), g(16), g(20),
            g(12) - g(0), g(16) - g(4), g(20) - g(8)
        );
    }

    println!("\n[textures]");
    println!("  unique       {}", ts.count);
    println!("  embedded     {}  (pixels in the BSP)", ts.embedded);
    println!("  WAD-external {}  (pixels in valve/*.wad)", ts.external);
    println!("  texels(mip0) {}", ts.texels);
    if !ts.largest.is_empty() {
        println!("  largest:");
        for (n, w, h) in &ts.largest {
            println!("    {:>4}x{:<4} {}", w, h, n);
        }
    }

    println!("\n[lightmaps / vis]  (PS1 drops lightmaps; vis drives leaf culling)");
    println!("  lighting     {} KB", lighting / 1024);
    println!("  visibility   {} KB", vis / 1024);
    println!(
        "  entities     {} ({} KB text)",
        ent_count,
        ents.len() / 1024
    );

    // PS1 texture VRAM region: X=320..1023 (704 px) × 512 lines × 2 bytes ≈ 704
    // KB, shared with CLUTs. Most HL textures are WAD-external, so this BSP-only
    // figure is a floor, not the whole texture set.
    println!("\n[PS1 texture budget]  (~704 KB VRAM for textures+CLUTs)");
    println!("  all unique @ 8bpp : {} KB", ts.texels / 1024);
    println!("  all unique @ 4bpp : {} KB", ts.texels / 2 / 1024);
    println!("  note: WAD-external pixels aren't in the BSP; resolve valve/*.wad");
    println!("        for the full set. PVS streaming keeps only nearby leaves'");
    println!("        textures resident, so this is the all-at-once worst case.");
}

// ---- Cook: BSP -> .hlm (PS1-native textured + lit triangle mesh) ----------
//
// Layout (all little-endian):
//   magic "HLMD" | u32 n_verts | u32 n_tris | u32 n_texs
//   verts:   i16 x,y,z   × n_verts          (world space, Y-up)
//   tri_rec[16] × n_tris:
//     u16 a,b,c | u8 uv[6] | u8 tex | u8 light_idx[3]
//   light palette: u16 rgb555 × 256   (per-corner lightmap, indexed above)
//   (pad to 4)
//   textures × n_texs, each (already 4-byte aligned):
//     u16 w | u16 h        (power-of-two, 8..=64)
//     u16 clut[16]         (BGR555 | 0x8000 opaque)
//     u8  pix[w*h/2]       (4-bit indices, 2 texels/byte, low nibble first)
//
// Each miptex is downscaled to a power-of-two <=64 and its palette crushed to
// 16 colours (median cut), so the whole campaign fits PS1 VRAM at 4bpp. UVs
// come from the BSP texinfo planes, scaled to the cooked texture size.

/// Largest power-of-two <= min(orig, MAX_TEX), clamped to [8, MAX_TEX].
const MAX_TEX: u32 = 64;
fn final_size(o: u32) -> u32 {
    let cap = o.min(MAX_TEX);
    let mut s = 8u32;
    while s * 2 <= cap {
        s *= 2;
    }
    s.clamp(8, MAX_TEX)
}

/// PS1 BGR555 with bit 15 set, so opaque black (0x0000 = the transparent
/// texel) is never produced.
fn to_bgr555(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) | ((g as u16 >> 3) << 5) | ((b as u16 >> 3) << 10)) | 0x8000
}

struct CookedTex {
    w: u16,
    h: u16,
    clut: [u16; 16],
    pix4: Vec<u8>,
}

struct RgbImage {
    w: usize,
    h: usize,
    pixels: Vec<(u8, u8, u8)>,
}

const SKY_TEX_SIZE: usize = 128;
const SKY_FACE_SUFFIXES: [&str; 6] = ["ft", "rt", "bk", "lf", "up", "dn"];

fn placeholder_tex() -> CookedTex {
    let mut clut = [0u16; 16];
    clut[0] = to_bgr555(110, 110, 110);
    CookedTex {
        w: 8,
        h: 8,
        clut,
        pix4: vec![0u8; 8 * 8 / 2],
    }
}

/// Compact the cooked texture set to the ids actually referenced by triangles
/// (plus `extra_keep` -- animation chain frames that no face references
/// directly but the +N/+a texture cycler swaps in at runtime). Returns the
/// compact set, the number of stripped source textures, and the old->new remap
/// (u16::MAX = dropped) so chain records can be rewritten in compact ids.
fn compact_used_textures(
    texs: Vec<CookedTex>,
    tri_tex: &mut [u16],
    extra_keep: &[u16],
) -> (Vec<CookedTex>, usize, Vec<u16>) {
    let original_count = texs.len();
    let mut source: Vec<Option<CookedTex>> = texs.into_iter().map(Some).collect();
    let mut remap = vec![u16::MAX; source.len()];
    let mut compact = Vec::new();
    let mut fallback_slot = u16::MAX;

    for slot in tri_tex {
        let old = *slot as usize;
        if old < remap.len() {
            let new = if remap[old] == u16::MAX {
                let next = compact.len().min(u16::MAX as usize) as u16;
                remap[old] = next;
                compact.push(source[old].take().unwrap_or_else(placeholder_tex));
                next
            } else {
                remap[old]
            };
            *slot = new;
        } else {
            // Malformed texinfo should not reach here, but keep the cooked file
            // internally valid if it does: all bad refs share one placeholder.
            if fallback_slot == u16::MAX {
                fallback_slot = compact.len().min(u16::MAX as usize) as u16;
                compact.push(placeholder_tex());
            }
            *slot = fallback_slot;
        }
    }

    for &keep in extra_keep {
        let old = keep as usize;
        if old < remap.len() && remap[old] == u16::MAX {
            remap[old] = compact.len().min(u16::MAX as usize) as u16;
            compact.push(source[old].take().unwrap_or_else(placeholder_tex));
        }
    }

    let stripped = original_count.saturating_sub(compact.len());
    (compact, stripped, remap)
}

/// One GoldSrc texture-animation group: miptexes named `+0name`..`+9name`
/// cycle as the primary chain at 10 Hz; `+aname`..`+jname` are the alternate
/// chain an entity swaps to when its frame state is toggled (R_TextureAnimation
/// semantics: frame != 0 shows the OTHER chain of the face's own texture).
/// Ids are BSP miptex indices at collection, remapped to compact ids for the
/// cooked file.
#[derive(Debug, PartialEq)]
struct TexAnimChain {
    primary: Vec<u16>,
    alt: Vec<u16>,
}

/// Texture compaction + animation-chain cooking in one auditable step. A
/// chain is live when any of its frames is referenced by a face; the whole
/// chain then survives compaction so the 10 Hz cycler can swap in frames no
/// face references directly (e.g. c1a0d's +0generic_113, reachable only from
/// the retinal scanner's +ageneric_113 face). Faces keep their AUTHORED
/// texture: `tri_tex` entries are remapped in place, never redirected to
/// another chain frame; the chain records come back rewritten in compact ids.
fn compact_textures_with_anim_chains(
    texs: Vec<CookedTex>,
    tri_tex: &mut [u16],
    tex_names: &[String],
) -> (Vec<CookedTex>, usize, Vec<TexAnimChain>) {
    let raw_chains = collect_tex_anim_chains(tex_names);
    let mut tex_referenced = vec![false; texs.len()];
    for &t in tri_tex.iter() {
        if (t as usize) < tex_referenced.len() {
            tex_referenced[t as usize] = true;
        }
    }
    let mut chain_keep: Vec<u16> = Vec::new();
    let live_chains: Vec<&TexAnimChain> = raw_chains
        .iter()
        .filter(|chain| {
            let live = chain
                .primary
                .iter()
                .chain(chain.alt.iter())
                .any(|&id| tex_referenced[id as usize]);
            if live {
                chain_keep.extend(chain.primary.iter().chain(chain.alt.iter()));
            }
            live
        })
        .collect();
    let (compact, stripped, tex_remap) = compact_used_textures(texs, tri_tex, &chain_keep);
    // Chains rewritten in compact ids. Every live member was force-kept above,
    // so a missed remap is cook corruption -- fail loudly rather than truncate
    // u16::MAX into a wild one-byte id in the chain section.
    let remap_member = |id: u16| -> u16 {
        let mapped = tex_remap[id as usize];
        assert!(
            mapped != u16::MAX,
            "animation chain frame {} ({}) was dropped by texture compaction",
            id,
            tex_names
                .get(id as usize)
                .map(String::as_str)
                .unwrap_or("?")
        );
        mapped
    };
    let chains = live_chains
        .iter()
        .map(|chain| TexAnimChain {
            primary: chain.primary.iter().map(|&id| remap_member(id)).collect(),
            alt: chain.alt.iter().map(|&id| remap_member(id)).collect(),
        })
        .collect();
    (compact, stripped, chains)
}

/// Group `+`-prefixed miptex names into animation chains. Frame chars are
/// '0'..'9' (primary) and 'a'..'j' (alternate, either case); grouping on the
/// remaining name is case-insensitive like the rest of GoldSrc's texture
/// lookups. Chains with a gap in their frame sequence are dropped with a
/// warning (GoldSrc errors on them), as are chains with nothing to cycle or
/// swap (a lone frame and no counterpart chain).
fn collect_tex_anim_chains(tex_names: &[String]) -> Vec<TexAnimChain> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, (Vec<(u8, u16)>, Vec<(u8, u16)>)> = BTreeMap::new();
    for (id, name) in tex_names.iter().enumerate() {
        let mut chars = name.chars();
        if chars.next() != Some('+') {
            continue;
        }
        let Some(frame_char) = chars.next() else {
            continue;
        };
        let rest: String = chars.as_str().to_ascii_lowercase();
        if rest.is_empty() || id > u16::MAX as usize {
            continue;
        }
        let entry = groups.entry(rest).or_default();
        match frame_char {
            '0'..='9' => entry.0.push((frame_char as u8 - b'0', id as u16)),
            'a'..='j' => entry.1.push((frame_char as u8 - b'a', id as u16)),
            'A'..='J' => entry.1.push((frame_char as u8 - b'A', id as u16)),
            _ => continue,
        }
    }
    let mut chains = Vec::new();
    for (name, (mut primary, mut alt)) in groups {
        primary.sort();
        alt.sort();
        let contiguous = |frames: &[(u8, u16)]| {
            frames
                .iter()
                .enumerate()
                .all(|(expect, &(frame, _))| frame as usize == expect)
        };
        if !contiguous(&primary) || !contiguous(&alt) {
            eprintln!("warning: texture chain +{name} has missing frames; not animated");
            continue;
        }
        if primary.len() + alt.len() < 2 {
            continue; // a lone frame has nothing to cycle or swap to
        }
        chains.push(TexAnimChain {
            primary: primary.into_iter().map(|(_, id)| id).collect(),
            alt: alt.into_iter().map(|(_, id)| id).collect(),
        });
    }
    chains
}

fn append_texture_blob(out: &mut Vec<u8>, texs: &[CookedTex]) {
    for tx in texs {
        out.extend_from_slice(&tx.w.to_le_bytes());
        out.extend_from_slice(&tx.h.to_le_bytes());
        for c in &tx.clut {
            out.extend_from_slice(&c.to_le_bytes());
        }
        out.extend_from_slice(&tx.pix4);
    }
}

fn build_texture_chunk(texs: &[CookedTex]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"HLTX");
    out.extend_from_slice(&(texs.len() as u32).to_le_bytes());
    append_texture_blob(&mut out, texs);
    out
}

fn entity_text(ents: &[u8]) -> Cow<'_, str> {
    let end = ents.iter().position(|&b| b == 0).unwrap_or(ents.len());
    String::from_utf8_lossy(&ents[..end])
}

fn worldspawn_skyname(ents: &[u8]) -> Option<String> {
    let s = entity_text(ents);
    for block in s.split('{') {
        if ent_value(block, "classname") == Some("worldspawn") {
            return ent_value(block, "skyname")
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| v.to_ascii_lowercase());
        }
    }
    None
}

fn sky_env_dir_for_bsp(path: &str) -> Option<std::path::PathBuf> {
    let maps = Path::new(path).parent()?;
    let valve = maps.parent()?;
    Some(valve.join("gfx").join("env"))
}

fn read_sky_image(env_dir: &Path, sky: &str, suffix: &str) -> Option<RgbImage> {
    let stem = format!("{}{}", sky, suffix);
    for ext in ["tga", "bmp"] {
        let path = env_dir.join(format!("{}.{}", stem, ext));
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let img = match ext {
            "tga" => read_tga_rgb(&data),
            "bmp" => read_bmp_rgb(&data),
            _ => None,
        };
        if let Some(img) = img {
            return Some(img);
        }
    }
    None
}

fn read_tga_rgb(data: &[u8]) -> Option<RgbImage> {
    if data.len() < 18 {
        return None;
    }
    let id_len = data[0] as usize;
    let cmap_type = data[1];
    let image_type = data[2];
    if cmap_type != 0 || image_type != 2 {
        return None;
    }
    let w = u16le(data, 12)? as usize;
    let h = u16le(data, 14)? as usize;
    let bpp = data[16];
    if w == 0 || h == 0 || (bpp != 24 && bpp != 32) {
        return None;
    }
    let bytes_pp = (bpp / 8) as usize;
    let src_off = 18usize.checked_add(id_len)?;
    if src_off + w.checked_mul(h)?.checked_mul(bytes_pp)? > data.len() {
        return None;
    }
    let top_origin = (data[17] & 0x20) != 0;
    let mut pixels = vec![(0u8, 0u8, 0u8); w * h];
    for y in 0..h {
        let sy = if top_origin { y } else { h - 1 - y };
        for x in 0..w {
            let o = src_off + (sy * w + x) * bytes_pp;
            pixels[y * w + x] = (data[o + 2], data[o + 1], data[o]);
        }
    }
    Some(RgbImage { w, h, pixels })
}

fn read_bmp_rgb(data: &[u8]) -> Option<RgbImage> {
    if data.len() < 54 || data.get(0..2)? != b"BM" {
        return None;
    }
    let pix_off = u32le(data, 10)? as usize;
    let dib = u32le(data, 14)? as usize;
    if dib < 40 {
        return None;
    }
    let w_raw = i32le(data, 18)?;
    let h_raw = i32le(data, 22)?;
    let planes = u16le(data, 26)?;
    let bpp = u16le(data, 28)?;
    let compression = u32le(data, 30)?;
    if planes != 1 || compression != 0 || w_raw == 0 || h_raw == 0 {
        return None;
    }
    let w = w_raw.unsigned_abs() as usize;
    let h = h_raw.unsigned_abs() as usize;
    if w == 0 || h == 0 || pix_off >= data.len() {
        return None;
    }
    let top_down = h_raw < 0;
    let row_bits = w.checked_mul(bpp as usize)?;
    let stride = row_bits.div_ceil(32).checked_mul(4)?;
    let mut pixels = vec![(0u8, 0u8, 0u8); w * h];
    match bpp {
        8 => {
            let palette_off = 14 + dib;
            if palette_off > pix_off {
                return None;
            }
            let pal_count = ((pix_off - palette_off) / 4).min(256);
            if pal_count == 0 {
                return None;
            }
            for y in 0..h {
                let sy = if top_down { y } else { h - 1 - y };
                let row = pix_off + sy * stride;
                if row + w > data.len() {
                    return None;
                }
                for x in 0..w {
                    let idx = data[row + x] as usize;
                    let po = palette_off + idx.min(pal_count - 1) * 4;
                    pixels[y * w + x] = (data[po + 2], data[po + 1], data[po]);
                }
            }
        }
        24 | 32 => {
            let bytes_pp = (bpp / 8) as usize;
            for y in 0..h {
                let sy = if top_down { y } else { h - 1 - y };
                let row = pix_off + sy * stride;
                if row + w * bytes_pp > data.len() {
                    return None;
                }
                for x in 0..w {
                    let o = row + x * bytes_pp;
                    pixels[y * w + x] = (data[o + 2], data[o + 1], data[o]);
                }
            }
        }
        _ => return None,
    }
    Some(RgbImage { w, h, pixels })
}

fn cook_rgb_texture(img: &RgbImage) -> CookedTex {
    let mut colors: Vec<(u8, u8, u8)> = Vec::with_capacity(SKY_TEX_SIZE * SKY_TEX_SIZE);
    for y in 0..SKY_TEX_SIZE {
        let sy = y * img.h / SKY_TEX_SIZE;
        for x in 0..SKY_TEX_SIZE {
            let sx = x * img.w / SKY_TEX_SIZE;
            colors.push(img.pixels[sy * img.w + sx]);
        }
    }
    let pal16 = median_cut16(&colors);
    let mut clut = [0u16; 16];
    for (i, c) in pal16.iter().enumerate() {
        clut[i] = to_bgr555(c.0, c.1, c.2);
    }
    let mut pix4 = vec![0u8; SKY_TEX_SIZE * SKY_TEX_SIZE / 2];
    for (i, chunk) in colors.chunks(2).enumerate() {
        let lo = nearest16(&pal16, chunk[0]);
        let hi = chunk.get(1).map(|c| nearest16(&pal16, *c)).unwrap_or(0);
        pix4[i] = lo | (hi << 4);
    }
    CookedTex {
        w: SKY_TEX_SIZE as u16,
        h: SKY_TEX_SIZE as u16,
        clut,
        pix4,
    }
}

fn load_skybox_textures(path: &str, sky: &str) -> Option<Vec<CookedTex>> {
    let env_dir = sky_env_dir_for_bsp(path)?;
    let mut out = Vec::with_capacity(SKY_FACE_SUFFIXES.len());
    for suffix in SKY_FACE_SUFFIXES {
        let img = read_sky_image(&env_dir, sky, suffix)?;
        out.push(cook_rgb_texture(&img));
    }
    Some(out)
}

/// Reachability-compact AND hash-cons the clipnode array: identical subtrees
/// (same source plane, same canonical children) collapse into one node, so the
/// output is a DAG. The runtime trace walks child indices without caring about
/// sharing, so this is a pure size win (~16% of clip bytes fleet-wide -- the
/// three world hulls repeat a lot of structure). Returns the old->new remap
/// (-1 = unreachable) plus the deduped node list as (src_planenum, c0, c1)
/// with children already in final id space.
fn compact_clipnode_remap(clipnodes: &[u8], roots: &[i32]) -> (Vec<i32>, Vec<(usize, i16, i16)>) {
    let n_clip = clipnodes.len() / SZ_CLIPNODE;
    let node = |idx: usize| -> (usize, i16, i16) {
        let co = idx * SZ_CLIPNODE;
        (
            i32le(clipnodes, co).unwrap_or(0).max(0) as usize,
            i16::from_le_bytes([clipnodes[co + 4], clipnodes[co + 5]]),
            i16::from_le_bytes([clipnodes[co + 6], clipnodes[co + 7]]),
        )
    };

    let mut remap = vec![-1i32; n_clip];
    let mut interned: std::collections::HashMap<(usize, i16, i16), i32> =
        std::collections::HashMap::new();
    let mut out: Vec<(usize, i16, i16)> = Vec::new();

    // Iterative post-order from every root: children canonicalized first, then
    // the node interns on (plane, canonical c0, canonical c1).
    let mut stack: Vec<usize> = Vec::new();
    for &root in roots {
        if root >= 0 && (root as usize) < n_clip {
            stack.push(root as usize);
        }
    }
    while let Some(&idx) = stack.last() {
        if remap[idx] >= 0 {
            stack.pop();
            continue;
        }
        let (plane, c0, c1) = node(idx);
        let mut ready = true;
        for child in [c0, c1] {
            if child >= 0 && (child as usize) < n_clip && remap[child as usize] < 0 {
                stack.push(child as usize);
                ready = false;
            }
        }
        if !ready {
            continue;
        }
        stack.pop();
        let canon = |child: i16| -> i16 {
            if child >= 0 && (child as usize) < n_clip {
                remap[child as usize] as i16
            } else {
                child.min(-1) // out-of-range refs degrade to -1 (empty), as before
            }
        };
        let key = (plane, canon(c0), canon(c1));
        let id = *interned.entry(key).or_insert_with(|| {
            out.push(key);
            (out.len() - 1) as i32
        });
        remap[idx] = id;
    }
    (remap, out)
}

fn remap_clip_head(head: i32, remap: &[i32]) -> i32 {
    if head >= 0 {
        remap.get(head as usize).copied().unwrap_or(-1)
    } else {
        head
    }
}

fn pack_rgb555(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) & 31) | (((g as u16 >> 3) & 31) << 5) | (((b as u16 >> 3) & 31) << 10)
}

fn plane_rec(planes: &[u8], planenum: usize, scale: f32) -> ([i16; 3], i32) {
    let po = planenum * SZ_PLANE;
    let nx = f32le(planes, po).unwrap_or(0.0);
    let ny = f32le(planes, po + 4).unwrap_or(0.0);
    let nz = f32le(planes, po + 8).unwrap_or(0.0);
    let d = f32le(planes, po + 12).unwrap_or(0.0);
    (
        [
            (nx * 4096.0).round() as i16,
            (nz * 4096.0).round() as i16,
            (ny * 4096.0).round() as i16,
        ],
        (d / scale).round() as i32,
    )
}

/// Runtime collision/BSP plane with a Q14 normal and Q5 distance.
///
/// The record remains the same 10 bytes (`i16[3] + i32`). Q14 uses two bits
/// that Q12 left idle in a unit normal, reducing plane-side error at large map
/// coordinates without adding a runtime load or multiply. Keep every Q14
/// component inside the truncation bin of the old Q12 value: collision impact
/// normals can then recover the exact established Q12 response with a rare
/// sign-aware shift, while point/segment classification gets the extra bits.
fn plane_normal_q14_compatible(value: f32) -> i16 {
    let q12 = (value * 4096.0).round() as i32;
    let exact_q14 = (value * 16384.0).round() as i32;
    let base = q12 * 4;
    let (lo, hi) = if q12 < 0 {
        (base - 3, base)
    } else {
        (base, base + 3)
    };
    exact_q14.clamp(lo, hi) as i16
}

fn plane_rec_q5(planes: &[u8], planenum: usize, scale: f32) -> ([i16; 3], i32) {
    let po = planenum * SZ_PLANE;
    let nx = f32le(planes, po).unwrap_or(0.0);
    let ny = f32le(planes, po + 4).unwrap_or(0.0);
    let nz = f32le(planes, po + 8).unwrap_or(0.0);
    let d = f32le(planes, po + 12).unwrap_or(0.0);
    (
        [
            plane_normal_q14_compatible(nx),
            plane_normal_q14_compatible(nz),
            plane_normal_q14_compatible(ny),
        ],
        (d / scale * 32.0).round() as i32,
    )
}

// HLMB/C/D clip PlaneRef: bits 13..0 are the remapped plane-table index; bits
// 15..14 classify exact positive axial normals after plane_rec quantization:
// 00=generic, 01=+X, 10=+Y, 11=+Z. Negative axes stay generic because their
// sign cannot be represented by the two-bit fast-path tag.
const CLIP_PLANE_INDEX_MASK: u16 = 0x3fff;
const CLIP_PLANE_TAG_X: u16 = 0x4000;
const CLIP_PLANE_TAG_Y: u16 = 0x8000;
const CLIP_PLANE_TAG_Z: u16 = 0xc000;

fn pack_clip_plane_ref(planenum: usize, normal: [i16; 3]) -> Result<u16, String> {
    if planenum > CLIP_PLANE_INDEX_MASK as usize {
        return Err(format!(
            "remapped clip plane index {} exceeds packed 14-bit limit of {}",
            planenum, CLIP_PLANE_INDEX_MASK
        ));
    }
    let tag = match normal {
        [4096, 0, 0] => CLIP_PLANE_TAG_X,
        [0, 4096, 0] => CLIP_PLANE_TAG_Y,
        [0, 0, 4096] => CLIP_PLANE_TAG_Z,
        _ => 0,
    };
    Ok(tag | planenum as u16)
}

fn signed_plane_ref(planenum: usize, side: u16) -> i16 {
    let idx = planenum.min(i16::MAX as usize) as i16;
    if side == 0 {
        idx
    } else {
        -idx - 1
    }
}

fn remap_plane_index(src: usize, remap: &mut [u16], cooked: &mut Vec<usize>) -> u16 {
    if src >= remap.len() {
        return 0;
    }
    let old = remap[src];
    if old != u16::MAX {
        return old;
    }
    let id = cooked.len().min(u16::MAX as usize) as u16;
    remap[src] = id;
    cooked.push(src);
    id
}

/// Read a miptex's 256-colour palette (RGB triples) from the BSP.
fn read_palette(l: &[u8], mo: usize, w: usize, h: usize) -> Option<[(u8, u8, u8); 256]> {
    let off3 = u32le(l, mo + 36)? as usize;
    let pal = mo + off3 + (w >> 3) * (h >> 3) + 2; // after mip3 + 2-byte count
    let mut p = [(0u8, 0u8, 0u8); 256];
    for (i, e) in p.iter_mut().enumerate() {
        *e = (
            *l.get(pal + i * 3)?,
            *l.get(pal + i * 3 + 1)?,
            *l.get(pal + i * 3 + 2)?,
        );
    }
    Some(p)
}

/// Cook one embedded miptex. Returns the cooked texture + its original
/// (width, height) so UVs (in original texels) can scale to the cooked size.
fn cook_miptex(l: &[u8], mo: usize, wads: &WadIndex) -> (CookedTex, (u32, u32)) {
    let name = miptex_name(l, mo);
    let (mut src, mut src_mo) = (l, mo);
    let (w0, h0) = match (u32le(src, src_mo + 16), u32le(src, src_mo + 20)) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as usize, h as usize),
        _ => return (placeholder_tex(), (64, 64)),
    };
    let mut off0 = u32le(src, src_mo + 24).unwrap_or(0) as usize;
    let (w0, h0) = if off0 == 0 {
        match wads.miptex(&name) {
            Some(wad_miptex) => {
                src = wad_miptex;
                src_mo = 0;
                let dims = match (u32le(src, 16), u32le(src, 20)) {
                    (Some(w), Some(h)) if w > 0 && h > 0 => (w as usize, h as usize),
                    _ => return (placeholder_tex(), (w0 as u32, h0 as u32)),
                };
                off0 = u32le(src, 24).unwrap_or(0) as usize;
                dims
            }
            None => return (placeholder_tex(), (w0 as u32, h0 as u32)),
        }
    } else {
        (w0, h0)
    };
    let pal = match read_palette(src, src_mo, w0, h0) {
        Some(p) if off0 != 0 => p,
        _ => return (placeholder_tex(), (w0 as u32, h0 as u32)),
    };
    let fw = final_size(w0 as u32) as usize;
    let fh = final_size(h0 as u32) as usize;
    let px = src_mo + off0;
    // "{..." textures are masked: source palette index 255 is transparent.
    let masked = src.get(src_mo) == Some(&b'{');
    // Nearest-neighbour downscale, keeping the source palette index per texel.
    let mut idxv: Vec<u8> = Vec::with_capacity(fw * fh);
    for y in 0..fh {
        for x in 0..fw {
            idxv.push(
                *src.get(px + (y * h0 / fh) * w0 + (x * w0 / fw))
                    .unwrap_or(&0),
            );
        }
    }
    let mut clut = [0u16; 16];
    let mut pix4 = vec![0u8; fw * fh / 2];
    if masked {
        // Slot 0 = 0x0000 (the PS1 GPU skips it); 15 opaque colours in 1..=15.
        let opaque: Vec<(u8, u8, u8)> = idxv
            .iter()
            .filter(|&&i| i != 255)
            .map(|&i| pal[i as usize])
            .collect();
        let pal15 = median_cut16(&opaque);
        let n = pal15.len().min(15);
        for i in 0..n {
            clut[i + 1] = to_bgr555(pal15[i].0, pal15[i].1, pal15[i].2);
        }
        let map = |i: u8| -> u8 {
            if i == 255 || n == 0 {
                0
            } else {
                nearest16(&pal15[..n], pal[i as usize]) + 1
            }
        };
        for (i, chunk) in idxv.chunks(2).enumerate() {
            pix4[i] = map(chunk[0]) | (chunk.get(1).map(|&j| map(j)).unwrap_or(0) << 4);
        }
    } else {
        let colors: Vec<(u8, u8, u8)> = idxv.iter().map(|&i| pal[i as usize]).collect();
        let pal16 = median_cut16(&colors);
        // A texture that is entirely (near-)black (e.g. GoldSrc's `black`, used as
        // a backdrop wall) renders as a solid black region that reads as a missing
        // triangle on a CRT/emulator. Lift it to a faint dark grey so it looks like
        // a wall. Targeted at all-black textures only, so shadow detail in normal
        // textures (which keep their black texels) is untouched.
        let all_black = pal16.iter().all(|c| c.0 < 8 && c.1 < 8 && c.2 < 8);
        for (i, c) in pal16.iter().enumerate() {
            clut[i] = if all_black {
                to_bgr555(28, 28, 28)
            } else {
                to_bgr555(c.0, c.1, c.2)
            };
        }
        for (i, chunk) in colors.chunks(2).enumerate() {
            let lo = nearest16(&pal16, chunk[0]);
            let hi = chunk.get(1).map(|c| nearest16(&pal16, *c)).unwrap_or(0);
            pix4[i] = lo | (hi << 4);
        }
    }
    (
        CookedTex {
            w: fw as u16,
            h: fh as u16,
            clut,
            pix4,
        },
        (w0 as u32, h0 as u32),
    )
}

fn nearest16(pal: &[(u8, u8, u8)], c: (u8, u8, u8)) -> u8 {
    let mut best = 0u8;
    let mut bd = i32::MAX;
    for (i, p) in pal.iter().enumerate() {
        let (dr, dg, db) = (
            c.0 as i32 - p.0 as i32,
            c.1 as i32 - p.1 as i32,
            c.2 as i32 - p.2 as i32,
        );
        let d = dr * dr + dg * dg + db * db;
        if d < bd {
            bd = d;
            best = i as u8;
        }
    }
    best
}

fn chan(c: (u8, u8, u8), ch: u8) -> u8 {
    match ch {
        0 => c.0,
        1 => c.1,
        _ => c.2,
    }
}

fn box_extent(b: &[(u8, u8, u8)]) -> (u8, i32) {
    let (mut mn, mut mx) = ((255u8, 255u8, 255u8), (0u8, 0u8, 0u8));
    for c in b {
        mn = (mn.0.min(c.0), mn.1.min(c.1), mn.2.min(c.2));
        mx = (mx.0.max(c.0), mx.1.max(c.1), mx.2.max(c.2));
    }
    let (rr, rg, rb) = (
        (mx.0 - mn.0) as i32,
        (mx.1 - mn.1) as i32,
        (mx.2 - mn.2) as i32,
    );
    let ch = if rr >= rg && rr >= rb {
        0
    } else if rg >= rb {
        1
    } else {
        2
    };
    (ch, rr.max(rg).max(rb))
}

/// Median-cut to <=16 representative colours.
fn median_cut16(colors: &[(u8, u8, u8)]) -> Vec<(u8, u8, u8)> {
    median_cut(colors, 16)
}

/// Append one FaceVert (u16 idx | u8 uv[2] | u8 light_idx) for the given global
/// triangle-corner index, reading from the flat cooked-triangle arrays.
fn push_facevert(
    dst: &mut Vec<u8>,
    tri_idx: &[u16],
    tri_uv: &[u8],
    light_idx: &[u8],
    corner: usize,
) {
    dst.extend_from_slice(&tri_idx[corner].to_le_bytes());
    dst.extend_from_slice(&tri_uv[corner * 2..corner * 2 + 2]);
    dst.push(light_idx[corner]);
}

fn push_patch_facevert(
    dst: &mut Vec<u8>,
    tri_idx: &[u16],
    tri_uv: &[u8],
    light_idx: &[u8],
    corner: usize,
    triangle_marker: bool,
    blocked_edge: bool,
) {
    debug_assert_eq!(tri_idx[corner] & !cooked::LOOPVERT_INDEX_MASK, 0);
    let encoded = tri_idx[corner]
        | if triangle_marker {
            cooked::LOOPVERT_TRIANGLE_FLAG
        } else {
            0
        }
        | if blocked_edge {
            cooked::LOOPVERT_BLOCKED_EDGE_FLAG
        } else {
            0
        };
    dst.extend_from_slice(&encoded.to_le_bytes());
    dst.extend_from_slice(&tri_uv[corner * 2..corner * 2 + 2]);
    dst.push(light_idx[corner]);
}

/// Store one native-patch GT3 record. Bit 15 on the repeated corner keeps the
/// existing triangle marker; bit 14 marks the first half of a geometry-matched
/// pair whose shared positions are valid even when UV/light attributes differ.
fn push_triangle_patch_record(
    dst: &mut Vec<u8>,
    tri_idx: &[u16],
    tri_uv: &[u8],
    light_idx: &[u8],
    triangle: usize,
    seamed_pair_start: bool,
) {
    push_triangle_patch_corners(
        dst,
        tri_idx,
        tri_uv,
        light_idx,
        [triangle * 3, triangle * 3 + 1, triangle * 3 + 2],
        seamed_pair_start,
    );
}

fn push_triangle_patch_corners(
    dst: &mut Vec<u8>,
    tri_idx: &[u16],
    tri_uv: &[u8],
    light_idx: &[u8],
    corners: [usize; 3],
    seamed_pair_start: bool,
) {
    for &corner in &corners {
        push_patch_facevert(dst, tri_idx, tri_uv, light_idx, corner, false, false);
    }
    push_patch_facevert(
        dst,
        tri_idx,
        tri_uv,
        light_idx,
        corners[2],
        true,
        seamed_pair_start,
    );
}

#[derive(Clone)]
struct WorldSourceFace {
    compact_face: u16,
    texture: u8,
    group: u16,
    center: [i16; 3],
    first_corner: u16,
    patch_count: u16,
    leaves: Vec<u16>,
    /// Owning static entity's serialized table index, or `u16::MAX` for the
    /// world. Entity faces never share a cell with world faces so the runtime
    /// can gate a whole cell on one ENT_ACTIVE test.
    ent: u16,
}

struct WorldCellCook {
    vertex_first: u16,
    vertex_count: u16,
    face_first: u16,
    face_count: u16,
    packet_count: u16,
    flags: u16,
    center: [i16; 3],
    radius: u16,
}

struct WorldCellVertexCook {
    position: [i16; 3],
    source_vertex: u16,
}

type WorldFaceCommandCook = CookedDrawSurfaceCommand;

#[derive(Default)]
struct WorldPipelineStats {
    source_faces: usize,
    source_patches: usize,
    cells: usize,
    vertices: usize,
    unique_source_vertices: usize,
    max_visible_cells: usize,
    max_visible_vertices: usize,
    max_visible_packets: usize,
    max_neighbor_union_packets: usize,
    max_exact_cells: usize,
    max_exact_groups: usize,
    max_exact_vertices: usize,
    max_exact_packets: usize,
    min_exact_fill_x100: usize,
}

struct WorldPipelineCook {
    cells: Vec<WorldCellCook>,
    vertices: Vec<WorldCellVertexCook>,
    faces: Vec<WorldFaceCommandCook>,
    cell_leaves: Vec<Vec<u16>>,
    stats: WorldPipelineStats,
}

struct PendingWorldCell {
    group: u16,
    /// Owning static entity (serialized table index) or `u16::MAX` for the
    /// world. Entity cells never mix with world faces.
    ent: u16,
    leaves: Vec<u16>,
    source_vertices: Vec<u16>,
    commands: Vec<WorldFaceCommandCook>,
}

const WORLD_CELL_MAX_EXTENT: i32 = 2048;

fn world_spatial_key(center: [i16; 3]) -> u32 {
    let quantized = center.map(|component| ((component as i32 + 32768) as u32) >> 6);
    let mut key = 0u32;
    for bit in 0..10 {
        for axis in 0..3 {
            key |= ((quantized[axis] >> bit) & 1) << (bit * 3 + axis);
        }
    }
    key
}

impl PendingWorldCell {
    fn new(group: u16, leaves: Vec<u16>) -> Self {
        Self {
            group,
            ent: u16::MAX,
            leaves,
            source_vertices: Vec::new(),
            commands: Vec::new(),
        }
    }

    fn additional_vertices(&self, corners: [u16; 4]) -> usize {
        let mut added = 0usize;
        for (i, vertex) in corners.into_iter().enumerate() {
            if self.source_vertices.contains(&vertex) {
                continue;
            }
            if corners[..i].contains(&vertex) {
                continue;
            }
            added += 1;
        }
        added
    }

    fn spatially_fits(&self, corners: [u16; 4], positions: &[[i16; 3]], max_extent: i32) -> bool {
        let mut mins = [i32::MAX; 3];
        let mut maxs = [i32::MIN; 3];
        for source in self.source_vertices.iter().copied().chain(corners) {
            let position = positions[source as usize];
            for axis in 0..3 {
                mins[axis] = mins[axis].min(position[axis] as i32);
                maxs[axis] = maxs[axis].max(position[axis] as i32);
            }
        }
        (0..3).all(|axis| maxs[axis] - mins[axis] <= max_extent)
    }

    fn local_indices(&mut self, corners: [u16; 4]) -> [u8; 4] {
        corners.map(|vertex| {
            if let Some(index) = self.source_vertices.iter().position(|&v| v == vertex) {
                index as u8
            } else {
                let index = self.source_vertices.len();
                self.source_vertices.push(vertex);
                index as u8
            }
        })
    }

    fn merge_leaves(&mut self, leaves: &[u16]) {
        for &leaf in leaves {
            if !self.leaves.contains(&leaf) {
                self.leaves.push(leaf);
            }
        }
        self.leaves.sort_unstable();
    }
}

impl WorldPipelineCook {
    fn new() -> Self {
        Self {
            cells: Vec::new(),
            vertices: Vec::new(),
            faces: Vec::new(),
            cell_leaves: Vec::new(),
            stats: WorldPipelineStats::default(),
        }
    }

    fn finish_cell(&mut self, pending: PendingWorldCell, positions: &[[i16; 3]]) {
        if pending.commands.is_empty() {
            return;
        }
        let vertex_first = self.vertices.len();
        let face_first = self.faces.len();
        let mut mins = [i16::MAX; 3];
        let mut maxs = [i16::MIN; 3];
        for &source_vertex in &pending.source_vertices {
            let position = positions[source_vertex as usize];
            for axis in 0..3 {
                mins[axis] = mins[axis].min(position[axis]);
                maxs[axis] = maxs[axis].max(position[axis]);
            }
        }
        let center = [
            ((mins[0] as i32 + maxs[0] as i32) / 2) as i16,
            ((mins[1] as i32 + maxs[1] as i32) / 2) as i16,
            ((mins[2] as i32 + maxs[2] as i32) / 2) as i16,
        ];
        let ex = maxs[0] as i32 - center[0] as i32;
        let ey = maxs[1] as i32 - center[1] as i32;
        let ez = maxs[2] as i32 - center[2] as i32;
        let radius = ((ex * ex + ey * ey + ez * ez) as f32)
            .sqrt()
            .ceil()
            .min(u16::MAX as f32) as u16;
        let vertex_count = pending.source_vertices.len();
        let face_count = pending.commands.len();
        for source_vertex in pending.source_vertices.iter().copied() {
            self.vertices.push(WorldCellVertexCook {
                position: positions[source_vertex as usize],
                source_vertex,
            });
        }
        self.faces.extend(pending.commands);
        self.cells.push(WorldCellCook {
            vertex_first: vertex_first as u16,
            vertex_count: vertex_count as u16,
            face_first: face_first as u16,
            face_count: face_count as u16,
            packet_count: face_count as u16,
            // Ordering remains deliberately unproven in Phase A. The runtime
            // cannot select these cells until the cell-overlap audit sets the
            // separate ORDER_PROVEN bit. Bits 8..15 carry the owning static
            // entity's table index + 1 (0 = world): the runtime gates a whole
            // entity cell on one ENT_ACTIVE test at plan-compile time.
            flags: cooked::WORLD_CELL_STATIC_OPAQUE
                | if pending.ent != u16::MAX {
                    (pending.ent + 1) << 8
                } else {
                    0
                },
            center,
            radius,
        });
        self.cell_leaves.push(pending.leaves);
    }

    fn validate_limits(&self, path: &str) -> Result<(), String> {
        for (name, count) in [
            ("cells", self.cells.len()),
            ("cell vertices", self.vertices.len()),
            ("face commands", self.faces.len()),
        ] {
            if count > u16::MAX as usize {
                return Err(format!(
                    "{}: world pipeline {} count {} exceeds u16",
                    path, name, count
                ));
            }
        }
        Ok(())
    }
}

fn build_world_pipeline(
    path: &str,
    positions: &[[i16; 3]],
    loopverts: &[u8],
    source_faces: &[WorldSourceFace],
    all_face_leaves: &[Vec<u16>],
    leaves: &[u8],
    vis: &[u8],
    n_visleaves: usize,
) -> Result<WorldPipelineCook, String> {
    let mut pipeline = WorldPipelineCook::new();
    // Storage cells are also transform units. Their positions are contiguous
    // and commands use byte-sized local indices, so the rendered frame never
    // gathers through a room-global vertex table.
    let mut faces = source_faces.iter().collect::<Vec<_>>();
    // World cells keep Morton locality; static-entity faces cluster after
    // them, one entity at a time, so a cell is never shared across owners.
    faces.sort_by_key(|source| {
        (
            if source.ent == u16::MAX {
                0u32
            } else {
                source.ent as u32 + 1
            },
            world_spatial_key(source.center),
        )
    });
    let mut pending = PendingWorldCell::new(u16::MAX, Vec::new());
    for source in faces {
        for patch in 0..source.patch_count as usize {
            let source_corner = source.first_corner as usize + patch * 4;
            let mut corners = [0u16; 4];
            let mut triangle = false;
            let mut blocked = 0u8;
            for corner in 0..4 {
                let o = (source_corner + corner) * 5;
                let encoded = u16::from_le_bytes([loopverts[o], loopverts[o + 1]]);
                triangle |= encoded & 0x8000 != 0;
                if encoded & 0x4000 != 0 {
                    blocked |= [1, 2, 8, 4][corner];
                }
                corners[corner] = encoded & 0x3fff;
            }
            // Three-corner records and UV/light seam pairs remain on the
            // established path. Phase A only describes exact GT4 patches.
            if triangle {
                continue;
            }
            let additional = pending.additional_vertices(corners);
            if !pending.commands.is_empty()
                && (pending.commands.len() >= cooked::WORLD_PIPELINE_MAX_CELL_FACES
                    || pending.source_vertices.len() + additional
                        > cooked::WORLD_PIPELINE_MAX_CELL_VERTICES
                    || pending.ent != source.ent
                    || !pending.spatially_fits(corners, positions, WORLD_CELL_MAX_EXTENT))
            {
                pipeline.finish_cell(pending, positions);
                pending = PendingWorldCell::new(source.group, source.leaves.clone());
            } else if pending.commands.is_empty() {
                pending.group = source.group;
            } else if pending.group != source.group {
                pending.group = u16::MAX;
            }
            if pending.commands.is_empty() {
                pending.ent = source.ent;
            }
            pending.merge_leaves(&source.leaves);
            let vertex = pending.local_indices(corners);
            let mut light = [0u8; 4];
            let mut uv = [0u16; 4];
            for corner in 0..4 {
                let o = (source_corner + corner) * 5;
                uv[corner] = u16::from_le_bytes([loopverts[o + 2], loopverts[o + 3]]);
                light[corner] = loopverts[o + 4];
            }
            pending.commands.push(WorldFaceCommandCook {
                vertex,
                light,
                uv,
                material: source.texture,
                flags: cooked::WORLD_FACE_QUAD,
                blocked_edges: blocked,
                subdivision_policy: 0,
                source_patch: source_corner as u16,
                source_face: source.compact_face,
            });
        }
    }
    pipeline.finish_cell(pending, positions);
    pipeline.validate_limits(path)?;

    let words = pipeline.cells.len().div_ceil(64);
    let mut visible = vec![vec![0u64; words]; n_visleaves + 1];
    for from in 1..=n_visleaves {
        for (cell, owners) in pipeline.cell_leaves.iter().enumerate() {
            if owners.iter().any(|&to| {
                let to = to as usize;
                from == to || leaf_row_sees(leaves, vis, n_visleaves, from, to)
            }) {
                visible[from][cell >> 6] |= 1u64 << (cell & 63);
            }
        }
    }
    let count_set = |set: &[u64]| -> (usize, usize, usize) {
        let mut cells = 0usize;
        let mut vertices = 0usize;
        let mut packets = 0usize;
        for (word_index, &word) in set.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                let cell = word_index * 64 + bit;
                if let Some(rec) = pipeline.cells.get(cell) {
                    cells += 1;
                    vertices += rec.vertex_count as usize;
                    packets += rec.packet_count as usize;
                }
                bits &= bits - 1;
            }
        }
        (cells, vertices, packets)
    };
    for set in visible.iter().skip(1) {
        let (cells, vertices, packets) = count_set(set);
        pipeline.stats.max_visible_cells = pipeline.stats.max_visible_cells.max(cells);
        pipeline.stats.max_visible_vertices = pipeline.stats.max_visible_vertices.max(vertices);
        pipeline.stats.max_visible_packets = pipeline.stats.max_visible_packets.max(packets);
    }

    let mut neighbors = std::collections::BTreeSet::<(u16, u16)>::new();
    for owners in all_face_leaves {
        for (i, &a) in owners.iter().enumerate() {
            for &b in owners.iter().skip(i + 1) {
                if a != 0 && b != 0 && (a as usize) <= n_visleaves && (b as usize) <= n_visleaves {
                    neighbors.insert(if a < b { (a, b) } else { (b, a) });
                }
            }
        }
    }
    let mut union = vec![0u64; words];
    for (a, b) in neighbors {
        for i in 0..words {
            union[i] = visible[a as usize][i] | visible[b as usize][i];
        }
        pipeline.stats.max_neighbor_union_packets = pipeline
            .stats
            .max_neighbor_union_packets
            .max(count_set(&union).2);
    }
    pipeline.stats.max_neighbor_union_packets = pipeline
        .stats
        .max_neighbor_union_packets
        .max(pipeline.stats.max_visible_packets);

    // Storage clusters are not visibility units. Audit the shipping contract:
    // exact BSP face marks select commands, then each active cluster projects
    // its bounded contiguous local transform range.
    let max_compact_face = source_faces
        .iter()
        .map(|source| source.compact_face as usize)
        .max()
        .unwrap_or(0);
    let mut face_owners = vec![&[][..]; max_compact_face + 1];
    let mut face_groups = vec![u16::MAX; max_compact_face + 1];
    for source in source_faces {
        face_owners[source.compact_face as usize] = &source.leaves;
        face_groups[source.compact_face as usize] = source.group;
    }
    pipeline.stats.min_exact_fill_x100 = usize::MAX;
    for from in 1..=n_visleaves {
        let mut exact_cells = 0usize;
        let mut exact_packets = 0usize;
        let mut exact_vertices = 0usize;
        let mut exact_groups = std::collections::BTreeSet::new();
        for cell in &pipeline.cells {
            let mut selected = 0usize;
            let first = cell.face_first as usize;
            let end = first + cell.face_count as usize;
            for command in &pipeline.faces[first..end] {
                let owners = face_owners
                    .get(command.source_face as usize)
                    .copied()
                    .unwrap_or(&[]);
                let visible = owners.iter().any(|&to| {
                    let to = to as usize;
                    from == to || leaf_row_sees(leaves, vis, n_visleaves, from, to)
                });
                if !visible {
                    continue;
                }
                selected += 1;
                if let Some(&group) = face_groups.get(command.source_face as usize) {
                    exact_groups.insert(group);
                }
            }
            if selected != 0 {
                exact_cells += 1;
                exact_packets += selected;
                exact_vertices += cell.vertex_count as usize;
            }
        }
        pipeline.stats.max_exact_cells = pipeline.stats.max_exact_cells.max(exact_cells);
        pipeline.stats.max_exact_groups = pipeline.stats.max_exact_groups.max(exact_groups.len());
        pipeline.stats.max_exact_vertices = pipeline.stats.max_exact_vertices.max(exact_vertices);
        pipeline.stats.max_exact_packets = pipeline.stats.max_exact_packets.max(exact_packets);
        if exact_packets >= 256 && exact_cells != 0 {
            pipeline.stats.min_exact_fill_x100 = pipeline
                .stats
                .min_exact_fill_x100
                .min(exact_packets * 100 / exact_cells);
        }
    }
    if pipeline.stats.min_exact_fill_x100 == usize::MAX {
        pipeline.stats.min_exact_fill_x100 = 0;
    }
    pipeline.stats.source_faces = source_faces.len();
    pipeline.stats.source_patches = pipeline.faces.len();
    pipeline.stats.cells = pipeline.cells.len();
    pipeline.stats.vertices = pipeline.vertices.len();
    let mut unique = std::collections::BTreeSet::new();
    for vertex in &pipeline.vertices {
        unique.insert(vertex.source_vertex);
    }
    pipeline.stats.unique_source_vertices = unique.len();
    Ok(pipeline)
}

/// Apply the same pair code decoded by game/src/map.rs and return absolute
/// triangle-corner indices in the runtime GT4 layout `[a,b,c,d]`.
fn encoded_quad_pair_corners(t0: usize, t1: usize, code: u8) -> [usize; 4] {
    let (first, second) = if code & 0x10 == 0 { (t0, t1) } else { (t1, t0) };
    let r0 = ((code >> 2) & 3) as usize;
    let r1 = (code & 3) as usize;
    [
        first * 3 + (r0 + 1) % 3,
        first * 3 + r0,
        first * 3 + (r0 + 2) % 3,
        second * 3 + (r1 + 1) % 3,
    ]
}

/// Prove that a cooked face still has the exact fan topology and packed corner
/// continuity required by the 5-byte loop representation. Triangle count
/// alone is insufficient: edge welding/refinement can replace a two-triangle
/// face with a different diagonal while retaining the same count, and a UV or
/// light seam can give one shared index two distinct packed corners. Rebuilding
/// either case as `[anchor, boundary...]` invents a triangle at runtime.
fn face_is_exact_packed_fan(
    first: usize,
    count: usize,
    tri_idx: &[u16],
    tri_tex: &[u16],
    tri_uv: &[u8],
    light_idx: &[u8],
) -> bool {
    if count == 0
        || first + count > tri_tex.len()
        || (first + count) * 3 > tri_idx.len()
        || (first + count) * 6 > tri_uv.len()
        || (first + count) * 3 > light_idx.len()
    {
        return false;
    }
    let same_corner = |a: usize, b: usize| {
        tri_idx[a] == tri_idx[b]
            && tri_uv[a * 2..a * 2 + 2] == tri_uv[b * 2..b * 2 + 2]
            && light_idx[a] == light_idx[b]
    };
    let anchor = first * 3;
    let texture = tri_tex[first];
    for t in first + 1..first + count {
        let corner = t * 3;
        let previous = (t - 1) * 3;
        if tri_tex[t] != texture
            || !same_corner(corner, anchor)
            || !same_corner(corner + 2, previous + 1)
        {
            return false;
        }
    }
    // A convex BSP face visits every perimeter vertex exactly once. UV split
    // support triangles can preserve the fan's corner-to-corner continuity
    // and triangle count while walking back through the anchor internally.
    // Compacting that sequence as a polygon loop creates degenerate/invented
    // children (the c1a0 model-*32 door face decoded with its anchor repeated
    // three times). Keep such topology as the original raw triangles.
    let loop_corner = |i: usize| -> usize {
        match i {
            0 => first * 3,
            1 => first * 3 + 2,
            _ => (first + i - 2) * 3 + 1,
        }
    };
    for i in 0..count + 2 {
        for j in 0..i {
            if tri_idx[loop_corner(i)] == tri_idx[loop_corner(j)] {
                return false;
            }
        }
    }
    true
}

/// Encode the swap and two rotations that map a pair to the runtime's GT4
/// layout `(b,a,c) + (a,d,c)`. Position, UV and palettised light must agree
/// across the shared diagonal, exactly matching the console requirements.
fn quad_pair_code(
    t0: usize,
    t1: usize,
    tri_idx: &[u16],
    tri_tex: &[u16],
    tri_uv: &[u8],
    light_idx: &[u8],
    light_pal: &[(u8, u8, u8)],
) -> Option<u8> {
    if tri_tex[t0] != tri_tex[t1] {
        return None;
    }
    let same_corner = |a: usize, b: usize| {
        tri_idx[a] == tri_idx[b]
            && tri_uv[a * 2..a * 2 + 2] == tri_uv[b * 2..b * 2 + 2]
            // The console pairs resolved RGB corners, not palette indices.
            // Median cut may produce duplicate colours, so two different
            // indices can still be the exact same packed corner at runtime.
            && light_pal[light_idx[a] as usize] == light_pal[light_idx[b] as usize]
    };
    for (swapped, (first, second)) in [(t0, t1), (t1, t0)].into_iter().enumerate() {
        for rotation in 0..3 {
            let b = first * 3 + rotation;
            let a = first * 3 + (rotation + 1) % 3;
            let c = first * 3 + (rotation + 2) % 3;
            if tri_idx[a] == tri_idx[b] || tri_idx[a] == tri_idx[c] || tri_idx[b] == tri_idx[c] {
                continue;
            }
            for second_rotation in 0..3 {
                let second_a = second * 3 + second_rotation;
                let d = second * 3 + (second_rotation + 1) % 3;
                let second_c = second * 3 + (second_rotation + 2) % 3;
                if same_corner(a, second_a)
                    && same_corner(c, second_c)
                    && tri_idx[d] != tri_idx[a]
                    && tri_idx[d] != tri_idx[b]
                    && tri_idx[d] != tri_idx[c]
                {
                    // Six bits fit exactly in the unused high two bits of the
                    // first triangle's three 14-bit vertex indices.
                    return Some(
                        0x20 | ((swapped as u8) << 4)
                            | ((rotation as u8) << 2)
                            | second_rotation as u8,
                    );
                }
            }
        }
    }
    None
}

/// Find the same four-sided positional topology as `quad_pair_code` without
/// requiring UV/light agreement on the shared diagonal. These pairs cannot be
/// collapsed to one GT4, but runtime can subdivide their two GT3s as a unit
/// while preserving each triangle's independently authored attributes.
fn geometry_quad_pair_code(t0: usize, t1: usize, tri_idx: &[u16], tri_tex: &[u16]) -> Option<u8> {
    if tri_tex[t0] != tri_tex[t1] {
        return None;
    }
    for (swapped, (first, second)) in [(t0, t1), (t1, t0)].into_iter().enumerate() {
        for rotation in 0..3 {
            let b = first * 3 + rotation;
            let a = first * 3 + (rotation + 1) % 3;
            let c = first * 3 + (rotation + 2) % 3;
            if tri_idx[a] == tri_idx[b] || tri_idx[a] == tri_idx[c] || tri_idx[b] == tri_idx[c] {
                continue;
            }
            for second_rotation in 0..3 {
                let second_a = second * 3 + second_rotation;
                let d = second * 3 + (second_rotation + 1) % 3;
                let second_c = second * 3 + (second_rotation + 2) % 3;
                if tri_idx[a] == tri_idx[second_a]
                    && tri_idx[c] == tri_idx[second_c]
                    && tri_idx[d] != tri_idx[a]
                    && tri_idx[d] != tri_idx[b]
                    && tri_idx[d] != tri_idx[c]
                {
                    return Some(
                        0x20 | ((swapped as u8) << 4)
                            | ((rotation as u8) << 2)
                            | second_rotation as u8,
                    );
                }
            }
        }
    }
    None
}

/// Nearest palette entry (squared-distance) for one colour.
fn nearest_pal_index(pal: &[(u8, u8, u8)], c: (u8, u8, u8)) -> u8 {
    let mut best = 0usize;
    let mut bd = i32::MAX;
    for (i, p) in pal.iter().enumerate() {
        let dr = c.0 as i32 - p.0 as i32;
        let dg = c.1 as i32 - p.1 as i32;
        let db = c.2 as i32 - p.2 as i32;
        let d = dr * dr + dg * dg + db * db;
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best as u8
}

fn median_cut(colors: &[(u8, u8, u8)], n_target: usize) -> Vec<(u8, u8, u8)> {
    if colors.is_empty() {
        return vec![(110, 110, 110)];
    }
    let mut boxes: Vec<Vec<(u8, u8, u8)>> = vec![colors.to_vec()];
    while boxes.len() < n_target {
        // Split the box with the widest channel range.
        let mut pick = None;
        let mut best = 0i32;
        for (i, b) in boxes.iter().enumerate() {
            if b.len() < 2 {
                continue;
            }
            let (_, r) = box_extent(b);
            if r > best {
                best = r;
                pick = Some(i);
            }
        }
        let Some(bi) = pick else { break };
        let mut b = boxes.swap_remove(bi);
        let (ch, _) = box_extent(&b);
        b.sort_by_key(|c| chan(*c, ch));
        let hi = b.split_off(b.len() / 2);
        boxes.push(b);
        boxes.push(hi);
    }
    boxes
        .iter()
        .map(|b| {
            let (mut r, mut g, mut bl) = (0u32, 0u32, 0u32);
            for c in b {
                r += c.0 as u32;
                g += c.1 as u32;
                bl += c.2 as u32;
            }
            let n = b.len().max(1) as u32;
            ((r / n) as u8, (g / n) as u8, (bl / n) as u8)
        })
        .collect()
}

/// Neutral modulation tint (PS1: 128 = 1.0x, texture unchanged).
const NEUTRAL: u8 = 128;

/// Sample the base-style lightmap at one vertex's luxel (luxels are 16 texels
/// apart in original texture space). Boosted ~1.5x so lit surfaces aren't dim
/// under 128=1.0x modulation. Returns neutral where there is no lightmap.
#[allow(clippy::too_many_arguments)]
fn vertex_shade(
    lighting: &[u8],
    lightofs: i32,
    style0: u8,
    lmw: usize,
    lmh: usize,
    ou: f32,
    ov: f32,
    mins_s: i32,
    mins_t: i32,
) -> (u8, u8, u8) {
    if lightofs < 0 || style0 == 0xFF {
        return (NEUTRAL, NEUTRAL, NEUTRAL);
    }
    let ls = (((ou / 16.0).floor() as i32) - mins_s).clamp(0, lmw as i32 - 1) as usize;
    let lt = (((ov / 16.0).floor() as i32) - mins_t).clamp(0, lmh as i32 - 1) as usize;
    let o = lightofs as usize + (lt * lmw + ls) * 3;
    match (lighting.get(o), lighting.get(o + 1), lighting.get(o + 2)) {
        (Some(&r), Some(&g), Some(&b)) => (light_curve(r), light_curve(g), light_curve(b)),
        _ => (NEUTRAL, NEUTRAL, NEUTRAL),
    }
}

/// Sample one qrad lightstyle plane without the base plane. GoldSrc combines
/// these RGB samples independently at runtime (`R_BuildLightMap`); keeping the
/// contribution separate is what lets an OFF style leave the texture and the
/// ordinary baked lighting completely unchanged.
#[allow(clippy::too_many_arguments)]
fn dynamic_vertex_shade(
    lighting: &[u8],
    lightofs: i32,
    layer: usize,
    lmw: usize,
    lmh: usize,
    ou: f32,
    ov: f32,
    mins_s: i32,
    mins_t: i32,
) -> (u8, u8, u8) {
    if lightofs < 0 || layer == 0 {
        return (0, 0, 0);
    }
    let ls = (((ou / 16.0).floor() as i32) - mins_s).clamp(0, lmw as i32 - 1) as usize;
    let lt = (((ov / 16.0).floor() as i32) - mins_t).clamp(0, lmh as i32 - 1) as usize;
    let plane_bytes = lmw * lmh * 3;
    let o = lightofs as usize + layer * plane_bytes + (lt * lmw + ls) * 3;
    match (lighting.get(o), lighting.get(o + 1), lighting.get(o + 2)) {
        (Some(&r), Some(&g), Some(&b)) => (light_curve(r), light_curve(g), light_curve(b)),
        _ => (0, 0, 0),
    }
}

/// Tone curve applied to every baked lightmap sample.
///
/// This replaces a flat `v * 3 / 2` gain with a hard clamp, which was wrong at
/// both ends of the range and in the middle. Shadows were lifted by the same
/// factor as everything else, so a luxel of 20 reached only 30 and stayed under
/// the 15-bit floor -- measured on the capture gallery, c1a4d-silo put 81% of
/// its frame below luminance 16 and c1a3-canals 51%, both of them maps whose
/// worldspawn does NOT set startdark. At the other end the gain saturated: a
/// luxel of 200 clamped to 255, and because each channel clamped independently
/// a warm (220,180,140) light became (255,255,210), losing its hue. c1a0-walk
/// blew 6% of its frame to pure white.
///
/// A gamma curve fixes all three: it lifts darks far more than mids, it reaches
/// 255 only when the source does, and being monotonic per channel it cannot
/// invent a hue shift. GoldSrc's own ramp lives in the engine, which is not in
/// the released SDK, so this exponent is not a claim about matching Valve's
/// exact constant -- it is tuned against measured capture histograms.
const LIGHT_GAMMA: f32 = 1.8;

fn light_curve(v: u8) -> u8 {
    let n = v as f32 / 255.0;
    (n.powf(1.0 / LIGHT_GAMMA) * 255.0 + 0.5).min(255.0) as u8
}

fn best_fan_anchor(uv: &[(f32, f32)]) -> usize {
    let n = uv.len();
    if n <= 3 {
        return 0;
    }

    let mut best = 0usize;
    let mut best_max = f32::MAX;
    let mut best_sum = f32::MAX;
    for anchor in 0..n {
        let mut max_span = 0.0f32;
        let mut sum_span = 0.0f32;
        for k in 1..n - 1 {
            let a = uv[anchor];
            let b = uv[(anchor + k + 1) % n];
            let c = uv[(anchor + k) % n];
            let min_u = a.0.min(b.0).min(c.0);
            let max_u = a.0.max(b.0).max(c.0);
            let min_v = a.1.min(b.1).min(c.1);
            let max_v = a.1.max(b.1).max(c.1);
            let du = max_u - min_u;
            let dv = max_v - min_v;
            let span = du * du + dv * dv;
            max_span = max_span.max(span);
            sum_span += span;
        }
        if max_span < best_max || (max_span == best_max && sum_span < best_sum) {
            best = anchor;
            best_max = max_span;
            best_sum = sum_span;
        }
    }
    best
}

const MAX_COOK_VERTS: usize = 12288;
// Only pre-split a shared BSP edge when its UVs cross the PS1's 8-bit wrap.
// View-dependent affine correction belongs to the runtime quad-patch path;
// cutting every 96 texels here permanently taxes RAM, ordering and fill rate.
const UV_SPLIT_SPAN: f32 = 256.0;
const UV_SPLIT_DEPTH: u8 = 2;
// Masked brush-entity faces (ladders, fences, rails pressed against walls)
// never reach the world-only quad-patch refinement, so their tall quads keep
// full affine warp. Split their edges tighter; the segment cap stays at
// 1 << UV_SPLIT_DEPTH because edge_split_verts holds three midpoints. 128
// (not 64): tall faces hit the 4-segment cap either way, and the gentler
// span keeps the fleet-wide vert growth inside the RAM ceiling.
const UV_SPLIT_SPAN_CUTOUT_ENT: f32 = 128.0;
// Cook only the UV-aligned cells required by the PS1's 8-bit texture
// coordinates. Both constants deliberately equal one wrap period: visual
// affine correction is now a bounded runtime operation on native quad patches,
// not a permanent quality grid baked into every view.
// A coarse quality floor for large faces inside one UV wrap. The shipping
// classic runtime quad path adds view-dependent refinement on top; this cook
// threshold is not evidence that runtime subdivision is disabled.
const AFFINE_QUALITY_MIN_WORLD_SPAN: u32 = 256;
const AFFINE_QUALITY_MIN_UV_SPAN: f32 = 64.0;
// MUST be smaller than the admitted face's UV span: at a step >= the span,
// emit_uv_grid_poly returns one cell, `added == 0` rejects it, and admitting
// the face changes nothing. 64 overshoots the cook's 1,536 added-triangle
// invariant (c0a0 hit 2,472); 128 clears it on all 103 chunks.
const UV_GRID_QUALITY: f32 = 128.0;
const UV_GRID_COARSE: f32 = 256.0;
const UV_GRID_FINE: f32 = 256.0;
const UV_GRID_FINE_WORLD_SPAN: u32 = 384;
const UV_GRID_FINE_TEXEL_SPAN: f32 = 256.0;
// Legacy candidate tests remain useful as cooker unit fixtures even though
// production quality selection moved to projected runtime patches.
#[cfg(test)]
const AFFINE_CANDIDATE_MIN_RADIUS: u32 = 96;
#[cfg(test)]
const AFFINE_CANDIDATE_MIN_UV_SPAN: f32 = 32.0;
#[cfg(test)]
const AFFINE_EDGE_MIN_WORLD: i64 = 96;
#[cfg(test)]
const AFFINE_EDGE_MIN_UV: i32 = 24;

#[cfg(test)]
fn affine_subdivision_candidate(extent: [u16; 3], uv: &[(f32, f32)]) -> bool {
    if uv.len() < 3 {
        return false;
    }
    let radius2 = extent.iter().map(|&v| v as u64 * v as u64).sum::<u64>();
    if radius2 < (AFFINE_CANDIDATE_MIN_RADIUS as u64 * AFFINE_CANDIDATE_MIN_RADIUS as u64) {
        return false;
    }
    let (mut min_u, mut max_u) = (f32::MAX, f32::MIN);
    let (mut min_v, mut max_v) = (f32::MAX, f32::MIN);
    for &(u, v) in uv {
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }
    (max_u - min_u).max(max_v - min_v) >= AFFINE_CANDIDATE_MIN_UV_SPAN
}

fn face_planes_match_unoriented(
    a_normal: [i16; 3],
    a_dist: i32,
    b_normal: [i16; 3],
    b_dist: i32,
) -> bool {
    let same = a_dist == b_dist && a_normal == b_normal;
    let opposite =
        a_dist == -b_dist && (0..3).all(|axis| a_normal[axis] as i32 == -(b_normal[axis] as i32));
    same || opposite
}

#[derive(Clone, Copy)]
struct CookCorner {
    idx: u16,
    pos: [i16; 3],
    uv: (f32, f32),
    shade: (u8, u8, u8),
}

/// Remove duplicated and exactly collinear boundary points while preserving
/// the BSP's ordered winding. GoldSrc commonly leaves T-junction points in a
/// four-corner brush face; those are topology constraints, not extra polygon
/// corners. The runtime patch midpoint remains on the same projected line.
fn collapse_collinear_loop(
    poly: &[u16],
    verts: &[[i16; 3]],
    original_vertex_count: usize,
) -> Vec<u16> {
    let mut out = poly.to_vec();
    let mut changed = true;
    while changed && out.len() > 3 {
        changed = false;
        let mut i = 0usize;
        while i < out.len() && out.len() > 3 {
            let n = out.len();
            let ia = out[(i + n - 1) % n] as usize;
            let ib = out[i] as usize;
            let ic = out[(i + 1) % n] as usize;
            if ia >= verts.len() || ib >= verts.len() || ic >= verts.len() {
                i += 1;
                continue;
            }
            let (a, b, c) = (verts[ia], verts[ib], verts[ic]);
            let ab = [
                b[0] as i64 - a[0] as i64,
                b[1] as i64 - a[1] as i64,
                b[2] as i64 - a[2] as i64,
            ];
            let bc = [
                c[0] as i64 - b[0] as i64,
                c[1] as i64 - b[1] as i64,
                c[2] as i64 - b[2] as i64,
            ];
            let cross = [
                ab[1] * bc[2] - ab[2] * bc[1],
                ab[2] * bc[0] - ab[0] * bc[2],
                ab[0] * bc[1] - ab[1] * bc[0],
            ];
            let ac = [
                c[0] as i64 - a[0] as i64,
                c[1] as i64 - a[1] as i64,
                c[2] as i64 - a[2] as i64,
            ];
            let ab_dot_ac = ab[0] * ac[0] + ab[1] * ac[1] + ab[2] * ac[2];
            let ac_len2 = ac[0] * ac[0] + ac[1] * ac[1] + ac[2] * ac[2];
            let cross_len2 = cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2];
            // Edge-split vertices are generated in source float space and then
            // rounded to the PS1 integer grid. A point that was exactly on a
            // GoldSrc edge can consequently land one packed unit off that
            // edge; exact cross==0 then invents a fifth/sixth logical corner
            // and destroys the original quad identity. Only generated points
            // receive this one-unit tolerance. Authored BSP corners still
            // require exact collinearity, so real bevels cannot disappear.
            let rounded_generated_point = ib >= original_vertex_count
                && ac_len2 != 0
                && ab_dot_ac >= 0
                && ab_dot_ac <= ac_len2
                && cross_len2 <= ac_len2;
            if a == b || b == c || cross == [0, 0, 0] || rounded_generated_point {
                out.remove(i);
                changed = true;
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Return the logical quad edges that contained one or more authored boundary
/// points before collinear collapse. Those points are GoldSrc's ownership of a
/// T-junction (or a mandatory shared UV-wrap cut), not decorative polygon
/// corners. A runtime midpoint must never be introduced anywhere on such a
/// span unless every smaller neighbour segment is represented too.
fn collapsed_boundary_spans(poly: &[u16], logical: &[u16]) -> Vec<(u16, u16)> {
    if logical.len() != 4 || poly.len() <= logical.len() {
        return Vec::new();
    }
    let mut positions = [usize::MAX; 4];
    let mut search_from = 0usize;
    for (slot, &corner) in logical.iter().enumerate() {
        let Some(step) =
            (0..poly.len()).find(|&step| poly[(search_from + step) % poly.len()] == corner)
        else {
            return Vec::new();
        };
        positions[slot] = (search_from + step) % poly.len();
        search_from = (positions[slot] + 1) % poly.len();
    }
    let mut spans = Vec::new();
    for edge in 0..4 {
        let a = positions[edge];
        let b = positions[(edge + 1) & 3];
        let steps = (b + poly.len() - a) % poly.len();
        if steps > 1 {
            spans.push((logical[edge], logical[(edge + 1) & 3]));
        }
    }
    spans
}

fn point_on_segment_3d(p: [i16; 3], a: [i16; 3], b: [i16; 3]) -> bool {
    let ab = [
        b[0] as i64 - a[0] as i64,
        b[1] as i64 - a[1] as i64,
        b[2] as i64 - a[2] as i64,
    ];
    let ap = [
        p[0] as i64 - a[0] as i64,
        p[1] as i64 - a[1] as i64,
        p[2] as i64 - a[2] as i64,
    ];
    let cross = [
        ab[1] * ap[2] - ab[2] * ap[1],
        ab[2] * ap[0] - ab[0] * ap[2],
        ab[0] * ap[1] - ab[1] * ap[0],
    ];
    if cross != [0, 0, 0] {
        return false;
    }
    let dot = ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    dot >= 0 && dot <= len2
}

fn patch_blocked_edge_mask(
    corners: [usize; 4],
    tri_idx: &[u16],
    verts: &[[i16; 3]],
    blocked_spans: &[([i16; 3], [i16; 3])],
) -> u8 {
    // A stored quad is `[a,b,c,d]`, but the GPU packet is `[b,a,c,d]`, so its
    // real boundary is `b-a-d-c` and the stored-slot edge pairs are
    // (1,0), (0,3), (3,2), (2,1). The previous mapping used the pre-packet
    // slot order and accidentally treated the two diagonals as boundaries,
    // which both missed real T-junction edges and blocked unrelated patches.
    let edges = [(1usize, 0usize), (0, 3), (3, 2), (2, 1)];
    let mut mask = 0u8;
    for (edge, &(ca, cb)) in edges.iter().enumerate() {
        let a = verts[tri_idx[corners[ca]] as usize];
        let b = verts[tri_idx[corners[cb]] as usize];
        let owned_span = blocked_spans
            .iter()
            .any(|&(s0, s1)| point_on_segment_3d(a, s0, s1) && point_on_segment_3d(b, s0, s1));
        // The T-junction weld has already cut a logical edge at every retained
        // ownership point. A resulting sub-segment is safe to split and gets
        // its own canonical shared-edge ID. Block only a patch edge that still
        // spans across an interior retained vertex (for example when the weld
        // budget could not perform the mandatory cut).
        let crosses_retained_vertex = owned_span
            && tri_idx.iter().any(|&index| {
                let p = verts[index as usize];
                p != a && p != b && point_on_segment_3d(p, a, b)
            });
        if crosses_retained_vertex {
            mask |= 1 << edge;
        }
    }
    mask
}

struct PendingAffineFace {
    face: usize,
    risk: u64,
    priority: u64,
    proximity_boost: u64,
    detail_boost: u64,
    estimated_added_tris: usize,
    source: Vec<CookCorner>,
    tex_id: u16,
    grid_step: f32,
    fan_anchor: usize,
    source_tris: usize,
}

fn affine_proximity_boost(
    center: [i16; 3],
    extent: [u16; 3],
    traversal_samples: &[[i32; 3]],
) -> u64 {
    let nearest_distance2 = traversal_samples
        .iter()
        .map(|sample| {
            (0..3)
                .map(|axis| {
                    let outside = (sample[axis] - center[axis] as i32)
                        .unsigned_abs()
                        .saturating_sub(extent[axis] as u32)
                        as u64;
                    outside * outside
                })
                .sum::<u64>()
        })
        .min();
    match nearest_distance2 {
        Some(d2) if d2 <= 256 * 256 => 8,
        Some(d2) if d2 <= 512 * 512 => 6,
        Some(d2) if d2 <= 1024 * 1024 => 4,
        Some(d2) if d2 <= 2048 * 2048 => 2,
        _ => 1,
    }
}

fn affine_budget_priority(risk: u64, proximity_boost: u64, added_tris: usize) -> u64 {
    if added_tris == 0 {
        return 0;
    }
    risk.saturating_mul(proximity_boost) / added_tris as u64
}

fn affine_texture_detail_boost(name: &str) -> u64 {
    // Binary-alpha cutouts such as grates and ladders expose affine error much
    // more harshly than opaque artwork: moving the sample by a single texel
    // can replace a bar with transparent index 0 and make a whole wedge look
    // like missing geometry. Keep the fixed room budget unchanged, but let
    // these surfaces reach the 64-texel UV grid before lower-risk opaque faces.
    if name.trim_start().starts_with('{') {
        16
    } else {
        1
    }
}

#[derive(Clone, Default)]
struct GridEmitStats {
    cells: usize,
    quad_cells: usize,
    boundary_tris: usize,
    // Triangle offsets, relative to this face's first emitted triangle, whose
    // following triangle is the other half of the same regular UV cell. This
    // is the only cook-time ownership relation allowed to become one GT4.
    pair_first: Vec<usize>,
}

fn grid_cross_len2(verts: &[[i16; 3]], tri: [CookCorner; 3]) -> u64 {
    let a = verts[tri[0].idx as usize];
    let b = verts[tri[1].idx as usize];
    let c = verts[tri[2].idx as usize];
    let ab = [
        b[0] as i64 - a[0] as i64,
        b[1] as i64 - a[1] as i64,
        b[2] as i64 - a[2] as i64,
    ];
    let ac = [
        c[0] as i64 - a[0] as i64,
        c[1] as i64 - a[1] as i64,
        c[2] as i64 - a[2] as i64,
    ];
    let cross = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]) as u64
}

fn grid_corner_at_uv_axis(
    a: CookCorner,
    b: CookCorner,
    axis: usize,
    target: f32,
    verts: &mut Vec<[i16; 3]>,
    position_cache: &mut HashMap<[i16; 3], u16>,
) -> Option<CookCorner> {
    let av = uv_axis(&a, axis);
    let bv = uv_axis(&b, axis);
    let den = bv - av;
    if den.abs() < 0.000_01 {
        return None;
    }
    let t = ((target - av) / den).clamp(0.0, 1.0);
    if t <= 0.000_01 {
        return Some(set_uv_axis(a, axis, target));
    }
    if t >= 0.999_99 {
        return Some(set_uv_axis(b, axis, target));
    }
    let lerp_i16 = |x: i16, y: i16| (x as f32 + (y as f32 - x as f32) * t).round() as i16;
    let lerp_u8 = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    let pos = [
        lerp_i16(a.pos[0], b.pos[0]),
        lerp_i16(a.pos[1], b.pos[1]),
        lerp_i16(a.pos[2], b.pos[2]),
    ];
    let idx = if let Some(&idx) = position_cache.get(&pos) {
        idx
    } else {
        if verts.len() >= MAX_COOK_VERTS {
            return None;
        }
        let idx = verts.len() as u16;
        verts.push(pos);
        position_cache.insert(pos, idx);
        idx
    };
    Some(CookCorner {
        idx,
        pos,
        uv: (
            a.uv.0 + (b.uv.0 - a.uv.0) * t,
            a.uv.1 + (b.uv.1 - a.uv.1) * t,
        ),
        shade: (
            lerp_u8(a.shade.0, b.shade.0),
            lerp_u8(a.shade.1, b.shade.1),
            lerp_u8(a.shade.2, b.shade.2),
        ),
    })
}

fn clip_grid_half_plane(
    input: &[CookCorner],
    axis: usize,
    boundary: f32,
    keep_high: bool,
    verts: &mut Vec<[i16; 3]>,
    position_cache: &mut HashMap<[i16; 3], u16>,
) -> Option<Vec<CookCorner>> {
    if input.is_empty() {
        return Some(Vec::new());
    }
    let inside = |corner: CookCorner| {
        if keep_high {
            uv_axis(&corner, axis) >= boundary - 0.000_1
        } else {
            uv_axis(&corner, axis) <= boundary + 0.000_1
        }
    };
    let mut out = Vec::with_capacity(input.len() + 2);
    for i in 0..input.len() {
        let cur = input[i];
        let prev = input[(i + input.len() - 1) % input.len()];
        let cur_in = inside(cur);
        let prev_in = inside(prev);
        if cur_in != prev_in {
            out.push(grid_corner_at_uv_axis(
                prev,
                cur,
                axis,
                boundary,
                verts,
                position_cache,
            )?);
        }
        if cur_in {
            out.push(cur);
        }
    }
    // Clipping on a line already represented by a source corner can return
    // the same packed position twice. Remove only true consecutive duplicates;
    // non-consecutive boundary cuts must remain for shared-edge conformity.
    let mut clean: Vec<CookCorner> = Vec::with_capacity(out.len());
    for corner in out {
        if clean.last().is_none_or(|last| last.idx != corner.idx) {
            clean.push(corner);
        }
    }
    if clean.len() > 1 && clean[0].idx == clean[clean.len() - 1].idx {
        clean.pop();
    }
    Some(clean)
}

fn grid_uv_byte(value: f32, cell_low: f32, cell_high: f32) -> u8 {
    let high_wrap = cell_high.rem_euclid(256.0).abs() < 0.000_1;
    if high_wrap && (value - cell_high).abs() < 0.000_1 && cell_high > cell_low {
        255
    } else {
        uv_byte(value)
    }
}

#[allow(clippy::too_many_arguments)]
fn push_grid_tri(
    tri: [CookCorner; 3],
    tex_id: u16,
    cell_u: (f32, f32),
    cell_v: (f32, f32),
    verts: &[[i16; 3]],
    tri_idx: &mut Vec<u16>,
    tri_tex: &mut Vec<u16>,
    tri_uv: &mut Vec<u8>,
    tri_rgb: &mut Vec<u8>,
) -> bool {
    if grid_cross_len2(verts, tri) == 0 {
        return false;
    }
    tri_idx.extend_from_slice(&[tri[0].idx, tri[1].idx, tri[2].idx]);
    tri_tex.push(tex_id);
    for corner in tri {
        tri_uv.push(grid_uv_byte(corner.uv.0, cell_u.0, cell_u.1));
        tri_uv.push(grid_uv_byte(corner.uv.1, cell_v.0, cell_v.1));
        tri_rgb.extend_from_slice(&[corner.shade.0, corner.shade.1, corner.shade.2]);
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn emit_grid_cell(
    poly: &[CookCorner],
    tex_id: u16,
    cell_u: (f32, f32),
    cell_v: (f32, f32),
    verts: &mut Vec<[i16; 3]>,
    _position_cache: &mut HashMap<[i16; 3], u16>,
    tri_idx: &mut Vec<u16>,
    tri_tex: &mut Vec<u16>,
    tri_uv: &mut Vec<u8>,
    tri_rgb: &mut Vec<u8>,
) -> Option<(usize, usize)> {
    match poly.len() {
        0..=2 => Some((0, 0)),
        3 => push_grid_tri(
            [poly[0], poly[2], poly[1]],
            tex_id,
            cell_u,
            cell_v,
            verts,
            tri_idx,
            tri_tex,
            tri_uv,
            tri_rgb,
        )
        .then_some((1, 0)),
        4 => {
            let a = [poly[0], poly[2], poly[1]];
            let b = [poly[0], poly[3], poly[2]];
            let c = [poly[1], poly[3], poly[2]];
            let d = [poly[1], poly[0], poly[3]];
            let qa = grid_cross_len2(verts, a).min(grid_cross_len2(verts, b));
            let qb = grid_cross_len2(verts, c).min(grid_cross_len2(verts, d));
            let pair = if qb > qa { [c, d] } else { [a, b] };
            if pair.iter().any(|&tri| grid_cross_len2(verts, tri) == 0) {
                return None;
            }
            for tri in pair {
                push_grid_tri(
                    tri, tex_id, cell_u, cell_v, verts, tri_idx, tri_tex, tri_uv, tri_rgb,
                );
            }
            Some((2, 1))
        }
        _ => {
            // Keep every boundary constraint, but pair a fan rooted at an
            // existing corner instead of inventing a centre vertex and a
            // visible star of tiny triangles.
            let first_tri = tri_tex.len();
            for i in 1..poly.len() - 1 {
                let tri = [poly[0], poly[i + 1], poly[i]];
                if grid_cross_len2(verts, tri) == 0 {
                    return None;
                }
                push_grid_tri(
                    tri, tex_id, cell_u, cell_v, verts, tri_idx, tri_tex, tri_uv, tri_rgb,
                );
            }
            let emitted = tri_tex.len() - first_tri;
            Some((emitted, emitted / 2))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_uv_grid_poly(
    source: &[CookCorner],
    tex_id: u16,
    step: f32,
    verts: &mut Vec<[i16; 3]>,
    tri_idx: &mut Vec<u16>,
    tri_tex: &mut Vec<u16>,
    tri_uv: &mut Vec<u8>,
    tri_rgb: &mut Vec<u8>,
) -> Option<GridEmitStats> {
    if source.len() < 3 || step <= 0.0 || 256.0 % step != 0.0 {
        return None;
    }
    let mut position_cache = HashMap::new();
    for corner in source {
        position_cache.entry(corner.pos).or_insert(corner.idx);
    }
    let min_u = source.iter().map(|c| c.uv.0).fold(f32::MAX, f32::min);
    let max_u = source.iter().map(|c| c.uv.0).fold(f32::MIN, f32::max);
    let min_v = source.iter().map(|c| c.uv.1).fold(f32::MAX, f32::min);
    let max_v = source.iter().map(|c| c.uv.1).fold(f32::MIN, f32::max);
    let u0 = (min_u / step).floor() as i32;
    let u1 = (max_u / step).ceil() as i32;
    let v0 = (min_v / step).floor() as i32;
    let v1 = (max_v / step).ceil() as i32;
    let face_tri_first = tri_tex.len();
    let mut stats = GridEmitStats::default();
    for gy in v0..v1 {
        for gx in u0..u1 {
            let cell_u = (gx as f32 * step, (gx + 1) as f32 * step);
            let cell_v = (gy as f32 * step, (gy + 1) as f32 * step);
            let mut clipped = source.to_vec();
            clipped =
                clip_grid_half_plane(&clipped, 0, cell_u.0, true, verts, &mut position_cache)?;
            clipped =
                clip_grid_half_plane(&clipped, 0, cell_u.1, false, verts, &mut position_cache)?;
            clipped =
                clip_grid_half_plane(&clipped, 1, cell_v.0, true, verts, &mut position_cache)?;
            clipped =
                clip_grid_half_plane(&clipped, 1, cell_v.1, false, verts, &mut position_cache)?;
            if clipped.len() < 3 {
                continue;
            }
            let cell_tri_first = tri_tex.len();
            let (emitted, pairs) = emit_grid_cell(
                &clipped,
                tex_id,
                cell_u,
                cell_v,
                verts,
                &mut position_cache,
                tri_idx,
                tri_tex,
                tri_uv,
                tri_rgb,
            )?;
            if emitted != 0 {
                stats.cells += 1;
                stats.quad_cells += pairs;
                stats.boundary_tris += emitted - pairs * 2;
                for pair in 0..pairs {
                    stats
                        .pair_first
                        .push(cell_tri_first - face_tri_first + pair * 2);
                }
            }
        }
    }
    (stats.cells != 0).then_some(stats)
}

fn uv_byte(v: f32) -> u8 {
    (v.round() as i32).rem_euclid(256) as u8
}

fn uv_axis(c: &CookCorner, axis: usize) -> f32 {
    if axis == 0 {
        c.uv.0
    } else {
        c.uv.1
    }
}

fn set_uv_axis(mut c: CookCorner, axis: usize, value: f32) -> CookCorner {
    if axis == 0 {
        c.uv.0 = value;
    } else {
        c.uv.1 = value;
    }
    c
}

fn uv_seam(c: &[CookCorner; 3], axis: usize) -> Option<f32> {
    let min_v = uv_axis(&c[0], axis)
        .min(uv_axis(&c[1], axis))
        .min(uv_axis(&c[2], axis));
    let max_v = uv_axis(&c[0], axis)
        .max(uv_axis(&c[1], axis))
        .max(uv_axis(&c[2], axis));
    let seam = (min_v / 256.0).floor() * 256.0 + 256.0;
    if min_v < seam && max_v >= seam {
        Some(seam)
    } else {
        None
    }
}

fn lerp_corner_at_uv_axis(
    a: CookCorner,
    b: CookCorner,
    axis: usize,
    target: f32,
    verts: &mut Vec<[i16; 3]>,
) -> Option<CookCorner> {
    if verts.len() >= MAX_COOK_VERTS {
        return None;
    }
    let av = uv_axis(&a, axis);
    let bv = uv_axis(&b, axis);
    let den = bv - av;
    let t = if den.abs() < 0.0001 {
        0.0
    } else {
        ((target - av) / den).clamp(0.0, 1.0)
    };
    let lerp_i16 = |x: i16, y: i16| (x as f32 + (y as f32 - x as f32) * t).round() as i16;
    let lerp_u8 = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    let pos = [
        lerp_i16(a.pos[0], b.pos[0]),
        lerp_i16(a.pos[1], b.pos[1]),
        lerp_i16(a.pos[2], b.pos[2]),
    ];
    let idx = verts.len() as u16;
    verts.push(pos);
    Some(CookCorner {
        idx,
        pos,
        uv: (
            a.uv.0 + (b.uv.0 - a.uv.0) * t,
            a.uv.1 + (b.uv.1 - a.uv.1) * t,
        ),
        shade: (
            lerp_u8(a.shade.0, b.shade.0),
            lerp_u8(a.shade.1, b.shade.1),
            lerp_u8(a.shade.2, b.shade.2),
        ),
    })
}

fn clip_uv_side(
    input: &[CookCorner],
    axis: usize,
    seam: f32,
    keep_low: bool,
    boundary_uv: f32,
    verts: &mut Vec<[i16; 3]>,
) -> Option<Vec<CookCorner>> {
    let mut out = Vec::with_capacity(input.len() + 2);
    for i in 0..input.len() {
        let cur = input[i];
        let prev = input[(i + input.len() - 1) % input.len()];
        let cur_in = if keep_low {
            uv_axis(&cur, axis) < seam
        } else {
            uv_axis(&cur, axis) >= seam
        };
        let prev_in = if keep_low {
            uv_axis(&prev, axis) < seam
        } else {
            uv_axis(&prev, axis) >= seam
        };
        if cur_in != prev_in {
            let cut = lerp_corner_at_uv_axis(prev, cur, axis, seam, verts)?;
            out.push(set_uv_axis(cut, axis, boundary_uv));
        }
        if cur_in {
            out.push(cur);
        }
    }
    Some(out)
}

fn emit_cooked_poly(
    poly: &[CookCorner],
    tex_id: u16,
    depth: u8,
    verts: &mut Vec<[i16; 3]>,
    tri_idx: &mut Vec<u16>,
    tri_tex: &mut Vec<u16>,
    tri_uv: &mut Vec<u8>,
    tri_rgb: &mut Vec<u8>,
) {
    if poly.len() < 3 {
        return;
    }
    for i in 1..poly.len() - 1 {
        emit_cooked_tri(
            [poly[0], poly[i], poly[i + 1]],
            tex_id,
            depth,
            verts,
            tri_idx,
            tri_tex,
            tri_uv,
            tri_rgb,
        );
    }
}

fn emit_cooked_tri(
    c: [CookCorner; 3],
    tex_id: u16,
    depth: u8,
    verts: &mut Vec<[i16; 3]>,
    tri_idx: &mut Vec<u16>,
    tri_tex: &mut Vec<u16>,
    tri_uv: &mut Vec<u8>,
    tri_rgb: &mut Vec<u8>,
) {
    for axis in 0..2 {
        if let Some(seam) = uv_seam(&c, axis) {
            let input = [c[0], c[1], c[2]];
            let Some(low) = clip_uv_side(&input, axis, seam, true, seam - 1.0, verts) else {
                break;
            };
            let Some(high) = clip_uv_side(&input, axis, seam, false, seam, verts) else {
                break;
            };
            emit_cooked_poly(
                &low, tex_id, depth, verts, tri_idx, tri_tex, tri_uv, tri_rgb,
            );
            emit_cooked_poly(
                &high, tex_id, depth, verts, tri_idx, tri_tex, tri_uv, tri_rgb,
            );
            return;
        }
    }

    tri_idx.extend_from_slice(&[c[0].idx, c[1].idx, c[2].idx]);
    tri_tex.push(tex_id);
    for v in &c {
        tri_uv.push(uv_byte(v.uv.0));
        tri_uv.push(uv_byte(v.uv.1));
    }
    for v in &c {
        tri_rgb.extend_from_slice(&[v.shade.0, v.shade.1, v.shade.2]);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_source_fan(
    source: &[CookCorner],
    tex_id: u16,
    fan_anchor: usize,
    verts: &mut Vec<[i16; 3]>,
    tri_idx: &mut Vec<u16>,
    tri_tex: &mut Vec<u16>,
    tri_uv: &mut Vec<u8>,
    tri_rgb: &mut Vec<u8>,
) {
    for k in 1..source.len() - 1 {
        let ia = fan_anchor;
        let ib = (fan_anchor + k + 1) % source.len();
        let ic = (fan_anchor + k) % source.len();
        emit_cooked_tri(
            [source[ia], source[ib], source[ic]],
            tex_id,
            UV_SPLIT_DEPTH,
            verts,
            tri_idx,
            tri_tex,
            tri_uv,
            tri_rgb,
        );
    }
}

#[derive(Clone, Copy)]
struct RefineCorner {
    idx: u16,
    uv: [u8; 2],
    rgb: [u8; 3],
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg(test)]
struct RefineEdge([i16; 3], [i16; 3]);

#[cfg(test)]
fn refine_edge(verts: &[[i16; 3]], a: u16, b: u16) -> RefineEdge {
    let (a, b) = (verts[a as usize], verts[b as usize]);
    if a <= b {
        RefineEdge(a, b)
    } else {
        RefineEdge(b, a)
    }
}

#[derive(Clone, Copy, Default)]
#[cfg(test)]
struct RefineEdgeInfo {
    incidents: usize,
    severity: u64,
    wanted: bool,
}

#[derive(Clone, Copy)]
#[cfg(test)]
struct RefineSplit {
    idx: u16,
    numerator: i32,
    denominator: i32,
}

#[cfg(test)]
fn gcd_i32(mut a: i32, mut b: i32) -> i32 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.abs()
}

/// Return an exact interior lattice point on `edge`, preferring the point
/// nearest its geometric midpoint. Cooked vertices are integer world units;
/// independently rounding an ideal half point in X/Y/Z can move it off the
/// source edge and make child triangles leave a visible wedge at grazing
/// angles. An integer point exists iff the edge direction's component GCD is
/// at least two. Edges without one are deliberately left unsplit.
#[cfg(test)]
fn exact_edge_split(edge: RefineEdge) -> Option<([i16; 3], i32, i32)> {
    let d = [
        edge.1[0] as i32 - edge.0[0] as i32,
        edge.1[1] as i32 - edge.0[1] as i32,
        edge.1[2] as i32 - edge.0[2] as i32,
    ];
    let denominator = gcd_i32(gcd_i32(d[0].abs(), d[1].abs()), d[2].abs());
    if denominator < 2 {
        return None;
    }
    let numerator = denominator / 2;
    let p = [
        edge.0[0] as i32 + (d[0] / denominator) * numerator,
        edge.0[1] as i32 + (d[1] / denominator) * numerator,
        edge.0[2] as i32 + (d[2] / denominator) * numerator,
    ];
    Some((
        [p[0] as i16, p[1] as i16, p[2] as i16],
        numerator,
        denominator,
    ))
}

#[inline]
#[cfg(test)]
fn local_uv_delta(a: u8, b: u8) -> i32 {
    b as i32 - a as i32
}

#[inline]
#[cfg(test)]
fn split_corner(
    a: RefineCorner,
    b: RefineCorner,
    edge: RefineEdge,
    split: RefineSplit,
    verts: &[[i16; 3]],
) -> RefineCorner {
    let pa = verts[a.idx as usize];
    let pb = verts[b.idx as usize];
    let numerator = if pa == edge.0 && pb == edge.1 {
        split.numerator
    } else {
        debug_assert!(pa == edge.1 && pb == edge.0);
        split.denominator - split.numerator
    };
    let round_ratio = |value: i32| {
        if value >= 0 {
            (value + split.denominator / 2) / split.denominator
        } else {
            -((-value + split.denominator / 2) / split.denominator)
        }
    };
    let split_uv = |x: u8, y: u8| {
        (x as i32 + round_ratio(local_uv_delta(x, y) * numerator)).clamp(0, 255) as u8
    };
    let split_rgb = |x: u8, y: u8| {
        (x as i32 + round_ratio((y as i32 - x as i32) * numerator)).clamp(0, 255) as u8
    };
    RefineCorner {
        idx: split.idx,
        uv: [split_uv(a.uv[0], b.uv[0]), split_uv(a.uv[1], b.uv[1])],
        rgb: [
            split_rgb(a.rgb[0], b.rgb[0]),
            split_rgb(a.rgb[1], b.rgb[1]),
            split_rgb(a.rgb[2], b.rgb[2]),
        ],
    }
}

fn push_refined_tri(
    c: [RefineCorner; 3],
    tex: u16,
    tri_idx: &mut Vec<u16>,
    tri_tex: &mut Vec<u16>,
    tri_uv: &mut Vec<u8>,
    tri_rgb: &mut Vec<u8>,
) {
    for v in c {
        tri_idx.push(v.idx);
        tri_uv.extend_from_slice(&v.uv);
        tri_rgb.extend_from_slice(&v.rgb);
    }
    tri_tex.push(tex);
}

#[cfg(test)]
fn dominant_projected_area(verts: &[[i16; 3]], tri: [u16; 3], drop_axis: usize) -> i64 {
    let project = |p: [i16; 3]| -> (i64, i64) {
        match drop_axis {
            0 => (p[1] as i64, p[2] as i64),
            1 => (p[0] as i64, p[2] as i64),
            _ => (p[0] as i64, p[1] as i64),
        }
    };
    let (a, b, c) = (
        project(verts[tri[0] as usize]),
        project(verts[tri[1] as usize]),
        project(verts[tri[2] as usize]),
    );
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

#[cfg(test)]
fn dominant_drop_axis(verts: &[[i16; 3]], tri: [u16; 3]) -> usize {
    let (a, b, c) = (
        verts[tri[0] as usize],
        verts[tri[1] as usize],
        verts[tri[2] as usize],
    );
    let ab = [
        b[0] as i64 - a[0] as i64,
        b[1] as i64 - a[1] as i64,
        b[2] as i64 - a[2] as i64,
    ];
    let ac = [
        c[0] as i64 - a[0] as i64,
        c[1] as i64 - a[1] as i64,
        c[2] as i64 - a[2] as i64,
    ];
    let n = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    if n[0].abs() >= n[1].abs() && n[0].abs() >= n[2].abs() {
        0
    } else if n[1].abs() >= n[2].abs() {
        1
    } else {
        2
    }
}

/// Ear-clip a convex triangle boundary after one or more points were inserted
/// on its edges. A fan from a boundary endpoint is invalid here: it creates
/// collinear filler triangles on the endpoint's own edge and leaves the source
/// triangle geometrically unsplit. Removing only strict, winding-preserving
/// ears guarantees every inserted edge vertex participates in real geometry.
fn triangulate_refined_boundary(
    boundary: &[RefineCorner],
    verts: &[[i16; 3]],
    tex: u16,
    tri_idx: &mut Vec<u16>,
    tri_tex: &mut Vec<u16>,
    tri_uv: &mut Vec<u8>,
    tri_rgb: &mut Vec<u8>,
    preserve_boundary: bool,
) -> usize {
    if boundary.len() < 3 {
        return 0;
    }
    let output_start = (tri_idx.len(), tri_tex.len(), tri_uv.len(), tri_rgb.len());
    let mut normal = [0i64; 3];
    for i in 0..boundary.len() {
        let a = verts[boundary[i].idx as usize];
        let b = verts[boundary[(i + 1) % boundary.len()].idx as usize];
        normal[0] += (a[1] as i64 - b[1] as i64) * (a[2] as i64 + b[2] as i64);
        normal[1] += (a[2] as i64 - b[2] as i64) * (a[0] as i64 + b[0] as i64);
        normal[2] += (a[0] as i64 - b[0] as i64) * (a[1] as i64 + b[1] as i64);
    }
    let drop_axis = if normal[0].abs() >= normal[1].abs() && normal[0].abs() >= normal[2].abs() {
        0
    } else if normal[1].abs() >= normal[2].abs() {
        1
    } else {
        2
    };
    let project = |p: [i16; 3]| -> (i64, i64) {
        match drop_axis {
            0 => (p[1] as i64, p[2] as i64),
            1 => (p[0] as i64, p[2] as i64),
            _ => (p[0] as i64, p[1] as i64),
        }
    };
    let turn = |a: RefineCorner, b: RefineCorner, c: RefineCorner| -> i64 {
        let (ax, ay) = project(verts[a.idx as usize]);
        let (bx, by) = project(verts[b.idx as usize]);
        let (cx, cy) = project(verts[c.idx as usize]);
        (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)
    };
    let mut winding = 0i64;
    for i in 0..boundary.len() {
        let (ax, ay) = project(verts[boundary[i].idx as usize]);
        let (bx, by) = project(verts[boundary[(i + 1) % boundary.len()].idx as usize]);
        winding += ax * by - ay * bx;
    }
    let sign = if winding < 0 { -1 } else { 1 };
    let mut poly = boundary.to_vec();
    let mut emitted = 0usize;
    while poly.len() > 3 {
        let mut ear = None;
        for i in 0..poly.len() {
            let a = poly[(i + poly.len() - 1) % poly.len()];
            let b = poly[i];
            let c = poly[(i + 1) % poly.len()];
            if turn(a, b, c) * sign > 0 {
                // A strict corner alone is insufficient: its diagonal can
                // skip the inserted points on the opposite edge, emitting the
                // original triangle and leaving a zero-area remainder. Keep
                // every boundary constraint outside the ear, including points
                // exactly on its diagonal.
                let contains_boundary = poly.iter().enumerate().any(|(j, &p)| {
                    j != i
                        && j != (i + poly.len() - 1) % poly.len()
                        && j != (i + 1) % poly.len()
                        && turn(a, b, p) * sign >= 0
                        && turn(b, c, p) * sign >= 0
                        && turn(c, a, p) * sign >= 0
                });
                if !preserve_boundary || !contains_boundary {
                    ear = Some((i, [a, b, c]));
                    break;
                }
            }
        }
        let Some((i, tri)) = ear else {
            break;
        };
        push_refined_tri(tri, tex, tri_idx, tri_tex, tri_uv, tri_rgb);
        emitted += 1;
        poly.remove(i);
    }
    if poly.len() == 3 && turn(poly[0], poly[1], poly[2]) != 0 {
        push_refined_tri(
            [poly[0], poly[1], poly[2]],
            tex,
            tri_idx,
            tri_tex,
            tri_uv,
            tri_rgb,
        );
        emitted += 1;
    }
    if preserve_boundary && emitted + 2 != boundary.len() {
        // Never leave a partially triangulated neighbour in the output. The
        // caller's grid transaction will reject an unmatched required edge.
        tri_idx.truncate(output_start.0);
        tri_tex.truncate(output_start.1);
        tri_uv.truncate(output_start.2);
        tri_rgb.truncate(output_start.3);
        return 0;
    }
    emitted
}

/// Refine selected geometric edges, then force every incident triangle to use
/// the exact same midpoint vertex. This is a conforming mesh pass: a selected
/// edge contributes one triangle per incident source triangle, and the 1/2/3
/// edge cases below cover the original area without collinear filler tris.
///
/// The previous weld path fanned a boundary containing an inserted midpoint
/// from one endpoint. That produced a zero-area `(a, midpoint, b)` triangle and
/// then re-emitted the unsplit source triangle, so packet counts went up while
/// the visible mesh often did not change at all.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn refine_affine_edges(
    verts: &mut Vec<[i16; 3]>,
    tri_idx: &mut Vec<u16>,
    tri_tex: &mut Vec<u16>,
    tri_uv: &mut Vec<u8>,
    tri_rgb: &mut Vec<u8>,
    face_first: &mut [u32],
    face_ntri: &mut [u16],
    face_candidate: &[bool],
    face_refined: &mut [bool],
    max_added_tris: usize,
) -> (usize, usize, usize) {
    use std::collections::{BTreeMap, BTreeSet};

    if max_added_tris == 0 || verts.len() >= MAX_COOK_VERTS {
        return (0, 0, 0);
    }

    let corner = |t: usize, k: usize| RefineCorner {
        idx: tri_idx[t * 3 + k],
        uv: [tri_uv[t * 6 + k * 2], tri_uv[t * 6 + k * 2 + 1]],
        rgb: [
            tri_rgb[t * 9 + k * 3],
            tri_rgb[t * 9 + k * 3 + 1],
            tri_rgb[t * 9 + k * 3 + 2],
        ],
    };

    let mut edges: BTreeMap<RefineEdge, RefineEdgeInfo> = BTreeMap::new();
    for f in 0..face_first.len() {
        let first = face_first[f] as usize;
        let end = first + face_ntri[f] as usize;
        for t in first..end {
            let c = [corner(t, 0), corner(t, 1), corner(t, 2)];
            for e in 0..3 {
                let a = c[e];
                let b = c[(e + 1) % 3];
                let key = refine_edge(verts, a.idx, b.idx);
                let info = edges.entry(key).or_default();
                info.incidents += 1;
                if !face_candidate.get(f).copied().unwrap_or(false) {
                    continue;
                }
                let d = [
                    key.1[0] as i64 - key.0[0] as i64,
                    key.1[1] as i64 - key.0[1] as i64,
                    key.1[2] as i64 - key.0[2] as i64,
                ];
                let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                let uv_span = local_uv_delta(a.uv[0], b.uv[0])
                    .abs()
                    .max(local_uv_delta(a.uv[1], b.uv[1]).abs());
                if len2 >= AFFINE_EDGE_MIN_WORLD * AFFINE_EDGE_MIN_WORLD
                    && uv_span >= AFFINE_EDGE_MIN_UV
                {
                    info.wanted = true;
                    info.severity = info
                        .severity
                        .max(len2 as u64 * (uv_span as u64 * uv_span as u64));
                }
            }
        }
    }

    let mut ranked: Vec<(u64, RefineEdge, usize)> = edges
        .iter()
        .filter_map(|(&edge, info)| {
            (info.wanted && exact_edge_split(edge).is_some()).then_some((
                info.severity,
                edge,
                info.incidents,
            ))
        })
        .collect();
    ranked.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut selected = BTreeSet::new();
    let mut selected_cost = 0usize;
    for (_, edge, incidents) in ranked {
        if selected_cost + incidents > max_added_tris
            || verts.len() + selected.len() >= MAX_COOK_VERTS
        {
            continue;
        }
        selected.insert(edge);
        selected_cost += incidents;
    }
    if selected.is_empty() {
        return (0, 0, 0);
    }

    let mut splits = BTreeMap::new();
    for &edge in &selected {
        let (position, numerator, denominator) =
            exact_edge_split(edge).expect("selected affine edge has a lattice split");
        let idx = verts.len() as u16;
        verts.push(position);
        splits.insert(
            edge,
            RefineSplit {
                idx,
                numerator,
                denominator,
            },
        );
    }

    let old_tris = tri_idx.len() / 3;
    let mut new_idx = Vec::with_capacity(tri_idx.len() + selected_cost * 3);
    let mut new_tex = Vec::with_capacity(tri_tex.len() + selected_cost);
    let mut new_uv = Vec::with_capacity(tri_uv.len() + selected_cost * 6);
    let mut new_rgb = Vec::with_capacity(tri_rgb.len() + selected_cost * 9);
    let mut changed_faces = 0usize;
    let mut coverage_mismatches = 0usize;
    let mut inverted_children = 0usize;
    let mut worst_coverage_delta = 0u64;

    for f in 0..face_first.len() {
        let first = face_first[f] as usize;
        let end = first + face_ntri[f] as usize;
        let new_first = new_tex.len();
        let mut changed = false;
        for t in first..end {
            let c = [corner(t, 0), corner(t, 1), corner(t, 2)];
            let child_first = new_tex.len();
            let mut mid = [None; 3];
            let mut mask = 0u8;
            for e in 0..3 {
                let key = refine_edge(verts, c[e].idx, c[(e + 1) % 3].idx);
                if let Some(&split) = splits.get(&key) {
                    mid[e] = Some(split_corner(c[e], c[(e + 1) % 3], key, split, verts));
                    mask |= 1 << e;
                }
            }
            let tex = tri_tex[t];
            match mask {
                0 => push_refined_tri(
                    c,
                    tex,
                    &mut new_idx,
                    &mut new_tex,
                    &mut new_uv,
                    &mut new_rgb,
                ),
                1 => {
                    let ab = mid[0].unwrap();
                    push_refined_tri(
                        [c[0], ab, c[2]],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [ab, c[1], c[2]],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                }
                2 => {
                    let bc = mid[1].unwrap();
                    push_refined_tri(
                        [c[0], c[1], bc],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [c[0], bc, c[2]],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                }
                4 => {
                    let ca = mid[2].unwrap();
                    push_refined_tri(
                        [c[0], c[1], ca],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [ca, c[1], c[2]],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                }
                3 => {
                    let (ab, bc) = (mid[0].unwrap(), mid[1].unwrap());
                    push_refined_tri(
                        [ab, c[1], bc],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [c[0], ab, bc],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [c[0], bc, c[2]],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                }
                6 => {
                    let (bc, ca) = (mid[1].unwrap(), mid[2].unwrap());
                    push_refined_tri(
                        [bc, c[2], ca],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [c[0], c[1], ca],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [c[1], bc, ca],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                }
                5 => {
                    let (ab, ca) = (mid[0].unwrap(), mid[2].unwrap());
                    push_refined_tri(
                        [ca, c[0], ab],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [ab, c[1], c[2]],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [ab, c[2], ca],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                }
                _ => {
                    let (ab, bc, ca) = (mid[0].unwrap(), mid[1].unwrap(), mid[2].unwrap());
                    push_refined_tri(
                        [c[0], ab, ca],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [ab, c[1], bc],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [ca, bc, c[2]],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                    push_refined_tri(
                        [ab, bc, ca],
                        tex,
                        &mut new_idx,
                        &mut new_tex,
                        &mut new_uv,
                        &mut new_rgb,
                    );
                }
            }
            if mask != 0 {
                let source = [c[0].idx, c[1].idx, c[2].idx];
                let drop_axis = dominant_drop_axis(verts, source);
                let source_area = dominant_projected_area(verts, source, drop_axis);
                let sign = source_area.signum();
                let mut child_area = 0i64;
                for child in child_first..new_tex.len() {
                    let tri = [
                        new_idx[child * 3],
                        new_idx[child * 3 + 1],
                        new_idx[child * 3 + 2],
                    ];
                    let area = dominant_projected_area(verts, tri, drop_axis);
                    child_area += area;
                    inverted_children += (area.signum() != sign) as usize;
                }
                if child_area != source_area {
                    coverage_mismatches += 1;
                    worst_coverage_delta =
                        worst_coverage_delta.max(child_area.abs_diff(source_area));
                }
            }
            changed |= mask != 0;
        }
        face_first[f] = new_first as u32;
        face_ntri[f] = (new_tex.len() - new_first).min(u16::MAX as usize) as u16;
        if changed {
            face_refined[f] = true;
            changed_faces += 1;
        }
    }

    *tri_idx = new_idx;
    *tri_tex = new_tex;
    *tri_uv = new_uv;
    *tri_rgb = new_rgb;
    eprintln!(
        "  affine coverage: {coverage_mismatches} mismatched source tris, {inverted_children} inverted/degenerate children, worst delta {worst_coverage_delta}"
    );
    assert_eq!(
        coverage_mismatches, 0,
        "affine refinement changed projected source-triangle coverage"
    );
    assert_eq!(
        inverted_children, 0,
        "affine refinement emitted an inverted or degenerate child"
    );
    (selected.len(), tri_tex.len() - old_tris, changed_faces)
}

/// Pull `"key" "value"` from one entity text block.
fn ent_value<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    let pat = ["\"", key, "\""].concat();
    let i = block.find(&pat)? + pat.len();
    let rest = &block[i..];
    let a = rest.find('"')? + 1;
    let b = rest[a..].find('"')? + a;
    Some(&rest[a..b])
}

fn parse_vec3(s: &str) -> Option<[f32; 3]> {
    let mut it = s.split_whitespace();
    Some([
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    ])
}

/// Reproduce the engine's ordered `angle`/`angles` key dispatch.
///
/// GoldSrc rewrites the legacy singular key to an `angles` key as it is
/// delivered to the game DLL. A non-negative singular value replaces yaw but
/// preserves the pitch/roll accumulated so far; -1/-2 are the old up/down
/// sentinels. Since duplicate keys are legal, the last key in entity-lump
/// order wins. Looking either key up independently loses all three rules.
fn ent_angles_degrees(block: &str) -> Option<[f32; 3]> {
    let mut angles = [0.0; 3];
    let mut found = false;
    iter_ent_pairs(block, |key, value| match key {
        "angles" => {
            if let Some(parsed) = parse_vec3(value) {
                angles = parsed;
                found = true;
            }
        }
        "angle" => {
            let Ok(angle) = value.parse::<f32>() else {
                return;
            };
            angles = if angle >= 0.0 {
                [angles[0], angle, angles[2]]
            } else if angle == -1.0 {
                [-90.0, 0.0, 0.0]
            } else if angle == -2.0 {
                [90.0, 0.0, 0.0]
            } else {
                [0.0; 3]
            };
            found = true;
        }
        _ => {}
    });
    found.then_some(angles)
}

fn ent_yaw_degrees(block: &str) -> Option<f32> {
    ent_angles_degrees(block).map(|angles| angles[1])
}

/// GoldSrc `SetMovedir`: turn the effective Euler angles into a forward
/// vector before the owning entity class clears its visual angles.
fn ent_move_dir(block: &str) -> [f32; 3] {
    let [pitch, yaw, _] = ent_angles_degrees(block).unwrap_or([0.0; 3]);
    let (sp, cp) = pitch.to_radians().sin_cos();
    let (sy, cy) = yaw.to_radians().sin_cos();
    [cp * cy, cp * sy, -sp]
}

fn hl_yaw_to_world_q12(deg: f32) -> i32 {
    // HL yaw 0 = +X, yaw 90 = +Y. World is [HL X, HL Z, HL Y], and this
    // runtime's yaw 0 forward is +world Z, so HL degrees map to 90 - yaw.
    (((90.0 - deg) / 360.0 * 4096.0).round() as i32) & 0xFFF
}

/// Pack a studio prop's complete authored transform without growing PropRec.
/// Low 12 bits retain the existing world yaw, bits 12..15 retain body, and the
/// formerly-unused upper half stores reflected GoldSrc pitch/roll as q8 turns.
fn pack_prop_orientation(angles: [f32; 3], body: u8) -> i32 {
    let q8 = |degrees: f32| ((-degrees * 256.0 / 360.0).round() as i32 & 0xff) as u32;
    let yaw = hl_yaw_to_world_q12(angles[1]) as u32;
    (yaw | ((body.min(15) as u32) << 12) | (q8(angles[0]) << 16) | (q8(angles[2]) << 24)) as i32
}

fn dot12_i16(row: [i16; 3], p: [i32; 3]) -> i32 {
    ((row[0] as i32 * p[0]) + (row[1] as i32 * p[1]) + (row[2] as i32 * p[2])) >> 12
}

fn view_rotation_yaw_rows(yaw_q12: i32) -> [[i16; 3]; 3] {
    let view_yaw = (-(yaw_q12 >> 4)) as f32 * std::f32::consts::TAU / 256.0;
    let (s, c) = view_yaw.sin_cos();
    let q = |v: f32| (v * 4096.0).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    [[q(-c), 0, q(-s)], [0, -4096, 0], [q(-s), 0, q(c)]]
}

fn box_visible_offline(
    center: [i16; 3],
    ext: [u16; 3],
    rot: [[i16; 3]; 3],
    base_t: [i32; 3],
) -> bool {
    const WORLD_BOUNDS_PAD: i32 = 256;
    let c = [center[0] as i32, center[1] as i32, center[2] as i32];
    let e = [ext[0] as i32, ext[1] as i32, ext[2] as i32];
    let abs_dot12 = |row: [i16; 3]| -> i32 {
        ((row[0].abs() as i32 * e[0]) + (row[1].abs() as i32 * e[1]) + (row[2].abs() as i32 * e[2]))
            >> 12
    };
    let vz = dot12_i16(rot[2], c) + base_t[2];
    let zmax = vz + abs_dot12(rot[2]);
    if zmax < 2 {
        return false;
    }
    let vx = dot12_i16(rot[0], c) + base_t[0];
    let ex = abs_dot12(rot[0]);
    if (vx - ex) * 2 > zmax * 2 + WORLD_BOUNDS_PAD || (-vx - ex) * 2 > zmax * 2 + WORLD_BOUNDS_PAD {
        return false;
    }
    let vy = dot12_i16(rot[1], c) + base_t[1];
    let ey = abs_dot12(rot[1]);
    (vy - ey) * 4 <= zmax * 3 + WORLD_BOUNDS_PAD && (-vy - ey) * 4 <= zmax * 3 + WORLD_BOUNDS_PAD
}

/// Find the single-player spawn (`info_player_start`) origin + yaw (HL coords,
/// degrees) from the entity lump.
fn find_spawn(ents: &[u8]) -> Option<([f32; 3], f32)> {
    let s = entity_text(ents);
    for block in s.split('{') {
        if block.contains("\"info_player_start\"") {
            let origin = parse_vec3(ent_value(block, "origin")?)?;
            let yaw = ent_yaw_degrees(block).unwrap_or(0.0);
            return Some((origin, yaw));
        }
    }
    None
}

#[derive(Clone, Copy)]
struct SpawnCandidate {
    origin_hl: [f32; 3],
    yaw_q12: Option<i32>,
    is_player_start: bool,
}

fn standalone_spawn_candidates(ents: &[u8]) -> Vec<SpawnCandidate> {
    let s = entity_text(ents);
    let mut out = Vec::new();
    // Only MoveTo=4 teleports an auto-start scripted actor to the mark.
    // MoveTo=0 waits at its authored origin; 1/2 walk/run there at runtime.
    let mut script_marks: Vec<(String, [f32; 3], Option<f32>)> = Vec::new();
    for block in s.split('{') {
        if !matches!(
            ent_value(block, "classname"),
            Some("scripted_sequence" | "aiscripted_sequence")
        ) {
            continue;
        }
        if ent_value(block, "targetname").is_some() {
            continue;
        }
        let move_to = ent_value(block, "m_fMoveTo")
            .or_else(|| ent_value(block, "m_flMoveTo"))
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        if move_to != 4 {
            continue;
        }
        let Some(target) = ent_value(block, "m_iszEntity") else {
            continue;
        };
        let Some(origin) = ent_value(block, "origin").and_then(parse_vec3) else {
            continue;
        };
        script_marks.push((target.to_string(), origin, ent_yaw_degrees(block)));
    }
    for block in s.split('{') {
        let cls = ent_value(block, "classname").unwrap_or("");
        if cls != "info_player_start" && cls != "info_landmark" {
            continue;
        }
        let Some(origin) = ent_value(block, "origin").and_then(parse_vec3) else {
            continue;
        };
        out.push(SpawnCandidate {
            origin_hl: origin,
            yaw_q12: ent_yaw_degrees(block).map(hl_yaw_to_world_q12),
            is_player_start: cls == "info_player_start",
        });
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn score_spawn_yaw(
    origin_hl: [f32; 3],
    yaw_q12: i32,
    scale: f32,
    nodes: &[u8],
    planes: &[u8],
    leaves: &[u8],
    n_visleaves: usize,
    marks: &[u8],
    vis: &[u8],
    face_ntri: &[u16],
    face_norm: &[[i16; 3]],
    face_dist: &[i32],
    face_center: &[[i16; 3]],
    face_extent: &[[u16; 3]],
) -> i32 {
    const VIEW_HEIGHT: i32 = 28;
    let n_leaves = leaves.len() / SZ_LEAF;
    let n_marks = marks.len() / SZ_MARKSURFACE;
    if n_leaves <= 1 || n_visleaves == 0 {
        return 0;
    }
    let eye_hl = [
        origin_hl[0],
        origin_hl[1],
        origin_hl[2] + VIEW_HEIGHT as f32 * scale,
    ];
    let leaf = point_leaf(eye_hl, nodes, planes);
    if leaf <= 0 || leaf as usize > n_visleaves {
        return 0;
    }
    let lo = leaf as usize * SZ_LEAF;
    let visofs = i32le(leaves, lo + SZ_LEAF_VISOFS).unwrap_or(-1);
    let row = (n_visleaves + 7) / 8;
    let mut bits = vec![0u8; row];
    if visofs < 0 {
        bits.fill(0xFF);
    } else {
        let mut v = visofs as usize;
        let mut c = 0usize;
        while c < row && v < vis.len() {
            if vis[v] != 0 {
                bits[c] = vis[v];
                v += 1;
                c += 1;
            } else {
                v += 1;
                if v >= vis.len() {
                    break;
                }
                c = (c + vis[v] as usize).min(row);
                v += 1;
            }
        }
    }

    let eye = to_world(origin_hl, scale);
    let eye = [eye[0], eye[1] + VIEW_HEIGHT, eye[2]];
    let rot = view_rotation_yaw_rows(yaw_q12);
    let base_t = [
        -dot12_i16(rot[0], eye),
        -dot12_i16(rot[1], eye),
        -dot12_i16(rot[2], eye),
    ];
    let mut seen = vec![false; face_ntri.len()];
    let mut score = 0i32;
    for bit in 0..n_visleaves.min(bits.len() * 8) {
        if bits[bit >> 3] & (1u8 << (bit & 7)) == 0 {
            continue;
        }
        let li = bit + 1;
        let lo = li * SZ_LEAF;
        let m0 = u16le(leaves, lo + SZ_LEAF_MARK0).unwrap_or(0) as usize;
        let mc = u16le(leaves, lo + SZ_LEAF_MARK0 + 2).unwrap_or(0) as usize;
        for mj in m0..m0 + mc {
            if mj >= n_marks {
                break;
            }
            let face = u16le(marks, mj * SZ_MARKSURFACE).unwrap_or(0) as usize;
            if face >= face_ntri.len() || seen[face] || face_ntri[face] == 0 {
                continue;
            }
            seen[face] = true;
            if dot12_i16(face_norm[face], eye) <= face_dist[face] {
                continue;
            }
            if !box_visible_offline(face_center[face], face_extent[face], rot, base_t) {
                continue;
            }
            score += face_ntri[face] as i32;
        }
    }
    score
}

fn spawn_leaf_contents(origin_hl: [f32; 3], nodes: &[u8], planes: &[u8], leaves: &[u8]) -> i32 {
    let leaf = point_leaf(origin_hl, nodes, planes);
    let lo = leaf as usize * SZ_LEAF;
    if leaf <= 0 || lo + 4 > leaves.len() {
        return CONTENTS_SOLID as i32;
    }
    i32le(leaves, lo).unwrap_or(CONTENTS_SOLID as i32)
}

fn spawn_candidate_clear(
    origin_hl: [f32; 3],
    nodes: &[u8],
    planes: &[u8],
    leaves: &[u8],
    clipnodes: &[u8],
    hull1_head: i32,
) -> bool {
    if spawn_leaf_contents(origin_hl, nodes, planes, leaves) == CONTENTS_SOLID as i32 {
        return false;
    }
    if hull1_head < 0 || hull1_head as usize >= clipnodes.len() / SZ_CLIPNODE {
        return true;
    }
    point_contents_raw(clipnodes, planes, hull1_head as i16, origin_hl) != CONTENTS_SOLID
}

/// Fallback spawn for chapter-select when every entity spawn sits in a tiny
/// visibility pocket. HL's `info_player_start` is only a fresh-game start;
/// in normal play you arrive elsewhere via a changelevel landmark. So a map
/// like c1a1 spawns you in a 227-face dead pocket and the level reads as black.
/// This picks the leaf that can see the most of the map and spawns at its
/// centre (gravity drops the player to the floor at runtime), so the level is
/// actually visible. Returns `(origin_hl, yaw_q12, score)`.
#[allow(clippy::too_many_arguments)]
fn best_visibility_spawn(
    scale: f32,
    nodes: &[u8],
    planes: &[u8],
    leaves: &[u8],
    n_visleaves: usize,
    clipnodes: &[u8],
    hull1_head: i32,
    marks: &[u8],
    vis: &[u8],
    face_ntri: &[u16],
    face_norm: &[[i16; 3]],
    face_dist: &[i32],
    face_center: &[[i16; 3]],
    face_extent: &[[u16; 3]],
    face_bright: &[u8],
) -> Option<([f32; 3], i32, i32)> {
    let n_leaves = leaves.len() / SZ_LEAF;
    let n_marks = marks.len() / SZ_MARKSURFACE;
    if n_leaves <= 1 || n_visleaves == 0 {
        return None;
    }
    let row = (n_visleaves + 7) / 8;
    let mut bits = vec![0u8; row];
    // Rank leaves by how many leaves they can see. This is a cheap proxy for an
    // open view; ranking by raw face count would favour leaves whose PVS blows
    // the render arena, whereas the most-connected leaves give a full, in-budget
    // view.
    let mut ranked: Vec<(u32, usize)> = Vec::new();
    for l in 1..=n_visleaves {
        let visofs = i32le(leaves, l * SZ_LEAF + SZ_LEAF_VISOFS).unwrap_or(-1);
        if visofs < 0 {
            continue; // degenerate "sees everything" leaf (outside/solid)
        }
        bits.fill(0);
        let mut v = visofs as usize;
        let mut c = 0usize;
        while c < row && v < vis.len() {
            if vis[v] != 0 {
                bits[c] = vis[v];
                v += 1;
                c += 1;
            } else {
                v += 1;
                if v >= vis.len() {
                    break;
                }
                c = (c + vis[v] as usize).min(row);
                v += 1;
            }
        }
        let seen: u32 = bits.iter().map(|b| b.count_ones()).sum();
        ranked.push((seen, l));
    }
    ranked.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    // Take the most-visible leaf whose centre is actually clear (not solid, not
    // mid-wall). The centroid of a convex BSP leaf's boundary faces lands inside
    // it; runtime gravity settles the player onto the floor.
    // Among the most-visible clear leaves, prefer the BRIGHTEST one -- a well-lit
    // open leaf, not a dark visible pocket (c4a3's chapter-select spawn landed in
    // a near-black room). Brightness = mean of the leaf faces' lightmap levels.
    let mut best_pick: Option<([f32; 3], i32, i32, i64)> = None;
    for (_, l) in ranked.into_iter().take(16) {
        let lo = l * SZ_LEAF;
        let m0 = u16le(leaves, lo + SZ_LEAF_MARK0).unwrap_or(0) as usize;
        let mc = u16le(leaves, lo + SZ_LEAF_MARK0 + 2).unwrap_or(0) as usize;
        let (mut sx, mut sy, mut sz, mut sb, mut n) = (0i64, 0i64, 0i64, 0i64, 0i64);
        for mj in m0..m0 + mc {
            if mj >= n_marks {
                break;
            }
            let f = u16le(marks, mj * SZ_MARKSURFACE).unwrap_or(0) as usize;
            if f >= face_center.len() {
                continue;
            }
            let c = face_center[f];
            sx += c[0] as i64;
            sy += c[1] as i64;
            sz += c[2] as i64;
            sb += *face_bright.get(f).unwrap_or(&128) as i64;
            n += 1;
        }
        if n == 0 {
            continue;
        }
        let (wx, wy, wz) = ((sx / n) as f32, (sy / n) as f32, (sz / n) as f32);
        // World centroid -> HL origin: world = (hl_x/scale, hl_z/scale, hl_y/scale).
        let origin_hl = [wx * scale, wz * scale, wy * scale];
        if !spawn_candidate_clear(origin_hl, nodes, planes, leaves, clipnodes, hull1_head) {
            continue;
        }
        let brightness = sb / n;
        let mut best_yaw = 0;
        let mut best_score = i32::MIN;
        for i in 0..8 {
            let yaw = (i * 512) & 0xFFF;
            let score = score_spawn_yaw(
                origin_hl,
                yaw,
                scale,
                nodes,
                planes,
                leaves,
                n_visleaves,
                marks,
                vis,
                face_ntri,
                face_norm,
                face_dist,
                face_center,
                face_extent,
            );
            if score > best_score {
                best_score = score;
                best_yaw = yaw;
            }
        }
        if best_pick.map_or(true, |(_, _, _, b)| brightness > b) {
            best_pick = Some((origin_hl, best_yaw, best_score, brightness));
        }
    }
    best_pick.map(|(o, y, s, _)| (o, y, s))
}

#[allow(clippy::too_many_arguments)]
fn choose_standalone_spawn(
    ents: &[u8],
    scale: f32,
    nodes: &[u8],
    planes: &[u8],
    leaves: &[u8],
    n_visleaves: usize,
    clipnodes: &[u8],
    hull1_head: i32,
    marks: &[u8],
    vis: &[u8],
    face_ntri: &[u16],
    face_norm: &[[i16; 3]],
    face_dist: &[i32],
    face_center: &[[i16; 3]],
    face_extent: &[[u16; 3]],
    face_bright: &[u8],
) -> Option<([f32; 3], i32)> {
    const GOOD_STANDALONE_SCORE: i32 = 900;
    const MIN_AUTHORED_STANDALONE_SCORE: i32 = 300;
    let candidates = standalone_spawn_candidates(ents);
    let mut first_player: Option<([f32; 3], i32, i32)> = None;
    let mut best_authored_player: Option<([f32; 3], i32, i32)> = None;
    let mut best_player: Option<([f32; 3], i32, i32)> = None;
    let mut best_landmark: Option<([f32; 3], i32, i32)> = None;
    let debug_spawn = std::env::var_os("HL_BSP_DEBUG_SPAWN").is_some();
    for cand in candidates {
        if !spawn_candidate_clear(cand.origin_hl, nodes, planes, leaves, clipnodes, hull1_head) {
            continue;
        }
        let mut yaws = [0i32; 9];
        for (i, yaw) in yaws[..8].iter_mut().enumerate() {
            *yaw = (i as i32 * 512) & 0xFFF;
        }
        let original_yaw = cand.yaw_q12.unwrap_or(0);
        yaws[8] = original_yaw;
        if cand.is_player_start && first_player.is_none() {
            let score = score_spawn_yaw(
                cand.origin_hl,
                original_yaw,
                scale,
                nodes,
                planes,
                leaves,
                n_visleaves,
                marks,
                vis,
                face_ntri,
                face_norm,
                face_dist,
                face_center,
                face_extent,
            );
            first_player = Some((cand.origin_hl, original_yaw, score));
        }
        if cand.is_player_start && cand.yaw_q12.is_some() {
            let score = score_spawn_yaw(
                cand.origin_hl,
                original_yaw,
                scale,
                nodes,
                planes,
                leaves,
                n_visleaves,
                marks,
                vis,
                face_ntri,
                face_norm,
                face_dist,
                face_center,
                face_extent,
            );
            if best_authored_player.map_or(true, |(_, _, best_score)| score > best_score) {
                best_authored_player = Some((cand.origin_hl, original_yaw, score));
            }
        }
        for yaw in yaws {
            let score = score_spawn_yaw(
                cand.origin_hl,
                yaw,
                scale,
                nodes,
                planes,
                leaves,
                n_visleaves,
                marks,
                vis,
                face_ntri,
                face_norm,
                face_dist,
                face_center,
                face_extent,
            );
            let target = if cand.is_player_start {
                &mut best_player
            } else {
                &mut best_landmark
            };
            if target.map_or(true, |(_, _, best_score)| score > best_score) {
                *target = Some((cand.origin_hl, yaw, score));
            }
        }
        if debug_spawn {
            let mut best_score = i32::MIN;
            let mut best_yaw = 0;
            for yaw in yaws {
                let score = score_spawn_yaw(
                    cand.origin_hl,
                    yaw,
                    scale,
                    nodes,
                    planes,
                    leaves,
                    n_visleaves,
                    marks,
                    vis,
                    face_ntri,
                    face_norm,
                    face_dist,
                    face_center,
                    face_extent,
                );
                if score > best_score {
                    best_score = score;
                    best_yaw = yaw;
                }
            }
            eprintln!(
                "spawn candidate player={} authored_yaw={} origin={:?} original_yaw={} best_yaw={} best_score={}",
                cand.is_player_start,
                cand.yaw_q12.is_some(),
                cand.origin_hl,
                original_yaw,
                best_yaw,
                best_score
            );
        }
    }
    if let Some((origin, yaw, score)) = first_player {
        if score >= GOOD_STANDALONE_SCORE {
            return Some((origin, yaw));
        }
    }
    // Only relocate when the authored spawn is a genuine dead pocket (mostly
    // black, like c1a1's 227-face info_player_start) AND there's a genuinely
    // good leaf to move to. A spawn that already sees a decent slice of the
    // level keeps its authored position even if some leaf scores a bit higher --
    // moving it would just drop the player somewhere arbitrary.
    const DEAD_POCKET_SCORE: i32 = 700;
    let best_entity_score = [
        first_player.map(|(_, _, s)| s),
        best_authored_player.map(|(_, _, s)| s),
        best_player.map(|(_, _, s)| s),
        best_landmark.map(|(_, _, s)| s),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(0);
    if let Some((origin, yaw, score)) = best_visibility_spawn(
        scale,
        nodes,
        planes,
        leaves,
        n_visleaves,
        clipnodes,
        hull1_head,
        marks,
        vis,
        face_ntri,
        face_norm,
        face_dist,
        face_center,
        face_extent,
        face_bright,
    ) {
        if best_entity_score < DEAD_POCKET_SCORE && score >= GOOD_STANDALONE_SCORE {
            if debug_spawn {
                eprintln!(
                    "spawn override: most-visible leaf origin={origin:?} yaw={yaw} score={score} (best entity {best_entity_score})"
                );
            }
            return Some((origin, yaw));
        }
    }
    if let Some((origin, yaw, score)) = best_authored_player {
        if score >= MIN_AUTHORED_STANDALONE_SCORE {
            return Some((origin, yaw));
        }
    }
    if let Some((origin, yaw, _)) = best_player {
        return Some((origin, yaw));
    }
    best_landmark.map(|(origin, yaw, _)| (origin, yaw))
}

fn to_world(p: [f32; 3], scale: f32) -> [i32; 3] {
    [
        (p[0] / scale).round() as i32,
        (p[2] / scale).round() as i32,
        (p[1] / scale).round() as i32,
    ]
}

struct EntRec {
    submodel: u16,
    kind: u16, // 0 static, 1 door, 2 visual, 3 button, 5 fan, 7 axial brush, 8 platrot, 9 pushable, 10 pendulum
    origin: [i32; 3],
    mv: [i32; 3],     // full-open displacement (world)
    center: [i32; 3], // submodel bounds centre; movers use closed-world centre
    r2: i32,          // conservative bounds radius^2 (world)
    head: i32,        // submodel hull-1 clipnode root (collision)
    head0: i32,       // submodel hull-0 BSP node root (point solidity: grates etc.)
    leaves: Vec<u16>, // BSP leaves touched by this entity's bounds, for PVS culling
}

// High-half profile bits in func_rotating EntRec.mv[2]. Authored fanfriction
// occupies only 1..100; these two cook-time facts reproduce GoldSrc pusher
// behavior without a runtime array or a larger streamed record.
const FAN_PROFILE_FRICTION_MASK: u16 = 0x007f;
const FAN_PROFILE_RAMP_EXTRA_THINK: u16 = 0x4000;
const FAN_PROFILE_BLOCKED_AFTER_FIRST_SAMPLE: u16 = 0x8000;

// Entity-local rotating platform. `origin` is the bottom pose/pivot,
// `mv[0]` is the signed full yaw in Q12 turns, and `mv[1]` is the vertical
// bottom-to-top displacement. The fixed EntRec stays 56 bytes.
const ENT_KIND_PLATROT: u16 = 8;
// Translated SOLID_BBOX brush. `origin` is the live runtime offset; mv packs
// max speed + local AABB half-extents without growing the 56-byte EntRec.
// The low speed word also carries GoldSrc's world-hull selection and the
// one-unit SET_MODEL mins padding. GoldSrc does not sweep the full visual box:
// SV_HullForBsp selects one of four canonical hulls and anchors it at mins.
const ENT_KIND_PUSHABLE: u16 = 9;
// Pivot-local CPendulum brush. mv[0] is its centre angle in signed Q19 turns;
// mv[1] packs max Q19-per-tick velocity + world rotation axis; mv[2] packs
// per-host-tick acceleration + authored spawnflags. Runtime phase shares the
// existing ENT_PHASE word with a compact GoldSrc heartbeat index.
const ENT_KIND_PENDULUM: u16 = 10;
// Pivot-local CRotButton / CMomentaryRotButton brush. It shares the rotating
// door's compact angle + axis encoding but keeps button logic and solidity.
const ENT_KIND_ROT_BUTTON: u16 = 11;
const SF_PUSH_BREAKABLE: u16 = 128;

const PUSHABLE_COLLISION_META_VALID: u16 = 0x8000;
const PUSHABLE_COLLISION_HULL_SHIFT: u16 = 8;
const PUSHABLE_COLLISION_MIN_CORR_SHIFT: u16 = 10;

fn pushable_collision_hull(mins: [f32; 3], maxs: [f32; 3]) -> u16 {
    let sx = maxs[0] - mins[0] + 2.0;
    let sz = maxs[2] - mins[2] + 2.0;
    if sx <= 8.0 {
        0 // hull 0: point
    } else if sx <= 36.0 && sz <= 36.0 {
        1 // hull 3: duck, 32x32x36
    } else if sx <= 36.0 {
        2 // hull 1: standing, 32x32x72
    } else {
        3 // hull 2: large, 64x64x64
    }
}

#[inline]
fn pack_pushable_speed_half_x(
    max_speed: i32,
    half_x: i32,
    collision_hull: u16,
    min_correction_mask: u16,
) -> i32 {
    let low = PUSHABLE_COLLISION_META_VALID
        | ((collision_hull & 3) << PUSHABLE_COLLISION_HULL_SHIFT)
        | ((min_correction_mask & 7) << PUSHABLE_COLLISION_MIN_CORR_SHIFT)
        | max_speed.clamp(0, u8::MAX as i32) as u16;
    (((half_x.clamp(0, u16::MAX as i32) as u32) << 16) | low as u32) as i32
}

const LOGIC_TRAIN_TERMINAL: u8 = 1;
const LOGIC_TRAIN_EXTENDED: u8 = 2;
const LOGIC_TRAIN_CYCLE_SHIFT: u8 = 2;
const TRAIN_CORNER_TELEPORT: u16 = 0x8000;
const TRAIN_CORNER_WAIT_TRIGGER_TELEPORT: u16 = 0xfffe;
const TRAIN_CORNER_WAIT_TRIGGER: u16 = 0xffff;
const MAX_RUNTIME_LIVE_PROPS: usize = 113; // 128 minus 15 zero-BSS carry mailbox rows

const DYNAMIC_LIGHTSTYLE_MAX: usize = 31;

#[derive(Clone, Debug)]
struct DynamicLightStyle {
    style: u8,
    targetname: String,
    origins_hl: Vec<[f32; 3]>,
}

/// GoldSrc's `~` texture family marks emissive surfaces. The Hazard Course
/// indicator boards use `+a~generic65`; their named lightstyles illuminate the
/// face independently after each traversal trigger fires.
fn is_dynamic_emissive_texture(name: &str) -> bool {
    name.starts_with('+') && name.contains('~')
}

/// Keep only named dynamic lightstyles that actually touch an emissive face.
/// This avoids serialising unrelated switchable room lighting while retaining
/// a generic face-to-target relationship for every authored indicator panel.
fn collect_dynamic_lightstyles(
    ents: &[u8],
    faces: &[u8],
    texinfo: &[u8],
    tex_names: &[String],
) -> Result<Vec<DynamicLightStyle>, String> {
    let mut used = std::collections::BTreeSet::<u8>::new();
    for f in 0..faces.len() / SZ_FACE {
        let fo = f * SZ_FACE;
        let ti = u16le(faces, fo + 10).unwrap_or(0) as usize;
        let tex = i32le(texinfo, ti * SZ_TEXINFO + 32).unwrap_or(-1);
        let Some(name) = (tex >= 0).then(|| tex_names.get(tex as usize)).flatten() else {
            continue;
        };
        if !is_dynamic_emissive_texture(name) {
            continue;
        }
        for &style in faces.get(fo + 12..fo + 16).unwrap_or(&[]) {
            if style != 0 && style != u8::MAX {
                used.insert(style);
            }
        }
    }

    let text = entity_text(ents);
    let mut by_style = std::collections::BTreeMap::<u8, DynamicLightStyle>::new();
    for block in text.split('{') {
        if !matches!(ent_value(block, "classname"), Some("light" | "light_spot")) {
            continue;
        }
        let Some(style) = ent_value(block, "style").and_then(|v| v.parse::<u8>().ok()) else {
            continue;
        };
        if !used.contains(&style) {
            continue;
        }
        let targetname = ent_value(block, "targetname").unwrap_or("").trim();
        if targetname.is_empty() {
            continue;
        }
        let origin = ent_value(block, "origin")
            .and_then(parse_vec3)
            .unwrap_or([0.0; 3]);
        match by_style.entry(style) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(DynamicLightStyle {
                    style,
                    targetname: targetname.to_string(),
                    origins_hl: vec![origin],
                });
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if entry.get().targetname != targetname {
                    return Err(format!(
                        "dynamic lightstyle {style} has conflicting targets '{}' and '{}'",
                        entry.get().targetname,
                        targetname
                    ));
                }
                entry.get_mut().origins_hl.push(origin);
            }
        }
    }
    let out: Vec<_> = by_style.into_values().collect();
    if out.len() > DYNAMIC_LIGHTSTYLE_MAX {
        return Err(format!(
            "{} emissive dynamic lightstyles exceed compact face limit {}",
            out.len(),
            DYNAMIC_LIGHTSTYLE_MAX
        ));
    }
    Ok(out)
}

fn dynamic_lightstyle_slots_for_face(
    source_styles: &[u8],
    lights: &[DynamicLightStyle],
) -> ([u8; 3], [u8; 3]) {
    let mut slots = [0u8; 3];
    let mut layers = [0u8; 3];
    let mut count = 0;
    // qrad writes style zero first, followed by as many as three independent
    // switchable lightmap planes. Preserve that authored plane order exactly:
    // choosing one light from the face centre (the old representation) made
    // multi-style panels and neighbouring bounce-light surfaces toggle as one.
    for (layer, &style) in source_styles.iter().take(4).enumerate() {
        if style == 0 || style == u8::MAX {
            continue;
        }
        let Some(slot) = lights
            .iter()
            .position(|light| light.style == style)
            .map(|index| index as u8 + 1)
        else {
            continue;
        };
        if count < slots.len() {
            slots[count] = slot;
            layers[count] = layer as u8;
            count += 1;
        }
    }
    (slots, layers)
}

/// FNV-1a 16-bit hash of a global-state name -- a stable cross-map key so the
/// runtime can match an env_global's global to a multisource's globalstate
/// without interning names across maps. Never 0 (0 = "no globalstate").
fn global_hash(name: &str) -> u16 {
    let n = name.trim().to_ascii_lowercase();
    if n.is_empty() {
        return 0;
    }
    let mut h: u32 = 0x811c_9dc5;
    for b in n.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    ((h ^ (h >> 16)) as u16).max(1)
}

const CARRY_GLOBAL_BIT: u16 = 0x8000;
// PropRec targetname bit marking a cooker-synthesized actor that exists only
// to make a direct menu launch self-contained. Runtime strips this bit before
// target dispatch; natural changelevels leave the template dormant and overlay
// it only when the real actor crossed the landmark.
const PROP_NAME_INCOMING_ONLY: u16 = 0x8000;

/// Stable actor identity stored in PropRec's former padding word. Ordinary
/// targetnames and globalnames share the same folded hash, with bit 15 keeping
/// their namespaces distinct. 0 and 0xffff remain runtime sentinels.
fn actor_carry_id(name: &str, global: bool) -> u16 {
    let n = name.trim().to_ascii_lowercase();
    if n.is_empty() {
        return 0;
    }
    let mut h: u32 = 0x811c_9dc5;
    for b in n.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    let mut id = ((h ^ (h >> 16)) as u16 & !CARRY_GLOBAL_BIT).max(1);
    if global && id == 0x7fff {
        id = 0x7ffe; // 0xffff is PROP_LOGIC_LINK's "none" sentinel.
    }
    id | if global { CARRY_GLOBAL_BIT } else { 0 }
}

fn entity_carry_id(block: &str) -> u16 {
    let global = ent_value(block, "globalname").unwrap_or("").trim();
    if !global.is_empty() {
        actor_carry_id(global, true)
    } else {
        actor_carry_id(ent_value(block, "targetname").unwrap_or(""), false)
    }
}

fn weapon_pickup_prop_kind(classname: &str) -> Option<u16> {
    Some(match classname {
        "weapon_crowbar" => 26,
        "weapon_9mmhandgun" | "weapon_glock" => 27,
        "weapon_357" | "weapon_python" => 28,
        "weapon_9mmAR" | "weapon_mp5" => 29,
        "weapon_shotgun" => 30,
        "weapon_crossbow" => 31,
        "weapon_rpg" => 32,
        "weapon_gauss" => 33,
        "weapon_egon" => 34,
        "weapon_hornetgun" => 35,
        "weapon_handgrenade" => 36,
        "weapon_snark" => 37,
        "weapon_tripmine" => 38,
        "weapon_satchel" => 39,
        _ => return None,
    })
}

#[inline]
fn prop_type_crosses_transition(ty: u16) -> bool {
    let base = ty & 0x0fff;
    // FCAP_DONT_SAVE / !FCAP_ACROSS_TRANSITION plus non-actors. Authored dead
    // bodies and monstermaker stock are map-local state, never live carries.
    ty & 0xc000 == 0 && !matches!(base, 3 | 4 | 16 | 26..=49 | 50) && base < 56
}

/// (map_index, key) -> per-map local voice id, from the VOICES_MANIFEST env file
/// written by `host/hl-content`. key = UPPERCASE sentence name (scripted_
/// sentence) or lowercase wav path (ambient_generic).
fn load_voices_manifest() -> std::collections::HashMap<(u16, String), u16> {
    let mut out = std::collections::HashMap::new();
    let Ok(path) = std::env::var("VOICES_MANIFEST") else {
        return out;
    };
    let Ok(txt) = std::fs::read_to_string(&path) else {
        eprintln!("warn: VOICES_MANIFEST unreadable: {}", path);
        return out;
    };
    for line in txt.lines() {
        let mut it = line.trim().split('|');
        let (Some(mi), Some(lid), Some(key)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if let (Ok(mi), Ok(lid)) = (mi.parse::<u16>(), lid.parse::<u16>()) {
            out.insert((mi, key.to_string()), lid);
        }
    }
    out
}

/// (map_index, spr_basename) -> (local_id, base_w, base_h) from SPRITES_MANIFEST
/// (`host/hl-content`). Resolves each env_sprite/env_glow model to its
/// per-map sprite pack slot + native pixel size (for the world billboard scale).
fn load_sprites_manifest() -> std::collections::HashMap<(u16, String), (u16, u16, u16)> {
    let mut out = std::collections::HashMap::new();
    let Ok(path) = std::env::var("SPRITES_MANIFEST") else {
        return out;
    };
    let Ok(txt) = std::fs::read_to_string(&path) else {
        eprintln!("warn: SPRITES_MANIFEST unreadable: {}", path);
        return out;
    };
    // map_idx|local_id|base|blend|n_frames|base_w|base_h
    for line in txt.lines() {
        let f: Vec<&str> = line.trim().split('|').collect();
        if f.len() < 7 {
            continue;
        }
        if let (Ok(mi), Ok(lid), Ok(bw), Ok(bh)) = (
            f[0].parse::<u16>(),
            f[1].parse::<u16>(),
            f[5].parse::<u16>(),
            f[6].parse::<u16>(),
        ) {
            out.insert((mi, f[2].to_string()), (lid, bw, bh));
        }
    }
    out
}

/// Intern a placed sprite's targetname into the shared logic-name table. Sprite
/// records are written before the table itself, so appending here preserves all
/// already-assigned logic ids and makes STARTON sprites targetable too.
fn intern_logic_name(names: &mut Vec<String>, name: &str) -> Result<u16, String> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(0);
    }
    if let Some(pos) = names.iter().position(|n| n == name) {
        return Ok((pos + 1) as u16);
    }
    if names.len() >= u16::MAX as usize {
        return Err("too many logic names while interning sprites".to_string());
    }
    names.push(name.to_string());
    Ok(names.len() as u16)
}

/// env_sprite / env_glow / cycler_sprite -> compact 12-byte SpriteRec payloads:
/// `(origin i16[3], leaf i16, targetname u16, packed u16)`. GoldSrc starts an
/// env_sprite OFF only when it is named and lacks STARTON; unnamed sprites are
/// visible. Packed bits: id 0..3, initial-on 4, once 5, half-width 6..15.
fn collect_sprite_props(
    ents: &[u8],
    nodes: &[u8],
    planes: &[u8],
    scale: f32,
    map_idx: u16,
    sprites: &std::collections::HashMap<(u16, String), (u16, u16, u16)>,
    logic_names: &mut Vec<String>,
) -> Result<Vec<([i16; 3], i16, u16, u16)>, String> {
    const SF_SPRITE_STARTON: u16 = 1;
    const SF_SPRITE_ONCE: u16 = 2;
    const PACK_INITIAL_ON: u16 = 1 << 4;
    const PACK_ONCE: u16 = 1 << 5;
    let s = entity_text(ents);
    let mut out = Vec::new();
    for block in s.split('{') {
        let cls = ent_value(block, "classname").unwrap_or("");
        if cls != "env_sprite" && cls != "env_glow" && cls != "cycler_sprite" {
            continue;
        }
        let sf = parse_spawnflags(block);
        let targetname = ent_value(block, "targetname").unwrap_or("").trim();
        let name = intern_logic_name(logic_names, targetname)?;
        let start_on =
            cls != "env_sprite" || targetname.is_empty() || (sf & SF_SPRITE_STARTON) != 0;
        let model = ent_value(block, "model").unwrap_or("");
        let base = model
            .rsplit(|c| c == '/' || c == '\\')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let Some(&(lid, bw, _bh)) = sprites.get(&(map_idx, base.clone())) else {
            continue;
        };
        if lid >= 16 {
            return Err(format!("sprite local id {lid} exceeds packed 4-bit limit"));
        }
        let Some(origin_hl) = ent_value(block, "origin").and_then(parse_vec3) else {
            continue;
        };
        let origin = to_world(origin_hl, scale);
        if origin
            .iter()
            .any(|&v| !(i16::MIN as i32..=i16::MAX as i32).contains(&v))
        {
            return Err(format!("sprite {base} origin {origin:?} exceeds i16"));
        }
        let ent_scale = parse_f32_key(block, "scale", 1.0).max(0.05);
        // World half-width = native px/2 * entity scale * (HL->world scale).
        let half = ((bw as f32 * 0.5 * ent_scale) * scale).round().max(1.0) as i32;
        if half > 1023 {
            return Err(format!(
                "sprite {base} half-width {half} exceeds packed 10-bit limit"
            ));
        }
        let mut packed = lid | ((half as u16) << 6);
        if start_on {
            packed |= PACK_INITIAL_ON;
        }
        if sf & SF_SPRITE_ONCE != 0 {
            packed |= PACK_ONCE;
        }
        out.push((
            [origin[0] as i16, origin[1] as i16, origin[2] as i16],
            point_leaf(origin_hl, nodes, planes),
            name,
            packed,
        ));
    }
    Ok(out)
}

/// Is this ambient_generic message a speech voice line (vs looping ambience)?
fn is_voice_message(msg: &str) -> bool {
    let m = msg
        .trim()
        .trim_start_matches(['/', '\\', '*'])
        .to_ascii_lowercase();
    (m.starts_with('!') && m.len() > 1)
        || (m.ends_with(".wav")
            && [
                "barney/",
                "scientist/",
                "gman/",
                "hgrunt/",
                "tride/",
                "vox/",
                "fvox/",
                "holo/",
            ]
            .iter()
            .any(|d| m.starts_with(d)))
}

fn voice_manifest_key(message: &str) -> String {
    let message = message.trim();
    if let Some(sentence) = message.strip_prefix('!') {
        sentence.to_ascii_uppercase()
    } else {
        message
            .trim_start_matches(['/', '\\', '*'])
            .to_ascii_lowercase()
    }
}

fn scripted_sentence_volume(block: &str) -> u8 {
    let volume = parse_f32_key(block, "volume", 0.0);
    // CScriptedSentence::Spawn replaces zero/missing volume with VOL_NORM.
    if volume <= 0.0 {
        100
    } else {
        (volume * 10.0).round().clamp(0.0, 100.0) as u8
    }
}

fn ambient_volume(block: &str) -> u8 {
    let preset = parse_f32_key(block, "preset", 0.0).round() as i32;
    if (1..=27).contains(&preset) {
        // GoldSrc's rgdpvpreset table overrides `health`; presets 7..9 run at
        // 50%, every other retail preset at 100%.
        if (7..=9).contains(&preset) {
            50
        } else {
            100
        }
    } else {
        (parse_f32_key(block, "health", 0.0) * 10.0)
            .round()
            .clamp(0.0, 100.0) as u8
    }
}

fn acoustic_volume(cls: &str, block: &str) -> u8 {
    match cls {
        "scripted_sentence" => scripted_sentence_volume(block),
        // CSpeaker passes `pev->health * 0.1` straight to the mixer; unlike a
        // scripted_sentence, zero is not replaced with VOL_NORM.
        "speaker" => (parse_f32_key(block, "health", 0.0) * 10.0)
            .round()
            .clamp(0.0, 100.0) as u8,
        _ => ambient_volume(block),
    }
}

fn acoustic_attenuation(cls: &str, block: &str, spawnflags: u16, scale: f32) -> u8 {
    // Low three bits select the exact SDK attenuation constant:
    // 0=2.0, 1=1.25, 2=0.8, 3=0, 4=0.3 (speaker). Upper bits retain the
    // power-of-two BSP coordinate shift so runtime distance remains in source
    // Half-Life units even on unusually large maps.
    let mode = match cls {
        "scripted_sentence" => parse_f32_key(block, "attenuation", 0.0)
            .round()
            .clamp(0.0, 3.0) as u8,
        "speaker" => 4,
        "env_beverage" => 2, // CItemSoda::CanThink always uses ATTN_NORM.
        _ if spawnflags & 1 != 0 => 3,
        _ if spawnflags & 2 != 0 => 0,
        _ if spawnflags & 4 != 0 => 1,
        _ if spawnflags & 8 != 0 => 2,
        _ => 1,
    };
    let shift = (scale.max(1.0) as u32).trailing_zeros().min(31) as u8;
    mode | (shift << 3)
}

const BUTTON_SOUNDS: [&str; 26] = [
    "common/null.wav",
    "buttons/button1.wav",
    "buttons/button2.wav",
    "buttons/button3.wav",
    "buttons/button4.wav",
    "buttons/button5.wav",
    "buttons/button6.wav",
    "buttons/button7.wav",
    "buttons/button8.wav",
    "buttons/button9.wav",
    "buttons/button10.wav",
    "buttons/button11.wav",
    "buttons/latchlocked1.wav",
    "buttons/latchunlocked1.wav",
    "buttons/lightswitch2.wav",
    "buttons/button9.wav",
    "buttons/button9.wav",
    "buttons/button9.wav",
    "buttons/button9.wav",
    "buttons/button9.wav",
    "buttons/button9.wav",
    "buttons/lever1.wav",
    "buttons/lever2.wav",
    "buttons/lever3.wav",
    "buttons/lever4.wav",
    "buttons/lever5.wav",
];
const DOOR_MOVE_SOUNDS: [&str; 11] = [
    "common/null.wav",
    "doors/doormove1.wav",
    "doors/doormove2.wav",
    "doors/doormove3.wav",
    "doors/doormove4.wav",
    "doors/doormove5.wav",
    "doors/doormove6.wav",
    "doors/doormove7.wav",
    "doors/doormove8.wav",
    "doors/doormove9.wav",
    "doors/doormove10.wav",
];
const DOOR_STOP_SOUNDS: [&str; 9] = [
    "common/null.wav",
    "doors/doorstop1.wav",
    "doors/doorstop2.wav",
    "doors/doorstop3.wav",
    "doors/doorstop4.wav",
    "doors/doorstop5.wav",
    "doors/doorstop6.wav",
    "doors/doorstop7.wav",
    "doors/doorstop8.wav",
];
const PLAT_MOVE_SOUNDS: [&str; 14] = [
    "common/null.wav",
    "plats/bigmove1.wav",
    "plats/bigmove2.wav",
    "plats/elevmove1.wav",
    "plats/elevmove2.wav",
    "plats/elevmove3.wav",
    "plats/freightmove1.wav",
    "plats/freightmove2.wav",
    "plats/heavymove1.wav",
    "plats/rackmove1.wav",
    "plats/railmove1.wav",
    "plats/squeekmove1.wav",
    "plats/talkmove1.wav",
    "plats/talkmove2.wav",
];
const PLAT_STOP_SOUNDS: [&str; 9] = [
    "common/null.wav",
    "plats/bigstop1.wav",
    "plats/bigstop2.wav",
    "plats/freightstop1.wav",
    "plats/heavystop2.wav",
    "plats/rackstop1.wav",
    "plats/railstop1.wav",
    "plats/squeekstop1.wav",
    "plats/talkstop1.wav",
];
const TRACKTRAIN_SOUNDS: [&str; 7] = [
    "common/null.wav",
    "plats/ttrain1.wav",
    "plats/ttrain2.wav",
    "plats/ttrain3.wav",
    "plats/ttrain4.wav",
    "plats/ttrain6.wav",
    "plats/ttrain7.wav",
];
const FAN_SOUNDS: [&str; 6] = [
    "common/null.wav",
    "fans/fan1.wav",
    "fans/fan2.wav",
    "fans/fan3.wav",
    "fans/fan4.wav",
    "fans/fan5.wav",
];
const BREAK_SOUNDS: [&str; 8] = [
    "debris/bustglass1.wav",
    "debris/bustcrate1.wav",
    "debris/bustmetal1.wav",
    "debris/bustflesh1.wav",
    "debris/bustconcrete1.wav",
    "debris/bustceiling.wav",
    "debris/bustmetal1.wav",
    "debris/bustglass1.wav",
];

fn ordinal_sound<'a>(block: &str, key: &str, table: &'a [&str]) -> &'a str {
    let ordinal = ent_value(block, key)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    table
        .get(ordinal)
        .copied()
        .unwrap_or_else(|| table.last().copied().unwrap_or("common/null.wav"))
}

fn map_audio_id(
    manifest: &std::collections::HashMap<(u16, String), u16>,
    map_index: u16,
    path: &str,
    looping: bool,
) -> u8 {
    let path = path
        .trim()
        .trim_start_matches(['/', '\\', '*'])
        .to_ascii_lowercase();
    if path.is_empty() || path == "common/null.wav" {
        return u8::MAX;
    }
    let mode = if looping { "loop" } else { "shot" };
    manifest
        .get(&(map_index, format!("sfx:{mode}:{path}")))
        .copied()
        .filter(|id| *id < u8::MAX as u16)
        .map(|id| id as u8)
        .unwrap_or(u8::MAX)
}

#[derive(Default)]
struct LogicNames {
    names: Vec<String>,
}

impl LogicNames {
    fn id(&mut self, name: Option<&str>) -> u16 {
        let Some(name) = name else {
            return 0;
        };
        let name = name.trim();
        if name.is_empty() {
            return 0;
        }
        if let Some(pos) = self.names.iter().position(|n| n == name) {
            return (pos + 1).min(u16::MAX as usize) as u16;
        }
        if self.names.len() >= u16::MAX as usize {
            return 0;
        }
        self.names.push(name.to_string());
        self.names.len() as u16
    }
}

#[derive(Clone)]
struct LogicRec {
    kind: u8,
    use_type: u8,
    spawnflags: u16,
    targetname: u16,
    target: u16,
    killtarget: u16,
    brush: u16,
    first_aux: u16,
    aux_count: u8,
    flags: u8,
    wait_ticks: i16,
    delay_ticks: u16,
    speed: u16,
    arg0: u16,
    arg1: u16,
    sound0: u8,
    sound1: u8,
    origin: [i32; 3],
    mins: [i32; 3],
    maxs: [i32; 3],
}

struct LogicAuxRec {
    target: u16,
    delay_ticks: u16,
}

struct LogicCook {
    ents: Vec<LogicRec>,
    aux: Vec<LogicAuxRec>,
    names: Vec<String>,
}

const NAV_NODE_HEIGHT: f32 = 8.0;
const MAX_NAV_NODES_COOK: usize = 255;
const COOKED_NAV_NODE_BYTES: usize = cooked::NAV_NODE_RECORD_SIZE;
const NAV_LINKS_PER_NODE: usize = 8;
const NAV_LINK_RANGE2: f32 = 1024.0 * 1024.0;
const NAV_LINK_TRACE_LIFT: f32 = 24.0;
const NAV_LINK_VERTICAL_MAX: f32 = 128.0;
const CONTENTS_SOLID: i16 = -2;
const NAV_NODE_LAND: u8 = 1;
const NAV_EXACT_ROUTES: u16 = 0x8000;
const NAV_ROUTE_BYTES_MAX: usize = (NAV_EXACT_ROUTES - 1) as usize;

// Retail Half-Life writes its 32-bit CGraph ABI image straight to `.nod`.
// Parse fixed little-endian offsets explicitly: using host Rust/C layouts would
// break on 64-bit machines and on any compiler with different padding.
const RETAIL_GRAPH_VERSION: i32 = 16;
const RETAIL_GRAPH_BYTES: usize = 8396;
const RETAIL_NODE_BYTES: usize = 88;
const RETAIL_LINK_BYTES: usize = 24;
const RETAIL_DIST_BYTES: usize = 16;
const RETAIL_GRAPH_NODES: usize = 24;
const RETAIL_GRAPH_LINKS: usize = 28;
const RETAIL_GRAPH_ROUTE_BYTES: usize = 32;
const RETAIL_GRAPH_HASH_LINKS: usize = 8384;
const RETAIL_NODE_HUMAN_DOOR_ROUTE: usize = 40 + (1 * 2 + 1) * 4;
const RETAIL_LINK_HUMAN: i32 = 1 << 1;

struct NavNodeRec {
    origin_hl: [f32; 3],
    origin: [i32; 3],
    leaf: i16,
    links: Vec<u8>,
    route_offset: u16,
    node_type: u8,
}

struct NavCook {
    nodes: Vec<NavNodeRec>,
    /// Exact GoldSrc compressed next-hop streams. `None` retains the synthetic
    /// adjacency fallback used by custom maps that ship no compatible `.nod`.
    routes: Option<Vec<u8>>,
}

struct RetailNavNode {
    origin: [f32; 3],
    peek: [f32; 3],
    node_type: u8,
    first_link: usize,
    link_count: usize,
    route_offset: usize,
}

struct RetailNavLink {
    source: usize,
    dest: usize,
    mask: i32,
}

fn bbox_plane_sides(mins: [f32; 3], maxs: [f32; 3], normal: [f32; 3], dist: f32) -> i32 {
    let mut front = 0.0;
    let mut back = 0.0;
    for i in 0..3 {
        if normal[i] >= 0.0 {
            front += normal[i] * maxs[i];
            back += normal[i] * mins[i];
        } else {
            front += normal[i] * mins[i];
            back += normal[i] * maxs[i];
        }
    }

    let mut sides = 0;
    if front >= dist {
        sides |= 1;
    }
    if back < dist {
        sides |= 2;
    }
    sides
}

fn push_leaf(out: &mut Vec<u16>, leaf: i32) {
    if leaf <= 0 || leaf > u16::MAX as i32 {
        return;
    }
    let leaf = leaf as u16;
    if !out.contains(&leaf) {
        out.push(leaf);
    }
}

fn split_bbox_leafs(
    node_idx: i32,
    mins: [f32; 3],
    maxs: [f32; 3],
    nodes: &[u8],
    planes: &[u8],
    out: &mut Vec<u16>,
) {
    if node_idx < 0 {
        push_leaf(out, -node_idx - 1);
        return;
    }
    let ni = node_idx as usize;
    if ni >= nodes.len() / SZ_NODE {
        return;
    }

    let no = ni * SZ_NODE;
    let planenum = i32le(nodes, no).unwrap_or(0).max(0) as usize;
    if planenum >= planes.len() / SZ_PLANE {
        return;
    }
    let po = planenum * SZ_PLANE;
    let normal = [
        f32le(planes, po).unwrap_or(0.0),
        f32le(planes, po + 4).unwrap_or(0.0),
        f32le(planes, po + 8).unwrap_or(0.0),
    ];
    let dist = f32le(planes, po + 12).unwrap_or(0.0);
    let sides = bbox_plane_sides(mins, maxs, normal, dist);
    let child0 = i16::from_le_bytes([nodes[no + 4], nodes[no + 5]]) as i32;
    let child1 = i16::from_le_bytes([nodes[no + 6], nodes[no + 7]]) as i32;
    if sides & 1 != 0 {
        split_bbox_leafs(child0, mins, maxs, nodes, planes, out);
    }
    if sides & 2 != 0 {
        split_bbox_leafs(child1, mins, maxs, nodes, planes, out);
    }
}

fn entity_leafs(
    mins: [f32; 3],
    maxs: [f32; 3],
    origin: [f32; 3],
    mv: Option<[f32; 3]>,
    nodes: &[u8],
    planes: &[u8],
) -> Vec<u16> {
    let mut out = Vec::new();
    let add_box = |offset: [f32; 3], out: &mut Vec<u16>| {
        let emins = [
            mins[0] + origin[0] + offset[0],
            mins[1] + origin[1] + offset[1],
            mins[2] + origin[2] + offset[2],
        ];
        let emaxs = [
            maxs[0] + origin[0] + offset[0],
            maxs[1] + origin[1] + offset[1],
            maxs[2] + origin[2] + offset[2],
        ];
        split_bbox_leafs(0, emins, emaxs, nodes, planes, out);
    };

    add_box([0.0; 3], &mut out);
    if let Some(mv) = mv {
        add_box(mv, &mut out);
    }
    out.sort_unstable();
    out
}

/// True when `to`'s PVS bit is set in `from`'s decompressed vis row.
fn leaf_row_sees(leaves: &[u8], vis: &[u8], n_visleaves: usize, from: usize, to: usize) -> bool {
    let n_leaves = leaves.len() / SZ_LEAF;
    if from == 0
        || to == 0
        || from > n_visleaves
        || to > n_visleaves
        || from >= n_leaves
        || to >= n_leaves
    {
        return false;
    }
    let visofs = i32le(leaves, from * SZ_LEAF + SZ_LEAF_VISOFS).unwrap_or(-1);
    if visofs < 0 {
        return true; // no vis data = everything visible
    }
    let bit = to - 1;
    let want_byte = bit >> 3;
    let mut v = visofs as usize;
    let mut c = 0usize;
    while v < vis.len() {
        if vis[v] != 0 {
            if c == want_byte {
                return vis[v] & (1 << (bit & 7)) != 0;
            }
            v += 1;
            c += 1;
        } else {
            v += 1;
            if v >= vis.len() {
                break;
            }
            c += vis[v] as usize;
            if c > want_byte {
                return false; // inside a zero run
            }
            v += 1;
        }
    }
    false
}

/// GoldSrc enables transparent world water per map, not per polygon: if any
/// liquid leaf can see an empty leaf, the VIS data was compiled across the
/// liquid boundary and every turbulent surface may be blended safely. Match
/// that rule instead of probing a face center, which misclassified fragmented
/// liquid brushes and made their opaque pieces fight subdivided floor tris in
/// the PS1 ordering table.
fn map_supports_water_alpha(leaves: &[u8], vis: &[u8], n_visleaves: usize) -> bool {
    if vis.is_empty() {
        return true;
    }
    let n_leaves = (leaves.len() / SZ_LEAF).min(n_visleaves.saturating_add(1));
    for from in 1..n_leaves {
        let contents = i32le(leaves, from * SZ_LEAF).unwrap_or(-2);
        if contents != -3 && contents != -4 {
            continue; // GoldSrc checks CONTENTS_WATER and CONTENTS_SLIME.
        }
        for to in 1..n_leaves {
            if i32le(leaves, to * SZ_LEAF).unwrap_or(-2) == -1
                && leaf_row_sees(leaves, vis, n_visleaves, from, to)
            {
                return true;
            }
        }
    }
    false
}

/// `point_leaf` from an arbitrary BSP head node (entity brush models own
/// sub-trees of the shared node array; the world tree starts at node 0).
fn point_leaf_from(head: i32, point: [f32; 3], nodes: &[u8], planes: &[u8]) -> i16 {
    let mut node_idx = head;
    let mut guard = 0;
    while node_idx >= 0 && guard < 256 {
        guard += 1;
        let ni = node_idx as usize;
        if ni >= nodes.len() / SZ_NODE {
            return 0;
        }
        let no = ni * SZ_NODE;
        let planenum = i32le(nodes, no).unwrap_or(0).max(0) as usize;
        if planenum >= planes.len() / SZ_PLANE {
            return 0;
        }
        let po = planenum * SZ_PLANE;
        let nx = f32le(planes, po).unwrap_or(0.0);
        let ny = f32le(planes, po + 4).unwrap_or(0.0);
        let nz = f32le(planes, po + 8).unwrap_or(0.0);
        let dist = f32le(planes, po + 12).unwrap_or(0.0);
        let side = point[0] * nx + point[1] * ny + point[2] * nz - dist;
        let child0 = i16::from_le_bytes([nodes[no + 4], nodes[no + 5]]) as i32;
        let child1 = i16::from_le_bytes([nodes[no + 6], nodes[no + 7]]) as i32;
        node_idx = if side >= 0.0 { child0 } else { child1 };
    }
    if node_idx < 0 {
        (-node_idx - 1).clamp(0, i16::MAX as i32) as i16
    } else {
        0
    }
}

fn point_leaf(point: [f32; 3], nodes: &[u8], planes: &[u8]) -> i16 {
    let mut node_idx = 0i32;
    let mut guard = 0;
    while node_idx >= 0 && guard < 256 {
        guard += 1;
        let ni = node_idx as usize;
        if ni >= nodes.len() / SZ_NODE {
            return 0;
        }
        let no = ni * SZ_NODE;
        let planenum = i32le(nodes, no).unwrap_or(0).max(0) as usize;
        if planenum >= planes.len() / SZ_PLANE {
            return 0;
        }
        let po = planenum * SZ_PLANE;
        let nx = f32le(planes, po).unwrap_or(0.0);
        let ny = f32le(planes, po + 4).unwrap_or(0.0);
        let nz = f32le(planes, po + 8).unwrap_or(0.0);
        let dist = f32le(planes, po + 12).unwrap_or(0.0);
        let side = point[0] * nx + point[1] * ny + point[2] * nz - dist;
        let child0 = i16::from_le_bytes([nodes[no + 4], nodes[no + 5]]) as i32;
        let child1 = i16::from_le_bytes([nodes[no + 6], nodes[no + 7]]) as i32;
        node_idx = if side >= 0.0 { child0 } else { child1 };
    }
    if node_idx < 0 {
        (-node_idx - 1).clamp(0, i16::MAX as i32) as i16
    } else {
        0
    }
}

fn clip_plane(clipnodes: &[u8], planes: &[u8], clip_idx: usize) -> Option<([f32; 3], f32)> {
    let co = clip_idx.checked_mul(SZ_CLIPNODE)?;
    if co + SZ_CLIPNODE > clipnodes.len() {
        return None;
    }
    let planenum = i32le(clipnodes, co)?.max(0) as usize;
    let po = planenum.checked_mul(SZ_PLANE)?;
    if po + SZ_PLANE > planes.len() {
        return None;
    }
    Some((
        [
            f32le(planes, po).unwrap_or(0.0),
            f32le(planes, po + 4).unwrap_or(0.0),
            f32le(planes, po + 8).unwrap_or(0.0),
        ],
        f32le(planes, po + 12).unwrap_or(0.0),
    ))
}

fn clip_child(clipnodes: &[u8], clip_idx: usize, child: usize) -> i16 {
    let co = clip_idx * SZ_CLIPNODE + 4 + child * 2;
    i16::from_le_bytes([clipnodes[co], clipnodes[co + 1]])
}

fn point_contents_raw(clipnodes: &[u8], planes: &[u8], mut node_idx: i16, p: [f32; 3]) -> i16 {
    let mut guard = 0;
    while node_idx >= 0 && guard < 256 {
        guard += 1;
        let ci = node_idx as usize;
        let Some((n, d)) = clip_plane(clipnodes, planes, ci) else {
            return -1;
        };
        let side = p[0] * n[0] + p[1] * n[1] + p[2] * n[2] - d;
        node_idx = if side >= 0.0 {
            clip_child(clipnodes, ci, 0)
        } else {
            clip_child(clipnodes, ci, 1)
        };
    }
    node_idx
}

fn segment_clear_raw(
    clipnodes: &[u8],
    planes: &[u8],
    node_idx: i16,
    p1: [f32; 3],
    p2: [f32; 3],
    depth: u8,
) -> bool {
    if depth > 80 {
        return true;
    }
    if node_idx < 0 {
        return node_idx != CONTENTS_SOLID;
    }
    let ci = node_idx as usize;
    let Some((n, d)) = clip_plane(clipnodes, planes, ci) else {
        return true;
    };
    let t1 = p1[0] * n[0] + p1[1] * n[1] + p1[2] * n[2] - d;
    let t2 = p2[0] * n[0] + p2[1] * n[1] + p2[2] * n[2] - d;
    if t1 >= 0.0 && t2 >= 0.0 {
        return segment_clear_raw(
            clipnodes,
            planes,
            clip_child(clipnodes, ci, 0),
            p1,
            p2,
            depth + 1,
        );
    }
    if t1 < 0.0 && t2 < 0.0 {
        return segment_clear_raw(
            clipnodes,
            planes,
            clip_child(clipnodes, ci, 1),
            p1,
            p2,
            depth + 1,
        );
    }

    let denom = t1 - t2;
    let frac = if denom.abs() <= f32::EPSILON {
        0.0
    } else {
        (t1 / denom).clamp(0.0, 1.0)
    };
    let mid = [
        p1[0] + (p2[0] - p1[0]) * frac,
        p1[1] + (p2[1] - p1[1]) * frac,
        p1[2] + (p2[2] - p1[2]) * frac,
    ];
    let side = t1 < 0.0;
    let near = clip_child(clipnodes, ci, if side { 1 } else { 0 });
    let far = clip_child(clipnodes, ci, if side { 0 } else { 1 });
    if !segment_clear_raw(clipnodes, planes, near, p1, mid, depth + 1) {
        return false;
    }
    if point_contents_raw(clipnodes, planes, far, mid) == CONTENTS_SOLID {
        return false;
    }
    segment_clear_raw(clipnodes, planes, far, mid, p2, depth + 1)
}

fn nav_segment_clear(
    clipnodes: &[u8],
    planes: &[u8],
    hull1_head: i32,
    a: [f32; 3],
    b: [f32; 3],
) -> bool {
    if hull1_head < 0 || hull1_head as usize >= clipnodes.len() / SZ_CLIPNODE {
        return true;
    }
    segment_clear_raw(clipnodes, planes, hull1_head as i16, a, b, 0)
}

fn add_nav_link(nodes: &mut [NavNodeRec], a: usize, b: usize) {
    if a == b || a >= nodes.len() || b >= nodes.len() || b > u8::MAX as usize {
        return;
    }
    let b = b as u8;
    if !nodes[a].links.contains(&b) {
        nodes[a].links.push(b);
    }
}

fn nav_dist2_hl(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

fn collect_nav_nodes(
    ents: &[u8],
    nodes_lump: &[u8],
    planes: &[u8],
    clipnodes: &[u8],
    hull1_head: i32,
    scale: f32,
) -> Vec<NavNodeRec> {
    let s = entity_text(ents);
    let mut out = Vec::new();
    // Only MoveTo=4 teleports an auto-start scripted actor to the mark.
    // MoveTo=0 waits at its authored origin; 1/2 walk/run there at runtime.
    let mut script_marks: Vec<(String, [f32; 3], Option<f32>)> = Vec::new();
    for block in s.split('{') {
        if ent_value(block, "classname") != Some("scripted_sequence") {
            continue;
        }
        if ent_value(block, "targetname").is_some() {
            continue;
        }
        let move_to = ent_value(block, "m_fMoveTo")
            .or_else(|| ent_value(block, "m_flMoveTo"))
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        if move_to != 4 {
            continue;
        }
        let Some(target) = ent_value(block, "m_iszEntity") else {
            continue;
        };
        let Some(origin) = ent_value(block, "origin").and_then(parse_vec3) else {
            continue;
        };
        script_marks.push((target.to_string(), origin, ent_yaw_degrees(block)));
    }
    for block in s.split('{') {
        if ent_value(block, "classname").unwrap_or("") != "info_node" {
            continue;
        }
        let mut origin_hl = ent_value(block, "origin")
            .and_then(parse_vec3)
            .unwrap_or([0.0; 3]);
        origin_hl[2] += NAV_NODE_HEIGHT;
        let origin = to_world(origin_hl, scale);
        let leaf = point_leaf(origin_hl, nodes_lump, planes);
        out.push(NavNodeRec {
            origin_hl,
            origin,
            leaf,
            links: Vec::new(),
            route_offset: 0,
            node_type: NAV_NODE_LAND,
        });
        if out.len() >= MAX_NAV_NODES_COOK {
            break;
        }
    }

    let n = out.len();
    for i in 0..n {
        let mut candidates: Vec<(usize, f32)> = Vec::new();
        for j in 0..n {
            if i == j {
                continue;
            }
            let a = out[i].origin_hl;
            let b = out[j].origin_hl;
            if (a[2] - b[2]).abs() > NAV_LINK_VERTICAL_MAX {
                continue;
            }
            let d2 = nav_dist2_hl(a, b);
            if d2 > NAV_LINK_RANGE2 {
                continue;
            }
            let mut ta = a;
            let mut tb = b;
            ta[2] += NAV_LINK_TRACE_LIFT;
            tb[2] += NAV_LINK_TRACE_LIFT;
            if nav_segment_clear(clipnodes, planes, hull1_head, ta, tb) {
                candidates.push((j, d2));
            }
        }
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        for &(j, _) in candidates.iter().take(NAV_LINKS_PER_NODE) {
            add_nav_link(&mut out, i, j);
            add_nav_link(&mut out, j, i);
        }
    }

    for i in 0..n {
        let origin = out[i].origin_hl;
        let mut links = std::mem::take(&mut out[i].links);
        links.sort_by(|&a, &b| {
            let da = nav_dist2_hl(origin, out[a as usize].origin_hl);
            let db = nav_dist2_hl(origin, out[b as usize].origin_hl);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });
        links.dedup();
        links.truncate(NAV_LINKS_PER_NODE);
        out[i].links = links;
    }

    out
}

fn retail_route_row(
    route: &[u8],
    offset: usize,
    node_count: usize,
    source: usize,
) -> Result<(Vec<u8>, Vec<usize>), String> {
    if offset >= route.len() && node_count != 0 {
        return Err(format!(
            "route offset {offset} exceeds {} bytes",
            route.len()
        ));
    }
    let mut encoded = Vec::new();
    let mut decoded = Vec::with_capacity(node_count);
    let mut p = offset;
    while decoded.len() < node_count {
        let raw = *route.get(p).ok_or_else(|| {
            format!(
                "route row {source} ends before destination {}",
                decoded.len()
            )
        })?;
        p += 1;
        encoded.push(raw);
        let phrase = raw as i8;
        if phrase < 0 {
            let count = -(phrase as i16) as usize;
            if count == 0 || decoded.len() + count > node_count {
                return Err(format!("route row {source} has invalid direct run {count}"));
            }
            let first = decoded.len();
            decoded.extend(first..first + count);
        } else {
            let delta_raw = *route
                .get(p)
                .ok_or_else(|| format!("route row {source} is missing a repeat delta"))?;
            p += 1;
            encoded.push(delta_raw);
            let count = phrase as usize + 1;
            if decoded.len() + count > node_count {
                return Err(format!("route row {source} has invalid repeat run {count}"));
            }
            let delta = delta_raw as i8 as i32;
            let next = (source as i32 + delta).rem_euclid(node_count as i32) as usize;
            decoded.resize(decoded.len() + count, next);
        }
    }
    Ok((encoded, decoded))
}

fn parse_retail_nav(
    data: &[u8],
    nodes_lump: &[u8],
    planes: &[u8],
    scale: f32,
) -> Result<NavCook, String> {
    if i32le(data, 0) != Some(RETAIL_GRAPH_VERSION) {
        return Err("not a retail version-16 graph".to_string());
    }
    let graph = 4usize;
    let read_count = |offset: usize, label: &str| -> Result<usize, String> {
        let value = i32le(data, graph + offset)
            .ok_or_else(|| format!("retail graph is missing {label}"))?;
        if value < 0 {
            Err(format!("retail graph has negative {label} {value}"))
        } else {
            Ok(value as usize)
        }
    };
    let node_count = read_count(RETAIL_GRAPH_NODES, "node count")?;
    let link_count = read_count(RETAIL_GRAPH_LINKS, "link count")?;
    let route_len = read_count(RETAIL_GRAPH_ROUTE_BYTES, "route byte count")?;
    let hash_count = read_count(RETAIL_GRAPH_HASH_LINKS, "hash-link count")?;
    if node_count > MAX_NAV_NODES_COOK {
        return Err(format!(
            "retail graph has {node_count} nodes, runtime cap is {MAX_NAV_NODES_COOK}"
        ));
    }
    if i32le(data, graph + 8) != Some(1) {
        return Err("retail graph has no completed routing table".to_string());
    }

    let nodes_off = graph
        .checked_add(RETAIL_GRAPH_BYTES)
        .ok_or_else(|| "retail graph node offset overflow".to_string())?;
    let links_off = nodes_off
        .checked_add(node_count.saturating_mul(RETAIL_NODE_BYTES))
        .ok_or_else(|| "retail graph link offset overflow".to_string())?;
    let dist_off = links_off
        .checked_add(link_count.saturating_mul(RETAIL_LINK_BYTES))
        .ok_or_else(|| "retail graph distance offset overflow".to_string())?;
    let route_off = dist_off
        .checked_add(node_count.saturating_mul(RETAIL_DIST_BYTES))
        .ok_or_else(|| "retail graph route offset overflow".to_string())?;
    let hash_off = route_off
        .checked_add(route_len)
        .ok_or_else(|| "retail graph hash offset overflow".to_string())?;
    let expected = hash_off
        .checked_add(hash_count.saturating_mul(2))
        .ok_or_else(|| "retail graph size overflow".to_string())?;
    if expected != data.len() {
        return Err(format!(
            "retail graph length is {}, fixed-layout decode expects {expected}",
            data.len()
        ));
    }
    if route_len > NAV_ROUTE_BYTES_MAX {
        // We repack one of the eight tables below, but reject absurd source
        // metadata before slicing it.
        if route_off + route_len > data.len() {
            return Err("retail graph route section is truncated".to_string());
        }
    }

    let mut original_nodes = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let o = nodes_off + i * RETAIL_NODE_BYTES;
        let origin = [
            f32le(data, o).ok_or_else(|| format!("node {i} has no X origin"))?,
            f32le(data, o + 4).ok_or_else(|| format!("node {i} has no Y origin"))?,
            f32le(data, o + 8).ok_or_else(|| format!("node {i} has no Z origin"))?,
        ];
        let peek = [
            f32le(data, o + 12).ok_or_else(|| format!("node {i} has no peek X"))?,
            f32le(data, o + 16).ok_or_else(|| format!("node {i} has no peek Y"))?,
            f32le(data, o + 20).ok_or_else(|| format!("node {i} has no peek Z"))?,
        ];
        if !origin.iter().chain(peek.iter()).all(|v| v.is_finite()) {
            return Err(format!("node {i} contains a non-finite coordinate"));
        }
        let node_type = i32le(data, o + 28).unwrap_or(0) as u8;
        let links = read_count_at(data, o + 32, "node link count")?;
        let first = read_count_at(data, o + 36, "node first link")?;
        if first.checked_add(links).is_none_or(|end| end > link_count) {
            return Err(format!(
                "node {i} link range {first}+{links} exceeds {link_count}"
            ));
        }
        let route_offset = read_count_at(
            data,
            o + RETAIL_NODE_HUMAN_DOOR_ROUTE,
            "node human route offset",
        )?;
        original_nodes.push(RetailNavNode {
            origin,
            peek,
            node_type,
            first_link: first,
            link_count: links,
            route_offset,
        });
    }

    let mut original_links = Vec::with_capacity(link_count);
    for i in 0..link_count {
        let o = links_off + i * RETAIL_LINK_BYTES;
        let source = read_count_at(data, o, "link source")?;
        let dest = read_count_at(data, o + 4, "link destination")?;
        let mask = i32le(data, o + 16).ok_or_else(|| format!("link {i} has no mask"))?;
        if source >= node_count || dest >= node_count {
            return Err(format!(
                "link {i} references {source}->{dest} outside {node_count} nodes"
            ));
        }
        original_links.push(RetailNavLink { source, dest, mask });
    }

    let source_route = &data[route_off..route_off + route_len];
    let mut routes = Vec::new();
    let mut dedup: HashMap<Vec<u8>, u16> = HashMap::new();
    let mut nodes = Vec::with_capacity(node_count);
    for (source, raw_node) in original_nodes.iter().enumerate() {
        let (row, decoded) =
            retail_route_row(source_route, raw_node.route_offset, node_count, source)?;
        for &next in &decoded {
            if next == source {
                continue; // unreachable/self entries intentionally stay put
            }
            let usable = original_links
                [raw_node.first_link..raw_node.first_link + raw_node.link_count]
                .iter()
                .any(|link| {
                    link.source == source && link.dest == next && link.mask & RETAIL_LINK_HUMAN != 0
                });
            if !usable {
                return Err(format!(
                    "node {source} route selects non-human or non-adjacent next hop {next}"
                ));
            }
        }
        let route_offset = if let Some(&offset) = dedup.get(&row) {
            offset
        } else {
            if routes.len() > NAV_ROUTE_BYTES_MAX
                || routes.len().saturating_add(row.len()) > NAV_ROUTE_BYTES_MAX
            {
                return Err(format!(
                    "repacked human route table exceeds {NAV_ROUTE_BYTES_MAX} bytes"
                ));
            }
            let offset = routes.len() as u16;
            routes.extend_from_slice(&row);
            dedup.insert(row, offset);
            offset
        };
        nodes.push(NavNodeRec {
            origin_hl: raw_node.origin,
            origin: to_world(raw_node.origin, scale),
            leaf: point_leaf(raw_node.peek, nodes_lump, planes),
            links: Vec::new(),
            route_offset,
            node_type: raw_node.node_type,
        });
    }
    Ok(NavCook {
        nodes,
        routes: Some(routes),
    })
}

fn read_count_at(data: &[u8], offset: usize, label: &str) -> Result<usize, String> {
    let value = i32le(data, offset).ok_or_else(|| format!("retail graph is missing {label}"))?;
    if value < 0 {
        Err(format!("retail graph has negative {label} {value}"))
    } else {
        Ok(value as usize)
    }
}

fn collect_nav(
    bsp_path: &str,
    ents: &[u8],
    nodes_lump: &[u8],
    planes: &[u8],
    clipnodes: &[u8],
    hull1_head: i32,
    scale: f32,
) -> NavCook {
    let bsp_path = Path::new(bsp_path);
    let nod_path = bsp_path.parent().and_then(|parent| {
        let stem = bsp_path.file_stem()?.to_str()?;
        Some(parent.join("graphs").join(format!("{stem}.nod")))
    });
    if let Some(path) = nod_path {
        match std::fs::read(&path) {
            Ok(data) => match parse_retail_nav(&data, nodes_lump, planes, scale) {
                Ok(nav) => return nav,
                Err(error) => eprintln!(
                    "warn: {}: cannot use authoritative GoldSrc graph ({error}); synthesizing links",
                    path.display()
                ),
            },
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                eprintln!("warn: {}: {error}; synthesizing links", path.display());
            }
            Err(_) => {}
        }
    }
    let nodes = collect_nav_nodes(ents, nodes_lump, planes, clipnodes, hull1_head, scale);
    if !nodes.is_empty() {
        eprintln!(
            "warn: {}: no compatible retail .nod; {} synthesized navigation nodes are not Gold-exact",
            bsp_path.display(),
            nodes.len()
        );
    }
    NavCook {
        nodes,
        routes: None,
    }
}

/// func_door move direction (HL) + distance. GoldSrc evaluates
/// `pev->size - 2`, but SET_MODEL's linked brush size is two units larger than
/// the BSP dmodel bounds read here. The terms cancel, leaving the raw cooked
/// model size minus lip. Subtracting two again made every PS1 door/lift short.
fn door_move(dir: [f32; 3], mins: [f32; 3], maxs: [f32; 3], lip: f32) -> ([f32; 3], f32) {
    let sz = [maxs[0] - mins[0], maxs[1] - mins[1], maxs[2] - mins[2]];
    // Equivalent to doors.cpp after accounting for SET_MODEL's link padding.
    let dist = (dir[0] * sz[0]).abs() + (dir[1] * sz[1]).abs() + (dir[2] * sz[2]).abs() - lip;
    (dir, dist)
}

fn parse_spawnflags_u32(block: &str) -> u32 {
    ent_value(block, "spawnflags")
        .and_then(|v| v.parse::<u32>().ok())
        // GoldSrc stores spawnflags in a signed 32-bit entvar. Some maps write
        // high-bit flags (for example SF_DOOR_SILENT) as a negative decimal.
        .or_else(|| {
            ent_value(block, "spawnflags")
                .and_then(|v| v.parse::<i32>().ok())
                .map(|v| v as u32)
        })
        .unwrap_or(0)
}

fn parse_spawnflags(block: &str) -> u16 {
    // Compact room records retain the low SDK flag word. Truncation is
    // intentional; saturating values above 65535 to 0xffff fabricated every
    // low flag at once and made an unrelated high flag change entity behavior.
    parse_spawnflags_u32(block) as u16
}

fn parse_f32_key(block: &str, key: &str, default: f32) -> f32 {
    ent_value(block, key)
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(default)
}

fn seconds_to_ticks_u16(seconds: f32) -> u16 {
    if seconds <= 0.0 {
        return 0;
    }
    (seconds * 20.0).round().clamp(0.0, u16::MAX as f32) as u16
}

fn seconds_to_ticks_i16(seconds: f32) -> i16 {
    if seconds < 0.0 {
        return -1;
    }
    (seconds * 20.0).round().clamp(0.0, i16::MAX as f32) as i16
}

#[inline]
fn pack_train_corner_wait(wait_seconds: f32, spawnflags: u16) -> u16 {
    let wait_for_trigger = wait_seconds < 0.0 || spawnflags & 1 != 0;
    let teleport = spawnflags & 2 != 0;
    if wait_for_trigger {
        if teleport {
            TRAIN_CORNER_WAIT_TRIGGER_TELEPORT
        } else {
            TRAIN_CORNER_WAIT_TRIGGER
        }
    } else {
        let wait = seconds_to_ticks_u16(wait_seconds).min(0x7ffd);
        wait | if teleport { TRAIN_CORNER_TELEPORT } else { 0 }
    }
}

fn triggerstate_use_type(block: &str) -> u8 {
    match ent_value(block, "triggerstate")
        .and_then(|v| v.parse::<i32>().ok())
        // The SDK stores CAutoTrigger/CTriggerRelay::triggerType in
        // zero-initialized entity memory. With no authored key the enum is
        // therefore USE_OFF (0), not Hammer's commonly-authored USE_ON (1).
        .unwrap_or(0)
    {
        0 => USE_OFF,
        2 => USE_TOGGLE,
        _ => USE_ON,
    }
}

fn block_model(block: &str) -> Option<usize> {
    let model = ent_value(block, "model")?;
    let rest = model.strip_prefix('*')?;
    let submodel = rest.parse::<usize>().ok()?;
    if submodel == 0 {
        None
    } else {
        Some(submodel)
    }
}

fn model_bounds_hl(models: &[u8], submodel: usize) -> Option<([f32; 3], [f32; 3])> {
    if submodel >= models.len() / SZ_MODEL {
        return None;
    }
    let mo = submodel * SZ_MODEL;
    let g = |o: usize| f32le(models, mo + o).unwrap_or(0.0);
    Some(([g(0), g(4), g(8)], [g(12), g(16), g(20)]))
}

fn transform_bounds_to_world(
    mins: [f32; 3],
    maxs: [f32; 3],
    origin: [f32; 3],
    scale: f32,
) -> ([i32; 3], [i32; 3]) {
    let mut wmin = [i32::MAX; 3];
    let mut wmax = [i32::MIN; 3];
    for &x in &[mins[0], maxs[0]] {
        for &y in &[mins[1], maxs[1]] {
            for &z in &[mins[2], maxs[2]] {
                let p = to_world([x + origin[0], y + origin[1], z + origin[2]], scale);
                for axis in 0..3 {
                    wmin[axis] = wmin[axis].min(p[axis]);
                    wmax[axis] = wmax[axis].max(p[axis]);
                }
            }
        }
    }
    (wmin, wmax)
}

fn entity_bounds_world(block: &str, models: &[u8], scale: f32) -> ([i32; 3], [i32; 3], [i32; 3]) {
    let origin_hl = ent_value(block, "origin")
        .and_then(parse_vec3)
        .unwrap_or([0.0; 3]);
    let origin = to_world(origin_hl, scale);
    if let Some(submodel) = block_model(block) {
        if let Some((mins, maxs)) = model_bounds_hl(models, submodel) {
            let (wmins, wmaxs) = transform_bounds_to_world(mins, maxs, origin_hl, scale);
            return (origin, wmins, wmaxs);
        }
    }
    (origin, origin, origin)
}

fn logic_common_key(key: &str) -> bool {
    matches!(
        key,
        "classname"
            | "model"
            | "origin"
            | "angle"
            | "angles"
            | "targetname"
            | "globalname"
            | "target"
            | "killtarget"
            | "delay"
            | "wait"
            | "speed"
            | "lip"
            | "spawnflags"
            | "renderamt"
            | "rendercolor"
            | "rendermode"
            | "renderfx"
            | "sounds"
            | "health"
            | "damage"
            | "damagetype"
            | "dmg"
            | "message"
            | "master"
            | "noise"
            | "netname"
            | "triggerstate"
            | "map"
            | "landmark"
            | "changetarget"
            | "m_iszNewTarget"
            | "changedelay"
            | "count"
            | "type"
    ) || key.starts_with('_')
}

fn iter_ent_pairs(block: &str, mut f: impl FnMut(&str, &str)) {
    let mut rest = block;
    while let Some(k0) = rest.find('"') {
        let rest1 = &rest[k0 + 1..];
        let Some(k1) = rest1.find('"') else {
            break;
        };
        let key = &rest1[..k1];
        let rest2 = &rest1[k1 + 1..];
        let Some(v0) = rest2.find('"') else {
            break;
        };
        let rest3 = &rest2[v0 + 1..];
        let Some(v1) = rest3.find('"') else {
            break;
        };
        let value = &rest3[..v1];
        f(key, value);
        rest = &rest3[v1 + 1..];
    }
}

#[inline]
fn multi_manager_target_key(key: &str) -> &str {
    key.split_once('#').map_or(key, |(base, _)| base)
}

/// A named door is inert unless something can actually call its Use method.
/// GoldSrc's DoorTouch rejects named doors, and PlayerUse sees a door only
/// with SF_DOOR_USE_ONLY. Preserve a real incoming target (including manager
/// keys), but omit unreachable decorative rotators such as c1a0's `introchair`
/// from the live logic table while retaining their authored render geometry.
fn named_door_has_activation(all_entities: &str, name: &str, spawnflags: u32) -> bool {
    if name.is_empty() || spawnflags & 256 != 0 {
        return true;
    }
    for block in all_entities.split('{') {
        if ent_value(block, "target") == Some(name)
            || ent_value(block, "killtarget") == Some(name)
            || ent_value(block, "m_iszNewTarget") == Some(name)
            || ent_value(block, "changetarget") == Some(name)
        {
            return true;
        }
        if ent_value(block, "classname") == Some("multi_manager") {
            let mut found = false;
            iter_ent_pairs(block, |key, _| {
                if !logic_common_key(key) && multi_manager_target_key(key) == name {
                    found = true;
                }
            });
            if found {
                return true;
            }
        }
    }
    false
}

/// PVS membership for every authored func_train stop. A train brush is stored
/// in model-local space and teleported to its first path_corner at spawn, so
/// using only the raw entity origin makes terminal lifts disappear as soon as
/// they leave that leaf (c1a1c's main elevator). The union costs only the leaf
/// ids actually touched by the authored stops and no runtime state.
fn moving_brush_leafs(
    all_entities: &str,
    train_block: &str,
    path_class: &str,
    mins: [f32; 3],
    maxs: [f32; 3],
    fallback_origin: [f32; 3],
    nodes: &[u8],
    planes: &[u8],
) -> Vec<u16> {
    let center = [
        (mins[0] + maxs[0]) * 0.5,
        (mins[1] + maxs[1]) * 0.5,
        (mins[2] + maxs[2]) * 0.5,
    ];
    let mut corner = ent_value(train_block, "target").unwrap_or("").to_string();
    let mut previous = None;
    let mut seen = Vec::<(String, [f32; 3], bool)>::new();
    let mut out = Vec::new();
    let mut hops = 0usize;
    while !corner.is_empty() && hops < 80 {
        let mut found = false;
        for block in all_entities.split('{') {
            if ent_value(block, "classname") != Some(path_class)
                || ent_value(block, "targetname") != Some(corner.as_str())
            {
                continue;
            }
            let origin = ent_value(block, "origin")
                .and_then(parse_vec3)
                .unwrap_or(fallback_origin);
            let teleport = parse_spawnflags(block) & 2 != 0;
            let add_sweep = |from: Option<[f32; 3]>, to: [f32; 3], out: &mut Vec<u16>| {
                let from = from.unwrap_or(to);
                let mut swept_min = [0.0; 3];
                let mut swept_max = [0.0; 3];
                for axis in 0..3 {
                    let lo = from[axis].min(to[axis]) - center[axis];
                    let hi = from[axis].max(to[axis]) - center[axis];
                    swept_min[axis] = mins[axis] + lo;
                    swept_max[axis] = maxs[axis] + hi;
                }
                split_bbox_leafs(0, swept_min, swept_max, nodes, planes, out);
            };
            // A dmodel's bounds are authored in its original world position.
            // Runtime motion sets TRAIN_OFF = corner - model_center, so adding
            // the absolute corner itself double-translates most lifts. Union
            // the swept brush AABB in that same coordinate system. Teleport
            // corners include only their endpoint, avoiding a giant false PVS
            // bridge across the skipped space.
            add_sweep(if teleport { None } else { previous }, origin, &mut out);
            previous = Some(origin);
            seen.push((corner.clone(), origin, teleport));
            let next = ent_value(block, "target").unwrap_or("").to_string();
            if let Some((_, cycle_pos, cycle_teleport)) =
                seen.iter().find(|(name, _, _)| name == &next)
            {
                if !next.is_empty() {
                    add_sweep(
                        if *cycle_teleport { None } else { previous },
                        *cycle_pos,
                        &mut out,
                    );
                }
            }
            corner = if seen.iter().any(|(name, _, _)| name == &next) {
                String::new()
            } else {
                next
            };
            found = true;
            break;
        }
        if !found {
            break;
        }
        hops += 1;
    }
    if out.is_empty() {
        out = entity_leafs(mins, maxs, fallback_origin, None, nodes, planes);
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn func_train_leafs(
    all_entities: &str,
    train_block: &str,
    mins: [f32; 3],
    maxs: [f32; 3],
    fallback_origin: [f32; 3],
    nodes: &[u8],
    planes: &[u8],
) -> Vec<u16> {
    moving_brush_leafs(
        all_entities,
        train_block,
        "path_corner",
        mins,
        maxs,
        fallback_origin,
        nodes,
        planes,
    )
}

fn guntarget_leafs(
    all_entities: &str,
    target_block: &str,
    mins: [f32; 3],
    maxs: [f32; 3],
    fallback_origin: [f32; 3],
    nodes: &[u8],
    planes: &[u8],
) -> Vec<u16> {
    moving_brush_leafs(
        all_entities,
        target_block,
        "path_track",
        mins,
        maxs,
        fallback_origin,
        nodes,
        planes,
    )
}

/// PSX physical rotation axis selected by GoldSrc's `func_rotating` Euler
/// component flags: 0=Y, 1=X, 2=Z for the runtime matrix switch.
///
/// The names in the SDK are deceptive here. `CFuncRotating` writes the chosen
/// component into `pev->angles`: the default Y component is yaw (physical HL
/// Z), the Z component is roll (physical HL X), and the X component is pitch
/// (physical HL Y). Only after resolving that Euler convention do we apply the
/// world=[HL x,HL z,HL y] remap.
fn rotating_world_axis(spawnflags: u32) -> u8 {
    if spawnflags & 4 != 0 {
        1 // Gold roll -> physical HL X -> PSX X
    } else if spawnflags & 8 != 0 {
        2 // Gold pitch -> physical HL Y -> PSX Z
    } else {
        0 // Gold yaw -> physical HL Z -> PSX Y
    }
}

/// PSX physical rotation axis selected by `func_door_rotating`'s distinct
/// spawnflag namespace. `AxisDir` writes one *Euler component* into `angles`:
/// flag 64 writes Z/roll (physical HL X), flag 128 writes X/pitch (physical
/// HL Y), and the default writes Y/yaw (physical HL Z). Only after resolving
/// that GoldSrc convention do we apply world=[HL x,HL z,HL y]. Treating the
/// component names as literal physical axes made c1a0's swivel chair tumble
/// and its door wheels turn through the wrong plane.
fn rotating_door_world_axis(spawnflags: u32) -> u8 {
    if spawnflags & 64 != 0 {
        1 // Gold roll -> physical HL X -> PSX X
    } else if spawnflags & 128 != 0 {
        2 // Gold pitch -> physical HL Y -> PSX Z
    } else {
        0 // Gold yaw -> physical HL Z -> PSX Y
    }
}

/// Pack the reflected GoldSrc base Euler angles into three q8 bytes for the
/// runtime's pivot-local brush matrix: pitch, yaw, roll. Conjugating the HL
/// matrix through world=[x,z,y] negates every axial angle.
fn rotating_base_angles_q8(block: &str) -> i32 {
    let angles = ent_angles_degrees(block).unwrap_or([0.0; 3]);
    let q8 = |degrees: f32| ((-degrees * 256.0 / 360.0).round() as i32 & 0xff) as u32;
    (q8(angles[0]) | (q8(angles[1]) << 8) | (q8(angles[2]) << 16)) as i32
}

/// Pack the reflected authored yaw of a translating func_train into the same
/// base-Euler word used by rotating brushes. GoldSrc applies the legacy
/// singular `angle` key as `angles.y`; dropping it left c0a0e/c5a1's tram door
/// in model-local orientation while the car around it faced the track.
fn func_train_base_angles_q8(block: &str) -> i32 {
    rotating_base_angles_q8(block)
}

fn rotating_sweep_bounds_hl_axis(
    mins: [f32; 3],
    maxs: [f32; 3],
    axis: usize,
) -> ([f32; 3], [f32; 3]) {
    let radial = |a: usize, b: usize| {
        [mins[a], maxs[a]]
            .into_iter()
            .flat_map(|x| {
                [mins[b], maxs[b]]
                    .into_iter()
                    .map(move |y| (x * x + y * y).sqrt())
            })
            .fold(0.0f32, f32::max)
    };
    match axis {
        0 => {
            let r = radial(1, 2);
            ([mins[0], -r, -r], [maxs[0], r, r])
        }
        1 => {
            let r = radial(0, 2);
            ([-r, mins[1], -r], [r, maxs[1], r])
        }
        _ => {
            let r = radial(0, 1);
            ([-r, -r, mins[2]], [r, r, maxs[2]])
        }
    }
}

/// Conservative local AABB for a complete func_rotating sweep. Resolve the
/// GoldSrc Euler component to its physical HL axis before expanding, then let
/// `entity_leafs` transform the result into PSX world coordinates.
fn rotating_sweep_bounds(mins: [f32; 3], maxs: [f32; 3], spawnflags: u32) -> ([f32; 3], [f32; 3]) {
    if spawnflags & 4 != 0 {
        // GoldSrc Z/roll component -> physical HL X / PSX world X axis.
        rotating_sweep_bounds_hl_axis(mins, maxs, 0)
    } else if spawnflags & 8 != 0 {
        // GoldSrc X/pitch component -> physical HL Y / PSX world Z axis.
        rotating_sweep_bounds_hl_axis(mins, maxs, 1)
    } else {
        // Default GoldSrc Y/yaw component -> physical HL Z / PSX world Y axis.
        rotating_sweep_bounds_hl_axis(mins, maxs, 2)
    }
}

fn rotating_door_sweep_bounds(
    mins: [f32; 3],
    maxs: [f32; 3],
    spawnflags: u32,
) -> ([f32; 3], [f32; 3]) {
    let hl_axis = if spawnflags & 64 != 0 {
        0 // angles.z / roll -> physical HL X
    } else if spawnflags & 128 != 0 {
        1 // angles.x / pitch -> physical HL Y
    } else {
        2 // angles.y / yaw -> physical HL Z
    };
    rotating_sweep_bounds_hl_axis(mins, maxs, hl_axis)
}

fn fan_ramp_needs_extra_think(speed: f32, friction_percent: u16) -> bool {
    let friction = friction_percent.clamp(1, 100) as u32;
    let steps = (100 + friction - 1) / friction;
    let increment = speed.abs() * (friction as f32 * 0.01);
    let mut reached = 0.0f32;
    for _ in 0..steps {
        reached += increment;
    }
    reached < speed.abs()
}

#[derive(Clone, Copy)]
struct CosmeticRotatorCandidate {
    submodel: usize,
    key: ([i32; 3], i16, u8),
    mins: [f32; 3],
    maxs: [f32; 3],
}

fn bounds_overlap(a: CosmeticRotatorCandidate, b: CosmeticRotatorCandidate) -> bool {
    // GoldSrc links BSP pushers with one-unit-expanded abs bounds. The c1a1c
    // light/blade overlays meet exactly at their authored model boundary, so
    // boundary contact is sufficient for the pair to block after one sample.
    (0..3).all(|axis| a.mins[axis] <= b.maxs[axis] && a.maxs[axis] >= b.mins[axis])
}

/// Identify the solid co-pivot overlay pairs which GoldSrc advances once and
/// then leaves blocked. Requiring the same signed angular velocity, axis and
/// overlapping local bounds avoids tagging opposite-running coaxial fans.
fn blocked_cosmetic_rotators(s: &str, models: &[u8], scale: f32) -> HashSet<usize> {
    let mut candidates = Vec::new();
    for block in s.split('{') {
        if ent_value(block, "classname") != Some("func_rotating")
            || !ent_value(block, "targetname").unwrap_or("").is_empty()
        {
            continue;
        }
        let sf = parse_spawnflags(block) as u32;
        if sf & 1 == 0 || sf & 16 != 0 || sf & 64 != 0 {
            continue;
        }
        let Some(submodel) = block_model(block) else {
            continue;
        };
        let Some((mins, maxs)) = model_bounds_hl(models, submodel) else {
            continue;
        };
        let origin_hl = ent_value(block, "origin")
            .and_then(parse_vec3)
            .unwrap_or([0.0; 3]);
        let degrees = parse_f32_key(block, "speed", 100.0);
        let q16 = (degrees * 65536.0 / 360.0 / 20.0)
            .round()
            .clamp(1.0, i16::MAX as f32) as i16;
        let signed_q16 = if sf & 2 != 0 { q16 } else { -q16 };
        let axis = rotating_world_axis(sf);
        candidates.push(CosmeticRotatorCandidate {
            submodel,
            key: (to_world(origin_hl, scale), signed_q16, axis),
            mins,
            maxs,
        });
    }

    let mut blocked = HashSet::new();
    for i in 0..candidates.len() {
        for j in i + 1..candidates.len() {
            if candidates[i].key == candidates[j].key
                && bounds_overlap(candidates[i], candidates[j])
            {
                blocked.insert(candidates[i].submodel);
                blocked.insert(candidates[j].submodel);
            }
        }
    }
    blocked
}

/// Collect renderable brush entities (skipping invisible triggers/ladders).
/// Phase B: submodels whose faces the world pipeline may own. Restricted to
/// classes that never move or animate, drawn unblended (opaque or alpha-test
/// cutout -- both emit ordinary opaque packets) at a zero origin. Breakables,
/// pushables, conveyors and every mover class stay on the entity path.
fn pipeline_static_submodels(ents: &[u8], n_models: usize) -> Vec<bool> {
    let s = entity_text(ents);
    let mut eligible = vec![false; n_models];
    for block in s.split('{') {
        let model = match ent_value(block, "model") {
            Some(m) if m.starts_with('*') => m,
            _ => continue,
        };
        let submodel: usize = model[1..].parse().unwrap_or(0);
        if submodel == 0 || submodel >= n_models {
            continue;
        }
        let cls = ent_value(block, "classname").unwrap_or("");
        if cls != "func_wall" && cls != "func_illusionary" {
            continue;
        }
        let rendermode = parse_f32_key(block, "rendermode", 0.0) as i32;
        let renderamt = parse_f32_key(block, "renderamt", 255.0) as i32;
        let blended = matches!(rendermode, 2 | 3 if renderamt < 250) || rendermode == 5;
        let origin = ent_value(block, "origin")
            .and_then(parse_vec3)
            .unwrap_or([0.0; 3]);
        let name = ent_value(block, "targetname").unwrap_or("");
        let render_controlled = !name.is_empty()
            && s.split('{').any(|controller| {
                ent_value(controller, "classname") == Some("env_render")
                    && ent_value(controller, "target") == Some(name)
            });
        if !blended && !render_controlled && origin == [0.0; 3] {
            eligible[submodel] = true;
        }
    }
    eligible
}

fn brush_render_class(block: &str) -> u16 {
    let mode = parse_f32_key(block, "rendermode", 0.0) as i32;
    let amount = parse_f32_key(block, "renderamt", 255.0) as i32;
    if mode != 0 && amount <= 0 {
        return 0x80;
    }
    match mode {
        2 | 3 if amount < 250 => 1,
        5 => 2,
        4 => 3,
        _ => 0,
    }
}

fn collect_entities(
    ents: &[u8],
    models: &[u8],
    nodes: &[u8],
    planes: &[u8],
    scale: f32,
    main_tram_submodel: usize,
) -> Vec<EntRec> {
    let s = entity_text(ents);
    let n_models = models.len() / SZ_MODEL;
    let blocked_rotators = blocked_cosmetic_rotators(&s, models, scale);
    let mut out = Vec::new();
    for block in s.split('{') {
        let model = match ent_value(block, "model") {
            Some(m) if m.starts_with('*') => m,
            _ => continue,
        };
        let submodel: usize = model[1..].parse().unwrap_or(0);
        if submodel == 0 || submodel >= n_models {
            continue;
        }
        let cls = ent_value(block, "classname").unwrap_or("");
        if cls.starts_with("trigger")
            || (cls == "func_tracktrain" && submodel == main_tram_submodel)
            || cls == "func_monsterclip"
            || cls == "func_friction"
            || cls == "func_mortar_field"
            || cls == "env_bubbles"
        {
            // Invisible/non-world-solid volumes. The selected player tram has
            // its own rotated render/collision path; other func_tracktrains are
            // retained here and driven through the generic train pool.
            // GoldSrc spawns func_friction as SOLID_TRIGGER and both
            // func_mortar_field and env_bubbles as non-solid, invisible
            // controller volumes; none may fall through to the static-solid
            // brush path. Their BSP model bounds remain available to a
            // dedicated logic/effect collector without retaining a
            // render/collision EntRec (as func_monsterclip already does).
            continue;
        }
        let origin_hl = ent_value(block, "origin")
            .and_then(parse_vec3)
            .unwrap_or([0.0; 3]);
        let origin = to_world(origin_hl, scale);
        // HL render modes -> brush render class in the ent kind's high byte:
        // 1 = semi-transparent (rendermode 2 texture / 3 glow with low amt),
        // 2 = additive (rendermode 5), 3 = alpha-tested/cutout (rendermode 4).
        // Class 3 still emits opaque PS1 packets, but remains distinct so a
        // transparent texel aperture does not hide an entire studio actor.
        let blend = brush_render_class(block);
        let mo = submodel * SZ_MODEL;
        let g = |o: usize| f32le(models, mo + o).unwrap_or(0.0);
        let mins = [g(0), g(4), g(8)];
        let maxs = [g(12), g(16), g(20)];
        let center = to_world(
            [
                (mins[0] + maxs[0]) * 0.5,
                (mins[1] + maxs[1]) * 0.5,
                (mins[2] + maxs[2]) * 0.5,
            ],
            scale,
        );
        let sz = [maxs[0] - mins[0], maxs[1] - mins[1], maxs[2] - mins[2]];
        let half = [sz[0] * 0.5, sz[1] * 0.5, sz[2] * 0.5];
        let rad =
            ((half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt() + 80.0) / scale;
        let r2 = (rad * rad) as i32;
        let head = model_headnode(models, submodel, 1).unwrap_or(0);
        let head0 = model_headnode(models, submodel, 0).unwrap_or(0); // BSP tree
        if cls == "func_pushable" {
            // CPushable::Spawn raises the SOLID_BBOX one HL unit so it does not
            // start embedded in its floor. The `friction` key is actually its
            // horizontal speed cap: (400-friction) u/s, converted to 20 Hz.
            let mut lifted_hl = origin_hl;
            lifted_hl[2] += 1.0;
            let lifted = to_world(lifted_hl, scale);
            let friction = parse_f32_key(block, "friction", 0.0).clamp(0.0, 399.0);
            let max_speed = ((400.0 - friction) / scale / 20.0)
                .round()
                .clamp(1.0, i8::MAX as f32) as i32;
            let h = to_world([half[0], half[1], half[2]], scale);
            let hx = h[0].abs();
            let hy = h[1].abs();
            let hz = h[2].abs();
            // SET_MODEL expands every model bound by one HL unit. Our rounded
            // centre/half representation has already absorbed that unit on
            // some odd-sized negative axes, so retain a three-bit correction
            // mask instead of pessimistically subtracting one everywhere.
            let padded_min = to_world([mins[0] - 1.0, mins[1] - 1.0, mins[2] - 1.0], scale);
            let rounded_min = [center[0] - hx, center[1] - hy, center[2] - hz];
            let mut min_correction_mask = 0u16;
            for axis in 0..3 {
                if padded_min[axis] < rounded_min[axis] {
                    min_correction_mask |= 1 << axis;
                }
            }
            let collision_hull = pushable_collision_hull(mins, maxs);
            let leaves = entity_leafs(mins, maxs, lifted_hl, None, nodes, planes);
            out.push(EntRec {
                submodel: submodel as u16,
                kind: ENT_KIND_PUSHABLE | (blend << 8),
                origin: lifted,
                mv: [
                    pack_pushable_speed_half_x(max_speed, hx, collision_hull, min_correction_mask),
                    hy,
                    hz,
                ],
                center,
                r2,
                head,
                head0,
                leaves,
            });
            continue;
        }
        if cls == "func_ladder" {
            // Invisible climb volume: never drawn, never collides. The world
            // half-extents ride in `mv` (unused for non-movers) so the runtime
            // can do a cheap AABB touch test against the player.
            let hx = to_world([half[0], half[1], half[2]], scale);
            out.push(EntRec {
                submodel: submodel as u16,
                kind: 4,
                origin,
                mv: [hx[0].abs(), hx[1].abs(), hx[2].abs()],
                center,
                r2,
                head: 0,
                head0: 0,
                leaves: Vec::new(),
            });
            continue;
        }
        if cls == "func_platrot" {
            // CFuncPlat::Setup defines position1 as the authored TOP and
            // position2.z = top.z - height. CFuncPlatRot then synchronizes its
            // full rotation to that linear travel time. Store the bottom as
            // the entity-local pivot so the runtime's ordinary 0..4096 phase
            // maps directly from bottom/angle 0 to top/full angle.
            let authored_height = parse_f32_key(block, "height", 0.0);
            let travel = if authored_height != 0.0 {
                authored_height
            } else {
                (sz[2] - 8.0).max(8.0)
            };
            let bottom_hl = [origin_hl[0], origin_hl[1], origin_hl[2] - travel];
            let bottom = to_world(bottom_hl, scale);
            let top_delta = to_world([0.0, 0.0, travel], scale);
            let full_rotation =
                (parse_f32_key(block, "rotation", 0.0) * 4096.0 / 360.0).round() as i32;
            let base_angles = rotating_base_angles_q8(block) as u32;
            let rotation_axis = rotating_door_world_axis(parse_spawnflags_u32(block));

            // A rotating origin-brush is entity-local. PVS membership must
            // cover the whole yaw sweep, not just its endpoint AABBs, or a
            // long/off-centre platform can disappear mid-turn.
            let sweep_r = [mins[0], maxs[0]]
                .into_iter()
                .flat_map(|x| {
                    [mins[1], maxs[1]].into_iter().flat_map(move |y| {
                        [mins[2], maxs[2]]
                            .into_iter()
                            .map(move |z| (x * x + y * y + z * z).sqrt())
                    })
                })
                .fold(0.0f32, f32::max);
            let leaves = entity_leafs(
                [-sweep_r; 3],
                [sweep_r; 3],
                bottom_hl,
                Some([0.0, 0.0, travel]),
                nodes,
                planes,
            );
            out.push(EntRec {
                submodel: submodel as u16,
                kind: ENT_KIND_PLATROT | (blend << 8),
                origin: bottom,
                mv: [
                    full_rotation,
                    top_delta[1],
                    (base_angles | ((rotation_axis as u32) << 24)) as i32,
                ],
                center,
                r2,
                head,
                head0,
                leaves,
            });
            continue;
        }
        if cls == "func_plat" {
            // Platform authored at its TOP position; travel is straight down by
            // `height` (or its own size minus the 8u lip). Runs on the door
            // machinery: touch -> descend, wait, return.
            let travel = ent_value(block, "height")
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|h| *h > 1.0)
                .unwrap_or((sz[2] - 8.0).max(8.0));
            let mv = to_world([0.0, 0.0, -travel], scale);
            let leaves = entity_leafs(
                mins,
                maxs,
                origin_hl,
                Some([0.0, 0.0, -travel]),
                nodes,
                planes,
            );
            out.push(EntRec {
                submodel: submodel as u16,
                kind: 1 | (blend << 8),
                origin,
                mv,
                center,
                r2,
                head,
                head0,
                leaves,
            });
            continue;
        }
        if cls == "func_door"
            || cls == "func_button"
            || cls == "func_rot_button"
            || cls == "momentary_door"
            || cls == "momentary_rot_button"
        {
            let is_rot_button = cls == "func_rot_button" || cls == "momentary_rot_button";
            let is_button = cls == "func_button" || is_rot_button;
            let lip = ent_value(block, "lip")
                .and_then(|a| a.parse().ok())
                .unwrap_or(if is_button { 4.0 } else { 8.0 });
            let sf = parse_spawnflags_u32(block);
            let (kind, mv, leaf_move, rot_sweep) = if is_rot_button {
                // CRotButton uses the same AxisDir/backwards convention as a
                // rotating door. Preserve the authored lever/wheel movement
                // instead of firing its target while leaving the model static.
                let deg = parse_f32_key(block, "distance", 0.0);
                let hl_angle = (deg.abs() * 4096.0 / 360.0).round() as i32;
                let reverse = sf & 2 != 0 || deg < 0.0;
                let angle = if reverse { hl_angle } else { -hl_angle };
                (
                    ENT_KIND_ROT_BUTTON,
                    [
                        angle,
                        rotating_door_world_axis(sf) as i32,
                        rotating_base_angles_q8(block),
                    ],
                    None,
                    true,
                )
            } else {
                let (dir, dist) = door_move(ent_move_dir(block), mins, maxs, lip);
                (
                    if is_button { 3 } else { 1 },
                    to_world([dir[0] * dist, dir[1] * dist, dir[2] * dist], scale),
                    Some([dir[0] * dist, dir[1] * dist, dir[2] * dist]),
                    false,
                )
            };
            let leaves = if rot_sweep {
                let (sweep_mins, sweep_maxs) = rotating_door_sweep_bounds(mins, maxs, sf);
                entity_leafs(sweep_mins, sweep_maxs, origin_hl, None, nodes, planes)
            } else {
                entity_leafs(mins, maxs, origin_hl, leaf_move, nodes, planes)
            };
            let rot_button_solid = if cls == "momentary_rot_button" {
                sf & 1 != 0 // SF_MOMENTARY_DOOR
            } else {
                sf & 1 == 0 // !SF_ROTBUTTON_NOTSOLID
            };
            out.push(EntRec {
                submodel: submodel as u16,
                kind: kind | (blend << 8),
                origin,
                mv,
                center,
                r2,
                head: if is_rot_button && !rot_button_solid {
                    0
                } else {
                    head
                },
                head0,
                leaves,
            });
        } else if cls == "func_water"
            || (cls == "func_train" && parse_f32_key(block, "skin", 0.0) as i32 == -3)
        {
            // Swimmable volume: renders like any translucent brush, and the
            // half-extents ride in mv (kind 6) so the runtime can switch the
            // player into swim physics inside it. GoldSrc also uses skin=-3
            // on a func_train for moving water (c1a1b); it remains non-solid
            // while the shared train pool supplies its live draw offset.
            let hx = to_world([half[0], half[1], half[2]], scale);
            let leaves = if cls == "func_train" {
                func_train_leafs(&s, block, mins, maxs, origin_hl, nodes, planes)
            } else {
                entity_leafs(mins, maxs, origin_hl, None, nodes, planes)
            };
            out.push(EntRec {
                submodel: submodel as u16,
                kind: 6 | (blend << 8),
                origin: [0; 3],
                mv: [hx[0].abs(), hx[1].abs(), hx[2].abs()],
                center,
                r2,
                head: 0,
                head0: 0,
                leaves,
            });
        } else if cls == "func_pendulum" {
            // CPendulum rotates an origin brush about AxisDir. Keep its Swing
            // constants in the existing three mover words so runtime needs no
            // pendulum-specific resident array. One turn is 2^19 phase units;
            // velocity is phase units per 20 Hz host sample and acceleration
            // is the velocity delta produced by one 0.05-second host sample.
            const PHASE_PER_TURN: f32 = 524_288.0;
            let sf = parse_spawnflags(block) as u32;
            let distance = parse_f32_key(block, "distance", 0.0);
            let authored_speed = parse_f32_key(block, "speed", 100.0);
            let speed = if authored_speed > 0.0 {
                authored_speed
            } else {
                100.0
            };
            let center_q19 = (distance * 0.5 * PHASE_PER_TURN / 360.0).round() as i32;
            let max_velocity_q19 = (speed * PHASE_PER_TURN / 360.0 / 20.0)
                .round()
                .clamp(1.0, i16::MAX as f32) as i32;
            let angular_accel = if distance.abs() > f32::EPSILON {
                speed * speed / (2.0 * distance.abs())
            } else {
                0.0
            };
            let accel_per_host_tick_q19 = (angular_accel * PHASE_PER_TURN / 360.0 / 400.0)
                .round()
                .clamp(1.0, i16::MAX as f32) as i32;
            // GoldSrc components map Z -> PSX Y, X -> PSX X and default Y ->
            // PSX Z. The coordinate swap is a reflection; runtime negates the
            // axial angle before building the matrix.
            let axis = if sf & 64 != 0 {
                0
            } else if sf & 128 != 0 {
                1
            } else {
                2
            };
            let sweep_flags = if sf & 64 != 0 {
                4
            } else if sf & 128 != 0 {
                8
            } else {
                0
            };
            let (sweep_mins, sweep_maxs) = rotating_sweep_bounds(mins, maxs, sweep_flags);
            let leaves = entity_leafs(sweep_mins, sweep_maxs, origin_hl, None, nodes, planes);
            let passable = sf & 8 != 0; // SDK Spawn checks SF_DOOR_PASSABLE
            out.push(EntRec {
                submodel: submodel as u16,
                kind: ENT_KIND_PENDULUM | (blend << 8),
                origin,
                mv: [
                    center_q19,
                    ((max_velocity_q19 as u32) << 16 | axis as u32) as i32,
                    ((accel_per_host_tick_q19 as u32) << 16 | (sf & 0xffff)) as i32,
                ],
                center,
                r2,
                head: if passable { 0 } else { head },
                head0: if passable { 0 } else { head0 },
                leaves,
            });
        } else if cls == "func_rotating" {
            // Spinning brush (fans). kind 5: mv[0] carries signed angular speed
            // in Q16-turn units per 20 Hz tick (four fractional bits beyond the
            // renderer's Q12 angle), mv[1] selects the PSX rotation axis, and
            // mv[2] packs the friction/profile in the high half and raw
            // spawnflags in the low half for the stateless cosmetic/collision
            // path. Targeted fans integrate this speed into ENT_PHASE;
            // untargeted cosmetic fans derive their angle from the map tick.
            // origin is the pivot.
            let sf = parse_spawnflags_u32(block);
            let raw_friction = parse_f32_key(block, "fanfriction", 0.0);
            let friction = (if raw_friction > 0.0 {
                raw_friction
            } else {
                100.0
            })
            .round()
            .clamp(1.0, 100.0) as u32;
            let degs = parse_f32_key(block, "speed", 100.0);
            let reverse = sf & 2 != 0; // SF 2 = reverse direction
            let hl_w = (degs * 65536.0 / 360.0 / 20.0)
                .round()
                .clamp(1.0, i16::MAX as f32) as i32;
            // world=[HL x,HL z,HL y] is a reflection, so axial rotation signs
            // invert. Resolve GoldSrc's Euler component first: Z/roll -> PSX X,
            // X/pitch -> PSX Z, and default Y/yaw -> PSX Y.
            let w = if reverse { hl_w } else { -hl_w };
            let axis = rotating_world_axis(sf);
            let (sweep_mins, sweep_maxs) = rotating_sweep_bounds(mins, maxs, sf);
            let leaves = entity_leafs(sweep_mins, sweep_maxs, origin_hl, None, nodes, planes);
            let mut profile = (friction as u16) & FAN_PROFILE_FRICTION_MASK;
            if fan_ramp_needs_extra_think(degs, friction as u16) {
                profile |= FAN_PROFILE_RAMP_EXTRA_THINK;
            }
            if blocked_rotators.contains(&submodel) {
                profile |= FAN_PROFILE_BLOCKED_AFTER_FIRST_SAMPLE;
            }
            out.push(EntRec {
                submodel: submodel as u16,
                kind: 5 | (blend << 8),
                origin,
                mv: [
                    w,
                    axis as i32,
                    ((profile as u32) << 16 | (sf & 0xffff)) as i32,
                ],
                center,
                r2,
                head: if sf & 64 != 0 { 0 } else { head },
                head0: if sf & 64 != 0 { 0 } else { head0 },
                leaves,
            });
        } else if cls == "func_door_rotating" {
            // Swinging door. kind 7: mv[0] = reflected signed open angle in
            // Q12 turns and mv[1] = PSX physical axis. GoldSrc door-axis flags
            // are 64/128/default, not func_rotating's 4/8/default. AxisDir
            // selects an Euler component (roll/pitch/yaw), which the helper
            // resolves to a physical axis. The world remap is a reflection, so
            // every axial angle changes sign after that resolution.
            let sf = parse_spawnflags_u32(block);
            let deg = parse_f32_key(block, "distance", 90.0);
            let hl_angle = (deg * 4096.0 / 360.0).round() as i32;
            let a = if sf & 2 != 0 { hl_angle } else { -hl_angle };
            let axis = rotating_door_world_axis(sf);
            let (sweep_mins, sweep_maxs) = rotating_door_sweep_bounds(mins, maxs, sf);
            let leaves = entity_leafs(sweep_mins, sweep_maxs, origin_hl, None, nodes, planes);
            out.push(EntRec {
                submodel: submodel as u16,
                kind: 7 | (blend << 8),
                origin,
                mv: [a, axis as i32, rotating_base_angles_q8(block)],
                center,
                r2,
                head: if sf & 8 != 0 { 0 } else { head },
                head0: if sf & 8 != 0 { 0 } else { head0 },
                leaves,
            });
        } else {
            let leaves = if cls == "func_train" {
                func_train_leafs(&s, block, mins, maxs, origin_hl, nodes, planes)
            } else if cls == "func_guntarget" {
                guntarget_leafs(&s, block, mins, maxs, origin_hl, nodes, planes)
            } else {
                entity_leafs(mins, maxs, origin_hl, None, nodes, planes)
            };
            let retained_angles = if matches!(
                cls,
                "func_train"
                    | "func_tracktrain"
                    | "func_tank"
                    | "func_tank2"
                    | "func_tank3"
                    | "func_tanklaser"
                    | "func_tankrocket"
                    | "func_tankmortar"
            ) {
                func_train_base_angles_q8(block)
            } else {
                0
            };
            out.push(EntRec {
                submodel: submodel as u16,
                kind: (if retained_angles != 0 {
                    // Reuse the zero-motion axial-brush representation. mv[0]
                    // remains zero while mv[2] carries the complete fixed Euler.
                    7
                } else if cls == "func_illusionary" {
                    2
                } else {
                    0
                }) | (blend << 8),
                origin,
                mv: [0, 0, retained_angles],
                center,
                r2,
                head,
                head0,
                leaves,
            });
        }
    }
    out
}

#[derive(Clone, Default)]
struct TitleDef {
    text: String, // lines joined with \n
    effect: u8,   // 0 fade, 1 flicker credits, 2 typewriter scan-out
    hold_ticks: u16,
    fade_ticks: u16,
    left_aligned: bool, // authored x >= 0 (the stock file only uses x = 0.1)
    /// Authored top edge in 1/31-screen steps; zero retains GoldSrc's -1 center.
    y_q5: u8,
}

fn title_y_q5(y: f32) -> u8 {
    if y < 0.0 {
        0
    } else {
        (y * 31.0).round().clamp(1.0, 31.0) as u8
    }
}

fn pack_title_flags(title: &TitleDef) -> u16 {
    (title.effect as u16 & 3)
        | ((title.left_aligned as u16) << 2)
        | (((title.y_q5 as u16) & 31) << 3)
        | ((title.fade_ticks.min(255)) << 8)
}

/// Translate the PC-centric Hazard Course captions to the fixed PS1 layout.
/// Keep this scoped to the HZ title block so campaign text remains byte-for-byte
/// faithful to Valve's source file.
fn adapt_hazard_course_text(text: &str) -> String {
    let mut out = text.to_string();
    for (pc, ps1) in [
        ("YOUR USE KEY", "SQUARE"),
        ("THE USE KEY", "SQUARE"),
        ("USE KEY", "SQUARE"),
        ("FORWARD KEY", "LEFT STICK UP"),
        ("BACKWARD KEY", "LEFT STICK DOWN"),
        ("MOVELEFT", "LEFT STICK LEFT"),
        ("MOVERIGHT", "LEFT STICK RIGHT"),
        ("JUMP KEY", "CROSS"),
        ("THE DUCK KEY", "TRIANGLE"),
        ("DUCK KEY", "TRIANGLE"),
        ("ATTACK1 KEY", "R2"),
        ("ATTACK2 KEY", "L2"),
        ("RELOAD KEY", "CIRCLE"),
        ("FLASHLIGHT KEY", "L3"),
        ("AIM WITH THE MOUSE", "AIM WITH THE RIGHT STICK"),
    ] {
        out = out.replace(pc, ps1);
    }
    out
}

/// Parse valve/titles.txt: stateful $directives then NAME { lines } blocks.
fn parse_titles(valve_dir: &std::path::Path) -> std::collections::HashMap<String, TitleDef> {
    let mut out = std::collections::HashMap::new();
    let Ok(txt) = std::fs::read_to_string(valve_dir.join("titles.txt")) else {
        return out;
    };
    let mut cur = TitleDef {
        effect: 0,
        hold_ticks: 60,
        fade_ticks: 20,
        left_aligned: false,
        y_q5: 0,
        ..Default::default()
    };
    let mut name: Option<String> = None;
    let mut lines: Vec<String> = Vec::new();
    let mut in_block = false;
    for raw in txt.lines() {
        let line = raw.trim();
        if line.starts_with("//") || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('$') {
            let mut it = rest.split_whitespace();
            match it.next().unwrap_or("") {
                "effect" => cur.effect = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                "holdtime" => {
                    let sec: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(3.0);
                    cur.hold_ticks = (sec * 20.0).round().clamp(1.0, 65535.0) as u16;
                }
                "fadeout" => {
                    let sec: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
                    cur.fade_ticks = (sec * 20.0).round().clamp(1.0, 255.0) as u16;
                }
                "position" => {
                    let x: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(-1.0);
                    let y: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(-1.0);
                    cur.left_aligned = x >= 0.0;
                    // The env_message record has five otherwise-unused low bits.
                    // Zero is the centered (-1) sentinel; all authored stock
                    // positions are in 0.3..0.8 and quantize within two pixels.
                    cur.y_q5 = title_y_q5(y);
                }
                _ => {}
            }
            continue;
        }
        if line == "{" {
            in_block = true;
            lines.clear();
            continue;
        }
        if line == "}" {
            if let Some(n) = name.take() {
                let mut def = cur.clone();
                def.text = lines.join("\n");
                if n.to_ascii_uppercase().starts_with("HZ") {
                    def.text = adapt_hazard_course_text(&def.text);
                }
                out.insert(n.to_uppercase(), def);
            }
            in_block = false;
            lines.clear();
            continue;
        }
        if in_block {
            lines.push(line.to_string());
        } else {
            name = Some(line.to_string());
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScriptClip {
    slot: u8,
    /// Retail sequence duration in 100 ms monster-think quanta. Zero keeps
    /// backward compatibility with old three-field manifests.
    hold_quanta: u16,
}

/// (type, lowercase clip name) -> visual slot + retail duration, from the
/// CLIPS_MANIFEST env file (written by the Rust model cook). Distinct retail
/// sequences may deliberately alias one RAM-resident pose clip; their timing
/// must remain distinct even when their geometry does not.
fn load_clips_manifest() -> std::collections::HashMap<(u16, String), ScriptClip> {
    let mut out = std::collections::HashMap::new();
    let Ok(path) = std::env::var("CLIPS_MANIFEST") else {
        return out;
    };
    let Ok(txt) = std::fs::read_to_string(&path) else {
        eprintln!("warn: CLIPS_MANIFEST unreadable: {}", path);
        return out;
    };
    for line in txt.lines() {
        let mut it = line.trim().split('|');
        let (Some(ty), Some(name), Some(slot)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let hold_quanta = it.next().and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
        if let (Ok(ty), Ok(slot)) = (ty.parse::<u16>(), slot.parse::<u8>()) {
            out.insert(
                (ty, name.to_ascii_lowercase()),
                ScriptClip { slot, hold_quanta },
            );
        }
    }
    out
}

#[derive(Clone)]
struct StudioTargetEvent {
    tick: u16,
    period: u16,
    target: String,
}

/// (actor type, authored clip name) -> GoldSrc studio event 1003 records. The
/// model extractor reads the retail MDL event tables; map cooking then folds
/// only events reachable by this room's scripted_sequences into LogicAux.
fn load_studio_events_manifest() -> std::collections::HashMap<(u16, String), Vec<StudioTargetEvent>>
{
    let mut out = std::collections::HashMap::new();
    let Ok(path) = std::env::var("STUDIO_EVENTS_MANIFEST") else {
        return out;
    };
    let Ok(txt) = std::fs::read_to_string(&path) else {
        eprintln!("warn: STUDIO_EVENTS_MANIFEST unreadable: {}", path);
        return out;
    };
    for line in txt.lines() {
        let fields: Vec<&str> = line.trim().split('|').collect();
        if fields.len() != 5 {
            continue;
        }
        let (Ok(ty), Ok(tick), Ok(period)) = (
            fields[0].parse::<u16>(),
            fields[2].parse::<u16>(),
            fields[3].parse::<u16>(),
        ) else {
            continue;
        };
        let name = fields[1].trim().to_ascii_lowercase();
        let target = fields[4].trim();
        if name.is_empty() || target.is_empty() || tick == 0 || period == 0 {
            continue;
        }
        out.entry((ty, name))
            .or_insert_with(Vec::new)
            .push(StudioTargetEvent {
                tick,
                period,
                target: target.to_string(),
            });
    }
    out
}

#[derive(Clone)]
struct TransitionTypeHint {
    ty: u16,
    targetname: String,
    origin: [f32; 3],
    yaw: f32,
    spawnflags: u32,
    body: u8,
}

/// Type hints for scripts that name an actor supplied only by an incoming
/// transition. Besides resolving animation clips, the manifest retains a
/// landmark-transformed default placement for fresh direct-map launches. A
/// natural changelevel still gets its state exclusively from the carry mailbox.
fn load_transition_type_hints(map_name: &str) -> Vec<TransitionTypeHint> {
    let mut out = Vec::new();
    let Ok(path) = std::env::var("TRANSITION_PROPS_MANIFEST") else {
        return out;
    };
    let Ok(txt) = std::fs::read_to_string(&path) else {
        eprintln!("warn: TRANSITION_PROPS_MANIFEST unreadable: {}", path);
        return out;
    };
    for line in txt.lines() {
        let fields: Vec<&str> = line.trim().split('|').collect();
        if fields.len() < 8 || fields[0] != map_name {
            continue;
        }
        let (Ok(ty), Ok(x), Ok(y), Ok(z), Ok(yaw)) = (
            fields[1].parse::<u16>(),
            fields[3].parse::<f32>(),
            fields[4].parse::<f32>(),
            fields[5].parse::<f32>(),
            fields[6].parse::<f32>(),
        ) else {
            continue;
        };
        out.push(TransitionTypeHint {
            ty,
            targetname: fields[2].to_string(),
            origin: [x, y, z],
            yaw,
            spawnflags: fields.get(8).and_then(|v| v.parse().ok()).unwrap_or(0),
            body: fields
                .get(9)
                .and_then(|v| v.parse::<u8>().ok())
                .unwrap_or(0)
                .min(7),
        });
    }
    out
}

fn transition_fallback_type(hint: &TransitionTypeHint) -> u16 {
    const PREDISASTER: u16 = 0x2000;
    const PRISONER: u16 = 0x1000;
    let mut ty = hint.ty & 0x0fff;
    if matches!(ty, 0 | 1) && hint.spawnflags & 256 != 0 {
        ty |= PREDISASTER;
    }
    if (5..=24).contains(&(ty & 0x0fff)) && hint.spawnflags & 16 != 0 {
        ty |= PRISONER;
    }
    ty
}

// c1a1b and c4a3 author a standing monster_scientist in a seated idle before
// playing the retail sitstand sequence. A compact dedicated scientist stream
// carries those two clips without making every ordinary scientist map pay for
// them (c4a3 is the campaign-wide model-pool peak).
const SCRIPTED_SITTING_SCIENTIST_TYPE: u16 = 54;
const VENT_SCRIPT_ZOMBIE_TYPE: u16 = 55;

fn scripted_sitting_scientist_target(all: &str, entity_name: &str) -> bool {
    !entity_name.is_empty()
        && all.split('{').any(|block| {
            ent_value(block, "classname") == Some("scripted_sequence")
                && ent_value(block, "m_iszEntity") == Some(entity_name)
                && ent_value(block, "m_iszIdle")
                    .map(|name| name.eq_ignore_ascii_case("sitidle"))
                    .unwrap_or(false)
        })
}

fn map_uses_vent_zombie_stream(all: &str) -> bool {
    all.split('{').any(|block| {
        ent_value(block, "classname") == Some("scripted_sequence")
            && (ent_value(block, "m_iszIdle")
                .map(|name| name.eq_ignore_ascii_case("ventclimbidle"))
                .unwrap_or(false)
                || ent_value(block, "m_iszPlay")
                    .map(|name| name.eq_ignore_ascii_case("ventclimb"))
                    .unwrap_or(false))
    })
}

/// Resolve a scripted_sequence's m_iszEntity targetname to its monster type id.
fn script_monster_type(all: &str, entity_name: &str) -> Option<u16> {
    if entity_name.is_empty() {
        return None;
    }
    for cb in all.split('{') {
        if ent_value(cb, "targetname") == Some(entity_name) {
            if let Some(cls) = ent_value(cb, "classname") {
                if cls == "monster_scientist" && scripted_sitting_scientist_target(all, entity_name)
                {
                    return Some(SCRIPTED_SITTING_SCIENTIST_TYPE);
                }
                if cls == "monster_zombie" && map_uses_vent_zombie_stream(all) {
                    return Some(VENT_SCRIPT_ZOMBIE_TYPE);
                }
                if matches!(cls, "monster_generic" | "monster_furniture" | "cycler") {
                    return monster_generic_type(cb);
                }
                if cls.starts_with("monster_") {
                    return monster_type_id(cls);
                }
            }
        }
    }
    None
}

/// Cooked classname fallback for `CCineMonster::FindEntity`.  The flags byte
/// stores type+1 so zero remains the exact-targetname-only representation.
fn script_class_selector(entity_name: &str) -> u8 {
    monster_type_id(entity_name)
        .and_then(|ty| u8::try_from(ty).ok())
        .map(|ty| ty.saturating_add(1))
        .unwrap_or(0)
}

fn script_target_monster_type(
    all: &str,
    entity_name: &str,
    transition_types: &std::collections::HashMap<String, u16>,
) -> Option<u16> {
    script_monster_type(all, entity_name)
        .or_else(|| transition_types.get(entity_name).copied())
        .or_else(|| monster_type_id(entity_name))
}

fn script_clip_slots(
    all: &str,
    block: &str,
    transition_types: &std::collections::HashMap<String, u16>,
    clips: &std::collections::HashMap<(u16, String), ScriptClip>,
) -> (u16, u16) {
    // Model rosters use fewer than 63 clips. Keep slot+1 in the low six bits
    // and fold up to 102.3 seconds of source duration into the otherwise free
    // high ten bits of the existing LogicAux word. This fixes sequence-chain
    // timing without adding one byte of resident map or actor state.
    const SLOT_MASK: u16 = 0x003f;
    const DURATION_SHIFT: u32 = 6;
    let entity_name = ent_value(block, "m_iszEntity").unwrap_or("");
    let ty = script_target_monster_type(all, entity_name, transition_types);
    let lookup = |key: &str| -> u16 {
        let Some(ty) = ty else { return 0 };
        let Some(name) = ent_value(block, key) else {
            return 0;
        };
        clips
            .get(&(ty, name.to_ascii_lowercase()))
            .map(|clip| {
                let slot = clip.slot as u16 + 1;
                assert!(slot <= SLOT_MASK, "script clip slot exceeds packed token");
                slot | (clip.hold_quanta.min(1023) << DURATION_SHIFT)
            })
            .unwrap_or(0)
    };
    (lookup("m_iszPlay"), lookup("m_iszIdle"))
}

#[cfg(test)]
fn collect_logic_entities(
    ents: &[u8],
    models: &[u8],
    brush_by_submodel: &[u16],
    scale: f32,
    titles: &std::collections::HashMap<String, TitleDef>,
    transition_types: &std::collections::HashMap<String, u16>,
) -> Result<LogicCook, String> {
    collect_logic_entities_with_lightstyles(
        ents,
        models,
        brush_by_submodel,
        scale,
        titles,
        transition_types,
        &[],
    )
}

fn collect_logic_entities_with_lightstyles(
    ents: &[u8],
    models: &[u8],
    brush_by_submodel: &[u16],
    scale: f32,
    titles: &std::collections::HashMap<String, TitleDef>,
    transition_types: &std::collections::HashMap<String, u16>,
    dynamic_lightstyles: &[DynamicLightStyle],
) -> Result<LogicCook, String> {
    let s = entity_text(ents);
    let mut names = LogicNames::default();
    let mut out = Vec::new();
    let mut aux = Vec::new();
    // One entry per record emitted by the main raw-entity pass. Registration
    // of multisource inputs happens only after every source has its final logic
    // index, so the runtime can compare an exact caller without name scans.
    let mut source_raw_index: Vec<usize> = Vec::new();
    let mut emitted_lightstyles = std::collections::HashSet::<u8>::new();
    let clips = load_clips_manifest();
    let studio_events = load_studio_events_manifest();
    let voices = load_voices_manifest();
    // This map's MAPLIST index -- keys the per-map voice manifest (set by the
    // rooms recipe alongside VOICES_MANIFEST).
    let map_index: u16 = std::env::var("MAP_INDEX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // Several authored set pieces put multiple func_tracktrains on one shared
    // path (c0a0c's three forklifts). A path_track FIREONCE message belongs to
    // the shared node, not to each cooked train copy. Assign those messages to
    // the first train on a start path so they cannot toggle progression three
    // times when the followers arrive.
    let mut track_path_owner: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for block in s.split('{') {
        if ent_value(block, "classname") != Some("func_tracktrain") {
            continue;
        }
        let start = ent_value(block, "target").unwrap_or("");
        let owner = ent_value(block, "model")
            .or_else(|| ent_value(block, "targetname"))
            .unwrap_or("");
        if !start.is_empty() && !owner.is_empty() {
            track_path_owner
                .entry(start.to_string())
                .or_insert_with(|| owner.to_string());
        }
    }

    // Teleport destinations, resolved at cook time (name -> world origin+yaw).
    let mut tp_dests: Vec<(String, [i32; 3], u16)> = Vec::new();
    for block in s.split('{') {
        let cls = ent_value(block, "classname").unwrap_or("");
        if cls == "info_teleport_destination" || cls == "info_target" {
            if let (Some(name), Some(o)) = (
                ent_value(block, "targetname"),
                ent_value(block, "origin").and_then(parse_vec3),
            ) {
                let yaw = hl_yaw_to_world_q12(ent_yaw_degrees(block).unwrap_or(0.0));
                tp_dests.push((name.to_string(), to_world(o, scale), yaw as u16));
            }
        }
    }

    for (raw_index, block) in s.split('{').enumerate() {
        let cls = ent_value(block, "classname").unwrap_or("");
        let dynamic_light_slot = if matches!(cls, "light" | "light_spot") {
            ent_value(block, "style")
                .and_then(|v| v.parse::<u8>().ok())
                .and_then(|style| {
                    dynamic_lightstyles
                        .iter()
                        .position(|light| light.style == style)
                        .map(|index| index as u8 + 1)
                })
        } else {
            None
        };
        if dynamic_light_slot.is_some_and(|slot| !emitted_lightstyles.insert(slot)) {
            continue;
        }
        let kind = match cls {
            "light" | "light_spot" if dynamic_light_slot.is_some() => LOGIC_LIGHTSTYLE,
            "func_door" | "func_plat" | "func_platrot" | "func_door_rotating"
            | "momentary_door" => LOGIC_FUNC_DOOR,
            "func_button" | "func_rot_button" => LOGIC_FUNC_BUTTON,
            "momentary_rot_button" => LOGIC_MOMENTARY,
            // Untargeted fans keep the cheaper map-tick-derived cosmetic path.
            // A named fan participates in FireTargets and therefore needs a
            // persistent angle/velocity state at runtime.
            "func_rotating" if !ent_value(block, "targetname").unwrap_or("").is_empty() => {
                LOGIC_FUNC_ROTATING
            }
            "func_pendulum"
                if parse_f32_key(block, "distance", 0.0) != 0.0
                    && (parse_spawnflags(block) & 1 != 0
                        || !ent_value(block, "targetname").unwrap_or("").is_empty()) =>
            {
                LOGIC_FUNC_PENDULUM
            }
            "func_breakable" => LOGIC_FUNC_BREAKABLE,
            "func_pushable" if parse_spawnflags(block) & SF_PUSH_BREAKABLE != 0 => {
                LOGIC_FUNC_BREAKABLE
            }
            // CPushable ignores health/material unless SF_PUSH_BREAKABLE is
            // explicitly authored. Its targetname still lives on EntRec kind9.
            "func_pushable" => continue,
            "trigger_teleport" => LOGIC_TRIGGER_TELEPORT,
            "trigger_push" | "func_conveyor" => LOGIC_TRIGGER_PUSH,
            "trigger_gravity" => LOGIC_TRIGGER_GRAVITY,
            "func_healthcharger" => LOGIC_HEALTH_CHARGER,
            "func_recharge" => LOGIC_HEV_CHARGER,
            "monstermaker" => LOGIC_MONSTERMAKER,
            "scripted_sequence" | "aiscripted_sequence" => LOGIC_SCRIPTED,
            "func_train" => LOGIC_FUNC_TRAIN,
            "func_guntarget" => LOGIC_FUNC_GUNTARGET,
            "trigger_endsection" => LOGIC_TRIGGER_ENDSECTION,
            "env_render" => LOGIC_ENV_RENDER,
            "env_beverage" => LOGIC_ENV_BEVERAGE,
            "player_weaponstrip" => LOGIC_WEAPONSTRIP,
            // CBasePlayerItem::DefaultTouch always calls SUB_UseTargets after
            // an accepted touch. Keep a logic identity only for weapons that
            // actually author an output, avoiding records for ordinary drops.
            _ if weapon_pickup_prop_kind(cls).is_some()
                && (!ent_value(block, "target").unwrap_or("").is_empty()
                    || !ent_value(block, "killtarget").unwrap_or("").is_empty()) =>
            {
                LOGIC_WEAPON_PICKUP
            }
            "trigger_once" => LOGIC_TRIGGER_ONCE,
            "trigger_multiple" => LOGIC_TRIGGER_MULTIPLE,
            "trigger_relay" => LOGIC_TRIGGER_RELAY,
            "multi_manager" => LOGIC_MULTI_MANAGER,
            "trigger_auto" => LOGIC_TRIGGER_AUTO,
            "env_message" => LOGIC_ENV_MESSAGE,
            "env_fade" => LOGIC_ENV_FADE,
            "worldspawn" => LOGIC_MAP_FLAGS,
            "trigger_cdaudio" | "target_cdaudio" => LOGIC_CDTRACK,
            "scripted_sentence" => LOGIC_SENTENCE,
            "ambient_generic" if is_voice_message(ent_value(block, "message").unwrap_or("")) => {
                LOGIC_AMBIENT
            }
            "ambient_generic"
                if map_audio_id(
                    &voices,
                    map_index,
                    ent_value(block, "message").unwrap_or(""),
                    parse_spawnflags(block) & 32 == 0,
                ) != u8::MAX =>
            {
                LOGIC_MAP_SOUND
            }
            // PA announcer (Black Mesa intercom): a speaker plays a sentence group.
            // Reuses the per-map voice path -- it cooks only when its line is in the
            // voice pack (the sentence-group extraction is the residual).
            "speaker" if is_voice_message(ent_value(block, "message").unwrap_or("")) => {
                LOGIC_AMBIENT
            }
            "env_shake" => LOGIC_ENV_SHAKE,
            "func_wall_toggle" => LOGIC_WALL_TOGGLE,
            // CFuncWall::Use toggles pev->frame, swapping the brush's textures
            // between their own +N chain and the +a alternate chain (c1a0d's
            // retinal scanner blink). Only named walls can ever be fired.
            "func_wall" | "func_illusionary"
                if !ent_value(block, "targetname").unwrap_or("").is_empty() =>
            {
                LOGIC_WALL_FRAME
            }
            "multisource" => LOGIC_MULTISOURCE,
            "env_global" => LOGIC_ENV_GLOBAL,
            "env_explosion" => LOGIC_ENV_EXPLOSION,
            "env_spark" | "env_debris" => LOGIC_ENV_SPARK,
            "func_monsterclip" => LOGIC_MONSTERCLIP,
            // On A Rail junctions: the path chain is stitched at cook so the train
            // drives straight through (progression). The platform also becomes a
            // +use lever that fires its target -- the minimal runtime routing
            // control (the full rotate-the-platform rig is deferred).
            "func_trackchange" | "func_trackautochange" => LOGIC_FUNC_BUTTON,
            // Mountable guns: the whole func_tank family renders as its brush and
            // is +use-mounted at runtime (aim with the view, fire hitscan). Laser/
            // rocket/mortar variants degrade to a bullet tank (no special projectile).
            "func_tank" | "func_tank2" | "func_tank3" | "func_tanklaser" | "func_tankrocket"
            | "func_tankmortar" => LOGIC_TANK,
            "trigger_changelevel" => LOGIC_TRIGGER_CHANGELEVEL,
            "trigger_transition" => LOGIC_TRIGGER_TRANSITION,
            "info_landmark" => LOGIC_INFO_LANDMARK,
            "trigger_counter" => LOGIC_TRIGGER_COUNTER,
            "trigger_changetarget" => LOGIC_TRIGGER_CHANGETARGET,
            "trigger_hurt" => LOGIC_TRIGGER_HURT,
            "func_tracktrain" => LOGIC_FUNC_TRACKTRAIN,
            "item_suit" => LOGIC_ITEM_SUIT,
            "item_battery" => LOGIC_ITEM_BATTERY,
            "world_items" => match ent_value(block, "type").and_then(|v| v.parse::<u16>().ok()) {
                Some(45) => LOGIC_ITEM_SUIT,
                Some(44) => LOGIC_ITEM_BATTERY,
                _ => continue,
            },
            _ => continue,
        };

        // `introchair` is authored as a named rotating door but nothing in
        // c1a0 targets it, named doors reject touch, and it lacks USE_ONLY.
        // It still needs its EntRec and nonzero angular metadata because its
        // BSP vertices are pivot-local; only the live door state is absent.
        if cls == "func_door_rotating" {
            let targetname = ent_value(block, "targetname").unwrap_or("");
            if !named_door_has_activation(&s, targetname, parse_spawnflags_u32(block)) {
                continue;
            }
        }

        let submodel = block_model(block);
        let brush = submodel
            .and_then(|sm| brush_by_submodel.get(sm).copied())
            .filter(|&b| b != LOGIC_BRUSH_NONE)
            .unwrap_or(LOGIC_BRUSH_NONE);
        if matches!(
            kind,
            LOGIC_FUNC_DOOR
                | LOGIC_WALL_TOGGLE
                | LOGIC_WALL_FRAME
                | LOGIC_FUNC_BUTTON
                | LOGIC_FUNC_BREAKABLE
                | LOGIC_HEALTH_CHARGER
                | LOGIC_HEV_CHARGER
                | LOGIC_TANK
                | LOGIC_FUNC_ROTATING
                | LOGIC_FUNC_PENDULUM
                | LOGIC_FUNC_GUNTARGET
        ) && brush == LOGIC_BRUSH_NONE
        {
            continue;
        }

        let (origin, mins, maxs) = entity_bounds_world(block, models, scale);
        let targetname = names.id(ent_value(block, "targetname"));
        let raw_spawnflags = parse_spawnflags(block);
        let spawnflags = if cls == "func_platrot" {
            // Plat bit 0 is TOGGLE, while the shared door state machine uses
            // bit 5. A named plat starts at its authored TOP/end angle.
            (if targetname != 0 { 1 } else { 0 }) | (if raw_spawnflags & 1 != 0 { 32 } else { 0 })
        } else {
            raw_spawnflags
        };
        let mut target = names.id(ent_value(block, "target"));
        let killtarget = names.id(ent_value(block, "killtarget"));
        let delay_ticks = seconds_to_ticks_u16(parse_f32_key(block, "delay", 0.0));
        let wait_default = match kind {
            LOGIC_FUNC_DOOR => 3.0,
            LOGIC_FUNC_BUTTON => 1.0,
            LOGIC_TRIGGER_ONCE => -1.0,
            LOGIC_TRIGGER_MULTIPLE => 0.2,
            _ => 0.0,
        };
        let wait_ticks = if kind == LOGIC_SCRIPTED {
            // Scripts do not use CBaseToggle::wait. Reuse this signed word for
            // the classname search radius in cooked world units.
            (parse_f32_key(block, "m_flRadius", 0.0) / scale)
                .round()
                .clamp(0.0, i16::MAX as f32) as i16
        } else if kind == LOGIC_SENTENCE {
            // Reuse the signed wait word for scripted_sentence's speaker
            // search radius; its duration/refire cooldown lives in speed.
            (parse_f32_key(block, "radius", 512.0) / scale)
                .round()
                .clamp(0.0, i16::MAX as f32) as i16
        } else {
            seconds_to_ticks_i16(parse_f32_key(block, "wait", wait_default))
        };
        let speed_default = match kind {
            LOGIC_FUNC_BUTTON => 40.0,
            LOGIC_FUNC_DOOR if cls == "func_platrot" => 150.0,
            LOGIC_FUNC_DOOR => 100.0,
            LOGIC_FUNC_TRAIN => 100.0,
            LOGIC_FUNC_TRACKTRAIN => 100.0,
            LOGIC_FUNC_GUNTARGET => 100.0,
            LOGIC_FUNC_ROTATING => 100.0,
            LOGIC_FUNC_PENDULUM => 100.0,
            LOGIC_MOMENTARY => 100.0,
            _ => 0.0,
        };
        let speed = if kind == LOGIC_SCRIPTED {
            // Scripts carry their facing yaw here (q12); they have no speed key.
            hl_yaw_to_world_q12(ent_yaw_degrees(block).unwrap_or(0.0)) as u16
        } else if kind == LOGIC_SENTENCE {
            seconds_to_ticks_u16(
                parse_f32_key(block, "duration", 0.0) + parse_f32_key(block, "refire", 0.0),
            )
        } else if kind == LOGIC_ENV_SHAKE {
            seconds_to_ticks_u16(parse_f32_key(block, "duration", 1.0))
        } else if kind == LOGIC_TANK {
            // firerate = shots/sec -> cooldown ticks at the 20Hz sim (min 2).
            let rate = parse_f32_key(block, "firerate", 1.0).max(0.1);
            (20.0 / rate).round().clamp(2.0, 60.0) as u16
        } else if kind == LOGIC_FUNC_PENDULUM {
            let authored = parse_f32_key(block, "speed", speed_default);
            (if authored > 0.0 {
                authored
            } else {
                speed_default
            })
            .round()
            .clamp(1.0, u16::MAX as f32) as u16
        } else if kind == LOGIC_ENV_MESSAGE {
            let key = ent_value(block, "message").unwrap_or("").to_uppercase();
            titles.get(&key).map(|t| t.hold_ticks).unwrap_or(60)
        } else if kind == LOGIC_ENV_FADE {
            seconds_to_ticks_u16(parse_f32_key(block, "holdtime", 0.0))
        } else if kind == LOGIC_MAP_FLAGS {
            let key = ent_value(block, "chaptertitle")
                .unwrap_or("")
                .to_uppercase();
            titles
                .get(&key)
                .map(|t| t.hold_ticks.max(80))
                .unwrap_or(120)
        } else if speed_default > 0.0 {
            let authored = parse_f32_key(block, "speed", speed_default);
            let authored = if authored > 0.0 {
                authored
            } else {
                speed_default
            };
            (authored / scale).round().clamp(1.0, u16::MAX as f32) as u16
        } else {
            0
        };
        let use_type = match kind {
            LOGIC_TRIGGER_RELAY | LOGIC_TRIGGER_AUTO => triggerstate_use_type(block),
            // Tracktrains never consume their record's relay-use byte. Reuse
            // it for the third authored audio id (startup); sound0 is the
            // running loop and sound1 the brake. This keeps LogicRec at 64 B.
            LOGIC_FUNC_TRACKTRAIN => {
                map_audio_id(&voices, map_index, "plats/ttrain_start1.wav", false)
            }
            _ => USE_TOGGLE,
        };
        let arg0 = match kind {
            LOGIC_ENV_BEVERAGE => {
                // Spawn replaces exactly zero with ten; a positive fractional
                // stock still permits its final can before Use decrements it.
                let stock = parse_f32_key(block, "health", 0.0);
                (if stock == 0.0 { 10.0 } else { stock.ceil() }).clamp(0.0, i16::MAX as f32) as u16
            }
            LOGIC_LIGHTSTYLE => dynamic_light_slot.unwrap_or(0) as u16,
            LOGIC_TRIGGER_CHANGELEVEL => names.id(ent_value(block, "map")),
            LOGIC_TRIGGER_COUNTER => parse_f32_key(block, "count", 2.0)
                .round()
                .clamp(1.0, u16::MAX as f32) as u16,
            LOGIC_TRIGGER_CHANGETARGET => {
                names
                    .id(ent_value(block, "m_iszNewTarget")
                        .or_else(|| ent_value(block, "changetarget")))
            }
            // CTriggerHurt::HurtTouch applies `pev->dmg * 0.5` once per
            // half-second semaphore interval. Store the effective pulse so the
            // fixed 20 Hz runtime does not deal twice GoldSrc's authored damage.
            LOGIC_TRIGGER_HURT => (ent_value(block, "damage")
                .or_else(|| ent_value(block, "dmg"))
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(10.0)
                * 0.5)
                .round()
                .clamp(1.0, u16::MAX as f32) as u16,
            LOGIC_FUNC_TRACKTRAIN => (parse_f32_key(block, "startspeed", 0.0) / scale)
                .round()
                .clamp(0.0, u16::MAX as f32) as u16,
            LOGIC_FUNC_GUNTARGET => parse_f32_key(block, "health", 5.0)
                .round()
                .clamp(1.0, u16::MAX as f32) as u16,
            LOGIC_ENV_RENDER => parse_f32_key(block, "renderamt", 255.0)
                .round()
                .clamp(0.0, 255.0) as u16,
            // Stable cross-map identity for CBasePlatTrain's global
            // overlay. func_train otherwise leaves arg0 unused.
            LOGIC_FUNC_TRAIN => actor_carry_id(ent_value(block, "globalname").unwrap_or(""), true),
            LOGIC_FUNC_BREAKABLE => parse_f32_key(block, "health", 20.0)
                .round()
                .clamp(1.0, u16::MAX as f32) as u16,
            // CFuncRotating::KeyValue converts this authored percentage
            // to a 0.01 multiplier. Zero/missing is replaced by 100% in
            // Spawn, so an ordinary fan reaches its endpoint in one think.
            LOGIC_FUNC_ROTATING => {
                let raw = parse_f32_key(block, "fanfriction", 0.0);
                (if raw > 0.0 { raw } else { 100.0 })
                    .round()
                    .clamp(1.0, u16::MAX as f32) as u16
            }
            LOGIC_FUNC_PENDULUM => parse_f32_key(block, "distance", 0.0)
                .abs()
                .round()
                .clamp(1.0, u16::MAX as f32) as u16,
            LOGIC_TRIGGER_GRAVITY => (parse_f32_key(block, "gravity", 1.0) * 4096.0)
                .round()
                .clamp(0.0, u16::MAX as f32) as u16,
            LOGIC_HEALTH_CHARGER => 50, // HL default juice
            LOGIC_HEV_CHARGER => 75,
            LOGIC_WEAPON_PICKUP => weapon_pickup_prop_kind(cls).unwrap_or(0),
            // Per-shot damage. HL varies by the "bullet" enum; the common
            // player tanks are 12mm (~20). Fixed default is close enough.
            LOGIC_TANK => 20,
            // CMomentaryRotButton::Return uses the separately authored
            // `returnspeed`; the regular `speed` field remains the held turn
            // rate. Keeping both lets runtime send one faithful normalized
            // position to the paired wheels and their USE_SET target.
            LOGIC_MOMENTARY => parse_f32_key(block, "returnspeed", 0.0)
                .round()
                .clamp(0.0, u16::MAX as f32) as u16,
            LOGIC_SCRIPTED => names.id(ent_value(block, "m_iszEntity")),
            LOGIC_ENV_MESSAGE => {
                let raw = ent_value(block, "message").unwrap_or("");
                let key = raw.to_uppercase();
                match titles.get(&key) {
                    Some(t) if !t.text.is_empty() => names.id(Some(&t.text)),
                    // Stock titles.txt deliberately leaves OPENTITLE3/4 empty.
                    // They are timed no-op cards, not literal player-facing
                    // strings. Keep the env_message record (and therefore its
                    // authored target graph) but give it the existing zero
                    // text sentinel so firing it draws nothing.
                    Some(_) => {
                        // Retain the authored token in the local name table.
                        // Besides keeping cooker output/layout stable, another
                        // entity may still address the env_message by that same
                        // string (as c0a0a does with OPENTITLE3).
                        names.id(Some(raw));
                        0
                    }
                    _ if !raw.is_empty() => {
                        let adapted = adapt_hazard_course_text(raw);
                        names.id(Some(&adapted))
                    }
                    _ => continue,
                }
            }
            LOGIC_ENV_FADE => seconds_to_ticks_u16(parse_f32_key(block, "duration", 2.0)),
            LOGIC_CDTRACK => (parse_f32_key(block, "health", 0.0) as i16) as u16,
            LOGIC_ITEM_SUIT => {
                let sentence = if raw_spawnflags & 1 != 0 {
                    "HEV_A0"
                } else {
                    "HEV_AAX"
                };
                voices
                    .get(&(map_index, sentence.to_string()))
                    .copied()
                    .unwrap_or(u16::MAX)
            }
            LOGIC_SENTENCE => {
                let s = ent_value(block, "sentence")
                    .unwrap_or("")
                    .trim_start_matches('!')
                    .to_ascii_uppercase();
                match voices.get(&(map_index, s)) {
                    Some(&id) => id,
                    None => continue, // this line isn't in the per-map voice pack
                }
            }
            LOGIC_AMBIENT => {
                let key = voice_manifest_key(ent_value(block, "message").unwrap_or(""));
                match voices.get(&(map_index, key)) {
                    Some(&id) => id,
                    None => continue,
                }
            }
            LOGIC_ENV_SHAKE => (parse_f32_key(block, "amplitude", 4.0) / scale)
                .round()
                .clamp(1.0, 64.0) as u16,
            // Filled by the registration post-pass once every source has
            // a stable cooked logic index.
            LOGIC_MULTISOURCE => 0,
            LOGIC_ENV_GLOBAL => global_hash(ent_value(block, "globalstate").unwrap_or("")),
            LOGIC_ENV_EXPLOSION => parse_f32_key(block, "iMagnitude", 100.0)
                .round()
                .clamp(1.0, 255.0) as u16,
            LOGIC_MAP_FLAGS => {
                let key = ent_value(block, "chaptertitle")
                    .unwrap_or("")
                    .to_uppercase();
                match titles.get(&key) {
                    Some(t) if !t.text.is_empty() => names.id(Some(&t.text)),
                    _ => 0,
                }
            }
            _ => names.id(ent_value(block, "changetarget")),
        };
        let arg1 = match kind {
            LOGIC_ENV_BEVERAGE => parse_f32_key(block, "skin", 0.0).clamp(0.0, 6.0) as u16,
            LOGIC_TRIGGER_CHANGELEVEL => names.id(ent_value(block, "landmark")),
            LOGIC_FUNC_TRACKTRAIN => submodel.unwrap_or(0).min(u16::MAX as usize) as u16,
            LOGIC_FUNC_GUNTARGET => names.id(ent_value(block, "message")),
            LOGIC_FUNC_BREAKABLE => parse_f32_key(block, "material", 0.0)
                .round()
                .clamp(0.0, 7.0) as u16,
            // GoldSrc's authored key is m_fMoveTo: 0 = pose in place,
            // 1 = walk, 2 = run, 4/5 = instant. Keep accepting the old
            // misspelling so already-modified/custom maps do not regress.
            LOGIC_SCRIPTED => ent_value(block, "m_fMoveTo")
                .or_else(|| ent_value(block, "m_flMoveTo"))
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0)
                .round()
                .clamp(0.0, 7.0) as u16,
            LOGIC_SENTENCE => names.id(ent_value(block, "entity")),
            // effect(0..2) | left_aligned<<2 | y_q5<<3 | fade_ticks<<8
            LOGIC_ENV_MESSAGE => {
                let key = ent_value(block, "message").unwrap_or("").to_uppercase();
                let t = titles.get(&key).cloned().unwrap_or_default();
                pack_title_flags(&t)
            }
            // bit0 = fade-in (HL SF_FADE_IN), bit1 = fade to white-ish
            LOGIC_ENV_FADE => {
                let white = ent_value(block, "rendercolor")
                    .and_then(parse_vec3)
                    .map(|c| c[0] + c[1] + c[2] > 384.0)
                    .unwrap_or(false);
                (spawnflags as u16 & 1) | ((white as u16) << 1)
            }
            LOGIC_ENV_RENDER => brush_render_class(block),
            LOGIC_MULTISOURCE => global_hash(ent_value(block, "globalstate").unwrap_or("")),
            // Master-gated entities carry their `master` (a multisource
            // targetname) here; the runtime keeps them locked until that
            // multisource is satisfied (SDK UTIL_IsMasterTriggered). These are
            // the classes the SDK gates: doors, buttons, trigger_once/multiple/
            // counter, trigger_teleport, func_tank. Their arg1 is otherwise 0.
            LOGIC_FUNC_DOOR
            | LOGIC_FUNC_BUTTON
            | LOGIC_TRIGGER_ONCE
            | LOGIC_TRIGGER_MULTIPLE
            | LOGIC_TRIGGER_COUNTER
            | LOGIC_TRIGGER_TELEPORT
            | LOGIC_TANK => names.id(ent_value(block, "master")),
            LOGIC_ENV_GLOBAL => parse_f32_key(block, "triggermode", 2.0)
                .round()
                .clamp(0.0, 3.0) as u16,
            // bit0 startdark, bit1 gametitle, bits 8..13 = CD music track
            LOGIC_MAP_FLAGS => {
                let dark = parse_f32_key(block, "startdark", 0.0) as u16 != 0;
                let title = parse_f32_key(block, "gametitle", 0.0) as u16 != 0;
                let track = parse_f32_key(block, "sounds", 0.0).round().clamp(0.0, 63.0) as u16;
                (dark as u16) | ((title as u16) << 1) | (track << 8)
            }
            _ => 0,
        };

        // The two bytes that used to pad LogicRec's scalar prefix now carry
        // per-map audio ids. This preserves every authored mover/material
        // choice without growing the 64-byte record or the PS1 RAM image.
        let (sound0, sound1) = match cls {
            "env_beverage" => (
                map_audio_id(&voices, map_index, "weapons/g_bounce3.wav", false),
                acoustic_attenuation(cls, block, spawnflags, scale),
            ),
            "scripted_sentence" | "speaker" => {
                (u8::MAX, acoustic_attenuation(cls, block, spawnflags, scale))
            }
            "ambient_generic" if kind == LOGIC_AMBIENT => {
                (u8::MAX, acoustic_attenuation(cls, block, spawnflags, scale))
            }
            "func_button" | "func_rot_button" | "momentary_rot_button" => (
                map_audio_id(
                    &voices,
                    map_index,
                    ordinal_sound(block, "sounds", &BUTTON_SOUNDS),
                    false,
                ),
                map_audio_id(
                    &voices,
                    map_index,
                    ordinal_sound(block, "locked_sound", &BUTTON_SOUNDS),
                    false,
                ),
            ),
            "func_door" | "func_door_rotating" | "momentary_door" => (
                map_audio_id(
                    &voices,
                    map_index,
                    ordinal_sound(block, "movesnd", &DOOR_MOVE_SOUNDS),
                    true,
                ),
                map_audio_id(
                    &voices,
                    map_index,
                    ordinal_sound(block, "stopsnd", &DOOR_STOP_SOUNDS),
                    false,
                ),
            ),
            "func_plat" | "func_platrot" | "func_train" => (
                map_audio_id(
                    &voices,
                    map_index,
                    ordinal_sound(block, "movesnd", &PLAT_MOVE_SOUNDS),
                    true,
                ),
                map_audio_id(
                    &voices,
                    map_index,
                    ordinal_sound(block, "stopsnd", &PLAT_STOP_SOUNDS),
                    false,
                ),
            ),
            "func_tracktrain" => (
                map_audio_id(
                    &voices,
                    map_index,
                    ordinal_sound(block, "sounds", &TRACKTRAIN_SOUNDS),
                    true,
                ),
                map_audio_id(&voices, map_index, "plats/ttrain_brake1.wav", false),
            ),
            "func_rotating" => {
                let path = ent_value(block, "message")
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| ordinal_sound(block, "sounds", &FAN_SOUNDS));
                (map_audio_id(&voices, map_index, path, true), u8::MAX)
            }
            "func_breakable" | "func_pushable" => {
                let material = parse_f32_key(block, "material", 0.0)
                    .round()
                    .clamp(0.0, 7.0) as usize;
                (
                    map_audio_id(&voices, map_index, BREAK_SOUNDS[material], false),
                    u8::MAX,
                )
            }
            "env_spark" | "env_debris" => (
                map_audio_id(&voices, map_index, "buttons/spark1.wav", false),
                u8::MAX,
            ),
            "ambient_generic" if kind == LOGIC_MAP_SOUND => (
                map_audio_id(
                    &voices,
                    map_index,
                    ent_value(block, "message").unwrap_or(""),
                    spawnflags & 32 == 0,
                ),
                acoustic_attenuation(cls, block, spawnflags, scale),
            ),
            _ => (u8::MAX, u8::MAX),
        };

        let mut record_flags = if kind == LOGIC_SCRIPTED {
            script_class_selector(ent_value(block, "m_iszEntity").unwrap_or(""))
        } else {
            0
        };
        if kind == LOGIC_TRIGGER_HURT {
            const DMG_RADIATION: u32 = 1 << 18;
            let damage_type = ent_value(block, "damagetype")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            if damage_type & DMG_RADIATION != 0 {
                record_flags |= LOGIC_TRIGGER_HURT_RADIATION;
            }
        }
        // Door/button lock feedback has one extra local id. Record flags are
        // class-specific and otherwise unused for these two kinds.
        if kind == LOGIC_FUNC_BUTTON {
            record_flags = map_audio_id(
                &voices,
                map_index,
                ordinal_sound(block, "unlocked_sound", &BUTTON_SOUNDS),
                false,
            );
        } else if kind == LOGIC_FUNC_DOOR
            && matches!(cls, "func_door" | "func_door_rotating" | "momentary_door")
        {
            record_flags = map_audio_id(
                &voices,
                map_index,
                ordinal_sound(block, "locked_sound", &BUTTON_SOUNDS),
                false,
            );
        }
        if kind == LOGIC_SCRIPTED
            && ent_value(block, "m_iszIdle").is_some_and(|idle| !idle.is_empty())
        {
            // Spawn starts CineThink for an authored idle even when the script
            // itself is targeted. Keep this independent of clip availability:
            // unsupported clips still need their actor primed at the mark.
            record_flags |= LOGIC_SCRIPTED_HAS_IDLE;
        }
        if kind == LOGIC_SCRIPTED
            && ent_value(block, "m_iszPlay").is_some_and(|play| !play.is_empty())
        {
            record_flags |= LOGIC_SCRIPTED_HAS_PLAY;
        }
        if matches!(kind, LOGIC_SENTENCE | LOGIC_AMBIENT | LOGIC_MAP_SOUND) {
            record_flags = acoustic_volume(cls, block);
        }

        let first_aux = aux.len().min(u16::MAX as usize) as u16;
        let mut aux_count = 0u8;
        if kind == LOGIC_ENV_RENDER {
            let target = ent_value(block, "target").unwrap_or("");
            if !target.is_empty() {
                for candidate in s.split('{') {
                    if ent_value(candidate, "targetname") != Some(target) {
                        continue;
                    }
                    let brush = ent_value(candidate, "model")
                        .and_then(|value| value.strip_prefix('*'))
                        .and_then(|value| value.parse::<usize>().ok())
                        .and_then(|model| brush_by_submodel.get(model))
                        .copied();
                    if let Some(brush) = brush.filter(|brush| *brush != u16::MAX) {
                        if aux_count == u8::MAX {
                            return Err("too many env_render brush targets".into());
                        }
                        aux.push(LogicAuxRec {
                            target: brush,
                            delay_ticks: 0,
                        });
                        aux_count += 1;
                    }
                }
            }
        }
        if kind == LOGIC_TRIGGER_CHANGELEVEL {
            // CHANGE_LEVEL stores this post-load output in the engine's
            // transition list, not as the changelevel entity's ordinary
            // target. Name ids are map-local; the runtime converts this id to
            // a stable hash before leaving and resolves it in the destination.
            let post_target = names.id(ent_value(block, "changetarget"));
            if post_target != 0 {
                aux.push(LogicAuxRec {
                    target: post_target,
                    delay_ticks: seconds_to_ticks_u16(parse_f32_key(block, "changedelay", 0.0)),
                });
                aux_count = 1;
            }
        }
        if kind == LOGIC_FUNC_DOOR {
            // GoldSrc uses netname as the close-only output, separate from the
            // ordinary target that fires at both travel endpoints.
            let close_target = names.id(ent_value(block, "netname"));
            if close_target != 0 {
                aux.push(LogicAuxRec {
                    target: close_target,
                    delay_ticks: 0,
                });
                aux_count = 1;
            }
        }
        if kind == LOGIC_FUNC_TRAIN || kind == LOGIC_FUNC_TRACKTRAIN || kind == LOGIC_FUNC_GUNTARGET
        {
            // func_train: aux pairs (x,y), (z,wait), conditionally extended
            // to triples with (pass target,speed) only where a path_corner
            // actually authors either value.
            // func_tracktrain: aux triples (x,y), (z,0), (pass target,speed).
            // The selected player tram still uses the compact dedicated path;
            // these records let every secondary tracktrain render, move, and
            // fire its authored path_track messages.
            let uses_path_track = kind == LOGIC_FUNC_TRACKTRAIN || kind == LOGIC_FUNC_GUNTARGET;
            let is_tracktrain = kind == LOGIC_FUNC_TRACKTRAIN;
            let path_class = if uses_path_track {
                "path_track"
            } else {
                "path_corner"
            };
            let mut corner = ent_value(block, "target").unwrap_or("").to_string();
            let first_corner = corner.clone();
            let mut extended = false;
            if !uses_path_track {
                let mut scan_corner = corner.clone();
                let mut scan_seen = Vec::<String>::new();
                let mut scan_hops = 0usize;
                while !scan_corner.is_empty() && scan_hops < 80 {
                    if scan_seen.iter().any(|name| name == &scan_corner) {
                        break;
                    }
                    scan_seen.push(scan_corner.clone());
                    let mut found = false;
                    for cb in s.split('{') {
                        if ent_value(cb, "classname") != Some(path_class)
                            || ent_value(cb, "targetname") != Some(scan_corner.as_str())
                        {
                            continue;
                        }
                        extended |= parse_f32_key(cb, "speed", 0.0) > 0.0
                            || !ent_value(cb, "message").unwrap_or("").is_empty();
                        let next = ent_value(cb, "target").unwrap_or("").to_string();
                        scan_corner = next;
                        found = true;
                        break;
                    }
                    if !found {
                        break;
                    }
                    scan_hops += 1;
                }
                if extended {
                    record_flags |= LOGIC_TRAIN_EXTENDED;
                }
            }
            if kind == LOGIC_FUNC_GUNTARGET {
                // Runtime treats gun targets as cyclic func_train machinery,
                // but path_track records still need their third pass/speed word.
                record_flags |= LOGIC_TRAIN_EXTENDED;
            }
            let stride = if uses_path_track || extended {
                3u8
            } else {
                2u8
            };
            let owner_id = ent_value(block, "model")
                .or_else(|| ent_value(block, "targetname"))
                .unwrap_or("");
            let owns_fire_once = track_path_owner
                .get(&first_corner)
                .map(|owner| owner == owner_id)
                .unwrap_or(true);
            let height = if is_tracktrain {
                (parse_f32_key(block, "height", 0.0) / scale).round() as i32
            } else {
                0
            };
            let mut dead_end_target = 0u16;
            let mut cycle_start = None;
            let mut seen_corners = Vec::<String>::new();
            let mut hops = 0usize;
            while !corner.is_empty() && hops < 80 {
                seen_corners.push(corner.clone());
                let mut found = false;
                for cb in s.split('{') {
                    if ent_value(cb, "classname") != Some(path_class) {
                        continue;
                    }
                    if ent_value(cb, "targetname") != Some(corner.as_str()) {
                        continue;
                    }
                    let mut o = to_world(
                        ent_value(cb, "origin")
                            .and_then(parse_vec3)
                            .unwrap_or([0.0; 3]),
                        scale,
                    );
                    o[1] += height;
                    let wait_seconds = parse_f32_key(cb, "wait", 0.0);
                    let wait = if uses_path_track {
                        0
                    } else {
                        pack_train_corner_wait(wait_seconds, parse_spawnflags(cb))
                    };
                    if aux.len() + stride as usize <= u16::MAX as usize
                        && aux_count <= u8::MAX - stride
                    {
                        aux.push(LogicAuxRec {
                            target: o[0].clamp(i16::MIN as i32, i16::MAX as i32) as u16,
                            delay_ticks: o[1].clamp(i16::MIN as i32, i16::MAX as i32) as u16,
                        });
                        aux.push(LogicAuxRec {
                            target: o[2].clamp(i16::MIN as i32, i16::MAX as i32) as u16,
                            delay_ticks: wait,
                        });
                        if uses_path_track || extended {
                            let pass = if uses_path_track {
                                let fire_once = parse_spawnflags(cb) & 2 != 0;
                                if is_tracktrain && fire_once && !owns_fire_once {
                                    0
                                } else {
                                    names.id(ent_value(cb, "message"))
                                }
                            } else {
                                names.id(ent_value(cb, "message"))
                            };
                            let node_speed = (parse_f32_key(cb, "speed", 0.0) / scale)
                                .round()
                                .clamp(0.0, u16::MAX as f32)
                                as u16;
                            aux.push(LogicAuxRec {
                                target: pass,
                                delay_ticks: node_speed,
                            });
                        }
                        aux_count += stride;
                    }
                    let next = ent_value(cb, "target").unwrap_or("").to_string();
                    if is_tracktrain && next.is_empty() {
                        dead_end_target = names.id(ent_value(cb, "netname"));
                    }
                    // Any repeated target closes a cycle, including a tail
                    // cycle that does not return to the train's first corner.
                    // Store its start index in LogicEnt.flags instead of
                    // serializing the repeated suffix up to the hop cap.
                    corner = if let Some(index) = seen_corners.iter().position(|name| name == &next)
                    {
                        if !next.is_empty() {
                            cycle_start = Some(index);
                        }
                        String::new()
                    } else {
                        next
                    };
                    found = true;
                    break;
                }
                if !found {
                    break;
                }
                hops += 1;
            }
            if is_tracktrain {
                // The path name is no longer needed after cooking. Reuse the
                // record's target for CFuncTrackTrain::DeadEnd's netname fire.
                target = dead_end_target;
            } else if let Some(start) = cycle_start.filter(|start| *start < 63) {
                record_flags |= ((start as u8 + 1) << LOGIC_TRAIN_CYCLE_SHIFT) as u8;
            } else {
                record_flags |= LOGIC_TRAIN_TERMINAL;
            }
        }
        if kind == LOGIC_MULTI_MANAGER {
            let mut targets: Vec<(u16, u16)> = Vec::new();
            // The old cap of 16 (the SDK's MAX_MULTI_TARGETS) truncated c1a0d's
            // retinal-scanner rig, which authors 33 timed outputs (12 scanner
            // frame toggles + 19 audio blips + the door + a duplicate). Losing
            // even one toggle flips the blink's parity and strands the scanner
            // animating, so keep the whole authored list (64 is ample; aux
            // records cost 4 bytes each on disc and stream, not resident RAM).
            iter_ent_pairs(block, |key, value| {
                if logic_common_key(key) || targets.len() >= 64 {
                    return;
                }
                // GoldSrc's CMultiManager passes every authored key through
                // UTIL_StripToken: Hammer represents duplicate outputs as
                // `target`, `target#1`, `target#2`, but every one fires the
                // same targetname. Keeping the suffix strands the c0a0d tram
                // after pausemm because its delayed `train#1` resume never
                // reaches the entity named `train`.
                let target = multi_manager_target_key(key);
                let target_id = names.id(Some(target));
                if target_id == 0 {
                    return;
                }
                let delay = value.parse::<f32>().unwrap_or(0.0);
                targets.push((target_id, seconds_to_ticks_u16(delay)));
            });
            targets.sort_by(|a, b| a.1.cmp(&b.1));
            for (target_id, delay) in targets {
                if aux.len() >= u16::MAX as usize {
                    break;
                }
                aux.push(LogicAuxRec {
                    target: target_id,
                    delay_ticks: delay,
                });
                aux_count = aux_count.saturating_add(1);
            }
            target = 0;
        }
        if kind == LOGIC_TRIGGER_TELEPORT {
            // Resolve the destination at cook time; pack world (x,y),(z,yaw)
            // as two aux entries (world coords fit i16).
            let dest_name = ent_value(block, "target").unwrap_or("");
            let Some((_, d, dyaw)) = tp_dests.iter().find(|(n, _, _)| n == dest_name) else {
                continue; // unresolvable teleport: skip rather than strand players
            };
            aux.push(LogicAuxRec {
                target: d[0] as i16 as u16,
                delay_ticks: d[1] as i16 as u16,
            });
            aux.push(LogicAuxRec {
                target: d[2] as i16 as u16,
                delay_ticks: *dyaw,
            });
            aux_count = 2;
            target = 0;
        }
        if kind == LOGIC_SCRIPTED {
            // aux[0] packs (slot+1) in six low bits and the retail sequence's
            // 100 ms duration quanta above it for play/idle respectively;
            // zero means none. Remaining aux
            // records are pairs for source MDL event 1003: (target name id,
            // event tick), then (source period, 0 idle / 1 play).
            let (play, idle) = script_clip_slots(&s, block, transition_types, &clips);
            let entity_name = ent_value(block, "m_iszEntity").unwrap_or("");
            let ty = script_target_monster_type(&s, entity_name, transition_types);
            let mut events: Vec<(bool, StudioTargetEvent)> = Vec::new();
            if let Some(ty) = ty {
                for (key, play_event) in [("m_iszPlay", true), ("m_iszIdle", false)] {
                    let Some(name) = ent_value(block, key) else {
                        continue;
                    };
                    events.extend(
                        studio_events
                            .get(&(ty, name.to_ascii_lowercase()))
                            .into_iter()
                            .flatten()
                            .cloned()
                            .map(|event| (play_event, event)),
                    );
                }
            }
            if play != 0 || idle != 0 || !events.is_empty() {
                aux.push(LogicAuxRec {
                    target: play,
                    delay_ticks: idle,
                });
                aux_count = 1;
                for (play_event, event) in events {
                    if aux_count > u8::MAX - 2 {
                        break;
                    }
                    let target = names.id(Some(&event.target));
                    if target == 0 {
                        continue;
                    }
                    aux.push(LogicAuxRec {
                        target,
                        delay_ticks: event.tick,
                    });
                    aux.push(LogicAuxRec {
                        target: event.period,
                        delay_ticks: play_event as u16,
                    });
                    aux_count += 2;
                }
            }
        }
        if kind == LOGIC_TRIGGER_PUSH {
            // Per-tick world push vector from HL angles + speed (u/s at 20 Hz).
            // func_conveyor: the belt itself pushes standers -- same math, but
            // the belt moves slower relative to its speed key in HL feel.
            let is_conveyor = cls == "func_conveyor";
            let spd_key = parse_f32_key(block, "speed", if is_conveyor { 100.0 } else { 100.0 });
            let spd = (if is_conveyor { spd_key * 0.5 } else { spd_key }) / scale / 20.0;
            let hl_dir = ent_move_dir(block);
            let w = to_world(
                [
                    hl_dir[0] * spd * scale,
                    hl_dir[1] * spd * scale,
                    hl_dir[2] * spd * scale,
                ],
                scale,
            );
            aux.push(LogicAuxRec {
                target: w[0] as i16 as u16,
                delay_ticks: w[1] as i16 as u16,
            });
            aux.push(LogicAuxRec {
                target: w[2] as i16 as u16,
                delay_ticks: 0,
            });
            aux_count = 2;
        }

        out.push(LogicRec {
            kind,
            use_type,
            spawnflags,
            targetname,
            target,
            killtarget,
            brush,
            first_aux,
            aux_count,
            flags: record_flags,
            wait_ticks,
            delay_ticks,
            speed,
            arg0,
            arg1,
            sound0,
            sound1,
            origin,
            mins,
            maxs,
        });
        source_raw_index.push(raw_index);
    }

    // GoldSrc CMultiSource::Register first finds every raw entity whose
    // `target` names this multisource, then registers each multi_manager that
    // has a (suffix-stripped) output for it. Keep duplicate MM outputs as
    // separate timed aux records above, but the manager itself is one input.
    // Membership aux records store the exact cooked source LogicRec index.
    {
        let cooked_by_raw: std::collections::HashMap<usize, usize> = source_raw_index
            .iter()
            .enumerate()
            .map(|(logic_index, raw_index)| (*raw_index, logic_index))
            .collect();
        let raw_blocks: Vec<&str> = s.split('{').collect();
        let mut plans: Vec<(usize, Vec<u16>)> = Vec::new();

        for (ms_index, ms) in out.iter().enumerate() {
            if ms.kind != LOGIC_MULTISOURCE || ms.targetname == 0 {
                continue;
            }
            let ms_name = names
                .names
                .get(ms.targetname as usize - 1)
                .cloned()
                .unwrap_or_default();
            let mut members = Vec::new();

            // Direct raw-target sources retain raw entity order. An entity that
            // GoldSrc would register but this cooker discarded must not silently
            // turn a required AND input into an impossible anonymous count.
            for (raw_index, raw) in raw_blocks.iter().enumerate() {
                if ent_value(raw, "target") != Some(ms_name.as_str()) {
                    continue;
                }
                let Some(&source_index) = cooked_by_raw.get(&raw_index) else {
                    let cls = ent_value(raw, "classname").unwrap_or("<unknown>");
                    return Err(format!(
                        "multisource {ms_name:?} requires uncooked direct source #{raw_index} ({cls})"
                    ));
                };
                members.push(source_index as u16);
            }

            // Register a matching manager once even when Hammer authored
            // target, target#1, target#2 as separate delayed outputs.
            for (source_index, source) in out.iter().enumerate() {
                if source.kind != LOGIC_MULTI_MANAGER {
                    continue;
                }
                let has_target = (0..source.aux_count as usize).any(|ai| {
                    aux.get(source.first_aux as usize + ai)
                        .is_some_and(|a| a.target == ms.targetname)
                });
                if has_target && !members.contains(&(source_index as u16)) {
                    members.push(source_index as u16);
                }
            }

            if members.len() > 32 {
                return Err(format!(
                    "multisource {ms_name:?} has {} inputs; GoldSrc/runtime limit is 32",
                    members.len()
                ));
            }
            plans.push((ms_index, members));
        }

        for (ms_index, members) in plans {
            if aux.len() + members.len() > u16::MAX as usize {
                return Err("logic aux table overflow while registering multisources".into());
            }
            let first = aux.len() as u16;
            for source_index in &members {
                aux.push(LogicAuxRec {
                    target: *source_index,
                    delay_ticks: 0,
                });
            }
            out[ms_index].first_aux = first;
            out[ms_index].aux_count = members.len() as u8;
            out[ms_index].arg0 = members.len() as u16;
        }
    }

    // GoldSrc links untargeted doors whose closed bounds touch: opening one
    // half of a split door opens its partner(s). Group them here (union-find
    // over AABB overlap) and stamp the group id into arg0 (doors otherwise
    // leave it 0); the runtime activates the whole group on touch/use.
    {
        let door_idx: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, r)| r.kind == LOGIC_FUNC_DOOR && r.targetname == 0)
            .map(|(i, _)| i)
            .collect();
        let overlap = |a: &LogicRec, b: &LogicRec| -> bool {
            (0..3).all(|k| a.mins[k] <= b.maxs[k] + 2 && b.mins[k] <= a.maxs[k] + 2)
        };
        let mut group_of = vec![usize::MAX; door_idx.len()];
        let mut next_group = 1u16;
        for i in 0..door_idx.len() {
            for j in 0..i {
                if overlap(&out[door_idx[i]], &out[door_idx[j]]) {
                    if group_of[j] == usize::MAX {
                        group_of[j] = next_group as usize;
                        next_group += 1;
                    }
                    group_of[i] = group_of[j];
                    break;
                }
            }
        }
        for (k, &di) in door_idx.iter().enumerate() {
            if group_of[k] != usize::MAX {
                out[di].arg0 = group_of[k] as u16;
            }
        }
    }

    // env_beam / env_laser: static (START_ON) beams -> LOGIC_BEAM. Endpoints are
    // LightningStart/LightningEnd targetnames (info_target origins), resolved here
    // + stored as 4 aux entries (start xy/z, end xy/z; world coords fit i16). arg1
    // = world half-width, speed = 15-bit packed color. Triggered beams (no
    // START_ON) need logic wiring and are left for a later pass.
    {
        let sprites_manifest = load_sprites_manifest();
        let tn_origin = |name: &str| -> Option<[i32; 3]> {
            if name.is_empty() {
                return None;
            }
            for b in s.split('{') {
                if ent_value(b, "targetname") == Some(name) {
                    if let Some(o) = ent_value(b, "origin").and_then(parse_vec3) {
                        return Some(to_world(o, scale));
                    }
                }
            }
            None
        };
        for block in s.split('{') {
            let cls = ent_value(block, "classname").unwrap_or("");
            if cls != "env_beam" && cls != "env_laser" {
                continue;
            }
            // START_ON beams draw from load; toggled ones (no START_ON) stay off
            // until their target fires. spawnflags bit0 carries that initial state.
            let start_on = (parse_spawnflags(block) & 1) != 0;
            let beam_targetname = names.id(ent_value(block, "targetname"));
            if !start_on && beam_targetname == 0 {
                continue; // never START_ON and nothing can toggle it -> skip
            }
            let own = ent_value(block, "origin")
                .and_then(parse_vec3)
                .map(|o| to_world(o, scale));
            let start = tn_origin(ent_value(block, "LightningStart").unwrap_or("")).or(own);
            let end = tn_origin(ent_value(block, "LightningEnd").unwrap_or(""))
                .or_else(|| tn_origin(ent_value(block, "target").unwrap_or("")));
            let Some(start) = start else {
                continue;
            };
            // No fixed end + a Radius = a random-area beam (CLightning::RandomArea
            // strikes random points around the origin). end mirrors start; the
            // runtime picks strike endpoints. A beam with neither is dropped.
            let radius_hl = parse_f32_key(block, "Radius", 0.0);
            let (end, random_area) = match end {
                Some(e) => (e, false),
                None if radius_hl > 0.0 => (start, true),
                None => continue,
            };
            let width_hl = parse_f32_key(block, "BoltWidth", 16.0).max(1.0);
            let half = ((width_hl * 0.5 * scale).round() as i32).clamp(1, 4000) as u16;
            let col = ent_value(block, "rendercolor")
                .and_then(parse_vec3)
                .unwrap_or([255.0, 255.0, 255.0]);
            // Half-weighted renderamt: full scaling makes low-amt beams vanish in
            // BGR555; halved keeps the grey smoke pillars faintly visible like HL.
            let amt = parse_f32_key(block, "renderamt", 255.0).clamp(0.0, 255.0);
            let amt_q = (255.0 + amt) / 510.0;
            let bgr = to_bgr555(
                (col[0] * amt_q) as u8,
                (col[1] * amt_q) as u8,
                (col[2] * amt_q) as u8,
            );
            // The beam sprite (lgtning/plasma/laserbeam), already in this map's
            // HSPR pack -- carry its local id so the runtime can texture the beam.
            let sprite_slot = ent_value(block, "texture")
                .map(|t| {
                    t.rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(t)
                        .to_ascii_lowercase()
                })
                .and_then(|base| sprites_manifest.get(&(map_index, base)))
                .map(|&(lid, _, _)| lid)
                .filter(|&lid| lid < 15);
            let noise = ((parse_f32_key(block, "NoiseAmplitude", 0.0).max(0.0) * scale) as u32)
                .min(2047) as u16;
            let arg0 = sprite_slot.map(|l| l + 1).unwrap_or(0)
                | ((random_area as u16) << 4)
                | (noise << 5);
            let life_ticks =
                ((parse_f32_key(block, "life", 0.0).max(0.0) * 20.0) as u32).min(255) as u16;
            let scroll =
                ((parse_f32_key(block, "TextureScroll", 0.0).max(0.0)) as u32).min(255) as u16;
            let strike = parse_f32_key(block, "StrikeTime", 1.0).abs();
            let rand_word = if random_area {
                let radius = ((radius_hl * scale) as u32).clamp(1, 4095) as u16;
                let period4 = (((strike * 20.0) as u32) / 4).clamp(1, 15) as u16;
                radius | (period4 << 12)
            } else {
                // Striking beams (life > 0) re-strike every life + StrikeTime
                // (GoldSrc StrikeThink); the full u16 holds huge StrikeTimes
                // (debris beams use 9999 = effectively one strike).
                (((parse_f32_key(block, "life", 0.0).max(0.0) + strike) * 20.0) as u32)
                    .clamp(1, 65535) as u16
            };
            if aux.len() + 4 > u16::MAX as usize {
                continue;
            }
            let first = aux.len() as u16;
            for (p, spare) in [(start, life_ticks | (scroll << 8)), (end, rand_word)] {
                aux.push(LogicAuxRec {
                    target: p[0] as i16 as u16,
                    delay_ticks: p[1] as i16 as u16,
                });
                aux.push(LogicAuxRec {
                    target: p[2] as i16 as u16,
                    delay_ticks: spare,
                });
            }
            out.push(LogicRec {
                kind: LOGIC_BEAM,
                use_type: 0,
                spawnflags: start_on as u16, // bit0 = START_ON (draw from load)
                targetname: beam_targetname,
                target: 0,
                killtarget: 0,
                brush: LOGIC_BRUSH_NONE,
                first_aux: first,
                aux_count: 4,
                flags: 0,
                wait_ticks: 0,
                delay_ticks: 0,
                speed: bgr,
                arg0,
                arg1: half,
                sound0: u8::MAX,
                sound1: u8::MAX,
                origin: start,
                mins: start,
                maxs: end,
            });
        }
    }

    Ok(LogicCook {
        ents: out,
        aux,
        names: names.names,
    })
}

#[inline]
fn pack_tram_motion(speed: i32, start: usize, wheels: i32) -> u32 {
    // Marker + speed[11:0] + wheels[9:0] + start[7:0]. This retains the
    // original tracktrain look-ahead without growing the room format.
    0x8000_0000
        | speed.clamp(0, 0x0fff) as u32
        | ((wheels.clamp(0, 0x03ff) as u32) << 12)
        | ((start.min(0xff) as u32) << 22)
}

/// The `func_tracktrain` (tram) submodel, speed, authored-start index, and its
/// `path_track` waypoint chain (world coords). Returns an empty chain if the map
/// has no tram. Unique upstream predecessors are retained so a train carried
/// across a changelevel can reattach before the map-authored starting node.
#[cfg(test)]
fn collect_tram(
    ents: &[u8],
    scale: f32,
) -> (u16, i32, u16, i32, Vec<([i32; 3], u16, String)>, [i32; 3]) {
    collect_tram_ranked(ents, &[], scale)
}

fn collect_tram_ranked(
    ents: &[u8],
    models: &[u8],
    scale: f32,
) -> (u16, i32, u16, i32, Vec<([i32; 3], u16, String)>, [i32; 3]) {
    let s = entity_text(ents);
    // (targetname, origin, target, speed, message): a nonzero path_track
    // "speed" key changes the train's speed as it passes (CPathTrack), and
    // "message" is HL's fire-on-pass -- c0a0b's ride fires the multi_manager
    // that fires the c0a0c changelevel this way (the trigger brush itself is
    // a rider-unreachable plate).
    let mut tracks: Vec<(String, [f32; 3], String, u16, String, String)> = Vec::new();
    // func_trackchange junctions: (toptrack, bottomtrack, speed) = the START
    // names of the two path chains the platform swaps between, plus the
    // platform travel speed. A vertical change is serialized as one synthetic
    // tram waypoint, so this needs no new room-format field.
    let mut trackchanges: Vec<(String, String, u16)> = Vec::new();
    // Multiple maps contain decorative/secondary tracktrains. Hazard Course
    // uniquely authors the actual 246-face car and a six-face control brush
    // with the same identity; "last wins" selected the control, leaving the
    // visible car static. Keep every candidate until model sizes can rank them.
    let mut trains: Vec<(u16, i32, String, i32, i32, [i32; 3], String, String)> = Vec::new();
    for block in s.split('{') {
        match ent_value(block, "classname") {
            Some("path_track") => tracks.push((
                ent_value(block, "targetname").unwrap_or("").to_string(),
                ent_value(block, "origin")
                    .and_then(parse_vec3)
                    .unwrap_or([0.0; 3]),
                ent_value(block, "target").unwrap_or("").to_string(),
                ent_value(block, "speed")
                    .and_then(|v| v.parse::<f32>().ok())
                    .map(|v| (v / scale).max(0.0) as u16)
                    .unwrap_or(0),
                ent_value(block, "message").unwrap_or("").to_string(),
                ent_value(block, "netname").unwrap_or("").to_string(),
            )),
            Some("func_trackchange") | Some("func_trackautochange") => trackchanges.push((
                ent_value(block, "toptrack").unwrap_or("").to_string(),
                ent_value(block, "bottomtrack").unwrap_or("").to_string(),
                ent_value(block, "speed")
                    .and_then(|v| v.parse::<f32>().ok())
                    .map(|v| (v / scale).max(0.0) as u16)
                    .unwrap_or(100),
            )),
            Some("func_tracktrain") => {
                let model = ent_value(block, "model")
                    .and_then(|value| value.strip_prefix('*'))
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                let speed = ent_value(block, "speed")
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(100.0) as i32;
                // CFuncTrackTrain::Spawn substitutes 100 when this key is
                // absent or zero. Convert it through the same map scale as the
                // waypoint coordinates.
                let wheels = ent_value(block, "wheels")
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| *v != 0.0)
                    .map(|v| (v / scale).round() as i32)
                    .unwrap_or(100);
                let first = ent_value(block, "target").unwrap_or("").to_string();
                // CFuncTrackTrain offsets its path reference vertically by
                // `height`. Gold Z becomes world Y after the axis swap.
                let height = ent_value(block, "height")
                    .and_then(|v| v.parse::<f32>().ok())
                    .map(|v| (v / scale).round() as i32)
                    .unwrap_or(0);
                let origin = to_world(
                    ent_value(block, "origin")
                        .and_then(parse_vec3)
                        .unwrap_or([0.0; 3]),
                    scale,
                );
                trains.push((
                    model,
                    speed,
                    first,
                    wheels,
                    height,
                    origin,
                    ent_value(block, "targetname").unwrap_or("").to_string(),
                    ent_value(block, "globalname").unwrap_or("").to_string(),
                ));
            }
            _ => {}
        }
    }
    let face_count = |submodel: u16| -> i32 {
        i32le(models, submodel as usize * SZ_MODEL + 60)
            .unwrap_or(0)
            .max(0)
    };
    let selected = trains
        .iter()
        .enumerate()
        .filter(|(_, train)| train.6 == "train")
        .max_by_key(|(_, train)| face_count(train.0))
        .map(|(index, _)| index)
        .or_else(|| {
            let last = trains.last()?;
            let matching: Vec<_> = trains
                .iter()
                .enumerate()
                .filter(|(_, train)| {
                    !last.6.is_empty()
                        && train.6 == last.6
                        && train.7 == last.7
                        && train.2 == last.2
                })
                .collect();
            if models.is_empty() || matching.len() < 2 {
                Some(trains.len() - 1)
            } else {
                matching
                    .into_iter()
                    .max_by_key(|(_, train)| face_count(train.0))
                    .map(|(index, _)| index)
            }
        });
    let Some(selected) = selected else {
        return (0, 0, 0, 100, Vec::new(), [0; 3]);
    };
    let (model, speed, first, wheels, height, origin, _, _) = trains.swap_remove(selected);
    if model == 0 || first.is_empty() {
        return (0, 0, 0, 100, Vec::new(), [0; 3]);
    }

    // Walk backward only while the predecessor is unique. Ambiguous forks are
    // not safe to guess, and a name set prevents circular tracks from filling
    // the compact 256-node budget. A trackchange is also a path edge even
    // though GoldSrc records it as toptrack/bottomtrack instead of `target`.
    let mut prefix = Vec::<String>::new();
    let mut cursor = first.clone();
    let mut prefix_synthetic = 0usize;
    let mut upstream_seen = HashSet::new();
    upstream_seen.insert(cursor.clone());
    loop {
        let mut predecessor = None;
        let mut ambiguous = false;
        for track in tracks.iter().filter(|track| track.2 == cursor) {
            if predecessor.is_some() {
                ambiguous = true;
                break;
            }
            predecessor = Some(track.0.clone());
        }
        if ambiguous {
            break;
        }

        // c0a0b's real incoming rail ends at upper1, whose connection to
        // lower1 is implicit in the `goingdown` autochange. Cross that edge
        // only when no ordinary predecessor exists and the partner is unique.
        let mut crossed_trackchange = false;
        if predecessor.is_none() {
            let mut partner: Option<String> = None;
            for (top, bottom, _) in &trackchanges {
                let other = if top == &cursor {
                    bottom
                } else if bottom == &cursor {
                    top
                } else {
                    continue;
                };
                if other.is_empty() || !tracks.iter().any(|track| &track.0 == other) {
                    continue;
                }
                if let Some(existing) = &partner {
                    if existing != other {
                        ambiguous = true;
                        break;
                    }
                } else {
                    partner = Some(other.clone());
                }
            }
            if ambiguous {
                break;
            }
            if let Some(name) = partner {
                // Seeing the partner already means we reached the reverse side
                // of this same two-way junction. Keep the useful prefix rather
                // than clearing it as though it were a path_track cycle.
                if upstream_seen.contains(&name) {
                    break;
                }
                predecessor = Some(name);
                crossed_trackchange = true;
            }
        }

        let Some(name) = predecessor else { break };
        if name.is_empty() {
            break;
        }
        // Leave room for the authored node and a forward suffix. A crossed
        // trackchange costs both its named endpoint and one synthetic platform
        // point in the emitted route.
        let emitted_cost = 1 + usize::from(crossed_trackchange);
        if prefix.len() + prefix_synthetic + emitted_cost >= 255 {
            prefix.clear();
            break;
        }
        if !upstream_seen.insert(name.clone()) {
            // A circular predecessor walk cannot produce a meaningful root.
            // Retain one authored-first lap instead of starting at its tail.
            prefix.clear();
            break;
        }
        prefix.push(name.clone());
        prefix_synthetic += usize::from(crossed_trackchange);
        cursor = name;
    }
    prefix.reverse();

    let mut way = Vec::new();
    let mut tram_start = None;
    let mut pending_speed = HashMap::<String, u16>::new();
    // Follow the path_track `target` chain; when a segment dead-ends at a
    // func_trackchange junction, stitch onto the platform's other chain so the
    // train reaches the exit instead of stopping. ponytail: we skip the rotate-
    // the-platform puzzle -- the train just drives straight through the junction.
    let mut seg_start = prefix.first().cloned().unwrap_or(first.clone());
    let mut used_tc = vec![false; trackchanges.len()];
    let mut forward_seen = HashSet::new();
    'segments: loop {
        let mut name = seg_start.clone();
        let mut last_found = String::new();
        while !name.is_empty() && way.len() < 256 {
            if !forward_seen.insert(name.clone()) {
                break;
            }
            match tracks.iter().find(|t| t.0 == name) {
                Some(t) => {
                    // `message` fires when the train passes this node;
                    // `netname` fires at a dead end (CFuncTrackTrain::DeadEnd).
                    // The compact tram format has one pass slot, so use the
                    // dead-end target at the terminal node. Authored intro
                    // nodes do not combine both keys.
                    let pass = if t.2.is_empty() && !t.5.is_empty() {
                        t.5.clone()
                    } else {
                        t.4.clone()
                    };
                    let resume = pending_speed.remove(&t.0).unwrap_or(0);
                    let node_speed = if t.3 > 0 { t.3 } else { resume };
                    let mut point = to_world(t.1, scale);
                    point[1] = point[1].saturating_add(height);
                    if t.0 == first && tram_start.is_none() {
                        // Synthetic trackchange points can precede the authored
                        // target, so prefix.len() is not a safe start index.
                        tram_start = Some(way.len());
                    }
                    way.push((point, node_speed, pass));
                    last_found = name.clone();
                    name = t.2.clone();
                }
                None => break,
            }
        }
        // The junction node is where the ridden chain meets the platform -- it
        // can be the segment's START (train parked on the platform, c2a1) or its
        // END (train drives into the platform, c2a2). Match either against a
        // trackchange's top/bottom and continue on the platform's other chain.
        // Prefer a junction at the dead-end just reached. Retain the prior
        // segment-start fallback for maps whose train begins parked on a
        // platform, but never guess between multiple eligible junctions.
        let mut next: Option<(usize, String, bool)> = None;
        let mut tc_ambiguous = false;
        for at_end in [true, false] {
            let at = if at_end { &last_found } else { &seg_start };
            if at.is_empty() {
                continue;
            }
            for (i, (top, bottom, _)) in trackchanges.iter().enumerate() {
                if used_tc[i] {
                    continue;
                }
                let other = if top == at {
                    bottom
                } else if bottom == at {
                    top
                } else {
                    continue;
                };
                if other.is_empty() || !tracks.iter().any(|track| &track.0 == other) {
                    continue;
                }
                if next.is_some() {
                    tc_ambiguous = true;
                    break;
                }
                next = Some((i, other.clone(), at_end));
            }
            if tc_ambiguous || next.is_some() {
                break;
            }
        }
        if tc_ambiguous {
            break;
        }
        match next {
            Some((tc_index, n, at_end)) if way.len() < 256 => {
                used_tc[tc_index] = true;
                if at_end {
                    let tc_speed = trackchanges[tc_index].2;
                    let resume_speed = way
                        .iter()
                        .rev()
                        .find_map(|(_, node_speed, _)| (*node_speed > 0).then_some(*node_speed))
                        .unwrap_or_else(|| speed.clamp(0, u16::MAX as i32) as u16);
                    if let Some(last) = way.last_mut() {
                        // Preserve tuple field 2: upper1's terminal netname
                        // `goingdown` still fires as the platform starts.
                        last.1 = tc_speed;
                    }

                    let from = tracks.iter().find(|track| track.0 == last_found);
                    let to = tracks.iter().find(|track| track.0 == n);
                    if let (Some(from), Some(to)) = (from, to) {
                        // The intro autochange first translates vertically at
                        // the upper track's GoldSrc X/Y, then releases the car
                        // toward the lower path node. Splitting those components
                        // reproduces the 1271-unit descent with existing waypoint
                        // data: upper1 -> synthetic bottom -> lower1.
                        let mut synthetic = to_world([from.1[0], from.1[1], to.1[2]], scale);
                        synthetic[1] = synthetic[1].saturating_add(height);
                        let mut destination = to_world(to.1, scale);
                        destination[1] = destination[1].saturating_add(height);
                        if way.last().map(|last| last.0) != Some(synthetic) {
                            if synthetic == destination {
                                // Purely vertical junction: the destination is
                                // itself the platform endpoint, so restore the
                                // train speed when that named node is emitted.
                                pending_speed.insert(n.clone(), resume_speed);
                            } else if way.len() + 1 < 256 {
                                way.push((synthetic, resume_speed, String::new()));
                            } else {
                                break 'segments;
                            }
                        } else {
                            pending_speed.insert(n.clone(), resume_speed);
                        }
                    }
                }
                seg_start = n;
            }
            _ => break,
        }
    }
    let tram_start = tram_start.unwrap_or(0).min(u16::MAX as usize) as u16;
    (model, speed, tram_start, wheels, way, origin)
}

const LOOT_SLOT_FLAGS: u16 = 0x4000 | 0x1000; // DORMANT | PRISONER, decoded specially by runtime
const HGRUNT_9MMAR: u32 = 1 << 0;
const HGRUNT_GRENADELAUNCHER: u32 = 1 << 2;
const HGRUNT_SHOTGUN: u32 = 1 << 3;

/// Retail monster death drops. Half-Life does not have a generic random loot
/// table: Barney drops his Glock, while each human grunt drops the weapon in
/// its authored `weapons` mask and, when fitted, an M203 grenade pickup.
fn monster_loot_types(cls: &str, block: &str) -> [u16; 2] {
    match cls {
        "monster_barney" => [27, 0], // weapon_9mmhandgun
        "monster_human_grunt" => {
            let mut weapons = ent_value(block, "weapons")
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0);
            if weapons == 0 {
                // CHGrunt::Spawn supplies this retail default.
                weapons = HGRUNT_9MMAR | (1 << 1); // MP5 + hand grenades
            }
            [
                if weapons & HGRUNT_SHOTGUN != 0 {
                    30 // weapon_shotgun
                } else {
                    29 // weapon_9mmAR
                },
                if weapons & HGRUNT_GRENADELAUNCHER != 0 {
                    47 // ammo_ARgrenades
                } else {
                    0
                },
            ]
        }
        _ => [0; 2],
    }
}

fn append_monster_loot_slots(
    out: &mut Vec<(u16, [i32; 3], i32, i16, u16, u16)>,
    owner_index: usize,
    origin: [i32; 3],
    yaw: i32,
    leaf: i16,
    loot: [u16; 2],
) {
    for kind in loot {
        if kind == 0 {
            continue;
        }
        // `name` is otherwise unused by an inactive item. Store owner+1 so the
        // runtime can release the exact slots without another RAM array.
        out.push((
            kind | LOOT_SLOT_FLAGS,
            origin,
            yaw,
            leaf,
            (owner_index + 1).min(u16::MAX as usize) as u16,
            0,
        ));
    }
}

/// Point entities that place an actor/item. The final word is a stable carry id
/// (targetname hash, or globalname hash with bit 15 set); it occupies PropRec's
/// existing padding and therefore does not grow map data.
/// type 0 = scientist, 1 = barney, 2 = headcrab, 3 = item_suit, 4 = item_battery.
fn collect_props(
    ents: &[u8],
    nodes: &[u8],
    planes: &[u8],
    models: &[u8],
    scale: f32,
    logic_names: &mut Vec<String>,
) -> Vec<(u16, [i32; 3], i32, i16, u16, u16)> {
    let s = entity_text(ents);
    let mut out = Vec::new();
    let map_uses_scripted_sitter = s.split('{').any(|candidate| {
        ent_value(candidate, "classname") == Some("monster_scientist")
            && ent_value(candidate, "targetname")
                .map(|name| scripted_sitting_scientist_target(&s, name))
                .unwrap_or(false)
    });
    let map_uses_vent_zombies = map_uses_vent_zombie_stream(&s);
    // Monster makers above an ALLOWMONSTERS teleport are a stock GoldSrc
    // set-piece idiom. Resolve the eventual destination at cook time: the PS1
    // runtime can wake the dormant actor directly in the remote room instead
    // of carrying a full second monster physics/trigger path in scarce code
    // RAM. The source portal FX still fires through its authored manager.
    let mut teleport_dests: Vec<(String, [f32; 3], i32)> = Vec::new();
    for block in s.split('{') {
        let cls = ent_value(block, "classname").unwrap_or("");
        if cls != "info_teleport_destination" && cls != "info_target" {
            continue;
        }
        if let (Some(name), Some(origin)) = (
            ent_value(block, "targetname"),
            ent_value(block, "origin").and_then(parse_vec3),
        ) {
            teleport_dests.push((
                name.to_string(),
                origin,
                pack_prop_orientation(ent_angles_degrees(block).unwrap_or([0.0; 3]), 0),
            ));
        }
    }
    let mut monster_portals: Vec<([i32; 3], [i32; 3], [f32; 3], i32)> = Vec::new();
    for block in s.split('{') {
        if ent_value(block, "classname") != Some("trigger_teleport")
            || parse_spawnflags(block) & 1 == 0
        {
            continue;
        }
        let Some(submodel) = block_model(block) else {
            continue;
        };
        let Some((mins, maxs)) = model_bounds_hl(models, submodel) else {
            continue;
        };
        let origin_hl = ent_value(block, "origin")
            .and_then(parse_vec3)
            .unwrap_or([0.0; 3]);
        let (wmins, wmaxs) = transform_bounds_to_world(mins, maxs, origin_hl, scale);
        let target = ent_value(block, "target").unwrap_or("");
        if let Some((_, dest_hl, yaw)) = teleport_dests.iter().find(|(n, _, _)| n == target) {
            monster_portals.push((wmins, wmaxs, *dest_hl, *yaw));
        }
    }
    // Only MoveTo=4 teleports an auto-start scripted actor to the mark.
    // MoveTo=0 waits at its authored origin; 1/2 walk/run there at runtime.
    let mut script_marks: Vec<(String, [f32; 3], Option<[f32; 3]>)> = Vec::new();
    for block in s.split('{') {
        if ent_value(block, "classname") != Some("scripted_sequence") {
            continue;
        }
        if ent_value(block, "targetname").is_some() {
            continue;
        }
        let move_to = ent_value(block, "m_fMoveTo")
            .or_else(|| ent_value(block, "m_flMoveTo"))
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        if move_to != 4 {
            continue;
        }
        let Some(target) = ent_value(block, "m_iszEntity") else {
            continue;
        };
        let Some(origin) = ent_value(block, "origin").and_then(parse_vec3) else {
            continue;
        };
        script_marks.push((target.to_string(), origin, ent_angles_degrees(block)));
    }
    for block in s.split('{') {
        // Model-type id space, shared with the runtime registry (game/src/model_defs).
        // 0-4 = the original NPCs/items; 5-24 = the full enemy roster; 25 = the
        // seated scientist (own baked sit pose, no ground snap). `*_dead`
        // corpses spawn as their live type with the DEAD bit (0x8000): the
        // runtime zeroes health and shows the death clip's final frame.
        const DEAD: u16 = 0x8000;
        const PREDISASTER: u16 = 0x2000;
        const PRISONER: u16 = 0x1000;
        let cls = ent_value(block, "classname").unwrap_or("");
        let ty = match cls {
            "monster_scientist" => {
                if ent_value(block, "targetname")
                    .map(|name| scripted_sitting_scientist_target(&s, name))
                    .unwrap_or(false)
                {
                    SCRIPTED_SITTING_SCIENTIST_TYPE
                } else {
                    0u16
                }
            }
            "monster_sitting_scientist" => 25u16,
            "monster_scientist_dead" => DEAD | 0,
            "monster_hevsuit_dead" if map_uses_scripted_sitter => {
                DEAD | SCRIPTED_SITTING_SCIENTIST_TYPE
            }
            "monster_hevsuit_dead" => DEAD | 0,
            "monster_barney_dead" => DEAD | 1,
            "monster_hgrunt_dead" => DEAD | 8,
            "monster_barney" => 1u16,
            "monster_headcrab" => 2u16,
            "item_suit" => 3u16,
            "item_battery" => 4u16,
            "monster_zombie" if map_uses_vent_zombies => VENT_SCRIPT_ZOMBIE_TYPE,
            "monster_zombie" => 5u16,
            "monster_houndeye" => 6u16,
            "monster_bullchicken" => 7u16,
            "monster_human_grunt" => 8u16,
            "monster_alien_slave" => 9u16,
            "monster_alien_grunt" => 10u16,
            "monster_alien_controller" => 11u16,
            "monster_barnacle" => 12u16,
            "monster_leech" => 13u16,
            "monster_cockroach" => 14u16,
            "monster_gman" => 15u16,
            "monster_gargantua" => 16u16,
            "monster_nihilanth" => 17u16,
            "monster_bigmomma" => 18u16,
            "monster_ichthyosaur" => 19u16,
            "monster_sentry" => 20u16,
            "monster_turret" => 21u16,
            "monster_miniturret" => 22u16,
            "monster_apache" => 23u16,
            "monster_flyer_flock" => 24u16,
            // Weapon / ammo / medkit pickups (w_* world models).
            "weapon_crowbar" => 26u16,
            "weapon_9mmhandgun" | "weapon_glock" => 27u16,
            "weapon_357" | "weapon_python" => 28u16,
            "weapon_9mmAR" | "weapon_mp5" => 29u16,
            "weapon_shotgun" => 30u16,
            "weapon_crossbow" => 31u16,
            "weapon_rpg" => 32u16,
            "weapon_gauss" => 33u16,
            "weapon_egon" => 34u16,
            "weapon_hornetgun" => 35u16,
            "weapon_handgrenade" => 36u16,
            "weapon_snark" => 37u16,
            "weapon_tripmine" => 38u16,
            "weapon_satchel" => 39u16,
            "ammo_9mmclip" | "ammo_glockclip" => 40u16,
            "ammo_9mmAR" | "ammo_mp5clip" => 41u16,
            "ammo_buckshot" => 42u16,
            "ammo_357" => 43u16,
            "ammo_crossbow" => 44u16,
            "ammo_rpgclip" => 45u16,
            "ammo_gaussclip" => 46u16,
            "ammo_ARgrenades" | "ammo_mp5grenades" => 47u16,
            "item_healthkit" => 48u16,
            // One dormant can per dispenser, reused only after collection.
            // GoldSrc blocks Use while that owner's can is still waiting.
            "env_beverage" => 75u16 | 0x4000,
            "item_longjump" => 49u16,
            "monster_tentacle" => 50u16,
            "monster_human_assassin" => 51u16,
            "monster_tripmine" => 57u16,
            "monster_snark" => 58u16,
            "monster_babycrab" => 59u16,
            "monster_rat" => 60u16,
            "monster_osprey" => 61u16,
            "xen_plantlight" => 73u16,
            "xen_tree" => 74u16,
            "xen_hair" => 68u16,
            "xen_spore_small" => 70u16,
            "xen_spore_medium" => 69u16,
            "xen_spore_large" => 71u16,
            "monster_generic" | "monster_furniture" | "cycler" => {
                match monster_generic_type(block) {
                    Some(ty) => ty,
                    None => continue,
                }
            }
            "world_items" => match ent_value(block, "type").and_then(|v| v.parse::<u16>().ok()) {
                Some(45) => 3u16, // ITEM_SUIT
                Some(44) => 4u16, // ITEM_BATTERY
                _ => continue,
            },
            "monstermaker" => {
                // Spawner: cook up to 4 DORMANT copies (bit 0x4000) of the
                // monster it makes; the runtime activates them one per fire.
                let mt = ent_value(block, "monstertype").unwrap_or("");
                let Some(base) = monster_type_id(mt) else {
                    continue;
                };
                let count = parse_f32_key(block, "monstercount", 1.0)
                    .round()
                    .clamp(1.0, 4.0) as usize;
                let origin_hl = ent_value(block, "origin")
                    .and_then(parse_vec3)
                    .unwrap_or([0.0; 3]);
                let origin = to_world(origin_hl, scale);
                let angles = ent_angles_degrees(block).unwrap_or([0.0; 3]);
                let mut spawn_hl = origin_hl;
                let mut spawn = origin;
                let mut orientation = pack_prop_orientation(angles, 0);
                if let Some((_, _, dest_hl, dest_orientation)) = monster_portals
                    .iter()
                    .filter(|(mins, maxs, _, _)| {
                        origin[0] + 16 >= mins[0]
                            && origin[0] - 16 <= maxs[0]
                            && origin[2] + 16 >= mins[2]
                            && origin[2] - 16 <= maxs[2]
                            && maxs[1] < origin[1]
                            && origin[1] - maxs[1] <= 4096
                    })
                    .min_by_key(|(_, maxs, _, _)| origin[1] - maxs[1])
                {
                    spawn_hl = *dest_hl;
                    spawn = to_world(*dest_hl, scale);
                    orientation = *dest_orientation;
                }
                let maker_name = ent_value(block, "targetname")
                    .and_then(|tn| logic_names.iter().position(|n| n == tn))
                    .map(|p| (p + 1).min(u16::MAX as usize) as u16)
                    .unwrap_or(0);
                for _ in 0..count {
                    let owner_index = out.len();
                    out.push((
                        base | 0x4000,
                        spawn,
                        orientation,
                        point_leaf(spawn_hl, nodes, planes),
                        maker_name,
                        0,
                    ));
                    append_monster_loot_slots(
                        &mut out,
                        owner_index,
                        spawn,
                        orientation,
                        point_leaf(spawn_hl, nodes, planes),
                        monster_loot_types(mt, block),
                    );
                }
                continue;
            }
            _ => continue,
        };
        // CGenericMonster is a passive/script puppet even when it reuses a
        // normal scientist/Barney studio model. Reuse the existing PRISONER
        // cook bit: scripted work runs before that runtime guard, while the
        // actor cannot acquire ordinary follow/combat schedules between
        // sequences. This costs no PropRec or resident-state bytes.
        let ty = if matches!(cls, "monster_generic" | "monster_furniture" | "cycler") {
            ty | PRISONER
        } else {
            ty
        };
        let spawnflags = parse_spawnflags(block);
        // CTalkMonster::FollowerUse rejects +use for every predisaster talk
        // monster, including Barney.  Preserve the shared GoldSrc spawnflag
        // for both concrete follower classes; limiting this to type 0 made
        // c1a0's door guard incorrectly follow Gordon when used.
        let ty = if cls == "monster_sitting_scientist"
            || (matches!(cls, "monster_scientist" | "monster_barney") && spawnflags & 256 != 0)
            || (cls == "monster_generic"
                && ty & 0x0fff == 56
                && parse_f32_key(block, "renderamt", 255.0) <= 0.0)
            || (matches!(ty & 0x0fff, 20 | 21 | 22) && spawnflags & 64 != 0)
        {
            ty | PREDISASTER
        } else {
            ty
        };
        // SF_MONSTER_PRISONER suppresses combat schedules while retaining the
        // actor for scripts and set pieces. c1a0e relies on this for the Xen
        // vision-room bullsquids/vorts; dropping it lets them kill the player
        // during an otherwise non-interactive teleport sequence.
        let hostile_type = ty & 0x0fff;
        let ty = if (5..=24).contains(&hostile_type) && spawnflags & 16 != 0 {
            ty | PRISONER
        } else {
            ty
        };
        let mut origin_hl = ent_value(block, "origin")
            .and_then(parse_vec3)
            .unwrap_or([0.0; 3]);
        let mut angles = ent_angles_degrees(block).unwrap_or([0.0; 3]);
        // Auto-start scripted_sequence targeting this monster's name: spawn
        // at the script mark (matches where real HL poses it at map start).
        if ty & DEAD == 0 {
            if let Some(tn) = ent_value(block, "targetname") {
                if let Some((_, mo, mark_angles)) = script_marks.iter().find(|(t, _, _)| t == tn) {
                    origin_hl = *mo;
                    if let Some(mark_angles) = mark_angles {
                        angles = *mark_angles;
                    }
                }
            }
        }
        let origin = to_world(origin_hl, scale);
        let model_type = ty & 0x0fff;
        let body = if matches!(model_type, 0 | 25 | SCRIPTED_SITTING_SCIENTIST_TYPE) {
            let authored = ent_value(block, "body")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            if authored == -1 {
                // GoldSrc picks 0..3 from its global RNG. Keep the same four
                // heads but derive the choice from authored placement so full
                // Rust rebuilds and deterministic traces remain stable.
                let seed = origin_hl[0].to_bits()
                    ^ origin_hl[1].to_bits().rotate_left(11)
                    ^ origin_hl[2].to_bits().rotate_left(22);
                (seed & 3) as u8
            } else {
                authored.clamp(0, 7) as u8
            }
        } else if model_type == 1 && ty & DEAD != 0 {
            2 // CBarney::Killed drops the gun and selects GUNGONE.
        } else {
            0
        };
        let orientation = pack_prop_orientation(angles, body);
        // Actor targetnames are semantic identities in GoldSrc even when no
        // other cooked logic record happens to refer to them. Intern them here
        // after the logic pass: appending preserves every already-issued id and
        // keeps FireTargets, transition overlays, and reference traces exact.
        let name_id = ent_value(block, "targetname")
            .map(|tn| intern_logic_name(logic_names, tn).unwrap_or(0))
            .unwrap_or(0);
        let carry_id = if cls == "monster_scientist_dead" {
            // Temporary source-pose index, resolved to the map-local clip at cook.
            cooked::PROP_CORPSE_CLIP
                | ent_value(block, "pose")
                    .and_then(|v| v.parse::<u16>().ok())
                    .unwrap_or(0)
                    .min(6)
        } else if prop_type_crosses_transition(ty) {
            entity_carry_id(block)
        } else {
            0
        };
        let owner_index = out.len();
        let leaf = point_leaf(origin_hl, nodes, planes);
        out.push((ty, origin, orientation, leaf, name_id, carry_id));
        if ty & DEAD == 0 {
            append_monster_loot_slots(
                &mut out,
                owner_index,
                origin,
                orientation,
                leaf,
                monster_loot_types(cls, block),
            );
        }
    }
    out
}

/// Monster classname -> model type id (the monstermaker's monstertype key).
fn monster_type_id(cls: &str) -> Option<u16> {
    Some(match cls {
        "monster_scientist" => 0,
        "monster_barney" => 1,
        "monster_headcrab" => 2,
        "monster_zombie" => 5,
        "monster_houndeye" => 6,
        "monster_bullchicken" => 7,
        "monster_human_grunt" => 8,
        "monster_alien_slave" => 9,
        "monster_alien_grunt" => 10,
        "monster_alien_controller" => 11,
        "monster_gman" => 15,
        "monster_cockroach" => 14,
        "monster_ichthyosaur" => 19,
        "monster_sentry" => 20,
        _ => return None,
    })
}

/// `monster_generic` selects its studio model through the entity's `model`
/// key, so classname alone cannot identify the streamed actor type. Keep this
/// deliberately allowlisted: unsupported generics remain absent instead of
/// silently rendering as the wrong set-piece model.
fn monster_generic_type(block: &str) -> Option<u16> {
    let model = ent_value(block, "model")?
        .replace('\\', "/")
        .to_ascii_lowercase();
    match model.rsplit('/').next().unwrap_or(model.as_str()) {
        "scientist.mdl" => Some(0),
        "barney.mdl" => Some(1),
        "loader.mdl" => Some(52),
        "forklift.mdl" => Some(53),
        "holo.mdl" => Some(56),
        // Authored scenery: gibs strewn through the Black Mesa maps, office
        // furniture, and the Xen flora/bubble cyclers. All single-pose props.
        "gib_legbone.mdl" => Some(62),
        "pelvis.mdl" => Some(63),
        "ribcage.mdl" => Some(64),
        "riblet1.mdl" => Some(65),
        "zombiegibs1.mdl" => Some(66),
        "filecabinet.mdl" => Some(67),
        "hair.mdl" => Some(68),
        "fungus.mdl" => Some(69),
        "fungus(small).mdl" => Some(70),
        "fungus(large).mdl" => Some(71),
        "pipe_bubbles.mdl" => Some(72),
        _ => None,
    }
}

fn miptex_name(l: &[u8], mo: usize) -> String {
    let name = match l.get(mo..mo + 16) {
        Some(n) => n,
        None => return String::new(),
    };
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    String::from_utf8_lossy(&name[..end]).to_string()
}

fn is_tool_texture(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    n == "origin"
        || n == "sky"
        || n == "clip"
        || n == "skip"
        || n == "hint"
        || n == "null"
        || n.starts_with("aaatrigger")
        || n.starts_with("trigger")
}

/// GoldSrc liquid surfaces (`!water`, `!lava`, `!slime`, plus the legacy Quake
/// `*` prefix) render semi-transparent -- you see the geometry below the
/// surface. These become the map's translucent faces (drawn in a second,
/// blended pass over the opaque world).
fn is_translucent_texture(name: &str) -> bool {
    let n = name.trim();
    n.starts_with('!') || n.starts_with('*')
}

// GoldSrc's sound/materials.txt tags only the first twelve characters after
// stripping animation/masked/liquid prefixes. Keep the compact runtime ids
// aligned with game::map::MATERIAL_*; concrete is deliberately zero so an old
// or untagged face retains the original fallback.
fn texture_sound_material(name: &str, materials: &HashMap<String, u8>) -> u8 {
    let mut clean = name;
    if clean.starts_with('-') || clean.starts_with('+') {
        clean = clean.get(2..).unwrap_or("");
    }
    if clean
        .as_bytes()
        .first()
        .is_some_and(|b| matches!(*b, b'{' | b'!' | b'~' | b' '))
    {
        clean = clean.get(1..).unwrap_or("");
    }
    let key: String = clean
        .chars()
        .take(12)
        .flat_map(char::to_uppercase)
        .collect();
    materials.get(&key).copied().unwrap_or(0)
}

fn load_texture_sound_materials(map_path: &str) -> HashMap<String, u8> {
    let Some(valve) = Path::new(map_path).parent().and_then(Path::parent) else {
        return HashMap::new();
    };
    let Ok(text) = std::fs::read_to_string(valve.join("sound/materials.txt")) else {
        return HashMap::new();
    };
    let mut result = HashMap::new();
    for raw in text.lines() {
        let line = raw.split("//").next().unwrap_or("").trim();
        let mut fields = line.split_whitespace();
        let Some(kind) = fields.next().and_then(|s| s.as_bytes().first()).copied() else {
            continue;
        };
        let Some(name) = fields.next() else { continue };
        let id = match kind.to_ascii_uppercase() {
            b'M' => 1,
            b'D' => 2,
            b'V' => 3,
            b'G' => 4,
            b'T' => 5,
            b'S' => 6,
            b'W' => 7,
            b'P' => 8,
            b'Y' => 9,
            _ => 0,
        };
        let key: String = name.chars().take(12).flat_map(char::to_uppercase).collect();
        result.insert(key, id);
    }
    result
}

fn tex_id_for_face(faces: &[u8], texinfo: &[u8], face: usize, n_texs: usize) -> usize {
    let ti = u16le(faces, face * SZ_FACE + 10).unwrap_or(0) as usize;
    let tex = i32le(texinfo, ti * SZ_TEXINFO + 32).unwrap_or(-1);
    if tex >= 0 && (tex as usize) < n_texs {
        tex as usize
    } else {
        0
    }
}

/// Watertight pass: split any triangle edge that another vertex lands on (a
/// T-junction) so neighbouring faces meet exactly instead of cracking open into
/// the background at grazing angles. Source BSP faces and UV seam clipping can
/// produce coincident edge vertices through different paths; this canonicalizes
/// them before the conforming affine-refinement pass runs.
/// Rebuilds the per-face triangle ranges since splitting changes tri counts.
/// Returns the number of triangles added.
fn weld_tjunctions(
    verts: &[[i16; 3]],
    tri_idx: &mut Vec<u16>,
    tri_tex: &mut Vec<u16>,
    tri_uv: &mut Vec<u8>,
    tri_rgb: &mut Vec<u8>,
    face_first: &mut [u32],
    face_ntri: &mut [u16],
    face_refined: &[bool],
    cell_pair_first: &mut std::collections::HashSet<usize>,
    max_added: usize,
    required_vertices_from: usize,
) -> usize {
    use std::collections::HashMap;
    if max_added == 0 {
        return 0; // no resident-RAM headroom on this map; leave it untouched
    }
    // One canonical index per distinct position so the coincident duplicate
    // verts the subdivision emits collapse to a single split point.
    let mut pos_idx: HashMap<[i16; 3], u16> = HashMap::new();
    for (i, &p) in verts.iter().enumerate() {
        pos_idx.entry(p).or_insert(i as u16);
    }
    const CELL: i32 = 128;
    let cell = |p: [i16; 3]| (p[0] as i32 / CELL, p[1] as i32 / CELL, p[2] as i32 / CELL);
    let mut grid: HashMap<(i32, i32, i32), Vec<u16>> = HashMap::new();
    for (&p, &i) in pos_idx.iter() {
        grid.entry(cell(p)).or_default().push(i);
    }

    // Verts strictly inside segment (a,b), as (parameter, index), sorted.
    let on_edge = |a: u16, b: u16| -> Vec<(f32, u16)> {
        let pa = verts[a as usize];
        let pb = verts[b as usize];
        let d = [
            pb[0] as i64 - pa[0] as i64,
            pb[1] as i64 - pa[1] as i64,
            pb[2] as i64 - pa[2] as i64,
        ];
        let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        // Only stitch long edges. A T-junction crack is only visible when the
        // edge spans enough screen at a grazing angle; short subdivision edges
        // crack sub-pixel and welding them all would explode the triangle count
        // on a fill-bound console for no visible gain.
        const MIN_EDGE2: i64 = 80 * 80;
        if len2 == 0 || (len2 < MIN_EDGE2 && required_vertices_from >= verts.len()) {
            return Vec::new();
        }
        let (ca, cb) = (cell(pa), cell(pb));
        let rng = |x: i32, y: i32| (x.min(y) - 1, x.max(y) + 1);
        let (x0, x1) = rng(ca.0, cb.0);
        let (y0, y1) = rng(ca.1, cb.1);
        let (z0, z1) = rng(ca.2, cb.2);
        let mut out: Vec<(i64, u16)> = Vec::new();
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                for cz in z0..=z1 {
                    let Some(bucket) = grid.get(&(cx, cy, cz)) else {
                        continue;
                    };
                    for &ci in bucket {
                        if len2 < MIN_EDGE2 && (ci as usize) < required_vertices_from {
                            continue;
                        }
                        let pc = verts[ci as usize];
                        if pc == pa || pc == pb {
                            continue;
                        }
                        let ac = [
                            pc[0] as i64 - pa[0] as i64,
                            pc[1] as i64 - pa[1] as i64,
                            pc[2] as i64 - pa[2] as i64,
                        ];
                        let dot = ac[0] * d[0] + ac[1] * d[1] + ac[2] * d[2];
                        if dot <= 0 || dot >= len2 {
                            continue; // not strictly between the endpoints
                        }
                        let ac2 = ac[0] * ac[0] + ac[1] * ac[1] + ac[2] * ac[2];
                        // perpendicular dist^2 = ac2 - dot^2/len2; collinear when
                        // < EPS^2 (EPS = 2 units). Cross-multiply to stay integer.
                        let off_line = ac2 * len2 - dot * dot;
                        if if ci as usize >= required_vertices_from {
                            off_line == 0
                        } else {
                            off_line < 4 * len2
                        } {
                            out.push((dot, ci));
                        }
                    }
                }
            }
        }
        // Exact new grid junctions must not be mixed with epsilon candidates:
        // two near-coincident vertices can have the same projected position,
        // making the boundary non-simple and preventing constrained ears.
        if out
            .iter()
            .any(|&(_, i)| i as usize >= required_vertices_from)
        {
            out.retain(|&(dot, i)| {
                let p = verts[i as usize];
                let v = [
                    p[0] as i64 - pa[0] as i64,
                    p[1] as i64 - pa[1] as i64,
                    p[2] as i64 - pa[2] as i64,
                ];
                (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) * len2 == dot * dot
            });
        }
        // `grid` buckets inherit randomized HashMap iteration order. Sort by
        // the exact projection, then by canonical vertex index, so distinct
        // near-collinear points at the same parameter cook identically.
        out.sort_unstable();
        out.dedup_by(|p, q| p.1 == q.1);
        out.into_iter()
            .map(|(dot, ci)| (dot as f32 / len2 as f32, ci))
            .collect()
    };

    let lerp = |a: u8, b: u8, t: f32| -> u8 {
        (a as f32 + (b as f32 - a as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };

    let mut new_idx: Vec<u16> = Vec::with_capacity(tri_idx.len());
    let mut new_tex: Vec<u16> = Vec::with_capacity(tri_tex.len());
    let mut new_uv: Vec<u8> = Vec::with_capacity(tri_uv.len());
    let mut new_rgb: Vec<u8> = Vec::with_capacity(tri_rgb.len());
    let mut new_cell_pair_first = std::collections::HashSet::new();
    let mut added = 0usize;

    for f in 0..face_first.len() {
        let first = face_first[f] as usize;
        let cnt = face_ntri[f] as usize;
        let new_first = new_idx.len() / 3;
        // Only source-face boundary edges can own a T-junction. An edge shared
        // by two triangles in this same face is merely the fan/grid diagonal;
        // welding arbitrary coplanar vertices onto it needlessly destroys the
        // original polygon identity. In particular, a GoldSrc four-sided face
        // stopped being one native quad whenever any unrelated vertex happened
        // to lie on its two-triangle diagonal. Count the pre-weld face edges so
        // those internal diagonals pass through unchanged.
        let mut face_edge_uses = HashMap::<(u16, u16), u8>::new();
        if !face_refined.get(f).copied().unwrap_or(false) {
            for t in first..first + cnt {
                for edge in 0..3 {
                    let a = tri_idx[t * 3 + edge];
                    let b = tri_idx[t * 3 + (edge + 1) % 3];
                    let key = if a < b { (a, b) } else { (b, a) };
                    let uses = face_edge_uses.entry(key).or_insert(0);
                    *uses = uses.saturating_add(1);
                }
            }
        }
        for t in first..first + cnt {
            let corner = |k: usize| RefineCorner {
                idx: tri_idx[t * 3 + k],
                uv: [tri_uv[t * 6 + k * 2], tri_uv[t * 6 + k * 2 + 1]],
                rgb: [
                    tri_rgb[t * 9 + k * 3],
                    tri_rgb[t * 9 + k * 3 + 1],
                    tri_rgb[t * 9 + k * 3 + 2],
                ],
            };
            let c = [corner(0), corner(1), corner(2)];
            // UV-grid faces are already conforming inside each cell. Their
            // newly-created boundary vertices remain in the global position
            // grid above, so neighbouring unrefined faces can weld to them;
            // never retriangulate the grid itself or its explicit two-triangle
            // cell ownership would be lost.
            if face_refined.get(f).copied().unwrap_or(false) {
                if cell_pair_first.contains(&t) {
                    new_cell_pair_first.insert(new_tex.len());
                }
                push_refined_tri(
                    c,
                    tri_tex[t],
                    &mut new_idx,
                    &mut new_tex,
                    &mut new_uv,
                    &mut new_rgb,
                );
                continue;
            }
            // Once the RAM-headroom budget for added tris is spent, copy the rest
            // of the map's triangles through unsplit (some far-map cracks remain,
            // but the map still fits its streaming buffer).
            let weld_this = added < max_added;
            // Boundary loop = corners with any on-edge verts inserted per edge.
            let mut loopv: Vec<RefineCorner> = Vec::with_capacity(4);
            for e in 0..3 {
                let a = c[e];
                let b = c[(e + 1) % 3];
                loopv.push(a);
                let key = if a.idx < b.idx {
                    (a.idx, b.idx)
                } else {
                    (b.idx, a.idx)
                };
                let source_boundary = face_edge_uses.get(&key).copied().unwrap_or(1) == 1;
                for (param, ci) in if weld_this && source_boundary {
                    on_edge(a.idx, b.idx)
                } else {
                    Vec::new()
                } {
                    loopv.push(RefineCorner {
                        idx: ci,
                        uv: [lerp(a.uv[0], b.uv[0], param), lerp(a.uv[1], b.uv[1], param)],
                        rgb: [
                            lerp(a.rgb[0], b.rgb[0], param),
                            lerp(a.rgb[1], b.rgb[1], param),
                            lerp(a.rgb[2], b.rgb[2], param),
                        ],
                    });
                }
            }
            let tex = tri_tex[t];
            let extra = loopv.len().saturating_sub(3);
            if added + extra > max_added {
                push_refined_tri(
                    c,
                    tex,
                    &mut new_idx,
                    &mut new_tex,
                    &mut new_uv,
                    &mut new_rgb,
                );
                continue;
            }
            let emitted = triangulate_refined_boundary(
                &loopv,
                verts,
                tex,
                &mut new_idx,
                &mut new_tex,
                &mut new_uv,
                &mut new_rgb,
                loopv
                    .iter()
                    .any(|c| c.idx as usize >= required_vertices_from),
            );
            if emitted == 0 {
                push_refined_tri(
                    c,
                    tex,
                    &mut new_idx,
                    &mut new_tex,
                    &mut new_uv,
                    &mut new_rgb,
                );
            } else {
                added += emitted - 1;
            }
        }
        face_first[f] = new_first as u32;
        face_ntri[f] = ((new_idx.len() / 3) - new_first).min(u16::MAX as usize) as u16;
    }
    *tri_idx = new_idx;
    *tri_tex = new_tex;
    *tri_uv = new_uv;
    *tri_rgb = new_rgb;
    *cell_pair_first = new_cell_pair_first;
    added
}

fn cook(path: &str, out: &str, tex_out: Option<&str>) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {}", path, e))?;
    let bsp = Bsp::parse(&bytes)?;
    let coverage_report = std::env::var("ENTITY_COVERAGE_REPORT").ok();
    let strict_coverage = std::env::var("ENTITY_COVERAGE_STRICT").as_deref() == Ok("1");
    entity_support::audit(
        &entity_text(bsp.lump(LUMP_ENTITIES)),
        path,
        coverage_report.as_deref(),
        strict_coverage,
    )?;
    let wads = WadIndex::load_for_bsp(path);

    // Raw vertices (f32, HL Z-up). Power-of-two shift so coords fit i16.
    let vl = bsp.lump(LUMP_VERTEXES);
    let orig_n_verts = vl.len() / SZ_VERTEX;
    let raw: Vec<[f32; 3]> = (0..orig_n_verts)
        .map(|i| {
            let o = i * SZ_VERTEX;
            [
                f32le(vl, o).unwrap(),
                f32le(vl, o + 4).unwrap(),
                f32le(vl, o + 8).unwrap(),
            ]
        })
        .collect();
    let maxabs = raw.iter().flatten().fold(0.0f32, |m, &c| m.max(c.abs()));
    let mut shift = 0u32;
    while (maxabs / (1 << shift) as f32) > 32767.0 {
        shift += 1;
    }
    if shift > 0 {
        eprintln!(
            "note: max |coord| {:.0} exceeds i16; scaling down by {}x",
            maxabs,
            1 << shift
        );
    }
    let scale = (1 << shift) as f32;
    // Starts and transition landmarks are cheap, authored samples of the space
    // the player can actually traverse. They keep the fixed affine-refinement
    // budget focused on useful nearby surfaces instead of distant map-scale
    // polygons that happen to have the largest raw dimensions.
    let affine_traversal_samples: Vec<[i32; 3]> =
        standalone_spawn_candidates(bsp.lump(LUMP_ENTITIES))
            .into_iter()
            .map(|candidate| {
                let p = candidate.origin_hl;
                [
                    (p[0] / scale).round() as i32,
                    (p[2] / scale).round() as i32,
                    (p[1] / scale).round() as i32,
                ]
            })
            .collect();
    // HL right-handed Z-up -> world Y-up (world = [x, z, y]); winding reversed.
    let mut verts: Vec<[i16; 3]> = raw
        .iter()
        .map(|v| {
            [
                (v[0] / scale).round() as i16,
                (v[2] / scale).round() as i16,
                (v[1] / scale).round() as i16,
            ]
        })
        .collect();

    // Cook every miptex up front because face UVs and texinfo use original BSP
    // texture indices. After geometry emission, compact to only referenced
    // render textures so triggers/tools/unused miptexes do not occupy RAM/VRAM.
    let tl = bsp.lump(LUMP_TEXTURES);
    let n_texs = i32le(tl, 0).filter(|&n| n >= 0).unwrap_or(0) as usize;
    let mut texs: Vec<CookedTex> = Vec::with_capacity(n_texs);
    let mut orig: Vec<(u32, u32)> = Vec::with_capacity(n_texs);
    let mut tex_names: Vec<String> = Vec::with_capacity(n_texs);
    for i in 0..n_texs {
        match i32le(tl, 4 + i * 4) {
            Some(d) if d >= 0 => {
                tex_names.push(miptex_name(tl, d as usize));
                let (t, o) = cook_miptex(tl, d as usize, &wads);
                texs.push(t);
                orig.push(o);
            }
            _ => {
                tex_names.push(String::new());
                texs.push(placeholder_tex());
                orig.push((64, 64));
            }
        }
    }

    let edges = bsp.lump(LUMP_EDGES);
    let surf = bsp.lump(LUMP_SURFEDGES);
    let texinfo = bsp.lump(LUMP_TEXINFO);
    let lighting = bsp.lump(LUMP_LIGHTING);
    let faces = bsp.lump(LUMP_FACES);
    let planes = bsp.lump(LUMP_PLANES);
    let nodes = bsp.lump(LUMP_NODES);
    let leaves = bsp.lump(LUMP_LEAVES);
    let models = bsp.lump(LUMP_MODELS);
    let n_faces = faces.len() / SZ_FACE;
    let dynamic_lightstyles =
        collect_dynamic_lightstyles(bsp.lump(LUMP_ENTITIES), faces, texinfo, &tex_names)?;
    let n_edges = edges.len() / SZ_EDGE;
    let world_first_face = u32le(models, 56).unwrap_or(0) as usize;
    let world_end_face = world_first_face
        .saturating_add(u32le(models, 60).unwrap_or(0) as usize)
        .min(n_faces);

    // Phase B (gated until the runtime consumes it): static brush entities
    // whose faces the world pipeline may own. The renderer-bound slow windows
    // are grate scenes, and every masked face on t0a0 is a submodel quad.
    let phase_b_static_ents = std::env::var_os("WORLD_PIPELINE_STATIC_ENTS").is_some();
    let n_models_total = models.len() / SZ_MODEL;
    let submodel_static_eligible = if phase_b_static_ents {
        pipeline_static_submodels(bsp.lump(LUMP_ENTITIES), n_models_total)
    } else {
        vec![false; n_models_total]
    };
    // Source-face -> submodel index (u16::MAX = world model 0 / none).
    let mut face_submodel = vec![u16::MAX; n_faces];
    for mi in 1..n_models_total {
        let ff = u32le(models, mi * SZ_MODEL + 56).unwrap_or(0) as usize;
        let nf = u32le(models, mi * SZ_MODEL + 60).unwrap_or(0) as usize;
        for f in ff..ff.saturating_add(nf).min(n_faces) {
            face_submodel[f] = mi as u16;
        }
    }

    let mut tri_idx: Vec<u16> = Vec::new();
    let mut tri_tex: Vec<u16> = Vec::new();
    let mut tri_uv: Vec<u8> = Vec::new();
    let mut tri_rgb: Vec<u8> = Vec::new();
    // Per-face triangle range (for PVS: leaf -> face -> tris). Skipped faces
    // keep count 0.
    let mut face_first = vec![0u32; n_faces];
    let mut face_ntri = vec![0u16; n_faces];
    // Clean-fan tri count (poly.len()-2) recorded pre-split/weld. A face whose
    // final face_ntri still equals this was neither UV-split nor welded, so its
    // tris are the clean fan and it can be stored as a vertex loop.
    let mut face_fan_ntri = vec![0u16; n_faces];
    let mut face_translucent = vec![false; n_faces];
    let mut face_center = vec![[0i16; 3]; n_faces];
    let mut face_dynamic_lightstyles = vec![[0u8; 3]; n_faces];
    let mut face_dynamic_layers = vec![[0u8; 3]; n_faces];
    let mut face_lm_dims = vec![[0u8; 2]; n_faces];
    let mut face_lm_mins = vec![[0i16; 2]; n_faces];
    let mut face_extent = vec![[0u16; 3]; n_faces];
    let mut face_affine_candidate = vec![false; n_faces];
    let mut face_affine_refined = vec![false; n_faces];
    // Native runtime patches are restricted to opaque world faces. Moving
    // brush models retain the established loop/raw paths and entity projection
    // cache; liquids retain their coplanar ordering fallback.
    let mut face_native_patch = vec![false; n_faces];
    // Masked-cutout faces (grate/fence "{" textures) with solid brushwork close
    // behind them. Only these take the runtime cutout OT pull-forward: a grate
    // must win the painter tie against the slabs it overlays, while a
    // freestanding gate pulled forward paints over nearer geometry instead
    // (the t0a0 door-frame wedges).
    let mut face_cutout_backed = vec![false; n_faces];
    // Which brush model owns each face (0 = world). Backing probes must skip
    // the face's own model so a thin grate brush cannot back itself, and must
    // test entity models too: the slabs seen through a walkway grate are often
    // func brushes that the world tree reports as empty space.
    let n_bsp_models = models.len() / SZ_MODEL;
    let mut face_model = vec![0u16; n_faces];
    for mi in 0..n_bsp_models {
        let mo = mi * SZ_MODEL;
        let firstface = i32le(models, mo + 56).unwrap_or(0).max(0) as usize;
        let numfaces = i32le(models, mo + 60).unwrap_or(0).max(0) as usize;
        for f in firstface..(firstface + numfaces).min(n_faces) {
            face_model[f] = mi as u16;
        }
    }
    let point_solid_any = |p: [f32; 3], exclude_model: usize| -> bool {
        if spawn_leaf_contents(p, nodes, planes, leaves) == CONTENTS_SOLID as i32 {
            return true;
        }
        for mi in 1..n_bsp_models {
            if mi == exclude_model {
                continue;
            }
            let mo = mi * SZ_MODEL;
            let origin = [
                f32le(models, mo + 24).unwrap_or(0.0),
                f32le(models, mo + 28).unwrap_or(0.0),
                f32le(models, mo + 32).unwrap_or(0.0),
            ];
            let Some(head) = model_headnode(models, mi, 0) else {
                continue;
            };
            let local = [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]];
            let leaf = point_leaf_from(head, local, nodes, planes);
            // Leaf 0 is the shared CONTENTS_SOLID leaf; other leaves carry
            // their contents word (mirrors spawn_leaf_contents).
            let lo = leaf as usize * SZ_LEAF;
            let contents = if leaf <= 0 || lo + 4 > leaves.len() {
                CONTENTS_SOLID as i32
            } else {
                i32le(leaves, lo).unwrap_or(0)
            };
            if contents == CONTENTS_SOLID as i32 {
                return true;
            }
        }
        false
    };
    let mut grid_cell_pair_first = std::collections::HashSet::new();
    let mut face_bright = vec![128u8; n_faces]; // mean lightmap level; drives spawn choice
    let mut raw_verts = raw.clone();

    // The triangle UV splitter can otherwise create T-junctions: one face gets
    // a midpoint on a shared BSP edge while its neighbour keeps the original
    // unsplit edge. Pre-split BSP edges once, using the strongest subdivision
    // requested by any face that references the edge, then all faces consume
    // the same boundary vertices.
    let mut edge_segments = vec![1u8; n_edges];
    for f in 0..n_faces {
        let fo = f * SZ_FACE;
        let firstedge = i32le(faces, fo + 4).unwrap_or(0) as usize;
        let numedges = u16le(faces, fo + 8).unwrap_or(0) as usize;
        let ti = u16le(faces, fo + 10).unwrap_or(0) as usize;
        if numedges < 3 {
            continue;
        }
        let mtx = i32le(texinfo, ti * SZ_TEXINFO + 32).unwrap_or(-1);
        let tex_id = if mtx >= 0 && (mtx as usize) < n_texs {
            mtx as usize
        } else {
            0
        };
        if is_tool_texture(&tex_names[tex_id]) {
            continue;
        }
        face_translucent[f] = is_translucent_texture(&tex_names[tex_id]);
        let (fw, fh) = (texs[tex_id].w as f32, texs[tex_id].h as f32);
        let (ow, oh) = orig[tex_id];
        let to = ti * SZ_TEXINFO;
        let s = [
            f32le(texinfo, to).unwrap_or(0.0),
            f32le(texinfo, to + 4).unwrap_or(0.0),
            f32le(texinfo, to + 8).unwrap_or(0.0),
        ];
        let s_off = f32le(texinfo, to + 12).unwrap_or(0.0);
        let t = [
            f32le(texinfo, to + 16).unwrap_or(0.0),
            f32le(texinfo, to + 20).unwrap_or(0.0),
            f32le(texinfo, to + 24).unwrap_or(0.0),
        ];
        let t_off = f32le(texinfo, to + 28).unwrap_or(0.0);
        for j in 0..numedges {
            let se = match i32le(surf, (firstedge + j) * SZ_SURFEDGE) {
                Some(v) => v,
                None => continue,
            };
            let edge_idx = se.unsigned_abs() as usize;
            if edge_idx >= n_edges {
                continue;
            }
            let eo = edge_idx * SZ_EDGE;
            let (Some(v0), Some(v1)) = (u16le(edges, eo), u16le(edges, eo + 2)) else {
                continue;
            };
            let (a, b) = if se >= 0 { (v0, v1) } else { (v1, v0) };
            if (a as usize) >= orig_n_verts || (b as usize) >= orig_n_verts {
                continue;
            }
            let uv_at = |p: [f32; 3]| {
                let ou = p[0] * s[0] + p[1] * s[1] + p[2] * s[2] + s_off;
                let ov = p[0] * t[0] + p[1] * t[1] + p[2] * t[2] + t_off;
                (ou * fw / ow as f32, ov * fh / oh as f32)
            };
            let ua = uv_at(raw[a as usize]);
            let ub = uv_at(raw[b as usize]);
            let span = (ua.0 - ub.0).abs().max((ua.1 - ub.1).abs());
            let split_span = if face_model[f] != 0 && tex_names[tex_id].starts_with('{') {
                UV_SPLIT_SPAN_CUTOUT_ENT
            } else {
                UV_SPLIT_SPAN
            };
            let segs = ((span / split_span).ceil() as u8).clamp(1, 1 << UV_SPLIT_DEPTH);
            edge_segments[edge_idx] = edge_segments[edge_idx].max(segs);
        }
    }
    let face_liquid = face_translucent.clone();
    let mut liquid_planes = vec![false; planes.len() / SZ_PLANE];
    for (f, &liquid) in face_liquid.iter().enumerate() {
        if liquid {
            let planenum = u16le(faces, f * SZ_FACE).unwrap_or(u16::MAX) as usize;
            if let Some(flag) = liquid_planes.get_mut(planenum) {
                *flag = true;
            }
        }
    }
    let mut edge_split_verts = vec![[u16::MAX; 3]; n_edges];
    for edge_idx in 0..n_edges {
        let segs = edge_segments[edge_idx] as usize;
        if segs <= 1 {
            continue;
        }
        let eo = edge_idx * SZ_EDGE;
        let (Some(v0), Some(v1)) = (u16le(edges, eo), u16le(edges, eo + 2)) else {
            continue;
        };
        if (v0 as usize) >= orig_n_verts || (v1 as usize) >= orig_n_verts {
            continue;
        }
        let a = raw[v0 as usize];
        let b = raw[v1 as usize];
        for r in 1..segs {
            if verts.len() >= MAX_COOK_VERTS {
                break;
            }
            let frac = r as f32 / segs as f32;
            let p = [
                a[0] + (b[0] - a[0]) * frac,
                a[1] + (b[1] - a[1]) * frac,
                a[2] + (b[2] - a[2]) * frac,
            ];
            let idx = verts.len() as u16;
            raw_verts.push(p);
            verts.push([
                (p[0] / scale).round() as i16,
                (p[2] / scale).round() as i16,
                (p[1] / scale).round() as i16,
            ]);
            edge_split_verts[edge_idx][r - 1] = idx;
        }
    }

    // Topology-retention diagnostics reported after the face pass.
    // Sidedness histogram over faces that actually emit: native `numedges` and
    // the emitted polygon length `poly.len()` (after T-junction edge splits).
    let mut hist_ne = [0usize; 14];
    let mut hist_pl = [0usize; 14];
    let mut faces_emitted = 0usize;
    let mut bakeable_quads = 0usize;
    let mut fan_tris = 0usize;
    let grid_base_verts = verts.len();
    let mut grid_added_tris = 0usize;
    let mut grid_cells = 0usize;
    let mut grid_quad_cells = 0usize;
    let mut grid_boundary_tris = 0usize;
    let mut grid_budget_fallbacks = 0usize;
    let mut face_affine_quality = vec![false; face_affine_candidate.len()];
    let mut pending_affine_faces = Vec::new();
    let mut native_blocked_spans = Vec::<([i16; 3], [i16; 3])>::new();
    for f in 0..n_faces {
        let fo = f * SZ_FACE;
        let firstedge = i32le(faces, fo + 4).unwrap() as usize;
        let numedges = u16le(faces, fo + 8).unwrap() as usize;
        let ti = u16le(faces, fo + 10).unwrap() as usize;
        if numedges < 3 {
            continue;
        }
        // Walk surfedges -> ordered polygon of vertex indices.
        let mut poly: Vec<u16> = Vec::with_capacity(numedges);
        for j in 0..numedges {
            let se = match i32le(surf, (firstedge + j) * SZ_SURFEDGE) {
                Some(v) => v,
                None => break,
            };
            let edge_idx = se.unsigned_abs() as usize;
            let e = edge_idx * SZ_EDGE;
            let start = if se >= 0 {
                u16le(edges, e)
            } else {
                u16le(edges, e + 2)
            };
            if let Some(v) = start {
                if (v as usize) < raw_verts.len() {
                    poly.push(v);
                }
            }
            if edge_idx < edge_split_verts.len() {
                let segs = edge_segments[edge_idx] as usize;
                if segs > 1 {
                    if se >= 0 {
                        for r in 1..segs {
                            let v = edge_split_verts[edge_idx][r - 1];
                            if v != u16::MAX && (v as usize) < raw_verts.len() {
                                poly.push(v);
                            }
                        }
                    } else {
                        for r in (1..segs).rev() {
                            let v = edge_split_verts[edge_idx][r - 1];
                            if v != u16::MAX && (v as usize) < raw_verts.len() {
                                poly.push(v);
                            }
                        }
                    }
                }
            }
        }
        if poly.len() < 3 {
            continue;
        }
        let mtx = i32le(texinfo, ti * SZ_TEXINFO + 32).unwrap_or(-1);
        let tex_id = if mtx >= 0 && (mtx as usize) < n_texs {
            mtx as usize
        } else {
            0
        };
        if is_tool_texture(&tex_names[tex_id]) {
            continue;
        }
        let planenum = u16le(faces, fo).unwrap_or(u16::MAX) as usize;
        let logical_poly = collapse_collinear_loop(&poly, &verts, orig_n_verts);
        let static_ent_face =
            face_submodel[f] != u16::MAX && submodel_static_eligible[face_submodel[f] as usize];
        let native_patch = (f >= world_first_face && f < world_end_face || static_ent_face)
            && logical_poly.len() == 4
            && !face_translucent[f]
            && !liquid_planes.get(planenum).copied().unwrap_or(false);
        face_native_patch[f] = native_patch;
        if native_patch {
            for (a, b) in collapsed_boundary_spans(&poly, &logical_poly) {
                native_blocked_spans.push((verts[a as usize], verts[b as usize]));
            }
        }
        if tex_names[tex_id].starts_with('{') {
            // Only roughly horizontal cutouts (floor grates/walkways) take the
            // runtime OT pull-forward, and only when solid brushwork exists
            // within ~96 units below: those must win the painter tie against
            // the slabs seen through them. WORLD vertical cutouts (gates,
            // fences, wall trim) are exactly the wedge hazard: pulled forward
            // they paint over nearer opaque geometry, so they never get the
            // flag. Vertical BRUSH-ENTITY cutouts (ladders, wall rails) are
            // the opposite case: entities emit after the world, so a ladder
            // pressed flat against a wall loses the painter tie to that wall
            // and vanishes. Give them the pull too, but only with backing
            // within 24 units (pressed-against, not free-standing).
            let side = u16le(faces, fo + 2).unwrap_or(0);
            let po = planenum * SZ_PLANE;
            let mut n = [
                f32le(planes, po).unwrap_or(0.0),
                f32le(planes, po + 4).unwrap_or(0.0),
                f32le(planes, po + 8).unwrap_or(0.0),
            ];
            if side != 0 {
                n = [-n[0], -n[1], -n[2]];
            }
            let vertical_ent = n[2].abs() < 0.35 && face_model[f] != 0;
            if n[2].abs() >= 0.35 || vertical_ent {
                let mut center = [0.0f32; 3];
                for &vi in &poly {
                    let p = raw_verts[vi as usize];
                    center = [center[0] + p[0], center[1] + p[1], center[2] + p[2]];
                }
                let inv = 1.0 / poly.len() as f32;
                center = [center[0] * inv, center[1] * inv, center[2] * inv];
                // Probe the centre plus corner-biased points: a wide grate's
                // centre can sit over a gap between the slabs beneath it.
                let mut probes: Vec<[f32; 3]> = vec![center];
                for &vi in poly.iter().take(4) {
                    let p = raw_verts[vi as usize];
                    probes.push([
                        p[0] * 0.75 + center[0] * 0.25,
                        p[1] * 0.75 + center[1] * 0.25,
                        p[2] * 0.75 + center[2] * 0.25,
                    ]);
                }
                // Deep probes: a walkway grate is often a thick box brush, so
                // the first spans land inside its own (excluded) volume before
                // reaching the slabs beneath.
                let depths: &[f32] = if vertical_ent {
                    &[8.0, 24.0]
                } else {
                    &[8.0, 24.0, 48.0, 96.0, 160.0, 256.0]
                };
                face_cutout_backed[f] = probes.iter().any(|q| {
                    depths.iter().any(|d| {
                        let p = [q[0] - n[0] * d, q[1] - n[1] * d, q[2] - n[2] * d];
                        point_solid_any(p, face_model[f] as usize)
                    })
                });
            }
        }
        let render_poly = if native_patch { &logical_poly } else { &poly };
        let mut mn = [i32::MAX; 3];
        let mut mx = [i32::MIN; 3];
        for &vi in &poly {
            let p = verts[vi as usize];
            let q = [p[0] as i32, p[1] as i32, p[2] as i32];
            for k in 0..3 {
                mn[k] = mn[k].min(q[k]);
                mx[k] = mx[k].max(q[k]);
            }
        }
        for k in 0..3 {
            let c = (mn[k] + mx[k]) / 2;
            let e = ((mx[k] - mn[k]).abs() / 2 + 4).clamp(0, u16::MAX as i32);
            face_center[f][k] = c.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            face_extent[f][k] = e as u16;
        }
        // The emissive strip identifies which named styles belong to an
        // indicator assembly, but GoldSrc applies those styles to every face
        // whose lightmap references them. Restricting the slot to the `~`
        // texture lights only the six-unit strip and leaves the large board
        // dark—the barely visible three-pixel change caught by the first
        // regression attempt.
        (face_dynamic_lightstyles[f], face_dynamic_layers[f]) = dynamic_lightstyle_slots_for_face(
            faces.get(fo + 12..fo + 16).unwrap_or(&[]),
            &dynamic_lightstyles,
        );
        let (fw, fh) = (texs[tex_id].w as f32, texs[tex_id].h as f32);
        let (ow, oh) = orig[tex_id];
        // texinfo s/t planes (original texels).
        let to = ti * SZ_TEXINFO;
        let s = [
            f32le(texinfo, to).unwrap(),
            f32le(texinfo, to + 4).unwrap(),
            f32le(texinfo, to + 8).unwrap(),
        ];
        let s_off = f32le(texinfo, to + 12).unwrap();
        let t = [
            f32le(texinfo, to + 16).unwrap(),
            f32le(texinfo, to + 20).unwrap(),
            f32le(texinfo, to + 24).unwrap(),
        ];
        let t_off = f32le(texinfo, to + 28).unwrap();
        // Per-poly-vertex UV: cooked-texel for output, original-texel for the
        // lightmap extents.
        let mut uv: Vec<(f32, f32)> = Vec::with_capacity(render_poly.len());
        let mut ouv: Vec<(f32, f32)> = Vec::with_capacity(render_poly.len());
        let (mut minu, mut minv) = (f32::MAX, f32::MAX);
        let (mut lu0, mut lu1, mut lv0, mut lv1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &vi in render_poly {
            let p = raw_verts[vi as usize];
            let ou = p[0] * s[0] + p[1] * s[1] + p[2] * s[2] + s_off; // original texels
            let ov = p[0] * t[0] + p[1] * t[1] + p[2] * t[2] + t_off;
            lu0 = lu0.min(ou);
            lu1 = lu1.max(ou);
            lv0 = lv0.min(ov);
            lv1 = lv1.max(ov);
            let (u, v) = (ou * fw / ow as f32, ov * fh / oh as f32);
            minu = minu.min(u);
            minv = minv.min(v);
            uv.push((u, v));
            ouv.push((ou, ov));
        }
        // Per-vertex lightmap shade: luxels are 16 texels apart (Quake/GoldSrc).
        let style0 = *faces.get(fo + 12).unwrap_or(&0xFF);
        let lightofs = i32le(faces, fo + 16).unwrap_or(-1);
        let lmw = (((lu1 / 16.0).ceil() - (lu0 / 16.0).floor()) as i64 + 1).clamp(1, 64) as usize;
        let lmh = (((lv1 / 16.0).ceil() - (lv0 / 16.0).floor()) as i64 + 1).clamp(1, 64) as usize;
        let mins_s = (lu0 / 16.0).floor() as i32;
        let mins_t = (lv0 / 16.0).floor() as i32;
        face_lm_dims[f] = [lmw as u8, lmh as u8];
        face_lm_mins[f] = [mins_s as i16, mins_t as i16];
        let shade: Vec<(u8, u8, u8)> = ouv
            .iter()
            .map(|&(ou, ov)| {
                vertex_shade(lighting, lightofs, style0, lmw, lmh, ou, ov, mins_s, mins_t)
            })
            .collect();
        // Face brightness (spawn choice): mean corner luminance. Unlit faces
        // (lightofs<0) render fullbright, so count them as bright.
        face_bright[f] = if lightofs < 0 {
            128
        } else {
            let sum: u32 = shade
                .iter()
                .map(|&(r, g, b)| (r as u32 + g as u32 + b as u32) / 3)
                .sum();
            (sum / shade.len().max(1) as u32).min(255) as u8
        };
        // Shift by whole texture tiles so values start near 0 (preserves tiling
        // phase), then saturate to u8. Faces tiling more than ~4x clamp at the
        // far edge -- proper tiling of huge surfaces needs UV subdivision (M3).
        let shu = (minu / fw).floor() * fw;
        let shv = (minv / fh).floor() * fh;
        let shifted_uv: Vec<(f32, f32)> = uv.iter().map(|&(u, v)| (u - shu, v - shv)).collect();
        // Turbulent/liquid materials already animate their UVs at runtime and
        // often overlap a visible floor. On a PS1 there is no Z buffer to
        // preserve ownership between two coplanar materials after only one is
        // split, so keep every face on a liquid plane at source topology and
        // spend the refinement budget on unambiguous opaque surfaces.
        // The 8-bit UV wrap is a correctness cut. Large faces inside one wrap
        // also receive a coarse quality grid; the enabled classic runtime
        // renderer decides their additional view-dependent refinement.
        let cand_uv_max = shifted_uv
            .iter()
            .flat_map(|&(u, v)| [u, v])
            .fold(0.0f32, f32::max);
        let cand_uv_span = {
            let us: Vec<f32> = shifted_uv.iter().map(|p| p.0).collect();
            let vs: Vec<f32> = shifted_uv.iter().map(|p| p.1).collect();
            (us.iter().cloned().fold(f32::MIN, f32::max)
                - us.iter().cloned().fold(f32::MAX, f32::min))
            .max(
                vs.iter().cloned().fold(f32::MIN, f32::max)
                    - vs.iter().cloned().fold(f32::MAX, f32::min),
            )
        };
        let cand_world_span = face_extent[f].iter().copied().max().unwrap_or(0) as u32 * 2;
        let cand_quality = cand_world_span >= AFFINE_QUALITY_MIN_WORLD_SPAN
            && cand_uv_span >= AFFINE_QUALITY_MIN_UV_SPAN;
        face_affine_quality[f] = cand_quality && cand_uv_max <= 255.0;
        face_affine_candidate[f] = native_patch && (cand_uv_max > 255.0 || cand_quality);
        // Preserve the ordered source polygon until the tessellation decision.
        // Large opaque faces are clipped into UV-aligned cells; only untouched
        // faces use the compact source fan.
        let source_corners: Vec<CookCorner> = render_poly
            .iter()
            .enumerate()
            .map(|(i, &idx)| CookCorner {
                idx,
                pos: verts[idx as usize],
                uv: shifted_uv[i],
                shade: shade[i],
            })
            .collect();
        // Count only faces that reach emission. fan_tris = poly.len()-2; a
        // face yields floor(fan_tris/2) cleanly bakeable quads.
        faces_emitted += 1;
        hist_ne[numedges.min(13)] += 1;
        hist_pl[render_poly.len().min(13)] += 1;
        let ft = render_poly.len() - 2;
        fan_tris += ft;
        bakeable_quads += ft / 2;
        face_fan_ntri[f] = ft.min(u16::MAX as usize) as u16;
        let uv_span = (uv.iter().map(|p| p.0).fold(f32::MIN, f32::max)
            - uv.iter().map(|p| p.0).fold(f32::MAX, f32::min))
        .max(
            uv.iter().map(|p| p.1).fold(f32::MIN, f32::max)
                - uv.iter().map(|p| p.1).fold(f32::MAX, f32::min),
        );
        let world_span = face_extent[f].iter().copied().max().unwrap_or(0) as u32 * 2;
        let proximity_boost =
            affine_proximity_boost(face_center[f], face_extent[f], &affine_traversal_samples);
        let detail_boost = affine_texture_detail_boost(&tex_names[tex_id]);
        let grid_step = if face_affine_quality[f] {
            UV_GRID_QUALITY
        } else if world_span >= UV_GRID_FINE_WORLD_SPAN || uv_span >= UV_GRID_FINE_TEXEL_SPAN {
            UV_GRID_FINE
        } else {
            UV_GRID_COARSE
        };
        if face_affine_candidate[f] {
            // Faces are emitted after this BSP walk. Face records carry
            // explicit triangle offsets, so storage order is independent of
            // BSP/PVS ownership.
            let risk =
                (world_span as u64).saturating_mul((uv_span.max(0.0) * 256.0).round() as u64);
            pending_affine_faces.push(PendingAffineFace {
                face: f,
                risk,
                priority: 0,
                proximity_boost,
                detail_boost,
                estimated_added_tris: 0,
                source: source_corners,
                tex_id: tex_id as u16,
                grid_step,
                fan_anchor: best_fan_anchor(&shifted_uv),
                source_tris: ft,
            });
        } else {
            let first_tri = tri_idx.len() / 3;
            face_first[f] = first_tri as u32;
            let fan0 = best_fan_anchor(&shifted_uv);
            emit_source_fan(
                &source_corners,
                tex_id as u16,
                fan0,
                &mut verts,
                &mut tri_idx,
                &mut tri_tex,
                &mut tri_uv,
                &mut tri_rgb,
            );
            face_ntri[f] = ((tri_idx.len() / 3) - first_tri).min(u16::MAX as usize) as u16;
        }
    }

    // Trial emission gives an exact added-triangle cost without retaining any
    // geometry. This is host-only cook work. Ranking risk-per-triangle avoids
    // letting a few enormous faces consume the whole fixed PS1 room budget.
    for pending in &mut pending_affine_faces {
        let verts_before = verts.len();
        let idx_before = tri_idx.len();
        let tex_before = tri_tex.len();
        let uv_before = tri_uv.len();
        let rgb_before = tri_rgb.len();
        let result = emit_uv_grid_poly(
            &pending.source,
            pending.tex_id,
            pending.grid_step,
            &mut verts,
            &mut tri_idx,
            &mut tri_tex,
            &mut tri_uv,
            &mut tri_rgb,
        );
        let emitted = tri_tex.len().saturating_sub(tex_before);
        pending.estimated_added_tris = if result.is_some() {
            emitted.saturating_sub(pending.source_tris)
        } else {
            0
        };
        pending.priority = affine_budget_priority(
            pending.risk,
            pending.proximity_boost.saturating_mul(pending.detail_boost),
            pending.estimated_added_tris,
        );
        verts.truncate(verts_before);
        tri_idx.truncate(idx_before);
        tri_tex.truncate(tex_before);
        tri_uv.truncate(uv_before);
        tri_rgb.truncate(rgb_before);
    }
    pending_affine_faces.sort_unstable_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| b.risk.cmp(&a.risk))
            .then_with(|| a.face.cmp(&b.face))
    });
    for pending in pending_affine_faces {
        let f = pending.face;
        let first_tri = tri_idx.len() / 3;
        face_first[f] = first_tri as u32;
        let verts_before = verts.len();
        let idx_before = tri_idx.len();
        let tex_before = tri_tex.len();
        let uv_before = tri_uv.len();
        let rgb_before = tri_rgb.len();
        let grid_result = emit_uv_grid_poly(
            &pending.source,
            pending.tex_id,
            pending.grid_step,
            &mut verts,
            &mut tri_idx,
            &mut tri_tex,
            &mut tri_uv,
            &mut tri_rgb,
        );
        let emitted_grid_tris = tri_tex.len().saturating_sub(tex_before);
        let added = emitted_grid_tris.saturating_sub(pending.source_tris);
        let grid_accepted = grid_result.is_some()
            // A one-cell result is the source polygon with a different
            // triangulation. Storing it as raw records wastes RAM and packets
            // without reducing affine error; keep that face as a compact loop.
            && added > 0
            && verts.len() <= MAX_COOK_VERTS;
        if grid_accepted {
            let stats = grid_result.unwrap();
            for pair_first in &stats.pair_first {
                grid_cell_pair_first.insert(tex_before + *pair_first);
            }
            grid_added_tris += added;
            grid_cells += stats.cells;
            grid_quad_cells += stats.quad_cells;
            grid_boundary_tris += stats.boundary_tris;
            face_affine_refined[f] = true;
        } else {
            grid_budget_fallbacks += 1;
            verts.truncate(verts_before);
            tri_idx.truncate(idx_before);
            tri_tex.truncate(tex_before);
            tri_uv.truncate(uv_before);
            tri_rgb.truncate(rgb_before);
            emit_source_fan(
                &pending.source,
                pending.tex_id,
                pending.fan_anchor,
                &mut verts,
                &mut tri_idx,
                &mut tri_tex,
                &mut tri_uv,
                &mut tri_rgb,
            );
        }
        face_ntri[f] = ((tri_idx.len() / 3) - first_tri).min(u16::MAX as usize) as u16;
    }

    let grid_added_verts = verts.len().saturating_sub(grid_base_verts);
    let grid_refined_faces = face_affine_refined.iter().filter(|&&v| v).count();
    eprintln!(
        "  UV cell grid: +{grid_added_verts} verts, +{grid_added_tris} tris, {grid_refined_faces} faces, {grid_cells} cells ({grid_quad_cells} quads, {grid_boundary_tris} boundary tris), {grid_budget_fallbacks} fallbacks"
    );

    // Report how much native polygon topology survives the mandatory UV-seam
    // cuts and can therefore use the compact quad-patch runtime path.
    {
        let pct = |n: usize| {
            if faces_emitted > 0 {
                100.0 * n as f64 / faces_emitted as f64
            } else {
                0.0
            }
        };
        let tri_after = fan_tris - bakeable_quads; // 1 record per quad + leftover tris
        let red = if fan_tris > 0 {
            100.0 * bakeable_quads as f64 / fan_tris as f64
        } else {
            0.0
        };
        eprintln!(
            "  native patches: faces={faces_emitted} fan_tris={fan_tris} quads={bakeable_quads}"
        );
        eprintln!(
            "  native sided: 3:{} 4:{} 5:{} 6:{} 7+:{}",
            hist_ne[3],
            hist_ne[4],
            hist_ne[5],
            hist_ne[6],
            hist_ne[7..].iter().sum::<usize>()
        );
        eprintln!(
            "  emitted poly: 3:{} 4:{} 5:{} 6:{} 7+:{}  (4-sided={:.0}% of faces)",
            hist_pl[3],
            hist_pl[4],
            hist_pl[5],
            hist_pl[6],
            hist_pl[7..].iter().sum::<usize>(),
            pct(hist_pl[4])
        );
        eprintln!("  compact records: triangle_fan={fan_tris} patch_path={tri_after} decode_reduction={red:.0}%");
    }

    // Budget the weld to the runtime's streaming buffer (`room_budget::MAP_WORDS`
    // u32 = ~914 KB). The resident size is everything-but-triangles (fixed by the
    // BSP) plus 19 B per triangle. Over-estimate the non-triangle bytes from the
    // raw BSP lumps (they are >= the compacted cooked sections) so the cap is
    // conservative and a map can never overflow. Maps already at the limit get a
    // zero budget and are left exactly as they were.
    const MAP_RESIDENT_BYTES: usize = 234_125 * 4 - 8192; // MAP_WORDS*4, 8 KB margin
    let lump_bytes = |i: usize| bsp.lump(i).len();
    let non_tri_est = verts.len() * 6
        + lump_bytes(LUMP_NODES)
        + lump_bytes(LUMP_LEAVES)
        + lump_bytes(LUMP_MARKSURFACES)
        + lump_bytes(LUMP_PLANES)
        + lump_bytes(LUMP_VISIBILITY)
        + lump_bytes(LUMP_CLIPNODES)
        + lump_bytes(LUMP_ENTITIES)
        + lump_bytes(LUMP_FACES)
        + 32768; // slop for nav/logic/prop/header sections not in the raw lumps
    let base_tri_bytes = (tri_idx.len() / 3) * 19;
    let max_added = MAP_RESIDENT_BYTES.saturating_sub(non_tri_est + base_tri_bytes) / 19;
    let quad_verts_before = verts.len();
    let before_quad_grid = face_affine_refined.clone();
    let backup = (
        verts.clone(),
        tri_idx.clone(),
        tri_tex.clone(),
        tri_uv.clone(),
        tri_rgb.clone(),
        face_first.clone(),
        face_ntri.clone(),
        grid_cell_pair_first.clone(),
    );
    let mut quad_added = quad_grid::refine(
        &mut verts,
        &mut tri_idx,
        &mut tri_tex,
        &mut tri_uv,
        &mut tri_rgb,
        &mut face_first,
        &mut face_ntri,
        &face_native_patch,
        &mut face_affine_refined,
        &mut grid_cell_pair_first,
        max_added.min(1536usize.saturating_sub(grid_added_tris)),
    );
    let mut tj_added = weld_tjunctions(
        &verts,
        &mut tri_idx,
        &mut tri_tex,
        &mut tri_uv,
        &mut tri_rgb,
        &mut face_first,
        &mut face_ntri,
        &face_affine_refined,
        &mut grid_cell_pair_first,
        max_added.saturating_sub(quad_added + ((verts.len() - quad_verts_before) * 6 + 18) / 19),
        quad_verts_before,
    );
    if quad_added != 0
        && !quad_grid::boundaries_conform(
            &verts,
            &tri_idx,
            &face_first,
            &face_ntri,
            quad_verts_before,
        )
    {
        eprintln!("  Conforming quad grid declined: neighbour boundary or weld budget");
        (
            verts,
            tri_idx,
            tri_tex,
            tri_uv,
            tri_rgb,
            face_first,
            face_ntri,
            grid_cell_pair_first,
        ) = backup;
        face_affine_refined = before_quad_grid.clone();
        quad_added = 0;
        tj_added = weld_tjunctions(
            &verts,
            &mut tri_idx,
            &mut tri_tex,
            &mut tri_uv,
            &mut tri_rgb,
            &mut face_first,
            &mut face_ntri,
            &face_affine_refined,
            &mut grid_cell_pair_first,
            max_added,
            usize::MAX,
        );
    }
    eprintln!("  Conforming quad grid: +{quad_added} triangles");
    for f in 0..n_faces {
        if face_affine_refined[f] && !before_quad_grid[f] {
            face_affine_candidate[f] = true;
            grid_cells += face_ntri[f] as usize / 2;
            grid_quad_cells += face_ntri[f] as usize / 2;
        }
    }
    grid_added_tris += quad_added;
    let grid_added_verts = grid_added_verts + verts.len() - quad_verts_before;
    let grid_refined_faces = face_affine_refined.iter().filter(|&&v| v).count();
    eprintln!("  T-junction weld: +{tj_added} tris (cap {max_added})");

    // Patch edge IDs are canonical pairs of 14-bit position indices. UV and
    // lighting live in each FaceVert, so coincident position indices can be
    // merged without losing a material seam. This makes an edge ID stable
    // across faces produced by different BSP/UV clipping paths.
    {
        let mut canonical = std::collections::HashMap::<[i16; 3], u16>::new();
        for index in &mut tri_idx {
            let p = verts[*index as usize];
            let first = *canonical.entry(p).or_insert(*index);
            *index = first;
        }
    }

    // Grid tessellation is the cook's affine-correction pass. Runtime classic
    // subdivision adds view-dependent detail. The weld above only restores
    // shared-boundary conformity and does not create vertices.
    let affine_base_verts = grid_base_verts;
    let affine_base_tris = (tri_idx.len() / 3).saturating_sub(grid_added_tris);
    let affine_added_verts = grid_added_verts;
    let affine_added_tris = grid_added_tris;
    let affine_refined_faces = grid_refined_faces;

    let n_verts = verts.len();
    let n_tris = tri_idx.len() / 3;
    // Runtime SCRATCH caps (main.rs MAX_VERTS): the render paths trust cooked
    // indices, so an oversized map must fail HERE, not panic on console.
    if n_verts > 12288 {
        return Err(format!(
            "{}: {} cooked verts exceeds the runtime MAX_VERTS cap of 12288",
            path, n_verts
        ));
    }
    // Hard guarantee for the runtime: every cooked corner (raw tris AND loop
    // faceverts read from this same array) references a real vertex. The
    // render hot paths trust this and skip per-tri bounds checks.
    for (i, &vi) in tri_idx.iter().enumerate() {
        assert!(
            (vi as usize) < n_verts,
            "tri corner {} references vert {} of {}",
            i,
            vi,
            n_verts
        );
    }
    if n_tris > u16::MAX as usize {
        return Err(format!(
            "{}: {} cooked triangles exceeds compact FaceRec limit of 65535",
            path, n_tris
        ));
    }
    let original_tex_count = texs.len();
    let (mut texs, stripped_tex_count, tex_anim_chains) =
        compact_textures_with_anim_chains(texs, &mut tri_tex, &tex_names);
    let skyname = worldspawn_skyname(bsp.lump(LUMP_ENTITIES));
    let sky_tex_base = match skyname.as_deref() {
        Some(sky) => match load_skybox_textures(path, sky) {
            Some(sky_texs) => {
                let base = texs.len();
                texs.extend(sky_texs);
                Some(base)
            }
            None => {
                eprintln!("warning: skybox '{}' not found under gfx/env", sky);
                None
            }
        },
        None => None,
    };
    let n_cooked_texs = texs.len();
    if n_cooked_texs > u8::MAX as usize + 1 {
        return Err(format!(
            "{}: {} cooked textures exceeds compact TriRec limit of 256",
            path, n_cooked_texs
        ));
    }
    // ---- Face geometry: palettise the lightmap, then split faces into vertex
    // loops (clean fans) vs raw tris (UV-split / welded). ----
    let n_corners = n_tris * 3;
    let corner_colors: Vec<(u8, u8, u8)> = (0..n_corners)
        .map(|c| (tri_rgb[c * 3], tri_rgb[c * 3 + 1], tri_rgb[c * 3 + 2]))
        .collect();
    let light_pal = median_cut(&corner_colors, 256);
    let light_idx: Vec<u8> = corner_colors
        .iter()
        .map(|&c| nearest_pal_index(&light_pal, c))
        .collect();
    let mut loopverts: Vec<u8> = Vec::new(); // FaceVert[5B] = u16 idx | u8 uv[2] | u8 light
    let mut raw_tris: Vec<u8> = Vec::new(); // TriRec[16B], dirty (UV-split/welded) faces only
    let mut face_lc_first = vec![0u32; n_faces];
    let mut face_lc_count = vec![0u16; n_faces];
    let mut face_lc_flag = vec![0u8; n_faces]; // bit0 loop, bit4 native patch, bit5 seamed pair
    let mut face_lc_tex = vec![0u8; n_faces];
    let mut tagged_quad_pairs = 0usize;
    let mut native_patch_quad_records = 0usize;
    let mut native_patch_triangle_records = 0usize;
    let mut native_patch_blocked_records = 0usize;
    let mut native_patch_seamed_pair_records = 0usize;
    for f in 0..n_faces {
        let first = face_first[f] as usize;
        let ntri = face_ntri[f] as usize;
        if ntri == 0 {
            continue;
        }
        face_lc_tex[f] = tri_tex[first] as u8;
        let lv_start = loopverts.len() / 5;
        // Clean fan AND the loop-vertex index fits u16 -> store as a loop:
        //   loop = [t0.c0, t0.c2, t0.c1, t1.c1, .., t_{ntri-1}.c1] (the fan poly).
        // Bit 2 means the cook changed this face's topology. Runtime gives its
        // descendants local painter keys and may merge only the exact regular
        // cell pairs recorded by `grid_cell_pair_first`.
        let refined_topology = face_affine_refined[f];
        if face_native_patch[f] {
            let mut patchverts = Vec::new();
            let mut seamed_patchverts = Vec::new();
            let mut has_seamed_pair = false;
            let end = first + ntri;
            let mut used = vec![false; ntri];
            for local in 0..ntri {
                if used[local] {
                    continue;
                }
                let t = first + local;
                let pair = if refined_topology {
                    // UV-grid ownership is exact: only the two triangles from
                    // one clipped cell may be reconstructed as a quad.
                    (t + 1 < end && grid_cell_pair_first.contains(&t))
                        .then(|| {
                            quad_pair_code(
                                t,
                                t + 1,
                                &tri_idx,
                                &tri_tex,
                                &tri_uv,
                                &light_idx,
                                &light_pal,
                            )
                            .map(|code| (local + 1, code))
                        })
                        .flatten()
                } else {
                    // T-junction welding retriangulates each source triangle
                    // independently. Its output order can therefore be
                    // [A0,A1,A2,B0,B1], where only A2/B0 are a valid quad;
                    // pairing consecutive records loses obvious four-sided
                    // patches. Search the same coplanar face for the first
                    // exact shared-diagonal partner, preferring adjacency.
                    ((local + 1)..ntri).find_map(|other| {
                        if used[other] {
                            return None;
                        }
                        quad_pair_code(
                            t,
                            first + other,
                            &tri_idx,
                            &tri_tex,
                            &tri_uv,
                            &light_idx,
                            &light_pal,
                        )
                        .map(|code| (other, code))
                    })
                };
                if let Some((other, code)) = pair {
                    let other_t = first + other;
                    let corners = encoded_quad_pair_corners(t, other_t, code);
                    let blocked =
                        patch_blocked_edge_mask(corners, &tri_idx, &verts, &native_blocked_spans);
                    native_patch_blocked_records += usize::from(blocked != 0);
                    // Stored corner slots 0,1,3,2 own runtime boundary edges
                    // 0,1,2,3 respectively. Bit14 preserves that edge mask in
                    // the otherwise-unused high index bits.
                    let blocked_slot = [
                        blocked & 1 != 0,
                        blocked & 2 != 0,
                        blocked & 8 != 0,
                        blocked & 4 != 0,
                    ];
                    for (slot, corner) in corners.into_iter().enumerate() {
                        push_patch_facevert(
                            &mut patchverts,
                            &tri_idx,
                            &tri_uv,
                            &light_idx,
                            corner,
                            false,
                            blocked_slot[slot],
                        );
                    }
                    tagged_quad_pairs += 1;
                    native_patch_quad_records += 1;
                    used[local] = true;
                    used[other] = true;
                } else if let Some((other, code)) = ((local + 1)..ntri).find_map(|other| {
                    if used[other] {
                        None
                    } else {
                        geometry_quad_pair_code(t, first + other, &tri_idx, &tri_tex)
                            .map(|code| (other, code))
                    }
                }) {
                    // Positional quad with an authored UV/light seam: retain
                    // both GT3 attribute sets. Canonicalise them as
                    // `(b,a,c) + (a,d,c)` so runtime gets the four boundary
                    // vertices directly without another topology search.
                    let other_t = first + other;
                    let (first_t, second_t) = if code & 0x10 == 0 {
                        (t, other_t)
                    } else {
                        (other_t, t)
                    };
                    let r0 = ((code >> 2) & 3) as usize;
                    let r1 = (code & 3) as usize;
                    push_triangle_patch_corners(
                        &mut seamed_patchverts,
                        &tri_idx,
                        &tri_uv,
                        &light_idx,
                        [
                            first_t * 3 + r0,
                            first_t * 3 + (r0 + 1) % 3,
                            first_t * 3 + (r0 + 2) % 3,
                        ],
                        true,
                    );
                    push_triangle_patch_corners(
                        &mut seamed_patchverts,
                        &tri_idx,
                        &tri_uv,
                        &light_idx,
                        [
                            second_t * 3 + r1,
                            second_t * 3 + (r1 + 1) % 3,
                            second_t * 3 + (r1 + 2) % 3,
                        ],
                        false,
                    );
                    native_patch_triangle_records += 2;
                    native_patch_seamed_pair_records += 1;
                    has_seamed_pair = true;
                    used[local] = true;
                    used[other] = true;
                } else {
                    push_triangle_patch_record(
                        &mut patchverts,
                        &tri_idx,
                        &tri_uv,
                        &light_idx,
                        t,
                        false,
                    );
                    native_patch_triangle_records += 1;
                    used[local] = true;
                }
            }
            if has_seamed_pair {
                seamed_patchverts.extend_from_slice(&patchverts);
                patchverts = seamed_patchverts;
            }
            let patch_count = patchverts.len() / (4 * 5);
            if patch_count != 0
                && patch_count <= u16::MAX as usize
                && lv_start + patch_count * 4 <= u16::MAX as usize
            {
                loopverts.extend_from_slice(&patchverts);
                face_lc_first[f] = lv_start as u32;
                face_lc_count[f] = patch_count as u16;
                face_lc_flag[f] =
                    0x10 | ((refined_topology as u8) << 2) | ((has_seamed_pair as u8) << 5);
                continue;
            }
        }
        let stored_count = ntri + 2;
        if !refined_topology
            && ntri == face_fan_ntri[f] as usize
            && face_is_exact_packed_fan(first, ntri, &tri_idx, &tri_tex, &tri_uv, &light_idx)
            && lv_start + stored_count <= u16::MAX as usize
        {
            push_facevert(&mut loopverts, &tri_idx, &tri_uv, &light_idx, first * 3);
            push_facevert(&mut loopverts, &tri_idx, &tri_uv, &light_idx, first * 3 + 2);
            for j in 0..ntri {
                push_facevert(
                    &mut loopverts,
                    &tri_idx,
                    &tri_uv,
                    &light_idx,
                    (first + j) * 3 + 1,
                );
            }
            face_lc_first[f] = lv_start as u32;
            face_lc_count[f] = stored_count as u16;
            face_lc_flag[f] = 1 | ((refined_topology as u8) << 2);
        } else {
            let rt_start = raw_tris.len() / 16;
            for j in 0..ntri {
                let t = first + j;
                let code = if refined_topology && j + 1 < ntri && grid_cell_pair_first.contains(&t)
                {
                    quad_pair_code(
                        t,
                        t + 1,
                        &tri_idx,
                        &tri_tex,
                        &tri_uv,
                        &light_idx,
                        &light_pal,
                    )
                    .unwrap_or(0)
                } else {
                    0
                };
                tagged_quad_pairs += (code != 0) as usize;
                for k in 0..3 {
                    let encoded = tri_idx[t * 3 + k] | (((code >> (k * 2)) & 3) as u16) << 14;
                    raw_tris.extend_from_slice(&encoded.to_le_bytes());
                }
                raw_tris.extend_from_slice(&tri_uv[t * 6..t * 6 + 6]);
                raw_tris.push(tri_tex[t] as u8);
                raw_tris.push(light_idx[t * 3]);
                raw_tris.push(light_idx[t * 3 + 1]);
                raw_tris.push(light_idx[t * 3 + 2]);
            }
            face_lc_first[f] = rt_start as u32;
            face_lc_count[f] = ntri as u16;
            face_lc_flag[f] = (refined_topology as u8) << 2;
        }
    }
    eprintln!("  legacy paired GT4 records: {tagged_quad_pairs}");
    let n_loopverts = loopverts.len() / 5;
    let n_raw_tris = raw_tris.len() / 16;

    // ---- Sparse GoldSrc lightstyle planes (HLMG) ----
    // Only faces that reference one of the named switchable styles carry this
    // appendix. Each stored geometry corner keeps an RGB contribution for each
    // authored style plane; OFF therefore adds exactly zero and cannot alter
    // neighbouring/base textures. Loop/native-patch faces remain compact (and
    // fast) instead of being demoted to raw triangles just to carry lighting.
    // A 64-colour RGB555 delta palette avoids another 1 KiB PS1 BSS table.
    struct DynamicFaceCook {
        compact_face: u16,
        first: u16,
        item_count: u16,
        slots: [u8; 3],
        loop_pool: bool,
        colors: Vec<(u8, u8, u8)>,
    }
    let mut dynamic_faces = Vec::<DynamicFaceCook>::new();
    let mut compact_face = 0u16;
    for f in 0..n_faces {
        if face_ntri[f] == 0 {
            continue;
        }
        let slots = face_dynamic_lightstyles[f];
        if slots.iter().any(|&slot| slot != 0) {
            let fo = f * SZ_FACE;
            let ti = u16le(faces, fo + 10).unwrap_or(0) as usize;
            let to = ti * SZ_TEXINFO;
            let s = [
                f32le(texinfo, to).unwrap_or(0.0),
                f32le(texinfo, to + 4).unwrap_or(0.0),
                f32le(texinfo, to + 8).unwrap_or(0.0),
            ];
            let s_off = f32le(texinfo, to + 12).unwrap_or(0.0);
            let t_axis = [
                f32le(texinfo, to + 16).unwrap_or(0.0),
                f32le(texinfo, to + 20).unwrap_or(0.0),
                f32le(texinfo, to + 24).unwrap_or(0.0),
            ];
            let t_off = f32le(texinfo, to + 28).unwrap_or(0.0);
            let [lmw, lmh] = face_lm_dims[f];
            let [mins_s, mins_t] = face_lm_mins[f];
            let lightofs = i32le(faces, fo + 16).unwrap_or(-1);
            let sample = |vertex: u16, plane: usize| {
                let p = verts[(vertex & 0x3fff) as usize];
                // Cooked world is [HL x, HL z, HL y] / scale.
                let hl = [
                    p[0] as f32 * scale,
                    p[2] as f32 * scale,
                    p[1] as f32 * scale,
                ];
                let ou = hl[0] * s[0] + hl[1] * s[1] + hl[2] * s[2] + s_off;
                let ov = hl[0] * t_axis[0] + hl[1] * t_axis[1] + hl[2] * t_axis[2] + t_off;
                if slots[plane] == 0 {
                    (0, 0, 0)
                } else {
                    dynamic_vertex_shade(
                        lighting,
                        lightofs,
                        face_dynamic_layers[f][plane] as usize,
                        lmw as usize,
                        lmh as usize,
                        ou,
                        ov,
                        mins_s as i32,
                        mins_t as i32,
                    )
                }
            };
            let loop_pool = face_lc_flag[f] & 0x11 != 0;
            let first = face_lc_first[f] as usize;
            let item_count = if face_lc_flag[f] & 0x10 != 0 {
                face_lc_count[f] as usize * 4
            } else {
                face_lc_count[f] as usize
            };
            let mut colors = Vec::with_capacity(item_count * if loop_pool { 3 } else { 9 });
            if loop_pool {
                for item in 0..item_count {
                    let o = (first + item) * 5;
                    let vertex = u16::from_le_bytes([loopverts[o], loopverts[o + 1]]);
                    for plane in 0..3 {
                        colors.push(sample(vertex, plane));
                    }
                }
            } else {
                let source_first = face_first[f] as usize;
                for local in 0..item_count {
                    let tri = source_first + local;
                    for plane in 0..3 {
                        for corner in 0..3 {
                            colors.push(sample(tri_idx[tri * 3 + corner], plane));
                        }
                    }
                }
            }
            dynamic_faces.push(DynamicFaceCook {
                compact_face,
                first: face_lc_first[f] as u16,
                item_count: item_count as u16,
                slots,
                loop_pool,
                colors,
            });
        }
        compact_face = compact_face.saturating_add(1);
    }
    let nonzero_dynamic_colors: Vec<_> = dynamic_faces
        .iter()
        .flat_map(|face| face.colors.iter().copied())
        .filter(|&color| color != (0, 0, 0))
        .collect();
    let mut dynamic_palette = vec![(0u8, 0u8, 0u8)];
    dynamic_palette.extend(median_cut(&nonzero_dynamic_colors, 63));
    dynamic_palette.truncate(64);
    let dynamic_indices: Vec<Vec<u8>> = dynamic_faces
        .iter()
        .map(|face| {
            face.colors
                .iter()
                .map(|&color| nearest_pal_index(&dynamic_palette, color))
                .collect()
        })
        .collect();
    let dynamic_bytes: usize = dynamic_indices.iter().map(Vec::len).sum();
    if dynamic_faces.len() > 0x7fff || dynamic_bytes > u16::MAX as usize {
        return Err(format!(
            "{}: sparse dynamic-light appendix exceeds 16-bit limits ({} faces, {} bytes)",
            path,
            dynamic_faces.len(),
            dynamic_bytes
        ));
    }
    eprintln!(
        "  dynamic lightmaps: {} faces, {} corner bytes, {} delta colours",
        dynamic_faces.len(),
        dynamic_bytes,
        dynamic_palette.len()
    );

    // GoldSrc makes water transparency a map-wide PVS decision. Keep that
    // decision separate from the per-face liquid identity: the runtime needs
    // the latter even on opaque-water maps to render the boundary two-sided
    // and to join visibility across it while the camera is submerged.
    let water_alpha_supported = map_supports_water_alpha(
        leaves,
        bsp.lump(LUMP_VISIBILITY),
        world_visleaf_count(models, leaves.len() / SZ_LEAF)?,
    );

    let mut o: Vec<u8> = Vec::new();
    // HLMH adds an explicit source-addressed world-pipeline section offset.
    // Earlier formats derived every appendix location from neighbouring data;
    // the renderer replacement is large enough to deserve a versioned,
    // independently auditable boundary.
    // HLMF appends the texture-animation chain section after the logic names
    // (the runtime derives its offset from the logic header, so the 52-byte
    // header layout is unchanged).
    // HLMF changes plane normals from Q12 to compatible Q14. Bumping the
    // format prevents an old room from being interpreted with the new scale.
    o.extend_from_slice(&cooked::MAGIC_LATEST_BYTES);
    o.extend_from_slice(&(n_verts as u32).to_le_bytes());
    o.extend_from_slice(&(n_raw_tris as u32).to_le_bytes());
    o.extend_from_slice(&(n_cooked_texs as u32).to_le_bytes());
    let face_count_pos = o.len();
    o.extend_from_slice(&0u32.to_le_bytes()); // compact cooked face count, patched below
    let bsp_off_pos = o.len();
    o.extend_from_slice(&0u32.to_le_bytes()); // BSP section offset, patched below
    let clip_off_pos = o.len();
    o.extend_from_slice(&0u32.to_le_bytes()); // clip/phys section offset, patched below
    let ent_off_pos = o.len();
    o.extend_from_slice(&0u32.to_le_bytes()); // entity section offset, patched below
    let tram_off_pos = o.len();
    o.extend_from_slice(&0u32.to_le_bytes()); // tram section offset, patched below
    let prop_off_pos = o.len();
    o.extend_from_slice(&0u32.to_le_bytes()); // prop (model placement) section offset
    o.extend_from_slice(&(sky_tex_base.map(|i| i as u32).unwrap_or(u32::MAX)).to_le_bytes());
    let nav_off_pos = o.len();
    o.extend_from_slice(&0u32.to_le_bytes()); // AI navigation section offset
    let logic_off_pos = o.len();
    o.extend_from_slice(&0u32.to_le_bytes()); // target/use/touch logic section offset
    let world_pipeline_off_pos = o.len();
    o.extend_from_slice(&0u32.to_le_bytes()); // source-addressed world pipeline
    debug_assert_eq!(o.len(), cooked::HEADER_SIZE);
    debug_assert_eq!(face_count_pos, cooked::HEADER_FACE_COUNT_OFFSET);
    debug_assert_eq!(bsp_off_pos, cooked::HEADER_BSP_OFFSET);
    debug_assert_eq!(clip_off_pos, cooked::HEADER_CLIP_OFFSET);
    debug_assert_eq!(ent_off_pos, cooked::HEADER_ENTITY_OFFSET);
    debug_assert_eq!(tram_off_pos, cooked::HEADER_TRAM_OFFSET);
    debug_assert_eq!(prop_off_pos, cooked::HEADER_PROP_OFFSET);
    debug_assert_eq!(nav_off_pos, cooked::HEADER_NAV_OFFSET);
    debug_assert_eq!(logic_off_pos, cooked::HEADER_LOGIC_OFFSET);
    debug_assert_eq!(world_pipeline_off_pos, cooked::HEADER_WORLD_PIPELINE_OFFSET);
    for v in &verts {
        for c in v {
            o.extend_from_slice(&c.to_le_bytes());
        }
    }
    // Section layout after verts: u32 n_loopverts | FaceVert[5B]×n_loopverts |
    // TriRec[16B]×n_raw_tris | light palette (u16 rgb555 × 256). Loop-faces store
    // their fan as a de-duplicated vertex loop; UV-split/welded faces keep tris.
    o.extend_from_slice(&(n_loopverts as u32).to_le_bytes());
    while o.len() % 4 != 0 {
        o.push(0);
    }
    for i in 0..n_loopverts {
        o.extend_from_slice(&loopverts[i * 5..i * 5 + 4]);
    }
    for i in 0..n_loopverts {
        o.push(loopverts[i * 5 + 4]);
    }
    // 4-align the TriRec array (5-byte FaceVerts break parity): the runtime
    // decodes each 16-byte record as four u32 loads.
    while o.len() % 4 != 0 {
        o.push(0);
    }
    o.extend_from_slice(&raw_tris);
    for i in 0..256 {
        let c = light_pal.get(i).copied().unwrap_or((110, 110, 110));
        o.extend_from_slice(&pack_rgb555(c.0, c.1, c.2).to_le_bytes());
    }
    // HLMG dynamic appendix: count/reserved, fixed 64-entry delta palette,
    // 12-byte face records, then 9 bytes per raw triangle (3 styles x 3
    // corners). Record data offsets are relative to the final byte blob.
    // Bit 15 is spare because the sparse face count is bounded to 15 bits.
    // Carry the map-wide water-alpha verdict here without growing HLMG.
    let dynamic_face_header =
        dynamic_faces.len() as u16 | if water_alpha_supported { 0x8000 } else { 0 };
    o.extend_from_slice(&dynamic_face_header.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    for i in 0..64 {
        let c = dynamic_palette.get(i).copied().unwrap_or((0, 0, 0));
        o.extend_from_slice(&pack_rgb555(c.0, c.1, c.2).to_le_bytes());
    }
    let mut dynamic_data_offset = 0u16;
    for (face, indices) in dynamic_faces.iter().zip(&dynamic_indices) {
        o.extend_from_slice(&face.compact_face.to_le_bytes());
        o.extend_from_slice(&face.first.to_le_bytes());
        o.extend_from_slice(&face.item_count.to_le_bytes());
        o.extend_from_slice(&face.slots);
        o.push(face.loop_pool as u8);
        o.extend_from_slice(&dynamic_data_offset.to_le_bytes());
        dynamic_data_offset = dynamic_data_offset.saturating_add(indices.len() as u16);
    }
    for indices in &dynamic_indices {
        o.extend_from_slice(indices);
    }
    while o.len() % 4 != 0 {
        o.push(0);
    }
    let texture_chunk = tex_out.map(|_| build_texture_chunk(&texs));
    if tex_out.is_none() {
        // Legacy single-file cook: keep the texture blob inline before BSP.
        // Runtime room builds pass `tex_out` and load the HLTX chunk only for
        // VRAM upload, then overwrite that staging buffer with resident HLMD.
        append_texture_blob(&mut o, &texs);
    }

    // ---- BSP visibility (PVS) ----
    // u32 n_planes,n_face_groups,n_nodes,leaf_counts,n_marks,vis_len |
    // leaf_counts = total n_leaves in low 16 | dmodel[0].visleafs in high 16.
    // PlaneRec[10B] | FaceGroup[2B] | FaceRec[18B] | nodes[6B] |
    // leaves[8B] | marks (pad) | vis (raw RLE, pad).
    // PlaneRec = i16 normal[3], i32 dist_q5. FaceGroup is a signed plane ref:
    // >=0 uses plane N, <0 uses inverted plane -N-1. FaceRec = u16 first_tri,
    // u16 tri_count, u16 plane_group, i16 center[3], u16 extent[3].
    let bsp_off = o.len() as u32;
    o[bsp_off_pos..bsp_off_pos + 4].copy_from_slice(&bsp_off.to_le_bytes());
    let marks = bsp.lump(LUMP_MARKSURFACES);
    let vis = bsp.lump(LUMP_VISIBILITY);
    let n_planes = planes.len() / SZ_PLANE;
    let n_nodes = nodes.len() / SZ_NODE;
    let n_leaves = leaves.len() / SZ_LEAF;
    let src_n_marks = marks.len() / SZ_MARKSURFACE;
    let clipnodes = bsp.lump(LUMP_CLIPNODES);
    let raw_n_clip = clipnodes.len() / SZ_CLIPNODE;
    let n_visleaves = world_visleaf_count(models, n_leaves)?;
    let packed_leaf_counts = pack_leaf_counts(n_leaves, n_visleaves)?;
    // dmodel_t.headnode[0] indexes LUMP_NODES, not LUMP_CLIPNODES. Runtime
    // point traces walk the already-cooked render-node tree directly; feeding
    // this value into the clipnode compactor aliases an unrelated expanded
    // hull (c1a1b prop floors ended up 37 units too high).
    let hull1_head_raw = model_headnode(models, 0, 1).unwrap_or(0); // standing player hull
    let hull3_head_raw = model_headnode(models, 0, 3).unwrap_or(0); // crouch hull, 32x32x36
    let (tram_model, tram_speed, tram_start, tram_wheels, way, _) =
        collect_tram_ranked(bsp.lump(LUMP_ENTITIES), models, scale);
    let mut ents = collect_entities(
        bsp.lump(LUMP_ENTITIES),
        models,
        nodes,
        planes,
        scale,
        tram_model as usize,
    );
    let n_models = models.len() / SZ_MODEL;
    let mut brush_by_submodel = vec![LOGIC_BRUSH_NONE; n_models];
    for (ei, e) in ents.iter().enumerate() {
        let sm = e.submodel as usize;
        if sm < brush_by_submodel.len() {
            brush_by_submodel[sm] = ei.min(u16::MAX as usize) as u16;
        }
    }
    // titles.txt lives beside the maps dir (valve/titles.txt).
    let titles = std::path::Path::new(path)
        .parent()
        .and_then(|maps| maps.parent())
        .map(parse_titles)
        .unwrap_or_default();
    let map_name = std::path::Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let transition_hints = load_transition_type_hints(map_name);
    let transition_types: std::collections::HashMap<String, u16> = transition_hints
        .iter()
        .map(|prop| (prop.targetname.clone(), prop.ty))
        .collect();
    let mut logic = collect_logic_entities_with_lightstyles(
        bsp.lump(LUMP_ENTITIES),
        models,
        &brush_by_submodel,
        scale,
        &titles,
        &transition_types,
        &dynamic_lightstyles,
    )?;
    let tram_head_raw = if tram_model > 0 {
        model_headnode(models, tram_model as usize, 1).unwrap_or(0)
    } else {
        0
    };
    let mut clip_roots = Vec::with_capacity(3 + ents.len());
    clip_roots.push(hull1_head_raw);
    clip_roots.push(hull3_head_raw); // crouch hull for the world model (fits low vents)
    clip_roots.push(tram_head_raw);
    for e in &ents {
        clip_roots.push(e.head);
    }
    let (clip_remap, clip_out) = compact_clipnode_remap(clipnodes, &clip_roots);
    let n_clip = clip_out.len();
    let stripped_clip_count = raw_n_clip.saturating_sub(n_clip);
    // Preserve the on-disc field for format compatibility, but make accidental
    // legacy use fail open instead of silently tracing an expanded clip hull.
    let hull0_head = -1i32;
    let hull1_head = remap_clip_head(hull1_head_raw, &clip_remap);
    let hull3_head = remap_clip_head(hull3_head_raw, &clip_remap);
    let tram_head = remap_clip_head(tram_head_raw, &clip_remap);
    for e in &mut ents {
        e.head = remap_clip_head(e.head, &clip_remap);
    }

    let mut plane_remap = vec![u16::MAX; n_planes];
    let mut cooked_planes: Vec<usize> = Vec::new();
    for ni in 0..n_nodes {
        let no = ni * SZ_NODE;
        let planenum = i32le(nodes, no).unwrap_or(0).max(0) as usize;
        remap_plane_index(planenum, &mut plane_remap, &mut cooked_planes);
    }
    for &(planenum, _, _) in &clip_out {
        remap_plane_index(planenum, &mut plane_remap, &mut cooked_planes);
    }

    let sound_materials = load_texture_sound_materials(path);
    let mut face_norm = vec![[0i16; 3]; n_faces];
    let mut face_dist = vec![0i32; n_faces];
    let mut face_group = vec![0u16; n_faces];
    let mut plane_groups: Vec<i16> = Vec::new();
    let mut plane_group_lookup: HashMap<i16, u16> = HashMap::new();
    for f in 0..n_faces {
        // Per-face world-space plane (side-adjusted): front-facing iff
        // dot(n,eye) > dist. Lets the runtime backface-cull a whole face
        // before any per-triangle work. Plane groups are cooked once so the PS1
        // can do one backface test for many coplanar faces.
        let fo2 = f * SZ_FACE;
        let planenum = u16le(faces, fo2).unwrap_or(0) as usize;
        let side = u16le(faces, fo2 + 2).unwrap_or(0);
        let (mut n, mut dist) = plane_rec(planes, planenum, scale);
        if side != 0 {
            n = [-n[0], -n[1], -n[2]];
            dist = -dist;
        }
        face_norm[f] = n;
        face_dist[f] = dist;
        let cooked_planenum =
            remap_plane_index(planenum, &mut plane_remap, &mut cooked_planes) as usize;
        let pref = signed_plane_ref(cooked_planenum, side);
        let gid = match plane_group_lookup.get(&pref).copied() {
            Some(id) => id,
            None => {
                let id = plane_groups.len().min(u16::MAX as usize) as u16;
                plane_groups.push(pref);
                plane_group_lookup.insert(pref, id);
                id
            }
        };
        face_group[f] = gid;
    }

    // GoldSrc BSPs can retain a liquid boundary directly coplanar with an
    // opaque terrain face (c2a5's !c2a54/out_mud1 pool is the campaign's
    // clearest example). With no Z buffer, the independently fragmented source
    // faces can choose substantially different OT depths despite sharing an
    // exact plane. Mark those liquid faces for a stable back bias at runtime;
    // the liquid remains visible wherever no opaque face covers it. The affine
    // candidate pass above also preserves source topology for the whole plane.
    let mut face_coplanar_backdrop = vec![false; n_faces];
    for liquid in 0..n_faces {
        if !face_liquid[liquid] || face_ntri[liquid] == 0 {
            continue;
        }
        for opaque in 0..n_faces {
            if face_liquid[opaque]
                || face_ntri[opaque] == 0
                || !face_planes_match_unoriented(
                    face_norm[opaque],
                    face_dist[opaque],
                    face_norm[liquid],
                    face_dist[liquid],
                )
            {
                continue;
            }
            // BSP compilation fragments the two materials independently, so
            // their per-face AABBs need not overlap even when they cover the
            // same authored surface. The exact unoriented plane is the stable
            // ownership invariant; the OT bias only matters where their
            // screen coverage actually meets.
            face_coplanar_backdrop[liquid] = true;
            break;
        }
    }

    let mut face_remap = vec![u16::MAX; n_faces];
    let mut compact_faces: Vec<usize> = Vec::new();
    for f in 0..n_faces {
        if face_ntri[f] != 0 {
            let id = compact_faces.len().min(u16::MAX as usize) as u16;
            face_remap[f] = id;
            compact_faces.push(f);
        }
    }
    let n_cooked_faces = compact_faces.len();
    o[face_count_pos..face_count_pos + 4].copy_from_slice(&(n_cooked_faces as u32).to_le_bytes());

    // Optional host-side provenance for source BSP versus emitted geometry
    // comparisons. Empty/tool faces are removed from the runtime table, so
    // indexing cooked faces by the original BSP face number is incorrect.
    // The report carries that mapping without adding a byte to the PS1 map.
    if let Some(directory) = std::env::var_os("HLPSX_FACE_TOPOLOGY_REPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let stem = std::path::Path::new(path)
            .file_stem()
            .ok_or_else(|| "BSP path has no file stem".to_string())?;
        let mut report = String::from("source_face,cooked_face,first,count,flags\n");
        for &f in &compact_faces {
            report.push_str(&format!(
                "{},{},{},{},{}\n",
                f, face_remap[f], face_lc_first[f], face_lc_count[f], face_lc_flag[f]
            ));
        }
        std::fs::write(directory.join(stem).with_extension("csv"), report)
            .map_err(|e| e.to_string())?;
    }

    let mut compact_marks: Vec<u16> = Vec::new();
    let mut source_face_leaves = vec![Vec::<u16>::new(); n_faces];
    let mut leaf_mark_ranges = vec![(0u16, 0u16); n_leaves];
    // World marks may reference SUBMODEL faces (the compiler leaves them in;
    // GoldSrc filters at draw time). Keep model-0 faces only -- brush entities
    // are drawn by the runtime entity path, so leaving them here rendered a
    // static "ghost" copy under every animated door/plat.
    let world_first = u32le(models, 56).unwrap_or(0) as usize;
    let world_end = world_first + u32le(models, 60).unwrap_or(0) as usize;
    for li in 0..n_leaves {
        let lo = li * SZ_LEAF;
        let m0 = u16le(leaves, lo + SZ_LEAF_MARK0).unwrap_or(0) as usize;
        let mc = u16le(leaves, lo + SZ_LEAF_MARK0 + 2).unwrap_or(0) as usize;
        let start = compact_marks.len().min(u16::MAX as usize) as u16;
        let end = m0.saturating_add(mc).min(src_n_marks);
        for mj in m0..end {
            let src_face = u16le(marks, mj * SZ_MARKSURFACE).unwrap_or(u16::MAX) as usize;
            if src_face < world_first || src_face >= world_end {
                continue;
            }
            if src_face < face_remap.len() {
                let mapped = face_remap[src_face];
                if mapped != u16::MAX {
                    compact_marks.push(mapped);
                    if li <= u16::MAX as usize
                        && !source_face_leaves[src_face].contains(&(li as u16))
                    {
                        source_face_leaves[src_face].push(li as u16);
                    }
                }
            }
        }
        let count = compact_marks
            .len()
            .saturating_sub(start as usize)
            .min(u16::MAX as usize) as u16;
        leaf_mark_ranges[li] = (start, count);
    }
    let n_cooked_marks = compact_marks.len();

    let mut animated_texture = vec![false; n_cooked_texs];
    for chain in &tex_anim_chains {
        for &texture in chain.primary.iter().chain(chain.alt.iter()) {
            if let Some(animated) = animated_texture.get_mut(texture as usize) {
                *animated = true;
            }
        }
    }
    let mut world_source_faces = Vec::new();
    let mut world_native_patches = 0usize;
    let mut world_ineligible_patches = 0usize;
    // Phase B: submodel -> (serialized entity index, entity leaves). Entity
    // faces carry no leaf marks (M24 filters submodels out of them), so their
    // cells take the owning entity's PVS leaves instead.
    let mut submodel_ent: Vec<Option<u16>> = vec![None; n_models_total];
    if phase_b_static_ents {
        for (ei, e) in ents.iter().enumerate() {
            let sm = e.submodel as usize;
            if ei < 254
                && sm < n_models_total
                && submodel_static_eligible[sm]
                && submodel_ent[sm].is_none()
            {
                submodel_ent[sm] = Some(ei as u16);
            }
        }
    }
    let mut static_ent_patches = 0usize;
    // Per-cause ineligibility census (patch counts, first matching cause):
    // translucent, backdrop, cutout-backed, masked, lightstyle, animated,
    // refined, black, no-leaves.
    let mut world_ineligible_causes = [0usize; 9];
    for &f in &compact_faces {
        if face_lc_flag[f] & 0x10 == 0 {
            continue;
        }
        world_native_patches += face_lc_count[f] as usize;
        let texture = face_lc_tex[f] as usize;
        let source_texture = tex_id_for_face(faces, texinfo, f, n_texs);
        let ent = if face_submodel[f] != u16::MAX {
            submodel_ent[face_submodel[f] as usize]
        } else {
            None
        };
        let is_ent_face = ent.is_some();
        if face_submodel[f] != u16::MAX && !is_ent_face {
            // Submodel face without an eligible owner: entity path as before.
            world_ineligible_patches += face_lc_count[f] as usize;
            continue;
        }
        let masked = tex_names
            .get(source_texture)
            .is_some_and(|name| name.starts_with('{'));
        let causes = [
            face_translucent[f],
            face_coplanar_backdrop[f],
            face_cutout_backed[f],
            // Static entities own their masked grates (the measured
            // slow-window population); world masked faces keep the
            // established cutout ordering path.
            masked && !is_ent_face,
            face_dynamic_lightstyles[f].iter().any(|&slot| slot != 0),
            animated_texture.get(texture).copied().unwrap_or(true),
            face_affine_refined[f],
            texs.get(texture)
                .is_some_and(|tex| !tex.pix4.is_empty() && tex.pix4.iter().all(|&byte| byte == 0)),
            !is_ent_face && source_face_leaves[f].is_empty(),
        ];
        if let Some(cause) = causes.iter().position(|&hit| hit) {
            world_ineligible_causes[cause] += face_lc_count[f] as usize;
            world_ineligible_patches += face_lc_count[f] as usize;
            continue;
        }
        let leaves = if let Some(ei) = ent {
            ents[ei as usize].leaves.clone()
        } else {
            source_face_leaves[f].clone()
        };
        if leaves.is_empty() {
            world_ineligible_causes[8] += face_lc_count[f] as usize;
            world_ineligible_patches += face_lc_count[f] as usize;
            continue;
        }
        // Per-face ownership: a mixed entity keeps only its ineligible faces
        // (backed grates, lightstyled panels) on the entity path. The runtime
        // marks owned faces in a load-time bitset for the legacy skip, so no
        // per-entity all-or-nothing rule is needed (it measured 30 owned
        // patches against 538 on t0a0).
        if let Some(ei) = ent {
            static_ent_patches += face_lc_count[f] as usize;
            world_source_faces.push(WorldSourceFace {
                compact_face: face_remap[f],
                texture: face_lc_tex[f],
                group: face_group[f],
                center: face_center[f],
                first_corner: face_lc_first[f] as u16,
                patch_count: face_lc_count[f],
                leaves,
                ent: ei,
            });
            continue;
        }
        world_source_faces.push(WorldSourceFace {
            compact_face: face_remap[f],
            texture: face_lc_tex[f],
            group: face_group[f],
            center: face_center[f],
            first_corner: face_lc_first[f] as u16,
            patch_count: face_lc_count[f],
            leaves,
            ent: u16::MAX,
        });
    }

    let world_pipeline = build_world_pipeline(
        path,
        &verts,
        &loopverts,
        &world_source_faces,
        &source_face_leaves,
        leaves,
        vis,
        n_visleaves,
    )?;
    world_ineligible_patches += world_native_patches
        .saturating_sub(world_ineligible_patches)
        .saturating_sub(world_pipeline.faces.len());
    let duplicated_vertices = world_pipeline
        .stats
        .vertices
        .saturating_sub(world_pipeline.stats.unique_source_vertices);
    eprintln!(
        "  world pipeline census: {} cells, {} exact patches from {} faces, {} cell vertices ({} duplicated), conservative PVS {}/{}/{} cells/verts/packets, neighbor union {} packets, exact PVS {}/{}/{}/{} cells/groups/verts/packets (worst fill {}.{:02}), {} native patches fallback",
        world_pipeline.stats.cells,
        world_pipeline.stats.source_patches,
        world_pipeline.stats.source_faces,
        world_pipeline.stats.vertices,
        duplicated_vertices,
        world_pipeline.stats.max_visible_cells,
        world_pipeline.stats.max_visible_vertices,
        world_pipeline.stats.max_visible_packets,
        world_pipeline.stats.max_neighbor_union_packets,
        world_pipeline.stats.max_exact_cells,
        world_pipeline.stats.max_exact_groups,
        world_pipeline.stats.max_exact_vertices,
        world_pipeline.stats.max_exact_packets,
        world_pipeline.stats.min_exact_fill_x100 / 100,
        world_pipeline.stats.min_exact_fill_x100 % 100,
        world_ineligible_patches,
    );
    if static_ent_patches != 0 {
        eprintln!(
            "  world pipeline static-entity patches owned: {}",
            static_ent_patches
        );
    }
    eprintln!(
        "  world pipeline ineligible causes: translucent={} backdrop={} cutout_backed={} masked={} lightstyle={} animated={} refined={} black={} no_leaves={}",
        world_ineligible_causes[0],
        world_ineligible_causes[1],
        world_ineligible_causes[2],
        world_ineligible_causes[3],
        world_ineligible_causes[4],
        world_ineligible_causes[5],
        world_ineligible_causes[6],
        world_ineligible_causes[7],
        world_ineligible_causes[8],
    );

    o.extend_from_slice(&(cooked_planes.len() as u32).to_le_bytes());
    o.extend_from_slice(&(plane_groups.len() as u32).to_le_bytes());
    o.extend_from_slice(&(n_nodes as u32).to_le_bytes());
    o.extend_from_slice(&packed_leaf_counts.to_le_bytes());
    o.extend_from_slice(&(n_cooked_marks as u32).to_le_bytes());
    o.extend_from_slice(&(vis.len() as u32).to_le_bytes());

    for &pi in &cooked_planes {
        let (n, dist) = plane_rec_q5(planes, pi, scale);
        for c in n {
            o.extend_from_slice(&c.to_le_bytes());
        }
        o.extend_from_slice(&dist.to_le_bytes());
    }

    for pref in &plane_groups {
        o.extend_from_slice(&pref.to_le_bytes());
    }

    if plane_groups.len() > cooked::FACE_GROUP_MASK as usize + 1 {
        return Err(format!(
            "{} face plane groups exceed packed dynamic-light limit {}",
            plane_groups.len(),
            cooked::FACE_GROUP_MASK as usize + 1
        ));
    }

    let face_records_start = o.len();
    for &f in &compact_faces {
        // FaceRec[16B]: first | count | plane_group | center[3] | radius
        //             | tex | flags (bit0: vertex loop; bit1: liquid;
        //                            bit2: raw topology must stay GT3;
        //                            bit3: coplanar liquid goes behind terrain)
        o.extend_from_slice(&(face_lc_first[f] as u16).to_le_bytes());
        o.extend_from_slice(&face_lc_count[f].to_le_bytes());
        // The packed field is now only a cheap "has sparse lightstyles" gate.
        // Exact (possibly multiple) style slots live in the HLMG appendix.
        let lightstyle = u16::from(face_dynamic_lightstyles[f].iter().any(|&slot| slot != 0));
        let material = texture_sound_material(
            &tex_names[tex_id_for_face(faces, texinfo, f, n_texs)],
            &sound_materials,
        );
        // Bits 13..15 were unused: HLMG stores only the boolean "has dynamic
        // lightstyles" gate in bit 12. The material's high bit occupies the
        // last spare face flag (bit 7), so this adds exact impact metadata with
        // no room-RAM or disc-record growth.
        let packed_group = face_group[f]
            | ((lightstyle & 1) << cooked::FACE_GROUP_BITS)
            | (u16::from(material & 7) << 13);
        o.extend_from_slice(&packed_group.to_le_bytes());
        for c in face_center[f] {
            o.extend_from_slice(&c.to_le_bytes());
        }
        // Frustum-cull sphere radius for the face AABB. This stays conservative
        // (covers the AABB corners) but is much tighter than the old
        // ex+ey+ez sum for long/thin faces, so station-scale PVS views reject
        // more off-screen faces before runtime projection.
        let e = face_extent[f];
        let r2 = (e[0] as f32) * (e[0] as f32)
            + (e[1] as f32) * (e[1] as f32)
            + (e[2] as f32) * (e[2] as f32);
        let radius = r2.sqrt().ceil().min(u16::MAX as f32) as u16;
        o.extend_from_slice(&radius.to_le_bytes());
        o.push(face_lc_tex[f]);
        o.push(
            face_lc_flag[f]
                | ((face_translucent[f] as u8) << 1)
                | ((face_coplanar_backdrop[f] as u8) << 3)
                | ((face_cutout_backed[f] as u8) << 6)
                | ((material & 8) << 4),
        );
    }
    debug_assert_eq!(
        o.len() - face_records_start,
        compact_faces.len() * cooked::FACE_RECORD_SIZE
    );

    let node_records_start = o.len();
    for ni in 0..n_nodes {
        let no = ni * SZ_NODE;
        let src_planenum = i32le(nodes, no).unwrap_or(0).max(0) as usize;
        let planenum = plane_remap.get(src_planenum).copied().unwrap_or(0);
        o.extend_from_slice(&planenum.to_le_bytes());
        o.extend_from_slice(&i16::from_le_bytes([nodes[no + 4], nodes[no + 5]]).to_le_bytes());
        o.extend_from_slice(&i16::from_le_bytes([nodes[no + 6], nodes[no + 7]]).to_le_bytes());
    }
    debug_assert_eq!(
        o.len() - node_records_start,
        n_nodes * cooked::NODE_RECORD_SIZE
    );

    let leaf_records_start = o.len();
    for (li, &(mark_start, mark_count)) in leaf_mark_ranges.iter().enumerate() {
        let lo = li * SZ_LEAF;
        o.extend_from_slice(
            &i32le(leaves, lo + SZ_LEAF_VISOFS)
                .unwrap_or(-1)
                .to_le_bytes(),
        );
        o.extend_from_slice(&mark_start.to_le_bytes());
        let packed_mark_count = pack_leaf_mark_count(mark_count, i32le(leaves, lo).unwrap_or(-2))
            .map_err(|e| format!("{}: leaf {}: {}", path, li, e))?;
        o.extend_from_slice(&packed_mark_count.to_le_bytes());
    }
    debug_assert_eq!(
        o.len() - leaf_records_start,
        leaf_mark_ranges.len() * cooked::LEAF_RECORD_SIZE
    );
    while o.len() % 4 != 0 {
        o.push(0);
    }

    for mark in &compact_marks {
        o.extend_from_slice(&mark.to_le_bytes());
    }
    while o.len() % 4 != 0 {
        o.push(0);
    }
    o.extend_from_slice(vis);
    while o.len() % 4 != 0 {
        o.push(0);
    }

    // ---- Clip hull (player collision / LOS) + spawn ----
    // u32 n_clip | i32 legacy_hull0_head (-1; point hull uses render nodes) |
    // i32 hull1_head | i32 hull3_head (crouch) |
    // i32 spawn x,y,z (world) | i32 spawn_yaw (Q0.12)
    // clipnodes (u16 plane_ref, i16 c0, i16 c1) × n_clip [6B]
    // plane_ref: bits 13..0 remapped plane index; bits 15..14 are
    // 00=generic, 01=exact +X, 10=exact +Y, 11=exact +Z.
    let clip_off = o.len() as u32;
    o[clip_off_pos..clip_off_pos + 4].copy_from_slice(&clip_off.to_le_bytes());
    let (sp, syaw) = choose_standalone_spawn(
        bsp.lump(LUMP_ENTITIES),
        scale,
        nodes,
        planes,
        leaves,
        n_visleaves,
        clipnodes,
        hull1_head_raw,
        marks,
        vis,
        // Spawn visibility is a property of the authored face, not its cooked
        // tessellation density. Using the post-refine triangle count made the
        // same BSP choose a different standalone spawn when subdivision knobs
        // changed.
        &face_fan_ntri,
        &face_norm,
        &face_dist,
        &face_center,
        &face_extent,
        &face_bright,
    )
    .or_else(|| {
        find_spawn(bsp.lump(LUMP_ENTITIES))
            .map(|(origin, yaw_deg)| (origin, hl_yaw_to_world_q12(yaw_deg)))
    })
    .unwrap_or_else(|| {
        // Mid-chapter maps (changelevel targets) have no info_player_start; you
        // arrive via an info_landmark. Fall back to the world bbox center near
        // the top so gravity drops the player onto the floor, not into the void.
        // ponytail: bbox-center heuristic; a center that lands in solid will need
        // a smarter pick (nearest empty leaf) -- revisit if a map spawns stuck.
        let mn = |i| f32le(models, i).unwrap_or(0.0);
        let (mins, maxs) = ([mn(0), mn(4), mn(8)], [mn(12), mn(16), mn(20)]);
        (
            [
                (mins[0] + maxs[0]) * 0.5,
                (mins[1] + maxs[1]) * 0.5,
                maxs[2] - 32.0,
            ],
            0,
        )
    });
    // World space: swap Y/Z, scale.
    let spawn = [
        (sp[0] / scale).round() as i32,
        (sp[2] / scale).round() as i32,
        (sp[1] / scale).round() as i32,
    ];

    o.extend_from_slice(&(n_clip as u32).to_le_bytes());
    o.extend_from_slice(&hull0_head.to_le_bytes());
    o.extend_from_slice(&hull1_head.to_le_bytes());
    o.extend_from_slice(&hull3_head.to_le_bytes());
    for c in &spawn {
        o.extend_from_slice(&c.to_le_bytes());
    }
    o.extend_from_slice(&syaw.to_le_bytes());
    // Children in clip_out are already final DAG ids -- write them verbatim.
    for (clip_idx, &(src_planenum, c0, c1)) in clip_out.iter().enumerate() {
        let planenum = plane_remap
            .get(src_planenum)
            .copied()
            .filter(|&idx| idx != u16::MAX)
            .ok_or_else(|| {
                format!(
                    "{}: clipnode {} source plane {} was not remapped",
                    path, clip_idx, src_planenum
                )
            })? as usize;
        let (normal, _) = plane_rec(planes, src_planenum, scale);
        let plane_ref = pack_clip_plane_ref(planenum, normal)
            .map_err(|e| format!("{}: clipnode {}: {}", path, clip_idx, e))?;
        o.extend_from_slice(&plane_ref.to_le_bytes());
        o.extend_from_slice(&c0.to_le_bytes());
        o.extend_from_slice(&c1.to_le_bytes());
    }

    // ---- Entities (brush models) ----
    // u32 n_models | (u32 firstface, u32 numface) × n_models
    // u32 n_ents   | EntRec[52B] × n_ents | u32 n_ent_leafs | u16 leaf_idx[]
    let ent_off = o.len() as u32;
    o[ent_off_pos..ent_off_pos + 4].copy_from_slice(&ent_off.to_le_bytes());
    o.extend_from_slice(&(n_models as u32).to_le_bytes());
    for mi in 0..n_models {
        let mo = mi * SZ_MODEL;
        let first_src = i32le(models, mo + 56).unwrap_or(0).max(0) as usize;
        let count_src = i32le(models, mo + 60).unwrap_or(0).max(0) as usize;
        let mut first = u32::MAX;
        let mut count = 0u32;
        for f in first_src..first_src.saturating_add(count_src).min(face_remap.len()) {
            let mapped = face_remap[f];
            if mapped != u16::MAX {
                if first == u32::MAX {
                    first = mapped as u32;
                }
                count += 1;
            }
        }
        o.extend_from_slice(&first.min(u16::MAX as u32).to_le_bytes());
        o.extend_from_slice(&count.to_le_bytes());
    }
    o.extend_from_slice(&(ents.len() as u32).to_le_bytes());
    let mut ent_leafs: Vec<u16> = Vec::new();
    let entity_records_start = o.len();
    for e in &ents {
        let leaf_start = ent_leafs.len().min(u16::MAX as usize) as u16;
        let room = (u16::MAX as usize).saturating_sub(leaf_start as usize);
        let leaf_count = e.leaves.len().min(room).min(u16::MAX as usize) as u16;
        ent_leafs.extend_from_slice(&e.leaves[..leaf_count as usize]);
        o.extend_from_slice(&e.submodel.to_le_bytes());
        o.extend_from_slice(&e.kind.to_le_bytes());
        for c in e.origin {
            o.extend_from_slice(&c.to_le_bytes());
        }
        for c in e.mv {
            o.extend_from_slice(&c.to_le_bytes());
        }
        for c in e.center {
            o.extend_from_slice(&c.to_le_bytes());
        }
        o.extend_from_slice(&e.r2.to_le_bytes());
        o.extend_from_slice(&e.head.to_le_bytes());
        o.extend_from_slice(&e.head0.to_le_bytes());
        o.extend_from_slice(&leaf_start.to_le_bytes());
        o.extend_from_slice(&leaf_count.to_le_bytes());
    }
    debug_assert_eq!(
        o.len() - entity_records_start,
        ents.len() * cooked::ENTITY_RECORD_SIZE
    );
    o.extend_from_slice(&(ent_leafs.len() as u32).to_le_bytes());
    for leaf in &ent_leafs {
        o.extend_from_slice(&leaf.to_le_bytes());
    }
    while o.len() % 4 != 0 {
        o.push(0);
    }

    // ---- Tram (func_tracktrain ride) ----
    // u16 submodel | u16 n_way | u32 (start_index<<16 | speed_u16) |
    // i32 clip_head | i32 base[3] | waypoints i32[3] × n_way (world).
    // Legacy rooms stored an ordinary positive i32 speed, whose high half is
    // zero and therefore decodes as authored start index 0.
    let tram_off = o.len() as u32;
    o[tram_off_pos..tram_off_pos + 4].copy_from_slice(&tram_off.to_le_bytes());
    // The tram brush verts are stored relative to the entity origin (bbox near
    // 0); HL renders them at verts + pev->origin, which the path drives. So the
    // render/collision offset is the full path position = authored waypoint +
    // ride_off. Predecessor nodes exist only for transferred-train reattachment.
    let tram_start = (tram_start as usize).min(way.len().saturating_sub(1));
    let tram_base = way.get(tram_start).map(|w| w.0).unwrap_or([0, 0, 0]);
    let tram_motion = pack_tram_motion(tram_speed, tram_start, tram_wheels);
    o.extend_from_slice(&tram_model.to_le_bytes());
    o.extend_from_slice(&(way.len() as u16).to_le_bytes());
    o.extend_from_slice(&tram_motion.to_le_bytes());
    o.extend_from_slice(&tram_head.to_le_bytes());
    for c in &tram_base {
        o.extend_from_slice(&c.to_le_bytes());
    }
    for (w, _, _) in &way {
        for c in w {
            o.extend_from_slice(&c.to_le_bytes());
        }
    }
    // Per-waypoint speed changes (path_track "speed", u/s; 0 = keep) so the
    // ride paces itself as authored instead of one constant.
    for (_, spd, _) in &way {
        o.extend_from_slice(&spd.to_le_bytes());
    }
    // Per-waypoint fire-on-pass (path_track "message") as logic-name ids
    // (0 = none). Only already-interned names resolve -- a message that
    // targets nothing cooked fires nothing, same as HL.
    for (_, _, msg) in &way {
        let id = if msg.is_empty() {
            0u16
        } else {
            logic
                .names
                .iter()
                .position(|n| n == msg)
                .map(|p| (p + 1).min(u16::MAX as usize) as u16)
                .unwrap_or(0)
        };
        o.extend_from_slice(&id.to_le_bytes());
    }

    // ---- Actors/items + independent sprite placements ----
    // u32 split counts | ActorRec[24B] | SpriteRec[12B].
    let prop_off = o.len() as u32;
    o[prop_off_pos..prop_off_pos + 4].copy_from_slice(&prop_off.to_le_bytes());
    let mut props = collect_props(
        bsp.lump(LUMP_ENTITIES),
        nodes,
        planes,
        models,
        scale,
        &mut logic.names,
    );
    let corpse_clips = load_clips_manifest();
    for prop in &mut props {
        if prop.0 & 0x8fff == 0x8000 && prop.5 & cooked::PROP_CORPSE_CLIP != 0 {
            let label =
                cooked::SCIENTIST_CORPSE_POSES[(prop.5 & !cooked::PROP_CORPSE_CLIP) as usize];
            let clip = corpse_clips
                .get(&(0, label.to_owned()))
                .ok_or_else(|| format!("{path}: missing corpse clip {label}; recook models"))?;
            prop.5 = cooked::PROP_CORPSE_CLIP | clip.slot as u16;
        }
        if prop.0 & 0x0fff == 25 {
            let raw = [
                prop.1[0] as f32 / scale,
                prop.1[2] as f32 / scale,
                prop.1[1] as f32 / scale,
            ];
            let grounded = seated_placement::origin(&bsp, raw);
            prop.1 = to_world(grounded, scale);
            prop.3 = point_leaf(grounded, nodes, planes);
        }
    }
    // Mid-map chapter selection has no source room from which to fill the
    // carry mailbox. Add one dormant-able template for every script dependency
    // supplied only by an incoming transition. Its transformed source origin
    // is the same position a default-state actor would have after GoldSrc's
    // landmark translation. Runtime activates it only for a fresh menu launch.
    for hint in &transition_hints {
        let origin = to_world(hint.origin, scale);
        let orientation = pack_prop_orientation([0.0, hint.yaw, 0.0], hint.body);
        let leaf = point_leaf(hint.origin, nodes, planes);
        let name = intern_logic_name(&mut logic.names, &hint.targetname)?;
        if name & PROP_NAME_INCOMING_ONLY != 0 {
            return Err(format!("{}: transition fallback name table overflow", path));
        }
        let owner_index = props.len();
        props.push((
            transition_fallback_type(hint),
            origin,
            orientation,
            leaf,
            name | PROP_NAME_INCOMING_ONLY,
            actor_carry_id(&hint.targetname, false),
        ));
        append_monster_loot_slots(
            &mut props,
            owner_index,
            origin,
            orientation,
            leaf,
            if hint.ty == 1 { [27, 0] } else { [0; 2] },
        );
    }
    // Sprite billboards have a separate compact capacity: dense Xen maps no
    // longer evict actors merely because both happened to share MAX_PROPS.
    let sprite_map_idx: u16 = std::env::var("MAP_INDEX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let sprites_manifest = load_sprites_manifest();
    let sprite_props = collect_sprite_props(
        bsp.lump(LUMP_ENTITIES),
        nodes,
        planes,
        scale,
        sprite_map_idx,
        &sprites_manifest,
        &mut logic.names,
    )?;
    if props.len() > MAX_RUNTIME_LIVE_PROPS {
        return Err(format!(
            "{}: {} authored actors exceed the runtime live-prop cap {}",
            path,
            props.len(),
            MAX_RUNTIME_LIVE_PROPS
        ));
    }
    if props.len() > u16::MAX as usize || sprite_props.len() > 0x7FFF {
        return Err(format!(
            "{}: prop section overflow ({} actors, {} sprites)",
            path,
            props.len(),
            sprite_props.len()
        ));
    }
    let prop_counts =
        cooked::PROP_SPLIT_FORMAT | ((sprite_props.len() as u32) << 16) | props.len() as u32;
    o.extend_from_slice(&prop_counts.to_le_bytes());
    // PropRec 24B: ty u16 | leaf i16 | org i32[3] | orientation i32 | name u16 | carry u16
    // (name = local targetname id; carry = stable target/global identity).
    let prop_records_start = o.len();
    for (ty, org, yaw, leaf, name, carry) in &props {
        o.extend_from_slice(&ty.to_le_bytes());
        o.extend_from_slice(&leaf.to_le_bytes());
        for c in org {
            o.extend_from_slice(&c.to_le_bytes());
        }
        o.extend_from_slice(&yaw.to_le_bytes());
        o.extend_from_slice(&name.to_le_bytes());
        o.extend_from_slice(&carry.to_le_bytes());
    }
    debug_assert_eq!(
        o.len() - prop_records_start,
        props.len() * cooked::PROP_RECORD_SIZE
    );
    // SpriteRec 12B: origin i16[3] | leaf i16 | targetname u16 | packed u16.
    let sprite_records_start = o.len();
    for (org, leaf, name, packed) in &sprite_props {
        for c in org {
            o.extend_from_slice(&c.to_le_bytes());
        }
        o.extend_from_slice(&leaf.to_le_bytes());
        o.extend_from_slice(&name.to_le_bytes());
        o.extend_from_slice(&packed.to_le_bytes());
    }
    debug_assert_eq!(
        o.len() - sprite_records_start,
        sprite_props.len() * cooked::SPRITE_RECORD_SIZE
    );
    while o.len() % 4 != 0 {
        o.push(0);
    }

    // ---- AI navigation graph ----
    // Exact retail mode:
    //   u16 n_nav | u16 0x8000|route_bytes |
    //   NavNode[18B] × n_nav: i32 origin[3], i16 leaf, u16 route_offset,
    //     u8 node_type, u8 pad | GoldSrc compressed route bytes.
    // Custom-map fallback retains the legacy synthesized adjacency layout:
    //   u16 n_nav | u16 n_links | NavNode(first_link,link_count) | u16 dest[].
    // Both keep the packed 18-byte node record; exact mode replaces runtime
    // BFS with the shipped graph's deterministic NextNodeInRoute stream.
    let nav_off = o.len() as u32;
    o[nav_off_pos..nav_off_pos + 4].copy_from_slice(&nav_off.to_le_bytes());
    let nav = collect_nav(
        path,
        bsp.lump(LUMP_ENTITIES),
        nodes,
        planes,
        clipnodes,
        hull1_head_raw,
        scale,
    );
    o.extend_from_slice(&(nav.nodes.len().min(u16::MAX as usize) as u16).to_le_bytes());
    let nav_meta_pos = o.len();
    o.extend_from_slice(&0u16.to_le_bytes());
    let nav_nodes_start = o.len();
    let nav_payload_count = if let Some(routes) = nav.routes.as_ref() {
        let meta = NAV_EXACT_ROUTES | routes.len() as u16;
        o[nav_meta_pos..nav_meta_pos + 2].copy_from_slice(&meta.to_le_bytes());
        for node in &nav.nodes {
            for c in node.origin {
                o.extend_from_slice(&c.to_le_bytes());
            }
            o.extend_from_slice(&node.leaf.to_le_bytes());
            o.extend_from_slice(&node.route_offset.to_le_bytes());
            o.push(node.node_type);
            o.push(0);
        }
        debug_assert_eq!(
            o.len() - nav_nodes_start,
            nav.nodes.len() * COOKED_NAV_NODE_BYTES
        );
        o.extend_from_slice(routes);
        routes.len()
    } else {
        let mut nav_links: Vec<u16> = Vec::new();
        for node in &nav.nodes {
            let first = nav_links.len().min(u16::MAX as usize) as u16;
            let room = (u16::MAX as usize).saturating_sub(first as usize);
            let count = node.links.len().min(room).min(u8::MAX as usize) as u8;
            nav_links.extend(node.links[..count as usize].iter().map(|&v| v as u16));
            for c in node.origin {
                o.extend_from_slice(&c.to_le_bytes());
            }
            o.extend_from_slice(&node.leaf.to_le_bytes());
            o.extend_from_slice(&first.to_le_bytes());
            o.push(count);
            o.push(0);
        }
        debug_assert_eq!(
            o.len() - nav_nodes_start,
            nav.nodes.len() * COOKED_NAV_NODE_BYTES
        );
        let count = nav_links.len().min((NAV_EXACT_ROUTES - 1) as usize) as u16;
        o[nav_meta_pos..nav_meta_pos + 2].copy_from_slice(&count.to_le_bytes());
        for link in nav_links.iter().take(count as usize) {
            o.extend_from_slice(&link.to_le_bytes());
        }
        count as usize
    };
    while o.len() % 4 != 0 {
        o.push(0);
    }

    // ---- Half-Life target/use/touch logic graph ----
    // u16 n_logic,n_aux,n_names,name_bytes |
    // LogicRec[64B] × n_logic | LogicAux[4B] × n_aux |
    // u16 name_offsets[n_names] | nul-terminated names
    if logic.ents.len() > u16::MAX as usize || logic.aux.len() > u16::MAX as usize {
        return Err(format!(
            "{}: cooked logic overflow ({} ents, {} aux)",
            path,
            logic.ents.len(),
            logic.aux.len()
        ));
    }
    let mut name_blob = Vec::new();
    let mut name_offsets = Vec::new();
    for name in &logic.names {
        if name_offsets.len() >= u16::MAX as usize {
            return Err(format!("{}: too many logic names", path));
        }
        if name_blob.len() > u16::MAX as usize {
            return Err(format!("{}: logic name blob too large", path));
        }
        name_offsets.push(name_blob.len() as u16);
        name_blob.extend_from_slice(name.as_bytes());
        name_blob.push(0);
    }
    if name_blob.len() > u16::MAX as usize {
        return Err(format!("{}: logic name blob too large", path));
    }
    let logic_off = o.len() as u32;
    o[logic_off_pos..logic_off_pos + 4].copy_from_slice(&logic_off.to_le_bytes());
    o.extend_from_slice(&(logic.ents.len() as u16).to_le_bytes());
    o.extend_from_slice(&(logic.aux.len() as u16).to_le_bytes());
    o.extend_from_slice(&(logic.names.len() as u16).to_le_bytes());
    o.extend_from_slice(&(name_blob.len() as u16).to_le_bytes());
    let logic_records_start = o.len();
    for rec in &logic.ents {
        o.push(rec.kind);
        o.push(rec.use_type);
        o.extend_from_slice(&rec.spawnflags.to_le_bytes());
        o.extend_from_slice(&rec.targetname.to_le_bytes());
        o.extend_from_slice(&rec.target.to_le_bytes());
        o.extend_from_slice(&rec.killtarget.to_le_bytes());
        o.extend_from_slice(&rec.brush.to_le_bytes());
        o.extend_from_slice(&rec.first_aux.to_le_bytes());
        o.push(rec.aux_count);
        o.push(rec.flags);
        o.extend_from_slice(&rec.wait_ticks.to_le_bytes());
        o.extend_from_slice(&rec.delay_ticks.to_le_bytes());
        o.extend_from_slice(&rec.speed.to_le_bytes());
        o.extend_from_slice(&rec.arg0.to_le_bytes());
        o.extend_from_slice(&rec.arg1.to_le_bytes());
        o.push(rec.sound0);
        o.push(rec.sound1);
        for c in rec.origin {
            o.extend_from_slice(&c.to_le_bytes());
        }
        for c in rec.mins {
            o.extend_from_slice(&c.to_le_bytes());
        }
        for c in rec.maxs {
            o.extend_from_slice(&c.to_le_bytes());
        }
    }
    debug_assert_eq!(
        o.len() - logic_records_start,
        logic.ents.len() * cooked::LOGIC_RECORD_SIZE
    );
    for rec in &logic.aux {
        o.extend_from_slice(&rec.target.to_le_bytes());
        o.extend_from_slice(&rec.delay_ticks.to_le_bytes());
    }
    for off in &name_offsets {
        o.extend_from_slice(&off.to_le_bytes());
    }
    o.extend_from_slice(&name_blob);
    while o.len() % 4 != 0 {
        o.push(0);
    }

    // ---- Texture-animation chains (HLMF) ----
    // u16 n_chains | u16 reserved | per chain: u8 n_primary | u8 n_alt |
    // u8 primary_ids[] | u8 alt_ids[] (compact texture ids). The runtime
    // derives this section's offset as align4(logic names end).
    if tex_anim_chains.len() > u16::MAX as usize {
        return Err(format!(
            "{}: {} texture animation chains overflow u16",
            path,
            tex_anim_chains.len()
        ));
    }
    o.extend_from_slice(&(tex_anim_chains.len() as u16).to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    for chain in &tex_anim_chains {
        o.push(chain.primary.len() as u8);
        o.push(chain.alt.len() as u8);
        for &id in chain.primary.iter().chain(chain.alt.iter()) {
            // Compact ids are < the 256-texture cap checked above; anything
            // else here is cook corruption the one-byte record would mask.
            assert!(id < 256, "animation chain frame id {id} exceeds u8");
            o.push(id as u8);
        }
    }
    while o.len() % 4 != 0 {
        o.push(0);
    }

    // Source-addressed static-world data lives after all legacy-derived
    // sections. Its explicit header pointer makes the placement independent of
    // texture-animation parsing while keeping the established renderer as the
    // only selected path during the census phase.
    let emit_world_pipeline = std::env::var_os("WORLD_PIPELINE_EMIT").is_some();
    let requested_cell_limit = std::env::var("WORLD_PIPELINE_CELL_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    let serialized_cells = if emit_world_pipeline {
        world_pipeline.cells.len().min(requested_cell_limit)
    } else {
        0
    };
    let serialized_faces = world_pipeline
        .cells
        .get(serialized_cells.wrapping_sub(1))
        .map_or(0, |cell| {
            cell.face_first as usize + cell.face_count as usize
        });
    let serialized_vertices = world_pipeline
        .cells
        .get(serialized_cells.wrapping_sub(1))
        .map_or(0, |cell| {
            cell.vertex_first as usize + cell.vertex_count as usize
        });
    // Mark only whole legacy faces whose every patch exists in this exact
    // serialized prefix. Partial faces (including three-corner or seamed
    // records) remain indivisibly owned by the established fallback. The
    // runtime masks this high bit from the count even before cell selection,
    // so diagnostic sections remain pixel-neutral.
    let mut owned_patch_counts = vec![0u16; n_cooked_faces];
    for command in world_pipeline.faces.iter().take(serialized_faces) {
        if let Some(count) = owned_patch_counts.get_mut(command.source_face as usize) {
            *count = count.saturating_add(1);
        }
    }
    for (compact_face, &owned_count) in owned_patch_counts.iter().enumerate() {
        if owned_count == 0 {
            continue;
        }
        let source_face = compact_faces[compact_face];
        if owned_count != face_lc_count[source_face] {
            continue;
        }
        let count_off = face_records_start + compact_face * cooked::FACE_RECORD_SIZE + 2;
        let count = u16::from_le_bytes([o[count_off], o[count_off + 1]]);
        debug_assert_eq!(count & !cooked::FACE_COUNT_MASK, 0);
        o[count_off..count_off + 2]
            .copy_from_slice(&(count | cooked::FACE_WORLD_PIPELINE_OWNED).to_le_bytes());
    }
    // Build the reverse incidence required by the runtime selector. PVS
    // rebuild already walks visible leaves, so each leaf owns one contiguous
    // list of candidate cell ids. This replaces the Phase-A cell->leaf array
    // whose only possible runtime consumer had to rescan every cell.
    let mut leaf_cells = if serialized_cells == 0 {
        Vec::new()
    } else {
        vec![Vec::<u16>::new(); n_visleaves + 1]
    };
    for (cell_index, owners) in world_pipeline
        .cell_leaves
        .iter()
        .take(serialized_cells)
        .enumerate()
    {
        for &leaf in owners {
            if let Some(cells) = leaf_cells.get_mut(leaf as usize) {
                cells.push(cell_index as u16);
            }
        }
    }
    let serialized_leaf_cells = leaf_cells.iter().map(Vec::len).sum::<usize>();
    if leaf_cells.len() > u16::MAX as usize || serialized_leaf_cells > u16::MAX as usize {
        return Err(format!(
            "{}: world pipeline leaf incidence exceeds u16 ({} ranges, {} cell refs)",
            path,
            leaf_cells.len(),
            serialized_leaf_cells
        ));
    }
    let world_pipeline_off = o.len() as u32;
    o[world_pipeline_off_pos..world_pipeline_off_pos + 4]
        .copy_from_slice(&world_pipeline_off.to_le_bytes());
    let cells_off = cooked::WORLD_PIPELINE_HEADER_SIZE;
    let leaf_ranges_off = cells_off + serialized_cells * cooked::WORLD_PIPELINE_CELL_SIZE;
    let leaf_cells_off =
        leaf_ranges_off + leaf_cells.len() * cooked::WORLD_PIPELINE_LEAF_RANGE_SIZE;
    let vertices_off =
        (leaf_cells_off + serialized_leaf_cells * cooked::WORLD_PIPELINE_LEAF_CELL_SIZE + 3) & !3;
    let faces_off = vertices_off + serialized_vertices * cooked::WORLD_PIPELINE_VERTEX_SIZE;
    let templates_off = faces_off + serialized_faces * CookedDrawSurfaceCommand::SIZE;
    o.extend_from_slice(&cooked::WORLD_PIPELINE_MAGIC.to_le_bytes());
    o.extend_from_slice(&cooked::WORLD_PIPELINE_VERSION.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes()); // no cell has ordering proof yet
    o.extend_from_slice(&(serialized_cells as u16).to_le_bytes());
    o.extend_from_slice(&(serialized_vertices as u16).to_le_bytes());
    o.extend_from_slice(&(serialized_faces as u16).to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&(leaf_cells.len() as u16).to_le_bytes());
    o.extend_from_slice(&(serialized_leaf_cells as u16).to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    for offset in [
        cells_off,
        leaf_ranges_off,
        leaf_cells_off,
        vertices_off,
        faces_off,
        templates_off,
    ] {
        o.extend_from_slice(&(offset as u32).to_le_bytes());
    }
    debug_assert_eq!(
        o.len(),
        world_pipeline_off as usize + cooked::WORLD_PIPELINE_HEADER_SIZE
    );
    for cell in world_pipeline.cells.iter().take(serialized_cells) {
        o.extend_from_slice(&cell.vertex_first.to_le_bytes());
        o.extend_from_slice(&cell.vertex_count.to_le_bytes());
        o.extend_from_slice(&cell.face_first.to_le_bytes());
        o.extend_from_slice(&cell.face_count.to_le_bytes());
        o.extend_from_slice(&cell.packet_count.to_le_bytes());
        o.extend_from_slice(&cell.flags.to_le_bytes());
        for coordinate in cell.center {
            o.extend_from_slice(&coordinate.to_le_bytes());
        }
        o.extend_from_slice(&cell.radius.to_le_bytes());
    }
    debug_assert_eq!(o.len(), world_pipeline_off as usize + leaf_ranges_off);
    let mut leaf_cell_first = 0usize;
    for cells in &leaf_cells {
        o.extend_from_slice(&(leaf_cell_first as u16).to_le_bytes());
        o.extend_from_slice(&(cells.len() as u16).to_le_bytes());
        leaf_cell_first += cells.len();
    }
    debug_assert_eq!(o.len(), world_pipeline_off as usize + leaf_cells_off);
    for cells in &leaf_cells {
        for cell in cells {
            o.extend_from_slice(&cell.to_le_bytes());
        }
    }
    while o.len() < world_pipeline_off as usize + vertices_off {
        o.push(0);
    }
    debug_assert_eq!(o.len(), world_pipeline_off as usize + vertices_off);
    for vertex in world_pipeline.vertices.iter().take(serialized_vertices) {
        for coordinate in vertex.position {
            o.extend_from_slice(&coordinate.to_le_bytes());
        }
        o.extend_from_slice(&vertex.source_vertex.to_le_bytes());
    }
    debug_assert_eq!(o.len(), world_pipeline_off as usize + faces_off);
    for face in world_pipeline.faces.iter().take(serialized_faces) {
        o.extend_from_slice(&face.encode());
    }
    debug_assert_eq!(o.len(), world_pipeline_off as usize + templates_off);
    debug_assert_eq!(o.len(), world_pipeline_off as usize + templates_off);
    if !emit_world_pipeline {
        eprintln!(
            "  world pipeline section: census-only (set WORLD_PIPELINE_EMIT=1 for a diagnostic room)"
        );
    } else if serialized_cells != world_pipeline.cells.len() {
        eprintln!(
            "  world pipeline section: diagnostic prefix {}/{} cells",
            serialized_cells,
            world_pipeline.cells.len()
        );
    }
    if !tex_anim_chains.is_empty() {
        eprintln!(
            "  texture animation: {} chains, {} frames",
            tex_anim_chains.len(),
            tex_anim_chains
                .iter()
                .map(|c| c.primary.len() + c.alt.len())
                .sum::<usize>()
        );
    }

    std::fs::write(out, &o).map_err(|e| format!("write {}: {}", out, e))?;
    let tex_kb = if let (Some(tex_out), Some(texture_chunk)) = (tex_out, texture_chunk.as_ref()) {
        std::fs::write(tex_out, texture_chunk).map_err(|e| format!("write {}: {}", tex_out, e))?;
        Some(texture_chunk.len() / 1024)
    } else {
        None
    };
    if let Some(report_path) = std::env::var_os("SUBDIVISION_REPORT") {
        let map_name = std::env::var("MAP_NAME").unwrap_or_else(|_| {
            Path::new(path)
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        let map_index = std::env::var("MAP_INDEX").unwrap_or_default();
        let candidate_faces = face_affine_candidate.iter().filter(|&&v| v).count();
        let liquid_plane_count = liquid_planes.iter().filter(|&&v| v).count();
        let coplanar_liquid_faces = face_coplanar_backdrop.iter().filter(|&&v| v).count();
        let texture_bytes = texture_chunk.as_ref().map_or(0, Vec::len);
        let native_patch_faces = face_native_patch.iter().filter(|&&v| v).count();
        let mut report = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&report_path)
            .map_err(|e| format!("open subdivision report {:?}: {}", report_path, e))?;
        writeln!(
            report,
            "{map_index},{map_name},{orig_n_verts},{fan_tris},{affine_base_verts},{affine_base_tris},{affine_added_verts},{affine_added_tris},{n_verts},{n_tris},{candidate_faces},{affine_refined_faces},{grid_cells},{grid_quad_cells},{grid_boundary_tris},{grid_budget_fallbacks},{liquid_plane_count},{coplanar_liquid_faces},{tj_added},{},{texture_bytes},{native_patch_faces},{native_patch_quad_records},{native_patch_triangle_records},{native_patch_blocked_records},{native_patch_seamed_pair_records},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            o.len(),
            world_pipeline.stats.cells,
            world_pipeline.stats.source_faces,
            world_pipeline.stats.source_patches,
            world_pipeline.stats.vertices,
            world_pipeline.stats.unique_source_vertices,
            world_pipeline.stats.max_visible_cells,
            world_pipeline.stats.max_visible_vertices,
            world_pipeline.stats.max_visible_packets,
            world_pipeline.stats.max_neighbor_union_packets,
            world_pipeline.stats.max_exact_cells,
            world_pipeline.stats.max_exact_vertices,
            world_pipeline.stats.max_exact_packets,
            world_pipeline.stats.min_exact_fill_x100,
            world_ineligible_patches,
        )
        .map_err(|e| format!("write subdivision report {:?}: {}", report_path, e))?;
    }
    println!(
        "cooked {} -> {}{}  ({} verts, {} tris, {} faces, {} leaves/{} vis, {} clipnodes kept/{} stripped from {}, {} ents, tram {} waypts, {} actors/{} sprites, {} nav nodes/{} route-or-link bytes, {} logic/{} aux/{} names, {} texs kept/{} stripped from {}, spawn [{},{},{}], {} KB resident{})",
        path,
        out,
        tex_out.map(|p| format!(" + {}", p)).unwrap_or_default(),
        n_verts,
        n_tris,
        n_faces,
        n_leaves,
        n_visleaves,
        n_clip,
        stripped_clip_count,
        raw_n_clip,
        ents.len(),
        way.len(),
        props.len(),
        sprite_props.len(),
        nav.nodes.len(),
        nav_payload_count,
        logic.ents.len(),
        logic.aux.len(),
        logic.names.len(),
        n_cooked_texs,
        stripped_tex_count,
        original_tex_count,
        spawn[0],
        spawn[1],
        spawn[2],
        o.len() / 1024,
        tex_kb
            .map(|kb| format!(", {} KB textures", kb))
            .unwrap_or_default()
    );
    Ok(())
}

// ---- MDL (Half-Life studio model) -> .hlmdl ------------------------------
//
// HMD8 is the sole animated-model format. GoldSrc assigns exactly one bone to
// each vertex, so storing every posed vertex is unnecessary: retain one
// bone-local vertex set and bake the already-composed model-space affine for
// every used bone/pose. Runtime interpolation remains visually equivalent
// because lerp(M0*v, M1*v) == lerp(M0, M1)*v, while matrix interpolation moves
// from every vertex to every bone.
//
//   "HMD8" | u32 n_verts,n_tris,n_texs,n_frames,n_clips,data_len
//          | u16 local_to_world_q12,flags,n_bones,n_ranges
//   ClipRec[4] * n_clips
//   u8 source_phase * n_frames
//   optional u8 alignment pad
//   BoneRange[8] * n_ranges
//     u16 first,count,bone | u8 body_mask,flags (bit 0 = mouth subtree)
//   bone-local vertices: i16 xyz * n_verts
//   pose affines: (packed signed Q11 3x3 + i16 xyz translation) * n_frames*n_bones
//   optional mouth-relative affine[24] * n_frames
//   TriRec[16 or 20] * n_tris | textures...
//
// Header flags:
//   bit 0: range/triangle body masks are meaningful
//   bit 1: mouth-relative affines are present
//   bit 2: TriRec[16] pad stores a signed 5:5:5 normal (viewmodels)
//   bit 3: TriRec is 20 bytes and stores an i8x3 normal (actors).

type Mat34 = ([[f32; 3]; 3], [f32; 3]); // rotation, translation

#[derive(Clone)]
struct BakedMdlPose {
    vertices: Vec<[i16; 3]>,
    bones: Vec<Mat34>,
    mouth_xform: Option<Mat34>,
}

// Default (viewmodel) vertex precision. Enemies/NPCs cook at a coarser scale
// (see ENEMY_VERTEX_LOCAL_SCALE) since they are viewed at distance: lower scale
// shrinks i8 deltas + the base, halving model RAM with no visible loss.
const MDL_VERTEX_LOCAL_SCALE: i32 = 8;
const ENEMY_VERTEX_LOCAL_SCALE: i32 = 4; // quarter-unit grid: same i16 RAM, half the near-GTE distortion zone (was 2)

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MdlCookMode {
    full_normals: bool,
    packed_normals: bool,
    vertex_scale: i32,
    simplify_target: usize,
    /// The held weapon is drawn through one fixed camera-relative transform,
    /// so its facing is decidable at cook time. Every other model is viewed
    /// from arbitrary angles and must keep its whole mesh.
    camera_locked: bool,
}

fn mdl_cook_mode(flag: &str) -> Option<MdlCookMode> {
    match flag {
        "--mdl7-vm" => Some(MdlCookMode {
            full_normals: false,
            packed_normals: true,
            vertex_scale: MDL_VERTEX_LOCAL_SCALE,
            simplify_target: 0,
            camera_locked: true,
        }),
        "--mdl7" => Some(MdlCookMode {
            full_normals: false,
            packed_normals: false,
            vertex_scale: ENEMY_VERTEX_LOCAL_SCALE,
            simplify_target: 0,
            camera_locked: false,
        }),
        "--mdl7-lean" => Some(MdlCookMode {
            full_normals: false,
            packed_normals: false,
            vertex_scale: ENEMY_VERTEX_LOCAL_SCALE,
            simplify_target: 768,
            camera_locked: false,
        }),
        "--mdl7-normals" => Some(MdlCookMode {
            full_normals: true,
            packed_normals: false,
            vertex_scale: ENEMY_VERTEX_LOCAL_SCALE,
            simplify_target: 0,
            camera_locked: false,
        }),
        _ => None,
    }
}

fn mdl_local_to_world_q12(vertex_scale: i32) -> u16 {
    (4096 / vertex_scale.max(1)) as u16
}

// ClipRec keeps its original four bytes. Actor cooks never use more than 16
// baked poses per clip, so the high byte of frame_count stores the low eight
// bits of the source duration in 100 ms monster-think quanta. The unused high
// bit of first_frame stores duration bit 8, extending the range to 51.1 s for
// long set-piece clips such as loader/rampwalk without growing resident data.
const MDL_CLIP_FRAME_COUNT_MASK: u16 = 0x00ff;
const MDL_CLIP_FIRST_FRAME_MASK: u16 = 0x7fff;
const MDL_CLIP_DURATION_EXT_BIT: u16 = 0x8000;
const MDL_CLIP_MAX_HOLD_QUANTA: u16 = 0x01ff;
const MDL_RUNTIME_VERTEX_LIMIT: usize = 1024;
const MDL_SIMPLIFIED_VERTEX_TARGET: usize = 960;
const HMD_FLAG_BODY_MASKS: u16 = 1 << 0;
const HMD_FLAG_MOUTH: u16 = 1 << 1;
const HMD_FLAG_PACKED_NORMALS: u16 = 1 << 2;
const HMD_FLAG_I8_NORMALS: u16 = 1 << 3;
const HMD_FLAG_HITBOXES: u16 = 1 << 4;
const HMD_FLAG_FRAME_TIMES: u16 = 1 << 5;
const HMD_FLAG_ALIGNED_MODEL_DATA: u16 = 1 << 6;
const HMD_FLAG_VERTEX_SOA: u16 = 1 << 7;
const HMD7_RANGE_MOUTH: u8 = 1 << 0;
const HMD7_RANGE_BYTES: usize = 8;
const HMD8_AFFINE_BYTES: usize = 20;
const HMD_MOUTH_AFFINE_BYTES: usize = 24;
const HMD7_HITBOX_BYTES: usize = 14;

#[derive(Clone, Copy)]
struct MdlMouthController {
    bone: usize,
    kind: i32,
    start: f32,
    end: f32,
}

fn studio_controller_add(dof: &mut [f32; 6], kind: i32, value: f32) {
    let (axis, rotational) = match kind & 0x7fff {
        0x0001 => (0, false),
        0x0002 => (1, false),
        0x0004 => (2, false),
        0x0008 => (3, true),
        0x0010 => (4, true),
        0x0020 => (5, true),
        _ => return,
    };
    dof[axis] += if rotational {
        value.to_radians()
    } else {
        value
    };
}

/// Affine map from the controller-free mouth bone to its full-open transform,
/// expressed in cooked model coordinates ([HL x,z,y], scaled vertex units).
fn mdl_mouth_relative_xform(closed: &Mat34, open: &Mat34, vertex_scale: i32) -> Mat34 {
    // Studio bone matrices contain rotations only, so inverse(closed.R) is its
    // transpose. A = open.R * closed.R^-1; t = open.t - A * closed.t.
    let mut a = [[0.0f32; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            a[r][c] = open.0[r][0] * closed.0[c][0]
                + open.0[r][1] * closed.0[c][1]
                + open.0[r][2] * closed.0[c][2];
        }
    }
    let t = [
        open.1[0] - (a[0][0] * closed.1[0] + a[0][1] * closed.1[1] + a[0][2] * closed.1[2]),
        open.1[1] - (a[1][0] * closed.1[0] + a[1][1] * closed.1[1] + a[1][2] * closed.1[2]),
        open.1[2] - (a[2][0] * closed.1[0] + a[2][1] * closed.1[1] + a[2][2] * closed.1[2]),
    ];
    let p = [0usize, 2, 1];
    let mut cooked_a = [[0.0f32; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            cooked_a[r][c] = a[p[r]][p[c]];
        }
    }
    (
        cooked_a,
        [
            t[0] * vertex_scale as f32,
            t[2] * vertex_scale as f32,
            t[1] * vertex_scale as f32,
        ],
    )
}

fn mdl_adjust_xform_for_floor_shift(xform: &mut Mat34, shift: i16) {
    // floor_anchor maps p -> p+s, s=(0,-shift,0). For p'=p+s the same
    // controller is A*p' + (t+s-A*s).
    let s = [0.0f32, -(shift as f32), 0.0f32];
    for r in 0..3 {
        let as_r = xform.0[r][0] * s[0] + xform.0[r][1] * s[1] + xform.0[r][2] * s[2];
        xform.1[r] += s[r] - as_r;
    }
}

fn append_mouth_xform(out: &mut Vec<u8>, xform: &Mat34) {
    for row in xform.0 {
        for value in row {
            let q12 = (value * 4096.0)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            out.extend_from_slice(&q12.to_le_bytes());
        }
    }
    for value in xform.1 {
        let q = value.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        out.extend_from_slice(&q.to_le_bytes());
    }
}

fn mdl_bone_local_vertex(vertex: [f32; 3], scale: i32) -> [i16; 3] {
    [
        quantize_mdl_coord(vertex[0], scale),
        quantize_mdl_coord(vertex[2], scale),
        quantize_mdl_coord(vertex[1], scale),
    ]
}

/// Convert an already hierarchy-composed GoldSrc bone matrix into cooked PS1
/// coordinates. `floor_shift` is in cooked coordinates and matches the exact
/// translation removed from the corresponding reference vertex frame.
fn hmd8_q11_code(value: i16) -> i16 {
    let value = value as i32;
    let rounded = if value >= 0 {
        (value + 1) / 2
    } else {
        (value - 1) / 2
    }
    .clamp(-2048, 2046);
    if (4096 - value).abs() < (rounded * 2 - value).abs() {
        2047
    } else {
        rounded as i16
    }
}

fn hmd8_q11_decode(code: i16) -> i16 {
    if code == 2047 {
        4096
    } else {
        code << 1
    }
}

fn append_hmd8_bone_xform(out: &mut Vec<u8>, xform: &Mat34, scale: i32, floor_shift: i16) {
    let permutation = [0usize, 2, 1];
    let mut rotation = [0u16; 9];
    for row in 0..3 {
        for column in 0..3 {
            let q12 = (xform.0[permutation[row]][permutation[column]] * 4096.0)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            rotation[row * 3 + column] = hmd8_q11_code(q12) as u16 & 0x0fff;
        }
    }
    for pair in 0..4 {
        let a = rotation[pair * 2];
        let b = rotation[pair * 2 + 1];
        out.push(a as u8);
        out.push(((a >> 8) as u8 & 0x0f) | ((b as u8 & 0x0f) << 4));
        out.push((b >> 4) as u8);
    }
    out.push(rotation[8] as u8);
    out.push((rotation[8] >> 8) as u8 & 0x0f);
    let translation = [
        quantize_mdl_coord(xform.1[0], scale),
        (quantize_mdl_coord(xform.1[2], scale) as i32 - floor_shift as i32)
            .clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        quantize_mdl_coord(xform.1[1], scale),
    ];
    for value in translation {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn mdl_sequence_hold_quanta(numframes: usize, fps: f32) -> u16 {
    if numframes <= 1 || !fps.is_finite() || fps <= 0.0 {
        return 1;
    }
    ((((numframes - 1) as f32 * 10.0) / fps).ceil() as usize)
        .clamp(1, MDL_CLIP_MAX_HOLD_QUANTA as usize) as u16
}

fn mdl_baked_frame_count(
    animindex: usize,
    numframes: usize,
    max_frames: usize,
    looping: bool,
) -> usize {
    if animindex == 0 {
        return 1;
    }
    // GoldSrc loop sequences include a duplicate terminal pose. It must not
    // consume a second baked slot when the whole cycle fits under the cap.
    let distinct = if looping {
        numframes.saturating_sub(1).max(1)
    } else {
        numframes.max(1)
    };
    distinct.min(max_frames).max(1)
}

/// Score one source pose against the piecewise-linear reconstruction produced
/// by the currently retained frames. Root translation is removed exactly as in
/// the audit metric, so selection spends scarce palettes on silhouette/limb
/// motion rather than authored travel that belongs to the entity path.
fn mdl_pose_frame_error_score(
    source: &[Vec<[i16; 3]>],
    sample_frames: &[usize],
    looping: bool,
    frame: usize,
) -> (i64, i64) {
    if source.is_empty() || sample_frames.is_empty() || source[0].is_empty() {
        return (0, 0);
    }
    let cycle_len = if looping && source.len() > 1 {
        source.len() - 1
    } else {
        source.len()
    };
    if frame >= cycle_len || cycle_len == 0 {
        return (0, 0);
    }
    let (left_sample, right_sample, left_pos, right_pos, frame_pos) = if sample_frames.len() == 1 {
        (0, 0, sample_frames[0], sample_frames[0], frame)
    } else if looping {
        let mut left = sample_frames.len() - 1;
        for (index, &position) in sample_frames.iter().enumerate() {
            if position <= frame {
                left = index;
            } else {
                break;
            }
        }
        let right = (left + 1) % sample_frames.len();
        let left_pos = sample_frames[left];
        let right_pos = if right == 0 {
            sample_frames[0] + cycle_len
        } else {
            sample_frames[right]
        };
        let frame_pos = if frame < left_pos {
            frame + cycle_len
        } else {
            frame
        };
        (left, right, left_pos, right_pos, frame_pos)
    } else {
        let mut left = 0usize;
        while left + 1 < sample_frames.len() && sample_frames[left + 1] <= frame {
            left += 1;
        }
        let right = (left + 1).min(sample_frames.len() - 1);
        (
            left,
            right,
            sample_frames[left],
            sample_frames[right],
            frame,
        )
    };
    let denominator = right_pos.saturating_sub(left_pos).max(1);
    let frac16 = frame_pos.saturating_sub(left_pos).saturating_mul(16) / denominator;
    let a = &source[sample_frames[left_sample].min(source.len() - 1)];
    let b = &source[sample_frames[right_sample].min(source.len() - 1)];
    let exact = &source[frame];
    let count = exact.len().min(a.len()).min(b.len());
    if count == 0 {
        return (0, 0);
    }
    let mut mean = [0i64; 3];
    for vertex in 0..count {
        for axis in 0..3 {
            let reconstructed = a[vertex][axis] as i32
                + (((b[vertex][axis] as i32 - a[vertex][axis] as i32) * frac16 as i32) >> 4);
            mean[axis] += exact[vertex][axis] as i64 - reconstructed as i64;
        }
    }
    for axis in &mut mean {
        *axis /= count as i64;
    }
    let mut squared_sum = 0i64;
    let mut max_squared = 0i64;
    for vertex in 0..count {
        let mut vertex_squared = 0i64;
        for axis in 0..3 {
            let reconstructed = a[vertex][axis] as i32
                + (((b[vertex][axis] as i32 - a[vertex][axis] as i32) * frac16 as i32) >> 4);
            let error = exact[vertex][axis] as i64 - reconstructed as i64 - mean[axis];
            vertex_squared += error * error;
        }
        squared_sum += vertex_squared;
        max_squared = max_squared.max(vertex_squared);
    }
    (max_squared, squared_sum)
}

/// Greedily retain the source pose that the current reconstruction misses by
/// the most. Endpoints are mandatory for one-shots; loops retain pose zero and
/// close cyclically. When a clip is exactly linear, the widest temporal gap is
/// split so selection remains well distributed and deterministic.
fn mdl_motion_sample_frames(source: &[Vec<[i16; 3]>], budget: usize, looping: bool) -> Vec<usize> {
    let distinct = if looping {
        source.len().saturating_sub(1).max(1)
    } else {
        source.len().max(1)
    };
    let budget = budget.clamp(1, distinct);
    let mut samples = vec![0usize];
    if !looping && budget > 1 && distinct > 1 {
        samples.push(distinct - 1);
    }
    while samples.len() < budget {
        samples.sort_unstable();
        let mut best = None::<(i64, i64, usize, usize)>;
        for candidate in 0..distinct {
            if samples.binary_search(&candidate).is_ok() {
                continue;
            }
            let (max_squared, squared_sum) =
                mdl_pose_frame_error_score(source, &samples, looping, candidate);
            let gap = samples
                .iter()
                .map(|&sample| {
                    let direct = candidate.abs_diff(sample);
                    if looping {
                        direct.min(distinct - direct)
                    } else {
                        direct
                    }
                })
                .min()
                .unwrap_or(0);
            let score = (max_squared, squared_sum, gap, usize::MAX - candidate);
            if best.is_none_or(|current| score > current) {
                best = Some(score);
            }
        }
        let Some((_, _, _, reverse_candidate)) = best else {
            break;
        };
        samples.push(usize::MAX - reverse_candidate);
    }
    samples.sort_unstable();
    samples
}

fn mdl_source_phase(source_frame: usize, numframes: usize, looping: bool) -> u8 {
    if numframes <= 1 {
        return 0;
    }
    if looping {
        let cycle = numframes - 1;
        (source_frame.saturating_mul(256) / cycle.max(1)).min(255) as u8
    } else {
        (source_frame.saturating_mul(255) / (numframes - 1)).min(255) as u8
    }
}

/// Translation-invariant vertex error for the exact uniform reconstruction
/// the PS1 can perform between retained poses. Global root motion/floor
/// anchoring is removed per source pose so the score measures limb/silhouette
/// loss rather than authored travel.
fn mdl_pose_reconstruction_error(
    source: &[Vec<[i16; 3]>],
    sample_frames: &[usize],
    looping: bool,
    vertex_scale: i32,
) -> (f64, f64) {
    if source.is_empty() || sample_frames.is_empty() || source[0].is_empty() {
        return (0.0, 0.0);
    }
    let cycle_len = if looping && source.len() > 1 {
        source.len() - 1
    } else {
        source.len()
    };
    if cycle_len == 0 {
        return (0.0, 0.0);
    }
    let mut squared_sum = 0f64;
    let mut component_count = 0usize;
    let mut max_squared = 0i64;
    for frame in 0..cycle_len {
        let (left_sample, right_sample, left_pos, right_pos) = if sample_frames.len() == 1 {
            (0, 0, sample_frames[0], sample_frames[0])
        } else if looping {
            let mut left = sample_frames.len() - 1;
            for (index, &position) in sample_frames.iter().enumerate() {
                if position <= frame {
                    left = index;
                } else {
                    break;
                }
            }
            let right = (left + 1) % sample_frames.len();
            let left_pos = sample_frames[left];
            let right_pos = if right == 0 {
                sample_frames[0] + cycle_len
            } else {
                sample_frames[right]
            };
            let frame_pos = if frame < left_pos {
                frame + cycle_len
            } else {
                frame
            };
            (left, right, left_pos, right_pos.max(frame_pos))
        } else {
            let mut left = 0usize;
            while left + 1 < sample_frames.len() && sample_frames[left + 1] <= frame {
                left += 1;
            }
            let right = (left + 1).min(sample_frames.len() - 1);
            (left, right, sample_frames[left], sample_frames[right])
        };
        let frame_pos = if looping && frame < left_pos {
            frame + cycle_len
        } else {
            frame
        };
        let denominator = right_pos.saturating_sub(left_pos).max(1);
        let frac16 = frame_pos.saturating_sub(left_pos).saturating_mul(16) / denominator;
        let a = &source[sample_frames[left_sample].min(source.len() - 1)];
        let b = &source[sample_frames[right_sample].min(source.len() - 1)];
        let exact = &source[frame];
        let count = exact.len().min(a.len()).min(b.len());
        if count == 0 {
            continue;
        }
        let mut mean = [0i64; 3];
        for vertex in 0..count {
            for axis in 0..3 {
                let reconstructed = a[vertex][axis] as i32
                    + (((b[vertex][axis] as i32 - a[vertex][axis] as i32) * frac16 as i32) >> 4);
                mean[axis] += exact[vertex][axis] as i64 - reconstructed as i64;
            }
        }
        for axis in &mut mean {
            *axis /= count as i64;
        }
        for vertex in 0..count {
            let mut vertex_squared = 0i64;
            for axis in 0..3 {
                let reconstructed = a[vertex][axis] as i32
                    + (((b[vertex][axis] as i32 - a[vertex][axis] as i32) * frac16 as i32) >> 4);
                let error = exact[vertex][axis] as i64 - reconstructed as i64 - mean[axis];
                vertex_squared += error * error;
                squared_sum += (error * error) as f64;
                component_count += 1;
            }
            max_squared = max_squared.max(vertex_squared);
        }
    }
    let scale = vertex_scale.max(1) as f64;
    (
        (squared_sum / component_count.max(1) as f64).sqrt() / scale,
        (max_squared as f64).sqrt() / scale,
    )
}

fn mdl_pack_clip(first_frame: u16, frame_count: u16, hold_quanta: u16) -> (u16, u16) {
    debug_assert_eq!(first_frame & !MDL_CLIP_FIRST_FRAME_MASK, 0);
    debug_assert!(frame_count > 0 && frame_count <= MDL_CLIP_FRAME_COUNT_MASK);
    debug_assert!(hold_quanta <= MDL_CLIP_MAX_HOLD_QUANTA);
    let packed_first = (first_frame & MDL_CLIP_FIRST_FRAME_MASK)
        | if hold_quanta & 0x0100 != 0 {
            MDL_CLIP_DURATION_EXT_BIT
        } else {
            0
        };
    let packed_count = (frame_count & MDL_CLIP_FRAME_COUNT_MASK) | ((hold_quanta & 0x00ff) << 8);
    (packed_first, packed_count)
}

fn quantize_mdl_coord(v: f32, scale: i32) -> i16 {
    (v * scale as f32)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

fn mdl_face_normal_i8(base: &[[i16; 3]], a: u16, b: u16, c: u16) -> [i8; 3] {
    let Some(va) = base.get(a as usize).copied() else {
        return [0; 3];
    };
    let Some(vb) = base.get(b as usize).copied() else {
        return [0; 3];
    };
    let Some(vc) = base.get(c as usize).copied() else {
        return [0; 3];
    };
    let ux = vb[0] as f64 - va[0] as f64;
    let uy = vb[1] as f64 - va[1] as f64;
    let uz = vb[2] as f64 - va[2] as f64;
    let vx = vc[0] as f64 - va[0] as f64;
    let vy = vc[1] as f64 - va[1] as f64;
    let vz = vc[2] as f64 - va[2] as f64;
    let nx = uy * vz - uz * vy;
    let ny = uz * vx - ux * vz;
    let nz = ux * vy - uy * vx;
    let len = (nx * nx + ny * ny + nz * nz).sqrt();
    if len <= 0.0001 {
        return [0; 3];
    }
    [
        (nx * 127.0 / len).round().clamp(-127.0, 127.0) as i8,
        (ny * 127.0 / len).round().clamp(-127.0, 127.0) as i8,
        (nz * 127.0 / len).round().clamp(-127.0, 127.0) as i8,
    ]
}

/// Signed 5:5:5 unit normal for the HMD8 triangle pad. Viewmodels never use
/// body masks, so preserving a direction here is free in both disc and RAM.
/// Fifteen levels per hemisphere are enough for flat PS1 lighting while the
/// HMD8 full-normal flag keeps i8 normals for actors that request them.
fn mdl_pack_normal_555(n: [i8; 3]) -> u16 {
    let q5 = |v: i8| -> u16 {
        let scaled = v as i32 * 15;
        let rounded = (scaled + if scaled < 0 { -63 } else { 63 }) / 127;
        (rounded.clamp(-15, 15) as i16 as u16) & 0x1f
    };
    q5(n[0]) | (q5(n[1]) << 5) | (q5(n[2]) << 10)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SimplifiedMdlMesh {
    frames: Vec<Vec<[i16; 3]>>,
    remap: Vec<usize>,
    bones: Vec<usize>,
    tri_idx: Vec<u16>,
    tri_tex: Vec<u16>,
    tri_uv: Vec<u8>,
    tri_norm: Vec<[i8; 3]>,
    grid_size: u32,
}

fn rounded_i16_mean(sum: i64, count: i64) -> i16 {
    debug_assert!(count > 0);
    let mean = if sum < 0 {
        -((-sum + count / 2) / count)
    } else {
        (sum + count / 2) / count
    };
    mean.clamp(i16::MIN as i64, i16::MAX as i64) as i16
}

fn mdl_triangle_is_degenerate(frame: &[[i16; 3]], idx: [u16; 3]) -> bool {
    if idx[0] == idx[1] || idx[1] == idx[2] || idx[2] == idx[0] {
        return true;
    }
    let (Some(a), Some(b), Some(c)) = (
        frame.get(idx[0] as usize),
        frame.get(idx[1] as usize),
        frame.get(idx[2] as usize),
    ) else {
        return true;
    };
    let u = [
        b[0] as i64 - a[0] as i64,
        b[1] as i64 - a[1] as i64,
        b[2] as i64 - a[2] as i64,
    ];
    let v = [
        c[0] as i64 - a[0] as i64,
        c[1] as i64 - a[1] as i64,
        c[2] as i64 - a[2] as i64,
    ];
    let cross = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    cross == [0; 3]
}

/// `VM_BASE` scaled by `VM_SCALE`: the held model's view rotation maps a model
/// (x,y,z) to view (-z,-y,x) and is the same every frame. `VM_VIEW_SHIFT` is in
/// world units and reaches the runtime multiplied by the model's local scale.
/// Keep these three in step with `game/src/main.rs`; the eye margin below is
/// what makes a drift there harmless rather than a hole in the weapon.
const VM_VIEW_SCALE: i64 = 5;
const VM_VIEW_SHIFT_WORLD: [i64; 3] = [30, 30, 40];

/// How far the cull assumes the eye could move relative to the authored
/// placement, in local units. `viewmodel_offset` is a screen-space draw offset
/// rather than a camera move, so nothing at runtime consumes this today: it
/// exists so retuning VM_SCALE or VM_VIEW_SHIFT cannot silently uncover a
/// triangle the cook already deleted. It costs about three points of yield.
const VM_CULL_EYE_MARGIN: i64 = 64;

/// A viewmodel is camera-locked, so whether a triangle faces the camera is a
/// property of the baked pose alone. Report the triangles that face away in
/// every pose: the runtime's per-triangle cross product rejects those before it
/// builds a packet, so they are pure RAM and load cost. Triangle 0's texture is
/// the two-sided sleeve the runtime never culls, which is why `tri_tex` is
/// consulted here.
///
/// This deliberately answers only "could this ever be front-facing", not "could
/// this ever be visible". The larger prize is triangles that face the camera and
/// are then covered by the weapon itself, but deciding that needs the engine's
/// exact projection and fill rule; a host reimplementation that is subtly wrong
/// punches holes through the model.
fn viewmodel_never_front_facing(
    frames: &[Vec<[i16; 3]>],
    tri_idx: &[u16],
    tri_tex: &[u16],
    vertex_scale: i32,
) -> Vec<bool> {
    let n_tris = tri_idx.len() / 3;
    let shift = VM_VIEW_SHIFT_WORLD.map(|axis| axis * vertex_scale.max(1) as i64);
    let mut never = vec![true; n_tris];
    for frame in frames {
        for tri in 0..n_tris {
            if !never[tri] || tri_tex.get(tri) == Some(&0) {
                never[tri] = false;
                continue;
            }
            let corner = |k: usize| -> Option<[i64; 3]> {
                let v = *frame.get(tri_idx[tri * 3 + k] as usize)?;
                Some([v[0] as i64, v[1] as i64, v[2] as i64])
            };
            let (Some(a), Some(b), Some(c)) = (corner(0), corner(1), corner(2)) else {
                never[tri] = false;
                continue;
            };
            for dx in [-VM_CULL_EYE_MARGIN, 0, VM_CULL_EYE_MARGIN] {
                for dy in [-VM_CULL_EYE_MARGIN, 0, VM_CULL_EYE_MARGIN] {
                    for dz in [-VM_CULL_EYE_MARGIN, 0, VM_CULL_EYE_MARGIN] {
                        let eye = [shift[0] + dx, shift[1] + dy, shift[2] + dz];
                        let view = |p: [i64; 3]| {
                            [
                                -p[2] * VM_VIEW_SCALE + eye[0],
                                -p[1] * VM_VIEW_SCALE + eye[1],
                                p[0] * VM_VIEW_SCALE + eye[2],
                            ]
                        };
                        let (va, vb, vc) = (view(a), view(b), view(c));
                        let u = [vb[0] - va[0], vb[1] - va[1], vb[2] - va[2]];
                        let w = [vc[0] - va[0], vc[1] - va[1], vc[2] - va[2]];
                        let normal = [
                            u[1] * w[2] - u[2] * w[1],
                            u[2] * w[0] - u[0] * w[2],
                            u[0] * w[1] - u[1] * w[0],
                        ];
                        if normal[0] * va[0] + normal[1] * va[1] + normal[2] * va[2] < 0 {
                            never[tri] = false;
                        }
                    }
                }
            }
        }
    }
    never
}

/// Reduce an oversized baked studio mesh without mixing vertices controlled by
/// different bones. The smallest integer cell size that meets `target` is
/// selected from frame zero. Cells are relative to the model minimum so a cell
/// larger than its span deterministically converges to one cluster per bone.
///
/// The stable original-vertex walk assigns cluster indices, then the same map
/// averages every baked frame. Triangle corner metadata remains per-corner;
/// triangles collapsed by welding are removed and normals are regenerated from
/// the simplified base frame. Inputs at or below `target` are exact no-ops.
fn simplify_mdl_animated_mesh(
    frames: &[Vec<[i16; 3]>],
    vbone: &[usize],
    tri_idx: &[u16],
    tri_tex: &[u16],
    tri_uv: &[u8],
    tri_norm: &[[i8; 3]],
    target: usize,
) -> SimplifiedMdlMesh {
    assert!(target > 0 && target <= u16::MAX as usize);
    let nverts = vbone.len();
    assert!(!frames.is_empty(), "studio model has no baked frames");
    for frame in frames {
        assert_eq!(frame.len(), nverts, "studio frame vertex count changed");
    }
    assert_eq!(tri_idx.len() % 3, 0, "studio triangle index tail");
    let ntris = tri_idx.len() / 3;
    assert_eq!(tri_tex.len(), ntris, "studio texture metadata drift");
    assert_eq!(tri_uv.len(), ntris * 6, "studio UV metadata drift");
    assert_eq!(tri_norm.len(), ntris, "studio normal metadata drift");

    if nverts <= target {
        return SimplifiedMdlMesh {
            frames: frames.to_vec(),
            remap: (0..nverts).collect(),
            bones: vbone.to_vec(),
            tri_idx: tri_idx.to_vec(),
            tri_tex: tri_tex.to_vec(),
            tri_uv: tri_uv.to_vec(),
            tri_norm: tri_norm.to_vec(),
            grid_size: 0,
        };
    }

    let mut used_bones = vbone.to_vec();
    used_bones.sort_unstable();
    used_bones.dedup();
    assert!(
        used_bones.len() <= target,
        "bone-safe studio target is smaller than the used bone count"
    );

    let base = &frames[0];
    let mut mins = [i16::MAX; 3];
    for v in base {
        for axis in 0..3 {
            mins[axis] = mins[axis].min(v[axis]);
        }
    }

    let mut grid_size = 1u32;
    let (remap, nclusters) = loop {
        let mut cluster_for_key: BTreeMap<(usize, u32, u32, u32), usize> = BTreeMap::new();
        let mut remap = Vec::with_capacity(nverts);
        for (vi, v) in base.iter().enumerate() {
            let key = (
                vbone[vi],
                (v[0] as i32 - mins[0] as i32) as u32 / grid_size,
                (v[1] as i32 - mins[1] as i32) as u32 / grid_size,
                (v[2] as i32 - mins[2] as i32) as u32 / grid_size,
            );
            let next = cluster_for_key.len();
            let cluster = *cluster_for_key.entry(key).or_insert(next);
            remap.push(cluster);
        }
        if cluster_for_key.len() <= target {
            break (remap, cluster_for_key.len());
        }
        // Oversized studio meshes are rare and small enough that testing each
        // integer grid is cheap at cook time. Doubling here made the loader
        // jump from 1,114 vertices straight down to 553 even though a grid of
        // 37 reaches the 960-vertex target with nearly twice the detail.
        grid_size = grid_size
            .checked_add(1)
            .expect("studio simplification grid overflow");
    };
    assert!(nclusters <= target);

    let mut counts = vec![0i64; nclusters];
    for &cluster in &remap {
        counts[cluster] += 1;
    }
    let mut simplified_frames = Vec::with_capacity(frames.len());
    for frame in frames {
        let mut sums = vec![[0i64; 3]; nclusters];
        for (vi, v) in frame.iter().enumerate() {
            let sum = &mut sums[remap[vi]];
            for axis in 0..3 {
                sum[axis] += v[axis] as i64;
            }
        }
        let mut simplified = Vec::with_capacity(nclusters);
        for (cluster, sum) in sums.iter().enumerate() {
            simplified.push([
                rounded_i16_mean(sum[0], counts[cluster]),
                rounded_i16_mean(sum[1], counts[cluster]),
                rounded_i16_mean(sum[2], counts[cluster]),
            ]);
        }
        simplified_frames.push(simplified);
    }

    let mut simplified_idx = Vec::with_capacity(tri_idx.len());
    let mut simplified_tex = Vec::with_capacity(tri_tex.len());
    let mut simplified_uv = Vec::with_capacity(tri_uv.len());
    let mut simplified_norm = Vec::with_capacity(tri_norm.len());
    for tri in 0..ntris {
        let old = [
            tri_idx[tri * 3] as usize,
            tri_idx[tri * 3 + 1] as usize,
            tri_idx[tri * 3 + 2] as usize,
        ];
        assert!(old.iter().all(|&vi| vi < nverts));
        let mapped = [
            remap[old[0]] as u16,
            remap[old[1]] as u16,
            remap[old[2]] as u16,
        ];
        if mdl_triangle_is_degenerate(&simplified_frames[0], mapped) {
            continue;
        }
        simplified_idx.extend_from_slice(&mapped);
        simplified_tex.push(tri_tex[tri]);
        simplified_uv.extend_from_slice(&tri_uv[tri * 6..tri * 6 + 6]);
        simplified_norm.push(mdl_face_normal_i8(
            &simplified_frames[0],
            mapped[0],
            mapped[1],
            mapped[2],
        ));
    }

    let mut simplified_bones = vec![usize::MAX; nclusters];
    for (old, &cluster) in remap.iter().enumerate() {
        if simplified_bones[cluster] == usize::MAX {
            simplified_bones[cluster] = vbone[old];
        } else {
            debug_assert_eq!(simplified_bones[cluster], vbone[old]);
        }
    }
    SimplifiedMdlMesh {
        frames: simplified_frames,
        remap,
        bones: simplified_bones,
        tri_idx: simplified_idx,
        tri_tex: simplified_tex,
        tri_uv: simplified_uv,
        tri_norm: simplified_norm,
        grid_size,
    }
}

/// Anchor baked frames to the floor. The 5 canonical clips (idle/walk/attack/
/// pain/death) anchor PER FRAME (min_y -> 0) so feet stay planted. Named
/// script clips (sit1, ...) instead shift by the CONSTANT baseline taken from
/// clip 0 frame 0: a seated pose keeps its authored root offset, so feet dip
/// BELOW the script mark (mark = chair seat, feet = floor) instead of the
/// whole body being lifted until its lowest vertex touches the mark -- the
/// "sitting guy floats above the desk" bug.
fn floor_anchor_mdl_frames(
    frames: &mut [Vec<[i16; 3]>],
    clips: &[(u16, u16)],
    canonical: usize,
) -> Vec<i16> {
    let base_shift: i32 = clips
        .first()
        .map(|&(f, _)| f as usize)
        .and_then(|f0| frames.get(f0))
        .and_then(|fv| fv.iter().map(|v| v[1] as i32).min())
        .unwrap_or(0);
    let mut script_frames = vec![false; frames.len()];
    for (ci, &(first, count)) in clips.iter().enumerate() {
        if ci < canonical {
            continue;
        }
        for f in first as usize..(first as usize + count as usize).min(frames.len()) {
            script_frames[f] = true;
        }
    }
    let mut shifts = vec![0i16; frames.len()];
    for (fi, fv) in frames.iter_mut().enumerate() {
        let shift = if script_frames.get(fi).copied().unwrap_or(false) {
            base_shift
        } else {
            match fv.iter().map(|v| v[1] as i32).min() {
                Some(m) => m,
                None => continue,
            }
        };
        shifts[fi] = shift.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        if shift == 0 {
            continue;
        }
        for v in fv {
            v[1] = (v[1] as i32 - shift).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        }
    }
    shifts
}

fn angle_quat(a: [f32; 3]) -> [f32; 4] {
    let (sr, cr) = ((a[0] * 0.5).sin(), (a[0] * 0.5).cos());
    let (sp, cp) = ((a[1] * 0.5).sin(), (a[1] * 0.5).cos());
    let (sy, cy) = ((a[2] * 0.5).sin(), (a[2] * 0.5).cos());
    [
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    ]
}

fn quat_mat(q: [f32; 4]) -> [[f32; 3]; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// `p ∘ c` (apply c then p).
fn concat(p: &Mat34, c: &Mat34) -> Mat34 {
    let mut r = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = p.0[i][0] * c.0[0][j] + p.0[i][1] * c.0[1][j] + p.0[i][2] * c.0[2][j];
        }
    }
    let t = [
        p.0[0][0] * c.1[0] + p.0[0][1] * c.1[1] + p.0[0][2] * c.1[2] + p.1[0],
        p.0[1][0] * c.1[0] + p.0[1][1] * c.1[1] + p.0[1][2] * c.1[2] + p.1[1],
        p.0[2][0] * c.1[0] + p.0[2][1] * c.1[1] + p.0[2][2] * c.1[2] + p.1[2],
    ];
    (r, t)
}

fn apply(m: &Mat34, v: [f32; 3]) -> [f32; 3] {
    [
        m.0[0][0] * v[0] + m.0[0][1] * v[1] + m.0[0][2] * v[2] + m.1[0],
        m.0[1][0] * v[0] + m.0[1][1] * v[1] + m.0[1][2] * v[2] + m.1[1],
        m.0[2][0] * v[0] + m.0[2][1] * v[1] + m.0[2][2] * v[2] + m.1[2],
    ]
}

/// Decode one studio RLE animation channel value at `frame`. `base` is the byte
/// offset of the `mstudioanimvalue_t` array (animindex + bone*12 + offset[dof]).
/// Format: [valid:u8][total:u8] then `valid` i16 values, repeated per span.
fn anim_value(b: &[u8], base: usize, frame: usize) -> i16 {
    let mut p = base;
    let mut k = frame as i32;
    loop {
        if p + 1 >= b.len() {
            return 0;
        }
        let valid = b[p] as i32;
        let total = b[p + 1] as i32;
        if total > k {
            let idx = if valid > k {
                (k + 1) as usize
            } else {
                valid as usize
            };
            let o = p + idx * 2;
            return if o + 1 < b.len() {
                i16::from_le_bytes([b[o], b[o + 1]])
            } else {
                0
            };
        }
        k -= total;
        p += (valid as usize + 1) * 2;
    }
}

/// Cook an MDL texture (8-bit indices + 256-colour palette) to 4-bit + CLUT.
fn cook_mdl_tex(b: &[u8], idx: usize, w0: usize, h0: usize) -> CookedTex {
    let palo = idx + w0 * h0;
    let pal = |p: usize| {
        (
            *b.get(palo + p * 3).unwrap_or(&0),
            *b.get(palo + p * 3 + 1).unwrap_or(&0),
            *b.get(palo + p * 3 + 2).unwrap_or(&0),
        )
    };
    let fw = final_size(w0 as u32) as usize;
    let fh = final_size(h0 as u32) as usize;
    let mut colors: Vec<(u8, u8, u8)> = Vec::with_capacity(fw * fh);
    for y in 0..fh {
        for x in 0..fw {
            let pi = *b
                .get(idx + (y * h0 / fh) * w0 + (x * w0 / fw))
                .unwrap_or(&0) as usize;
            colors.push(pal(pi));
        }
    }
    let pal16 = median_cut16(&colors);
    let mut clut = [0u16; 16];
    for (i, c) in pal16.iter().enumerate() {
        clut[i] = to_bgr555(c.0, c.1, c.2);
    }
    let mut pix4 = vec![0u8; fw * fh / 2];
    for (i, ch) in colors.chunks(2).enumerate() {
        let lo = nearest16(&pal16, ch[0]);
        let hi = ch.get(1).map(|c| nearest16(&pal16, *c)).unwrap_or(0);
        pix4[i] = lo | (hi << 4);
    }
    CookedTex {
        w: fw as u16,
        h: fh as u16,
        clut,
        pix4,
    }
}

const STUDIO_NF_FLATSHADE: i32 = 0x0001;
const STUDIO_NF_CHROME: i32 = 0x0002;
// Sixteen remains the conservative default, but small models can afford more
// source poses without increasing runtime interpolation work. Keep this limit
// synchronized with host/hl-build/model_audit.rs.
const MAX_BAKED_SEQUENCE_FRAMES: usize = 64;

#[derive(Clone)]
struct SeqSpec {
    seq: i32, // -2 = resolve `name` against the MDL's sequence labels
    name: String,
    max_frames: usize,
}

fn parse_seq_specs(text: &str) -> Result<Vec<SeqSpec>, String> {
    let mut out = Vec::new();
    for raw in text.split(',') {
        let raw = raw.trim();
        if raw.is_empty() || raw.contains('=') {
            continue; // alias tokens (a=b) only feed the clips manifest
        }
        let (seq_text, frame_text) = raw.split_once(':').unwrap_or((raw, "16"));
        let max_frames = frame_text
            .parse::<usize>()
            .map_err(|_| format!("bad frame cap '{frame_text}'"))?
            .clamp(1, MAX_BAKED_SEQUENCE_FRAMES);
        match seq_text.parse::<i32>() {
            Ok(seq) => out.push(SeqSpec {
                seq,
                name: String::new(),
                max_frames,
            }),
            Err(_) => out.push(SeqSpec {
                seq: -2,
                name: seq_text.to_ascii_lowercase(),
                max_frames,
            }),
        }
    }
    if out.is_empty() {
        out.push(SeqSpec {
            seq: 0,
            name: String::new(),
            max_frames: 16,
        });
    }
    Ok(out)
}

/// Optional `body=N` token in a roster clip spec: GoldSrc's packed bodygroup
/// index, exactly as an entity's `pev->body`. The world tripmine is body 3 of
/// v_tripmine.mdl (blank hands + the boned world mine), so cooking sub-model 0
/// of every bodypart would emit the viewmodel's hands instead. Zero (the
/// default) selects sub-model 0 everywhere, which is what every other roster
/// entry already got.
fn parse_body_group(text: &str) -> usize {
    text.split(',')
        .filter_map(|field| field.trim().split_once('='))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("body"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

fn chrome_uv(n: [f32; 3], fw: f32, fh: f32) -> (u8, u8) {
    // GoldSrc maps a chrome dot product from [-1,+1] across the full 64-pixel
    // source texture: (dot + 1) * 32. The old 0.25 factor sampled only the
    // middle half, pinning shotgun barrel faces into barrel_chrome's white
    // centre stripe. Viewmodels are camera-locked, so this static axis
    // approximation keeps the cheap baked-UV format while restoring the
    // source renderer's full reflection range.
    let u = (0.5 + n[0].clamp(-1.0, 1.0) * 0.5) * (fw - 1.0).max(1.0);
    let v = (0.5 - n[2].clamp(-1.0, 1.0) * 0.5) * (fh - 1.0).max(1.0);
    (
        u.round().clamp(0.0, 255.0) as u8,
        v.round().clamp(0.0, 255.0) as u8,
    )
}

fn cook_mdl(
    path: &str,
    out: &str,
    tex_out: Option<&str>,
    specs: &[SeqSpec],
    body_group: usize,
    full_normals: bool,
    packed_normals: bool,
    vertex_scale: i32,
    simplify_target: usize,
    camera_locked: bool,
    audit_out: Option<&str>,
) -> Result<(), String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
    if b.get(0..4) != Some(b"IDST") {
        return Err(format!("{}: not a studio MDL", path));
    }
    let floor_anchor_frames = std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map_or(true, |name| !name.starts_with("v_") && name != "can.mdl");
    let i = |o: usize| i32le(&b, o).unwrap_or(0);
    let f = |o: usize| f32le(&b, o).unwrap_or(0.0);
    let h16 = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
    let model_name = std::path::Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let human_bodygroups = model_name == "scientist" || model_name == "barney";
    let scientist_bodygroups = model_name == "scientist";
    // Reuse the existing body visibility masks for the can's six skin
    // families. Geometry is shared; only texture face runs differ.
    let can_skins = model_name == "can";

    // Bone metadata: parent + value[6] (pos/rot defaults) + scale[6] (anim deltas).
    let (numbones, boneindex) = (i(140) as usize, i(144) as usize);
    struct BMeta {
        parent: i32,
        value: [f32; 6],
        scale: [f32; 6],
    }
    let bmeta: Vec<BMeta> = (0..numbones)
        .map(|bi| {
            let bo = boneindex + bi * 112;
            let mut value = [0.0f32; 6];
            let mut scale = [0.0f32; 6];
            for j in 0..6 {
                value[j] = f(bo + 64 + j * 4);
                scale[j] = f(bo + 88 + j * 4);
            }
            BMeta {
                parent: i(bo + 32),
                value,
                scale,
            }
        })
        .collect();
    // GoldSrc traces bullets against these bone-local boxes after applying
    // the current studio pose. Keep the authored boxes and their source bone
    // so OC-03 does not fall back to a static movement-hull approximation.
    #[derive(Clone, Copy)]
    struct MdlHitbox {
        bone: usize,
        bbmin: [f32; 3],
        bbmax: [f32; 3],
    }
    let (numhitboxes, hitboxindex) = (i(156).max(0) as usize, i(160).max(0) as usize);
    let source_hitboxes = if floor_anchor_frames {
        (0..numhitboxes)
            .filter_map(|hi| {
                let ho = hitboxindex.checked_add(hi.checked_mul(32)?)?;
                let bone = i(ho);
                (bone >= 0 && (bone as usize) < numbones && ho + 32 <= b.len()).then(|| MdlHitbox {
                    bone: bone as usize,
                    bbmin: [f(ho + 8), f(ho + 12), f(ho + 16)],
                    bbmax: [f(ho + 20), f(ho + 24), f(ho + 28)],
                })
            })
            .collect::<Vec<_>>()
    } else {
        // Viewmodels never participate in world traces, and the largest one
        // already exactly fills the fixed 80,896-byte staging pool.
        Vec::new()
    };
    let (numcontrollers, controllerindex) = (i(148).max(0) as usize, i(152) as usize);
    let mouth_controller = if human_bodygroups {
        (0..numcontrollers).find_map(|ci| {
            let co = controllerindex + ci * 24;
            if i(co + 20) == 4 {
                let bone = i(co);
                (bone >= 0 && bone < numbones as i32).then_some(MdlMouthController {
                    bone: bone as usize,
                    kind: i(co + 4),
                    start: f(co + 8),
                    end: f(co + 12),
                })
            } else {
                None
            }
        })
    } else {
        None
    };

    // Textures: human models keep them in an external <base>T.mdl (numtextures==0).
    let ext = i(180) == 0;
    let tbuf: Vec<u8> = if ext {
        let tp = path
            .strip_suffix(".mdl")
            .map(|s| format!("{}T.mdl", s))
            .unwrap_or_else(|| format!("{}T.mdl", path));
        std::fs::read(&tp).map_err(|e| format!("texture file {}: {}", tp, e))?
    } else {
        Vec::new()
    };
    let tb: &[u8] = if ext { &tbuf } else { &b };
    let ti = |o: usize| i32le(tb, o).unwrap_or(0);
    let th16 = |o: usize| i16::from_le_bytes([tb[o], tb[o + 1]]);
    let (textureindex, skinindex) = (ti(184) as usize, ti(200) as usize);
    let (numskinref, numskinfamilies) = (ti(192).max(0) as usize, ti(196).max(0) as usize);
    // Attachment 0 of a v_ model is its muzzle. GoldSrc places the flash
    // there; without it the runtime has to guess a screen position, which can
    // only ever suit one weapon. Bone-relative, so it needs the same posed
    // bone matrices the vertices are baked through.
    let (numattachments, attachmentindex) = (i(212) as usize, i(216) as usize);
    let muzzle_attachment = (numattachments > 0 && attachmentindex + 52 <= b.len()).then(|| {
        let o = attachmentindex;
        (
            i32le(&b, o + 36).unwrap_or(0).max(0) as usize,
            [
                f32le(&b, o + 40).unwrap_or(0.0),
                f32le(&b, o + 44).unwrap_or(0.0),
                f32le(&b, o + 48).unwrap_or(0.0),
            ],
        )
    });
    let (numbodyparts, bodypartindex) = (i(204) as usize, i(208) as usize);
    struct MdlPart {
        nummesh: usize,
        meshindex: usize,
        vert_base: usize,
        norm_base: usize,
        body_mask: u8,
    }

    // Raw vertices + their bone (positions are constant; bone matrices animate).
    // Ordinary models retain body value 0. Scientist and Barney keep every
    // authored bodygroup model in one shared stream; per-vertex/per-face masks
    // let the PS1 project and draw only the selected head/weapon at runtime.
    let mut parts: Vec<MdlPart> = Vec::new();
    let mut vp: Vec<[f32; 3]> = Vec::new();
    let mut vbone: Vec<usize> = Vec::new();
    let mut vert_body_mask: Vec<u8> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    for bp in 0..numbodyparts {
        let bpo = bodypartindex + bp * 76;
        let nummodels = i(bpo + 64).max(0) as usize;
        if nummodels == 0 {
            continue;
        }
        let base = i(bpo + 68).max(1) as usize;
        let models = if human_bodygroups { nummodels } else { 1 };
        // Non-human models cook ONE sub-model per bodypart: the one the entity's
        // body index selects, GoldSrc's own (body / base) % nummodels.
        let selected = (!human_bodygroups).then(|| (body_group / base) % nummodels);
        let first_model = i(bpo + 72) as usize;
        for model in 0..models {
            let modelindex = first_model + selected.unwrap_or(model) * 112;
            let body_mask = if human_bodygroups {
                let mut mask = 0u8;
                for body in 0..8usize {
                    if (body / base) % nummodels == model {
                        mask |= 1 << body;
                    }
                }
                mask
            } else {
                0xff
            };
            let (nummesh, meshindex) = (
                i(modelindex + 72).max(0) as usize,
                i(modelindex + 76) as usize,
            );
            let numverts = i(modelindex + 80).max(0) as usize;
            let (vinfoindex, vertindex) =
                (i(modelindex + 84) as usize, i(modelindex + 88) as usize);
            let numnorms = i(modelindex + 92).max(0) as usize;
            let normindex = i(modelindex + 100) as usize;
            let vert_base = vp.len();
            let norm_base = normals.len();
            parts.push(MdlPart {
                nummesh,
                meshindex,
                vert_base,
                norm_base,
                body_mask,
            });
            for v in 0..numverts {
                vp.push([
                    f(vertindex + v * 12),
                    f(vertindex + v * 12 + 4),
                    f(vertindex + v * 12 + 8),
                ]);
                vbone.push(*b.get(vinfoindex + v).unwrap_or(&0) as usize);
                vert_body_mask.push(body_mask);
            }
            for n in 0..numnorms {
                normals.push([
                    f(normindex + n * 12),
                    f(normindex + n * 12 + 4),
                    f(normindex + n * 12 + 8),
                ]);
            }
        }
    }
    debug_assert_eq!(vert_body_mask.len(), vp.len());
    let mut mouth_bones = mouth_controller.map(|mouth| {
        let mut descendants = vec![false; numbones];
        descendants[mouth.bone] = true;
        for bi in mouth.bone + 1..numbones {
            let parent = bmeta[bi].parent;
            descendants[bi] = parent >= 0 && descendants[parent as usize];
        }
        descendants
    });

    // Pick the requested sequence(s) and retain up to MAX_FRAMES authored
    // poses per clip. HMD8 stores one bone-local mesh and a compact composed
    // bone palette for each retained pose; every clip shares the same mesh,
    // palette stream, triangle section, and texture section.
    // seqdesc (176 B): numframes@56, animindex@124, seqgroup@156. mstudioanim per
    // bone is 12 B (6 u16 channel offsets) at animindex + bone*12; offset 0 = no
    // anim for that DOF (use the bone default). PS1 has no FPU, so hierarchy
    // evaluation is baked host-side and runtime interpolation stays fixed-point.
    let (numseq, seqindex) = (i(164), i(168) as usize);
    // Load external sequence-group files (<model>0N.mdl). Many HL monsters keep
    // most animations in seqgroup 1+; without these their clips bake as the bind
    // pose (splayed). seqgroup 0 lives in the main file.
    let numseqgroups = i(172).max(1) as usize;
    let seqgroup_files: Vec<Option<Vec<u8>>> = {
        let p = std::path::Path::new(path);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let dir = p.parent().unwrap_or_else(|| std::path::Path::new("."));
        (0..numseqgroups)
            .map(|g| {
                if g == 0 {
                    None
                } else {
                    std::fs::read(dir.join(format!("{stem}{g:02}.mdl"))).ok()
                }
            })
            .collect()
    };
    let ident: Mat34 = (
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        [0.0; 3],
    );
    let mut frames: Vec<Vec<[i16; 3]>> = Vec::new();
    let mut pose_bones: Vec<Vec<Mat34>> = Vec::new();
    let mut mouth_xforms: Vec<Mat34> = Vec::new();
    let mut frame_times: Vec<u8> = Vec::new();
    let mut clips: Vec<(u16, u16)> = Vec::with_capacity(specs.len());
    let mut clip_hold_quanta: Vec<u16> = Vec::with_capacity(specs.len());
    let mut cooked_clip_cache = HashMap::<(i32, Vec<usize>), (u16, u16)>::new();
    let mut animation_audit = String::from(
        "slot,sequence,label,source_frames,baked_frames,fps,looping,source_ticks,rms_vertex_error,max_vertex_error,sampled_source_frames,error_curve_rms_max\n",
    );
    // Resolve name-labeled specs against the MDL's sequence labels (32-byte
    // string at the head of each mstudioseqdesc).
    let seq_by_label = |label: &str| -> Result<i32, String> {
        for si in 0..numseq {
            let sd = seqindex + si as usize * 176;
            let raw = &b[sd..sd + 32];
            let end = raw.iter().position(|&c| c == 0).unwrap_or(32);
            if let Ok(n) = core::str::from_utf8(&raw[..end]) {
                if n.eq_ignore_ascii_case(label) {
                    return Ok(si);
                }
            }
        }
        Err(format!(
            "{}: requested sequence '{}' not found",
            path, label
        ))
    };
    for spec in specs {
        let seq_resolved = if spec.seq == -2 {
            seq_by_label(&spec.name)?
        } else {
            spec.seq
        };
        if seq_resolved < 0 || seq_resolved >= numseq {
            return Err(format!(
                "{}: requested sequence {} is outside 0..{}",
                path,
                seq_resolved,
                numseq.saturating_sub(1)
            ));
        }
        let spec = SeqSpec {
            seq: seq_resolved,
            name: spec.name.clone(),
            max_frames: spec.max_frames,
        };
        let spec = &spec;
        let (anim_b, animindex, numframes, fps, looping): (&[u8], usize, usize, f32, bool) =
            if spec.seq >= 0 && spec.seq < numseq {
                let sd = seqindex + spec.seq as usize * 176;
                let group = i(sd + 156) as usize;
                let ai = i(sd + 124) as usize;
                let nf = i(sd + 56).max(1) as usize;
                let fps = f(sd + 32);
                let looping = i(sd + 36) & 1 != 0; // STUDIO_LOOPING
                if group == 0 {
                    (&b, ai, nf, fps, looping)
                } else if let Some(Some(gd)) = seqgroup_files.get(group) {
                    (gd.as_slice(), ai, nf, fps, looping) // anim lives in <model>0N.mdl
                } else {
                    return Err(format!(
                        "{}: sequence {} requires missing group file {:02}",
                        path, spec.seq, group
                    ));
                }
            } else {
                unreachable!("sequence bounds validated above")
            };
        let nbake = mdl_baked_frame_count(animindex, numframes, spec.max_frames, looping);
        let clip_first = frames.len().min(u16::MAX as usize) as u16;
        let bake_pose = |sframe: usize| -> BakedMdlPose {
            let mut bones: Vec<Mat34> = Vec::with_capacity(numbones);
            let mut open_bones: Vec<Mat34> = Vec::with_capacity(numbones);
            for bi in 0..numbones {
                let bm = &bmeta[bi];
                let mut dof = bm.value;
                if animindex != 0 {
                    let at = animindex + bi * 12; // this bone's mstudioanim_t
                    for d in 0..6 {
                        let off = u16::from_le_bytes([anim_b[at + d * 2], anim_b[at + d * 2 + 1]])
                            as usize;
                        if off != 0 {
                            dof[d] = bm.value[d]
                                + anim_value(anim_b, at + off, sframe) as f32 * bm.scale[d];
                        }
                    }
                }
                let mut open_dof = dof;
                if let Some(mouth) = mouth_controller {
                    if bi == mouth.bone {
                        studio_controller_add(&mut dof, mouth.kind, mouth.start);
                        studio_controller_add(&mut open_dof, mouth.kind, mouth.end);
                    }
                }
                let local: Mat34 = (
                    quat_mat(angle_quat([dof[3], dof[4], dof[5]])),
                    [dof[0], dof[1], dof[2]],
                );
                let world = if bm.parent < 0 {
                    local
                } else {
                    concat(&bones[bm.parent as usize], &local)
                };
                bones.push(world);
                if mouth_controller.is_some() {
                    let open_local: Mat34 = (
                        quat_mat(angle_quat([open_dof[3], open_dof[4], open_dof[5]])),
                        [open_dof[0], open_dof[1], open_dof[2]],
                    );
                    let open_world = if bm.parent < 0 {
                        open_local
                    } else {
                        concat(&open_bones[bm.parent as usize], &open_local)
                    };
                    open_bones.push(open_world);
                }
            }
            let mouth_xform = mouth_controller.map(|mouth| {
                mdl_mouth_relative_xform(&bones[mouth.bone], &open_bones[mouth.bone], vertex_scale)
            });
            let mut fv: Vec<[i16; 3]> = Vec::with_capacity(vp.len());
            for v in 0..vp.len() {
                let p = apply(bones.get(vbone[v]).unwrap_or(&ident), vp[v]);
                fv.push([
                    quantize_mdl_coord(p[0], vertex_scale),
                    quantize_mdl_coord(p[2], vertex_scale),
                    quantize_mdl_coord(p[1], vertex_scale),
                ]);
            }
            // Viewmodels only: this is the table the runtime muzzle flash is
            // generated from (VM_MUZZLE_MODEL in the game). Take the first
            // line per model, which is frame 0 of its first cooked clip.
            if sframe == 0 && model_name.starts_with("v_") {
                if let Some((abone, aorg)) = muzzle_attachment {
                    if let Some(bone) = bones.get(abone) {
                        let m = apply(bone, aorg);
                        eprintln!(
                            "[muzzle] {} bone={} model=[{}, {}, {}]",
                            model_name,
                            abone,
                            quantize_mdl_coord(m[0], vertex_scale),
                            quantize_mdl_coord(m[2], vertex_scale),
                            quantize_mdl_coord(m[1], vertex_scale),
                        );
                    }
                }
            }
            BakedMdlPose {
                vertices: fv,
                bones,
                mouth_xform,
            }
        };
        // Motion-aware selection needs to inspect the complete source curve,
        // but only the selected palettes enter the PS1 payload. This is host-
        // side temporary memory and has no runtime RAM cost.
        let full = (0..numframes)
            .map(&bake_pose)
            .collect::<Vec<BakedMdlPose>>();
        let full_frames = full
            .iter()
            .map(|pose| pose.vertices.clone())
            .collect::<Vec<_>>();
        let sample_frames = mdl_motion_sample_frames(&full_frames, nbake, looping);
        let cache_key = (spec.seq, sample_frames.clone());
        let cached_clip = cooked_clip_cache.get(&cache_key).copied();
        if cached_clip.is_none() {
            for &source_frame in &sample_frames {
                frames.push(full[source_frame].vertices.clone());
                pose_bones.push(full[source_frame].bones.clone());
                frame_times.push(mdl_source_phase(source_frame, numframes, looping));
                if let Some(xform) = full[source_frame].mouth_xform {
                    mouth_xforms.push(xform);
                }
            }
        }
        if audit_out.is_some() {
            let (rms, max) =
                mdl_pose_reconstruction_error(&full_frames, &sample_frames, looping, vertex_scale);
            let sd = seqindex + spec.seq as usize * 176;
            let raw_label = &b[sd..sd + 32];
            let label_end = raw_label.iter().position(|&byte| byte == 0).unwrap_or(32);
            let label = String::from_utf8_lossy(&raw_label[..label_end]).replace(',', "_");
            let source_ticks = if numframes <= 1 || !fps.is_finite() || fps <= 0.0 {
                1
            } else {
                (((numframes - 1) as f32 * 20.0 / fps).ceil() as usize).max(1)
            };
            let samples = sample_frames
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(";");
            let distinct_frames = if looping {
                numframes.saturating_sub(1).max(1)
            } else {
                numframes.max(1)
            };
            // Always retain the original 1..16 tuning curve, and extend it
            // when this clip explicitly opts into a larger pose budget.
            let error_curve_limit = distinct_frames.min(spec.max_frames.max(16));
            let error_curve = (1..=error_curve_limit)
                .map(|candidate| {
                    let candidate_frames =
                        mdl_motion_sample_frames(&full_frames, candidate, looping);
                    let (candidate_rms, candidate_max) = mdl_pose_reconstruction_error(
                        &full_frames,
                        &candidate_frames,
                        looping,
                        vertex_scale,
                    );
                    format!("{candidate}:{candidate_rms:.4}:{candidate_max:.4}")
                })
                .collect::<Vec<_>>()
                .join(";");
            animation_audit.push_str(&format!(
                "{},{},{},{},{},{:.3},{},{},{:.4},{:.4},{},{}\n",
                clips.len(),
                spec.seq,
                label,
                numframes,
                nbake,
                fps,
                usize::from(looping),
                source_ticks,
                rms,
                max,
                samples,
                error_curve,
            ));
        }
        let cooked_clip = cached_clip.unwrap_or((
            clip_first,
            nbake.min(MDL_CLIP_FRAME_COUNT_MASK as usize) as u16,
        ));
        cooked_clip_cache.entry(cache_key).or_insert(cooked_clip);
        clips.push(cooked_clip);
        clip_hold_quanta.push(mdl_sequence_hold_quanta(numframes, fps));
    }
    let mut floor_shifts = vec![0i16; frames.len()];
    if floor_anchor_frames {
        // Seated models (sitting scientist) bake ONLY seated poses. GoldSrc
        // renders them at the authored entity origin (the chair SEAT) with the
        // sequence's own root offset: pelvis near the origin, feet dipping
        // BELOW it toward the floor. Any min-Y anchoring (even the constant
        // clip-0 baseline) still raises clip 0's lowest vertex to the seat and
        // floats the figure a whole seat-height above the chair (AM-01), so
        // seated models skip anchoring entirely. Detect by the first clip's
        // sequence label starting with "sit"; everything else keeps the 5
        // canonical clips (idle/walk/attack/pain/death) feet-planted.
        let first_seq = specs
            .first()
            .map(|s| {
                if s.seq == -2 {
                    seq_by_label(&s.name).unwrap_or(-1)
                } else {
                    s.seq
                }
            })
            .unwrap_or(-1);
        let seated = first_seq >= 0 && first_seq < numseq && {
            let sd = seqindex + first_seq as usize * 176;
            let raw = &b[sd..sd + 32];
            let end = raw.iter().position(|&c| c == 0).unwrap_or(32);
            core::str::from_utf8(&raw[..end])
                .map(|n| n.to_ascii_lowercase().starts_with("sit"))
                .unwrap_or(false)
        };
        if !seated {
            floor_shifts = floor_anchor_mdl_frames(&mut frames, &clips, 5);
        }
    }
    for (xform, shift) in mouth_xforms.iter_mut().zip(floor_shifts.iter().copied()) {
        mdl_adjust_xform_for_floor_shift(xform, shift);
    }

    let mut tri_idx: Vec<u16> = Vec::new();
    let mut tri_tex: Vec<u16> = Vec::new();
    let mut tri_uv: Vec<u8> = Vec::new();
    let mut tri_norm: Vec<[i8; 3]> = Vec::new();
    let mut tri_body_mask: Vec<u8> = Vec::new();
    let mut texs: Vec<CookedTex> = Vec::new();
    let mut slot_of: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();

    for part in &parts {
        for m in 0..part.nummesh {
            let me = part.meshindex + m * 20;
            let triindex = i(me + 4) as usize;
            let skinref = i(me + 8) as usize;
            // Resolve texture families into visibility masks. Scientist body
            // values 2/6 are Luther and use family 1 for the two hand refs;
            // identical mappings collapse back into one ordinary 0xff run.
            let mut texture_masks: Vec<(usize, u8)> = Vec::new();
            for body in 0..8usize {
                let body_bit = 1u8 << body;
                if part.body_mask & body_bit == 0 {
                    continue;
                }
                let family = if can_skins {
                    body % numskinfamilies.max(1)
                } else if scientist_bodygroups && body % 4 == 2 && numskinfamilies > 1 {
                    1
                } else {
                    0
                };
                let texid = if skinref < numskinref && family < numskinfamilies {
                    th16(skinindex + (family * numskinref + skinref) * 2).max(0) as usize
                } else {
                    0
                };
                if let Some((_, mask)) = texture_masks.iter_mut().find(|(id, _)| *id == texid) {
                    *mask |= body_bit;
                } else {
                    texture_masks.push((texid, body_bit));
                }
            }
            for (texid, body_mask) in texture_masks {
                let to = textureindex + texid * 80;
                let flags = ti(to + 64);
                let (tw, th, tpix) = (
                    ti(to + 68) as usize,
                    ti(to + 72) as usize,
                    ti(to + 76) as usize,
                );
                let slot = *slot_of.entry(texid).or_insert_with(|| {
                    texs.push(cook_mdl_tex(tb, tpix, tw.max(1), th.max(1)));
                    texs.len() - 1
                });
                let (fw, fh) = (texs[slot].w as f32, texs[slot].h as f32);
                let mut o = triindex;
                loop {
                    let cmd = h16(o) as i32;
                    o += 2;
                    if cmd == 0 {
                        break;
                    }
                    let (n, fan) = (cmd.unsigned_abs() as usize, cmd < 0);
                    let mut s: Vec<(u16, u8, u8)> = Vec::with_capacity(n);
                    for _ in 0..n {
                        let local_vi = h16(o).max(0) as usize;
                        let local_ni = h16(o + 2).max(0) as usize;
                        let vi = (part.vert_base + local_vi).min(u16::MAX as usize) as u16;
                        let (ss, tt) = (h16(o + 4) as f32, h16(o + 6) as f32);
                        o += 8;
                        let (u, vv) = if flags & STUDIO_NF_CHROME != 0 {
                            let n = normals
                                .get(part.norm_base + local_ni)
                                .copied()
                                .unwrap_or([0.0, 0.0, 1.0]);
                            chrome_uv(n, fw, fh)
                        } else {
                            (
                                (ss * fw / tw.max(1) as f32).clamp(0.0, 255.0) as u8,
                                (tt * fh / th.max(1) as f32).clamp(0.0, 255.0) as u8,
                            )
                        };
                        s.push((vi, u, vv));
                    }
                    for k in 0..n.saturating_sub(2) {
                        let (a, bb, c) = if fan {
                            (0, k + 1, k + 2)
                        } else if k % 2 == 0 {
                            (k, k + 1, k + 2)
                        } else {
                            (k + 1, k, k + 2)
                        };
                        // Reversed winding (c,b,a) to match the BSP cook's
                        // Y/Z swap, so runtime backface culling keeps fronts.
                        let (va, vb, vc) = (s[a], s[bb], s[c]);
                        tri_idx.extend_from_slice(&[vc.0, vb.0, va.0]);
                        tri_tex.push(slot as u16);
                        tri_uv.extend_from_slice(&[vc.1, vc.2, vb.1, vb.2, va.1, va.2]);
                        let normal = frames
                            .first()
                            .map(|base| mdl_face_normal_i8(base, vc.0, vb.0, va.0))
                            .unwrap_or([0; 3]);
                        // GoldSrc's FLATSHADE textures (notably the shotgun's
                        // chrome glove and barrel) deliberately ignore the
                        // directional term. A zero packed normal retains that
                        // fixed illumination without another per-face flag.
                        tri_norm.push(if packed_normals && flags & STUDIO_NF_FLATSHADE != 0 {
                            [0; 3]
                        } else {
                            normal
                        });
                        tri_body_mask.push(body_mask);
                    }
                }
            }
        }
    }

    let source_n_verts = vp.len();
    if source_n_verts > MDL_RUNTIME_VERTEX_LIMIT
        || (simplify_target > 0 && source_n_verts > simplify_target)
    {
        let source_n_tris = tri_idx.len() / 3;
        let simplified = simplify_mdl_animated_mesh(
            &frames,
            &vbone,
            &tri_idx,
            &tri_tex,
            &tri_uv,
            &tri_norm,
            if simplify_target > 0 {
                simplify_target
            } else {
                MDL_SIMPLIFIED_VERTEX_TARGET
            },
        );
        let simplified_n_verts = simplified.frames[0].len();
        let simplified_n_tris = simplified.tri_idx.len() / 3;
        eprintln!(
            "simplified oversized studio mesh: {} -> {} verts, {} -> {} tris (bone-local grid {} = {:.2} source units)",
            source_n_verts,
            simplified_n_verts,
            source_n_tris,
            simplified_n_tris,
            simplified.grid_size,
            simplified.grid_size as f32 / vertex_scale.max(1) as f32,
        );
        let mut sums = vec![[0.0f64; 3]; simplified_n_verts];
        let mut counts = vec![0usize; simplified_n_verts];
        for (old, &cluster) in simplified.remap.iter().enumerate() {
            for axis in 0..3 {
                sums[cluster][axis] += vp[old][axis] as f64;
            }
            counts[cluster] += 1;
        }
        vp = sums
            .into_iter()
            .zip(counts)
            .map(|(sum, count)| {
                let divisor = count.max(1) as f64;
                [
                    (sum[0] / divisor) as f32,
                    (sum[1] / divisor) as f32,
                    (sum[2] / divisor) as f32,
                ]
            })
            .collect();
        vbone = simplified.bones;
        frames = simplified.frames;
        tri_idx = simplified.tri_idx;
        tri_tex = simplified.tri_tex;
        tri_uv = simplified.tri_uv;
        tri_norm = simplified.tri_norm;
        // Human bodygroup models are below the runtime limit. Oversized
        // creatures retain the legacy all-visible topology after welding.
        vert_body_mask = vec![0xff; simplified_n_verts];
        tri_body_mask = vec![0xff; simplified_n_tris];
        mouth_bones = None;
        mouth_xforms.clear();
    }

    if camera_locked && !frames.is_empty() {
        let never = viewmodel_never_front_facing(&frames, &tri_idx, &tri_tex, vertex_scale);
        let dropped = never.iter().filter(|&&drop| drop).count();
        if dropped > 0 {
            let mut idx = Vec::with_capacity((never.len() - dropped) * 3);
            let mut tex = Vec::with_capacity(never.len() - dropped);
            let mut uv = Vec::with_capacity((never.len() - dropped) * 6);
            let mut norm = Vec::with_capacity(never.len() - dropped);
            let mut body = Vec::with_capacity(never.len() - dropped);
            for (tri, &drop) in never.iter().enumerate() {
                if drop {
                    continue;
                }
                idx.extend_from_slice(&tri_idx[tri * 3..tri * 3 + 3]);
                tex.push(tri_tex[tri]);
                uv.extend_from_slice(&tri_uv[tri * 6..tri * 6 + 6]);
                norm.push(tri_norm[tri]);
                body.push(tri_body_mask[tri]);
            }
            eprintln!(
                "viewmodel cull: {} -> {} tris ({} face away in all {} baked poses)",
                never.len(),
                never.len() - dropped,
                dropped,
                frames.len(),
            );
            // ponytail: the orphaned vertices stay. They cost six bytes each and
            // removing them would have to renumber the contiguous per-bone
            // ranges the runtime walks; revisit if the pool ever needs it.
            tri_idx = idx;
            tri_tex = tex;
            tri_uv = uv;
            tri_norm = norm;
            tri_body_mask = body;
        }
    }

    let n_tris = tri_idx.len() / 3;
    // Keep the legacy count expression for ordinary models: simplification is
    // deliberately absent from their output path, including all frame bytes.
    let n_verts = if source_n_verts > MDL_RUNTIME_VERTEX_LIMIT
        || (simplify_target > 0 && source_n_verts > simplify_target)
    {
        frames[0].len()
    } else {
        source_n_verts
    };
    // Same hard guarantee as the map cook: the runtime model walk trusts
    // every corner index and skips per-tri bounds checks.
    for &vi in &tri_idx {
        assert!((vi as usize) < n_verts, "model tri corner out of range");
    }
    assert_eq!(
        vert_body_mask.len(),
        n_verts,
        "model vertex body-mask drift"
    );
    assert_eq!(
        tri_body_mask.len(),
        n_tris,
        "model triangle body-mask drift"
    );
    assert_eq!(
        pose_bones.len(),
        frames.len(),
        "HMD8 pose/bone stream drift"
    );
    assert_eq!(vp.len(), n_verts, "HMD8 bone-local vertex stream drift");
    assert_eq!(vbone.len(), n_verts, "HMD8 vertex-bone stream drift");

    // Reorder vertices into stable bone/body ranges. Triangles are remapped
    // once at cook time; runtime can then change the GTE matrix once per range
    // and feed contiguous vertices directly to RTPT.
    let mut vertex_order = (0..n_verts).collect::<Vec<_>>();
    vertex_order.sort_by_key(|&vertex| (vbone[vertex], vert_body_mask[vertex], vertex));
    let mut vertex_remap = vec![0u16; n_verts];
    for (new, &old) in vertex_order.iter().enumerate() {
        vertex_remap[old] = new as u16;
    }
    for index in &mut tri_idx {
        *index = vertex_remap[*index as usize];
    }

    let mut used_bones = vertex_order
        .iter()
        .map(|&vertex| vbone[vertex])
        .collect::<Vec<_>>();
    used_bones.extend(source_hitboxes.iter().map(|hitbox| hitbox.bone));
    used_bones.sort_unstable();
    used_bones.dedup();
    if used_bones.len() > u16::MAX as usize {
        return Err("HMD8 used-bone count exceeds u16".to_string());
    }
    let mut compact_bone = vec![u16::MAX; numbones.max(1)];
    for (compact, &source) in used_bones.iter().enumerate() {
        if source >= compact_bone.len() {
            return Err(format!("HMD8 vertex references missing bone {source}"));
        }
        compact_bone[source] = compact as u16;
    }

    #[derive(Clone, Copy)]
    struct Hmd7Range {
        first: u16,
        count: u16,
        bone: u16,
        body_mask: u8,
        flags: u8,
    }
    let mut ranges = Vec::<Hmd7Range>::new();
    for (new, &old) in vertex_order.iter().enumerate() {
        let bone = compact_bone[vbone[old]];
        let body_mask = vert_body_mask[old];
        let mouth = mouth_bones
            .as_ref()
            .and_then(|bones| bones.get(vbone[old]))
            .copied()
            .unwrap_or(false);
        let flags = if mouth { HMD7_RANGE_MOUTH } else { 0 };
        if let Some(last) = ranges.last_mut() {
            if last.bone == bone
                && last.body_mask == body_mask
                && last.flags == flags
                && last.first as usize + last.count as usize == new
            {
                last.count = last.count.saturating_add(1);
                continue;
            }
        }
        ranges.push(Hmd7Range {
            first: new as u16,
            count: 1,
            bone,
            body_mask,
            flags,
        });
    }
    if ranges.len() > u16::MAX as usize {
        return Err("HMD8 bone-range count exceeds u16".to_string());
    }

    let has_body_masks = human_bodygroups || can_skins;
    let has_mouth = mouth_bones.is_some() && mouth_xforms.len() == frames.len();
    let has_frame_times = frame_times.len() == frames.len();
    let has_hitboxes = !source_hitboxes.is_empty();
    if source_hitboxes.len() > u16::MAX as usize {
        return Err("HMD8 hitbox count exceeds u16".to_string());
    }
    if packed_normals && has_body_masks {
        return Err("packed HMD8 normals overlap body-mask triangle metadata".to_string());
    }
    if packed_normals && full_normals {
        return Err("HMD8 cannot use packed and full normals together".to_string());
    }
    let hmd_flags = (if has_body_masks {
        HMD_FLAG_BODY_MASKS
    } else {
        0
    }) | (if has_mouth { HMD_FLAG_MOUTH } else { 0 })
        | (if packed_normals {
            HMD_FLAG_PACKED_NORMALS
        } else {
            0
        })
        | (if full_normals { HMD_FLAG_I8_NORMALS } else { 0 })
        | (if has_hitboxes { HMD_FLAG_HITBOXES } else { 0 });
    let hmd_flags = hmd_flags
        | (if has_frame_times {
            HMD_FLAG_FRAME_TIMES
        } else {
            0
        })
        | HMD_FLAG_ALIGNED_MODEL_DATA
        | HMD_FLAG_VERTEX_SOA;

    let mut model_data = Vec::new();
    for range in &ranges {
        model_data.extend_from_slice(&range.first.to_le_bytes());
        model_data.extend_from_slice(&range.count.to_le_bytes());
        model_data.extend_from_slice(&range.bone.to_le_bytes());
        model_data.push(range.body_mask);
        model_data.push(range.flags);
    }
    debug_assert_eq!(model_data.len(), ranges.len() * HMD7_RANGE_BYTES);
    let local_vertices = vertex_order
        .iter()
        .map(|&old| mdl_bone_local_vertex(vp[old], vertex_scale))
        .collect::<Vec<_>>();
    // Keep XY as an aligned u32 stream so each GTE vertex costs one LW rather
    // than an LWL/LWR pair. Z follows as aligned i16; total size is unchanged.
    for vertex in &local_vertices {
        model_data.extend_from_slice(&vertex[0].to_le_bytes());
        model_data.extend_from_slice(&vertex[1].to_le_bytes());
    }
    for vertex in &local_vertices {
        model_data.extend_from_slice(&vertex[2].to_le_bytes());
    }
    let pose_bytes_start = model_data.len();
    for (frame, bones) in pose_bones.iter().enumerate() {
        for &source_bone in &used_bones {
            append_hmd8_bone_xform(
                &mut model_data,
                bones.get(source_bone).unwrap_or(&ident),
                vertex_scale,
                floor_shifts.get(frame).copied().unwrap_or(0),
            );
        }
    }
    debug_assert_eq!(
        model_data.len() - pose_bytes_start,
        frames.len() * used_bones.len() * HMD8_AFFINE_BYTES
    );
    let mut palette_squared = 0f64;
    let mut palette_components = 0usize;
    let mut palette_max_squared = 0i64;
    let static_vertices_off = ranges.len() * HMD7_RANGE_BYTES;
    let static_vertices_z_off = static_vertices_off + n_verts * 4;
    for (frame, expected) in frames.iter().enumerate() {
        for (new, &old) in vertex_order.iter().enumerate() {
            let vertex_off = static_vertices_off + new * 4;
            let vertex_z_off = static_vertices_z_off + new * 2;
            let vertex = [
                i16::from_le_bytes([model_data[vertex_off], model_data[vertex_off + 1]]) as i32,
                i16::from_le_bytes([model_data[vertex_off + 2], model_data[vertex_off + 3]]) as i32,
                i16::from_le_bytes([model_data[vertex_z_off], model_data[vertex_z_off + 1]]) as i32,
            ];
            let bone = compact_bone[vbone[old]] as usize;
            let affine_off =
                pose_bytes_start + (frame * used_bones.len() + bone) * HMD8_AFFINE_BYTES;
            let read_i16 = |offset: usize| {
                i16::from_le_bytes([model_data[offset], model_data[offset + 1]]) as i32
            };
            let read_q11_pair = |offset: usize| {
                let a = model_data[offset] as u16 | ((model_data[offset + 1] as u16 & 0x0f) << 8);
                let b =
                    (model_data[offset + 1] as u16 >> 4) | ((model_data[offset + 2] as u16) << 4);
                let sign = |raw: u16| {
                    let code = if raw & 0x0800 != 0 {
                        (raw | 0xf000) as i16
                    } else {
                        raw as i16
                    };
                    hmd8_q11_decode(code) as i32
                };
                (sign(a), sign(b))
            };
            let mut rotation = [0i32; 9];
            for pair in 0..4 {
                let (a, b) = read_q11_pair(affine_off + pair * 3);
                rotation[pair * 2] = a;
                rotation[pair * 2 + 1] = b;
            }
            let raw = model_data[affine_off + 12] as u16
                | ((model_data[affine_off + 13] as u16 & 0x0f) << 8);
            let code = if raw & 0x0800 != 0 {
                (raw | 0xf000) as i16
            } else {
                raw as i16
            };
            rotation[8] = hmd8_q11_decode(code) as i32;
            let mut reconstructed = [0i32; 3];
            for row in 0..3 {
                reconstructed[row] = ((rotation[row * 3] * vertex[0]
                    + rotation[row * 3 + 1] * vertex[1]
                    + rotation[row * 3 + 2] * vertex[2])
                    >> 12)
                    + read_i16(affine_off + 14 + row * 2);
            }
            let mut vertex_squared = 0i64;
            for axis in 0..3 {
                let error = expected[old][axis] as i64 - reconstructed[axis] as i64;
                vertex_squared += error * error;
                palette_squared += (error * error) as f64;
                palette_components += 1;
            }
            palette_max_squared = palette_max_squared.max(vertex_squared);
        }
    }
    let palette_rms =
        (palette_squared / palette_components.max(1) as f64).sqrt() / vertex_scale.max(1) as f64;
    let palette_max = (palette_max_squared as f64).sqrt() / vertex_scale.max(1) as f64;
    if has_mouth {
        for xform in &mouth_xforms {
            append_mouth_xform(&mut model_data, xform);
        }
    }
    // HitboxRec: compact bone u16, cooked/scaled bbmin + bbmax i16[3].
    // The boxes stay bone-local and therefore track every retained pose.
    for hitbox in &source_hitboxes {
        model_data.extend_from_slice(&compact_bone[hitbox.bone].to_le_bytes());
        for component in mdl_bone_local_vertex(hitbox.bbmin, vertex_scale) {
            model_data.extend_from_slice(&component.to_le_bytes());
        }
        for component in mdl_bone_local_vertex(hitbox.bbmax, vertex_scale) {
            model_data.extend_from_slice(&component.to_le_bytes());
        }
    }
    debug_assert_eq!(
        source_hitboxes.len() * HMD7_HITBOX_BYTES,
        model_data.len()
            - ranges.len() * HMD7_RANGE_BYTES
            - n_verts * 6
            - frames.len() * used_bones.len() * HMD8_AFFINE_BYTES
            - if has_mouth {
                frames.len() * HMD_MOUTH_AFFINE_BYTES
            } else {
                0
            }
    );

    let mut o: Vec<u8> = Vec::new();
    o.extend_from_slice(b"HMD8");
    o.extend_from_slice(&(n_verts as u32).to_le_bytes());
    o.extend_from_slice(&(n_tris as u32).to_le_bytes());
    // The split runtime does not consume the geometry header's texture count.
    // Keep it in the low half and store the hitbox count in the high half.
    let texture_hitbox_counts =
        (texs.len() as u32 & 0xffff) | ((source_hitboxes.len() as u32 & 0xffff) << 16);
    o.extend_from_slice(&texture_hitbox_counts.to_le_bytes());
    o.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    o.extend_from_slice(&(clips.len() as u32).to_le_bytes());
    o.extend_from_slice(&(model_data.len() as u32).to_le_bytes());
    o.extend_from_slice(&mdl_local_to_world_q12(vertex_scale).to_le_bytes());
    o.extend_from_slice(&hmd_flags.to_le_bytes());
    o.extend_from_slice(&(used_bones.len() as u16).to_le_bytes());
    o.extend_from_slice(&(ranges.len() as u16).to_le_bytes());
    for ((first, count), hold_quanta) in clips.iter().zip(clip_hold_quanta.iter()) {
        let (packed_first, packed_count) = mdl_pack_clip(*first, *count, *hold_quanta);
        o.extend_from_slice(&packed_first.to_le_bytes());
        o.extend_from_slice(&packed_count.to_le_bytes());
    }
    o.extend_from_slice(&frame_times);
    // Runtime geometry begins on a word boundary. The range table is a
    // multiple of eight bytes, so the following XY stream remains LW-aligned.
    while o.len() & 3 != 0 {
        o.push(0);
    }
    o.extend_from_slice(&model_data);
    for t in 0..n_tris {
        let ib = t * 3;
        o.extend_from_slice(&tri_idx[ib].to_le_bytes());
        o.extend_from_slice(&tri_idx[ib + 1].to_le_bytes());
        o.extend_from_slice(&tri_idx[ib + 2].to_le_bytes());
        o.extend_from_slice(&tri_tex[t].to_le_bytes());
        o.extend_from_slice(&tri_uv[t * 6..t * 6 + 6]);
        if full_normals {
            let n = tri_norm.get(t).copied().unwrap_or([0; 3]);
            o.extend_from_slice(&[n[0] as u8, n[1] as u8, n[2] as u8, tri_body_mask[t]]);
            o.extend_from_slice(&0u16.to_le_bytes());
        } else if packed_normals {
            let n = tri_norm.get(t).copied().unwrap_or([0; 3]);
            o.extend_from_slice(&mdl_pack_normal_555(n).to_le_bytes());
        } else {
            o.extend_from_slice(&[tri_body_mask[t], 0]);
        }
    }
    let texture_chunk = tex_out.map(|_| build_texture_chunk(&texs));
    if tex_out.is_none() {
        append_texture_blob(&mut o, &texs);
    }
    std::fs::write(out, &o).map_err(|e| format!("write {}: {}", out, e))?;
    if let Some(audit_out) = audit_out {
        let mut hmd7_audit = String::new();
        for (line, row) in animation_audit.lines().enumerate() {
            if line == 0 {
                hmd7_audit.push_str(row);
                hmd7_audit.push_str(",hmd7_palette_rms,hmd7_palette_max\n");
            } else {
                hmd7_audit.push_str(row);
                hmd7_audit.push_str(&format!(",{palette_rms:.4},{palette_max:.4}\n"));
            }
        }
        std::fs::write(audit_out, &hmd7_audit)
            .map_err(|e| format!("write {}: {}", audit_out, e))?;
    }
    let tex_kb = if let (Some(tex_out), Some(texture_chunk)) = (tex_out, texture_chunk.as_ref()) {
        std::fs::write(tex_out, texture_chunk).map_err(|e| format!("write {}: {}", tex_out, e))?;
        Some(texture_chunk.len() / 1024)
    } else {
        None
    };
    let seq_desc = specs
        .iter()
        .map(|s| format!("{}:{}", s.seq, s.max_frames))
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "cooked {} -> {}{} ({} seqs {}, {} clips, {} frames, {} verts, {} tris, {} texs, {} KB resident{}, HMD8 palette RMS {:.3} max {:.3})",
        path,
        out,
        tex_out.map(|p| format!(" + {}", p)).unwrap_or_default(),
        if full_normals {
            "HMD8+i8 normals"
        } else if packed_normals {
            "HMD8+packed normals"
        } else {
            "HMD8"
        },
        seq_desc,
        clips.len(),
        frames.len(),
        n_verts,
        n_tris,
        texs.len(),
        o.len() / 1024,
        tex_kb
            .map(|kb| format!(", {} KB textures", kb))
            .unwrap_or_default(),
        palette_rms,
        palette_max,
    );
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--gen-testmap") {
        match args.get(2) {
            Some(out) => {
                if let Err(e) = testmap::generate(out) {
                    eprintln!("{}", e);
                    exit(2);
                }
                return;
            }
            None => {
                eprintln!("usage: hl-bsp --gen-testmap <out.bsp>");
                exit(2);
            }
        }
    }
    if let Some(mode) = args.get(1).and_then(|flag| mdl_cook_mode(flag)) {
        match (args.get(2), args.get(3)) {
            (Some(inp), Some(out)) => {
                let seq_text = args.get(4).map(|s| s.as_str()).unwrap_or("0");
                let tex_out = args.get(5).map(|s| s.as_str());
                let audit_out = args.get(6).map(|s| s.as_str());
                let specs = match parse_seq_specs(seq_text) {
                    Ok(specs) => specs,
                    Err(e) => {
                        eprintln!("{}", e);
                        exit(2);
                    }
                };
                if let Err(e) = cook_mdl(
                    inp,
                    out,
                    tex_out,
                    &specs,
                    parse_body_group(seq_text),
                    mode.full_normals,
                    mode.packed_normals,
                    mode.vertex_scale,
                    mode.simplify_target,
                    mode.camera_locked,
                    audit_out,
                ) {
                    eprintln!("{}", e);
                    exit(1);
                }
                return;
            }
            _ => {
                eprintln!(
                    "usage: hl-bsp --mdl7-vm|--mdl7|--mdl7-lean|--mdl7-normals <in.mdl> <out.hlmdl> [seq|seq:max_frames,...] [out.hltx] [animation-audit.csv]"
                );
                exit(2);
            }
        }
    }
    // `--cook <in.bsp> <out.hlm> [out.hltx]` cooks; otherwise `<map.bsp>` reports.
    if args.get(1).map(|s| s.as_str()) == Some("--cook") {
        match (args.get(2), args.get(3)) {
            (Some(inp), Some(out)) => {
                if let Err(e) = cook(inp, out, args.get(4).map(|s| s.as_str())) {
                    eprintln!("{}", e);
                    exit(1);
                }
                return;
            }
            _ => {
                eprintln!("usage: hl-bsp --cook <in.bsp> <out.hlm> [out.hltx]");
                exit(2);
            }
        }
    }
    let path = match args.get(1) {
        Some(p) => p.clone(),
        None => {
            eprintln!("usage: hl-bsp <map.bsp>  |  hl-bsp --cook <in.bsp> <out.hlm> [out.hltx]");
            exit(2);
        }
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {}: {}", path, e);
            exit(1);
        }
    };
    match Bsp::parse(&bytes) {
        Ok(bsp) => report(&path, &bsp),
        Err(e) => {
            eprintln!("{}: {}", path, e);
            exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_i32(buf: &mut Vec<u8>, v: i32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn quad_pair_tag_matches_runtime_resolved_palette_colours() {
        let tri_idx = [0, 4, 2, 4, 1, 2];
        let tri_tex = [7, 7];
        let tri_uv = [10, 10, 40, 40, 20, 20, 40, 40, 11, 11, 20, 20];
        // Shared corners deliberately use different palette indices with the
        // same resolved RGB. This is legal at runtime and must be tagged by
        // the cooker or the fast path would silently miss a GT4 pair.
        let light_idx = [0, 1, 2, 3, 4, 5];
        let light_pal = [
            (10, 10, 10),
            (40, 40, 40),
            (20, 20, 20),
            (40, 40, 40),
            (11, 11, 11),
            (20, 20, 20),
        ];
        assert_eq!(
            quad_pair_code(0, 1, &tri_idx, &tri_tex, &tri_uv, &light_idx, &light_pal,),
            Some(0x20)
        );
    }

    #[test]
    fn geometry_pair_survives_an_authored_diagonal_uv_seam() {
        let tri_idx = [0, 4, 2, 4, 1, 2];
        let tri_tex = [7, 7];
        let tri_uv = [10, 10, 40, 40, 20, 20, 41, 40, 11, 11, 20, 20];
        let light_idx = [0, 1, 2, 1, 3, 2];
        let light_pal = [(10, 10, 10), (40, 40, 40), (20, 20, 20), (11, 11, 11)];
        assert_eq!(
            quad_pair_code(0, 1, &tri_idx, &tri_tex, &tri_uv, &light_idx, &light_pal),
            None
        );
        assert_eq!(
            geometry_quad_pair_code(0, 1, &tri_idx, &tri_tex),
            Some(0x20)
        );
    }

    #[test]
    fn retail_route_phrase_decode_matches_goldsrc() {
        // c0a0e node zero: destinations 0..3 are direct, destinations 4..6
        // all take node 3 as their first hop.
        let (encoded, decoded) = retail_route_row(&[0xfc, 0x02, 0x03], 0, 7, 0).expect("route row");
        assert_eq!(encoded, [0xfc, 0x02, 0x03]);
        assert_eq!(decoded, [0, 1, 2, 3, 3, 3, 3]);
        assert!(retail_route_row(&[0x02], 0, 7, 0).is_err());
    }

    #[test]
    fn retail_nav_parser_repacks_and_deduplicates_human_routes() {
        let n = 2usize;
        let l = 2usize;
        let route = [0xfeu8]; // both destinations are direct
        let total = 4
            + RETAIL_GRAPH_BYTES
            + n * RETAIL_NODE_BYTES
            + l * RETAIL_LINK_BYTES
            + n * RETAIL_DIST_BYTES
            + route.len();
        let mut data = vec![0u8; total];
        data[0..4].copy_from_slice(&RETAIL_GRAPH_VERSION.to_le_bytes());
        data[12..16].copy_from_slice(&1i32.to_le_bytes()); // routing complete
        data[28..32].copy_from_slice(&(n as i32).to_le_bytes());
        data[32..36].copy_from_slice(&(l as i32).to_le_bytes());
        data[36..40].copy_from_slice(&(route.len() as i32).to_le_bytes());
        data[8388..8392].copy_from_slice(&0i32.to_le_bytes());

        let nodes_off = 4 + RETAIL_GRAPH_BYTES;
        for i in 0..n {
            let o = nodes_off + i * RETAIL_NODE_BYTES;
            let origin = [i as f32 * 64.0, 0.0, 0.0];
            for (axis, value) in origin.into_iter().enumerate() {
                data[o + axis * 4..o + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
                data[o + 12 + axis * 4..o + 16 + axis * 4].copy_from_slice(&value.to_le_bytes());
            }
            data[o + 28..o + 32].copy_from_slice(&(NAV_NODE_LAND as i32).to_le_bytes());
            data[o + 32..o + 36].copy_from_slice(&1i32.to_le_bytes());
            data[o + 36..o + 40].copy_from_slice(&(i as i32).to_le_bytes());
            data[o + RETAIL_NODE_HUMAN_DOOR_ROUTE..o + RETAIL_NODE_HUMAN_DOOR_ROUTE + 4]
                .copy_from_slice(&0i32.to_le_bytes());
        }
        let links_off = nodes_off + n * RETAIL_NODE_BYTES;
        for (i, (source, dest)) in [(0i32, 1i32), (1, 0)].into_iter().enumerate() {
            let o = links_off + i * RETAIL_LINK_BYTES;
            data[o..o + 4].copy_from_slice(&source.to_le_bytes());
            data[o + 4..o + 8].copy_from_slice(&dest.to_le_bytes());
            data[o + 16..o + 20].copy_from_slice(&RETAIL_LINK_HUMAN.to_le_bytes());
        }
        let route_off = links_off + l * RETAIL_LINK_BYTES + n * RETAIL_DIST_BYTES;
        data[route_off..route_off + route.len()].copy_from_slice(&route);

        let nav = parse_retail_nav(&data, &[], &[], 1.0).expect("retail nav");
        assert_eq!(nav.nodes.len(), 2);
        assert_eq!(nav.routes.as_deref(), Some(route.as_slice()));
        assert_eq!(nav.nodes[0].route_offset, 0);
        assert_eq!(nav.nodes[1].route_offset, 0, "identical rows share storage");
        assert_eq!(nav.nodes[1].origin, [64, 0, 0]);
    }

    #[test]
    fn world_pvs_count_comes_from_dmodel_not_leaf_lump() {
        let mut models = vec![0u8; SZ_MODEL];
        models[SZ_MODEL_VISLEAFS..SZ_MODEL_VISLEAFS + 4].copy_from_slice(&856i32.to_le_bytes());

        assert_eq!(world_visleaf_count(&models, 1326).unwrap(), 856);
        assert!(world_visleaf_count(&models, 800).is_err());
    }

    #[test]
    fn hlmd_leaf_counts_pack_without_growing_bsp_header() {
        let packed = pack_leaf_counts(1326, 856).unwrap();
        assert_eq!(packed & 0xffff, 1326);
        assert_eq!(packed >> 16, 856);
        assert!(pack_leaf_counts(u16::MAX as usize + 1, 856).is_err());
    }

    #[test]
    fn q5_plane_distance_preserves_c1a1f_ramp_fraction_without_growing_record() {
        let mut planes = Vec::with_capacity(SZ_PLANE);
        planes.extend_from_slice(&0.0f32.to_le_bytes());
        planes.extend_from_slice(&0.5002776f32.to_le_bytes());
        planes.extend_from_slice(&0.86586505f32.to_le_bytes());
        planes.extend_from_slice(&(-80.48697f32).to_le_bytes());
        planes.extend_from_slice(&0i32.to_le_bytes()); // BSP plane type

        let (normal, dist_q5) = plane_rec_q5(&planes, 0, 1.0);
        assert_eq!(normal, [0, 14188, 8197]);
        assert_eq!(
            normal.map(|value| {
                let value = value as i32;
                if value < 0 {
                    -((-value) >> 2)
                } else {
                    value >> 2
                }
            }),
            [0, 3547, 2049],
            "compatible Q14 retains the exact legacy Q12 impact normal"
        );
        assert_eq!(dist_q5, -2576);
        assert_eq!(2 * 3 + 4, 10, "PlaneRec stays byte-for-byte the same size");
    }

    #[test]
    fn pvs_lookup_rejects_submodel_only_leaf_records() {
        let mut leaves = vec![0u8; 4 * SZ_LEAF];
        leaves[SZ_LEAF + SZ_LEAF_VISOFS..SZ_LEAF + SZ_LEAF_VISOFS + 4]
            .copy_from_slice(&0i32.to_le_bytes());
        let vis = [0b0000_0111];

        assert!(leaf_row_sees(&leaves, &vis, 2, 1, 2));
        assert!(!leaf_row_sees(&leaves, &vis, 2, 1, 3));
        assert!(!leaf_row_sees(&leaves, &vis, 2, 3, 1));
    }

    // The runtime recovers the model inflation factor as `4096 /
    // local_to_world_q12` and uses it to deflate translation/depth. That must
    // round-trip exactly to the cook's MDL_VERTEX_LOCAL_SCALE, or world-placed
    // models drift in size and OT depth. Guards the "bump the scale" knob.
    #[test]
    fn model_scale_round_trips_through_q12() {
        let q12 = mdl_local_to_world_q12(MDL_VERTEX_LOCAL_SCALE);
        let runtime_s = (4096 / q12 as i32).max(1);
        assert_eq!(runtime_s, MDL_VERTEX_LOCAL_SCALE);
        assert!(quantize_mdl_coord(1.0, MDL_VERTEX_LOCAL_SCALE) == MDL_VERTEX_LOCAL_SCALE as i16);
        // ×s before rounding
    }

    #[test]
    fn packed_viewmodel_normal_uses_the_hmd7_triangle_pad() {
        let packed = mdl_pack_normal_555([127, -127, 0]);
        assert_eq!(packed & 0x1f, 15);
        assert_eq!((packed >> 5) & 0x1f, 17); // signed five-bit -15
        assert_eq!((packed >> 10) & 0x1f, 0);
        assert_eq!(mdl_pack_normal_555([0; 3]), 0);
    }

    #[test]
    fn chrome_uv_uses_goldsrcs_full_reflection_range() {
        assert_eq!(chrome_uv([-1.0, 0.0, 1.0], 64.0, 64.0), (0, 0));
        assert_eq!(chrome_uv([1.0, 0.0, -1.0], 64.0, 64.0), (63, 63));
    }

    #[test]
    fn only_one_winding_of_a_facing_pair_survives_the_viewmodel_cull() {
        // Same three points, opposite windings: exactly one of them has to be
        // the back face from any single eye, whichever way the sign convention
        // falls out of VM_BASE's reflection.
        let frames = vec![vec![[100, 0, 0], [100, 20, 0], [100, 0, 20]]];
        let tri_idx = [0u16, 1, 2, 0, 2, 1];
        let never = viewmodel_never_front_facing(&frames, &tri_idx, &[1, 1], 8);
        assert_eq!(never.iter().filter(|&&drop| drop).count(), 1);

        // Texture 0 is the two-sided sleeve; the runtime never culls it, so the
        // cook must never delete it either.
        let sleeve = viewmodel_never_front_facing(&frames, &tri_idx, &[0, 0], 8);
        assert_eq!(sleeve, vec![false, false]);
    }

    #[test]
    fn model_cook_modes_select_format_and_scale() {
        assert_eq!(
            mdl_cook_mode("--mdl7-vm"),
            Some(MdlCookMode {
                full_normals: false,
                packed_normals: true,
                vertex_scale: 8,
                simplify_target: 0,
                camera_locked: true,
            })
        );
        assert_eq!(
            mdl_cook_mode("--mdl7"),
            Some(MdlCookMode {
                full_normals: false,
                packed_normals: false,
                vertex_scale: 4,
                simplify_target: 0,
                camera_locked: false,
            })
        );
        assert_eq!(
            mdl_cook_mode("--mdl7-lean"),
            Some(MdlCookMode {
                full_normals: false,
                packed_normals: false,
                vertex_scale: 4,
                simplify_target: 768,
                camera_locked: false,
            })
        );
        assert_eq!(
            mdl_cook_mode("--mdl7-normals"),
            Some(MdlCookMode {
                full_normals: true,
                packed_normals: false,
                vertex_scale: 4,
                simplify_target: 0,
                camera_locked: false,
            })
        );
    }

    #[test]
    fn actor_hmd7_normal_modes_share_q12_scale() {
        let plain = mdl_cook_mode("--mdl7").unwrap();
        let normals = mdl_cook_mode("--mdl7-normals").unwrap();

        assert_eq!(plain.vertex_scale, normals.vertex_scale);
        assert_eq!(mdl_local_to_world_q12(plain.vertex_scale), 1024);
        assert_eq!(mdl_local_to_world_q12(normals.vertex_scale), 1024);
        assert!(!plain.full_normals);
        assert!(normals.full_normals);
    }

    #[test]
    fn mdl_simplification_is_deterministic() {
        let frame: Vec<[i16; 3]> = (0..33)
            .map(|i| [(i * 3) as i16, (i % 5) as i16, (i % 3) as i16])
            .collect();
        let frames = vec![frame];
        let bones = vec![0usize; 33];
        let a = simplify_mdl_animated_mesh(&frames, &bones, &[], &[], &[], &[], 12);
        let b = simplify_mdl_animated_mesh(&frames, &bones, &[], &[], &[], &[], 12);

        assert_eq!(a, b);
        assert!(a.grid_size > 0);
    }

    #[test]
    fn mdl_simplification_keeps_triangle_metadata_aligned() {
        let frames = vec![vec![
            [0, 0, 0],
            [0, 0, 0],
            [10, 0, 0],
            [10, 10, 0],
            [0, 10, 0],
            [20, 20, 5],
        ]];
        let simplified = simplify_mdl_animated_mesh(
            &frames,
            &[0; 6],
            &[0, 1, 2, 2, 3, 4],
            &[7, 9],
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            &[[1, 2, 3], [4, 5, 6]],
            5,
        );

        assert_eq!(simplified.tri_idx, [1, 2, 3]);
        assert_eq!(simplified.tri_tex, [9]);
        assert_eq!(simplified.tri_uv, [6, 7, 8, 9, 10, 11]);
        assert_eq!(simplified.tri_norm, [[0, 0, 127]]);
    }

    #[test]
    fn mdl_simplification_averages_every_baked_frame() {
        let frames = vec![
            vec![[0, 0, 0], [0, 0, 0], [10, 0, 0], [20, 0, 0]],
            vec![[1, -2, 0], [4, -3, 0], [12, 1, 0], [24, 2, 0]],
        ];
        let simplified = simplify_mdl_animated_mesh(&frames, &[0; 4], &[], &[], &[], &[], 3);

        assert_eq!(simplified.frames[0], [[0, 0, 0], [10, 0, 0], [20, 0, 0]]);
        assert_eq!(simplified.frames[1], [[3, -3, 0], [12, 1, 0], [24, 2, 0]]);
    }

    #[test]
    fn mdl_simplification_reaches_ps1_headroom_target() {
        let frame: Vec<[i16; 3]> = (0..1025)
            .map(|i| [i as i16, (i % 17) as i16, (i % 11) as i16])
            .collect();
        let simplified = simplify_mdl_animated_mesh(
            &[frame],
            &vec![0usize; 1025],
            &[],
            &[],
            &[],
            &[],
            MDL_SIMPLIFIED_VERTEX_TARGET,
        );

        assert!(simplified.frames[0].len() <= MDL_SIMPLIFIED_VERTEX_TARGET);
        assert!(simplified.frames[0].len() < 1025);
    }

    #[test]
    fn mdl_simplification_is_exact_noop_at_target() {
        let frames = vec![vec![[0, 0, 0], [8, 0, 0], [0, 8, 0]]];
        let simplified = simplify_mdl_animated_mesh(
            &frames,
            &[0; 3],
            &[0, 1, 2],
            &[4],
            &[1, 2, 3, 4, 5, 6],
            &[[7, 8, 9]],
            3,
        );

        assert_eq!(simplified.frames, frames);
        assert_eq!(simplified.tri_idx, [0, 1, 2]);
        assert_eq!(simplified.tri_tex, [4]);
        assert_eq!(simplified.tri_uv, [1, 2, 3, 4, 5, 6]);
        assert_eq!(simplified.tri_norm, [[7, 8, 9]]);
        assert_eq!(simplified.grid_size, 0);
    }

    #[test]
    fn model_sequence_pose_cap_allows_measured_high_fidelity_clips() {
        let specs = parse_seq_specs("idle:48,fire:999").unwrap();
        assert_eq!(specs[0].max_frames, 48);
        assert_eq!(specs[1].max_frames, MAX_BAKED_SEQUENCE_FRAMES);
        assert_eq!(parse_seq_specs("idle").unwrap()[0].max_frames, 16);
    }

    #[test]
    fn model_clip_record_packs_source_duration_without_growing() {
        let quanta = mdl_sequence_hold_quanta(61, 16.0);
        assert_eq!(quanta, 38, "intropush is 3.75 seconds, rounded to 3.8");
        let (first, count) = mdl_pack_clip(7, 2, quanta);
        assert_eq!(first, 7);
        assert_eq!(count & MDL_CLIP_FRAME_COUNT_MASK, 2);
        assert_eq!(count >> 8, 38);
        assert_eq!(core::mem::size_of_val(&(first, count)), 4);
    }

    #[test]
    fn model_clip_record_extends_loader_duration_without_growing() {
        let quanta = mdl_sequence_hold_quanta(501, 15.0);
        assert_eq!(quanta, 334, "rampwalk is 33.33 seconds, rounded to 33.4");
        let (first, count) = mdl_pack_clip(11, 8, quanta);
        assert_eq!(first & MDL_CLIP_FIRST_FRAME_MASK, 11);
        assert_ne!(first & MDL_CLIP_DURATION_EXT_BIT, 0);
        assert_eq!(count & MDL_CLIP_FRAME_COUNT_MASK, 8);
        assert_eq!((count >> 8) | 0x100, 334);
        assert_eq!(core::mem::size_of_val(&(first, count)), 4);
    }

    #[test]
    fn looped_model_sampling_does_not_bake_the_duplicate_endpoint() {
        assert_eq!(mdl_baked_frame_count(64, 4, 4, true), 3);
        assert_eq!(mdl_baked_frame_count(64, 4, 4, false), 4);
        let frames = vec![
            vec![[0, 0, 0], [8, 0, 0]],
            vec![[0, 0, 0], [8, 8, 0]],
            vec![[0, 0, 0], [8, -8, 0]],
            vec![[0, 0, 0], [8, 0, 0]], // duplicate GoldSrc loop endpoint
        ];
        let samples = mdl_motion_sample_frames(&frames, 3, true);
        assert_eq!(samples.len(), 3);
        assert!(samples.iter().all(|&frame| frame < 3));
    }

    #[test]
    fn model_pose_error_is_zero_for_linear_motion_and_ignores_root_travel() {
        let frames = vec![
            vec![[0, 0, 0], [8, 0, 0]],
            vec![[4, 4, 0], [12, 4, 0]],
            vec![[8, 8, 0], [16, 8, 0]],
        ];
        let (rms, max) = mdl_pose_reconstruction_error(&frames, &[0, 2], false, 4);
        assert_eq!(rms, 0.0);
        assert_eq!(max, 0.0);
    }

    #[test]
    fn model_pose_error_detects_lost_midcycle_limb_motion() {
        let frames = vec![
            vec![[0, 0, 0], [8, 0, 0]],
            vec![[0, 0, 0], [8, 8, 0]],
            vec![[0, 0, 0], [8, 0, 0]],
        ];
        let (rms, max) = mdl_pose_reconstruction_error(&frames, &[0, 2], false, 4);
        assert!(rms > 0.0);
        assert!(max >= 1.0);
    }

    #[test]
    fn motion_sampling_keeps_a_brief_limb_extreme_and_authored_endpoints() {
        let mut frames = vec![vec![[0, 0, 0], [8, 0, 0]]; 9];
        frames[6][1][1] = 32;
        assert_eq!(mdl_motion_sample_frames(&frames, 3, false), [0, 6, 8]);
        assert_eq!(mdl_source_phase(0, 9, false), 0);
        assert_eq!(mdl_source_phase(6, 9, false), 191);
        assert_eq!(mdl_source_phase(8, 9, false), 255);
    }

    #[test]
    fn motion_sampling_splits_widest_gap_when_curve_is_linear() {
        let frames = (0..9)
            .map(|frame| vec![[frame * 4, 0, 0], [frame * 4 + 8, 0, 0]])
            .map(|vertices| {
                vertices
                    .into_iter()
                    .map(|vertex| [vertex[0] as i16, vertex[1] as i16, vertex[2] as i16])
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(mdl_motion_sample_frames(&frames, 3, false), [0, 4, 8]);
    }

    #[test]
    fn entity_leafs_split_by_bsp_plane() {
        let mut planes = Vec::new();
        planes.extend_from_slice(&1.0f32.to_le_bytes());
        planes.extend_from_slice(&0.0f32.to_le_bytes());
        planes.extend_from_slice(&0.0f32.to_le_bytes());
        planes.extend_from_slice(&0.0f32.to_le_bytes());
        planes.extend_from_slice(&0i32.to_le_bytes());

        let mut nodes = Vec::new();
        nodes.extend_from_slice(&0i32.to_le_bytes()); // planenum
        nodes.extend_from_slice(&(-2i16).to_le_bytes()); // front -> leaf 1
        nodes.extend_from_slice(&(-3i16).to_le_bytes()); // back -> leaf 2
        nodes.resize(SZ_NODE, 0);

        let front = entity_leafs(
            [1.0, -1.0, -1.0],
            [2.0, 1.0, 1.0],
            [0.0; 3],
            None,
            &nodes,
            &planes,
        );
        assert_eq!(front, vec![1]);

        let crossing = entity_leafs(
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0; 3],
            None,
            &nodes,
            &planes,
        );
        assert_eq!(crossing, vec![1, 2]);

        assert_eq!(point_leaf([8.0, 0.0, 0.0], &nodes, &planes), 1);
        assert_eq!(point_leaf([-8.0, 0.0, 0.0], &nodes, &planes), 2);
    }

    #[test]
    fn func_train_leafs_use_corner_minus_model_center_and_swept_bounds() {
        let mut planes = Vec::new();
        planes.extend_from_slice(&1.0f32.to_le_bytes());
        planes.extend_from_slice(&0.0f32.to_le_bytes());
        planes.extend_from_slice(&0.0f32.to_le_bytes());
        planes.extend_from_slice(&50.0f32.to_le_bytes());
        planes.extend_from_slice(&0i32.to_le_bytes());

        let mut nodes = Vec::new();
        nodes.extend_from_slice(&0i32.to_le_bytes());
        nodes.extend_from_slice(&(-2i16).to_le_bytes());
        nodes.extend_from_slice(&(-3i16).to_le_bytes());
        nodes.resize(SZ_NODE, 0);

        let ents = r#"
        { "classname" "func_train" "target" "a" }
        { "classname" "path_corner" "targetname" "a" "target" "b" "origin" "10 0 0" }
        { "classname" "path_corner" "targetname" "b" "origin" "20 0 0" }
        "#;
        // This model was authored at x=100..120 (center 110), but its live
        // center travels 10..20. Correct swept bounds are x=0..30: back leaf.
        let train = ents.split('{').nth(1).unwrap();
        let leaves = func_train_leafs(
            ents,
            train,
            [100.0, -1.0, -1.0],
            [120.0, 1.0, 1.0],
            [0.0; 3],
            &nodes,
            &planes,
        );
        assert_eq!(leaves, vec![2]);
    }

    #[test]
    fn collect_entities_guntarget_covers_its_full_path() {
        let mut planes = Vec::new();
        planes.extend_from_slice(&1.0f32.to_le_bytes());
        planes.extend_from_slice(&0.0f32.to_le_bytes());
        planes.extend_from_slice(&0.0f32.to_le_bytes());
        planes.extend_from_slice(&50.0f32.to_le_bytes());
        planes.extend_from_slice(&0i32.to_le_bytes());

        let mut nodes = Vec::new();
        nodes.extend_from_slice(&0i32.to_le_bytes());
        nodes.extend_from_slice(&(-2i16).to_le_bytes());
        nodes.extend_from_slice(&(-3i16).to_le_bytes());
        nodes.resize(SZ_NODE, 0);

        let ents = br#"
        { "classname" "func_guntarget" "model" "*1" "target" "a" }
        { "classname" "path_track" "targetname" "a" "target" "b" "origin" "10 0 0" }
        { "classname" "path_track" "targetname" "b" "origin" "120 0 0" }
        "#;
        let mut models = vec![0u8; 2 * SZ_MODEL];
        for (offset, value) in [
            (0usize, 100.0f32),
            (4, -1.0),
            (8, -1.0),
            (12, 120.0),
            (16, 1.0),
            (20, 1.0),
        ] {
            let at = SZ_MODEL + offset;
            models[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }

        let cooked = collect_entities(ents, &models, &nodes, &planes, 1.0, 0);
        assert_eq!(cooked.len(), 1);
        assert_eq!(cooked[0].leaves, vec![1, 2]);
    }

    #[test]
    fn parses_synthetic_bsp() {
        // 2 vertices (24 bytes).
        let verts = vec![0u8; 2 * SZ_VERTEX];
        // TEXTURES lump: nummiptex=1, dataofs[0]=8, one embedded 16×16 miptex.
        let mut tex = Vec::new();
        put_i32(&mut tex, 1); // nummiptex
        put_i32(&mut tex, 8); // dataofs[0] -> miptex starts right after these 8 bytes
        let mut name = [0u8; 16];
        name[..4].copy_from_slice(b"test");
        tex.extend_from_slice(&name);
        tex.extend_from_slice(&16u32.to_le_bytes()); // width
        tex.extend_from_slice(&16u32.to_le_bytes()); // height
        tex.extend_from_slice(&40u32.to_le_bytes()); // offsets[0] != 0 => embedded
        tex.extend_from_slice(&[0u8; 12]); // offsets[1..4] = 0

        let v_ofs = HEADER_LEN;
        let t_ofs = v_ofs + verts.len();
        let mut buf = Vec::new();
        put_i32(&mut buf, 30); // version
        for i in 0..NUM_LUMPS {
            let (o, l) = match i {
                LUMP_VERTEXES => (v_ofs, verts.len()),
                LUMP_TEXTURES => (t_ofs, tex.len()),
                _ => (HEADER_LEN, 0),
            };
            put_i32(&mut buf, o as i32);
            put_i32(&mut buf, l as i32);
        }
        assert_eq!(buf.len(), HEADER_LEN);
        buf.extend_from_slice(&verts);
        buf.extend_from_slice(&tex);

        let bsp = Bsp::parse(&buf).expect("should parse");
        assert_eq!(bsp.lump(LUMP_VERTEXES).len() / SZ_VERTEX, 2);
        let ts = texture_stats(&bsp);
        assert_eq!(ts.count, 1);
        assert_eq!(ts.embedded, 1);
        assert_eq!(ts.texels, 256);
        assert_eq!(ts.largest[0].0, "test");
    }

    #[test]
    fn rejects_wrong_version() {
        let mut buf = vec![0u8; HEADER_LEN];
        buf[..4].copy_from_slice(&29i32.to_le_bytes()); // Quake, not GoldSrc
        assert!(Bsp::parse(&buf).is_err());
    }

    #[test]
    fn rejects_overrunning_lump() {
        let mut buf = vec![0u8; HEADER_LEN];
        buf[..4].copy_from_slice(&30i32.to_le_bytes());
        // lump 0: ofs=0, len=huge -> must be rejected, not panic.
        buf[4..8].copy_from_slice(&0i32.to_le_bytes());
        buf[8..12].copy_from_slice(&1_000_000i32.to_le_bytes());
        assert!(Bsp::parse(&buf).is_err());
    }

    #[test]
    fn spawn_yaw_accepts_single_angle_key() {
        let ents = br#"
        {
        "origin" "484 318 -204"
        "angle" "180"
        "classname" "info_player_start"
        }
        "#;
        let (origin, yaw) = find_spawn(ents).expect("spawn");
        assert_eq!(origin, [484.0, 318.0, -204.0]);
        assert_eq!(yaw, 180.0);
    }

    #[test]
    fn standalone_landmark_keeps_authored_origin() {
        let ents = br#"
        {
        "origin" "1974 -256 -1068"
        "targetname" "c1a4dtoc1a4e"
        "classname" "info_landmark"
        }
        "#;
        let candidates = standalone_spawn_candidates(ents);

        assert_eq!(candidates.len(), 1);
        assert!(!candidates[0].is_player_start);
        assert_eq!(candidates[0].origin_hl, [1974.0, -256.0, -1068.0]);
    }

    #[test]
    fn hl_yaw_maps_to_world_forward_axes() {
        assert_eq!(hl_yaw_to_world_q12(90.0), 0);
        assert_eq!(hl_yaw_to_world_q12(0.0), 1024);
        assert_eq!(hl_yaw_to_world_q12(180.0), 3072);
        assert_eq!(hl_yaw_to_world_q12(270.0), 2048);
    }

    #[test]
    fn microwave_render_controllers_keep_all_brush_targets() {
        let ents = br#"
        { "classname" "func_rotating" "model" "*1" "targetname" "stew" }
        { "classname" "func_rotating" "model" "*2" "targetname" "stew2" }
        { "classname" "func_wall" "model" "*3" "targetname" "stew2" }
        { "classname" "env_render" "target" "stew" "rendermode" "2" "renderamt" "0" }
        { "classname" "env_render" "target" "stew2" "rendermode" "0" "renderamt" "255" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &vec![0; 4 * SZ_MODEL],
            &[LOGIC_BRUSH_NONE, 0, 1, 2],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
        let controllers: Vec<_> = logic
            .ents
            .iter()
            .filter(|rec| rec.kind == LOGIC_ENV_RENDER)
            .collect();
        assert_eq!(controllers[0].arg1, 0x80);
        assert_eq!(controllers[1].arg1, 0);
        let targets = |rec: &LogicRec| {
            (0..rec.aux_count as usize)
                .map(|i| logic.aux[rec.first_aux as usize + i].target)
                .collect::<Vec<_>>()
        };
        assert_eq!(targets(controllers[0]), [0]);
        assert_eq!(targets(controllers[1]), [1, 2]);
        assert!(!pipeline_static_submodels(ents, 4)[3]);
        assert_eq!(brush_render_class(r#""rendermode" "0" "renderamt" "0""#), 0);
    }

    #[test]
    fn cooks_counter_and_changetarget_logic() {
        let ents = br#"
        {
        "classname" "trigger_counter"
        "targetname" "counter_a"
        "target" "relay_a"
        "count" "3"
        }
        {
        "classname" "trigger_changetarget"
        "target" "relay_a"
        "m_iszNewTarget" "door_b"
        }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 2);
        assert_eq!(logic.ents[0].kind, LOGIC_TRIGGER_COUNTER);
        assert_eq!(logic.ents[0].arg0, 3);
        assert_eq!(logic.names[logic.ents[0].target as usize - 1], "relay_a");

        assert_eq!(logic.ents[1].kind, LOGIC_TRIGGER_CHANGETARGET);
        assert_eq!(logic.names[logic.ents[1].target as usize - 1], "relay_a");
        assert_eq!(logic.names[logic.ents[1].arg0 as usize - 1], "door_b");
    }

    #[test]
    fn cooks_hazard_course_guntarget_path_and_completion_controls() {
        let ents = br#"
        { "classname" "func_guntarget" "model" "*1" "targetname" "target1"
          "target" "p1" "message" "target1mm" "health" "5" "speed" "100" }
        { "classname" "path_track" "targetname" "p1" "target" "p2" "origin" "0 0 0" }
        { "classname" "path_track" "targetname" "p2" "target" "p1" "origin" "64 0 0" }
        { "classname" "env_render" "targetname" "show_holo" "target" "holo"
          "renderamt" "172" }
        { "classname" "trigger_endsection" "model" "*2" "targetname" "training_done" }
        "#;
        let models = vec![0u8; 3 * SZ_MODEL];
        let logic = collect_logic_entities(
            ents,
            &models,
            &[LOGIC_BRUSH_NONE, 0, LOGIC_BRUSH_NONE],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        let gun = logic
            .ents
            .iter()
            .find(|rec| rec.kind == LOGIC_FUNC_GUNTARGET)
            .expect("gun target");
        assert_eq!(gun.brush, 0);
        assert_eq!(gun.arg0, 5);
        assert_eq!(logic.names[gun.arg1 as usize - 1], "target1mm");
        assert_eq!(gun.aux_count, 6, "two path_track nodes use triples");
        assert_ne!(gun.flags & LOGIC_TRAIN_EXTENDED, 0);
        assert_eq!(gun.flags >> LOGIC_TRAIN_CYCLE_SHIFT, 1);

        let render = logic
            .ents
            .iter()
            .find(|rec| rec.kind == LOGIC_ENV_RENDER)
            .expect("render controller");
        assert_eq!(render.arg0, 172);
        assert_eq!(logic.names[render.target as usize - 1], "holo");
        assert!(logic
            .ents
            .iter()
            .any(|rec| rec.kind == LOGIC_TRIGGER_ENDSECTION));
    }

    #[test]
    fn hazard_course_text_uses_playstation_controls() {
        let adapted = adapt_hazard_course_text(
            "PRESS YOUR USE KEY\nPRESS FORWARD KEY\nPRESS BACKWARD KEY\nPRESS JUMP KEY\nAIM WITH THE MOUSE\nPRESS ATTACK1 KEY",
        );
        assert_eq!(
            adapted,
            "PRESS SQUARE\nPRESS LEFT STICK UP\nPRESS LEFT STICK DOWN\nPRESS CROSS\nAIM WITH THE RIGHT STICK\nPRESS R2"
        );
    }

    #[test]
    fn title_flags_preserve_goldsrc_alignment_and_vertical_position() {
        assert_eq!(title_y_q5(-1.0), 0);
        assert_eq!(title_y_q5(0.3), 9);
        assert_eq!(title_y_q5(0.65), 20);
        assert_eq!(title_y_q5(0.8), 25);

        let title = TitleDef {
            effect: 2,
            fade_ticks: 30,
            left_aligned: true,
            y_q5: title_y_q5(0.65),
            ..Default::default()
        };
        let packed = pack_title_flags(&title);
        assert_eq!(packed & 3, 2);
        assert_ne!(packed & 4, 0);
        assert_eq!((packed >> 3) & 31, 20);
        assert_eq!(packed >> 8, 30);
    }

    #[test]
    fn known_empty_title_is_a_noop_instead_of_leaking_its_token() {
        let ents = br#"
        {
        "classname" "env_message"
        "targetname" "title_manager"
        "message" "OPENTITLE3"
        }
        {
        "classname" "env_message"
        "targetname" "literal_manager"
        "message" "DIRECT MESSAGE"
        }
        "#;
        let mut titles = std::collections::HashMap::new();
        titles.insert("OPENTITLE3".to_string(), TitleDef::default());

        let logic = collect_logic_entities(ents, &[], &[], 1.0, &titles, &Default::default())
            .expect("title logic");
        let empty = logic
            .ents
            .iter()
            .find(|rec| logic.names[rec.targetname as usize - 1] == "title_manager")
            .expect("empty stock title");
        let literal = logic
            .ents
            .iter()
            .find(|rec| logic.names[rec.targetname as usize - 1] == "literal_manager")
            .expect("literal title");

        assert_eq!(empty.arg0, 0, "known empty title must draw nothing");
        assert!(logic.names.iter().any(|name| name == "OPENTITLE3"));
        assert_ne!(literal.arg0, 0, "unknown literal messages still render");
        assert_eq!(logic.names[literal.arg0 as usize - 1], "DIRECT MESSAGE");
    }

    #[test]
    fn beverage_keeps_stock_skin_owner_and_dormant_can() {
        for (health, expected) in [("0", 10), ("2", 2), ("-2", 0), ("0.2", 1)] {
            let text = format!(
                r#"{{ "classname" "env_beverage" "targetname" "soda"
                "health" "{health}" "skin" "6" "origin" "10 20 30" }}"#
            );
            let logic = collect_logic_entities(
                text.as_bytes(),
                &[],
                &[],
                1.0,
                &Default::default(),
                &Default::default(),
            )
            .unwrap();
            assert_eq!(logic.ents.len(), 1);
            assert_eq!(logic.ents[0].kind, LOGIC_ENV_BEVERAGE);
            assert_eq!(logic.ents[0].arg0, expected);
            assert_eq!(logic.ents[0].arg1, 6);
            assert_eq!(logic.ents[0].sound1, 2);
            assert_eq!(logic.ents[0].origin, [10, 30, 20]);
            let mut names = logic.names;
            let props = collect_props(text.as_bytes(), &[], &[], &[], 1.0, &mut names);
            assert_eq!(props.len(), 1);
            assert_eq!(props[0].0, 75 | 0x4000);
        }
    }

    #[test]
    fn cooks_world_items_as_logic_identities() {
        let ents = br#"
        {
        "classname" "world_items"
        "type" "44"
        "targetname" "hev_battery_once"
        "origin" "10 20 30"
        }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 1);
        assert_eq!(logic.ents[0].kind, LOGIC_ITEM_BATTERY);
        assert_eq!(
            logic.names[logic.ents[0].targetname as usize - 1],
            "hev_battery_once"
        );
    }

    #[test]
    fn targeted_weapon_pickup_preserves_its_use_target_without_cooking_plain_drops() {
        let ents = br#"
        {
        "classname" "weapon_mp5"
        "origin" "-1258 1084 -84"
        "target" "mp5mm"
        }
        {
        "classname" "weapon_glock"
        "origin" "0 0 0"
        }
        {
        "classname" "multi_manager"
        "targetname" "mp5mm"
        "training_door" "0"
        }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        let pickup = logic
            .ents
            .iter()
            .find(|rec| rec.kind == LOGIC_WEAPON_PICKUP)
            .expect("targeted weapon pickup");
        let manager = logic
            .ents
            .iter()
            .find(|rec| rec.kind == LOGIC_MULTI_MANAGER)
            .expect("target manager");
        assert_eq!(pickup.arg0, 29, "MP5 prop kind");
        assert_eq!(pickup.target, manager.targetname);
        assert_eq!(logic.names[pickup.target as usize - 1], "mp5mm");
        assert_eq!(
            logic
                .ents
                .iter()
                .filter(|rec| rec.kind == LOGIC_WEAPON_PICKUP)
                .count(),
            1,
            "the untargeted Glock must not consume a logic record"
        );
    }

    #[test]
    fn cooks_trigger_hurt_damage() {
        let ents = br#"
        {
        "classname" "trigger_hurt"
        "targetname" "acid_hurt"
        "target" "acid_alarm"
        "damage" "12"
        }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 1);
        assert_eq!(logic.ents[0].kind, LOGIC_TRIGGER_HURT);
        // CTriggerHurt::HurtTouch runs every 0.5 seconds and applies half of
        // the authored damage each pulse.
        assert_eq!(logic.ents[0].arg0, 6);
        assert_eq!(logic.names[logic.ents[0].target as usize - 1], "acid_alarm");
    }

    #[test]
    fn cooks_trigger_hurt_radiation_feedback_flag() {
        let ents = br#"
        {
        "classname" "trigger_hurt"
        "damage" "10"
        "damagetype" "262144"
        }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents[0].arg0, 5);
        assert_ne!(
            logic.ents[0].flags & LOGIC_TRIGGER_HURT_RADIATION,
            0,
            "DMG_RADIATION must drive the Geiger proximity system"
        );
    }

    #[test]
    fn cooks_tracktrain_as_logic_target() {
        let ents = br#"
        {
        "classname" "func_tracktrain"
        "targetname" "train"
        "target" "trainstop1"
        "model" "*12"
        "speed" "300"
        "startspeed" "50"
        }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 1);
        assert_eq!(logic.ents[0].kind, LOGIC_FUNC_TRACKTRAIN);
        assert_eq!(logic.names[logic.ents[0].targetname as usize - 1], "train");
        assert_eq!(logic.ents[0].speed, 300);
        assert_eq!(logic.ents[0].arg0, 50);
        assert_eq!(logic.ents[0].arg1, 12);
    }

    #[test]
    fn multi_manager_strips_duplicate_key_suffixes_like_goldsrc() {
        let ents = br#"
        {
        "classname" "multi_manager"
        "targetname" "pausemm"
        "train" "0"
        "train#1" "5"
        }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 1);
        let manager = &logic.ents[0];
        assert_eq!(manager.kind, LOGIC_MULTI_MANAGER);
        assert_eq!(manager.aux_count, 2);
        let first = &logic.aux[manager.first_aux as usize];
        let second = &logic.aux[manager.first_aux as usize + 1];
        assert_eq!(first.target, second.target);
        assert_eq!(logic.names[first.target as usize - 1], "train");
        assert_eq!(first.delay_ticks, 0);
        assert_eq!(second.delay_ticks, 100);
    }

    #[test]
    fn relay_and_auto_triggerstate_default_to_sdk_use_off() {
        assert_eq!(
            triggerstate_use_type(r#"{ "classname" "trigger_auto" }"#),
            USE_OFF
        );
        assert_eq!(
            triggerstate_use_type(r#"{ "classname" "trigger_relay" }"#),
            USE_OFF
        );
        assert_eq!(
            triggerstate_use_type(r#"{ "classname" "trigger_auto" "triggerstate" "1" }"#,),
            USE_ON,
        );
        assert_eq!(
            triggerstate_use_type(r#"{ "classname" "trigger_relay" "triggerstate" "2" }"#,),
            USE_TOGGLE,
        );
    }

    #[test]
    fn multisource_registers_direct_sources_then_each_matching_manager_once() {
        let ents = br#"
        { "classname" "trigger_relay" "targetname" "direct" "target" "gate" }
        { "classname" "multi_manager" "targetname" "manager"
          "gate" "0.5" "gate#1" "1.0" }
        { "classname" "multisource" "targetname" "gate" "target" "done" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        let ms = logic
            .ents
            .iter()
            .find(|rec| rec.kind == LOGIC_MULTISOURCE)
            .expect("multisource");
        assert_eq!(ms.arg0, 2);
        assert_eq!(ms.aux_count, 2);
        let members: Vec<u16> = (0..ms.aux_count as usize)
            .map(|i| logic.aux[ms.first_aux as usize + i].target)
            .collect();
        assert_eq!(members, vec![0, 1], "direct relay, then one manager");

        let manager = &logic.ents[1];
        assert_eq!(manager.kind, LOGIC_MULTI_MANAGER);
        assert_eq!(manager.aux_count, 2, "duplicate outputs remain timed");
        assert_eq!(
            logic.aux[manager.first_aux as usize].target,
            logic.aux[manager.first_aux as usize + 1].target
        );
    }

    #[test]
    fn multisource_rejects_required_source_that_was_not_cooked() {
        let ents = br#"
        { "classname" "info_target" "targetname" "marker" "target" "gate" }
        { "classname" "multisource" "targetname" "gate" "target" "done" }
        "#;
        let err = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .err()
        .expect("uncooked source must fail");
        assert!(err.contains("uncooked direct source"), "{err}");
    }

    #[test]
    fn multisource_rejects_more_than_32_members() {
        let mut ents = String::new();
        for i in 0..33 {
            ents.push_str(&format!(
                "{{ \"classname\" \"trigger_relay\" \"targetname\" \"r{i}\" \"target\" \"gate\" }}\n"
            ));
        }
        ents.push_str(
            "{ \"classname\" \"multisource\" \"targetname\" \"gate\" \"target\" \"done\" }",
        );
        let err = collect_logic_entities(
            ents.as_bytes(),
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .err()
        .expect("33-member source must fail");
        assert!(err.contains("limit is 32"), "{err}");
    }

    #[test]
    fn cooks_secondary_tracktrain_path_events_and_dead_end() {
        let ents = br#"
        { "classname" "func_tracktrain" "model" "*1" "targetname" "forktruck" "target" "f1" "speed" "150" "height" "4" }
        { "classname" "path_track" "targetname" "f1" "target" "f2" "origin" "10 20 30" }
        { "classname" "path_track" "targetname" "f2" "target" "f3" "origin" "40 50 60" "message" "gate1mm" "speed" "200" }
        { "classname" "path_track" "targetname" "f3" "origin" "70 80 90" "netname" "done_mm" }
        "#;
        let brush_by_submodel = [LOGIC_BRUSH_NONE, 7];
        let logic = collect_logic_entities(
            ents,
            &[],
            &brush_by_submodel,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 1);
        let train = &logic.ents[0];
        assert_eq!(train.kind, LOGIC_FUNC_TRACKTRAIN);
        assert_eq!(train.brush, 7);
        assert_eq!(train.aux_count, 9, "three path nodes use aux triples");
        assert_eq!(
            (logic.aux[0].target as i16, logic.aux[0].delay_ticks as i16),
            (10, 34),
            "HL z=30 plus height=4 becomes world y=34"
        );
        assert_eq!(logic.aux[5].delay_ticks, 200);
        assert_eq!(logic.names[logic.aux[5].target as usize - 1], "gate1mm");
        assert_eq!(logic.names[train.target as usize - 1], "done_mm");
    }

    #[test]
    fn shared_track_fireonce_message_has_one_owner() {
        let ents = br#"
        { "classname" "func_tracktrain" "model" "*1" "targetname" "fork1" "target" "f1" "speed" "150" }
        { "classname" "func_tracktrain" "model" "*2" "targetname" "fork2" "target" "f1" "speed" "150" }
        { "classname" "path_track" "targetname" "f1" "target" "f2" "origin" "0 0 0" }
        { "classname" "path_track" "targetname" "f2" "origin" "100 0 0" "message" "gate_once" "spawnflags" "2" }
        "#;
        let brush_by_submodel = [LOGIC_BRUSH_NONE, 3, 4];
        let logic = collect_logic_entities(
            ents,
            &[],
            &brush_by_submodel,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 2);
        let first_pass = logic.aux[logic.ents[0].first_aux as usize + 5].target;
        let second_pass = logic.aux[logic.ents[1].first_aux as usize + 5].target;
        assert_ne!(first_pass, 0);
        assert_eq!(second_pass, 0, "shared FIREONCE node must not be copied");
    }

    #[test]
    fn tram_terminal_netname_becomes_final_pass() {
        let ents = br#"
        { "classname" "func_tracktrain" "model" "*2" "targetname" "train" "target" "lower1" "speed" "300" }
        { "classname" "path_track" "targetname" "lower1" "target" "lower2" "origin" "0 0 0" }
        { "classname" "path_track" "targetname" "lower2" "origin" "100 0 0" "netname" "levelchangetoemm" }
        "#;
        let (_, _, _, _, way, _) = collect_tram(ents, 1.0);

        assert_eq!(way.len(), 2);
        assert_eq!(way[1].2, "levelchangetoemm");
    }

    #[test]
    fn duplicate_training_train_selects_the_visible_body_not_the_control_brush() {
        let ents = br#"
        { "classname" "func_tracktrain" "model" "*23" "targetname" "trainingtrain" "globalname" "trainingtrain" "target" "train1" "speed" "100" }
        { "classname" "func_tracktrain" "model" "*24" "targetname" "trainingtrain" "globalname" "trainingtrain" "target" "train1" "speed" "100" }
        { "classname" "path_track" "targetname" "train1" "target" "train1a" "origin" "0 0 0" }
        { "classname" "path_track" "targetname" "train1a" "origin" "0 -100 0" }
        "#;
        let mut models = vec![0u8; 25 * SZ_MODEL];
        models[23 * SZ_MODEL + 60..23 * SZ_MODEL + 64].copy_from_slice(&246i32.to_le_bytes());
        models[24 * SZ_MODEL + 60..24 * SZ_MODEL + 64].copy_from_slice(&6i32.to_le_bytes());

        let (model, _, _, _, way, _) = collect_tram_ranked(ents, &models, 1.0);

        assert_eq!(model, 23);
        assert_eq!(way.len(), 2);
    }

    #[test]
    fn tram_prepends_unique_predecessors_but_preserves_authored_start_and_height() {
        let ents = br#"
        { "classname" "path_track" "targetname" "stop16" "target" "stop17" "origin" "-2525 -1476 0" }
        { "classname" "path_track" "targetname" "stop17" "target" "stop18" "origin" "-2222 -1476 0" }
        { "classname" "path_track" "targetname" "stop18" "target" "stop27" "origin" "-1999 -1476 0" }
        { "classname" "path_track" "targetname" "stop27" "target" "stop28" "origin" "0 -876 0" }
        { "classname" "path_track" "targetname" "stop28" "origin" "0 -592 0" }
        { "classname" "func_tracktrain" "model" "*24" "target" "stop27" "speed" "300" "height" "4" }
        "#;
        let (model, speed, start, wheels, way, _) = collect_tram(ents, 1.0);

        assert_eq!((model, speed, start, way.len()), (24, 300, 3, 5));
        assert_eq!(wheels, 100);
        assert_eq!(pack_tram_motion(speed, start as usize, wheels), 0x80c6_412c);
        assert_eq!(way[0].0, [-2525, 4, -1476]);
        assert_eq!(way[1].0, [-2222, 4, -1476]);
        assert_eq!(way[start as usize].0, [0, 4, -876]);
        assert_eq!(way[4].0, [0, 4, -592]);
    }

    #[test]
    fn c0a0b_tram_cooks_incoming_upper_rail_and_timed_autochange_descent() {
        let ents = br#"
        { "classname" "path_track" "targetname" "trainstop55" "target" "trainstop56" "origin" "-2022 3138 -473" }
        { "classname" "path_track" "targetname" "trainstop56" "target" "upper1" "origin" "-3200 3138 -473" }
        { "classname" "path_track" "targetname" "upper1" "netname" "goingdown" "speed" "0" "origin" "-3543 3138 -473" }
        { "classname" "func_trackautochange" "targetname" "goingdown" "toptrack" "upper1" "bottomtrack" "lower1" "train" "train" "speed" "100" "height" "1271" }
        { "classname" "path_track" "targetname" "lower1" "target" "lower2" "speed" "0" "origin" "-3543 2946 -1744" }
        { "classname" "path_track" "targetname" "lower2" "target" "lower3" "speed" "0" "origin" "-3543 2782 -1744" }
        { "classname" "path_track" "targetname" "lower3" "target" "lower4" "speed" "0" "origin" "-3498 2698 -1744" }
        { "classname" "path_track" "targetname" "lower4" "target" "lower5" "speed" "0" "origin" "-3405 2625 -1744" }
        { "classname" "path_track" "targetname" "lower5" "target" "lower6" "speed" "300" "origin" "-3279 2570 -1744" }
        { "classname" "path_track" "targetname" "lower6" "target" "lower7" "speed" "400" "origin" "-3120 2526 -1744" }
        { "classname" "path_track" "targetname" "lower7" "target" "lower8" "speed" "450" "origin" "-2944 2504 -1744" }
        { "classname" "path_track" "targetname" "lower8" "target" "lower9" "speed" "500" "origin" "-2752 2504 -1744" }
        { "classname" "path_track" "targetname" "lower9" "target" "lower10" "speed" "0" "origin" "-2050 2504 -1744" }
        { "classname" "path_track" "targetname" "lower10" "target" "lower11" "speed" "0" "origin" "-1062 2504 -1744" }
        { "classname" "path_track" "targetname" "lower11" "target" "lower12" "speed" "0" "origin" "-72 2504 -1744" }
        { "classname" "path_track" "targetname" "lower12" "target" "lower13" "speed" "0" "origin" "928 2504 -1744" }
        { "classname" "path_track" "targetname" "lower13" "target" "lower14" "speed" "400" "origin" "1920 2504 -1749" }
        { "classname" "path_track" "targetname" "lower14" "target" "lower15" "speed" "330" "origin" "2256 2504 -1749" }
        { "classname" "path_track" "targetname" "lower15" "target" "lower16" "speed" "0" "origin" "2432 2458 -1749" }
        { "classname" "path_track" "targetname" "lower16" "target" "lower17" "speed" "0" "origin" "2580 2356 -1749" }
        { "classname" "path_track" "targetname" "lower17" "target" "lower18" "speed" "0" "origin" "2697 2217 -1749" }
        { "classname" "path_track" "targetname" "lower18" "target" "lower19" "speed" "0" "origin" "2736 2041 -1749" }
        { "classname" "path_track" "targetname" "lower19" "target" "lower20" "speed" "0" "origin" "2736 1730 -1749" }
        { "classname" "path_track" "targetname" "lower20" "target" "lower20a" "speed" "0" "origin" "2736 1674 -1749" }
        { "classname" "path_track" "targetname" "lower20a" "target" "lower28" "message" "helirun1" "speed" "0" "origin" "2751 1398 -1749" }
        { "classname" "path_track" "targetname" "lower28" "target" "lower29a" "message" "scatter" "speed" "300" "origin" "2857 -1000 -1748" }
        { "classname" "path_track" "targetname" "lower29a" "target" "lower29" "speed" "200" "origin" "2891 -2237 -1748" }
        { "classname" "path_track" "targetname" "lower29" "target" "lower30" "speed" "150" "origin" "2868 -2324 -1748" }
        { "classname" "path_track" "targetname" "lower30" "target" "lower31" "speed" "0" "origin" "2767 -2481 -1748" }
        { "classname" "path_track" "targetname" "lower31" "target" "lower32" "message" "mountain1" "speed" "0" "origin" "2608 -2579 -1748" }
        { "classname" "path_track" "targetname" "lower32" "target" "lower33" "speed" "0" "origin" "2427 -2620 -1748" }
        { "classname" "path_track" "targetname" "lower33" "target" "lower34" "message" "connectionmm" "speed" "100" "origin" "1192 -2620 -1748" }
        { "classname" "path_track" "targetname" "lower34" "target" "lower34a" "message" "train" "speed" "0" "origin" "1143 -2620 -1748" }
        { "classname" "path_track" "targetname" "lower34a" "target" "lower35" "speed" "100" "origin" "1134 -2620 -1748" }
        { "classname" "path_track" "targetname" "lower35" "target" "lower36" "message" "connection2mm" "speed" "0" "origin" "-43 -2620 -1748" }
        { "classname" "path_track" "targetname" "lower36" "target" "lower37" "message" "transitionmm" "netname" "transitionmm" "speed" "0" "origin" "-95 -2620 -1748" }
        { "classname" "path_track" "targetname" "lower37" "speed" "0" "origin" "-117 -2620 -1748" }
        { "classname" "func_tracktrain" "model" "*15" "globalname" "intro_train" "targetname" "train" "target" "lower19" "speed" "300" "height" "4" "spawnflags" "3" }
        "#;
        let (model, speed, start, wheels, way, _) = collect_tram(ents, 1.0);

        assert_eq!((model, speed, start, way.len()), (15, 300, 22, 37));
        assert_eq!(wheels, 100);
        assert_eq!(way[0].0, [-2022, -469, 3138]);
        assert_eq!(way[1].0, [-3200, -469, 3138]);
        assert_eq!(way[2], ([-3543, -469, 3138], 100, "goingdown".into()));
        assert_eq!(way[3], ([-3543, -1740, 3138], 300, String::new()));
        assert_eq!(way[4].0, [-3543, -1740, 2946]);
        assert_eq!(way[start as usize].0, [2736, -1745, 1730]);
        assert_eq!(way.last().map(|entry| entry.0), Some([-117, -1744, -2620]));
        assert_eq!(
            way.iter()
                .filter(|entry| entry.0 == [-3543, -469, 3138])
                .count(),
            1,
            "upper1 belongs at the incoming head, never after lower37"
        );
    }

    #[test]
    fn tram_predecessor_walk_stops_at_ambiguous_fork_and_circular_track() {
        let fork = br#"
        { "classname" "path_track" "targetname" "left" "target" "start" "origin" "-10 0 0" }
        { "classname" "path_track" "targetname" "right" "target" "start" "origin" "10 0 0" }
        { "classname" "path_track" "targetname" "start" "target" "end" "origin" "0 0 0" }
        { "classname" "path_track" "targetname" "end" "origin" "0 100 0" }
        { "classname" "func_tracktrain" "model" "*1" "target" "start" }
        "#;
        let (_, _, fork_start, _, fork_way, _) = collect_tram(fork, 1.0);
        assert_eq!(fork_start, 0, "an ambiguous upstream branch is not guessed");
        assert_eq!(fork_way.len(), 2);

        let cycle = br#"
        { "classname" "path_track" "targetname" "a" "target" "b" "origin" "0 0 0" }
        { "classname" "path_track" "targetname" "b" "target" "c" "origin" "100 0 0" }
        { "classname" "path_track" "targetname" "c" "target" "a" "origin" "200 0 0" }
        { "classname" "func_tracktrain" "model" "*1" "target" "a" }
        "#;
        let (_, _, cycle_start, _, cycle_way, _) = collect_tram(cycle, 1.0);
        assert_eq!(cycle_start, 0, "a cycle retains the authored start");
        assert_eq!(cycle_way.len(), 3, "one authored A,B,C lap is retained");
        assert_eq!(cycle_way[0].0, [0, 0, 0]);
        assert_eq!(cycle_way[1].0, [100, 0, 0]);
        assert_eq!(cycle_way[2].0, [200, 0, 0]);
    }

    #[test]
    fn tram_trackchange_predecessor_preserves_unique_cycle_and_rejects_ambiguity() {
        let unique_cycle = br#"
        { "classname" "path_track" "targetname" "upper" "origin" "0 0 100" "netname" "drop" }
        { "classname" "path_track" "targetname" "lower" "target" "end" "origin" "0 50 0" }
        { "classname" "path_track" "targetname" "end" "origin" "100 50 0" }
        { "classname" "func_trackautochange" "toptrack" "upper" "bottomtrack" "lower" "speed" "20" }
        { "classname" "func_tracktrain" "model" "*1" "target" "lower" "speed" "60" }
        "#;
        let (_, _, start, _, way, _) = collect_tram(unique_cycle, 1.0);
        assert_eq!((start, way.len()), (2, 4));
        assert_eq!(way[0], ([0, 100, 0], 20, "drop".into()));
        assert_eq!(way[1], ([0, 0, 0], 60, String::new()));
        assert_eq!(way[2].0, [0, 0, 50]);
        assert_eq!(way[3].0, [100, 0, 50]);

        let ambiguous = br#"
        { "classname" "path_track" "targetname" "upper_left" "origin" "-10 0 100" }
        { "classname" "path_track" "targetname" "upper_right" "origin" "10 0 100" }
        { "classname" "path_track" "targetname" "lower" "target" "end" "origin" "0 0 0" }
        { "classname" "path_track" "targetname" "end" "origin" "0 100 0" }
        { "classname" "func_trackchange" "toptrack" "upper_left" "bottomtrack" "lower" }
        { "classname" "func_trackchange" "toptrack" "upper_right" "bottomtrack" "lower" }
        { "classname" "func_tracktrain" "model" "*1" "target" "lower" "speed" "60" }
        "#;
        let (_, _, start, _, way, _) = collect_tram(ambiguous, 1.0);
        assert_eq!(start, 0, "ambiguous implicit predecessors are not guessed");
        assert_eq!(way.len(), 2, "forward stitching also rejects the fork");
        assert_eq!(way[0].0, [0, 0, 0]);
        assert_eq!(way[1].0, [0, 0, 100]);
    }

    #[test]
    fn tram_predecessor_budget_falls_back_to_authored_start() {
        let mut ents = String::new();
        for i in 0..260 {
            let target = if i + 1 < 260 {
                format!(" \"target\" \"p{}\"", i + 1)
            } else {
                String::new()
            };
            ents.push_str(&format!(
                "{{ \"classname\" \"path_track\" \"targetname\" \"p{i}\"{target} \"origin\" \"{i} 0 0\" }}\n"
            ));
        }
        ents.push_str("{ \"classname\" \"func_tracktrain\" \"model\" \"*1\" \"target\" \"p259\" }");

        let (_, _, start, _, way, _) = collect_tram(ents.as_bytes(), 1.0);
        assert_eq!(start, 0);
        assert_eq!(way.len(), 1, "oversized prefixes use authored-first order");
        assert_eq!(way[0].0, [259, 0, 0]);
    }

    #[test]
    fn secondary_tracktrain_brush_is_retained_but_player_tram_is_not() {
        let ents = br#"
        { "classname" "func_tracktrain" "model" "*1" "targetname" "forktruck" "target" "f1" }
        { "classname" "func_tracktrain" "model" "*2" "targetname" "train" "target" "main1" }
        "#;
        let models = vec![0u8; 3 * SZ_MODEL];
        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 2);

        assert_eq!(cooked.len(), 1);
        assert_eq!(cooked[0].submodel, 1);
    }

    #[test]
    fn actor_carry_ids_are_stable_and_namespaced() {
        let ordinary = actor_carry_id("Barney1", false);
        assert_eq!(ordinary, actor_carry_id(" barney1 ", false));
        assert_ne!(ordinary, 0);
        assert_eq!(ordinary & CARRY_GLOBAL_BIT, 0);
        assert_eq!(actor_carry_id("barney1", true), ordinary | CARRY_GLOBAL_BIT);
        assert_ne!(actor_carry_id("barney1", true), u16::MAX);
        assert_eq!(actor_carry_id("", false), 0);
    }

    #[test]
    fn authored_prop_pass_leaves_transition_fallbacks_to_manifest_pass() {
        let mut names = vec!["barney1".to_string()];
        let props = collect_props(b"", &[], &[], &[], 1.0, &mut names);
        assert!(props.is_empty());
    }

    #[test]
    fn transition_fallback_preserves_predisaster_and_prisoner_flags() {
        let mut hint = TransitionTypeHint {
            ty: 1,
            targetname: "barney1".to_string(),
            origin: [0.0; 3],
            yaw: 0.0,
            spawnflags: 256,
            body: 0,
        };
        assert_eq!(transition_fallback_type(&hint), 0x2001);
        hint.ty = 9;
        hint.spawnflags = 16;
        assert_eq!(transition_fallback_type(&hint), 0x1009);
    }

    #[test]
    fn prop_carry_ids_include_live_actors_and_exclude_sdk_non_carries() {
        let ents = br#"
        { "classname" "monster_barney" "targetname" "barney1" }
        { "classname" "monster_apache" "globalname" "apache1" }
        { "classname" "monster_tentacle" "targetname" "tentacle1" }
        { "classname" "monster_barney_dead" "targetname" "dead_barney" }
        { "classname" "item_suit" "targetname" "suit1" }
        "#;
        let mut names = vec!["barney1".to_string()];
        let props = collect_props(ents, &[], &[], &[], 1.0, &mut names);

        assert_eq!(props.len(), 6); // five authored props + Barney's dormant Glock
        assert_eq!(props[0].5, actor_carry_id("barney1", false));
        assert_eq!(props[1].0, LOOT_SLOT_FLAGS | 27);
        assert_eq!(props[1].5, 0, "death loot is map-local");
        assert_eq!(props[2].5, actor_carry_id("apache1", true));
        assert_eq!(props[2].5 & CARRY_GLOBAL_BIT, CARRY_GLOBAL_BIT);
        assert_eq!(props[3].5, 0, "tentacle lacks ACROSS_TRANSITION");
        assert_eq!(props[4].5, 0, "authored corpses are map-local");
        assert_eq!(props[5].5, 0, "items use their own player inventory path");
    }

    #[test]
    fn human_bodygroups_share_low_orientation_word_without_changing_angle() {
        let ents = br#"
        { "classname" "monster_scientist" "body" "2" "angle" "90" }
        { "classname" "monster_sitting_scientist" "body" "6" "angle" "-45" }
        { "classname" "monster_barney_dead" "angle" "180" }
        { "classname" "monster_scientist" "body" "-1" "origin" "7 11 13" }
        "#;
        let mut names = Vec::new();
        let props = collect_props(ents, &[], &[], &[], 1.0, &mut names);

        assert_eq!(props.len(), 4);
        assert_eq!((props[0].2 as u16) >> 12, 2);
        assert_eq!(
            (props[0].2 as u16) & 0x0fff,
            hl_yaw_to_world_q12(90.0) as u16 & 0x0fff
        );
        assert_eq!((props[1].2 as u16) >> 12, 6);
        assert_eq!(
            (props[1].2 as u16) & 0x0fff,
            hl_yaw_to_world_q12(0.0) as u16 & 0x0fff,
            "legacy negative angles other than -1/-2 are invalid and become zero"
        );
        assert_eq!((props[2].2 as u16) >> 12, 2, "dead Barney selects GUNGONE");
        assert!(
            (props[3].2 as u16) >> 12 < 4,
            "body -1 selects a deterministic head"
        );
    }

    #[test]
    fn mouth_relative_transform_uses_cooked_axis_order_and_scale() {
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let closed = (identity, [10.0, 20.0, 30.0]);
        let open = (identity, [11.0, 22.0, 33.0]);
        let xform = mdl_mouth_relative_xform(&closed, &open, 4);

        assert_eq!(xform.0, identity);
        assert_eq!(xform.1, [4.0, 12.0, 8.0], "HL xyz becomes cooked xzy");
    }

    #[test]
    fn trigger_transition_cooks_landmark_and_brush_bounds_without_entrec() {
        let ents = br#"
        { "classname" "trigger_transition" "model" "*1"
          "targetname" "c0a0dtoe" "origin" "10 20 30" }
        "#;
        let mut models = vec![0u8; 2 * SZ_MODEL];
        for (offset, value) in [
            (0usize, -1.0f32),
            (4, -2.0),
            (8, -3.0),
            (12, 4.0),
            (16, 5.0),
            (20, 6.0),
        ] {
            let at = SZ_MODEL + offset;
            models[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let logic = collect_logic_entities(
            ents,
            &models,
            &[LOGIC_BRUSH_NONE; 2],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 1);
        let rec = &logic.ents[0];
        assert_eq!(rec.kind, LOGIC_TRIGGER_TRANSITION);
        assert_eq!(logic.names[rec.targetname as usize - 1], "c0a0dtoe");
        assert_eq!(rec.mins, [9, 27, 18]);
        assert_eq!(rec.maxs, [14, 36, 25]);
        assert_eq!(rec.brush, LOGIC_BRUSH_NONE);
    }

    #[test]
    fn changelevel_cooks_destination_target_and_delay_as_one_aux_record() {
        let ents = br#"
        { "classname" "trigger_changelevel" "model" "*1"
          "map" "c1a2" "landmark" "c1a1ctoc1a2"
          "changetarget" "EleStartMM" "changedelay" "1.25" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[LOGIC_BRUSH_NONE; 2],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("changelevel cook");

        let rec = &logic.ents[0];
        assert_eq!(rec.kind, LOGIC_TRIGGER_CHANGELEVEL);
        assert_eq!(rec.aux_count, 1);
        let post = &logic.aux[rec.first_aux as usize];
        assert_eq!(logic.names[post.target as usize - 1], "EleStartMM");
        assert_eq!(post.delay_ticks, 25);
    }

    #[test]
    fn door_netname_cooks_as_close_only_aux_output() {
        let ents = br#"
        { "classname" "func_door" "model" "*1" "target" "opened_or_closed"
          "netname" "eledoordelaymm" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[LOGIC_BRUSH_NONE, 3],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("door close target cook");

        let door = &logic.ents[0];
        assert_eq!(door.kind, LOGIC_FUNC_DOOR);
        assert_eq!(door.aux_count, 1);
        let close = &logic.aux[door.first_aux as usize];
        assert_eq!(logic.names[close.target as usize - 1], "eledoordelaymm");
    }

    #[test]
    fn only_instant_auto_script_repositions_actor_at_cook() {
        let ents = br#"
        { "classname" "monster_barney" "targetname" "walker" "origin" "1 2 3" }
        { "classname" "scripted_sequence" "m_iszEntity" "walker" "m_fMoveTo" "1" "origin" "10 20 30" }
        { "classname" "monster_barney" "targetname" "teleporter" "origin" "4 5 6" }
        { "classname" "scripted_sequence" "m_iszEntity" "teleporter" "m_fMoveTo" "4" "origin" "40 50 60" }
        "#;
        let mut names = vec!["walker".to_string(), "teleporter".to_string()];
        let props = collect_props(ents, &[], &[], &[], 1.0, &mut names);

        assert_eq!(props.len(), 4);
        assert_eq!(props[0].1, [1, 3, 2], "walk script keeps source spawn");
        assert_eq!(props[2].1, [40, 60, 50], "MoveTo=4 teleports to mark");
    }

    #[test]
    fn cooks_scripted_move_mode_from_goldsrc_key() {
        let ents = br#"
        {
        "classname" "scripted_sequence"
        "targetname" "forklift_path"
        "m_iszEntity" "forklift_actor"
        "m_fMoveTo" "2"
        "origin" "10 20 30"
        }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 1);
        assert_eq!(logic.ents[0].kind, LOGIC_SCRIPTED);
        assert_eq!(logic.ents[0].arg1, 2, "m_fMoveTo=2 must cook as run");
        assert_eq!(
            logic.names[logic.ents[0].arg0 as usize - 1],
            "forklift_actor"
        );
    }

    #[test]
    fn targeted_scripted_idle_primes_even_when_the_clip_is_not_baked() {
        let ents = br#"
        {
        "classname" "scripted_sequence"
        "targetname" "vent_pull"
        "m_iszEntity" "vent_pull_sci"
        "m_fMoveTo" "4"
        "m_iszIdle" "ceiling_dangle"
        "m_iszPlay" "ceiling_dangle"
        "origin" "-493 -725 -72"
        }
        { "classname" "monster_scientist" "targetname" "vent_pull_sci" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("targeted idle cook");

        let rec = &logic.ents[0];
        assert_eq!(rec.kind, LOGIC_SCRIPTED);
        assert_ne!(rec.flags & LOGIC_SCRIPTED_HAS_IDLE, 0);
        assert_ne!(rec.flags & LOGIC_SCRIPTED_HAS_PLAY, 0);
        assert_eq!(
            rec.flags & !(LOGIC_SCRIPTED_HAS_IDLE | LOGIC_SCRIPTED_HAS_PLAY),
            0
        );
        assert_eq!(rec.aux_count, 0, "missing clip must not suppress priming");
    }

    #[test]
    fn cooks_c1a0c_class_retinal_selector_radius_and_completion_links() {
        let ents = br#"
        {
        "classname" "scripted_sequence"
        "targetname" "control_retinal1"
        "m_iszEntity" "monster_scientist"
        "m_flRadius" "150"
        "m_fMoveTo" "1"
        "spawnflags" "32"
        "killtarget" "trigger_for_retinal"
        "target" "control_retinal1mm"
        "origin" "784 278 -144"
        }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("retinal script cook");

        let rec = &logic.ents[0];
        assert_eq!(rec.kind, LOGIC_SCRIPTED);
        assert_eq!(rec.flags, 1, "scientist type zero is encoded as selector 1");
        assert_eq!(
            rec.wait_ticks, 150,
            "m_flRadius reuses the script wait word"
        );
        assert_eq!(rec.spawnflags, 32);
        assert_eq!(logic.names[rec.arg0 as usize - 1], "monster_scientist");
        assert_eq!(
            logic.names[rec.killtarget as usize - 1],
            "trigger_for_retinal"
        );
        assert_eq!(logic.names[rec.target as usize - 1], "control_retinal1mm");
    }

    #[test]
    fn c1a1b_class_selector_resolves_retina_clip_as_scientist() {
        let all = r#"
        { "classname" "scripted_sequence" "m_iszEntity" "monster_scientist"
          "m_iszPlay" "retina" "m_flRadius" "150" }
        "#;
        let block = all.split('{').nth(1).unwrap();
        let mut clips = std::collections::HashMap::new();
        clips.insert(
            (0u16, "retina".to_string()),
            ScriptClip {
                slot: 7,
                hold_quanta: 42,
            },
        );
        let slots = script_clip_slots(all, block, &Default::default(), &clips);

        assert_eq!(
            script_target_monster_type(all, "monster_scientist", &Default::default()),
            Some(0)
        );
        assert_eq!(
            slots,
            ((42 << 6) | 8, 0),
            "clip slot+1 and source duration share the existing LogicAux word"
        );
    }

    #[test]
    fn c1a1b_seated_script_uses_the_compact_hybrid_scientist_stream() {
        let all = r#"
        { "classname" "monster_scientist" "targetname" "sitting_scientist"
          "origin" "609 -1185 -72" }
        { "classname" "scripted_sequence" "m_iszEntity" "sitting_scientist"
          "m_iszPlay" "sitstand" "m_iszIdle" "sitidle" "m_fMoveTo" "4" }
        "#;
        let script = all.split('{').nth(2).unwrap();
        let mut clips = std::collections::HashMap::new();
        clips.insert(
            (SCRIPTED_SITTING_SCIENTIST_TYPE, "sitidle".to_string()),
            ScriptClip {
                slot: 5,
                hold_quanta: 0,
            },
        );
        clips.insert(
            (SCRIPTED_SITTING_SCIENTIST_TYPE, "sitstand".to_string()),
            ScriptClip {
                slot: 6,
                hold_quanta: 0,
            },
        );

        assert!(scripted_sitting_scientist_target(all, "sitting_scientist"));
        assert_eq!(
            script_monster_type(all, "sitting_scientist"),
            Some(SCRIPTED_SITTING_SCIENTIST_TYPE)
        );
        assert_eq!(
            script_clip_slots(all, script, &Default::default(), &clips),
            (7, 6)
        );

        let mut names = vec!["sitting_scientist".to_string()];
        let props = collect_props(all.as_bytes(), &[], &[], &[], 1.0, &mut names);
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].0, SCRIPTED_SITTING_SCIENTIST_TYPE);
    }

    #[test]
    fn c1a1b_zombies_share_the_compact_vent_script_stream() {
        let all = r#"
        { "classname" "monster_zombie" "targetname" "hungry" }
        { "classname" "monster_zombie" "targetname" "vent_zombie" }
        { "classname" "scripted_sequence" "m_iszEntity" "vent_zombie"
          "m_iszPlay" "ventclimb" "m_iszIdle" "ventclimbidle" }
        "#;
        assert!(map_uses_vent_zombie_stream(all));
        assert_eq!(
            script_monster_type(all, "hungry"),
            Some(VENT_SCRIPT_ZOMBIE_TYPE)
        );
        assert_eq!(
            script_monster_type(all, "vent_zombie"),
            Some(VENT_SCRIPT_ZOMBIE_TYPE)
        );

        let mut names = vec!["hungry".to_string(), "vent_zombie".to_string()];
        let props = collect_props(all.as_bytes(), &[], &[], &[], 1.0, &mut names);
        assert_eq!(props.len(), 2);
        assert!(props.iter().all(|prop| prop.0 == VENT_SCRIPT_ZOMBIE_TYPE));
    }

    #[test]
    fn c4a3_hev_corpses_reuse_the_scripted_scientist_stream() {
        let all = br#"
        { "classname" "monster_scientist" "targetname" "sitting_scientist" }
        { "classname" "scripted_sequence" "m_iszEntity" "sitting_scientist"
          "m_iszPlay" "sitstand" "m_iszIdle" "sitidle" }
        { "classname" "monster_hevsuit_dead" }
        "#;
        let mut names = vec!["sitting_scientist".to_string()];
        let props = collect_props(all, &[], &[], &[], 1.0, &mut names);
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].0, SCRIPTED_SITTING_SCIENTIST_TYPE);
        assert_eq!(props[1].0, 0x8000 | SCRIPTED_SITTING_SCIENTIST_TYPE);
    }

    #[test]
    fn exact_script_targetname_remains_primary_over_class_fallback() {
        let all = r#"
        { "classname" "monster_barney" "targetname" "monster_scientist" }
        { "classname" "scripted_sequence" "m_iszEntity" "monster_scientist" }
        "#;
        assert_eq!(
            script_target_monster_type(all, "monster_scientist", &Default::default()),
            Some(1),
            "exact named Barney determines clips before scientist classname fallback"
        );
        assert_eq!(script_class_selector("monster_scientist"), 1);
    }

    #[test]
    fn loader_generic_cooks_as_type_52_and_resolves_its_script_clips() {
        let all = r#"
        { "classname" "monster_generic" "model" "models/loader.mdl"
          "targetname" "lo" "origin" "1 2 3" }
        { "classname" "scripted_sequence" "m_iszEntity" "lo"
          "m_iszPlay" "rampwalk" "m_iszIdle" "idle" }
        "#;
        let block = all.split('{').nth(2).unwrap();
        let mut clips = std::collections::HashMap::new();
        clips.insert(
            (52u16, "idle".to_string()),
            ScriptClip {
                slot: 0,
                hold_quanta: 0,
            },
        );
        clips.insert(
            (52u16, "rampwalk".to_string()),
            ScriptClip {
                slot: 1,
                hold_quanta: 0,
            },
        );

        assert_eq!(script_monster_type(all, "lo"), Some(52));
        assert_eq!(
            script_clip_slots(all, block, &Default::default(), &clips),
            (2, 1)
        );
        let mut names = vec!["lo".to_string()];
        let props = collect_props(all.as_bytes(), &[], &[], &[], 1.0, &mut names);
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].0, 0x1000 | 52, "generic puppets stay passive");
        assert_eq!(props[0].4, 1);
    }

    #[test]
    fn opening_generic_models_resolve_to_reused_and_dedicated_types() {
        let scientist = r#""classname" "monster_generic" "model" "models/scientist.mdl""#;
        let barney = r#""classname" "monster_generic" "model" "models/barney.mdl""#;
        let forklift = r#""classname" "monster_generic" "model" "models\\forklift.mdl""#;
        let unsupported = r#""classname" "monster_generic" "model" "models/otis.mdl""#;

        assert_eq!(monster_generic_type(scientist), Some(0));
        assert_eq!(monster_generic_type(barney), Some(1));
        assert_eq!(monster_generic_type(forklift), Some(53));
        assert_eq!(monster_generic_type(unsupported), None);
        assert!(prop_type_crosses_transition(53));
    }

    #[test]
    fn c1a1_generic_barneys_reuse_the_model_passively_and_actor_names_are_interned() {
        let ents = br#"
        { "classname" "monster_generic" "model" "models/barney.mdl"
          "targetname" "b1" "origin" "1697 1731 -144" "spawnflags" "4" }
        { "classname" "monster_barney" "targetname" "fighting_barney"
          "origin" "1168 1968 728" }
        "#;
        let mut names = Vec::new();
        let props = collect_props(ents, &[], &[], &[], 1.0, &mut names);

        assert_eq!(props.len(), 3); // live Barney also owns a dormant Glock slot
        assert_eq!(props[0].0, 0x1000 | 1, "monster_generic has no Barney AI");
        assert_eq!(props[0].4, 1);
        assert_eq!(props[1].0, 1);
        assert_eq!(props[1].4, 2);
        assert_eq!(names, ["b1", "fighting_barney"]);
    }

    #[test]
    fn talk_monster_predisaster_flag_packs_without_growing_prop_record() {
        let ents = br#"
        { "classname" "monster_scientist" "spawnflags" "256" "targetname" "before" }
        { "classname" "monster_scientist" "spawnflags" "16" "targetname" "prisoner" }
        { "classname" "monster_barney" "origin" "-132 -1083 -216"
          "targetname" "leaningbarney" "spawnflags" "256" "angle" "177" }
        { "classname" "monster_sitting_scientist" "origin" "-1054 -451 -173"
          "body" "1" "spawnflags" "256" "angle" "140" }
        "#;
        let mut names = vec![
            "before".to_string(),
            "prisoner".to_string(),
            "leaningbarney".to_string(),
        ];
        let props = collect_props(ents, &[], &[], &[], 1.0, &mut names);
        assert_eq!(props.len(), 5); // live Barney also owns a dormant Glock slot
        assert_eq!(props[0].0 & 0x2000, 0x2000);
        assert_eq!(props[0].0 & 0x0fff, 0);
        assert_eq!(
            props[1].0 & 0x2000,
            0,
            "prisoner flag 16 stays follow-usable"
        );
        assert_eq!(props[1].0 & 0x1000, 0, "human followers keep normal AI");
        assert_eq!(
            props[2].0,
            0x2000 | 1,
            "c1a0's predisaster Barney must decline following"
        );
        assert_eq!(
            props[4].0,
            0x2000 | 25,
            "CSittingScientist::Spawn always sets SF_MONSTER_PREDISASTER"
        );
    }

    #[test]
    fn hazard_course_holo_and_inactive_turret_reuse_type_specific_cook_bit() {
        let ents = br#"
        { "classname" "monster_generic" "model" "models/holo.mdl"
          "targetname" "holo" "rendermode" "5" "renderamt" "0" }
        { "classname" "monster_miniturret" "targetname" "turret" "spawnflags" "96" }
        "#;
        let mut names = Vec::new();
        let props = collect_props(ents, &[], &[], &[], 1.0, &mut names);
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].0 & 0x0fff, 56);
        assert_ne!(props[0].0 & 0x1000, 0, "hologram remains a passive puppet");
        assert_ne!(props[0].0 & 0x2000, 0, "renderamt zero starts hidden");
        assert_eq!(props[1].0 & 0x0fff, 22);
        assert_ne!(props[1].0 & 0x2000, 0, "START_INACTIVE is retained");
    }

    #[test]
    fn hostile_prisoner_flag_packs_without_growing_prop_record() {
        let ents = br#"
        { "classname" "monster_bullchicken" "spawnflags" "16" }
        { "classname" "monster_alien_slave" "spawnflags" "0" }
        "#;
        let mut names = Vec::new();
        let props = collect_props(ents, &[], &[], &[], 1.0, &mut names);
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].0 & 0x1000, 0x1000);
        assert_eq!(props[0].0 & 0x0fff, 7);
        assert_eq!(props[1].0 & 0x1000, 0);
        assert_eq!(props[1].0 & 0x0fff, 9);
    }

    #[test]
    fn monstermaker_over_monster_portal_cooks_at_destination() {
        let ents = br#"
        { "classname" "info_target" "targetname" "arrival" "origin" "100 200 300" "angle" "90" }
        { "classname" "trigger_teleport" "model" "*1" "spawnflags" "3" "target" "arrival" }
        { "classname" "monstermaker" "targetname" "maker" "monstertype" "monster_alien_slave"
          "monstercount" "1" "origin" "10 20 100" }
        "#;
        let mut models = vec![0u8; 2 * SZ_MODEL];
        for (offset, value) in [
            (0usize, 0.0f32),
            (4, 10.0),
            (8, 0.0),
            (12, 20.0),
            (16, 30.0),
            (20, 20.0),
        ] {
            let at = SZ_MODEL + offset;
            models[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut names = vec!["maker".to_string()];
        let props = collect_props(ents, &[], &[], &models, 1.0, &mut names);
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].0, 0x4000 | 9);
        assert_eq!(props[0].1, [100, 300, 200]);
        assert_eq!(props[0].2, hl_yaw_to_world_q12(90.0));
        assert_eq!(props[0].4, 1, "runtime wakes stock by maker name");
    }

    #[test]
    fn cooks_func_train_default_speed_and_single_path_cycle() {
        let ents = br#"
        {
        "classname" "func_train"
        "model" "*1"
        "target" "corner_a"
        }
        {
        "classname" "path_corner"
        "targetname" "corner_a"
        "target" "corner_b"
        "origin" "10 20 30"
        }
        {
        "classname" "path_corner"
        "targetname" "corner_b"
        "target" "corner_a"
        "origin" "40 50 60"
        }
        "#;
        let brush_by_submodel = [LOGIC_BRUSH_NONE, 3];
        let logic = collect_logic_entities(
            ents,
            &[],
            &brush_by_submodel,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 1);
        assert_eq!(logic.ents[0].kind, LOGIC_FUNC_TRAIN);
        assert_eq!(logic.ents[0].brush, 3);
        assert_eq!(logic.ents[0].speed, 100);
        assert_eq!(logic.ents[0].flags & LOGIC_TRAIN_TERMINAL, 0);
        assert_eq!(
            logic.ents[0].aux_count, 4,
            "two corners, not 24 repeated hops"
        );
        assert_eq!(logic.aux.len(), 4);
    }

    #[test]
    fn func_train_preserves_authored_yaw_for_render_and_collision() {
        let ents = br#"
        { "classname" "func_train" "model" "*1" "origin" "-3151 -2564 -2471"
          "target" "traindoor1" "targetname" "traindoor" "angle" "90" }
        { "classname" "path_corner" "targetname" "traindoor1"
          "origin" "-1197 -715 -205" }
        "#;
        let mut models = vec![0u8; 2 * SZ_MODEL];
        let mo = SZ_MODEL;
        for (off, value) in [
            (0usize, -28.0f32),
            (4, -9.0),
            (8, -43.0),
            (12, 29.0),
            (16, 2.0),
            (20, 51.0),
        ] {
            models[mo + off..mo + off + 4].copy_from_slice(&value.to_le_bytes());
        }

        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 1);
        assert_eq!(cooked[0].kind & 0xff, 7);
        assert_eq!(cooked[0].mv, [0, 0, 0x00c000]);
        assert_eq!(
            func_train_base_angles_q8(r#"{ "angles" "0 270 0" }"#) as u32,
            0x004000,
            "plural angles uses the same reflected yaw convention"
        );
    }

    #[test]
    fn terminal_func_train_and_wait_for_trigger_are_packed_without_extra_state() {
        let ents = br#"
        { "classname" "func_train" "model" "*1" "target" "corner_a" }
        { "classname" "path_corner" "targetname" "corner_a" "target" "corner_b"
          "origin" "0 0 0" }
        { "classname" "path_corner" "targetname" "corner_b" "origin" "0 0 64"
          "wait" "-1" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[LOGIC_BRUSH_NONE, 3],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("terminal train cook");

        let train = &logic.ents[0];
        assert_eq!(train.flags & LOGIC_TRAIN_TERMINAL, LOGIC_TRAIN_TERMINAL);
        assert_eq!(train.aux_count, 4);
        assert_eq!(logic.aux[3].delay_ticks, u16::MAX);
    }

    #[test]
    fn path_corner_wait_and_teleport_spawnflags_share_the_wait_word() {
        let ents = br#"
        { "classname" "func_train" "model" "*1" "target" "a" }
        { "classname" "path_corner" "targetname" "a" "target" "b"
          "origin" "0 0 0" "spawnflags" "1" }
        { "classname" "path_corner" "targetname" "b" "origin" "100 0 0"
          "spawnflags" "2" "wait" "1.5" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[LOGIC_BRUSH_NONE, 3],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("corner flags cook");
        assert_eq!(logic.aux[1].delay_ticks, TRAIN_CORNER_WAIT_TRIGGER);
        assert_eq!(logic.aux[3].delay_ticks, TRAIN_CORNER_TELEPORT | 30);
    }

    #[test]
    fn func_train_tail_cycle_is_serialized_once_with_exact_cycle_start() {
        let ents = br#"
        { "classname" "func_train" "model" "*1" "target" "a" }
        { "classname" "path_corner" "targetname" "a" "target" "b" "origin" "0 0 0" }
        { "classname" "path_corner" "targetname" "b" "target" "c" "origin" "100 0 0" }
        { "classname" "path_corner" "targetname" "c" "target" "b" "origin" "200 0 0" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[LOGIC_BRUSH_NONE, 3],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("tail cycle cook");
        let train = &logic.ents[0];
        assert_eq!(train.aux_count, 6, "a,b,c are each serialized once");
        assert_eq!(train.flags & LOGIC_TRAIN_TERMINAL, 0);
        assert_eq!(
            train.flags >> LOGIC_TRAIN_CYCLE_SHIFT,
            2,
            "cycle begins at b/index 1"
        );
    }

    #[test]
    fn func_train_conditionally_cooks_path_corner_speed_and_message() {
        let ents = br#"
        { "classname" "func_train" "model" "*1" "target" "waterpath1" "speed" "100" }
        { "classname" "path_corner" "targetname" "waterpath1" "target" "waterpath2"
          "origin" "2676 -814 -872" "speed" "150" "message" "water_started" }
        { "classname" "path_corner" "targetname" "waterpath2"
          "origin" "1776 -814 -707" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[LOGIC_BRUSH_NONE, 3],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("extended train cook");

        let train = &logic.ents[0];
        assert_eq!(train.flags & LOGIC_TRAIN_EXTENDED, LOGIC_TRAIN_EXTENDED);
        assert_eq!(train.aux_count, 6);
        assert_eq!(logic.aux[2].delay_ticks, 150);
        assert_eq!(
            logic.names[logic.aux[2].target as usize - 1],
            "water_started"
        );
        assert_eq!(logic.aux[5].delay_ticks, 0);
    }

    #[test]
    fn global_func_train_cooks_stable_transition_identity_in_spare_arg0() {
        let ents = br#"
        { "classname" "func_train" "model" "*1" "target" "vent1"
          "globalname" "c1a1b_floor_vent1" }
        { "classname" "path_corner" "targetname" "vent1" "target" "vent2"
          "origin" "0 0 0" }
        { "classname" "path_corner" "targetname" "vent2" "origin" "0 0 64" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[LOGIC_BRUSH_NONE, 3],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("global train cook");

        let train = &logic.ents[0];
        assert_eq!(train.arg0, actor_carry_id("c1a1b_floor_vent1", true));
        assert_ne!(train.arg0, 0);
        assert_ne!(train.arg0 & CARRY_GLOBAL_BIT, 0);
    }

    #[test]
    fn skin_minus_three_func_train_cooks_as_moving_swimmable_water() {
        let ents = br#"
        { "classname" "func_train" "model" "*1" "skin" "-3"
          "rendermode" "2" "renderamt" "120" "target" "water1" }
        { "classname" "path_corner" "targetname" "water1" "target" "water2"
          "origin" "10 20 30" }
        { "classname" "path_corner" "targetname" "water2" "origin" "40 50 60" }
        "#;
        let mut models = vec![0u8; 2 * SZ_MODEL];
        for (offset, value) in [
            (0usize, -16.0f32),
            (4, -32.0),
            (8, -8.0),
            (12, 16.0),
            (16, 32.0),
            (20, 8.0),
        ] {
            let at = SZ_MODEL + offset;
            models[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }

        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 1);
        let water = &cooked[0];
        assert_eq!(water.kind & 0xff, 6);
        assert_eq!(water.kind >> 8, 1, "rendermode 2 remains translucent");
        assert_eq!(water.mv, [16, 8, 32]);
        assert_eq!((water.head, water.head0), (0, 0), "water is non-solid");
    }

    #[test]
    fn alpha_tested_func_wall_retains_cutout_visibility_class() {
        let ents = br#"
        { "classname" "func_wall" "model" "*1"
          "rendermode" "4" "renderamt" "255" }
        "#;
        let mut models = vec![0u8; 2 * SZ_MODEL];
        for (offset, value) in [
            (0usize, -16.0f32),
            (4, -32.0),
            (8, -8.0),
            (12, 16.0),
            (16, 32.0),
            (20, 8.0),
        ] {
            let at = SZ_MODEL + offset;
            models[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }

        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 1);
        assert_eq!(
            cooked[0].kind >> 8,
            3,
            "rendermode 4 must not collapse into an opaque visual occluder"
        );
    }

    #[test]
    fn cooks_rotating_button_with_authored_axis_and_travel() {
        let ents = br#"
        {
        "classname" "func_rot_button"
        "model" "*1"
        "origin" "10 20 30"
        "target" "water_doormm"
        "speed" "40"
        "distance" "90"
        "spawnflags" "66"
        }
        "#;
        let brush_by_submodel = [LOGIC_BRUSH_NONE, 7];
        let logic = collect_logic_entities(
            ents,
            &[],
            &brush_by_submodel,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");

        assert_eq!(logic.ents.len(), 1);
        assert_eq!(logic.ents[0].kind, LOGIC_FUNC_BUTTON);
        assert_eq!(logic.ents[0].brush, 7);
        assert_eq!(
            logic.names[logic.ents[0].target as usize - 1],
            "water_doormm"
        );

        // Two valid dmodel_t records are sufficient for the entity classifier.
        // SF 64 selects Gold roll / physical X and SF 2 reverses the swing.
        let models = vec![0u8; 2 * SZ_MODEL];
        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 1);
        assert_eq!(cooked[0].submodel, 1);
        assert_eq!(cooked[0].kind & 0xff, ENT_KIND_ROT_BUTTON);
        assert_eq!(cooked[0].mv, [1024, 1, 0]);
    }

    #[test]
    fn cooks_paired_momentary_wheels_and_use_set_door() {
        let ents = br#"
        { "classname" "momentary_rot_button" "model" "*1"
          "origin" "1358 -2028 -107" "targetname" "momwheel"
          "target" "momentary" "distance" "115" "speed" "35"
          "returnspeed" "5" "spawnflags" "17" }
        { "classname" "momentary_rot_button" "model" "*2"
          "origin" "1087 -2133 -129" "targetname" "momwheel"
          "target" "momentary" "distance" "115" "speed" "35"
          "returnspeed" "5" "spawnflags" "80" }
        { "classname" "momentary_door" "model" "*3"
          "targetname" "momentary" "speed" "100" }
        "#;
        let brush_by_submodel = [LOGIC_BRUSH_NONE, 4, 5, 6];
        let logic = collect_logic_entities(
            ents,
            &[],
            &brush_by_submodel,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("momentary logic cook");

        assert_eq!(logic.ents.len(), 3);
        let first = &logic.ents[0];
        let second = &logic.ents[1];
        let door = &logic.ents[2];
        assert_eq!(
            (first.kind, first.brush, first.speed, first.arg0),
            (LOGIC_MOMENTARY, 4, 35, 5)
        );
        assert_eq!(
            (second.kind, second.brush, second.speed, second.arg0),
            (LOGIC_MOMENTARY, 5, 35, 5)
        );
        assert_eq!(first.target, second.target);
        assert_eq!(logic.names[first.target as usize - 1], "momentary");
        assert_eq!(
            (door.kind, door.brush, door.targetname),
            (LOGIC_FUNC_DOOR, 6, first.target)
        );
    }

    #[test]
    fn spawnflags_preserve_high_signed_bits_without_fabricating_low_flags() {
        let high_unsigned = r#"{ "spawnflags" "2147483648" }"#;
        let high_signed = r#"{ "spawnflags" "-2147483648" }"#;
        let low_plus_high = r#"{ "spawnflags" "65538" }"#;
        assert_eq!(parse_spawnflags_u32(high_unsigned), 0x8000_0000);
        assert_eq!(parse_spawnflags_u32(high_signed), 0x8000_0000);
        assert_eq!(parse_spawnflags(high_unsigned), 0);
        assert_eq!(parse_spawnflags(low_plus_high), 2);
    }

    #[test]
    fn cooks_c1a2_targeted_fan_with_persistent_q16_spin_metadata() {
        let ents = br#"
        {
        "classname" "func_rotating"
        "model" "*1"
        "origin" "1860 -254 -532"
        "targetname" "fanpwr"
        "speed" "400"
        "fanfriction" "2"
        "spawnflags" "151"
        }
        "#;
        let brush_by_submodel = [LOGIC_BRUSH_NONE, 9];
        let logic = collect_logic_entities(
            ents,
            &[],
            &brush_by_submodel,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("fan logic cook");

        assert_eq!(logic.ents.len(), 1);
        let fan = &logic.ents[0];
        assert_eq!(fan.kind, LOGIC_FUNC_ROTATING);
        assert_eq!(fan.brush, 9);
        assert_eq!(fan.spawnflags, 151);
        assert_eq!(fan.speed, 400);
        assert_eq!(fan.arg0, 2, "fanfriction remains an authored percentage");
        assert_eq!(logic.names[fan.targetname as usize - 1], "fanpwr");

        let models = vec![0u8; 2 * SZ_MODEL];
        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 1);
        assert_eq!(cooked[0].kind & 0xff, 5);
        assert_eq!(cooked[0].origin, [1860, -532, -254]);
        assert_eq!(
            cooked[0].mv,
            [3641, 1, (2 << 16) | 151],
            "reverse GoldSrc Z/roll fan maps to reflected PSX X rotation"
        );
    }

    #[test]
    fn fan_profile_cooks_float_clamp_and_copivot_blocking_without_new_ram() {
        assert!(!fan_ramp_needs_extra_think(400.0, 2));
        assert!(fan_ramp_needs_extra_think(300.0, 10));
        assert!(fan_ramp_needs_extra_think(160.0, 20));
        assert!(fan_ramp_needs_extra_think(200.0, 20));
        assert!(!fan_ramp_needs_extra_think(200.0, 100));

        let ents = br#"
        { "classname" "func_rotating" "model" "*1" "origin" "4 5 6"
          "speed" "160" "fanfriction" "20" "spawnflags" "1" }
        { "classname" "func_rotating" "model" "*2" "origin" "4 5 6"
          "speed" "160" "fanfriction" "20" "spawnflags" "1" }
        { "classname" "func_rotating" "model" "*3" "origin" "4 5 6"
          "speed" "160" "fanfriction" "20" "spawnflags" "1" }
        "#;
        let mut models = vec![0u8; 4 * SZ_MODEL];
        for (submodel, mins, maxs) in [
            (1usize, [-5.0f32; 3], [5.0f32; 3]),
            (2, [5.0f32, -4.0, -4.0], [9.0f32, 4.0, 4.0]),
            (3, [20.0f32; 3], [30.0f32; 3]),
        ] {
            let base = submodel * SZ_MODEL;
            for axis in 0..3 {
                models[base + axis * 4..base + axis * 4 + 4]
                    .copy_from_slice(&mins[axis].to_le_bytes());
                models[base + 12 + axis * 4..base + 16 + axis * 4]
                    .copy_from_slice(&maxs[axis].to_le_bytes());
            }
        }

        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 3);
        let profile = |submodel: u16| {
            (cooked
                .iter()
                .find(|ent| ent.submodel == submodel)
                .unwrap()
                .mv[2] as u32
                >> 16) as u16
        };
        assert_eq!(
            profile(1),
            20 | FAN_PROFILE_RAMP_EXTRA_THINK | FAN_PROFILE_BLOCKED_AFTER_FIRST_SAMPLE
        );
        assert_eq!(profile(2), profile(1));
        assert_eq!(profile(3), 20 | FAN_PROFILE_RAMP_EXTRA_THINK);
    }

    #[test]
    fn cooks_c1a1b_pendulum_with_fixed_swing_and_axis_metadata() {
        let ents = br#"
        {
        "classname" "func_pendulum"
        "model" "*1"
        "origin" "1750 -429 -261"
        "distance" "3"
        "speed" "5"
        "spawnflags" "65"
        }
        "#;
        let brush_by_submodel = [LOGIC_BRUSH_NONE, 11];
        let logic = collect_logic_entities(
            ents,
            &[],
            &brush_by_submodel,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("pendulum logic cook");
        assert_eq!(logic.ents.len(), 1);
        let rec = &logic.ents[0];
        assert_eq!(rec.kind, LOGIC_FUNC_PENDULUM);
        assert_eq!(rec.brush, 11);
        assert_eq!(rec.spawnflags, 65);
        assert_eq!(rec.speed, 5);
        assert_eq!(rec.arg0, 3);

        let mut models = vec![0u8; 2 * SZ_MODEL];
        let mo = SZ_MODEL;
        for (off, value) in [
            (0, -132.0f32),
            (4, -2.0),
            (8, -11.0),
            (12, 132.0),
            (16, 1.0),
            (20, 1.0),
        ] {
            models[mo + off..mo + off + 4].copy_from_slice(&value.to_le_bytes());
        }
        models[mo + 36..mo + 40].copy_from_slice(&123i32.to_le_bytes());
        models[mo + 40..mo + 44].copy_from_slice(&456i32.to_le_bytes());
        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 1);
        let pendulum = &cooked[0];
        assert_eq!(pendulum.kind & 0xff, ENT_KIND_PENDULUM);
        assert_eq!(pendulum.origin, [1750, -261, -429]);
        assert_eq!(pendulum.mv[0], 2185, "1.5 degrees in Q19 turns");
        assert_eq!(pendulum.mv[1] as u32, 364u32 << 16, "PSX Y axis");
        assert_eq!(pendulum.mv[2] as u32, (15u32 << 16) | 65);
        assert_eq!((pendulum.head0, pendulum.head), (123, 456));
    }

    #[test]
    fn rotating_sweep_bounds_follow_each_goldsrc_axis() {
        let mins = [-2.0, -64.0, -10.0];
        let maxs = [18.0, 32.0, 20.0];
        assert_eq!(rotating_world_axis(0), 0, "default yaw -> PSX Y");
        assert_eq!(rotating_world_axis(4), 1, "Gold roll -> PSX X");
        assert_eq!(rotating_world_axis(8), 2, "Gold pitch -> PSX Z");
        assert_eq!(rotating_door_world_axis(64), 1, "door roll -> PSX X");
        assert_eq!(rotating_door_world_axis(128), 2, "door pitch -> PSX Z");
        assert_eq!(rotating_door_world_axis(0), 0, "door yaw -> PSX Y");

        let (roll_min, roll_max) = rotating_sweep_bounds(mins, maxs, 4);
        let roll_r = (64.0f32 * 64.0 + 20.0 * 20.0).sqrt();
        assert_eq!((roll_min[0], roll_max[0]), (-2.0, 18.0));
        assert!((roll_min[1] + roll_r).abs() < 0.001 && (roll_max[2] - roll_r).abs() < 0.001);

        let (pitch_min, pitch_max) = rotating_sweep_bounds(mins, maxs, 8);
        let pitch_r = (18.0f32 * 18.0 + 20.0 * 20.0).sqrt();
        assert_eq!((pitch_min[1], pitch_max[1]), (-64.0, 32.0));
        assert!((pitch_min[0] + pitch_r).abs() < 0.001 && (pitch_max[2] - pitch_r).abs() < 0.001);

        let (yaw_min, yaw_max) = rotating_sweep_bounds(mins, maxs, 0);
        let yaw_r = (18.0f32 * 18.0 + 64.0 * 64.0).sqrt();
        assert_eq!((yaw_min[2], yaw_max[2]), (-10.0, 20.0));
        assert!((yaw_min[0] + yaw_r).abs() < 0.001 && (yaw_max[1] - yaw_r).abs() < 0.001);

        let (door_z_min, door_z_max) = rotating_door_sweep_bounds(mins, maxs, 64);
        assert_eq!((door_z_min[0], door_z_max[0]), (-2.0, 18.0));
        assert!((door_z_min[1] + roll_r).abs() < 0.001);

        // A leaf boundary beyond the authored x=18 extent is still part of
        // the default yaw fan's ±66.48 sweep, preventing the blade from vanishing
        // as it rotates into that leaf.
        let mut planes = Vec::new();
        planes.extend_from_slice(&1.0f32.to_le_bytes());
        planes.extend_from_slice(&0.0f32.to_le_bytes());
        planes.extend_from_slice(&0.0f32.to_le_bytes());
        planes.extend_from_slice(&30.0f32.to_le_bytes());
        planes.extend_from_slice(&0i32.to_le_bytes());
        let mut nodes = Vec::new();
        nodes.extend_from_slice(&0i32.to_le_bytes());
        nodes.extend_from_slice(&(-2i16).to_le_bytes());
        nodes.extend_from_slice(&(-3i16).to_le_bytes());
        nodes.resize(SZ_NODE, 0);
        assert_eq!(
            entity_leafs(mins, maxs, [0.0; 3], None, &nodes, &planes),
            vec![2]
        );
        assert_eq!(
            entity_leafs(yaw_min, yaw_max, [0.0; 3], None, &nodes, &planes),
            vec![1, 2]
        );
    }

    #[test]
    fn rotating_base_euler_angles_are_reflected_and_packed() {
        assert_eq!(
            rotating_base_angles_q8(r#"{ "angles" "-45 180 90" }"#) as u32,
            0xc0_80_20
        );
    }

    #[test]
    fn ordered_legacy_angle_keys_match_goldsrc_dispatch() {
        assert_eq!(
            ent_angles_degrees(r#"{ "angles" "180 60 30" "angle" "129" }"#),
            Some([180.0, 129.0, 30.0]),
            "later non-negative angle replaces yaw but preserves pitch/roll"
        );
        assert_eq!(
            ent_angles_degrees(r#"{ "angle" "45" "angles" "10 20 30" }"#),
            Some([10.0, 20.0, 30.0]),
            "a later plural key overwrites the earlier legacy key"
        );
        assert_eq!(
            ent_angles_degrees(r#"{ "angles" "1 2 3" "angle" "-1" }"#),
            Some([-90.0, 0.0, 0.0])
        );
        let down = ent_move_dir(r#"{ "angle" "-2" }"#);
        assert!(down[0].abs() < 0.0001 && down[1].abs() < 0.0001 && down[2] < -0.9999);
    }

    #[test]
    fn prop_orientation_uses_existing_upper_word_for_pitch_and_roll() {
        assert_eq!(
            pack_prop_orientation([180.0, 129.0, 90.0], 3) as u32,
            0xc0_80_3e44
        );
    }

    #[test]
    fn rotating_doors_cook_source_axes_and_reflected_angles() {
        let ents = br#"
        { "classname" "func_door_rotating" "model" "*1" "origin" "334 242 -234"
          "distance" "148" "spawnflags" "96" }
        { "classname" "func_door_rotating" "model" "*2" "distance" "90"
          "spawnflags" "130" }
        { "classname" "func_door_rotating" "model" "*3" "distance" "90"
          "spawnflags" "0" }
        "#;
        let models = vec![0u8; 4 * SZ_MODEL];
        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 3);
        assert_eq!(cooked[0].origin, [334, -234, 242]);
        assert_eq!(cooked[0].mv, [-1684, 1, 0], "c1a0 wheel: roll -> PSX X");
        assert_eq!(cooked[1].mv, [1024, 2, 0], "reverse pitch -> PSX Z");
        assert_eq!(cooked[2].mv, [-1024, 0, 0], "c1a0 chair: yaw -> PSX Y");
    }

    #[test]
    fn c1a0e_cover_keeps_its_roll_swing_but_is_passable() {
        let ents = br#"
        { "classname" "func_door_rotating" "model" "*1"
          "origin" "1892 264 129" "targetname" "motor_button_cover"
          "angles" "0 0 0" "angle" "45"
          "rendermode" "2" "renderamt" "175" "distance" "160"
          "speed" "120" "spawnflags" "106" }
        { "classname" "func_button" "model" "*2"
          "target" "motor_button_cover" }
        "#;
        let mut models = vec![0u8; 3 * SZ_MODEL];
        models[SZ_MODEL + 36..SZ_MODEL + 40].copy_from_slice(&123i32.to_le_bytes());
        models[SZ_MODEL + 40..SZ_MODEL + 44].copy_from_slice(&456i32.to_le_bytes());
        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 2);
        assert_eq!(cooked[0].kind, 7 | (1 << 8));
        assert_eq!(cooked[0].origin, [1892, 129, 264]);
        assert_eq!(cooked[0].mv, [1820, 1, 0x00e000]);
        assert_eq!((cooked[0].head, cooked[0].head0), (0, 0));
    }

    #[test]
    fn c1a0_introchair_keeps_geometry_but_has_no_live_door_logic() {
        let chair = br#"
        { "classname" "func_door_rotating" "model" "*1"
          "origin" "-74 323 -220" "targetname" "introchair"
          "distance" "90" "speed" "65" "spawnflags" "58" }
        "#;
        let models = vec![0u8; 2 * SZ_MODEL];
        let cooked = collect_entities(chair, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 1);
        assert_eq!(cooked[0].kind & 0xff, 7);
        assert_eq!(
            cooked[0].mv[0], 1024,
            "pivot-local geometry must retain its authored 90-degree metadata"
        );
        let logic = collect_logic_entities(
            chair,
            &models,
            &[LOGIC_BRUSH_NONE, 0],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("dormant chair logic cook");
        assert!(logic.ents.is_empty(), "unreachable named door stays inert");

        let wired = br#"
        { "classname" "func_door_rotating" "model" "*1"
          "targetname" "introchair" "distance" "90" "spawnflags" "58" }
        { "classname" "trigger_relay" "target" "introchair" }
        "#;
        let cooked = collect_entities(wired, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked[0].mv[0], 1024, "a wired rotator must remain live");
        let logic = collect_logic_entities(
            wired,
            &models,
            &[LOGIC_BRUSH_NONE, 0],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("wired chair logic cook");
        assert!(
            logic.ents.iter().any(|rec| rec.kind == LOGIC_FUNC_DOOR),
            "a targeted rotating door must keep its live logic record"
        );
    }

    #[test]
    fn untargeted_rotating_brush_stays_on_cosmetic_path() {
        let ents = br#"
        { "classname" "func_rotating" "model" "*1" "speed" "200" "spawnflags" "65" }
        "#;
        let logic = collect_logic_entities(
            ents,
            &[],
            &[LOGIC_BRUSH_NONE, 4],
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("untargeted fan cook");
        assert!(logic.ents.is_empty());

        let mut models = vec![0u8; 2 * SZ_MODEL];
        models[SZ_MODEL + 36..SZ_MODEL + 40].copy_from_slice(&7i32.to_le_bytes());
        models[SZ_MODEL + 40..SZ_MODEL + 44].copy_from_slice(&8i32.to_le_bytes());
        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked[0].kind & 0xff, 5);
        assert_eq!(
            cooked[0].mv[2],
            (100 << 16) | 65,
            "START_ON and NOT_SOLID survive without a logic record"
        );
        assert_eq!((cooked[0].head, cooked[0].head0), (0, 0));
    }

    #[test]
    fn cooks_c1a0_platrot_as_local_synchronous_toggle() {
        let ents = br#"
        {
        "classname" "func_platrot"
        "model" "*1"
        "origin" "136 608 -218"
        "targetname" "ele_2"
        "speed" "80"
        "height" "-216"
        "rotation" "90"
        "spawnflags" "1"
        }
        "#;

        let mut models = vec![0u8; 2 * SZ_MODEL];
        let mo = SZ_MODEL;
        for (off, value) in [
            (0, -68.0f32),
            (4, -68.0),
            (8, -142.0),
            (12, 68.0),
            (16, 68.0),
            (20, 298.0),
        ] {
            models[mo + off..mo + off + 4].copy_from_slice(&value.to_le_bytes());
        }
        models[mo + 36..mo + 40].copy_from_slice(&123i32.to_le_bytes());
        models[mo + 40..mo + 44].copy_from_slice(&456i32.to_le_bytes());

        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 1);
        let plat = &cooked[0];
        assert_eq!(plat.kind & 0xff, ENT_KIND_PLATROT);
        assert_eq!(plat.submodel, 1);
        assert_eq!(
            plat.origin,
            [136, -2, 608],
            "phase zero is the physical bottom"
        );
        assert_eq!(plat.mv, [1024, -216, 0], "phase one is top + yaw 90");
        assert_eq!(plat.center, [0, 78, 0]);
        assert_eq!(plat.head0, 123);
        assert_eq!(plat.head, 456);

        let brush_by_submodel = [LOGIC_BRUSH_NONE, 7];
        let logic = collect_logic_entities(
            ents,
            &models,
            &brush_by_submodel,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("logic cook");
        assert_eq!(logic.ents.len(), 1);
        let rec = &logic.ents[0];
        assert_eq!(rec.kind, LOGIC_FUNC_DOOR);
        assert_eq!(rec.brush, 7);
        assert_eq!(rec.speed, 80);
        assert_eq!(rec.spawnflags, 1 | 32, "named TOP + SF_PLAT_TOGGLE");
        assert_eq!(logic.names[rec.targetname as usize - 1], "ele_2");
    }

    #[test]
    fn cooks_pushable_as_kind9_with_speed_lift_and_packed_bounds() {
        let ents = br#"
        {
        "classname" "func_pushable"
        "model" "*1"
        "origin" "1302 620 -504"
        "friction" "220"
        "health" "1"
        "material" "6"
        }
        "#;
        let mut models = vec![0u8; 2 * SZ_MODEL];
        let mo = SZ_MODEL;
        for (off, value) in [
            (0, -120.0f32),
            (4, -32.0),
            (8, -35.0),
            (12, 12.0),
            (16, 32.0),
            (20, 28.0),
        ] {
            models[mo + off..mo + off + 4].copy_from_slice(&value.to_le_bytes());
        }
        models[mo + 36..mo + 40].copy_from_slice(&123i32.to_le_bytes());
        models[mo + 40..mo + 44].copy_from_slice(&456i32.to_le_bytes());

        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked.len(), 1);
        let cart = &cooked[0];
        assert_eq!(cart.kind & 0xff, ENT_KIND_PUSHABLE);
        assert_eq!(
            cart.origin,
            [1302, -503, 620],
            "CPushable raises HL Z by one"
        );
        assert_eq!(
            cart.mv[0] as u32 & 0xff,
            9,
            "friction 220 => 180u/s => 9u/tick"
        );
        let collision_meta = cart.mv[0] as u16;
        assert_ne!(collision_meta & PUSHABLE_COLLISION_META_VALID, 0);
        assert_eq!(
            (collision_meta >> PUSHABLE_COLLISION_HULL_SHIFT) & 3,
            3,
            "134-unit SET_MODEL width selects GoldSrc large hull 2"
        );
        assert_eq!(
            (collision_meta >> PUSHABLE_COLLISION_MIN_CORR_SHIFT) & 7,
            0b101,
            "even X/Y bounds still need SET_MODEL's lower padding"
        );
        assert_eq!(cart.mv[0] as u32 >> 16, 66, "local half-X");
        assert_eq!((cart.mv[1], cart.mv[2]), (32, 32));
        assert_eq!((cart.head0, cart.head), (123, 456));
    }

    #[test]
    fn c1a0e_lift_travel_and_settled_cart_reach_probe_trigger() {
        // Exact stock c1a0e brush bounds/keys, remapped to compact synthetic
        // submodel indices. This guards the mandatory sample-delivery route:
        // lift *29 carries cart *30, then SF_PUSHABLES trigger *48 starts
        // probe_arm_mm once the player pushes the raised cart 208 HL units.
        let ents = br#"
        {
        "classname" "func_door"
        "model" "*1"
        "targetname" "sample_cart2_lift"
        "angle" "-1"
        "lip" "16"
        "speed" "30"
        "spawnflags" "32"
        }
        {
        "classname" "func_pushable"
        "model" "*2"
        "origin" "1302 620 -504"
        "targetname" "sample_cart2"
        "friction" "220"
        }
        {
        "classname" "trigger_once"
        "model" "*3"
        "target" "probe_arm_mm"
        "spawnflags" "6"
        }
        "#;
        let mut models = vec![0u8; 4 * SZ_MODEL];
        let mut put_bounds = |submodel: usize, mins: [f32; 3], maxs: [f32; 3]| {
            let mo = submodel * SZ_MODEL;
            for (axis, value) in mins.into_iter().chain(maxs).enumerate() {
                let off = mo + axis * 4;
                models[off..off + 4].copy_from_slice(&value.to_le_bytes());
            }
        };
        put_bounds(1, [1144.0, 560.0, -720.0], [1321.0, 680.0, -543.0]);
        put_bounds(2, [-120.0, -32.0, -35.0], [12.0, 32.0, 28.0]);
        put_bounds(3, [1522.0, 567.0, -360.0], [1616.0, 679.0, -305.0]);

        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        let lift = cooked.iter().find(|e| e.kind & 0xff == 1).unwrap();
        let cart = cooked
            .iter()
            .find(|e| e.kind & 0xff == ENT_KIND_PUSHABLE)
            .unwrap();
        assert_eq!(lift.mv, [0, 161, 0], "raw dmodel 177 - lip 16");

        let brushes = [LOGIC_BRUSH_NONE, 0, LOGIC_BRUSH_NONE, LOGIC_BRUSH_NONE];
        let logic = collect_logic_entities(
            ents,
            &models,
            &brushes,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("c1a0e delivery logic");
        let trigger = logic
            .ents
            .iter()
            .find(|rec| rec.kind == LOGIC_TRIGGER_ONCE)
            .unwrap();
        assert_eq!(
            trigger.spawnflags, 6,
            "NOCLIENTS and PUSHABLES stay independent"
        );

        let half = [
            (cart.mv[0] as u32 >> 16) as i32,
            cart.mv[1].abs(),
            cart.mv[2].abs(),
        ];
        let raised_and_pushed = [
            cart.origin[0] + 208,
            // GoldSrc gravity settles the authored five-unit gap onto the lift
            // before the lift carries the cart through its full travel.
            cart.origin[1] - 5 + lift.mv[1],
            cart.origin[2],
        ];
        let center = [
            raised_and_pushed[0] + cart.center[0],
            raised_and_pushed[1] + cart.center[1],
            raised_and_pushed[2] + cart.center[2],
        ];
        let cart_mins = [
            center[0] - half[0],
            center[1] - half[1],
            center[2] - half[2],
        ];
        let cart_maxs = [
            center[0] + half[0],
            center[1] + half[1],
            center[2] + half[2],
        ];
        assert_eq!(cart_mins, [1390, -383, 588]);
        assert_eq!(cart_maxs, [1522, -319, 652]);
        for axis in 0..3 {
            assert!(
                cart_mins[axis] <= trigger.maxs[axis] && cart_maxs[axis] >= trigger.mins[axis],
                "raised cart and probe trigger must overlap on axis {axis}"
            );
        }
    }

    #[test]
    fn diagonal_door_uses_raw_dmodel_size_after_link_pad_cancels() {
        let q = core::f32::consts::FRAC_1_SQRT_2;
        let (dir, dist) = door_move([q, q, 0.0], [0.0; 3], [102.0, 202.0, 52.0], 8.0);
        assert!((dir[0] - q).abs() < 0.0001 && (dir[1] - q).abs() < 0.0001);
        let sdk_dist = q * 102.0 + q * 202.0 - 8.0;
        assert!((dist - sdk_dist).abs() < 0.001);
        let double_subtracted_pad = q * 100.0 + q * 200.0 - 8.0;
        assert!((dist - double_subtracted_pad).abs() > 0.5);
    }

    #[test]
    fn pushable_only_cooks_breakable_logic_with_spawnflag_128() {
        let ordinary = br#"
        { "classname" "func_pushable" "model" "*1" "health" "1" "material" "6" }
        "#;
        let breakable = br#"
        { "classname" "func_pushable" "model" "*1" "spawnflags" "128"
          "health" "15" "material" "6" }
        "#;
        let brushes = [LOGIC_BRUSH_NONE, 7];
        let plain = collect_logic_entities(
            ordinary,
            &[],
            &brushes,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("ordinary pushable cook");
        assert!(
            plain.ents.is_empty(),
            "health alone must not make it breakable"
        );

        let armed = collect_logic_entities(
            breakable,
            &[],
            &brushes,
            1.0,
            &Default::default(),
            &Default::default(),
        )
        .expect("breakable pushable cook");
        assert_eq!(armed.ents.len(), 1);
        assert_eq!(armed.ents[0].kind, LOGIC_FUNC_BREAKABLE);
        assert_eq!(armed.ents[0].arg0, 15);
        assert_eq!(armed.ents[0].brush, 7);
    }

    #[test]
    fn platrot_keeps_signed_height_and_multi_turn_rotation() {
        let ents = br#"
        {
        "classname" "func_platrot"
        "model" "*1"
        "origin" "10 20 30"
        "targetname" "dn_3"
        "height" "-976"
        "rotation" "720"
        "angles" "0 0 0"
        "angle" "-2"
        "spawnflags" "1"
        }
        "#;
        let mut models = vec![0u8; 2 * SZ_MODEL];
        let mo = SZ_MODEL;
        for (off, value) in [
            (0, -16.0f32),
            (4, -32.0),
            (8, -8.0),
            (12, 16.0),
            (16, 32.0),
            (20, 8.0),
        ] {
            models[mo + off..mo + off + 4].copy_from_slice(&value.to_le_bytes());
        }
        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);
        assert_eq!(cooked[0].origin, [10, 1006, 20]);
        assert_eq!(
            cooked[0].mv,
            [8192, -976, 192],
            "the later legacy up-angle becomes the platform's base pitch"
        );
    }

    #[test]
    fn controller_volumes_do_not_cook_as_render_or_solid_entities() {
        let ents = br#"
        {
        "classname" "func_wall"
        "model" "*1"
        }
        {
        "classname" "func_friction"
        "model" "*2"
        "modifier" "20"
        }
        {
        "classname" "func_mortar_field"
        "model" "*3"
        "targetname" "mortar_field"
        }
        {
        "classname" "env_bubbles"
        "model" "*4"
        "density" "8"
        }
        "#;
        // Five valid dmodel_t records are enough for this classification test.
        // Empty BSP node/plane lumps make the visible control's PVS leaf list
        // empty but do not change whether it is emitted as an EntRec.
        let models = vec![0u8; 5 * SZ_MODEL];
        let cooked = collect_entities(ents, &models, &[], &[], 1.0, 0);

        assert_eq!(cooked.len(), 1, "only the visible func_wall is emitted");
        assert_eq!(cooked[0].submodel, 1);
        assert_eq!(cooked[0].kind & 0xff, 0, "control remains a solid brush");
        assert!(cooked.iter().all(|ent| ent.submodel != 2));
        assert!(cooked.iter().all(|ent| ent.submodel != 3));
        assert!(cooked.iter().all(|ent| ent.submodel != 4));
    }

    #[test]
    fn cooks_sprite_initial_toggle_and_once_state() {
        let ents = br#"
        { "classname" "env_sprite" "model" "sprites/test.spr" "origin" "1 2 3" }
        { "classname" "env_sprite" "model" "sprites/test.spr" "origin" "4 5 6" "targetname" "lamp" }
        { "classname" "env_sprite" "model" "sprites/test.spr" "origin" "7 8 9" "targetname" "lamp" "spawnflags" "1" }
        { "classname" "env_sprite" "model" "sprites/test.spr" "origin" "10 11 12" "targetname" "flash" "spawnflags" "3" }
        "#;
        let mut manifest = std::collections::HashMap::new();
        manifest.insert((0u16, "test.spr".to_string()), (3u16, 20u16, 10u16));
        let mut names = Vec::new();
        let sprites = collect_sprite_props(ents, &[], &[], 1.0, 0, &manifest, &mut names)
            .expect("sprite cook");

        assert_eq!(sprites.len(), 4);
        assert_ne!(sprites[0].3 & (1 << 4), 0, "unnamed sprites start on");
        assert_eq!(sprites[1].3 & (1 << 4), 0, "named sprite starts off");
        assert_ne!(sprites[2].3 & (1 << 4), 0, "STARTON is visible");
        assert_ne!(sprites[3].3 & (1 << 5), 0, "ONCE is preserved");
        assert_eq!(sprites[0].3 & 0xF, 3);
        assert_eq!(sprites[0].3 >> 6, 10);
        assert_eq!(names, vec!["lamp".to_string(), "flash".to_string()]);
        assert_eq!(sprites[1].2, sprites[2].2, "same targetname shares id");
    }

    #[test]
    fn tool_textures_are_not_renderable() {
        assert!(is_tool_texture("aaatrigger"));
        assert!(is_tool_texture("clip"));
        assert!(is_tool_texture("origin"));
        assert!(is_tool_texture("sky"));
        assert!(!is_tool_texture("c1a0_labw5"));
    }

    fn test_tex(w: u16) -> CookedTex {
        let mut tex = placeholder_tex();
        tex.w = w;
        tex.h = 8;
        tex.pix4 = vec![0; (w as usize * tex.h as usize) / 2];
        tex
    }

    #[test]
    fn compact_used_textures_remaps_triangle_texture_ids() {
        let texs = vec![test_tex(8), test_tex(16), test_tex(32), test_tex(64)];
        let mut tri_tex = vec![2, 0, 2, 1];

        let (compact, stripped, remap) = compact_used_textures(texs, &mut tri_tex, &[]);

        assert_eq!(stripped, 1);
        assert_eq!(tri_tex, vec![0, 1, 0, 2]);
        assert_eq!(compact.len(), 3);
        assert_eq!(compact[0].w, 32);
        assert_eq!(compact[1].w, 8);
        assert_eq!(compact[2].w, 16);
        assert_eq!(remap, vec![1, 2, 0, u16::MAX]);
    }

    #[test]
    fn compact_used_textures_retains_animation_chain_frames() {
        let texs = vec![test_tex(8), test_tex(16), test_tex(32), test_tex(64)];
        let mut tri_tex = vec![2];

        // Ids 0 and 3 are chain siblings of a referenced frame: kept and
        // remapped even though no triangle references them. Id 1 still drops.
        let (compact, stripped, remap) = compact_used_textures(texs, &mut tri_tex, &[0, 3]);

        assert_eq!(stripped, 1);
        assert_eq!(compact.len(), 3);
        assert_eq!(remap, vec![1, u16::MAX, 0, 2]);
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tex_anim_chains_group_primary_and_alternate_frames() {
        // Deliberately out of order, mixed case, with an unrelated static
        // texture in between (grouping is on the name after the +N prefix).
        let chains = collect_tex_anim_chains(&names(&[
            "+1generic_113",
            "crate01",
            "+Ageneric_113",
            "+0generic_113",
            "+0lab_pc",
            "+1lab_pc",
        ]));

        assert_eq!(
            chains,
            vec![
                TexAnimChain {
                    primary: vec![3, 0],
                    alt: vec![2],
                },
                TexAnimChain {
                    primary: vec![4, 5],
                    alt: vec![],
                },
            ]
        );
    }

    #[test]
    fn animated_face_keeps_its_authored_alternate_texture_through_compaction() {
        // AM-04 regression shape (c1a0d retinal scanner): the face references
        // ONLY the alternate +a frame; the primary +0/+1 frames enter solely
        // via chain retention. The face must still resolve to +ax after the
        // remap, and the cooked primary chain must carry BOTH +0x and +1x.
        let texs = vec![test_tex(8), test_tex(16), test_tex(32), test_tex(64)];
        let tex_names = names(&["plain", "+0x", "+1x", "+ax"]);
        let mut tri_tex = vec![0, 3, 3];

        let (compact, stripped, chains) =
            compact_textures_with_anim_chains(texs, &mut tri_tex, &tex_names);

        assert_eq!(stripped, 0); // +0x/+1x survive via the chain keep-list
        assert_eq!(compact.len(), 4);
        // The scanner face still points at its authored +ax image (w=64).
        assert_eq!(tri_tex, vec![0, 1, 1]);
        assert_eq!(compact[1].w, 64);
        // The chain kept both primary frames + the alternate, in compact ids
        // bound to the right images (+0x w=16, +1x w=32).
        assert_eq!(
            chains,
            vec![TexAnimChain {
                primary: vec![2, 3],
                alt: vec![1],
            }]
        );
        assert_eq!(compact[2].w, 16);
        assert_eq!(compact[3].w, 32);
    }

    #[test]
    fn tex_anim_chains_drop_gapped_and_single_frame_groups() {
        let chains = collect_tex_anim_chains(&names(&[
            "+0broken", "+2broken", // gap at frame 1: not animated
            "+0lonely", // one frame, no alternate: nothing to cycle or swap
        ]));
        assert!(chains.is_empty());
    }

    #[test]
    fn impact_material_lookup_matches_goldsrc_prefix_and_twelve_char_rules() {
        let materials = HashMap::from([
            ("WOOD_PANEL_0".to_string(), 7),
            ("GENERIC015V".to_string(), 4),
        ]);
        assert_eq!(texture_sound_material("{WOOD_PANEL_0_extra", &materials), 7);
        assert_eq!(texture_sound_material("+0GENERIC015V", &materials), 4);
        assert_eq!(texture_sound_material("UNTAGGED", &materials), 0);
    }

    fn put_clipnode(buf: &mut Vec<u8>, planenum: i32, c0: i16, c1: i16) {
        buf.extend_from_slice(&planenum.to_le_bytes());
        buf.extend_from_slice(&c0.to_le_bytes());
        buf.extend_from_slice(&c1.to_le_bytes());
    }

    #[test]
    fn q14_plane_keeps_hazard_course_hull_boundary_solid() {
        // Exact t0a0 hull-1 plane and player origin recovered from PSoXide
        // save slot 8. Q12 rounds this plane four Q5 units toward empty at the
        // large negative coordinates, opening a one-cell strip through the
        // expanded wall. Compatible Q14 preserves the source plane's side.
        let mut planes = vec![0u8; SZ_PLANE];
        for (off, value) in [
            (0, 0.7808688282966614f32),
            (4, 0.0f32),
            (8, 0.6246950626373291f32),
            (12, -1193.0113525390625f32),
        ] {
            planes[off..off + 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        let p = [-1507, -26, -1788];
        let (q12_n, _) = plane_rec(&planes, 0, 1.0);
        let (_, q5_d) = plane_rec_q5(&planes, 0, 1.0);
        let q12_delta =
            ((q12_n[0] as i32 * p[0] + q12_n[1] as i32 * p[1] + q12_n[2] as i32 * p[2]) >> 7)
                - q5_d;
        assert_eq!(q12_delta, 4, "the old Q12 cook must reproduce the seam");

        let (q14_n, q5_d) = plane_rec_q5(&planes, 0, 1.0);
        let q14_delta =
            ((q14_n[0] as i32 * p[0] + q14_n[1] as i32 * p[1] + q14_n[2] as i32 * p[2]) >> 9)
                - q5_d;
        assert!(
            q14_delta < 0,
            "captured origin must remain on the solid side"
        );

        let recover_q12 = |value: i16| {
            let value = value as i32;
            if value < 0 {
                -((-value) >> 2)
            } else {
                value >> 2
            }
        };
        assert_eq!(
            q14_n.map(recover_q12),
            q12_n.map(i32::from),
            "impact response stays bit-identical to the established Q12 normal"
        );
    }

    #[test]
    fn reads_all_four_dmodel_headnodes_at_their_goldsrc_offsets() {
        let mut models = vec![0u8; SZ_MODEL];
        for (hull, value) in [101i32, 202, 303, 404].into_iter().enumerate() {
            let off = SZ_MODEL_HEADNODE0 + hull * core::mem::size_of::<i32>();
            models[off..off + 4].copy_from_slice(&value.to_le_bytes());
        }

        assert_eq!(model_headnode(&models, 0, 0), Some(101));
        assert_eq!(model_headnode(&models, 0, 1), Some(202));
        assert_eq!(model_headnode(&models, 0, 2), Some(303));
        assert_eq!(model_headnode(&models, 0, 3), Some(404));
        assert_eq!(model_headnode(&models, 0, 4), None);
    }

    #[test]
    fn clip_plane_ref_tags_exact_positive_axes() {
        assert_eq!(
            pack_clip_plane_ref(7, [4096, 0, 0]).unwrap(),
            CLIP_PLANE_TAG_X | 7
        );
        assert_eq!(
            pack_clip_plane_ref(11, [0, 4096, 0]).unwrap(),
            CLIP_PLANE_TAG_Y | 11
        );
        assert_eq!(
            pack_clip_plane_ref(13, [0, 0, 4096]).unwrap(),
            CLIP_PLANE_TAG_Z | 13
        );
    }

    #[test]
    fn clip_plane_ref_leaves_non_axial_normals_generic() {
        assert_eq!(pack_clip_plane_ref(19, [4095, 0, 0]).unwrap(), 19);
        assert_eq!(pack_clip_plane_ref(19, [4096, 1, 0]).unwrap(), 19);
        assert_eq!(pack_clip_plane_ref(19, [2365, 2365, 2366]).unwrap(), 19);
    }

    #[test]
    fn clip_plane_ref_leaves_negative_axes_generic() {
        assert_eq!(pack_clip_plane_ref(23, [-4096, 0, 0]).unwrap(), 23);
        assert_eq!(pack_clip_plane_ref(23, [0, -4096, 0]).unwrap(), 23);
        assert_eq!(pack_clip_plane_ref(23, [0, 0, -4096]).unwrap(), 23);
    }

    #[test]
    fn clip_plane_ref_enforces_14_bit_index_without_truncation() {
        assert_eq!(CLIP_PLANE_INDEX_MASK, 16_383);
        assert_eq!(pack_clip_plane_ref(16_383, [0, 0, 4096]).unwrap(), 0xffff);
        let err = pack_clip_plane_ref(16_384, [4096, 0, 0]).unwrap_err();
        assert!(err.contains("16384"));
        assert!(err.contains("14-bit"));
    }

    #[test]
    fn compact_clipnodes_dedups_and_keeps_only_reachable() {
        let mut clipnodes = Vec::new();
        put_clipnode(&mut clipnodes, 0, 1, 3); // root: two structurally identical children
        put_clipnode(&mut clipnodes, 5, -1, -2); // subtree A
        put_clipnode(&mut clipnodes, 9, -2, -2); // unreachable -> stripped
        put_clipnode(&mut clipnodes, 5, -1, -2); // subtree B == A -> collapses

        let (remap, out) = compact_clipnode_remap(&clipnodes, &[0]);

        assert_eq!(remap[2], -1); // unreachable stays stripped
        assert_eq!(remap[1], remap[3]); // identical subtrees share one id
        assert_eq!(out.len(), 2); // root + the shared leaf
        let root = remap_clip_head(0, &remap) as usize;
        let (plane, c0, c1) = out[root];
        assert_eq!(plane, 0);
        assert_eq!(c0, c1); // both children point at the shared node
        assert_eq!(out[c0 as usize], (5, -1, -2));
    }

    #[test]
    fn cooked_leaf_mark_count_preserves_goldsrc_liquid_class_without_growth() {
        assert_eq!(pack_leaf_mark_count(37, -1).unwrap(), 37);
        assert_eq!(pack_leaf_mark_count(37, -2).unwrap(), 37);
        assert_eq!(pack_leaf_mark_count(37, -3).unwrap(), 37 | (1 << 14));
        assert_eq!(pack_leaf_mark_count(37, -4).unwrap(), 37 | (2 << 14));
        assert_eq!(pack_leaf_mark_count(37, -5).unwrap(), 37 | (3 << 14));
        assert!(pack_leaf_mark_count(cooked::LEAF_MARK_COUNT_MASK + 1, -3).is_err());
    }

    #[test]
    fn tjunction_weld_orders_equal_projection_vertices_deterministically() {
        let expected = vec![4, 0, 2, 2, 3, 1, 2, 1, 4];
        for _ in 0..32 {
            // Vertices 2 and 3 have the same projection onto the long 0->1
            // edge but sit on opposite sides within the two-unit weld epsilon.
            // Their spatial-bucket insertion order must not choose the cook.
            let verts = vec![
                [0, 0, 0],
                [200, 0, 0],
                [100, 1, 0],
                [100, -1, 0],
                [0, 200, 0],
            ];
            let mut tri_idx = vec![0, 1, 4];
            let mut tri_tex = vec![0];
            let mut tri_uv = vec![0, 0, 200, 0, 0, 200];
            let mut tri_rgb = vec![128; 9];
            let mut face_first = vec![0];
            let mut face_ntri = vec![1];
            let face_refined = vec![false];
            let mut cell_pair_first = std::collections::HashSet::new();

            let added = weld_tjunctions(
                &verts,
                &mut tri_idx,
                &mut tri_tex,
                &mut tri_uv,
                &mut tri_rgb,
                &mut face_first,
                &mut face_ntri,
                &face_refined,
                &mut cell_pair_first,
                8,
                usize::MAX,
            );

            assert_eq!(added, 2);
            assert_eq!(tri_idx, expected);
            assert_eq!(face_first, vec![0]);
            assert_eq!(face_ntri, vec![3]);
        }
    }

    #[test]
    fn boundary_ears_retain_opposite_edge_junctions_in_every_rotation() {
        let verts = [
            [0, 0, 0],
            [128, 0, 0],
            [96, 32, 0],
            [64, 64, 0],
            [0, 128, 0],
        ];
        for reverse in [false, true] {
            for rotation in 0..verts.len() {
                let mut boundary: Vec<_> = (0..verts.len())
                    .map(|k| RefineCorner {
                        idx: ((k + rotation) % verts.len()) as u16,
                        uv: [0; 2],
                        rgb: [128; 3],
                    })
                    .collect();
                if reverse {
                    boundary.reverse();
                }
                let (mut i, mut t, mut u, mut r) = (vec![], vec![], vec![], vec![]);
                assert_eq!(
                    triangulate_refined_boundary(
                        &boundary, &verts, 0, &mut i, &mut t, &mut u, &mut r, true
                    ),
                    3
                );
                let mut area = 0i64;
                for tri in i.chunks_exact(3) {
                    let [a, b, c] = [
                        verts[tri[0] as usize],
                        verts[tri[1] as usize],
                        verts[tri[2] as usize],
                    ];
                    let cross = (b[0] as i64 - a[0] as i64) * (c[1] as i64 - a[1] as i64)
                        - (b[1] as i64 - a[1] as i64) * (c[0] as i64 - a[0] as i64);
                    assert!(if reverse { cross < 0 } else { cross > 0 });
                    area += cross.abs();
                }
                assert_eq!(area, 128 * 128);
                for k in 0..5 {
                    assert!(i.contains(&k));
                }
            }
        }
    }

    #[test]
    fn tjunction_weld_preserves_an_original_quad_diagonal() {
        // Vertex 4 lies exactly on the source quad's 0--2 fan diagonal. It may
        // belong to unrelated coplanar geometry, but it cannot create a crack:
        // both triangles of this face already own the complete shared edge.
        let verts = vec![
            [0, 0, 0],
            [200, 0, 0],
            [200, 200, 0],
            [0, 200, 0],
            [100, 100, 0],
        ];
        let mut tri_idx = vec![0, 1, 2, 0, 2, 3];
        let original_idx = tri_idx.clone();
        let mut tri_tex = vec![0, 0];
        let mut tri_uv = vec![0, 0, 200, 0, 200, 200, 0, 0, 200, 200, 0, 200];
        let mut tri_rgb = vec![128; 18];
        let mut face_first = vec![0];
        let mut face_ntri = vec![2];
        let face_refined = vec![false];
        let mut cell_pair_first = std::collections::HashSet::new();

        let added = weld_tjunctions(
            &verts,
            &mut tri_idx,
            &mut tri_tex,
            &mut tri_uv,
            &mut tri_rgb,
            &mut face_first,
            &mut face_ntri,
            &face_refined,
            &mut cell_pair_first,
            8,
            usize::MAX,
        );

        assert_eq!(added, 0);
        assert_eq!(tri_idx, original_idx);
        assert_eq!(face_ntri, vec![2]);
    }

    #[test]
    fn affine_refinement_splits_both_sides_of_a_shared_edge() {
        let mut verts = vec![[0, 0, 0], [200, 0, 0], [100, 20, 0], [100, -20, 0]];
        let mut tri_idx = vec![0, 1, 2, 1, 0, 3];
        let mut tri_tex = vec![0, 0];
        let mut tri_uv = vec![0, 0, 80, 0, 40, 0, 80, 0, 0, 0, 40, 0];
        let mut tri_rgb = vec![128; 18];
        let mut face_first = vec![0, 1];
        let mut face_ntri = vec![1, 1];
        let mut face_refined = vec![false; 2];

        let (added_verts, added_tris, changed_faces) = refine_affine_edges(
            &mut verts,
            &mut tri_idx,
            &mut tri_tex,
            &mut tri_uv,
            &mut tri_rgb,
            &mut face_first,
            &mut face_ntri,
            &[true, false],
            &mut face_refined,
            2,
        );

        assert_eq!((added_verts, added_tris, changed_faces), (1, 2, 2));
        assert_eq!(verts[4], [100, 0, 0]);
        assert_eq!(face_first, vec![0, 2]);
        assert_eq!(face_ntri, vec![2, 2]);
        assert_eq!(face_refined, vec![true, true]);
        assert_eq!(tri_idx.iter().filter(|&&i| i == 4).count(), 4);
        for tri in tri_idx.chunks_exact(3) {
            let a = verts[tri[0] as usize];
            let b = verts[tri[1] as usize];
            let c = verts[tri[2] as usize];
            let area = (b[0] as i64 - a[0] as i64) * (c[1] as i64 - a[1] as i64)
                - (b[1] as i64 - a[1] as i64) * (c[0] as i64 - a[0] as i64);
            assert_ne!(area, 0, "refinement emitted a collinear filler triangle");
        }
    }

    #[test]
    fn affine_split_uses_only_exact_integer_points_on_the_source_edge() {
        let splittable = RefineEdge([0, 0, 0], [402, 201, 0]);
        let (point, numerator, denominator) = exact_edge_split(splittable).unwrap();
        assert_eq!((point, numerator, denominator), ([200, 100, 0], 100, 201));
        assert_eq!(
            (point[0] as i32) * 201,
            (splittable.1[0] as i32 - splittable.0[0] as i32) * 100
        );
        assert_eq!(
            (point[1] as i32) * 201,
            (splittable.1[1] as i32 - splittable.0[1] as i32) * 100
        );

        assert_eq!(
            exact_edge_split(RefineEdge([0, 0, 0], [5, 3, 0])),
            None,
            "a primitive lattice edge has no exact interior integer point"
        );
    }

    #[test]
    fn affine_refinement_all_edge_masks_preserve_area_without_fillers() {
        for budget in 1..=3 {
            let mut verts = vec![[0, 0, 0], [256, 0, 0], [0, 256, 0]];
            let mut tri_idx = vec![0, 1, 2];
            let mut tri_tex = vec![0];
            let mut tri_uv = vec![0, 0, 80, 0, 0, 80];
            let mut tri_rgb = vec![128; 9];
            let mut face_first = vec![0];
            let mut face_ntri = vec![1];
            let mut face_refined = vec![false];

            let (added_verts, added_tris, changed_faces) = refine_affine_edges(
                &mut verts,
                &mut tri_idx,
                &mut tri_tex,
                &mut tri_uv,
                &mut tri_rgb,
                &mut face_first,
                &mut face_ntri,
                &[true],
                &mut face_refined,
                budget,
            );

            assert_eq!(
                (added_verts, added_tris, changed_faces),
                (budget, budget, 1)
            );
            assert_eq!(face_ntri, vec![(budget + 1) as u16]);
            let mut area_sum = 0i64;
            for tri in tri_idx.chunks_exact(3) {
                let a = verts[tri[0] as usize];
                let b = verts[tri[1] as usize];
                let c = verts[tri[2] as usize];
                let area = (b[0] as i64 - a[0] as i64) * (c[1] as i64 - a[1] as i64)
                    - (b[1] as i64 - a[1] as i64) * (c[0] as i64 - a[0] as i64);
                assert!(
                    area > 0,
                    "budget {budget} emitted a filler or flipped triangle"
                );
                area_sum += area;
            }
            assert_eq!(area_sum, 256 * 256, "budget {budget} changed source area");
        }
    }

    #[test]
    fn affine_subdivision_candidates_include_close_wall_scale() {
        let wide_uv = [(0.0, 0.0), (96.0, 0.0), (96.0, 96.0), (0.0, 96.0)];
        let close_uv = [(0.0, 0.0), (32.0, 0.0), (32.0, 32.0), (0.0, 32.0)];
        let narrow_uv = [(0.0, 0.0), (31.0, 0.0), (31.0, 31.0), (0.0, 31.0)];
        assert!(affine_subdivision_candidate([256, 8, 256], &wide_uv));
        assert!(!affine_subdivision_candidate([64, 8, 64], &wide_uv));
        assert!(affine_subdivision_candidate([96, 8, 96], &close_uv));
        assert!(!affine_subdivision_candidate([256, 8, 256], &narrow_uv));
    }

    #[test]
    fn affine_budget_prefers_cheap_traversed_faces() {
        let samples = [[0, 0, 0]];
        let near = affine_proximity_boost([200, 0, 0], [16, 16, 16], &samples);
        let far = affine_proximity_boost([4096, 0, 0], [16, 16, 16], &samples);
        assert_eq!(near, 8);
        assert_eq!(far, 1);
        assert_eq!(affine_proximity_boost([0, 0, 0], [16; 3], &[]), 1);

        let cheap_near = affine_budget_priority(1_000, near, 4);
        let expensive_far = affine_budget_priority(8_000, far, 64);
        assert!(cheap_near > expensive_far);
        assert_eq!(affine_budget_priority(1_000, near, 0), 0);
    }

    #[test]
    fn affine_budget_prioritizes_binary_alpha_cutouts() {
        assert_eq!(affine_texture_detail_boost("{grate4a"), 16);
        assert_eq!(affine_texture_detail_boost("  {ladder1"), 16);
        assert_eq!(affine_texture_detail_boost("concrete"), 1);
        let opaque = affine_budget_priority(4_000, 1, 16);
        let cutout = affine_budget_priority(4_000, affine_texture_detail_boost("{grate4a"), 16);
        assert_eq!(cutout, opaque * 16);
    }

    #[test]
    fn coplanar_material_guard_ignores_face_orientation() {
        let normal = [0, 4096, 0];
        assert!(face_planes_match_unoriented(normal, -1408, normal, -1408));
        assert!(face_planes_match_unoriented(
            normal,
            -1408,
            [0, -4096, 0],
            1408
        ));
        assert!(!face_planes_match_unoriented(normal, -1408, normal, -1407));
    }

    #[test]
    fn uv_grid_builds_regular_quad_cells_instead_of_a_face_fan() {
        let mut verts = vec![[0, 0, 0], [256, 0, 0], [256, 256, 0], [0, 256, 0]];
        let source = [
            CookCorner {
                idx: 0,
                pos: verts[0],
                uv: (0.0, 0.0),
                shade: (128, 128, 128),
            },
            CookCorner {
                idx: 1,
                pos: verts[1],
                uv: (256.0, 0.0),
                shade: (128, 128, 128),
            },
            CookCorner {
                idx: 2,
                pos: verts[2],
                uv: (256.0, 256.0),
                shade: (128, 128, 128),
            },
            CookCorner {
                idx: 3,
                pos: verts[3],
                uv: (0.0, 256.0),
                shade: (128, 128, 128),
            },
        ];
        let mut tri_idx = Vec::new();
        let mut tri_tex = Vec::new();
        let mut tri_uv = Vec::new();
        let mut tri_rgb = Vec::new();
        let stats = emit_uv_grid_poly(
            &source,
            7,
            64.0,
            &mut verts,
            &mut tri_idx,
            &mut tri_tex,
            &mut tri_uv,
            &mut tri_rgb,
        )
        .unwrap();

        assert_eq!(stats.cells, 16);
        assert_eq!(stats.quad_cells, 16);
        assert_eq!(stats.boundary_tris, 0);
        assert_eq!(stats.pair_first, (0..32).step_by(2).collect::<Vec<_>>());
        assert_eq!(tri_tex.len(), 32);
        assert_eq!(verts.len(), 25, "a 4x4 cell grid has a 5x5 vertex lattice");
        for tri in tri_idx.chunks_exact(3) {
            let corners = [tri[0], tri[1], tri[2]].map(|idx| CookCorner {
                idx,
                pos: verts[idx as usize],
                uv: (0.0, 0.0),
                shade: (0, 0, 0),
            });
            assert_ne!(grid_cross_len2(&verts, corners), 0);
        }
        for uv in tri_uv.chunks_exact(6) {
            let min_u = uv[0].min(uv[2]).min(uv[4]);
            let max_u = uv[0].max(uv[2]).max(uv[4]);
            let min_v = uv[1].min(uv[3]).min(uv[5]);
            let max_v = uv[1].max(uv[3]).max(uv[5]);
            assert!(max_u - min_u <= 64 && max_v - min_v <= 64);
        }
    }

    #[test]
    fn uv_seam_split_keeps_wrapped_bytes_local() {
        let mut verts = vec![[0, 0, 0], [32, 0, 0], [0, 32, 0]];
        let corners = [
            CookCorner {
                idx: 0,
                pos: verts[0],
                uv: (250.0, 12.0),
                shade: (10, 20, 30),
            },
            CookCorner {
                idx: 1,
                pos: verts[1],
                uv: (270.0, 12.0),
                shade: (30, 40, 50),
            },
            CookCorner {
                idx: 2,
                pos: verts[2],
                uv: (260.0, 44.0),
                shade: (50, 60, 70),
            },
        ];
        let mut tri_idx = Vec::new();
        let mut tri_tex = Vec::new();
        let mut tri_uv = Vec::new();
        let mut tri_rgb = Vec::new();

        emit_cooked_tri(
            corners,
            9,
            UV_SPLIT_DEPTH,
            &mut verts,
            &mut tri_idx,
            &mut tri_tex,
            &mut tri_uv,
            &mut tri_rgb,
        );

        assert!(tri_idx.len() / 3 > 1);
        assert_eq!(tri_tex.len(), tri_idx.len() / 3);
        for uv in tri_uv.chunks_exact(6) {
            let min_u = uv[0].min(uv[2]).min(uv[4]);
            let max_u = uv[0].max(uv[2]).max(uv[4]);
            assert!(
                max_u - min_u <= 24,
                "triangle crosses the 255->0 byte seam: {:?}",
                uv
            );
        }
    }

    #[test]
    fn logical_quad_collapse_removes_collinear_tjunction_points() {
        let verts = vec![
            [0, 0, 0],
            [8, 0, 0],
            [16, 0, 0],
            [16, 8, 0],
            [16, 16, 0],
            [8, 16, 0],
            [0, 16, 0],
            [0, 8, 0],
        ];
        let source = [0, 1, 2, 3, 4, 5, 6, 7];
        let logical = collapse_collinear_loop(&source, &verts, verts.len());
        assert_eq!(logical, vec![0, 2, 4, 6]);
        assert_eq!(
            collapsed_boundary_spans(&source, &logical),
            vec![(0, 2), (2, 4), (4, 6), (6, 0)]
        );

        // A logical edge that still crosses a retained T-junction point is
        // blocked, while the welded sub-segments on either side are safe and
        // receive independent canonical edge IDs.
        let tri_idx = vec![0, 2, 6, 4, 1];
        assert_eq!(
            patch_blocked_edge_mask([0, 1, 2, 3], &tri_idx, &verts, &[(verts[0], verts[2])],),
            1
        );
        let segment = vec![0, 1, 7, 6];
        assert_eq!(
            patch_blocked_edge_mask([0, 1, 2, 3], &segment, &verts, &[(verts[0], verts[2])],) & 1,
            0
        );
    }

    /// Pin the native-patch facevert flag contract the runtime decodes:
    /// slot 3 bit 15 marks a triangle record, bit 14 is the independent
    /// blocked-edge flag, and the vertex index owns only the low 14 bits.
    #[test]
    fn patch_facevert_flags_use_distinct_slots() {
        let tri_idx: Vec<u16> = vec![5, 6, 7, 8];
        let tri_uv = vec![0u8; 8];
        let light_idx = vec![0u8; 4];
        let slot_idx =
            |dst: &[u8], slot: usize| u16::from_le_bytes([dst[slot * 5], dst[slot * 5 + 1]]);

        // Quad record with a blocked edge on slot 1.
        let mut quad = Vec::new();
        for slot in 0..4 {
            push_patch_facevert(
                &mut quad,
                &tri_idx,
                &tri_uv,
                &light_idx,
                slot,
                false,
                slot == 1,
            );
        }
        assert_eq!(quad.len(), 4 * 5);
        for slot in 0..4 {
            let word = slot_idx(&quad, slot);
            assert_eq!(word & cooked::LOOPVERT_INDEX_MASK, tri_idx[slot]);
            assert_eq!(word & cooked::LOOPVERT_TRIANGLE_FLAG, 0);
            assert_eq!(
                word & cooked::LOOPVERT_BLOCKED_EDGE_FLAG != 0,
                slot == 1,
                "blocked bit only on slot 1"
            );
        }

        // Triangle record: marker rides the repeated slot-3 corner only.
        let mut triangle = Vec::new();
        push_triangle_patch_corners(
            &mut triangle,
            &tri_idx,
            &tri_uv,
            &light_idx,
            [0, 1, 2],
            false,
        );
        assert_eq!(triangle.len(), 4 * 5);
        assert_eq!(slot_idx(&triangle, 0) & cooked::LOOPVERT_TRIANGLE_FLAG, 0);
        assert_ne!(slot_idx(&triangle, 3) & cooked::LOOPVERT_TRIANGLE_FLAG, 0);
        assert_eq!(
            slot_idx(&triangle, 3) & cooked::LOOPVERT_INDEX_MASK,
            tri_idx[2]
        );

        // Mirror of the runtime decode in `Map::patch_corners_meta`.
        let decode = |dst: &[u8]| {
            let flags: Vec<u16> = (0..4).map(|slot| slot_idx(dst, slot) >> 14).collect();
            let is_triangle = flags[3] & 2 != 0;
            let blocked = (flags[0] & 1)
                | ((flags[1] & 1) << 1)
                | ((flags[3] & 1) << 2)
                | ((flags[2] & 1) << 3);
            (is_triangle, blocked)
        };
        assert_eq!(decode(&quad), (false, 2));
        assert_eq!(decode(&triangle), (true, 0));
    }

    #[test]
    fn logical_quad_collapse_recovers_rounded_generated_edge_points_only() {
        let verts = vec![
            [0, 0, 0],
            [16, 0, 0],
            [16, 16, 0],
            [0, 16, 0],
            // A source-space midpoint rounded one packed unit off the edge.
            [8, 1, 0],
            // The same geometry as an authored corner must remain a bevel.
            [8, 1, 0],
        ];
        assert_eq!(
            collapse_collinear_loop(&[0, 4, 1, 2, 3], &verts, 4),
            vec![0, 1, 2, 3]
        );
        assert_eq!(
            collapse_collinear_loop(&[0, 5, 1, 2, 3], &verts, 6),
            vec![0, 5, 1, 2, 3]
        );
    }

    #[test]
    fn irregular_grid_cell_pairs_boundary_fan_without_a_center_star() {
        let mut verts = vec![[0, 0, 0], [8, 0, 0], [16, 8, 0], [8, 16, 0], [0, 8, 0]];
        let source = (0..5)
            .map(|idx| CookCorner {
                idx,
                pos: verts[idx as usize],
                uv: (verts[idx as usize][0] as f32, verts[idx as usize][1] as f32),
                shade: (128, 128, 128),
            })
            .collect::<Vec<_>>();
        let mut cache = HashMap::new();
        let mut tri_idx = Vec::new();
        let mut tri_tex = Vec::new();
        let mut tri_uv = Vec::new();
        let mut tri_rgb = Vec::new();
        let result = emit_grid_cell(
            &source,
            3,
            (0.0, 256.0),
            (0.0, 256.0),
            &mut verts,
            &mut cache,
            &mut tri_idx,
            &mut tri_tex,
            &mut tri_uv,
            &mut tri_rgb,
        );
        assert_eq!(result, Some((3, 1)));
        assert_eq!(verts.len(), 5, "the cell must not invent a centre vertex");
        assert!(tri_idx.iter().all(|&idx| idx < 5));
    }

    #[test]
    fn compact_face_loop_requires_exact_packed_fan_continuity() {
        let tri_idx = [0, 2, 1, 0, 3, 2];
        let tri_tex = [7, 7];
        let tri_uv = [0, 0, 20, 0, 10, 0, 0, 0, 30, 0, 20, 0];
        let light = [4, 5, 6, 4, 7, 5];
        assert!(face_is_exact_packed_fan(
            0,
            tri_tex.len(),
            &tri_idx,
            &tri_tex,
            &tri_uv,
            &light,
        ));

        let mut wrong_diagonal = tri_idx;
        wrong_diagonal[5] = 1;
        assert!(!face_is_exact_packed_fan(
            0,
            tri_tex.len(),
            &wrong_diagonal,
            &tri_tex,
            &tri_uv,
            &light,
        ));

        let mut uv_seam = tri_uv;
        uv_seam[10] = 19;
        assert!(!face_is_exact_packed_fan(
            0,
            tri_tex.len(),
            &tri_idx,
            &tri_tex,
            &uv_seam,
            &light,
        ));

        // UV support topology may still satisfy the shared-corner walk while
        // revisiting the fan anchor internally. It is not a polygon loop.
        let repeated_anchor = [0, 2, 1, 0, 3, 2, 0, 0, 3, 0, 0, 0];
        let repeated_tex = [7; 4];
        let repeated_uv = [0; 24];
        let repeated_light = [0; 12];
        assert!(!face_is_exact_packed_fan(
            0,
            repeated_tex.len(),
            &repeated_anchor,
            &repeated_tex,
            &repeated_uv,
            &repeated_light,
        ));
    }

    #[test]
    fn map_audio_ids_normalize_goldsrc_stream_markers_and_modes() {
        let mut manifest = std::collections::HashMap::new();
        manifest.insert((7, "sfx:shot:scientist/c1a0_test.wav".to_string()), 12);
        manifest.insert((7, "sfx:loop:plats/ttrain3.wav".to_string()), 19);
        assert_eq!(
            map_audio_id(&manifest, 7, "*/Scientist/C1A0_Test.WAV", false),
            12
        );
        assert_eq!(map_audio_id(&manifest, 7, "/PLATS/TTRAIN3.WAV", true), 19);
        assert_eq!(
            map_audio_id(&manifest, 7, "/PLATS/TTRAIN3.WAV", false),
            u8::MAX
        );
    }

    #[test]
    fn dialogue_manifest_keys_cover_sentence_and_hologram_sources() {
        assert!(is_voice_message("!SC_DIS16A"));
        assert!(is_voice_message("holo/tr_holo_nicejob.wav"));
        assert_eq!(voice_manifest_key("!sc_dis16a"), "SC_DIS16A");
        assert_eq!(
            voice_manifest_key("*/Scientist/C1A0_Sci_Stall.WAV"),
            "scientist/c1a0_sci_stall.wav"
        );
    }

    #[test]
    fn authored_audio_parameters_match_goldsrc_entity_rules() {
        assert_eq!(scripted_sentence_volume(r#""volume" "4""#), 40);
        assert_eq!(scripted_sentence_volume(""), 100);
        assert_eq!(ambient_volume(r#""health" "9""#), 90);
        assert_eq!(acoustic_volume("speaker", r#""health" "7""#), 70);
        assert_eq!(acoustic_volume("speaker", ""), 0);
        assert_eq!(
            ambient_volume(r#""health" "1" "preset" "7""#),
            50,
            "preset volrun overrides ambient health"
        );
        assert_eq!(acoustic_attenuation("scripted_sentence", "", 0, 1.0), 0);
        assert_eq!(
            acoustic_attenuation("scripted_sentence", r#""attenuation" "3""#, 0, 1.0),
            3
        );
        assert_eq!(acoustic_attenuation("ambient_generic", "", 4, 1.0), 1);
        assert_eq!(
            acoustic_attenuation("ambient_generic", "", 2, 2.0),
            8,
            "small-radius mode plus one source-coordinate shift"
        );
    }

    #[test]
    fn emissive_named_lightstyles_cook_as_toggleable_face_slots() {
        let mut faces = vec![0u8; SZ_FACE];
        faces[10..12].copy_from_slice(&0u16.to_le_bytes());
        faces[12..16].copy_from_slice(&[36, u8::MAX, u8::MAX, u8::MAX]);
        let mut texinfo = vec![0u8; SZ_TEXINFO];
        texinfo[32..36].copy_from_slice(&0i32.to_le_bytes());
        let ents = br#"
        {
          "classname" "light"
          "targetname" "jump1l"
          "style" "36"
          "spawnflags" "1"
          "origin" "444 1906 90"
        }
        {
          "classname" "trigger_once"
          "target" "jump1l"
        }
        "#;
        let lights =
            collect_dynamic_lightstyles(ents, &faces, &texinfo, &["+a~generic65".to_string()])
                .expect("dynamic styles");
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].style, 36);
        assert_eq!(lights[0].targetname, "jump1l");

        let logic = collect_logic_entities_with_lightstyles(
            ents,
            &[],
            &[],
            1.0,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            &lights,
        )
        .expect("light logic");
        assert_eq!(logic.ents.len(), 2);
        let light = logic
            .ents
            .iter()
            .find(|rec| rec.kind == LOGIC_LIGHTSTYLE)
            .expect("toggleable light");
        let trigger = logic
            .ents
            .iter()
            .find(|rec| rec.kind == LOGIC_TRIGGER_ONCE)
            .expect("panel trigger");
        assert_eq!(light.arg0, 1);
        assert_eq!(light.spawnflags & 1, 1);
        assert_ne!(light.targetname, 0);
        assert_eq!(trigger.target, light.targetname);
    }

    #[test]
    fn overlapping_panel_lightmaps_preserve_every_authored_style_plane() {
        let lights = vec![
            DynamicLightStyle {
                style: 36,
                targetname: "jump1l".to_string(),
                origins_hl: vec![[0.0, 0.0, 0.0]],
            },
            DynamicLightStyle {
                style: 37,
                targetname: "jump2l".to_string(),
                origins_hl: vec![[100.0, 0.0, 0.0]],
            },
        ];
        assert_eq!(
            dynamic_lightstyle_slots_for_face(&[0, 36, 37, u8::MAX], &lights),
            ([1, 2, 0], [1, 2, 0])
        );
    }

    #[test]
    fn monster_loot_matches_retail_carried_weapons() {
        assert_eq!(monster_loot_types("monster_barney", ""), [27, 0]);
        assert_eq!(
            monster_loot_types("monster_human_grunt", ""),
            [29, 0],
            "the SDK default grunt carries an MP5 and hand grenades"
        );
        assert_eq!(
            monster_loot_types("monster_human_grunt", r#""weapons" "8""#),
            [30, 0]
        );
        assert_eq!(
            monster_loot_types("monster_human_grunt", r#""weapons" "5""#),
            [29, 47]
        );
        assert_eq!(monster_loot_types("monster_scientist", ""), [0, 0]);
    }
}
