//! Ordering oracle for hl-psx: compares a frame's GPU draw order with a depth-tested render of
//! the identical packets. See `game/src/order_trace.rs` for the stream it reads.

mod analyze;
mod bsp;
mod log;
mod poses;
mod pvsmax;
mod raster;

use analyze::{analyze, plane_depth_range, Class, Context, FrameStats, Tolerance, CAUSES};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: hl-order-oracle analyze --out DIR [--bsp-dir DIR] [--face-map DIR] [--png-all] [--png-major N] LOG...\n       hl-order-oracle poses BSP_DIR MAP [MAP...] [--spots N] [--yaws A,B,C]";

struct Args {
    out: PathBuf,
    bsp_dir: Option<PathBuf>,
    face_map: Option<PathBuf>,
    png_all: bool,
    png_major: u64,
    logs: Vec<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    if it.next().as_deref() != Some("analyze") {
        return Err(USAGE.to_string());
    }
    let mut out = None;
    let mut bsp_dir = None;
    let mut face_map = None;
    let mut png_all = false;
    let mut png_major = 400;
    let mut logs = Vec::new();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" => out = it.next().map(PathBuf::from),
            "--bsp-dir" => bsp_dir = it.next().map(PathBuf::from),
            "--face-map" => face_map = it.next().map(PathBuf::from),
            "--png-all" => png_all = true,
            "--png-major" => {
                png_major = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--png-major needs a number")?
            }
            _ => logs.push(PathBuf::from(arg)),
        }
    }
    Ok(Args {
        out: out.ok_or(USAGE)?,
        bsp_dir,
        face_map,
        png_all,
        png_major,
        logs,
    })
}

/// Map name from a log file stem like `t0a0a` or `t0a0a-p3`.
fn map_of(stem: &str) -> &str {
    stem.split('-').next().unwrap_or(stem)
}

fn write_png(path: &Path, width: u32, height: u32, rgb: &[u8]) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgb).map_err(|e| e.to_string())
}

/// Three panes side by side: painter order, depth test, mask.
fn compose(images: &analyze::Images, scale: usize) -> (u32, u32, Vec<u8>) {
    let (w, h) = (raster::WIDTH as usize, raster::HEIGHT as usize);
    let out_w = w * 3 * scale;
    let out_h = h * scale;
    let mut out = vec![0u8; out_w * out_h * 3];
    for (pane, src) in [&images.painter, &images.depth, &images.mask]
        .into_iter()
        .enumerate()
    {
        for y in 0..out_h {
            for x in 0..w * scale {
                let s = ((y / scale) * w + x / scale) * 3;
                let d = (y * out_w + pane * w * scale + x) * 3;
                out[d..d + 3].copy_from_slice(&src[s..s + 3]);
            }
        }
    }
    (out_w as u32, out_h as u32, out)
}

fn poses_command() -> Result<(), String> {
    let mut it = std::env::args().skip(2);
    let dir = PathBuf::from(it.next().ok_or(USAGE)?);
    let mut maps = Vec::new();
    let mut spots = 14usize;
    let mut yaws = vec![0, 120, 240];
    let mut start = 60u32;
    let mut late: Vec<String> = Vec::new();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--spots" => {
                spots = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--spots needs a number")?
            }
            "--start" => {
                start = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--start needs a number")?
            }
            "--late" => late.push(it.next().ok_or("--late needs a map name")?),
            "--yaws" => {
                yaws = it
                    .next()
                    .ok_or("--yaws needs a list")?
                    .split(',')
                    .filter_map(|v| v.parse().ok())
                    .collect()
            }
            _ => maps.push(arg),
        }
    }
    for map in maps {
        let entities = poses::read_entities(&dir.join(format!("{map}.bsp")))?;
        let player = entities
            .iter()
            .find(|e| e.class == "info_player_start")
            .and_then(|e| e.origin);
        let pool = poses::empty_leaf_centres(&dir.join(format!("{map}.bsp")))?;
        let chosen = if pool.len() >= 4 {
            poses::spread(&pool, player, spots)
        } else {
            poses::pick_spots(&entities, spots)
        };
        let begin = if late.contains(&map) { 700 } else { start };
        print!("{}", poses::section(&map, &chosen, &yaws, 12, begin));
    }
    Ok(())
}

/// Offline experiment ("lab"): replay the packets of traced frames in the order a BSP-sequenced
/// renderer would draw them and report the wrong-order pixels of that order next to the order the
/// game used. World faces draw at the farthest visible leaf that marks them (leaves ranked by a
/// back-to-front walk from the eye); every brush face draws just after the world faces of the leaf
/// in front of its centre, ordered inside that leaf by the game's own depth key (`own_bsp` orders
/// a brush entity's faces by the entity's own BSP instead). Models, beams and packets without
/// depth keep the game's order after everything. One summary line per log.
fn lab_command() -> Result<(), String> {
    let mut it = std::env::args().skip(2);
    let dir = PathBuf::from(it.next().ok_or(USAGE)?);
    let own_bsp = std::env::var_os("LAB_OWN_BSP").is_some();
    let verbose = std::env::var_os("LAB_VERBOSE").is_some();
    let mut grand = [0u64; 5];
    for path in it {
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        let name = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let map = map_of(&name).to_string();
        let bsp = bsp::Bsp::load(&dir.join(format!("{map}.bsp")))?;
        let info = poses::MapInfo::load(&dir.join(format!("{map}.bsp")))?;
        let plane_of = |face: u16, off: [i32; 3]| bsp.face_plane_cooked(usize::from(face), off);
        let class_of = |kind: u8, face: u16| {
            if kind == log::kind::SUBMODEL || info.model_of(u32::from(face)) != 0 {
                Class::Brush(info.model_of(u32::from(face)))
            } else {
                Class::World
            }
        };
        let ctx = Context {
            class_of: &class_of,
            plane_of: &plane_of,
        };
        let mut parsed = log::parse(&text);
        if let Some(face_dir) = std::env::var_os("FACE_MAP") {
            if let Ok(csv) = fs::read_to_string(Path::new(&face_dir).join(format!("{map}.csv"))) {
                log::remap_faces(&mut parsed, &csv);
            }
        }
        let mut totals = [0u64; 5];
        let mut pair_game: std::collections::BTreeMap<(&str, &str), u64> = Default::default();
        let mut pair_seq: std::collections::BTreeMap<(&str, &str), u64> = Default::default();
        let mut face_seq: std::collections::BTreeMap<(u8, u16, u8, u16), u64> = Default::default();
        for frame in parsed {
            let eye = [
                frame.eye[0] as f32,
                frame.eye[2] as f32,
                frame.eye[1] as f32,
            ];
            let lrank = bsp.leaf_ranks(0, eye);
            let face_rank = |face: usize| -> u32 {
                bsp.face_leaves
                    .get(face)
                    .into_iter()
                    .flatten()
                    .map(|leaf| lrank[*leaf as usize])
                    .filter(|r| *r != u32::MAX)
                    .min()
                    .unwrap_or(u32::MAX)
            };
            let open = bsp.open_nodes(&lrank);
            // Per brush entity (model, draw offset): the leaf ranks of the nearest and farthest
            // points of its bounding sphere along the eye ray, and the span of its game keys.
            let mut spans: std::collections::HashMap<(u16, [i16; 3]), (u32, u32, i32, i32, f32)> =
                Default::default();
            if std::env::var("LAB_BRUSH").as_deref() == Ok("span") || std::env::var("LAB_BRUSH").as_deref() == Ok("hyb") {
                let mut verts: std::collections::HashMap<
                    (u16, [i16; 3]),
                    (Vec<[f32; 3]>, i32, i32),
                > = Default::default();
                for w in &frame.walk {
                    let Some(inf) = w.info else { continue };
                    if matches!(inf.kind, log::kind::MODEL | log::kind::BEAM) {
                        continue;
                    }
                    let model = if inf.kind == log::kind::SUBMODEL {
                        inf.brush
                    } else {
                        info.model_of(u32::from(inf.face))
                    };
                    if model == 0 {
                        continue;
                    }
                    let e = verts.entry((model, inf.offset)).or_insert((
                        Vec::new(),
                        i32::MAX,
                        i32::MIN,
                    ));
                    let off = [
                        f32::from(inf.offset[0]),
                        f32::from(inf.offset[2]),
                        f32::from(inf.offset[1]),
                    ];
                    if let Some(poly) = bsp.faces.get(usize::from(inf.face)) {
                        for v in poly {
                            e.0.push([v[0] + off[0], v[1] + off[1], v[2] + off[2]]);
                        }
                    }
                    e.1 = e.1.min(i32::from(inf.key));
                    e.2 = e.2.max(i32::from(inf.key));
                }
                for (k, (vs, kmin, kmax)) in verts {
                    if vs.is_empty() {
                        continue;
                    }
                    let n = vs.len() as f32;
                    let mut c = [0f32; 3];
                    for v in &vs {
                        for a in 0..3 {
                            c[a] += v[a] / n;
                        }
                    }
                    let r = vs
                        .iter()
                        .map(|v| {
                            ((v[0] - c[0]).powi(2) + (v[1] - c[1]).powi(2) + (v[2] - c[2]).powi(2))
                                .sqrt()
                        })
                        .fold(0f32, f32::max);
                    let d = [c[0] - eye[0], c[1] - eye[1], c[2] - eye[2]];
                    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1.0);
                    let t = r / dist;
                    let near_p = if dist < r {
                        eye
                    } else {
                        [c[0] - d[0] * t, c[1] - d[1] * t, c[2] - d[2] * t]
                    };
                    let far_p = [c[0] + d[0] * t, c[1] + d[1] * t, c[2] + d[2] * t];
                    let rank_of = |p: [f32; 3]| bsp.open_leaf(p, &lrank, &open).map(|l| lrank[l]);
                    let (Some(rn), Some(rf)) = (rank_of(near_p), rank_of(far_p)) else {
                        continue;
                    };
                    let (lo, hi) = (rf.min(rn), rf.max(rn));
                    spans.insert(k, (lo, hi, kmin, kmax, r));
                }
            }
            let mut brush_ranks: std::collections::HashMap<u16, Vec<u32>> = Default::default();
            let mut keyed: Vec<(u64, u64, u64, usize)> = Vec::new();
            // (game key, rank) of every world packet, for brush faces that cannot find a leaf.
            let key_to_rank: Vec<(u16, u32)> = frame
                .walk
                .iter()
                .filter_map(|w| {
                    let inf = w.info?;
                    let model = if inf.kind == log::kind::SUBMODEL {
                        inf.brush
                    } else {
                        info.model_of(u32::from(inf.face))
                    };
                    let rank = face_rank(usize::from(inf.face));
                    (model == 0
                        && !matches!(inf.kind, log::kind::MODEL | log::kind::BEAM)
                        && rank != u32::MAX)
                        .then_some((inf.key, rank))
                })
                .collect();
            if std::env::var("LAB_BRUSH").as_deref() == Ok("mono") {
                // Keys stay in the game's depth space; world keys are bent so they are
                // non-increasing along the BSP sequence (farther leaf, larger key).
                let mut leaf_base: std::collections::BTreeMap<u32, i32> = Default::default();
                for w in &frame.walk {
                    let Some(inf) = w.info else { continue };
                    let model = if inf.kind == log::kind::SUBMODEL {
                        inf.brush
                    } else {
                        info.model_of(u32::from(inf.face))
                    };
                    if model != 0 || matches!(inf.kind, log::kind::MODEL | log::kind::BEAM) {
                        continue;
                    }
                    let r = face_rank(usize::from(inf.face));
                    if r == u32::MAX {
                        continue;
                    }
                    let e = leaf_base.entry(r).or_insert(0);
                    *e = (*e).max(i32::from(inf.key));
                }
                let mut run = i32::MAX;
                let mut fixed: std::collections::BTreeMap<u32, i32> = Default::default();
                for (r, b) in &leaf_base {
                    run = run.min(*b);
                    fixed.insert(*r, run);
                }
                for (i, w) in frame.walk.iter().enumerate() {
                    let Some(inf) = w.info else {
                        keyed.push((u64::MAX, 0, 0, i));
                        continue;
                    };
                    let model = if inf.kind == log::kind::SUBMODEL {
                        inf.brush
                    } else {
                        info.model_of(u32::from(inf.face))
                    };
                    let key =
                        if model != 0 || matches!(inf.kind, log::kind::MODEL | log::kind::BEAM) {
                            i32::from(inf.key)
                        } else {
                            let r = face_rank(usize::from(inf.face));
                            fixed.get(&r).copied().unwrap_or(i32::from(inf.key))
                        };
                    keyed.push(((639 - key.clamp(0, 639)) as u64, 0, 0, i));
                }
            } else {
                for (i, w) in frame.walk.iter().enumerate() {
                    let Some(inf) = w.info else {
                        keyed.push((u64::MAX, 0, 0, i));
                        continue;
                    };
                    let model = if inf.kind == log::kind::SUBMODEL {
                        inf.brush
                    } else {
                        info.model_of(u32::from(inf.face))
                    };
                    if std::env::var("LAB_BRUSH").as_deref() == Ok("legacy2")
                        && (model != 0 || matches!(inf.kind, log::kind::MODEL | log::kind::BEAM))
                    {
                        // Place the entity where the fewest world packets are out of order with its key.
                        let k = i32::from(inf.key);
                        let mut sorted = key_to_rank.clone();
                        sorted.sort_by_key(|(_, r)| *r);
                        let mut best = (usize::MAX, 0usize);
                        let after_greater =
                            sorted.iter().filter(|(kk, _)| i32::from(*kk) > k).count();
                        let mut cost = after_greater;
                        // position 0: entity before everything: world packets with key < k are "after" but should be before? far-first: larger key first.
                        let mut a_less = 0usize; // packets before the entity with key < k (wrongly before)
                        let mut b_greater = after_greater; // packets after the entity with key > k (wrongly after)
                        let mut pos = 0usize;
                        best = (a_less + b_greater, 0);
                        for (j, (kk, _)) in sorted.iter().enumerate() {
                            if i32::from(*kk) < k {
                                a_less += 1;
                            }
                            if i32::from(*kk) > k {
                                b_greater -= 1;
                            }
                            cost = a_less + b_greater;
                            pos = j + 1;
                            if cost < best.0 {
                                best = (cost, pos);
                            }
                        }
                        let _ = (cost, pos);
                        let anchor_rank = if best.1 == 0 { 0 } else { sorted[best.1 - 1].1 };
                        keyed.push((
                            u64::from(anchor_rank) * 2 + 1,
                            u64::from(639 - inf.key.min(639)),
                            u64::from(model),
                            i,
                        ));
                    } else if std::env::var("LAB_BRUSH").as_deref() == Ok("legacy")
                        && (model != 0 || matches!(inf.kind, log::kind::MODEL | log::kind::BEAM))
                    {
                        // Keep the entity where its own depth key put it among the world packets.
                        let anchor = key_to_rank
                            .iter()
                            .min_by_key(|(k, _)| (i32::from(*k) - i32::from(inf.key)).abs())
                            .map_or(0, |(_, r)| *r);
                        keyed.push((
                            u64::from(anchor) * 2 + 1,
                            u64::from(639 - inf.key.min(639)),
                            u64::from(model),
                            i,
                        ));
                    } else if matches!(inf.kind, log::kind::MODEL | log::kind::BEAM) {
                        keyed.push((u64::MAX, 0, 0, i));
                    } else if model == 0 {
                        keyed.push((u64::from(face_rank(usize::from(inf.face))) * 2, 0, 0, i));
                    } else {
                        let off = [
                            f32::from(inf.offset[0]),
                            f32::from(inf.offset[2]),
                            f32::from(inf.offset[1]),
                        ];
                        let poly = &bsp.faces[usize::from(inf.face)];
                        let n = poly.len().max(1) as f32;
                        let mut c = [0f32; 3];
                        for v in poly {
                            for k in 0..3 {
                                c[k] += v[k] / n;
                            }
                        }
                        // The leaf in front of the face: centre nudged out of rock along the normal;
                        // a face with no open space in front draws behind everything.
                        let normal = bsp.face_normal(usize::from(inf.face));
                        let p = [
                            c[0] + off[0] + 4.0 * normal[0],
                            c[1] + off[1] + 4.0 * normal[1],
                            c[2] + off[2] + 4.0 * normal[2],
                        ];
                        let leaf = bsp.point_leaf(p);
                        let mut strategy = std::env::var("LAB_BRUSH").unwrap_or_default();
                        let thr: f32 = std::env::var("LAB_HYB_R")
                            .ok()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(300.0);
                        if strategy == "hyb" {
                            let big = spans.get(&(model, inf.offset)).is_some_and(|s| s.4 >= thr);
                            strategy = if big { "span".into() } else { "pkt".into() };
                        }
                        if strategy == "span" {
                            if let Some(&(lo, hi, kmin, kmax, _)) = spans.get(&(model, inf.offset)) {
                                let k = i32::from(inf.key).clamp(kmin, kmax);
                                let span = (kmax - kmin).max(1) as f64;
                                let pos =
                                    lo as f64 + f64::from(kmax - k) / span * f64::from(hi - lo);
                                keyed.push((
                                    (pos as u64) * 2 + 1,
                                    u64::from(639 - inf.key.min(639)),
                                    u64::from(model),
                                    i,
                                ));
                                continue;
                            }
                        }
                        let anchor = if !strategy.is_empty() {
                            let pts: Vec<[f32; 3]> = if strategy == "cent" {
                                vec![p]
                            } else if strategy == "pkt" {
                                // The packet's own centroid, unprojected with the guest's vertex depths.
                                match raster::decode(&w.words) {
                                    Some(poly) if poly.n >= 3 => {
                                        let mut c = [0f64; 3];
                                        for k in 0..poly.n {
                                            let z = f64::from(inf.z[k].max(1));
                                            let u = [
                                                (f64::from(poly.verts[k].0) - 160.0) / frame.h * z,
                                                (f64::from(poly.verts[k].1) - 120.0) / frame.h * z,
                                                z,
                                            ];
                                            for ax in 0..3 {
                                                // rot rows are the camera axes: world = eye + R^T u
                                                c[ax] += (frame.rot[0][ax] * u[0]
                                                    + frame.rot[1][ax] * u[1]
                                                    + frame.rot[2][ax] * u[2])
                                                    / poly.n as f64;
                                            }
                                        }
                                        let q = [
                                            frame.eye[0] as f64 + c[0],
                                            frame.eye[1] as f64 + c[1],
                                            frame.eye[2] as f64 + c[2],
                                        ];
                                        vec![[
                                            q[0] as f32 + 4.0 * normal[0],
                                            q[2] as f32 + 4.0 * normal[1],
                                            q[1] as f32 + 4.0 * normal[2],
                                        ]]
                                    }
                                    _ => vec![p],
                                }
                            } else {
                                let mut v: Vec<[f32; 3]> = poly
                                    .iter()
                                    .map(|q| {
                                        [
                                            q[0] + off[0] + 4.0 * normal[0],
                                            q[1] + off[1] + 4.0 * normal[1],
                                            q[2] + off[2] + 4.0 * normal[2],
                                        ]
                                    })
                                    .collect();
                                v.push(p);
                                v
                            };
                            let ranks: Vec<u32> = pts
                                .iter()
                                .filter_map(|q| bsp.open_leaf(*q, &lrank, &open))
                                .map(|l| lrank[l])
                                .collect();
                            let pick = match strategy.as_str() {
                                "near" => ranks.iter().copied().max(),
                                _ => ranks.iter().copied().min(),
                            };
                            if std::env::var("LAB_DEBUG_FACE")
                                .ok()
                                .and_then(|v| v.parse::<u16>().ok())
                                .is_some_and(|f| f == inf.face || f == 0)
                            {
                                println!(
                                "DBG {name} frame {:x} pkt {i} face {} key {} pts {:?} ranks {ranks:?} pick {pick:?} eye {:?}",
                                frame.index, inf.face, inf.key, pts, frame.eye
                            );
                            }
                            pick.unwrap_or_else(|| {
                                key_to_rank
                                    .iter()
                                    .min_by_key(|(k, _)| (i32::from(*k) - i32::from(inf.key)).abs())
                                    .map_or(0, |(_, r)| *r)
                            })
                        } else if leaf == 0 || lrank[leaf] == u32::MAX {
                            // No open space in front: borrow the position of the world packet whose
                            // game key is closest to this face's.
                            key_to_rank
                                .iter()
                                .min_by_key(|(k, _)| (i32::from(*k) - i32::from(inf.key)).abs())
                                .map_or(0, |(_, r)| *r)
                        } else {
                            lrank[leaf]
                        };
                        let sub = if own_bsp {
                            let local = [eye[0] - off[0], eye[1] - off[1], eye[2] - off[2]];
                            let ranks = brush_ranks.entry(model).or_insert_with(|| {
                                bsp.back_to_front_ranks(usize::from(model), local)
                            });
                            let node = bsp
                                .node_of_face
                                .get(usize::from(inf.face))
                                .copied()
                                .unwrap_or(u32::MAX);
                            if node == u32::MAX {
                                0
                            } else {
                                u64::from(ranks[node as usize])
                            }
                        } else {
                            // far first: the larger the game's depth key, the earlier
                            u64::from(639 - inf.key.min(639))
                        };
                        if std::env::var("LAB_DEBUG_FACE")
                            .ok()
                            .and_then(|v| v.parse::<u16>().ok())
                            .is_some_and(|f| f == inf.face || f == 0)
                        {
                            println!("DBG   anchor {anchor} sort {}", anchor * 2 + 1);
                        }
                        keyed.push((u64::from(anchor) * 2 + 1, sub, u64::from(model), i));
                    }
                }
                if std::env::var("LAB_MODE").as_deref() == Ok("bounded") {
                    // World faces sequenced by farthest leaf but keyed in the game's own depth space:
                    // each world packet keeps its key unless a nearer-in-sequence packet has a larger
                    // one, in which case it is raised to that (never past its own farthest vertex).
                    // Everything else keeps the game's key. Draw order is key descending.
                    let mut seq: Vec<(u32, usize)> = Vec::new();
                    for (i, w) in frame.walk.iter().enumerate() {
                        if let Some(inf) = w.info {
                            let model = if inf.kind == log::kind::SUBMODEL {
                                inf.brush
                            } else {
                                info.model_of(u32::from(inf.face))
                            };
                            if model == 0
                                && !matches!(inf.kind, log::kind::MODEL | log::kind::BEAM)
                                && inf.policy & 0x2f == 0
                            {
                                seq.push((face_rank(usize::from(inf.face)), i));
                            }
                        }
                    }
                    seq.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
                    let mut newkey = vec![i64::MIN; frame.walk.len()];
                    let mut tie = vec![0i64; frame.walk.len()];
                    let mut run = 0i64;
                    for (pos, (_, i)) in seq.iter().enumerate() {
                        let k = frame.walk[*i].info.map_or(0, |inf| i64::from(inf.key));
                        let zmax = plane_depth_range(&frame, &frame.walk[*i], &ctx)
                            .map_or(k, |(_, far)| (far as i64) >> 2);
                        let nk = k.max(run.min(zmax));
                        newkey[*i] = nk;
                        tie[*i] = -(pos as i64);
                        run = run.max(nk);
                    }
                    keyed.clear();
                    for (i, w) in frame.walk.iter().enumerate() {
                        let key = if newkey[i] != i64::MIN {
                            newkey[i]
                        } else {
                            w.info.map_or(0, |inf| i64::from(inf.key))
                        };
                        keyed.push((
                            (639 - key.clamp(0, 639)) as u64,
                            if newkey[i] != i64::MIN {
                                (tie[i] + 1_000_000) as u64
                            } else {
                                2_000_000 + i as u64
                            },
                            0,
                            i,
                        ));
                    }
                }
            }
            keyed.sort();
            let mut sequenced = frame.clone();
            sequenced.walk = keyed
                .iter()
                .map(|(_, _, _, i)| frame.walk[*i].clone())
                .collect();
            let game = analyze(&frame, Tolerance::default(), &ctx, false).0;
            let png_dir = std::env::var_os("LAB_PNG_DIR");
            let (seq, seq_images) =
                analyze(&sequenced, Tolerance::default(), &ctx, png_dir.is_some());
            if let (Some(dir), Some(images)) = (png_dir, seq_images) {
                if seq.wrong_major >= 1500 {
                    let (w, h, rgb) = compose(&images, 1);
                    let _ = fs::create_dir_all(&dir);
                    write_png(
                        &Path::new(&dir).join(format!("{name}-{:x}.png", frame.index)),
                        w,
                        h,
                        &rgb,
                    )?;
                }
            }
            for (k, v) in &game.pairs {
                *pair_game.entry(*k).or_default() += v.major;
            }
            for (k, v) in &seq.pairs {
                *pair_seq.entry(*k).or_default() += v.major;
            }
            for (k, v) in &seq.face_pairs {
                *face_seq.entry(*k).or_default() += v;
            }
            totals[0] += game.covered;
            totals[1] += game.wrong_minor;
            totals[2] += game.wrong_major;
            totals[3] += seq.wrong_minor;
            totals[4] += seq.wrong_major;
            if verbose {
                println!(
                    "{name} frame {:x}: game wrong {} (major {}) sequenced {} (major {})",
                    frame.index,
                    game.wrong_minor,
                    game.wrong_major,
                    seq.wrong_minor,
                    seq.wrong_major
                );
            }
        }
        println!(
            "{name}: covered {} game wrong {} (major {}) sequenced {} (major {})",
            totals[0], totals[1], totals[2], totals[3], totals[4]
        );
        if std::env::var_os("LAB_PAIRS").is_some() {
            println!("   game pairs {pair_game:?}\n   seq pairs  {pair_seq:?}");
            let mut top: Vec<_> = face_seq.iter().collect();
            top.sort_by(|a, b| b.1.cmp(a.1));
            for ((pk, pf, vk, vf), px) in top.into_iter().take(6) {
                println!(
                    "   worst seq pair: painter kind {pk} face {pf} (model {}) over victim kind {vk} face {vf} (model {}): {px} px",
                    info.model_of(u32::from(*pf)),
                    info.model_of(u32::from(*vf))
                );
            }
        }
        for (g, t) in grand.iter_mut().zip(totals) {
            *g += t;
        }
    }
    println!(
        "total: covered {} game wrong {} (major {}) sequenced {} (major {})",
        grand[0], grand[1], grand[2], grand[3], grand[4]
    );
    Ok(())
}

/// Debug: for every brush model in a frame, solve the translation that puts its faces on the
/// depths the guest recorded, and print it next to the offset the guest reported.
fn fit_command() -> Result<(), String> {
    let mut it = std::env::args().skip(2);
    let dir = PathBuf::from(it.next().ok_or(USAGE)?);
    for path in it {
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        let name = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bsp = bsp::Bsp::load(&dir.join(format!("{}.bsp", map_of(&name))))?;
        for frame in log::parse(&text) {
            let mut eqs: std::collections::BTreeMap<u16, (Vec<([f64; 3], f64)>, [i16; 3])> =
                Default::default();
            for w in &frame.walk {
                let Some(inf) = w.info else { continue };
                if inf.kind != log::kind::SUBMODEL || inf.brush == 0 || !matches!(inf.mode, 1 | 2) {
                    continue;
                }
                let Some(poly) = raster::decode(&w.words) else {
                    continue;
                };
                let Some((n, d)) = bsp.face_plane_cooked(usize::from(inf.face), [0; 3]) else {
                    continue;
                };
                for k in 0..poly.n {
                    let z = f64::from(inf.z[k]);
                    let u = [
                        (f64::from(poly.verts[k].0) - 160.0) / frame.h,
                        (f64::from(poly.verts[k].1) - 120.0) / frame.h,
                        1.0,
                    ];
                    // camera point z*u -> world: P = eye + R^T (z u)
                    let mut p = [
                        f64::from(frame.eye[0]),
                        f64::from(frame.eye[1]),
                        f64::from(frame.eye[2]),
                    ];
                    for j in 0..3 {
                        for r in 0..3 {
                            p[j] += frame.rot[r][j] * z * u[r];
                        }
                    }
                    // n . (p - t) = d  ->  n . t = n . p - d
                    let rhs = n[0] * p[0] + n[1] * p[1] + n[2] * p[2] - d;
                    eqs.entry(inf.brush)
                        .or_insert_with(|| (Vec::new(), inf.offset))
                        .0
                        .push((n, rhs));
                }
            }
            // plane-vs-vertex agreement per model with the guest's offset
            let mut agree: std::collections::BTreeMap<u16, (u32, u32)> = Default::default();
            for w in &frame.walk {
                let Some(inf) = w.info else { continue };
                if inf.kind != log::kind::SUBMODEL || inf.brush == 0 || !matches!(inf.mode, 1 | 2) {
                    continue;
                }
                let Some(poly) = raster::decode(&w.words) else {
                    continue;
                };
                let off = [
                    i32::from(inf.offset[0]),
                    i32::from(inf.offset[1]),
                    i32::from(inf.offset[2]),
                ];
                let Some((n, d)) = bsp.face_plane_cooked(usize::from(inf.face), off) else {
                    continue;
                };
                let nc = [
                    frame.rot[0][0] * n[0] + frame.rot[0][1] * n[1] + frame.rot[0][2] * n[2],
                    frame.rot[1][0] * n[0] + frame.rot[1][1] * n[1] + frame.rot[1][2] * n[2],
                    frame.rot[2][0] * n[0] + frame.rot[2][1] * n[1] + frame.rot[2][2] * n[2],
                ];
                let dp = d
                    - (n[0] * f64::from(frame.eye[0])
                        + n[1] * f64::from(frame.eye[1])
                        + n[2] * f64::from(frame.eye[2]));
                let ok = (0..poly.n).all(|k| {
                    let u = [
                        (f64::from(poly.verts[k].0) - 160.0) / frame.h,
                        (f64::from(poly.verts[k].1) - 120.0) / frame.h,
                        1.0,
                    ];
                    let den = nc[0] * u[0] + nc[1] * u[1] + nc[2] * u[2];
                    let z = dp / den;
                    (z - f64::from(inf.z[k])).abs() <= 4.0 + 0.03 * f64::from(inf.z[k])
                });
                let e = agree.entry(inf.brush).or_default();
                e.0 += u32::from(ok);
                e.1 += 1;
            }
            println!(
                "frame {:x} plane agreement per brush model: {:?}",
                frame.index, agree
            );
            for (model, (rows, off)) in eqs {
                // normal equations A^T A t = A^T b
                let mut ata = [[0f64; 3]; 3];
                let mut atb = [0f64; 3];
                for (n, b) in &rows {
                    for i in 0..3 {
                        for j in 0..3 {
                            ata[i][j] += n[i] * n[j];
                        }
                        atb[i] += n[i] * b;
                    }
                }
                let det = ata[0][0] * (ata[1][1] * ata[2][2] - ata[1][2] * ata[2][1])
                    - ata[0][1] * (ata[1][0] * ata[2][2] - ata[1][2] * ata[2][0])
                    + ata[0][2] * (ata[1][0] * ata[2][1] - ata[1][1] * ata[2][0]);
                if det.abs() < 1e-6 {
                    println!(
                        "frame {:x} model {model}: degenerate ({} eq) off {:?}",
                        frame.index,
                        rows.len(),
                        off
                    );
                    continue;
                }
                let solve = |col: usize| {
                    let mut m = ata;
                    for i in 0..3 {
                        m[i][col] = atb[i];
                    }
                    (m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]))
                        / det
                };
                println!("frame {:x} model {model}: fitted t=({:.0},{:.0},{:.0}) from {} eq; guest offset {:?}", frame.index, solve(0), solve(1), solve(2), rows.len(), off);
            }
        }
    }
    Ok(())
}

fn main() {
    let command = std::env::args().nth(1);
    let result = if command.as_deref() == Some("poses") {
        poses_command()
    } else if command.as_deref() == Some("lab") {
        lab_command()
    } else if command.as_deref() == Some("fit") {
        fit_command()
    } else if command.as_deref() == Some("inspect") {
        // inspect LOG MAP FRAMEHEX FACE... : the packets of those source faces, in draw order
        let mut it = std::env::args().skip(2);
        let (log_path, map, frame_hex) = (
            it.next().unwrap_or_default(),
            it.next().unwrap_or_default(),
            it.next().unwrap_or_default(),
        );
        let faces: Vec<u16> = it.filter_map(|f| f.parse().ok()).collect();
        let text = fs::read_to_string(&log_path).unwrap_or_default();
        let mut frames = log::parse(&text);
        if let Some(dir) = std::env::var_os("FACE_MAP") {
            if let Ok(csv) = fs::read_to_string(Path::new(&dir).join(format!("{map}.csv"))) {
                log::remap_faces(&mut frames, &csv);
            }
        }
        for frame in frames
            .iter()
            .filter(|f| format!("{:x}", f.index) == frame_hex)
        {
            for (i, w) in frame.walk.iter().enumerate() {
                if let Some(inf) = w.info {
                    if faces.contains(&inf.face) {
                        let poly = raster::decode(&w.words);
                        println!("draw#{i} kind {} face {} key {} policy {:02x} mode {} z {:?} verts {:?}", inf.kind, inf.face, inf.key, inf.policy, inf.mode, inf.z, poly.map(|p| p.verts));
                    }
                }
            }
        }
        Ok(())
    } else if command.as_deref() == Some("leafrange") {
        // leafrange DIR MAP... : whether the leaves under every node are a contiguous run of numbers
        let mut it = std::env::args().skip(2);
        let dir = PathBuf::from(it.next().unwrap_or_default());
        for map in it {
            let Ok(bsp) = bsp::Bsp::load(&dir.join(format!("{map}.bsp"))) else {
                continue;
            };
            let n = bsp.nodes.len();
            let mut lo = vec![u32::MAX; n];
            let mut hi = vec![0u32; n];
            let mut cnt = vec![0u32; n];
            fn walk(b: &bsp::Bsp, i: usize, lo: &mut [u32], hi: &mut [u32], cnt: &mut [u32]) {
                if cnt[i] != 0 {
                    return;
                }
                for c in b.nodes[i].children {
                    if c < 0 {
                        let l = (-1 - c) as u32;
                        lo[i] = lo[i].min(l);
                        hi[i] = hi[i].max(l);
                        cnt[i] += 1;
                    } else {
                        walk(b, c as usize, lo, hi, cnt);
                        let c = c as usize;
                        lo[i] = lo[i].min(lo[c]);
                        hi[i] = hi[i].max(hi[c]);
                        cnt[i] += cnt[c];
                    }
                }
            }
            let root = bsp.models[0].head_node as usize;
            walk(&bsp, root, &mut lo, &mut hi, &mut cnt);
            let mut contiguous = 0;
            let mut total = 0;
            for i in 0..n {
                if cnt[i] == 0 {
                    continue;
                }
                total += 1;
                if hi[i] - lo[i] + 1 == cnt[i] {
                    contiguous += 1;
                }
            }
            println!("{map} nodes {total} contiguous {contiguous}");
        }
        Ok(())
    } else if command.as_deref() == Some("pvsmax") {
        let mut it = std::env::args().skip(2);
        let dir = PathBuf::from(it.next().unwrap_or_default());
        for map in it {
            match pvsmax::bound(&dir.join(format!("{map}.bsp"))) {
                Ok((l, f, n, tn, tl)) => println!("{map} max_visible_leaves {l} max_visible_faces {f} max_visible_nodes {n} nodes {tn} leaves {tl}"),
                Err(e) => eprintln!("{e}"),
            }
        }
        Ok(())
    } else {
        run()
    };
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    fs::create_dir_all(args.out.join("masks")).map_err(|e| e.to_string())?;
    let mut frames_csv = String::from(
        "log,frame,label,packets,unknown,approx,covered,wrong_minor,wrong_major,trans_wrong,tie,span,thin,inverted\n",
    );
    let mut pairs_csv =
        String::from("log,frame,painter,victim,minor,major,tie,span,thin,inverted\n");
    let mut faces_csv =
        String::from("log,frame,painter_kind,painter_face,victim_kind,victim_face,pixels\n");
    let mut brush_csv = String::from("log,model,entity,wrong_pixels\n");
    let mut total = FrameStats::default();
    for path in &args.logs {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let info = args.bsp_dir.as_ref().and_then(|dir| {
            poses::MapInfo::load(&dir.join(format!("{}.bsp", map_of(&name))))
                .map_err(|e| eprintln!("{e}"))
                .ok()
        });
        let class_of = |kind: u8, face: u16| -> Class {
            let model = info.as_ref().map_or(0, |i| i.model_of(u32::from(face)));
            if kind == log::kind::SUBMODEL || model != 0 {
                Class::Brush(model)
            } else {
                Class::World
            }
        };
        let mut brush_totals: std::collections::BTreeMap<u16, u64> = Default::default();
        let bsp = args
            .bsp_dir
            .as_ref()
            .and_then(|dir| bsp::Bsp::load(&dir.join(format!("{}.bsp", map_of(&name)))).ok());
        let plane_of = |face: u16, off: [i32; 3]| {
            bsp.as_ref()
                .and_then(|b| b.face_plane_cooked(usize::from(face), off))
        };
        let ctx = Context {
            class_of: &class_of,
            plane_of: &plane_of,
        };
        let mut frames = log::parse(&text);
        if let Some(dir) = &args.face_map {
            match fs::read_to_string(dir.join(format!("{}.csv", map_of(&name)))) {
                Ok(csv) => log::remap_faces(&mut frames, &csv),
                Err(e) => eprintln!("{name}: no face map ({e}); face numbers stay cooked"),
            }
        }
        for frame in &frames {
            if frame.reported as usize != frame.walk.len() {
                eprintln!(
                    "{name} frame {:x}: guest walked {} packets, parsed {}",
                    frame.index,
                    frame.reported,
                    frame.walk.len()
                );
            }
            let (stats, images) = analyze(frame, Tolerance::default(), &ctx, true);
            let [tie, span, thin, inverted] = stats.by_cause;
            let _ = writeln!(
                frames_csv,
                "{name},{:x},{},{},{},{},{},{},{},{},{tie},{span},{thin},{inverted}",
                stats.index,
                stats.label.replace(',', ";"),
                stats.packets,
                stats.unknown_packets,
                stats.approx_packets,
                stats.covered,
                stats.wrong_minor,
                stats.wrong_major,
                stats.translucent_wrong
            );
            for ((painter, victim), pair) in &stats.pairs {
                let [t, s, n, i] = pair.by_cause;
                let _ = writeln!(
                    pairs_csv,
                    "{name},{:x},{painter},{victim},{},{},{t},{s},{n},{i}",
                    stats.index, pair.minor, pair.major
                );
            }
            let mut top: Vec<_> = stats.face_pairs.iter().collect();
            top.sort_by(|a, b| b.1.cmp(a.1));
            for ((pk, pf, vk, vf), px) in top.into_iter().take(8) {
                let _ = writeln!(
                    faces_csv,
                    "{name},{:x},{pk},{pf},{vk},{vf},{px}",
                    stats.index
                );
            }
            if let Some(images) = images {
                if args.png_all || stats.wrong_major >= args.png_major {
                    let (w, h, rgb) = compose(&images, 1);
                    write_png(
                        &args
                            .out
                            .join("masks")
                            .join(format!("{name}-{:x}.png", stats.index)),
                        w,
                        h,
                        &rgb,
                    )?;
                }
            }
            for (model, px) in &stats.brush {
                *brush_totals.entry(*model).or_default() += px;
            }
            total.packets += stats.packets;
            total.covered += stats.covered;
            total.wrong_minor += stats.wrong_minor;
            total.wrong_major += stats.wrong_major;
            total.translucent_wrong += stats.translucent_wrong;
            total.unknown_packets += stats.unknown_packets;
            total.approx_packets += stats.approx_packets;
            total.plane_used += stats.plane_used;
            total.plane_disagree += stats.plane_disagree;
            for (a, b) in total.by_cause.iter_mut().zip(stats.by_cause) {
                *a += b;
            }
        }
        for (model, px) in &brush_totals {
            let entity = info
                .as_ref()
                .and_then(|i| i.names.get(usize::from(*model)))
                .map_or("", String::as_str);
            let _ = writeln!(brush_csv, "{name},{model},{entity},{px}");
        }
        println!("{name}: {} frames", frames.len());
    }
    fs::write(args.out.join("frames.csv"), frames_csv).map_err(|e| e.to_string())?;
    fs::write(args.out.join("pairs.csv"), pairs_csv).map_err(|e| e.to_string())?;
    fs::write(args.out.join("faces.csv"), faces_csv).map_err(|e| e.to_string())?;
    fs::write(args.out.join("brush.csv"), brush_csv).map_err(|e| e.to_string())?;
    let pct = |n: u64| 100.0 * n as f64 / total.covered.max(1) as f64;
    println!(
        "total: {} packets, {} covered px, wrong {} ({:.3}%), major {} ({:.3}%), translucent {} ({:.3}%)",
        total.packets,
        total.covered,
        total.wrong_minor,
        pct(total.wrong_minor),
        total.wrong_major,
        pct(total.wrong_major),
        total.translucent_wrong,
        pct(total.translucent_wrong)
    );
    println!(
        "depth source: plane {} packets, plane disagreed with vertex depth {}, flat-key fallback {}, no depth {}",
        total.plane_used, total.plane_disagree, total.approx_packets, total.unknown_packets
    );
    for (cause, n) in CAUSES.iter().zip(total.by_cause) {
        println!("  {:9} {:>9} px ({:.3}%)", cause.name(), n, pct(n));
    }
    Ok(())
}
