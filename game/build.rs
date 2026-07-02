//! Inject PSoXide's PSX linker script into the final link, by absolute
//! path derived from this crate's location. This keeps the crate buildable
//! from anywhere (no brittle relative `-T` paths in RUSTFLAGS) while the
//! script itself lives in the sibling PSoXide checkout.

use std::{fs, path::PathBuf};

const FALLBACK_MAP_WORDS: usize = 255_000;
const FALLBACK_MAX_FACES: usize = 6144;
const FALLBACK_MAX_FACE_GROUPS: usize = 3072;
const FALLBACK_MAX_LEAVES: usize = 8192;
const FALLBACK_MAX_ENTS: usize = 192;
const FALLBACK_MAX_TEX_SLOTS: usize = 256;
const FALLBACK_MODEL_WORDS: usize = 24_576;
// MODEL_BUF must hold a whole per-map model SET (viewmodel + every NPC/enemy
// type the map places), not just the largest single chunk. Floor it at 512 KB;
// the per-map streamer drops the farthest types if a heavy map's set overflows.
// Raised +8192 words (+32 KB) to hand the enemy pool the budget the FaceRec
// 20B->16B shrink freed from MAP_BUF (net .bss-neutral vs before that change):
// enemy geometry pool = MODEL_WORDS - VM_POOL_WORDS, so this is ~185 -> ~217 KB,
// fewer dropped enemy types on the heaviest maps.
const MODEL_POOL_WORDS: usize = 99_584; // enemy region (kept whole: the roster audit cliffs below this)

fn rd_u32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *d.get(o)?,
        *d.get(o + 1)?,
        *d.get(o + 2)?,
        *d.get(o + 3)?,
    ]))
}

fn rd_u16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

fn round_up(value: usize, step: usize) -> usize {
    if value == 0 {
        0
    } else {
        value.div_ceil(step) * step
    }
}

fn scan_room_budget(repo_root: &std::path::Path) -> (usize, usize, usize, usize, usize, usize) {
    let rooms = repo_root.join("data/rooms");
    println!("cargo:rerun-if-changed={}", rooms.display());

    let Ok(entries) = fs::read_dir(&rooms) else {
        return (
            FALLBACK_MAP_WORDS,
            FALLBACK_MAX_FACES,
            FALLBACK_MAX_FACE_GROUPS,
            FALLBACK_MAX_LEAVES,
            FALLBACK_MAX_ENTS,
            FALLBACK_MAX_TEX_SLOTS,
        );
    };

    let mut max_bytes = 0usize;
    let mut max_face_records = 0usize;
    let mut max_face_groups = 0usize;
    let mut max_leaves = 0usize;
    let mut max_ents = 0usize;
    let mut max_texs = 0usize;

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("room_") || !name.ends_with(".psxc") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        if data.len() >= 8 && &data[0..4] == b"HLTX" {
            max_bytes = max_bytes.max(data.len());
            max_texs = max_texs.max(rd_u32(&data, 4).unwrap_or(0) as usize);
            continue;
        }
        if data.len() < 52 || &data[0..4] != b"HLMA" {
            continue;
        }

        max_bytes = max_bytes.max(data.len());
        let n_texs = rd_u32(&data, 12).unwrap_or(0) as usize;
        let n_faces = rd_u32(&data, 16).unwrap_or(0) as usize;
        let bsp_off = rd_u32(&data, 20).unwrap_or(0) as usize;
        let ent_off = rd_u32(&data, 28).unwrap_or(0) as usize;
        max_texs = max_texs.max(n_texs);

        if bsp_off + 24 <= data.len() {
            let n_face_groups = rd_u32(&data, bsp_off + 4).unwrap_or(0) as usize;
            let n_leaves = rd_u32(&data, bsp_off + 12).unwrap_or(0) as usize;
            max_leaves = max_leaves.max(n_leaves);
            let n_planes = rd_u32(&data, bsp_off).unwrap_or(0) as usize;
            let faces_off = bsp_off + 24 + n_planes * 10 + n_face_groups * 2;
            let mut max_group = 0usize;
            for face in 0..n_faces {
                let o = faces_off + face * 16 + 4; // FaceRec is 16 bytes
                max_group = max_group.max(rd_u16(&data, o).unwrap_or(0) as usize);
            }
            max_face_records = max_face_records.max(n_faces);
            max_face_groups = max_face_groups.max(n_face_groups.max(max_group.saturating_add(1)));
        }

        if ent_off + 4 <= data.len() {
            let n_models = rd_u32(&data, ent_off).unwrap_or(0) as usize;
            let n_ents_off = ent_off + 4 + n_models * 8;
            if n_ents_off + 4 <= data.len() {
                max_ents = max_ents.max(rd_u32(&data, n_ents_off).unwrap_or(0) as usize);
            }
        }
    }

    if max_bytes == 0 {
        return (
            FALLBACK_MAP_WORDS,
            FALLBACK_MAX_FACES,
            FALLBACK_MAX_FACE_GROUPS,
            FALLBACK_MAX_LEAVES,
            FALLBACK_MAX_ENTS,
            FALLBACK_MAX_TEX_SLOTS,
        );
    }

    (
        // +4 KB: LZ4 in-place slack. Compressed chunks are staged at the
        // buffer TAIL and decoded back to the head; the margin keeps the
        // write cursor behind the unread source even on the biggest map.
        (max_bytes + 4096).div_ceil(4),
        round_up(max_face_records + 32, 256),
        round_up(max_face_groups + 32, 256),
        round_up(max_leaves + 64, 256),
        round_up(max_ents + 8, 16),
        round_up(max_texs + 8, 16),
    )
}

fn scan_model_budget(repo_root: &std::path::Path) -> usize {
    let modelpack = repo_root.join("data/modelpack");
    println!("cargo:rerun-if-changed={}", modelpack.display());

    let Ok(entries) = fs::read_dir(&modelpack) else {
        return FALLBACK_MODEL_WORDS;
    };

    let mut max_bytes = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("chunk_") || !name.ends_with(".psxm") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());
        if let Ok(meta) = entry.metadata() {
            max_bytes = max_bytes.max(meta.len() as usize);
        }
    }

    if max_bytes == 0 {
        FALLBACK_MODEL_WORDS.max(MODEL_POOL_WORDS)
    } else {
        round_up(max_bytes.div_ceil(4) + 256, 256).max(MODEL_POOL_WORDS)
    }
}

fn main() {
    // This crate lives at <repo>/game, so the repo root is one level up.
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest.parent().expect("crate must live at <repo>/game");
    let psoxide = std::env::var("PSOXIDE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            repo_root
                .parent()
                .expect("repo root must have a parent")
                .join("PSoXide")
        });
    let ld = psoxide.join("sdk/psoxide.ld");
    let ld = ld.canonicalize().unwrap_or(ld);

    // `-T` selects the linker script; `--oformat=binary` dumps a flat PSX-EXE
    // image (the script lays out the executable header) instead of an ELF.
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    println!("cargo:rustc-link-arg=--oformat=binary");
    println!("cargo:rerun-if-changed={}", ld.display());

    let (map_words, max_faces, max_face_groups, max_leaves, max_ents, max_tex_slots) =
        scan_room_budget(repo_root);
    let model_words = scan_model_budget(repo_root);
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let budget = format!(
        "pub const MAP_WORDS: usize = {map_words};\n\
         pub const MODEL_WORDS: usize = {model_words};\n\
         pub const MAX_FACES: usize = {max_faces};\n\
         pub const MAX_FACE_GROUPS: usize = {max_face_groups};\n\
         pub const MAX_LEAVES: usize = {max_leaves};\n\
         pub const MAX_ENTS: usize = {max_ents};\n\
         pub const MAX_TEX_SLOTS: usize = {max_tex_slots};\n"
    );
    fs::write(out_dir.join("room_budget.rs"), budget).expect("write generated room budget");
}
