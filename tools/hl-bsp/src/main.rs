//! hl-bsp -- a host-side inspector for GoldSrc (Half-Life) BSP v30 maps.
//!
//! Parses the lump table and reports the geometry + texture budget that
//! decides PS1 feasibility (Milestone 1): vertex/face counts, texture count
//! and total texels, embedded-vs-WAD split, lightmap/vis sizes, and the map
//! bounding box. Seed of the eventual map extractor; for now it only reads and
//! reports (no cooking).
//!
//! Usage:  hl-bsp <path/to/map.bsp>
//!     make bsp-info MAP=c1a0   # runs it on $(HL_GAME)/maps/c1a0.bsp

use std::cmp::Reverse;
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
fn f32le(b: &[u8], o: usize) -> Option<f32> {
    u32le(b, o).map(f32::from_bits)
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

// ---- Cook: BSP -> .hlm v3 (PS1-native textured + lit triangle mesh) -------
//
// Layout (all little-endian):
//   magic "HLM3" | u32 n_verts | u32 n_tris | u32 n_texs
//   verts:   i16 x,y,z   × n_verts          (world space, Y-up)
//   tri_idx: u16 a,b,c   × n_tris           (indices into verts)
//   tri_tex: u16         × n_tris           (texture id == miptex index)
//   tri_uv:  u8 u0,v0,u1,v1,u2,v2 × n_tris  (per-corner, texture-local texels)
//   tri_rgb: u8 (r,g,b)×3 × n_tris          (per-corner lightmap shade tint)
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
fn cook_miptex(l: &[u8], mo: usize) -> (CookedTex, (u32, u32)) {
    let (w0, h0) = match (u32le(l, mo + 16), u32le(l, mo + 20)) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as usize, h as usize),
        _ => return (placeholder_tex(), (64, 64)),
    };
    let off0 = u32le(l, mo + 24).unwrap_or(0) as usize;
    let pal = match read_palette(l, mo, w0, h0) {
        Some(p) if off0 != 0 => p,
        _ => return (placeholder_tex(), (w0 as u32, h0 as u32)),
    };
    let fw = final_size(w0 as u32) as usize;
    let fh = final_size(h0 as u32) as usize;
    let px = mo + off0;
    // "{..." textures are masked: source palette index 255 is transparent.
    let masked = l.get(mo) == Some(&b'{');
    // Nearest-neighbour downscale, keeping the source palette index per texel.
    let mut idxv: Vec<u8> = Vec::with_capacity(fw * fh);
    for y in 0..fh {
        for x in 0..fw {
            idxv.push(*l.get(px + (y * h0 / fh) * w0 + (x * w0 / fw)).unwrap_or(&0));
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
        for (i, c) in pal16.iter().enumerate() {
            clut[i] = to_bgr555(c.0, c.1, c.2);
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
    if colors.is_empty() {
        return vec![(110, 110, 110)];
    }
    let mut boxes: Vec<Vec<(u8, u8, u8)>> = vec![colors.to_vec()];
    while boxes.len() < 16 {
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
        (Some(&r), Some(&g), Some(&b)) => {
            let boost = |v: u8| (v as u32 * 3 / 2).min(255) as u8;
            (boost(r), boost(g), boost(b))
        }
        _ => (NEUTRAL, NEUTRAL, NEUTRAL),
    }
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

/// Find the single-player spawn (`info_player_start`) origin + yaw (HL coords,
/// degrees) from the entity lump.
fn find_spawn(ents: &[u8]) -> Option<([f32; 3], f32)> {
    let s = std::str::from_utf8(ents).ok()?;
    for block in s.split('{') {
        if block.contains("\"info_player_start\"") {
            let origin = parse_vec3(ent_value(block, "origin")?)?;
            let yaw = ent_value(block, "angles")
                .and_then(parse_vec3)
                .map(|a| a[1])
                .unwrap_or(0.0);
            return Some((origin, yaw));
        }
    }
    None
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
    kind: u16, // 0 = static brush, 1 = func_door
    origin: [i32; 3],
    mv: [i32; 3],     // door full-open displacement (world)
    center: [i32; 3], // door trigger centre (world)
    r2: i32,          // trigger radius^2 (world)
    head: i32,        // submodel hull-1 clipnode root (collision)
}

/// func_door move direction (HL) + distance: slides `size_along_axis - lip`.
fn door_move(angle: f32, mins: [f32; 3], maxs: [f32; 3], lip: f32) -> ([f32; 3], f32) {
    let sz = [maxs[0] - mins[0], maxs[1] - mins[1], maxs[2] - mins[2]];
    if angle == -1.0 {
        ([0.0, 0.0, 1.0], sz[2] - lip) // up
    } else if angle == -2.0 {
        ([0.0, 0.0, -1.0], sz[2] - lip) // down
    } else {
        let r = angle.to_radians();
        let (c, s) = (r.cos(), r.sin());
        ([c, s, 0.0], sz[0] * c.abs() + sz[1] * s.abs() - lip)
    }
}

/// Collect renderable brush entities (skipping invisible triggers/ladders).
fn collect_entities(ents: &[u8], models: &[u8], scale: f32) -> Vec<EntRec> {
    let s = match std::str::from_utf8(ents) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let n_models = models.len() / SZ_MODEL;
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
        if cls.starts_with("trigger") || cls == "func_ladder" || cls == "func_tracktrain" {
            continue; // invisible, or handled by the tram section
        }
        let origin = to_world(
            ent_value(block, "origin")
                .and_then(parse_vec3)
                .unwrap_or([0.0; 3]),
            scale,
        );
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
        let rad = (sz[0].max(sz[1]).max(sz[2]) * 0.5 + 80.0) / scale;
        let r2 = (rad * rad) as i32;
        let head = i32le(models, mo + 40).unwrap_or(0); // dmodel_t.headnode[1]
        if cls == "func_door" {
            let angle = ent_value(block, "angle")
                .and_then(|a| a.parse().ok())
                .unwrap_or(0.0);
            let lip = ent_value(block, "lip")
                .and_then(|a| a.parse().ok())
                .unwrap_or(8.0);
            let (dir, dist) = door_move(angle, mins, maxs, lip);
            let mv = to_world([dir[0] * dist, dir[1] * dist, dir[2] * dist], scale);
            out.push(EntRec {
                submodel: submodel as u16,
                kind: 1,
                origin,
                mv,
                center,
                r2,
                head,
            });
        } else {
            out.push(EntRec {
                submodel: submodel as u16,
                kind: 0,
                origin,
                mv: [0; 3],
                center,
                r2: 0,
                head,
            });
        }
    }
    out
}

/// The `func_tracktrain` (tram) submodel, speed, and its `path_track` waypoint
/// chain (world coords). Returns `(0, 0, [])` if the map has no tram.
fn collect_tram(ents: &[u8], scale: f32) -> (u16, i32, Vec<[i32; 3]>, [i32; 3]) {
    let s = match std::str::from_utf8(ents) {
        Ok(s) => s,
        Err(_) => return (0, 0, Vec::new(), [0; 3]),
    };
    let mut tracks: Vec<(String, [f32; 3], String)> = Vec::new();
    let (mut model, mut speed, mut first) = (0u16, 0i32, String::new());
    let mut origin = [0i32; 3]; // tram's editor origin (its reference point), world
    for block in s.split('{') {
        match ent_value(block, "classname") {
            Some("path_track") => tracks.push((
                ent_value(block, "targetname").unwrap_or("").to_string(),
                ent_value(block, "origin")
                    .and_then(parse_vec3)
                    .unwrap_or([0.0; 3]),
                ent_value(block, "target").unwrap_or("").to_string(),
            )),
            Some("func_tracktrain") => {
                // Last tracktrain wins (c0a0: the player "train"). ponytail.
                if let Some(m) = ent_value(block, "model") {
                    if let Some(n) = m.strip_prefix('*') {
                        model = n.parse().unwrap_or(0);
                    }
                }
                speed = ent_value(block, "speed")
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(100.0) as i32;
                first = ent_value(block, "target").unwrap_or("").to_string();
                origin = to_world(
                    ent_value(block, "origin")
                        .and_then(parse_vec3)
                        .unwrap_or([0.0; 3]),
                    scale,
                );
            }
            _ => {}
        }
    }
    if model == 0 || first.is_empty() {
        return (0, 0, Vec::new(), [0; 3]);
    }
    let mut way = Vec::new();
    let mut name = first;
    while !name.is_empty() && way.len() < 256 {
        match tracks.iter().find(|t| t.0 == name) {
            Some(t) => {
                way.push(to_world(t.1, scale));
                name = t.2.clone();
            }
            None => break,
        }
    }
    (model, speed, way, origin)
}

/// Point entities that place a studio model: `(model_type, origin_world, yaw)`.
/// type 0 = scientist, 1 = barney.
fn collect_props(ents: &[u8], scale: f32) -> Vec<(u16, [i32; 3], i32)> {
    let s = match std::str::from_utf8(ents) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for block in s.split('{') {
        let ty = match ent_value(block, "classname").unwrap_or("") {
            "monster_scientist" | "monster_sitting_scientist" => 0u16,
            "monster_barney" => 1u16,
            _ => continue,
        };
        let origin = to_world(
            ent_value(block, "origin")
                .and_then(parse_vec3)
                .unwrap_or([0.0; 3]),
            scale,
        );
        let deg = ent_value(block, "angles")
            .and_then(parse_vec3)
            .map(|a| a[1])
            .unwrap_or(0.0);
        let yaw = (((deg - 90.0) / 360.0 * 4096.0).round() as i32) & 0xFFF;
        out.push((ty, origin, yaw));
    }
    out
}

fn cook(path: &str, out: &str) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {}", path, e))?;
    let bsp = Bsp::parse(&bytes)?;

    // Raw vertices (f32, HL Z-up). Power-of-two shift so coords fit i16.
    let vl = bsp.lump(LUMP_VERTEXES);
    let n_verts = vl.len() / SZ_VERTEX;
    let raw: Vec<[f32; 3]> = (0..n_verts)
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
    // HL right-handed Z-up -> world Y-up (world = [x, z, y]); winding reversed.
    let verts: Vec<[i16; 3]> = raw
        .iter()
        .map(|v| {
            [
                (v[0] / scale).round() as i16,
                (v[2] / scale).round() as i16,
                (v[1] / scale).round() as i16,
            ]
        })
        .collect();

    // Cook every miptex (tex_id == miptex index). Keep original sizes for UVs.
    let tl = bsp.lump(LUMP_TEXTURES);
    let n_texs = i32le(tl, 0).filter(|&n| n >= 0).unwrap_or(0) as usize;
    let mut texs: Vec<CookedTex> = Vec::with_capacity(n_texs);
    let mut orig: Vec<(u32, u32)> = Vec::with_capacity(n_texs);
    for i in 0..n_texs {
        match i32le(tl, 4 + i * 4) {
            Some(d) if d >= 0 => {
                let (t, o) = cook_miptex(tl, d as usize);
                texs.push(t);
                orig.push(o);
            }
            _ => {
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
    let n_faces = faces.len() / SZ_FACE;

    let mut tri_idx: Vec<u16> = Vec::new();
    let mut tri_tex: Vec<u16> = Vec::new();
    let mut tri_uv: Vec<u8> = Vec::new();
    let mut tri_rgb: Vec<u8> = Vec::new();
    // Per-face triangle range (for PVS: leaf -> face -> tris). Skipped faces
    // keep count 0.
    let mut face_first = vec![0u32; n_faces];
    let mut face_ntri = vec![0u16; n_faces];

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
            let e = se.unsigned_abs() as usize * SZ_EDGE;
            let v = if se >= 0 {
                u16le(edges, e)
            } else {
                u16le(edges, e + 2)
            };
            if let Some(v) = v {
                if (v as usize) < n_verts {
                    poly.push(v);
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
        let mut uv: Vec<(f32, f32)> = Vec::with_capacity(poly.len());
        let mut ouv: Vec<(f32, f32)> = Vec::with_capacity(poly.len());
        let (mut minu, mut minv) = (f32::MAX, f32::MAX);
        let (mut lu0, mut lu1, mut lv0, mut lv1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &vi in &poly {
            let p = raw[vi as usize];
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
        let shade: Vec<(u8, u8, u8)> = ouv
            .iter()
            .map(|&(ou, ov)| {
                vertex_shade(lighting, lightofs, style0, lmw, lmh, ou, ov, mins_s, mins_t)
            })
            .collect();
        // Shift by whole texture tiles so values start near 0 (preserves tiling
        // phase), then saturate to u8. Faces tiling more than ~4x clamp at the
        // far edge -- proper tiling of huge surfaces needs UV subdivision (M3).
        let shu = (minu / fw).floor() * fw;
        let shv = (minv / fh).floor() * fh;
        let uvb: Vec<(u8, u8)> = uv
            .iter()
            .map(|&(u, v)| {
                (
                    (u - shu).round().clamp(0.0, 255.0) as u8,
                    (v - shv).round().clamp(0.0, 255.0) as u8,
                )
            })
            .collect();
        // Fan, reversed winding. Record this face's triangle range for PVS.
        face_first[f] = (tri_idx.len() / 3) as u32;
        face_ntri[f] = (poly.len() - 2) as u16;
        for k in 1..poly.len() - 1 {
            tri_idx.extend_from_slice(&[poly[0], poly[k + 1], poly[k]]);
            tri_tex.push(tex_id as u16);
            let (a, b, c) = (uvb[0], uvb[k + 1], uvb[k]);
            tri_uv.extend_from_slice(&[a.0, a.1, b.0, b.1, c.0, c.1]);
            let (s0, s1, s2) = (shade[0], shade[k + 1], shade[k]);
            tri_rgb.extend_from_slice(&[s0.0, s0.1, s0.2, s1.0, s1.1, s1.2, s2.0, s2.1, s2.2]);
        }
    }

    let n_tris = tri_idx.len() / 3;
    let mut o: Vec<u8> = Vec::new();
    o.extend_from_slice(b"HLM9");
    o.extend_from_slice(&(n_verts as u32).to_le_bytes());
    o.extend_from_slice(&(n_tris as u32).to_le_bytes());
    o.extend_from_slice(&(n_texs as u32).to_le_bytes());
    o.extend_from_slice(&(n_faces as u32).to_le_bytes());
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
    for v in &verts {
        for c in v {
            o.extend_from_slice(&c.to_le_bytes());
        }
    }
    for i in &tri_idx {
        o.extend_from_slice(&i.to_le_bytes());
    }
    for tx in &tri_tex {
        o.extend_from_slice(&tx.to_le_bytes());
    }
    o.extend_from_slice(&tri_uv);
    o.extend_from_slice(&tri_rgb);
    while o.len() % 4 != 0 {
        o.push(0);
    }
    // Texture blob (each block is already a multiple of 4 bytes).
    for tx in &texs {
        o.extend_from_slice(&tx.w.to_le_bytes());
        o.extend_from_slice(&tx.h.to_le_bytes());
        for c in &tx.clut {
            o.extend_from_slice(&c.to_le_bytes());
        }
        o.extend_from_slice(&tx.pix4);
    }

    // ---- BSP visibility (PVS) ----
    // u32 n_nodes,n_leaves,n_marks,vis_len | face_first[u32×n_faces] |
    // face_ntri[u16×n_faces] (pad) | nodes[20B] | leaves[8B] | marks (pad) |
    // vis (raw RLE, pad). Node planes are transformed to world space so the
    // runtime can walk the tree with the world-space camera directly.
    let bsp_off = o.len() as u32;
    o[bsp_off_pos..bsp_off_pos + 4].copy_from_slice(&bsp_off.to_le_bytes());
    let planes = bsp.lump(LUMP_PLANES);
    let nodes = bsp.lump(LUMP_NODES);
    let leaves = bsp.lump(LUMP_LEAVES);
    let marks = bsp.lump(LUMP_MARKSURFACES);
    let vis = bsp.lump(LUMP_VISIBILITY);
    let n_nodes = nodes.len() / SZ_NODE;
    let n_leaves = leaves.len() / SZ_LEAF;
    let n_marks = marks.len() / SZ_MARKSURFACE;

    o.extend_from_slice(&(n_nodes as u32).to_le_bytes());
    o.extend_from_slice(&(n_leaves as u32).to_le_bytes());
    o.extend_from_slice(&(n_marks as u32).to_le_bytes());
    o.extend_from_slice(&(vis.len() as u32).to_le_bytes());

    for v in &face_first {
        o.extend_from_slice(&v.to_le_bytes());
    }
    for v in &face_ntri {
        o.extend_from_slice(&v.to_le_bytes());
    }
    while o.len() % 4 != 0 {
        o.push(0);
    }

    // Per-face world-space plane (side-adjusted): front-facing iff dot(n,eye) > dist.
    // Lets the runtime backface-cull a whole face before any per-triangle work.
    for f in 0..n_faces {
        let fo2 = f * SZ_FACE;
        let planenum = u16le(faces, fo2).unwrap_or(0) as usize;
        let s = if u16le(faces, fo2 + 2).unwrap_or(0) == 0 {
            1.0
        } else {
            -1.0
        };
        let po = planenum * SZ_PLANE;
        let nx = f32le(planes, po).unwrap_or(0.0) * s;
        let ny = f32le(planes, po + 4).unwrap_or(0.0) * s;
        let nz = f32le(planes, po + 8).unwrap_or(0.0) * s;
        let d = f32le(planes, po + 12).unwrap_or(0.0) * s;
        o.extend_from_slice(&((nx * 4096.0).round() as i16).to_le_bytes()); // world: swap Y/Z
        o.extend_from_slice(&((nz * 4096.0).round() as i16).to_le_bytes());
        o.extend_from_slice(&((ny * 4096.0).round() as i16).to_le_bytes());
        o.extend_from_slice(&0i16.to_le_bytes());
        o.extend_from_slice(&((d / scale).round() as i32).to_le_bytes());
    }

    for ni in 0..n_nodes {
        let no = ni * SZ_NODE;
        let planenum = i32le(nodes, no).unwrap_or(0).max(0) as usize;
        let po = planenum * SZ_PLANE;
        let nx = f32le(planes, po).unwrap_or(0.0);
        let ny = f32le(planes, po + 4).unwrap_or(0.0);
        let nz = f32le(planes, po + 8).unwrap_or(0.0);
        let d = f32le(planes, po + 12).unwrap_or(0.0);
        // World space: swap Y/Z of the normal, scale the distance.
        o.extend_from_slice(&((nx * 4096.0).round() as i16).to_le_bytes());
        o.extend_from_slice(&((nz * 4096.0).round() as i16).to_le_bytes());
        o.extend_from_slice(&((ny * 4096.0).round() as i16).to_le_bytes());
        o.extend_from_slice(&0i16.to_le_bytes()); // pad
        o.extend_from_slice(&((d / scale).round() as i32).to_le_bytes());
        o.extend_from_slice(
            &(i16::from_le_bytes([nodes[no + 4], nodes[no + 5]]) as i32).to_le_bytes(),
        );
        o.extend_from_slice(
            &(i16::from_le_bytes([nodes[no + 6], nodes[no + 7]]) as i32).to_le_bytes(),
        );
    }

    for li in 0..n_leaves {
        let lo = li * SZ_LEAF;
        o.extend_from_slice(
            &i32le(leaves, lo + SZ_LEAF_VISOFS)
                .unwrap_or(-1)
                .to_le_bytes(),
        );
        o.extend_from_slice(&u16le(leaves, lo + SZ_LEAF_MARK0).unwrap_or(0).to_le_bytes());
        o.extend_from_slice(
            &u16le(leaves, lo + SZ_LEAF_MARK0 + 2)
                .unwrap_or(0)
                .to_le_bytes(),
        );
        // dleaf_t bbox (i16 mins@8, maxs@14) -> world centre + radius (frustum cull).
        let g = |o2: usize| i16::from_le_bytes([leaves[lo + o2], leaves[lo + o2 + 1]]) as f32;
        let (mn, mx) = ([g(8), g(10), g(12)], [g(14), g(16), g(18)]);
        let c = to_world(
            [
                (mn[0] + mx[0]) * 0.5,
                (mn[1] + mx[1]) * 0.5,
                (mn[2] + mx[2]) * 0.5,
            ],
            scale,
        );
        let d = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
        let rad = ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() * 0.5 / scale) as i32;
        o.extend_from_slice(&(c[0] as i16).to_le_bytes());
        o.extend_from_slice(&(c[1] as i16).to_le_bytes());
        o.extend_from_slice(&(c[2] as i16).to_le_bytes());
        o.extend_from_slice(&(rad.clamp(0, 65535) as u16).to_le_bytes());
    }
    while o.len() % 4 != 0 {
        o.push(0);
    }

    o.extend_from_slice(marks);
    while o.len() % 4 != 0 {
        o.push(0);
    }
    o.extend_from_slice(vis);
    while o.len() % 4 != 0 {
        o.push(0);
    }

    // ---- Clip hull (player collision) + spawn ----
    // u32 n_clip | i32 hull1_head | i32 spawn x,y,z (world) | i32 spawn_yaw (Q0.12)
    // clipnodes (i16 nx,ny,nz, i16 c0, i16 c1, i16 pad, i32 dist) × n_clip [16B]
    let clip_off = o.len() as u32;
    o[clip_off_pos..clip_off_pos + 4].copy_from_slice(&clip_off.to_le_bytes());
    let clipnodes = bsp.lump(LUMP_CLIPNODES);
    let n_clip = clipnodes.len() / SZ_CLIPNODE;
    let models = bsp.lump(LUMP_MODELS);
    let hull1_head = i32le(models, 40).unwrap_or(0); // dmodel_t.headnode[1]
    let (sp, syaw_deg) = find_spawn(bsp.lump(LUMP_ENTITIES)).unwrap_or_else(|| {
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
            0.0,
        )
    });
    // World space: swap Y/Z, scale.
    let spawn = [
        (sp[0] / scale).round() as i32,
        (sp[2] / scale).round() as i32,
        (sp[1] / scale).round() as i32,
    ];
    // HL yaw 0 = +X; our forward at yaw 0 = +Z (world Z = HL Y), so offset -90 deg.
    let syaw = (((syaw_deg - 90.0) / 360.0 * 4096.0).round() as i32) & 0xFFF;

    o.extend_from_slice(&(n_clip as u32).to_le_bytes());
    o.extend_from_slice(&hull1_head.to_le_bytes());
    for c in &spawn {
        o.extend_from_slice(&c.to_le_bytes());
    }
    o.extend_from_slice(&syaw.to_le_bytes());
    for ci in 0..n_clip {
        let co = ci * SZ_CLIPNODE;
        let planenum = i32le(clipnodes, co).unwrap_or(0).max(0) as usize;
        let po = planenum * SZ_PLANE;
        let nx = f32le(planes, po).unwrap_or(0.0);
        let ny = f32le(planes, po + 4).unwrap_or(0.0);
        let nz = f32le(planes, po + 8).unwrap_or(0.0);
        let d = f32le(planes, po + 12).unwrap_or(0.0);
        o.extend_from_slice(&((nx * 4096.0).round() as i16).to_le_bytes());
        o.extend_from_slice(&((nz * 4096.0).round() as i16).to_le_bytes());
        o.extend_from_slice(&((ny * 4096.0).round() as i16).to_le_bytes());
        o.extend_from_slice(
            &i16::from_le_bytes([clipnodes[co + 4], clipnodes[co + 5]]).to_le_bytes(),
        );
        o.extend_from_slice(
            &i16::from_le_bytes([clipnodes[co + 6], clipnodes[co + 7]]).to_le_bytes(),
        );
        o.extend_from_slice(&0i16.to_le_bytes()); // pad
        o.extend_from_slice(&((d / scale).round() as i32).to_le_bytes());
    }

    // ---- Entities (brush models) ----
    // u32 n_models | (u32 firstface, u32 numface) × n_models
    // u32 n_ents   | EntRec[48B] × n_ents
    let ent_off = o.len() as u32;
    o[ent_off_pos..ent_off_pos + 4].copy_from_slice(&ent_off.to_le_bytes());
    let n_models = models.len() / SZ_MODEL;
    o.extend_from_slice(&(n_models as u32).to_le_bytes());
    for mi in 0..n_models {
        let mo = mi * SZ_MODEL;
        o.extend_from_slice(&(i32le(models, mo + 56).unwrap_or(0) as u32).to_le_bytes()); // firstface
        o.extend_from_slice(&(i32le(models, mo + 60).unwrap_or(0) as u32).to_le_bytes());
        // numfaces
    }
    let ents = collect_entities(bsp.lump(LUMP_ENTITIES), models, scale);
    o.extend_from_slice(&(ents.len() as u32).to_le_bytes());
    for e in &ents {
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
    }

    // ---- Tram (func_tracktrain ride) ----
    // u16 submodel | u16 n_way | i32 speed | waypoints i32[3] × n_way (world)
    let tram_off = o.len() as u32;
    o[tram_off_pos..tram_off_pos + 4].copy_from_slice(&tram_off.to_le_bytes());
    let (tram_model, tram_speed, way, _) = collect_tram(bsp.lump(LUMP_ENTITIES), scale);
    let tram_head = if tram_model > 0 {
        i32le(models, tram_model as usize * SZ_MODEL + 40).unwrap_or(0)
    } else {
        0
    };
    // The tram brush verts are stored relative to the entity origin (bbox near
    // 0); HL renders them at verts + pev->origin, which the path drives. So the
    // render/collision offset is the full path position = wp0 + ride_off.
    let tram_base = if !way.is_empty() { way[0] } else { [0, 0, 0] };
    o.extend_from_slice(&tram_model.to_le_bytes());
    o.extend_from_slice(&(way.len() as u16).to_le_bytes());
    o.extend_from_slice(&tram_speed.to_le_bytes());
    o.extend_from_slice(&tram_head.to_le_bytes());
    for c in &tram_base {
        o.extend_from_slice(&c.to_le_bytes());
    }
    for w in &way {
        for c in w {
            o.extend_from_slice(&c.to_le_bytes());
        }
    }

    // ---- Props (point-entity model placements) ----
    // u32 n_props | (u16 type, u16 pad, i32 origin[3], i32 yaw) × n_props
    let prop_off = o.len() as u32;
    o[prop_off_pos..prop_off_pos + 4].copy_from_slice(&prop_off.to_le_bytes());
    let props = collect_props(bsp.lump(LUMP_ENTITIES), scale);
    o.extend_from_slice(&(props.len() as u32).to_le_bytes());
    for (ty, org, yaw) in &props {
        o.extend_from_slice(&ty.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        for c in org {
            o.extend_from_slice(&c.to_le_bytes());
        }
        o.extend_from_slice(&yaw.to_le_bytes());
    }

    std::fs::write(out, &o).map_err(|e| format!("write {}: {}", out, e))?;
    println!(
        "cooked {} -> {}  ({} verts, {} tris, {} faces, {} leaves, {} clipnodes, {} ents, tram {} waypts, {} props, spawn [{},{},{}], {} KB)",
        path, out, n_verts, n_tris, n_faces, n_leaves, n_clip, ents.len(), way.len(), props.len(), spawn[0], spawn[1], spawn[2], o.len() / 1024
    );
    Ok(())
}

// ---- MDL (Half-Life studio model) -> .hlmdl ------------------------------
//
// Bakes the model's reference pose into a static posed textured mesh (bone
// matrices applied at cook time). Default bodyparts are concatenated so view
// models keep separate visible pieces such as magazines/clips. Layout matches
// .hlm geometry+textures:
//   magic "HMD2" | u32 n_verts,n_tris,n_texs,n_frames
//   verts i16×3 | tri_idx u16×3 | tri_tex u16 | tri_uv u8×6 (pad4) | textures...

type Mat34 = ([[f32; 3]; 3], [f32; 3]); // rotation, translation

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

fn cook_mdl(path: &str, out: &str, seq: i32) -> Result<(), String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
    if b.get(0..4) != Some(b"IDST") {
        return Err(format!("{}: not a studio MDL", path));
    }
    let i = |o: usize| i32le(&b, o).unwrap_or(0);
    let f = |o: usize| f32le(&b, o).unwrap_or(0.0);
    let h16 = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);

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
    let (numbodyparts, bodypartindex) = (i(204) as usize, i(208) as usize);
    struct MdlPart {
        nummesh: usize,
        meshindex: usize,
        vert_base: usize,
    }

    // Raw vertices + their bone (positions are constant; bone matrices animate).
    // Studio MDLs can split a viewmodel across bodyparts; body value 0 selects
    // model 0 in each bodypart, so concatenate those defaults into one mesh.
    let mut parts: Vec<MdlPart> = Vec::new();
    let mut vp: Vec<[f32; 3]> = Vec::new();
    let mut vbone: Vec<usize> = Vec::new();
    for bp in 0..numbodyparts {
        let bpo = bodypartindex + bp * 76;
        let nummodels = i(bpo + 64).max(0) as usize;
        if nummodels == 0 {
            continue;
        }
        let modelindex = i(bpo + 72) as usize; // bodypart model 0 (default body)
        let (nummesh, meshindex) = (i(modelindex + 72).max(0) as usize, i(modelindex + 76) as usize);
        let numverts = i(modelindex + 80).max(0) as usize;
        let (vinfoindex, vertindex) = (i(modelindex + 84) as usize, i(modelindex + 88) as usize);
        let vert_base = vp.len();
        parts.push(MdlPart {
            nummesh,
            meshindex,
            vert_base,
        });
        for v in 0..numverts {
            vp.push([
                f(vertindex + v * 12),
                f(vertindex + v * 12 + 4),
                f(vertindex + v * 12 + 8),
            ]);
            vbone.push(*b.get(vinfoindex + v).unwrap_or(&0) as usize);
        }
    }

    // Pick sequence `seq` (default 0) and bake up to MAX_FRAMES posed-vertex frames.
    // seqdesc (176 B): numframes@56, animindex@124, seqgroup@156. mstudioanim per
    // bone is 12 B (6 u16 channel offsets) at animindex + bone*12; offset 0 = no
    // anim for that DOF (use the bone default). PS1 has no FPU, so we bake frames
    // host-side -- the runtime just swaps vertex sets.
    const MAX_FRAMES: usize = 16;
    let (numseq, seqindex) = (i(164), i(168) as usize);
    let (animindex, numframes) = if seq >= 0 && seq < numseq {
        let sd = seqindex + seq as usize * 176;
        if i(sd + 156) == 0 {
            (i(sd + 124) as usize, i(sd + 56).max(1) as usize)
        } else {
            (0, 1) // sequence lives in a separate group file (not loaded)
        }
    } else {
        (0, 1)
    };
    let nbake = if animindex == 0 {
        1
    } else {
        numframes.min(MAX_FRAMES).max(1)
    };

    let ident: Mat34 = (
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        [0.0; 3],
    );
    let mut frames: Vec<Vec<[i16; 3]>> = Vec::with_capacity(nbake);
    for fi in 0..nbake {
        let sframe = if nbake > 1 { fi * numframes / nbake } else { 0 };
        let mut bones: Vec<Mat34> = Vec::with_capacity(numbones);
        for bi in 0..numbones {
            let bm = &bmeta[bi];
            let mut dof = bm.value;
            if animindex != 0 {
                let at = animindex + bi * 12; // this bone's mstudioanim_t
                for d in 0..6 {
                    let off = u16::from_le_bytes([b[at + d * 2], b[at + d * 2 + 1]]) as usize;
                    if off != 0 {
                        dof[d] =
                            bm.value[d] + anim_value(&b, at + off, sframe) as f32 * bm.scale[d];
                    }
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
        }
        let mut fv: Vec<[i16; 3]> = Vec::with_capacity(vp.len());
        for v in 0..vp.len() {
            let p = apply(bones.get(vbone[v]).unwrap_or(&ident), vp[v]);
            fv.push([
                p[0].round() as i16,
                p[2].round() as i16,
                p[1].round() as i16,
            ]);
        }
        frames.push(fv);
    }

    let mut tri_idx: Vec<u16> = Vec::new();
    let mut tri_tex: Vec<u16> = Vec::new();
    let mut tri_uv: Vec<u8> = Vec::new();
    let mut texs: Vec<CookedTex> = Vec::new();
    let mut slot_of: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();

    for part in &parts {
        for m in 0..part.nummesh {
            let me = part.meshindex + m * 20;
            let triindex = i(me + 4) as usize;
            let skinref = i(me + 8) as usize;
            let texid = th16(skinindex + skinref * 2) as usize; // skin family 0 (texture file)
            let to = textureindex + texid * 80;
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
                    let vi = (part.vert_base + local_vi).min(u16::MAX as usize) as u16;
                    let (ss, tt) = (h16(o + 4) as f32, h16(o + 6) as f32);
                    o += 8;
                    let u = (ss * fw / tw.max(1) as f32).clamp(0.0, 255.0) as u8;
                    let vv = (tt * fh / th.max(1) as f32).clamp(0.0, 255.0) as u8;
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
                    // Reversed winding (c,b,a) to match the BSP cook's Y/Z-swap convention,
                    // so the runtime's backface cull keeps front faces.
                    let (va, vb, vc) = (s[a], s[bb], s[c]);
                    tri_idx.extend_from_slice(&[vc.0, vb.0, va.0]);
                    tri_tex.push(slot as u16);
                    tri_uv.extend_from_slice(&[vc.1, vc.2, vb.1, vb.2, va.1, va.2]);
                }
            }
        }
    }

    let n_tris = tri_idx.len() / 3;
    let n_verts = vp.len();
    let mut o: Vec<u8> = Vec::new();
    o.extend_from_slice(b"HMD2"); // multi-frame: header + n_frames x vert-set, then shared tris/tex
    o.extend_from_slice(&(n_verts as u32).to_le_bytes());
    o.extend_from_slice(&(n_tris as u32).to_le_bytes());
    o.extend_from_slice(&(texs.len() as u32).to_le_bytes());
    o.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    for fv in &frames {
        for v in fv {
            for c in v {
                o.extend_from_slice(&c.to_le_bytes());
            }
        }
    }
    for x in &tri_idx {
        o.extend_from_slice(&x.to_le_bytes());
    }
    for x in &tri_tex {
        o.extend_from_slice(&x.to_le_bytes());
    }
    o.extend_from_slice(&tri_uv);
    while o.len() % 4 != 0 {
        o.push(0);
    }
    for tx in &texs {
        o.extend_from_slice(&tx.w.to_le_bytes());
        o.extend_from_slice(&tx.h.to_le_bytes());
        for c in &tx.clut {
            o.extend_from_slice(&c.to_le_bytes());
        }
        o.extend_from_slice(&tx.pix4);
    }
    std::fs::write(out, &o).map_err(|e| format!("write {}: {}", out, e))?;
    println!(
        "cooked {} -> {} (seq {}, {} frames, {} verts, {} tris, {} texs, {} KB)",
        path,
        out,
        seq,
        frames.len(),
        n_verts,
        n_tris,
        texs.len(),
        o.len() / 1024
    );
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--mdl") {
        match (args.get(2), args.get(3)) {
            (Some(inp), Some(out)) => {
                let seq = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
                if let Err(e) = cook_mdl(inp, out, seq) {
                    eprintln!("{}", e);
                    exit(1);
                }
                return;
            }
            _ => {
                eprintln!("usage: hl-bsp --mdl <in.mdl> <out.hlmdl> [seq]");
                exit(2);
            }
        }
    }
    // `--cook <in.bsp> <out.hlm>` cooks; otherwise `<map.bsp>` reports.
    if args.get(1).map(|s| s.as_str()) == Some("--cook") {
        match (args.get(2), args.get(3)) {
            (Some(inp), Some(out)) => {
                if let Err(e) = cook(inp, out) {
                    eprintln!("{}", e);
                    exit(1);
                }
                return;
            }
            _ => {
                eprintln!("usage: hl-bsp --cook <in.bsp> <out.hlm>");
                exit(2);
            }
        }
    }
    let path = match args.get(1) {
        Some(p) => p.clone(),
        None => {
            eprintln!("usage: hl-bsp <map.bsp>  |  hl-bsp --cook <in.bsp> <out.hlm>");
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
}
