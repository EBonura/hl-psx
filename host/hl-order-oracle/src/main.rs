//! Ordering oracle for hl-psx: compares a frame's GPU draw order with a depth-tested render of
//! the identical packets. See `game/src/order_trace.rs` for the stream it reads.

mod analyze;
mod bsp;
mod log;
mod poses;
mod raster;

use analyze::{analyze, Class, FrameStats, Tolerance, CAUSES};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: hl-order-oracle analyze --out DIR [--bsp-dir DIR] [--png-all] [--png-major N] LOG...\n       hl-order-oracle poses BSP_DIR MAP [MAP...] [--spots N] [--yaws A,B,C]";

struct Args {
    out: PathBuf,
    bsp_dir: Option<PathBuf>,
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
    let mut png_all = false;
    let mut png_major = 400;
    let mut logs = Vec::new();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" => out = it.next().map(PathBuf::from),
            "--bsp-dir" => bsp_dir = it.next().map(PathBuf::from),
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
    for (pane, src) in [&images.painter, &images.depth, &images.mask].into_iter().enumerate() {
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
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--spots" => spots = it.next().and_then(|v| v.parse().ok()).ok_or("--spots needs a number")?,
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
        let chosen = poses::pick_spots(&entities, spots);
        print!("{}", poses::section(&map, &chosen, &yaws, 12));
    }
    Ok(())
}

/// Offline experiment: replay the world packets of traced frames in the exact BSP back-to-front
/// order and report the wrong-order pixels of that order next to the order the game used.
fn lab_command() -> Result<(), String> {
    let mut it = std::env::args().skip(2);
    let dir = PathBuf::from(it.next().ok_or(USAGE)?);
    let mut grand = [0u64; 3];
    for path in it {
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        let name = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let map = map_of(&name).to_string();
        let bsp = bsp::Bsp::load(&dir.join(format!("{map}.bsp")))?;
        let info = poses::MapInfo::load(&dir.join(format!("{map}.bsp")))?;
        let class_of = |_kind: u8, _face: u16| Class::World;
        for frame in log::parse(&text) {
            let world = |w: &log::Walked| {
                w.info.is_some_and(|i| {
                    !matches!(i.kind, log::kind::SUBMODEL | log::kind::MODEL | log::kind::BEAM)
                        && info.model_of(u32::from(i.face)) == 0
                        && (i.face as usize) < bsp.node_of_face.len()
                })
            };
            let eye = [frame.eye[0] as f32, frame.eye[2] as f32, frame.eye[1] as f32];
            let rank = bsp.back_to_front_ranks(0, eye);
            let mut base = frame.clone();
            base.walk.retain(|w| world(w));
            let mut ordered = base.clone();
            let key = |w: &log::Walked| {
                let node = w.info.map_or(u32::MAX, |i| bsp.node_of_face[i.face as usize]);
                if node == u32::MAX { u32::MAX } else { rank[node as usize] }
            };
            let mut keyed: Vec<(u32, usize)> = ordered.walk.iter().enumerate().map(|(i, w)| (key(w), i)).collect();
            keyed.sort();
            ordered.walk = keyed.iter().map(|(_, i)| base.walk[*i].clone()).collect();
            let (a, _) = analyze(&base, Tolerance::default(), &class_of, false);
            let (b, _) = analyze(&ordered, Tolerance::default(), &class_of, false);
            println!(
                "{name} frame {:x} {}: world packets {} covered {}  game order wrong {} (major {})  bsp order wrong {} (major {})",
                frame.index, frame.label, base.walk.len(), a.covered, a.wrong_minor, a.wrong_major, b.wrong_minor, b.wrong_major
            );
            grand[0] += a.covered;
            grand[1] += a.wrong_minor;
            grand[2] += b.wrong_minor;
        }
    }
    println!("total covered {} game-order wrong {} bsp-order wrong {}", grand[0], grand[1], grand[2]);
    Ok(())
}

fn main() {
    let command = std::env::args().nth(1);
    let result = if command.as_deref() == Some("poses") {
        poses_command()
    } else if command.as_deref() == Some("lab") {
        lab_command()
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
    let mut faces_csv = String::from("log,frame,painter_kind,painter_face,victim_kind,victim_face,pixels\n");
    let mut brush_csv = String::from("log,model,entity,wrong_pixels\n");
    let mut total = FrameStats::default();
    for path in &args.logs {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let info = args
            .bsp_dir
            .as_ref()
            .and_then(|dir| poses::MapInfo::load(&dir.join(format!("{}.bsp", map_of(&name))))
                .map_err(|e| eprintln!("{e}"))
                .ok());
        let class_of = |kind: u8, face: u16| -> Class {
            let model = info.as_ref().map_or(0, |i| i.model_of(u32::from(face)));
            if kind == log::kind::SUBMODEL || model != 0 {
                Class::Brush(model)
            } else {
                Class::World
            }
        };
        let mut brush_totals: std::collections::BTreeMap<u16, u64> = Default::default();
        let frames = log::parse(&text);
        for frame in &frames {
            if frame.reported as usize != frame.walk.len() {
                eprintln!(
                    "{name} frame {:x}: guest walked {} packets, parsed {}",
                    frame.index,
                    frame.reported,
                    frame.walk.len()
                );
            }
            let (stats, images) = analyze(frame, Tolerance::default(), &class_of, true);
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
                let _ = writeln!(faces_csv, "{name},{:x},{pk},{pf},{vk},{vf},{px}", stats.index);
            }
            if let Some(images) = images {
                if args.png_all || stats.wrong_major >= args.png_major {
                    let (w, h, rgb) = compose(&images, 1);
                    write_png(
                        &args.out.join("masks").join(format!("{name}-{:x}.png", stats.index)),
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
    for (cause, n) in CAUSES.iter().zip(total.by_cause) {
        println!("  {:9} {:>9} px ({:.3}%)", cause.name(), n, pct(n));
    }
    Ok(())
}
