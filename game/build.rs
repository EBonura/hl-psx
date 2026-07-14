//! Inject PSoXide's PSX linker script into the final link, by absolute
//! path derived from this crate's location. This keeps the crate buildable
//! from anywhere (no brittle relative `-T` paths in RUSTFLAGS) while the
//! script itself lives in the sibling PSoXide checkout.

use std::{collections::HashSet, fs, path::PathBuf};

const FALLBACK_MAP_WORDS: usize = 255_000;
const FALLBACK_MAX_VERTS: usize = 12_288;
const FALLBACK_MAX_FACES: usize = 6144;
const FALLBACK_MAX_FACE_GROUPS: usize = 3072;
const FALLBACK_MAX_LEAVES: usize = 8192;
const FALLBACK_MAX_ENTS: usize = 192;
const FALLBACK_MAX_TEX_SLOTS: usize = 256;
const FALLBACK_MODEL_WORDS: usize = 24_576;
const FALLBACK_PACK_CACHE_ENTRIES: usize = 512;
const MODEL_INDEX_VERTEX_LIMIT: usize = 1024;
// MODEL_BUF must hold a whole per-map model set (the fixed viewmodel reserve,
// every resident NPC/enemy frame section, and one whole incoming HMRG chunk),
// not just the largest individual asset. The per-map streamer drops whole
// types if this arena, the face pools, or the texture slots overflow.
// Audited against the worst per-map streaming peak (tools/roster_audit.py):
// HMD5 actors remove runtime-unused face normals. The current c4a3 roster peaks
// at 90,008 words transient (VM reserve + resident frame sections + the whole
// in-flight HMRG chunk); 90,624 leaves 616 words / 2,464 bytes of drift.
// Re-run the audit after `make models`/`make rooms` before trimming further.
const MODEL_POOL_WORDS: usize = 90_624;

/// Parse one optional three-component diagnostic vector from the build
/// environment.  These values are intentionally build-time only: semantic
/// replay discs can start from the same declared GoldSrc checkpoint without
/// adding a parser, strings, or state to the shipping executable.
fn diagnostic_vec3(name: &str) -> Option<[f64; 3]> {
    println!("cargo:rerun-if-env-changed={name}");
    let raw = std::env::var(name).ok()?;
    let values = raw
        .split_whitespace()
        .map(|value| {
            value
                .parse::<f64>()
                .unwrap_or_else(|_| panic!("{name} contains non-numeric value {value:?}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        values.len(),
        3,
        "{name} must contain exactly three whitespace-separated numbers"
    );
    assert!(
        values
            .iter()
            .all(|value| value.is_finite() && value.abs() <= 1_000_000.0),
        "{name} values must be finite and within +/-1,000,000"
    );
    Some([values[0], values[1], values[2]])
}

fn diagnostic_i32(value: f64, name: &str) -> i32 {
    let rounded = value.round();
    assert!(
        rounded >= i32::MIN as f64 && rounded <= i32::MAX as f64,
        "{name} is outside i32 range"
    );
    rounded as i32
}

/// Emit the optional semantic-replay checkpoint in PSX world/view units.
/// Input is deliberately the same canonical convention as the GoldSrc runner:
/// origin is HL `(x,y,z)` and angles are degrees `(pitch,yaw,roll)`.
fn write_reference_checkpoint(out_dir: &std::path::Path) {
    let origin_hl = diagnostic_vec3("HLPSX_INITIAL_ORIGIN");
    let angles_hl = diagnostic_vec3("HLPSX_INITIAL_ANGLES");

    let origin_psx = origin_hl.map(|value| {
        // Cook/runtime world convention is [HL x, HL z, HL y].
        [
            diagnostic_i32(value[0], "HLPSX_INITIAL_ORIGIN x"),
            diagnostic_i32(value[2], "HLPSX_INITIAL_ORIGIN z"),
            diagnostic_i32(value[1], "HLPSX_INITIAL_ORIGIN y"),
        ]
    });
    let (yaw_psx, pitch_psx) = if let Some(value) = angles_hl {
        assert!(
            value[2].abs() < 1.0e-9,
            "HLPSX_INITIAL_ANGLES roll is unsupported; expected zero"
        );
        // HL yaw 0 points +X and yaw 90 points +Y. After [x,z,y], runtime
        // yaw 0 points +world Z, hence 90-yaw. Semantic pitch has the opposite
        // sign on the two sides (positive input subtracts Gold pitch but adds
        // runtime pitch).
        let yaw = diagnostic_i32((90.0 - value[1]) * 4096.0 / 360.0, "initial yaw").rem_euclid(4096)
            as u16;
        let pitch = diagnostic_i32(-value[0] * 4096.0 / 360.0, "initial pitch");
        assert!(
            pitch >= i16::MIN as i32 && pitch <= i16::MAX as i32,
            "initial pitch is outside i16 range"
        );
        (Some(yaw), Some(pitch as i16))
    } else {
        (None, None)
    };

    if origin_psx.is_some() || angles_hl.is_some() {
        assert!(
            std::env::var_os("CARGO_FEATURE_SEMANTIC_INPUT").is_some(),
            "HLPSX_INITIAL_ORIGIN/ANGLES are diagnostic-only and require --features semantic-input"
        );
    }
    let generated = format!(
        "pub const INITIAL_ORIGIN: Option<[i32; 3]> = {origin_psx:?};\n\
         pub const INITIAL_YAW: Option<u16> = {yaw_psx:?};\n\
         pub const INITIAL_PITCH: Option<i16> = {pitch_psx:?};\n"
    );
    fs::write(out_dir.join("reference_checkpoint.rs"), generated)
        .expect("write generated reference checkpoint");
}

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

/// Minimum byte capacity in which the SDK's tail-staged decoder accepts the
/// exact LZ4 stream mkisopsx will write, plus a small non-format guard. Success
/// is monotonic as the staged source moves farther above the output, so a
/// binary search avoids baking a conservative worst-case overlap allowance
/// into two megabytes of console RAM.
fn packed_map_capacity(raw: &[u8]) -> usize {
    const GUARD_BYTES: usize = 64;
    let comp = lz4_flex::block::compress(raw);
    if comp.len() + 8 >= raw.len() {
        return raw.len() + GUARD_BYTES;
    }
    let mut framed = Vec::with_capacity(comp.len() + 8);
    framed.extend_from_slice(b"HLZC");
    framed.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    framed.extend_from_slice(&comp);

    let accepts = |cap: usize| {
        let mut buf = vec![0u8; cap];
        buf[..framed.len()].copy_from_slice(&framed);
        psx_pack::decompress_hlzc_in_place(&mut buf, framed.len()) == Some(raw.len())
    };
    let mut low = raw.len().max(framed.len());
    let mut high = raw.len() + comp.len(); // disjoint output/source always fits
    assert!(accepts(high), "HLZC decoder rejected disjoint staging");
    while low < high {
        let mid = low + (high - low) / 2;
        if accepts(mid) {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    low + GUARD_BYTES
}

fn scan_room_budget(
    repo_root: &std::path::Path,
) -> (usize, usize, usize, usize, usize, usize, usize) {
    let rooms = repo_root.join("data/rooms");
    println!("cargo:rerun-if-changed={}", rooms.display());

    let Ok(entries) = fs::read_dir(&rooms) else {
        return (
            FALLBACK_MAP_WORDS,
            FALLBACK_MAX_VERTS,
            FALLBACK_MAX_FACES,
            FALLBACK_MAX_FACE_GROUPS,
            FALLBACK_MAX_LEAVES,
            FALLBACK_MAX_ENTS,
            FALLBACK_MAX_TEX_SLOTS,
        );
    };

    let mut max_bytes = 0usize;
    let mut max_verts = 0usize;
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
        if !name.starts_with("room_") || !(name.ends_with(".psxc") || name.ends_with(".psxw")) {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        if data.len() >= 8 && &data[0..4] == b"HLTX" {
            max_bytes = max_bytes.max(packed_map_capacity(&data));
            max_texs = max_texs.max(rd_u32(&data, 4).unwrap_or(0) as usize);
            continue;
        }
        if data.len() < 52
            || (&data[0..4] != b"HLMA"
                && &data[0..4] != b"HLMB"
                && &data[0..4] != b"HLMC"
                && &data[0..4] != b"HLMD")
        {
            continue;
        }

        max_bytes = max_bytes.max(packed_map_capacity(&data));
        max_verts = max_verts.max(rd_u32(&data, 4).unwrap_or(0) as usize);
        let n_texs = rd_u32(&data, 12).unwrap_or(0) as usize;
        let n_faces = rd_u32(&data, 16).unwrap_or(0) as usize;
        let bsp_off = rd_u32(&data, 20).unwrap_or(0) as usize;
        let ent_off = rd_u32(&data, 28).unwrap_or(0) as usize;
        max_texs = max_texs.max(n_texs);

        if bsp_off + 24 <= data.len() {
            let n_face_groups = rd_u32(&data, bsp_off + 4).unwrap_or(0) as usize;
            let leaf_counts = rd_u32(&data, bsp_off + 12).unwrap_or(0);
            // VIS_BITS only holds world PVS clusters. HLMC/D separate those
            // from submodel-only leaf records; legacy formats assumed every
            // non-solid leaf was a PVS bit.
            let n_visleaves = if &data[0..4] == b"HLMC" || &data[0..4] == b"HLMD" {
                (leaf_counts >> 16) as usize
            } else {
                (leaf_counts as usize).saturating_sub(1)
            };
            max_leaves = max_leaves.max(n_visleaves);
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

    // These chunk families are also HLZC-compressed by mkisopsx and staged in
    // MAP_BUF before the resident world chunk replaces them. Model chunks use
    // MODEL_BUF and have ids below the packer's compression threshold.
    for relative in ["data/sfx", "data/voices", "data/sprites"] {
        let dir = repo_root.join(relative);
        println!("cargo:rerun-if-changed={}", dir.display());
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Some(raw_id) = stem.strip_prefix("chunk_") else {
                continue;
            };
            if raw_id.parse::<u32>().ok().is_some_and(|id| id >= 3000) {
                println!("cargo:rerun-if-changed={}", path.display());
                if let Ok(data) = fs::read(path) {
                    max_bytes = max_bytes.max(packed_map_capacity(&data));
                }
            }
        }
    }

    if max_bytes == 0 {
        return (
            FALLBACK_MAP_WORDS,
            FALLBACK_MAX_VERTS,
            FALLBACK_MAX_FACES,
            FALLBACK_MAX_FACE_GROUPS,
            FALLBACK_MAX_LEAVES,
            FALLBACK_MAX_ENTS,
            FALLBACK_MAX_TEX_SLOTS,
        );
    }

    (
        max_bytes.div_ceil(4),
        // Projection scratch is indexed directly, but has no SIMD/alignment
        // requirement. Preserve the audited +128-vertex recook guard while
        // avoiding up to 255 vertices of dead linker allocation.
        round_up(max_verts + 128, 32),
        round_up(max_face_records + 32, 256),
        // PVS_GROUP_VIS stores one bit per group, so keep this divisible by 32.
        round_up(max_face_groups + 32, 32),
        round_up(max_leaves + 64, 256),
        // Every entity-backed table is build-time sized from this value and
        // rooms are immutable at runtime. Keep two spare records for cooker
        // drift; a recook automatically grows the generated cap.
        round_up(max_ents + 2, 2),
        round_up(max_texs + 8, 16),
    )
}

fn scan_model_budget(repo_root: &std::path::Path) -> (usize, usize) {
    let modelpack = repo_root.join("data/modelpack");
    println!("cargo:rerun-if-changed={}", modelpack.display());

    let Ok(entries) = fs::read_dir(&modelpack) else {
        return (FALLBACK_MODEL_WORDS, MODEL_INDEX_VERTEX_LIMIT);
    };

    let mut max_bytes = 0usize;
    let mut max_verts = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("chunk_") || !name.ends_with(".psxm") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());
        if let Ok(data) = fs::read(&path) {
            max_bytes = max_bytes.max(data.len());
            // HMRG wrapper (8 bytes), followed by HMDx's u32 vertex count at
            // geometry offset +4. Face indices are ten-bit at runtime.
            if data.get(0..4) == Some(b"HMRG") && data.len() >= 16 {
                max_verts = max_verts.max(rd_u32(&data, 12).unwrap_or(0) as usize);
            }
        }
    }

    let model_words = if max_bytes == 0 {
        FALLBACK_MODEL_WORDS.max(MODEL_POOL_WORDS)
    } else {
        round_up(max_bytes.div_ceil(4) + 256, 256).max(MODEL_POOL_WORDS)
    };
    let model_verts = if max_verts == 0 {
        MODEL_INDEX_VERTEX_LIMIT
    } else {
        assert!(
            max_verts <= MODEL_INDEX_VERTEX_LIMIT,
            "cooked model has {max_verts} vertices; packed face indices support at most {MODEL_INDEX_VERTEX_LIMIT}"
        );
        // Projection is in-place into MODEL_SCRATCH. Keep a small asset-drift
        // guard and let the shipping linker/memory gate expose future growth.
        round_up(max_verts + 16, 16).min(MODEL_INDEX_VERTEX_LIMIT)
    };
    (model_words, model_verts)
}

fn scan_pack_cache_budget(repo_root: &std::path::Path) -> usize {
    let inputs = [
        ("data/rooms", "room_"),
        ("data/modelpack", "chunk_"),
        ("data/sfx", "chunk_"),
        ("data/voices", "chunk_"),
        ("data/sprites", "chunk_"),
    ];
    let mut ids = HashSet::new();
    for (relative, prefix) in inputs {
        let dir = repo_root.join(relative);
        println!("cargo:rerun-if-changed={}", dir.display());
        let Ok(entries) = fs::read_dir(&dir) else {
            return FALLBACK_PACK_CACHE_ENTRIES;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if prefix == "room_"
                && !matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("psxc" | "psxw")
                )
            {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Some(raw_id) = stem.strip_prefix(prefix) else {
                continue;
            };
            let id = raw_id
                .parse::<u32>()
                .unwrap_or_else(|_| panic!("invalid WORLD.PAK chunk filename: {}", path.display()));
            assert!(ids.insert(id), "duplicate WORLD.PAK chunk id {id}");
        }
    }
    if ids.is_empty() {
        FALLBACK_PACK_CACHE_ENTRIES
    } else {
        // Keep 16 spare chunks, rounded to a stable cache-allocation step.
        round_up(ids.len() + 16, 32)
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

    let (map_words, max_verts, max_faces, max_face_groups, max_leaves, max_ents, max_tex_slots) =
        scan_room_budget(repo_root);
    let (model_words, max_model_verts) = scan_model_budget(repo_root);
    let pack_cache_entries = scan_pack_cache_budget(repo_root);
    assert_eq!(max_face_groups % 32, 0);
    assert!(MODEL_INDEX_VERTEX_LIMIT <= 1 << 10);
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    write_reference_checkpoint(&out_dir);
    let budget = format!(
        "pub const MAP_WORDS: usize = {map_words};\n\
         pub const MODEL_WORDS: usize = {model_words};\n\
         pub const MAX_VERTS: usize = {max_verts};\n\
         pub const MAX_MODEL_VERTS: usize = {max_model_verts};\n\
         pub const MAX_FACES: usize = {max_faces};\n\
         pub const MAX_FACE_GROUPS: usize = {max_face_groups};\n\
         pub const MAX_LEAVES: usize = {max_leaves};\n\
         pub const MAX_ENTS: usize = {max_ents};\n\
         pub const MAX_TEX_SLOTS: usize = {max_tex_slots};\n\
         pub const PACK_CACHE_ENTRIES: usize = {pack_cache_entries};\n"
    );
    fs::write(out_dir.join("room_budget.rs"), budget).expect("write generated room budget");
}
