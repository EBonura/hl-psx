//! Inject PSoXide's PSX linker script into the final link. The root Rust build
//! driver hydrates the pinned SDK source under `.psoxide`; `PSOXIDE`
//! remains an explicit override for development and diagnostics.

use std::{collections::HashSet, env, fs, path::PathBuf};

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
// Audited against the worst per-map and carry-first transition streaming peak
// by the Rust root builder (`host/hl-build/model_audit.rs`):
// HMD8 actors store one bone-local mesh plus packed baked bone palettes. The
// complete 103-map + 237-transition simulation currently peaks at 55,636 words
// on the c4a1c -> c4a1b carry path. Reserve 61,376 words, leaving a 22,960 B
// guard while returning obsolete HMD6-era headroom to PS1 RAM. The
// audit runs after asset/model cooks and fails before a growing model can hide
// an actor on hardware.
// The map/model audit mirrors this cap and rejects any future campaign recook
// that outgrows it.
// Full-campaign recook peaks at 56,206 words (c1a2b). Keeping 56,960 leaves
// 3,016 bytes of measured model-stream slack while returning another 2,048
// bytes to the executable/linker budget for the debug menus. `cargo run --
// audit` re-measures the peak and fails before a growing model can hide an
// actor on hardware, so this floor is only ever as safe as that run.
const MODEL_POOL_WORDS: usize = 55_936;

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
    println!("cargo:rerun-if-env-changed=HLPSX_INITIAL_HEALTH");
    let initial_health = std::env::var("HLPSX_INITIAL_HEALTH").ok().map(|raw| {
        raw.parse::<u16>()
            .expect("HLPSX_INITIAL_HEALTH must be an unsigned 16-bit integer")
    });

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

    if origin_psx.is_some() || angles_hl.is_some() || initial_health.is_some() {
        assert!(
            std::env::var_os("CARGO_FEATURE_SEMANTIC_INPUT").is_some(),
            "HLPSX_INITIAL_ORIGIN/ANGLES/HEALTH are diagnostic-only and require --features semantic-input"
        );
    }
    // HLPSX_DBG_CAM pins the camera to "x y z yaw pitch" in RAW runtime
    // units: exactly the numbers the on-screen debug-stats pose lines show,
    // so a photo of real hardware reproduces the same view here with no
    // convention conversion. Unset emits None and compiles out entirely.
    println!("cargo:rerun-if-env-changed=HLPSX_DBG_CAM");
    let dbg_cam = std::env::var("HLPSX_DBG_CAM").ok().map(|raw| {
        let values = raw
            .split_whitespace()
            .map(|value| {
                value
                    .parse::<i32>()
                    .unwrap_or_else(|_| panic!("HLPSX_DBG_CAM contains non-integer {value:?}"))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            values.len(),
            5,
            "HLPSX_DBG_CAM must be five integers: x y z yaw pitch"
        );
        let yaw = values[3].rem_euclid(4096) as u16;
        let pitch = i16::try_from(values[4]).expect("HLPSX_DBG_CAM pitch is outside i16 range");
        ([values[0], values[1], values[2]], yaw, pitch)
    });
    let generated = format!(
        "pub const INITIAL_ORIGIN: Option<[i32; 3]> = {origin_psx:?};\n\
         pub const INITIAL_HEALTH: Option<u16> = {initial_health:?};\n\
         pub const INITIAL_YAW: Option<u16> = {yaw_psx:?};\n\
         pub const INITIAL_PITCH: Option<i16> = {pitch_psx:?};\n\
         pub const DBG_CAM_OVERRIDE: Option<([i32; 3], u16, i16)> = {dbg_cam:?};\n"
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

fn rd_i32(d: &[u8], o: usize) -> Option<i32> {
    Some(rd_u32(d, o)? as i32)
}

fn vec3_i32(d: &[u8], o: usize) -> Option<[i32; 3]> {
    Some([rd_i32(d, o)?, rd_i32(d, o + 4)?, rd_i32(d, o + 8)?])
}

fn cooked_line_clear(data: &[u8], p1: [i32; 3], p2: [i32; 3]) -> bool {
    fn recurse(
        data: &[u8],
        planes: usize,
        nodes: usize,
        n_nodes: usize,
        node: i32,
        p1: [i32; 3],
        p2: [i32; 3],
        depth: u8,
    ) -> bool {
        if depth > 120 {
            return false;
        }
        if node < 0 {
            return (-node - 1) != 0; // leaf zero is the shared solid leaf
        }
        let node = node as usize;
        if node >= n_nodes {
            return false;
        }
        let no = nodes + node * 6;
        let Some(plane) = rd_u16(data, no).map(usize::from) else {
            return false;
        };
        let po = planes + plane * 10;
        let Some(n) = (|| {
            Some([
                rd_u16(data, po)? as i16 as i32,
                rd_u16(data, po + 2)? as i16 as i32,
                rd_u16(data, po + 4)? as i16 as i32,
            ])
        })() else {
            return false;
        };
        let Some(dist) = rd_i32(data, po + 6) else {
            return false;
        };
        let side = |p: [i32; 3]| {
            (((n[0] as i64 * p[0] as i64)
                + (n[1] as i64 * p[1] as i64)
                + (n[2] as i64 * p[2] as i64))
                >> 7) as i32
                - dist
        };
        let t1 = side(p1);
        let t2 = side(p2);
        let c0 = rd_u16(data, no + 2).unwrap_or(0) as i16 as i32;
        let c1 = rd_u16(data, no + 4).unwrap_or(0) as i16 as i32;
        if t1 >= 0 && t2 >= 0 {
            return recurse(data, planes, nodes, n_nodes, c0, p1, p2, depth + 1);
        }
        if t1 < 0 && t2 < 0 {
            return recurse(data, planes, nodes, n_nodes, c1, p1, p2, depth + 1);
        }
        let denom = t1 as i64 - t2 as i64;
        let frac = if denom == 0 {
            0
        } else {
            ((t1 as i64 * 4096) / denom).clamp(0, 4096) as i32
        };
        let mid = [
            p1[0] + (((p2[0] - p1[0]) * frac) >> 12),
            p1[1] + (((p2[1] - p1[1]) * frac) >> 12),
            p1[2] + (((p2[2] - p1[2]) * frac) >> 12),
        ];
        let (near, far) = if t1 < 0 { (c1, c0) } else { (c0, c1) };
        recurse(data, planes, nodes, n_nodes, near, p1, mid, depth + 1)
            && recurse(data, planes, nodes, n_nodes, far, mid, p2, depth + 1)
    }

    let Some(bsp) = rd_u32(data, 20).map(|v| v as usize) else {
        return false;
    };
    let n_planes = rd_u32(data, bsp).unwrap_or(0) as usize;
    let n_groups = rd_u32(data, bsp + 4).unwrap_or(0) as usize;
    let n_nodes = rd_u32(data, bsp + 8).unwrap_or(0) as usize;
    let n_faces = rd_u32(data, 16).unwrap_or(0) as usize;
    let planes = bsp + 24;
    let nodes = planes + n_planes * 10 + n_groups * 2 + n_faces * 16;
    n_nodes > 0 && recurse(data, planes, nodes, n_nodes, 0, p1, p2, 0)
}

/// Derive the headless subject cameras from cooked runtime data on the host.
/// The PS1 only receives the tiny poses; none of this search/parser code
/// or the original Steam coordinates enters the executable.
fn regression_viewpoint(repo_root: &std::path::Path, map_index: usize) -> ([i32; 3], u16, i16) {
    if map_index == 97 {
        // Hazard Course ordering repro from the audited GoldSrc camera:
        // setpos -150 250 204; setang 12 90 0. Runtime coordinates are
        // [Gold X, Gold Z, Gold Y], yaw is (90-HL yaw), and pitch is Q0.12.
        return ([-150, 204, 250], 0, 137);
    }
    let path = repo_root
        .join("data/rooms")
        .join(format!("room_{}.psxc", map_index * 2));
    let data = fs::read(&path).unwrap_or_default();
    let clip_off = rd_u32(&data, 24).unwrap_or(0) as usize;
    let ent_off = rd_u32(&data, 28).unwrap_or(0) as usize;
    let prop_off = rd_u32(&data, 36).unwrap_or(0) as usize;
    let nav_off = rd_u32(&data, 44).unwrap_or(0) as usize;
    let logic_off = rd_u32(&data, 48).unwrap_or(0) as usize;
    let spawn = vec3_i32(&data, clip_off + 16).unwrap_or([0; 3]);
    if matches!(map_index, 6 | 9 | 11) {
        let spawn_yaw = rd_i32(&data, clip_off + 28).unwrap_or(0).rem_euclid(4096) as u16;
        return (spawn, spawn_yaw, 0);
    }
    let mut target = None;
    let mut direct_pos = None;
    let mut fan_axis = None;
    let mut beam_segment = None;

    let n_models = rd_u32(&data, ent_off).unwrap_or(0) as usize;
    let n_ents_off = ent_off + 4 + n_models * 8;
    let n_ents = rd_u32(&data, n_ents_off).unwrap_or(0) as usize;
    let ents_off = n_ents_off + 4;
    if map_index == 18 {
        for ei in 0..n_ents {
            let o = ents_off + ei * 56;
            if rd_u16(&data, o + 2).unwrap_or(0) & 0xff == 5 {
                let origin = vec3_i32(&data, o + 4).unwrap_or([0; 3]);
                let center = vec3_i32(&data, o + 28).unwrap_or([0; 3]);
                fan_axis = rd_i32(&data, o + 20);
                // Rotating submodel bounds are entity-local; the subject is
                // the authored pivot plus that local centre.
                target = Some([
                    origin[0] + center[0],
                    origin[1] + center[1],
                    origin[2] + center[2],
                ]);
                break;
            }
        }
    } else if map_index == 64 {
        let mut best_area = -1i64;
        for ei in 0..n_ents {
            let o = ents_off + ei * 56;
            if rd_u16(&data, o + 2).unwrap_or(0) & 0xff != 6 {
                continue;
            }
            let movement = vec3_i32(&data, o + 16).unwrap_or([0; 3]);
            let center = vec3_i32(&data, o + 28).unwrap_or([0; 3]);
            let area = movement[0].abs() as i64 * movement[2].abs() as i64;
            if area > best_area {
                best_area = area;
                let surface = center[1] + movement[1].abs();
                direct_pos = Some([center[0], surface + 24, center[2]]);
                target = Some([center[0] + movement[0] / 2, surface, center[2]]);
            }
        }
    }

    if map_index == 23 {
        let n_logic = rd_u16(&data, logic_off).unwrap_or(0) as usize;
        let n_aux = rd_u16(&data, logic_off + 2).unwrap_or(0) as usize;
        let records = logic_off + 8;
        let aux = records + n_logic * 64;
        let mut best_length2 = -1i64;
        for li in 0..n_logic {
            let o = records + li * 64;
            let first = rd_u16(&data, o + 12).unwrap_or(0) as usize;
            if data.get(o).copied() != Some(39)
                || rd_u16(&data, o + 2).unwrap_or(0) & 1 == 0
                || data.get(o + 14).copied().unwrap_or(0) < 4
                || first + 3 >= n_aux
            {
                continue;
            }
            let a = |index: usize| {
                let ao = aux + index * 4;
                [
                    rd_u16(&data, ao).unwrap_or(0) as i16 as i32,
                    rd_u16(&data, ao + 2).unwrap_or(0) as i16 as i32,
                ]
            };
            let (a0, a1, a2, a3) = (a(first), a(first + 1), a(first + 2), a(first + 3));
            let start = [a0[0], a0[1], a1[0]];
            let end = [a2[0], a2[1], a3[0]];
            let dx = (end[0] - start[0]) as i64;
            let dy = (end[1] - start[1]) as i64;
            let dz = (end[2] - start[2]) as i64;
            let length2 = dx * dx + dy * dy + dz * dz;
            if length2 > best_length2 {
                best_length2 = length2;
                beam_segment = Some((start, end));
                target = Some([
                    (start[0] + end[0]) / 2,
                    (start[1] + end[1]) / 2,
                    (start[2] + end[2]) / 2,
                ]);
            }
        }
    } else if map_index == 94 {
        let count = (rd_u32(&data, prop_off).unwrap_or(0) & 0xffff) as usize;
        for pi in 0..count {
            let o = prop_off + 4 + pi * 24;
            if rd_u16(&data, o).unwrap_or(0) & 0x0fff == 17 {
                // Nihilanth's cooked first-frame bounds reach roughly 1,500
                // world units above its origin. Aim at the torso, not its feet.
                target = vec3_i32(&data, o + 4).map(|p| [p[0], p[1] + 600, p[2]]);
                break;
            }
        }
    }

    let target = target.unwrap_or([spawn[0], spawn[1] + 28, spawn[2] + 128]);
    let mut position = direct_pos.unwrap_or(spawn);
    if direct_pos.is_none() {
        let n_nav = rd_u16(&data, nav_off).unwrap_or(0) as usize;
        let nodes = nav_off + 4;
        let desired = if map_index == 94 { 760i64 } else { 220i64 };
        let desired2 = desired * desired;
        let mut best_score = i64::MAX;
        for ni in 0..n_nav {
            let Some(p) = vec3_i32(&data, nodes + ni * 18) else {
                continue;
            };
            let eye = [p[0], p[1] + 28, p[2]];
            let dx = (target[0] - eye[0]) as i64;
            let dy = (target[1] - eye[1]) as i64;
            let dz = (target[2] - eye[2]) as i64;
            let planar2 = dx * dx + dz * dz;
            let max_distance = if map_index == 94 { 1600 } else { 640 };
            if !(48 * 48..max_distance * max_distance).contains(&planar2) {
                continue;
            }
            if !cooked_line_clear(&data, eye, target) {
                continue;
            }
            let mut score = (planar2 - desired2).abs() + dy * dy * 2;
            if fan_axis == Some(1) {
                // The c1a2 wall fan spins around X; viewing primarily along X
                // shows the blade face instead of reducing it to a thin edge.
                score = score.saturating_add(dz * dz * 8);
            }
            if let Some((start, end)) = beam_segment {
                // Prefer a view where a wall hides exactly one endpoint. This
                // makes the depth-order regression visible in the artifact:
                // the beam remains present, but cannot paint over that wall.
                let occlusion_proof =
                    cooked_line_clear(&data, eye, start) ^ cooked_line_clear(&data, eye, end);
                if !occlusion_proof {
                    score = score.saturating_add(400_000);
                }
            }
            if score < best_score {
                position = p;
                best_score = score;
            }
        }
    }
    let eye = [position[0], position[1] + 28, position[2]];
    let dx = target[0] - eye[0];
    let dy = target[1] - eye[1];
    let dz = target[2] - eye[2];
    let yaw = (((dx as f64).atan2(dz as f64) * 4096.0 / std::f64::consts::TAU).round() as i32)
        .rem_euclid(4096) as u16;
    let horizontal = ((dx as f64).hypot(dz as f64)).round().max(1.0) as i32;
    let pitch = (dy * 900 / (horizontal + dy.abs()).max(1)).clamp(-700, 700) as i16;
    (position, yaw, pitch)
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
                && &data[0..4] != b"HLMD"
                && &data[0..4] != b"HLME"
                && &data[0..4] != b"HLMF"
                && &data[0..4] != b"HLMG"
                && &data[0..4] != b"HLMH")
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
            let n_visleaves = if &data[0..4] == b"HLMC"
                || &data[0..4] == b"HLMD"
                || &data[0..4] == b"HLME"
                || &data[0..4] == b"HLMF"
                || &data[0..4] == b"HLMG"
                || &data[0..4] == b"HLMH"
            {
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
                                                   // FaceRec's high four plane-group bits now carry the compact
                                                   // dynamic-lightstyle slot. Runtime group lookup masks the same
                                                   // 12-bit id; budget scanning must not treat style 15 as sixty
                                                   // thousand authored plane groups.
                let group = rd_u16(&data, o).unwrap_or(0) & 0x0fff;
                max_group = max_group.max(group as usize);
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

/// Model type count, mirrored by game/src/main.rs N_MODEL_TYPES.
const MODEL_TYPES: usize = 76;

fn scan_model_budget(repo_root: &std::path::Path) -> (usize, usize, usize, [u8; MODEL_TYPES]) {
    let modelpack = repo_root.join("data/modelpack");
    println!("cargo:rerun-if-changed={}", modelpack.display());

    let Ok(entries) = fs::read_dir(&modelpack) else {
        return (
            FALLBACK_MODEL_WORDS,
            MODEL_INDEX_VERTEX_LIMIT,
            20_224,
            core::array::from_fn(|index| index as u8),
        );
    };

    let mut max_bytes = 0usize;
    let mut max_verts = 0usize;
    let mut max_viewmodel_words = 0usize;
    let mut stream_sizes = [0usize; MODEL_TYPES];
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
            if name
                .strip_prefix("chunk_")
                .and_then(|suffix| suffix.strip_suffix(".psxm"))
                .and_then(|suffix| suffix.parse::<usize>().ok())
                .is_some_and(|id| (1000..1016).contains(&id))
            {
                max_viewmodel_words = max_viewmodel_words.max(data.len().div_ceil(4));
            }
            if let Some(type_id) = name
                .strip_prefix("chunk_13")
                .and_then(|suffix| suffix.strip_suffix(".psxm"))
                .and_then(|suffix| suffix.parse::<usize>().ok())
                .filter(|&type_id| type_id < stream_sizes.len())
            {
                stream_sizes[type_id] = data.len();
            }
            // HMRG wrapper (8 bytes), followed by HMDx's u32 vertex count at
            // geometry offset +4. Face indices are ten-bit at runtime.
            if data.get(0..4) == Some(b"HMRG") && data.len() >= 16 {
                max_verts = max_verts.max(rd_u32(&data, 12).unwrap_or(0) as usize);
            }
        }
    }

    // Map-specialized HMD8 chunks live outside the legacy 1300+type id range.
    // Fold their exact payloads back into each type's priority so the runtime
    // still streams the largest transient first within an AI tier.
    let variants = modelpack.join("map-model-variants.txt");
    println!("cargo:rerun-if-changed={}", variants.display());
    if let Ok(index) = fs::read_to_string(&variants) {
        for line in index.lines().filter(|line| !line.starts_with('#')) {
            let fields = line.split('|').collect::<Vec<_>>();
            if fields.len() < 4 {
                continue;
            }
            let (Ok(type_id), Ok(chunk_id)) =
                (fields[2].parse::<usize>(), fields[3].parse::<u32>())
            else {
                continue;
            };
            if type_id >= stream_sizes.len() {
                continue;
            }
            let path = modelpack.join(format!("chunk_{chunk_id}.psxm"));
            println!("cargo:rerun-if-changed={}", path.display());
            if let Ok(data) = fs::read(path) {
                stream_sizes[type_id] = stream_sizes[type_id].max(data.len());
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
    // Load the largest staging payloads first within each runtime importance
    // tier. Resident frame prefixes only grow as streaming proceeds, so this
    // order minimizes transient peak RAM without changing draw/AI semantics.
    let mut stream_order = core::array::from_fn(|index| index as u8);
    stream_order
        .sort_by_key(|&type_id| (core::cmp::Reverse(stream_sizes[type_id as usize]), type_id));
    let max_viewmodel_words = if max_viewmodel_words == 0 {
        20_224
    } else {
        max_viewmodel_words
    };
    (model_words, model_verts, max_viewmodel_words, stream_order)
}

/// Exact cooked mesh signatures for the 16 first-person models. The debug
/// weapon gallery uses these to prove that an on-demand stream/cache hit did
/// not merely publish the requested id over stale geometry.
fn scan_viewmodel_signatures(repo_root: &std::path::Path) -> ([u16; 16], [u16; 16]) {
    let modelpack = repo_root.join("data/modelpack");
    let mut verts = [0u16; 16];
    let mut tris = [0u16; 16];
    for index in 0..16 {
        let path = modelpack.join(format!("chunk_{}.psxm", 1000 + index));
        println!("cargo:rerun-if-changed={}", path.display());
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        if data.get(0..4) != Some(b"HMRG") || data.get(8..12) != Some(b"HMD8") {
            continue;
        }
        let n_verts = rd_u32(&data, 12).unwrap_or(0);
        let n_tris = rd_u32(&data, 16).unwrap_or(0);
        verts[index] = u16::try_from(n_verts).expect("viewmodel vertex count exceeds u16");
        tris[index] = u16::try_from(n_tris).expect("viewmodel triangle count exceeds u16");
    }
    (verts, tris)
}

fn scan_model_variant_index(repo_root: &std::path::Path) -> (Vec<u16>, Vec<u8>) {
    let path = repo_root.join("data/modelpack/map-model-variants.txt");
    println!("cargo:rerun-if-changed={}", path.display());
    let Ok(text) = fs::read_to_string(&path) else {
        return (vec![0], Vec::new());
    };
    let mut records = Vec::new();
    let mut max_map = 0usize;
    let mut seen = HashSet::new();
    for (line_no, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('|').collect::<Vec<_>>();
        assert!(
            fields.len() >= 4,
            "{}:{}: malformed model variant record",
            path.display(),
            line_no + 1
        );
        let map = fields[0].parse::<usize>().expect("variant map index");
        let ty = fields[2].parse::<u8>().expect("variant model type");
        let chunk = fields[3].parse::<u16>().expect("variant chunk id");
        assert!(ty < 57, "variant model type {ty} is out of range");
        assert!(
            seen.insert((map, ty)),
            "duplicate map/type variant {map}/{ty}"
        );
        max_map = max_map.max(map);
        records.push((map, ty, chunk));
    }
    records.sort_unstable();
    let mut offsets = vec![0u16; max_map + 2];
    let mut bytes = Vec::with_capacity(records.len() * 3);
    let mut cursor = 0usize;
    for map in 0..=max_map {
        offsets[map] = u16::try_from(cursor).expect("too many model variants");
        while cursor < records.len() && records[cursor].0 == map {
            let (_, ty, chunk) = records[cursor];
            bytes.push(ty);
            bytes.extend_from_slice(&chunk.to_le_bytes());
            cursor += 1;
        }
    }
    offsets[max_map + 1] = u16::try_from(cursor).expect("too many model variants");
    (offsets, bytes)
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

fn talk_voice_layout(repo: &std::path::Path) -> Vec<[u8; 7]> {
    let path = repo.join("data/voices/manifest.txt");
    println!("cargo:rerun-if-changed={}", path.display());
    let manifest = fs::read_to_string(path).expect("recook voices before compiling NPC replies");
    let map_list = repo.join("host/hl-content/map-list.txt");
    println!("cargo:rerun-if-changed={}", map_list.display());
    let map_count = fs::read_to_string(map_list)
        .expect("read campaign map list")
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
        .count();
    let mut rows = vec![[255, 255, 255, 255, 255, 255, 0]; map_count];
    let mut greetings = std::collections::HashSet::new();
    for line in manifest.lines() {
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() != 3 {
            continue;
        }
        let (Ok(map), Ok(id)) = (fields[0].parse::<usize>(), fields[1].parse::<u8>()) else {
            continue;
        };
        assert!(map < map_count, "voice manifest references an unknown map");
        if fields[2].starts_with("SC_HELLO") || fields[2].starts_with("SC_PHELLO") {
            greetings.insert(map);
        }
        let slot = match fields[2] {
            "use:barney:start" => Some(0),
            "use:barney:stop" => Some(1),
            "use:barney:decline" => Some(2),
            "use:scientist:start" => Some(3),
            "use:scientist:stop" => Some(4),
            "use:scientist:decline" => Some(5),
            _ => None,
        };
        if let Some(slot) = slot {
            assert!(
                id < 64,
                "NPC use voice exceeds runtime mouth-controller id mask"
            );
            rows[map][slot] = id;
            rows[map][6] = rows[map][6].max(id + 1);
        }
    }
    for (map, row) in rows.iter_mut().enumerate() {
        if !greetings.contains(&map) {
            row[6] |= 0x80;
        }
    }
    assert!(
        rows.iter()
            .any(|row| row[..6].iter().any(|id| *id != u8::MAX)),
        "recook voices to add NPC use replies"
    );
    rows
}

fn main() {
    // This crate lives at <repo>/game, so the repo root is one level up.
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest.parent().expect("crate must live at <repo>/game");
    let psoxide = std::env::var("PSOXIDE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo_root.join(".psoxide"));
    let ld = psoxide.join("sdk/psoxide.ld");
    let ld = ld.canonicalize().unwrap_or(ld);
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // hl-psx measures its real stack high-water mark in emulator-telemetry
    // builds. Keep the SDK's full 32 KiB link-time reservation: menu/intro
    // entry points are deliberately kept out of main so their mutually
    // exclusive scratch frames do not inflate main's permanent frame and
    // collide with the tail of BSS. Generate the derivative in OUT_DIR so the
    // pinned SDK remains pristine and an upstream linker-layout change fails
    // loudly rather than being patched blindly.
    let sdk_linker = fs::read_to_string(&ld).expect("read PSoXide linker script");
    const SDK_STACK: &str = "STACK_RESERVE = 0x8000;";
    const HLPSX_STACK: &str = SDK_STACK;
    assert_eq!(sdk_linker.matches(SDK_STACK).count(), 1);
    // Rare renderer extensions live outside `.text.*` so enabling one does
    // not shift the PS1's cache-sensitive world loop. Keep the custom bucket
    // inside the executable text output, but deliberately after ordinary code.
    const SDK_TEXT: &str = "        *(.text .text.*);";
    const HLPSX_TEXT: &str = "        *(.text .text.*);\n        *(.hlpsx_cold .hlpsx_cold.*);";
    assert_eq!(sdk_linker.matches(SDK_TEXT).count(), 1);
    let linker = sdk_linker
        .replace(SDK_STACK, HLPSX_STACK)
        .replace(SDK_TEXT, HLPSX_TEXT);
    let hlpsx_ld = out_dir.join("hl-psx.ld");
    fs::write(&hlpsx_ld, linker).expect("write hl-psx linker script");

    // `-T` selects the linker script; `--oformat=binary` dumps a flat PSX-EXE
    // image (the script lays out the executable header) instead of an ELF.
    println!("cargo:rustc-link-arg=-T{}", hlpsx_ld.display());
    println!("cargo:rustc-link-arg=--oformat=binary");
    println!("cargo:rerun-if-changed={}", ld.display());
    // Optional linker map for PC-sample attribution. A link-only argument
    // leaves the emitted bytes untouched, unlike RUSTFLAGS, which enters
    // every crate's fingerprint and reshuffles the whole code layout (a
    // measured 133 KB of byte differences). Verified byte-identical with the
    // hook on and off before first use.
    println!("cargo:rerun-if-env-changed=HLPSX_LINK_MAP");
    if let Ok(map_path) = env::var("HLPSX_LINK_MAP") {
        if !map_path.is_empty() {
            println!("cargo:rustc-link-arg=-Map={map_path}");
        }
    }

    let (map_words, max_verts, max_faces, max_face_groups, max_leaves, max_ents, max_tex_slots) =
        scan_room_budget(repo_root);
    let (model_words, max_model_verts, max_viewmodel_words, model_stream_order) =
        scan_model_budget(repo_root);
    let (viewmodel_verts, viewmodel_tris) = scan_viewmodel_signatures(repo_root);
    let (model_variant_offsets, model_variant_bytes) = scan_model_variant_index(repo_root);
    let pack_cache_entries = scan_pack_cache_budget(repo_root);
    let regression_viewpoints = [64usize, 23, 18, 94, 6, 9, 11, 97]
        .map(|map_index| regression_viewpoint(repo_root, map_index));
    assert_eq!(max_face_groups % 32, 0);
    assert!(MODEL_INDEX_VERTEX_LIMIT <= 1 << 10);
    write_reference_checkpoint(&out_dir);
    let talk_voices = talk_voice_layout(repo_root);
    let budget = format!(
        "pub const TALK_VOICES: [[u8; 7]; {}] = {talk_voices:?};\n\
         pub const MAP_WORDS: usize = {map_words};\n\
         pub const MODEL_WORDS: usize = {model_words};\n\
         pub const MAX_VERTS: usize = {max_verts};\n\
         pub const MAX_MODEL_VERTS: usize = {max_model_verts};\n\
         pub const MAX_VIEWMODEL_WORDS: usize = {max_viewmodel_words};\n\
         pub const VIEWMODEL_VERTS: [u16; 16] = {viewmodel_verts:?};\n\
         pub const VIEWMODEL_TRIS: [u16; 16] = {viewmodel_tris:?};\n\
         pub const MODEL_STREAM_ORDER: [u8; {MODEL_TYPES}] = {model_stream_order:?};\n\
         pub const MODEL_VARIANT_OFFSETS: [u16; {}] = {model_variant_offsets:?};\n\
         pub const MODEL_VARIANT_BYTES: [u8; {}] = {model_variant_bytes:?};\n\
         pub const REGRESSION_VIEWPOINTS: [([i32; 3], u16, i16); 8] = {regression_viewpoints:?};\n\
         pub const MAX_FACES: usize = {max_faces};\n\
         pub const MAX_FACE_GROUPS: usize = {max_face_groups};\n\
         pub const MAX_LEAVES: usize = {max_leaves};\n\
         pub const MAX_ENTS: usize = {max_ents};\n\
         pub const MAX_TEX_SLOTS: usize = {max_tex_slots};\n\
         pub const PACK_CACHE_ENTRIES: usize = {pack_cache_entries};\n",
        talk_voices.len(),
        model_variant_offsets.len(),
        model_variant_bytes.len(),
    );
    fs::write(out_dir.join("room_budget.rs"), budget).expect("write generated room budget");
}
