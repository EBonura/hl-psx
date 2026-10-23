//! Ordering oracle for hl-psx: compares a frame's GPU draw order with a depth-tested render of
//! the identical packets. See `game/src/order_trace.rs` for the stream it reads.

mod analyze;
mod bsp;
mod log;
mod poses;
mod pvsmax;
mod raster;

use analyze::{analyze, Class, Context, FrameStats, Tolerance, CAUSES};
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
                if matches!(inf.kind, log::kind::MODEL | log::kind::BEAM) {
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
                    let anchor = if leaf == 0 || lrank[leaf] == u32::MAX {
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
                        let ranks = brush_ranks
                            .entry(model)
                            .or_insert_with(|| bsp.back_to_front_ranks(usize::from(model), local));
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
                    keyed.push((u64::from(anchor) * 2 + 1, sub, u64::from(model), i));
                }
            }
            keyed.sort();
            let mut sequenced = frame.clone();
            sequenced.walk = keyed
                .iter()
                .map(|(_, _, _, i)| frame.walk[*i].clone())
                .collect();
            let game = analyze(&frame, Tolerance::default(), &ctx, false).0;
            let seq = analyze(&sequenced, Tolerance::default(), &ctx, false).0;
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

fn main() {
    let command = std::env::args().nth(1);
    let result = if command.as_deref() == Some("poses") {
        poses_command()
    } else if command.as_deref() == Some("lab") {
        lab_command()
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
