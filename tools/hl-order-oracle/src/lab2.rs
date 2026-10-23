//! Second lab: replays traced frames with a per-pixel painter key, which lets an experiment ask
//! "what if every piece of a brush face were ordered on its own" without cutting the packets.
//!
//! World packets draw at the slot of the farthest visible leaf that marks their face (exact for
//! world geometry). Every other fragment is ordered by the open leaf that holds the 3D point it
//! shows (strategy `frag`), the leaf holding the face centroid (`cent`), or by the leaf of the
//! packet's centroid (`pkt`). Inside a leaf, brush and model pieces draw after the leaf's world
//! faces, ordered by the game's own depth key.

use crate::analyze::{analyze, Class, Context, FrameStats, Tolerance};
use crate::bsp::Bsp;
use crate::log::{self, kind, Info};
use crate::poses::MapInfo;
use crate::raster;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn map_of(stem: &str) -> &str {
    stem.split('-').next().unwrap_or(stem)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Anchor {
    /// The leaf that holds the 3D point of every pixel (an ideal split of the face).
    Frag,
    /// The leaf that holds the face centroid.
    Cent,
    /// The leaf that holds the centroid of the traced packet.
    Pkt,
    /// Farthest / nearest slot among the packet's vertices and centroid.
    PktMin,
    PktMax,
    /// Screen centroid at the mean vertex depth (what the guest can compute from a packet).
    PktZ,
    /// Last of all (the game's order for the class is not modelled).
    Last,
    /// The leaf that holds the centre of the entity's cooked bounds at its draw offset.
    Ent,
    /// The farthest / nearest slot (in the sequence) among the slots of every vertex of the entity.
    EntMin,
    EntMax,
    /// A mover whose vertices all sit in one slot keys there; one that spans several draws with
    /// the world faces of that slot range, all ordered by the game's own depth key.
    Fallback,
    /// The leaf that holds the centre of the grid cell (L2_CELL units, entity-local) the pixel's
    /// point falls in, projected onto the face plane: faces cooked as cells of that size.
    Cell,
}

fn anchor_of(var: &str, default: Anchor) -> Anchor {
    match std::env::var(var).as_deref() {
        Ok("frag") => Anchor::Frag,
        Ok("cent") => Anchor::Cent,
        Ok("pkt") => Anchor::Pkt,
        Ok("pktz") => Anchor::PktZ,
        Ok("pktmin") => Anchor::PktMin,
        Ok("pktmax") => Anchor::PktMax,
        Ok("last") => Anchor::Last,
        Ok("ent") => Anchor::Ent,
        Ok("cell") => Anchor::Cell,
        Ok("fallback") => Anchor::Fallback,
        Ok("entmin") => Anchor::EntMin,
        Ok("entmax") => Anchor::EntMax,
        _ => default,
    }
}

/// Whether an entity of this `classname targetname` never moves on its own.
pub(crate) fn is_static(name: &str) -> bool {
    let class = name.split(' ').next().unwrap_or("");
    matches!(
        class,
        "func_wall"
            | "func_illusionary"
            | "func_breakable"
            | "func_wall_toggle"
            | "func_healthcharger"
            | "func_recharge"
            | "func_monsterclip"
            | "func_clip"
    )
}

type Pairs = BTreeMap<(&'static str, &'static str), u64>;

fn add_pairs(into: &mut Pairs, stats: &FrameStats) {
    for (k, v) in &stats.pairs {
        *into.entry(*k).or_default() += v.major;
    }
}

fn print_pairs(label: &str, pairs: &Pairs) {
    let total: u64 = pairs.values().sum();
    let mut v: Vec<_> = pairs.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    let body: Vec<String> = v
        .iter()
        .take(9)
        .map(|((a, b), n)| format!("{a}/{b} {n}"))
        .collect();
    println!("  {label:>6} {total:>8}: {}", body.join(", "));
}

pub fn lab2_command() -> Result<(), String> {
    let mut it = std::env::args().skip(2);
    let dir = PathBuf::from(it.next().ok_or("lab2 BSP_DIR LOG...")?);
    let brush_anchor = anchor_of("L2_BRUSH", Anchor::Frag);
    let model_anchor = anchor_of("L2_MODEL", Anchor::Last);
    let mover_anchor = anchor_of("L2_MOVER", brush_anchor);
    let verbose = std::env::var_os("L2_VERBOSE").is_some();
    let png_dir = std::env::var_os("L2_PNG_DIR");
    let pair_set: std::collections::HashSet<(String, u32)> = std::env::var("L2_PAIR")
        .unwrap_or_default()
        .split(',')
        .filter_map(|e| {
            let (m, f) = e.split_once(':')?;
            Some((m.to_string(), u32::from_str_radix(f, 16).ok()?))
        })
        .collect();
    let mut grand_game = Pairs::new();
    let mut grand_lab = Pairs::new();
    let mut covered = 0u64;
    let mut by_entity: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut dstat = (0u64, 0u64, 0u64, 0u64);
    let mut hyb_frames = 0u64;
    let mut hyb_legacy_frames = 0u64;
    let mut self_ent: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for path in it {
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        let name = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let map = map_of(&name).to_string();
        let bsp = Bsp::load(&dir.join(format!("{map}.bsp")))?;
        let info = MapInfo::load(&dir.join(format!("{map}.bsp")))?;
        let plane_of = |face: u16, off: [i32; 3]| bsp.face_plane_cooked(usize::from(face), off);
        let model_of = |inf: &Info| -> u16 {
            if inf.kind == kind::SUBMODEL {
                inf.brush
            } else {
                info.model_of(u32::from(inf.face))
            }
        };
        let class_of = |k: u8, face: u16| {
            if k == kind::SUBMODEL || info.model_of(u32::from(face)) != 0 {
                Class::Brush(info.model_of(u32::from(face)))
            } else {
                Class::World
            }
        };
        let mut parsed = log::parse(&text);
        if let Some(face_dir) = std::env::var_os("FACE_MAP") {
            if let Ok(csv) = fs::read_to_string(Path::new(&face_dir).join(format!("{map}.csv"))) {
                log::remap_faces(&mut parsed, &csv);
            }
        }
        let mut pg = Pairs::new();
        let mut pl = Pairs::new();
        let mut face_pairs: BTreeMap<(u8, u16, u8, u16), u64> = BTreeMap::new();
        for frame in parsed {
            let eye = [
                frame.eye[0] as f32,
                frame.eye[2] as f32,
                frame.eye[1] as f32,
            ];
            let (slot_rank, leaf_slot) = bsp.slot_ranks(eye);
            let node_mode = std::env::var_os("L2_NODE").is_some();
            let (slot_rank, node_rank) = if node_mode {
                let (a, b) = bsp.event_ranks(eye);
                (a, b)
            } else {
                (slot_rank, Vec::new())
            };
            let face_rank = |face: usize| -> u64 {
                if node_mode {
                    if let Some(&nd) = bsp.node_of_face.get(face) {
                        if nd != u32::MAX {
                            return u64::from(node_rank[nd as usize]);
                        }
                    }
                    return 1 << 30;
                }
                bsp.face_leaves
                    .get(face)
                    .into_iter()
                    .flatten()
                    .filter_map(|leaf| {
                        let s = leaf_slot[*leaf as usize];
                        (s != u32::MAX).then(|| slot_rank[s as usize])
                    })
                    .filter(|r| *r != u32::MAX)
                    .min()
                    .map_or(1 << 30, u64::from)
            };
            // Group strategy: a world face keys at the farthest slot among all faces of its plane
            // that the frame draws.
            let group_min: std::collections::HashMap<(usize, bool), u64> =
                if std::env::var_os("L2_GROUP").is_some() {
                    let mut g: std::collections::HashMap<(usize, bool), u64> = Default::default();
                    for w in &frame.walk {
                        let Some(inf) = w.info else { continue };
                        if matches!(inf.kind, kind::MODEL | kind::BEAM) || model_of(&inf) != 0 {
                            continue;
                        }
                        if let Some(&pl) = bsp.face_plane.get(usize::from(inf.face)) {
                            let r = face_rank(usize::from(inf.face));
                            let e = g.entry(pl).or_insert(u64::MAX);
                            *e = (*e).min(r);
                        }
                    }
                    g
                } else {
                    Default::default()
                };
            let face_rank = |face: usize| -> u64 {
                let r = face_rank(face);
                if group_min.is_empty() {
                    return r;
                }
                bsp.face_plane
                    .get(face)
                    .and_then(|pl| group_min.get(pl))
                    .copied()
                    .unwrap_or(r)
                    .min(r)
            };
            // Cooked point of a pixel at depth `z`, in the BSP's axes.
            let unproject = |x: f64, y: f64, z: f64| -> [f32; 3] {
                let u = [(x - 160.0) / frame.h * z, (y - 120.0) / frame.h * z, z];
                let mut w = [
                    f64::from(frame.eye[0]),
                    f64::from(frame.eye[1]),
                    f64::from(frame.eye[2]),
                ];
                for (j, wj) in w.iter_mut().enumerate() {
                    for (r, ur) in u.iter().enumerate() {
                        *wj += frame.rot[r][j] * ur;
                    }
                }
                [w[0] as f32, w[2] as f32, w[1] as f32]
            };
            let rank_at = |p: [f32; 3]| -> u64 {
                let r = slot_rank[bsp.point_slot(p)];
                if r == u32::MAX {
                    1 << 30
                } else {
                    u64::from(r)
                }
            };
            // Slot span of every non-static entity (vertex slots), for the fallback strategy.
            let mut ent_span: std::collections::HashMap<(u16, [i16; 3]), (u64, u64)> =
                Default::default();
            let mut fb_ranges: Vec<(u64, u64)> = Vec::new();
            if mover_anchor == Anchor::Fallback {
                for w in &frame.walk {
                    let Some(inf) = w.info else { continue };
                    if matches!(inf.kind, kind::MODEL | kind::BEAM) {
                        continue;
                    }
                    let model = model_of(&inf);
                    if model == 0 || is_static(&info.names[usize::from(model)]) {
                        continue;
                    }
                    if ent_span.contains_key(&(model, inf.offset)) {
                        continue;
                    }
                    let off = [
                        f32::from(inf.offset[0]),
                        f32::from(inf.offset[2]),
                        f32::from(inf.offset[1]),
                    ];
                    let (first, num) = info.models[usize::from(model)];
                    let (mut lo, mut hi) = (u64::MAX, 0u64);
                    for f in first..first + num {
                        let nrm = bsp.face_normal(f as usize);
                        for v in &bsp.faces[f as usize] {
                            let r = rank_at([
                                v[0] + off[0] + 4.0 * nrm[0],
                                v[1] + off[1] + 4.0 * nrm[1],
                                v[2] + off[2] + 4.0 * nrm[2],
                            ]);
                            lo = lo.min(r);
                            hi = hi.max(r);
                        }
                    }
                    ent_span.insert((model, inf.offset), (lo, hi));
                    if lo < hi {
                        fb_ranges.push((lo, hi));
                    }
                }
                fb_ranges.sort();
                let mut merged: Vec<(u64, u64)> = Vec::new();
                for (a, b) in fb_ranges.drain(..) {
                    match merged.last_mut() {
                        Some(last) if a <= last.1 => last.1 = last.1.max(b),
                        _ => merged.push((a, b)),
                    }
                }
                fb_ranges = merged;
            }
            let in_range = |r: u64| {
                fb_ranges
                    .iter()
                    .find(|(a, b)| r >= *a && r <= *b)
                    .map(|x| x.0)
            };
            // (game key, rank) of every world packet: where a game-keyed entity would sit.
            let key_to_rank: Vec<(u16, u64)> = frame
                .walk
                .iter()
                .filter_map(|w| {
                    let inf = w.info?;
                    (model_of(&inf) == 0 && !matches!(inf.kind, kind::MODEL | kind::BEAM))
                        .then(|| (inf.key, face_rank(usize::from(inf.face))))
                })
                .filter(|(_, r)| *r < (1 << 29))
                .collect();
            let water_legacy = std::env::var_os("L2_WATER_LEGACY").is_some();
            // Per packet anchors that do not depend on the pixel.
            let mut packet_anchor: Vec<Option<u64>> = vec![None; frame.walk.len()];
            let mut pkt_rank: Vec<Option<u64>> = vec![None; frame.walk.len()];
            let diff = std::cell::Cell::new((0u64, 0u64, 0u64, 0u64));
            for (i, w) in frame.walk.iter().enumerate() {
                let Some(inf) = w.info else { continue };
                if inf.kind == kind::MODEL && model_anchor == Anchor::Pkt {
                    if let Some(poly) = raster::decode(&w.words) {
                        if poly.n >= 3 {
                            let mut c = [0f32; 3];
                            for k in 0..poly.n {
                                let z = f64::from(inf.z[k].max(1));
                                let q = unproject(
                                    f64::from(poly.verts[k].0),
                                    f64::from(poly.verts[k].1),
                                    z,
                                );
                                for a in 0..3 {
                                    c[a] += q[a] / poly.n as f32;
                                }
                            }
                            packet_anchor[i] = Some(rank_at(c));
                        }
                    }
                    continue;
                }
                if matches!(inf.kind, kind::MODEL | kind::BEAM) {
                    continue;
                }
                if model_of(&inf) == 0 {
                    continue;
                }
                let off = [
                    f32::from(inf.offset[0]),
                    f32::from(inf.offset[2]),
                    f32::from(inf.offset[1]),
                ];
                let normal = bsp.face_normal(usize::from(inf.face));
                let anchor = if is_static(&info.names[usize::from(model_of(&inf))]) {
                    brush_anchor
                } else {
                    mover_anchor
                };
                if std::env::var_os("L2_DIFFSTAT").is_some() {
                    if let Some(poly) = raster::decode(&w.words) {
                        if poly.n >= 3 {
                            let mut c = [0f32; 3];
                            for k in 0..poly.n {
                                let z = f64::from(inf.z[k].max(1));
                                let q = unproject(
                                    f64::from(poly.verts[k].0),
                                    f64::from(poly.verts[k].1),
                                    z,
                                );
                                for a in 0..3 {
                                    c[a] += q[a] / poly.n as f32;
                                }
                            }
                            let p = [
                                c[0] + 4.0 * normal[0],
                                c[1] + 4.0 * normal[1],
                                c[2] + 4.0 * normal[2],
                            ];
                            pkt_rank[i] = Some(rank_at(p));
                        }
                    }
                }
                match anchor {
                    Anchor::EntMin | Anchor::EntMax => {
                        let mdl = usize::from(model_of(&inf));
                        let (first, num) = info.models[mdl];
                        let mut best: Option<u64> = None;
                        for f in first..first + num {
                            let poly = &bsp.faces[f as usize];
                            let nrm = bsp.face_normal(f as usize);
                            for v in poly {
                                let q = [
                                    v[0] + off[0] + 4.0 * nrm[0],
                                    v[1] + off[1] + 4.0 * nrm[1],
                                    v[2] + off[2] + 4.0 * nrm[2],
                                ];
                                let r = rank_at(q);
                                best = Some(match (best, anchor) {
                                    (None, _) => r,
                                    (Some(b), Anchor::EntMin) => b.min(r),
                                    (Some(b), _) => b.max(r),
                                });
                            }
                        }
                        packet_anchor[i] = best;
                    }
                    Anchor::Ent => {
                        let m = &bsp.models[usize::from(model_of(&inf))];
                        let p = [
                            m.center[0] + off[0],
                            m.center[1] + off[1],
                            m.center[2] + off[2],
                        ];
                        packet_anchor[i] = Some(rank_at(p));
                    }
                    Anchor::Cent => {
                        let poly = &bsp.faces[usize::from(inf.face)];
                        let n = poly.len().max(1) as f32;
                        let mut c = [0f32; 3];
                        for v in poly {
                            for k in 0..3 {
                                c[k] += v[k] / n;
                            }
                        }
                        let p = [
                            c[0] + off[0] + 4.0 * normal[0],
                            c[1] + off[1] + 4.0 * normal[1],
                            c[2] + off[2] + 4.0 * normal[2],
                        ];
                        packet_anchor[i] = Some(rank_at(p));
                    }
                    Anchor::PktZ => {
                        if let Some(poly) = raster::decode(&w.words) {
                            if poly.n >= 3 {
                                let (mut sx, mut sy, mut sz) = (0f64, 0f64, 0f64);
                                for k in 0..poly.n {
                                    sx += f64::from(poly.verts[k].0) / poly.n as f64;
                                    sy += f64::from(poly.verts[k].1) / poly.n as f64;
                                    sz += f64::from(inf.z[k].max(1)) / poly.n as f64;
                                }
                                let c = unproject(sx, sy, sz);
                                let p = [
                                    c[0] + 4.0 * normal[0],
                                    c[1] + 4.0 * normal[1],
                                    c[2] + 4.0 * normal[2],
                                ];
                                packet_anchor[i] = Some(rank_at(p));
                            }
                        }
                    }
                    Anchor::PktMin | Anchor::PktMax => {
                        if let Some(poly) = raster::decode(&w.words) {
                            if poly.n >= 3 {
                                let mut pts: Vec<[f32; 3]> = Vec::new();
                                let mut c = [0f32; 3];
                                for k in 0..poly.n {
                                    let z = f64::from(inf.z[k].max(1));
                                    let q = unproject(
                                        f64::from(poly.verts[k].0),
                                        f64::from(poly.verts[k].1),
                                        z,
                                    );
                                    pts.push(q);
                                    for a in 0..3 {
                                        c[a] += q[a] / poly.n as f32;
                                    }
                                }
                                pts.push(c);
                                let ranks: Vec<u64> = pts
                                    .iter()
                                    .map(|q| {
                                        rank_at([
                                            q[0] + 4.0 * normal[0],
                                            q[1] + 4.0 * normal[1],
                                            q[2] + 4.0 * normal[2],
                                        ])
                                    })
                                    .collect();
                                packet_anchor[i] = if anchor == Anchor::PktMin {
                                    ranks.iter().copied().min()
                                } else {
                                    ranks.iter().copied().max()
                                };
                            }
                        }
                    }
                    Anchor::Pkt => {
                        if let Some(poly) = raster::decode(&w.words) {
                            if poly.n >= 3 {
                                let mut c = [0f32; 3];
                                for k in 0..poly.n {
                                    let z = f64::from(inf.z[k].max(1));
                                    let q = unproject(
                                        f64::from(poly.verts[k].0),
                                        f64::from(poly.verts[k].1),
                                        z,
                                    );
                                    for a in 0..3 {
                                        c[a] += q[a] / poly.n as f32;
                                    }
                                }
                                let p = [
                                    c[0] + 4.0 * normal[0],
                                    c[1] + 4.0 * normal[1],
                                    c[2] + 4.0 * normal[2],
                                ];
                                packet_anchor[i] = Some(rank_at(p));
                            }
                        }
                    }
                    _ => {}
                }
            }
            let debug_px: Option<(u32, i32, i32)> =
                std::env::var("L2_DEBUG_PIX").ok().and_then(|v| {
                    let f: Vec<&str> = v.split(',').collect();
                    Some((
                        u32::from_str_radix(f.first()?, 16).ok()?,
                        f.get(1)?.parse().ok()?,
                        f.get(2)?.parse().ok()?,
                    ))
                });
            let legacy = std::env::var("L2_LEGACY").ok();
            let pixel_key = |idx: usize, inf: &Info, x: i32, y: i32, z: f32| -> u64 {
                const SUB: u64 = 10;
                if let Some(mode) = legacy.as_deref() {
                    // Everything keeps the game's depth space; brush packets may re-key.
                    let gk = u64::from(inf.key.min(639));
                    let n = usize::from(inf.n.max(3)).min(4);
                    let zs: Vec<u64> = inf.z[..n].iter().map(|v| u64::from(*v)).collect();
                    let zmin = zs.iter().copied().min().unwrap_or(0);
                    let zmax = zs.iter().copied().max().unwrap_or(0);
                    let zmean = zs.iter().sum::<u64>() / n as u64;
                    let is_brush =
                        inf.kind != kind::MODEL && inf.kind != kind::BEAM && model_of(inf) != 0;
                    let k = if is_brush && matches!(inf.mode, 1 | 2) {
                        match mode {
                            "min" => (zmin / 4).min(639),
                            "mean" => (zmean / 4).min(639),
                            "max" => (zmax / 4).min(639),
                            "minb" => (zmin / 4).saturating_sub(2).min(639),
                            "meanb" => (zmean / 4).saturating_sub(3).min(639),
                            _ => gk,
                        }
                    } else {
                        gk
                    };
                    return (639 - k) << SUB;
                }
                if debug_px == Some((frame.index, x, y)) {
                    let pp = unproject(f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z));
                    let nrm = bsp.face_normal(usize::from(inf.face));
                    let q = [
                        pp[0] + 4.0 * nrm[0],
                        pp[1] + 4.0 * nrm[1],
                        pp[2] + 4.0 * nrm[2],
                    ];
                    println!(
                        "DBG walk {idx} kind {} face {} model {} key {} z {z:.1} worldrank {} P {pp:?} leaf_here {} rank_open {}",
                        inf.kind, inf.face, model_of(inf), inf.key, face_rank(usize::from(inf.face)),
                        bsp.point_slot(pp), rank_at(q)
                    );
                }
                let gkey = 639 - u64::from(inf.key.min(639));
                if matches!(inf.kind, kind::BEAM) {
                    return (1 << 41) | gkey;
                }
                if inf.kind == kind::MODEL {
                    return match model_anchor {
                        Anchor::Frag => {
                            let p = unproject(f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z));
                            ((rank_at(p) * 2 + 1) << SUB) | gkey
                        }
                        Anchor::Pkt => {
                            ((packet_anchor[idx].unwrap_or(1 << 30) * 2 + 1) << SUB) | gkey
                        }
                        _ => (1 << 41) | gkey,
                    };
                }
                let model = model_of(inf);
                if model == 0 {
                    let fr = face_rank(usize::from(inf.face));
                    if let Some(group) = in_range(fr) {
                        return ((group * 2) << SUB) | gkey;
                    }
                    return (fr * 2) << SUB;
                }
                let anchor = if is_static(&info.names[usize::from(model)]) {
                    brush_anchor
                } else {
                    mover_anchor
                };
                if anchor == Anchor::Fallback {
                    if let Some(&(lo, hi)) = ent_span.get(&(model, inf.offset)) {
                        if lo < hi {
                            return ((lo * 2) << SUB) | gkey;
                        }
                        return ((lo * 2 + 1) << SUB) | gkey;
                    }
                }
                if water_legacy && info.names[usize::from(model)].starts_with("func_water") {
                    let r = key_to_rank
                        .iter()
                        .min_by_key(|(k, _)| (i32::from(*k) - i32::from(inf.key)).abs())
                        .map_or(0, |(_, r)| *r);
                    return ((r * 2 + 1) << SUB) | gkey;
                }
                let rank = match anchor {
                    Anchor::Frag => {
                        let n = bsp.face_normal(usize::from(inf.face));
                        let p = unproject(f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z));
                        let r = rank_at([p[0] + 4.0 * n[0], p[1] + 4.0 * n[1], p[2] + 4.0 * n[2]]);
                        if let Some(pr) = pkt_rank[idx] {
                            let mut d = diff.get();
                            let st = is_static(&info.names[usize::from(model)]);
                            if st {
                                d.0 += 1;
                                d.1 += u64::from(pr != r);
                            } else {
                                d.2 += 1;
                                d.3 += u64::from(pr != r);
                            }
                            diff.set(d);
                        }
                        r
                    }
                    Anchor::Cell => {
                        let n = bsp.face_normal(usize::from(inf.face));
                        let off = [
                            f32::from(inf.offset[0]),
                            f32::from(inf.offset[2]),
                            f32::from(inf.offset[1]),
                        ];
                        let p = unproject(f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z));
                        let local = [p[0] - off[0], p[1] - off[1], p[2] - off[2]];
                        let cell: f32 = std::env::var("L2_CELL")
                            .ok()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(128.0);
                        let c = [
                            ((local[0] / cell).floor() + 0.5) * cell,
                            ((local[1] / cell).floor() + 0.5) * cell,
                            ((local[2] / cell).floor() + 0.5) * cell,
                        ];
                        // project onto the plane through `local` with normal n
                        let dd = (c[0] - local[0]) * n[0]
                            + (c[1] - local[1]) * n[1]
                            + (c[2] - local[2]) * n[2];
                        let q = [c[0] - dd * n[0], c[1] - dd * n[1], c[2] - dd * n[2]];
                        rank_at([
                            q[0] + off[0] + 4.0 * n[0],
                            q[1] + off[1] + 4.0 * n[1],
                            q[2] + off[2] + 4.0 * n[2],
                        ])
                    }
                    Anchor::Last => 1 << 30,
                    _ => packet_anchor[idx].unwrap_or(1 << 30),
                };
                if let Some(group) = in_range(rank) {
                    return ((group * 2) << SUB) | gkey;
                }
                ((rank * 2 + 1) << SUB) | gkey
            };
            let game_ctx = Context {
                class_of: &class_of,
                plane_of: &plane_of,
                pixel_key: None,
            };
            let lab_ctx = Context {
                class_of: &class_of,
                plane_of: &plane_of,
                pixel_key: Some(&pixel_key),
            };
            let want_pair = pair_set.contains(&(name.clone(), frame.index));
            let (game, game_images) = analyze(&frame, Tolerance::default(), &game_ctx, want_pair);
            let (mut lab, images) = analyze(
                &frame,
                Tolerance::default(),
                &lab_ctx,
                png_dir.is_some() || want_pair,
            );
            if want_pair {
                if let (Some(g), Some(l), Some(dir)) =
                    (&game_images, &images, std::env::var_os("L2_PAIR_DIR"))
                {
                    let (w, h) = (raster::WIDTH as usize, raster::HEIGHT as usize);
                    let panes: [&Vec<u8>; 4] = [&g.painter, &g.mask, &l.painter, &l.mask];
                    let mut rgb = vec![0u8; w * 4 * h * 3];
                    for (pi, pane) in panes.iter().enumerate() {
                        for y in 0..h {
                            let src = &pane[y * w * 3..(y + 1) * w * 3];
                            let dst = (y * w * 4 + pi * w) * 3;
                            rgb[dst..dst + w * 3].copy_from_slice(src);
                        }
                    }
                    let _ = fs::create_dir_all(&dir);
                    crate::write_png(
                        &Path::new(&dir).join(format!("pair-{name}-{:x}.png", frame.index)),
                        (w * 4) as u32,
                        h as u32,
                        &rgb,
                    )?;
                }
            }
            if std::env::var_os("L2_FRAME_HYBRID").is_some() {
                // A frame that shows a mover spanning several slots keeps the game's own order.
                let mut multi = false;
                let any_entity = std::env::var("L2_FRAME_HYBRID").as_deref() == Ok("entity");
                let mut seen: std::collections::HashSet<(u16, [i16; 3])> = Default::default();
                for w in &frame.walk {
                    let Some(inf) = w.info else { continue };
                    if any_entity
                        && (model_of(&inf) != 0 || matches!(inf.kind, kind::MODEL | kind::BEAM))
                    {
                        multi = true;
                    }
                    if any_entity {
                        continue;
                    }
                    if matches!(inf.kind, kind::MODEL | kind::BEAM) {
                        continue;
                    }
                    let model = model_of(&inf);
                    if model == 0 || is_static(&info.names[usize::from(model)]) {
                        continue;
                    }
                    if !seen.insert((model, inf.offset)) {
                        continue;
                    }
                    let off = [
                        f32::from(inf.offset[0]),
                        f32::from(inf.offset[2]),
                        f32::from(inf.offset[1]),
                    ];
                    let (first, num) = info.models[usize::from(model)];
                    let (mut lo, mut hi) = (u64::MAX, 0u64);
                    for f in first..first + num {
                        let nrm = bsp.face_normal(f as usize);
                        for v in &bsp.faces[f as usize] {
                            let r = rank_at([
                                v[0] + off[0] + 4.0 * nrm[0],
                                v[1] + off[1] + 4.0 * nrm[1],
                                v[2] + off[2] + 4.0 * nrm[2],
                            ]);
                            lo = lo.min(r);
                            hi = hi.max(r);
                        }
                    }
                    if lo < hi {
                        multi = true;
                    }
                }
                if multi {
                    lab = game.clone();
                    hyb_legacy_frames += 1;
                }
                hyb_frames += 1;
            }
            covered += game.covered;
            {
                let d = diff.get();
                dstat.0 += d.0;
                dstat.1 += d.1;
                dstat.2 += d.2;
                dstat.3 += d.3;
            }
            for (m, px) in &game.brush {
                let nm = info.names.get(usize::from(*m)).cloned().unwrap_or_default();
                let cls = nm.split(' ').next().unwrap_or("").to_string();
                by_entity
                    .entry(format!(
                        "{name} {m} {nm} {}",
                        if is_static(&nm) { "S" } else { "M" }
                    ))
                    .or_default()
                    .0 += px;
                let _ = cls;
            }
            for (m, px) in &game.brush_self {
                let nm = info.names.get(usize::from(*m)).cloned().unwrap_or_default();
                self_ent.entry(format!("{name} {m} {nm}")).or_default().0 += px;
            }
            for (m, px) in &lab.brush_self {
                let nm = info.names.get(usize::from(*m)).cloned().unwrap_or_default();
                self_ent.entry(format!("{name} {m} {nm}")).or_default().1 += px;
            }
            for (m, px) in &lab.brush {
                let nm = info.names.get(usize::from(*m)).cloned().unwrap_or_default();
                by_entity
                    .entry(format!(
                        "{name} {m} {nm} {}",
                        if is_static(&nm) { "S" } else { "M" }
                    ))
                    .or_default()
                    .1 += px;
            }
            add_pairs(&mut pg, &game);
            add_pairs(&mut pl, &lab);
            for (k, v) in &lab.face_pairs {
                *face_pairs.entry(*k).or_default() += v;
            }
            if let (Some(dir), Some(images)) = (&png_dir, images) {
                if lab.wrong_major >= 1500 {
                    let (w, h, rgb) = crate::compose(&images, 1);
                    let _ = fs::create_dir_all(dir);
                    crate::write_png(
                        &Path::new(dir).join(format!("{name}-{:x}.png", frame.index)),
                        w,
                        h,
                        &rgb,
                    )?;
                }
            }
            if let Some(path) = std::env::var_os("L2_FRAMECSV") {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|e| e.to_string())?;
                for (k, v) in &game.pairs {
                    let _ = writeln!(
                        f,
                        "{name},{:x},game,{}/{},{}",
                        frame.index, k.0, k.1, v.major
                    );
                }
                for (k, v) in &lab.pairs {
                    let _ = writeln!(
                        f,
                        "{name},{:x},lab,{}/{},{}",
                        frame.index, k.0, k.1, v.major
                    );
                }
            }
            if verbose {
                println!(
                    "{name} frame {:x}: game {} lab {}",
                    frame.index, game.wrong_major, lab.wrong_major
                );
            }
        }
        println!(
            "{name}: game {} lab {}",
            pg.values().sum::<u64>(),
            pl.values().sum::<u64>()
        );
        if std::env::var_os("L2_FACES").is_some() {
            let mut top: Vec<_> = face_pairs.iter().collect();
            top.sort_by(|a, b| b.1.cmp(a.1));
            for ((pk, pf, vk, vf), px) in top.into_iter().take(6) {
                println!(
                    "   {name} worst: painter kind {pk} face {pf} (model {}) over victim kind {vk} face {vf} (model {}): {px} px",
                    info.model_of(u32::from(*pf)),
                    info.model_of(u32::from(*vf))
                );
            }
        }
        for (k, v) in &pg {
            *grand_game.entry(*k).or_default() += v;
        }
        for (k, v) in &pl {
            *grand_lab.entry(*k).or_default() += v;
        }
    }
    println!("total covered {covered}");
    if std::env::var_os("L2_FRAME_HYBRID").is_some() {
        println!("   HYBRID frames {hyb_frames} legacy {hyb_legacy_frames}");
    }
    if std::env::var_os("L2_DIFFSTAT").is_some() {
        println!(
            "   DIFF static px {} differ {}; mover px {} differ {}",
            dstat.0, dstat.1, dstat.2, dstat.3
        );
    }
    {
        let (mut sg, mut sl, mut mg, mut ml) = (0u64, 0u64, 0u64, 0u64);
        for (k, (g, l)) in &by_entity {
            if k.ends_with(" S") {
                sg += g;
                sl += l;
            } else {
                mg += g;
                ml += l;
            }
        }
        println!("   STATIC brush: game {sg} lab {sl}; MOVERS: game {mg} lab {ml}");
    }
    if std::env::var_os("L2_SELF").is_some() {
        let mut v: Vec<_> = self_ent.iter().collect();
        v.sort_by(|a, b| (b.1 .1).cmp(&(a.1 .1)));
        for (k, (g, l)) in v.into_iter().take(12) {
            println!("   SELF {k}: game {g} lab {l}");
        }
    }
    if std::env::var_os("L2_ENTITIES").is_some() {
        let mut v: Vec<_> = by_entity.iter().collect();
        v.sort_by(|a, b| (b.1 .0).cmp(&(a.1 .0)));
        for (k, (g, l)) in v.into_iter().take(40) {
            println!("   ENT {k}: game {g} lab {l}");
        }
    }
    print_pairs("game", &grand_game);
    print_pairs("lab", &grand_lab);
    Ok(())
}
