//! Frame analysis: painter order against a per-pixel depth test of the same packets.

use crate::log::{kind, Frame, Info};
use crate::raster::{self, HEIGHT, WIDTH};
use std::collections::BTreeMap;

/// Which kind of surface a packet draws, for the census.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    World,
    /// A brush entity face, with its submodel number when the cooked map is known (0 otherwise).
    Brush(u16),
    Model,
    Beam,
    /// A walked packet the guest gave no depth for (sprites, replays).
    Unknown,
}

impl Class {
    /// Name without the submodel number, for the pair table.
    pub fn group(self) -> &'static str {
        match self {
            Class::World => "world",
            Class::Brush(_) => "brush",
            Class::Model => "model",
            Class::Beam => "beam",
            Class::Unknown => "other",
        }
    }
}

/// Why the table order contradicts the depth order at a pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    /// Same table slot: the bucket cannot separate them and insertion order lost.
    Tie,
    /// One of the two polygons spans a deep range, so no single key can be right.
    Span,
    /// The two surfaces are within a few units of each other (decals, plates, trim).
    Thin,
    /// Distinct slots and compact polygons, yet the keys are inverted.
    Inverted,
}

pub const CAUSES: [Cause; 4] = [Cause::Tie, Cause::Span, Cause::Thin, Cause::Inverted];

impl Cause {
    pub fn name(self) -> &'static str {
        match self {
            Cause::Tie => "tie",
            Cause::Span => "span",
            Cause::Thin => "thin",
            Cause::Inverted => "inverted",
        }
    }
}

/// Tolerances in GTE depth units: a pixel is wrong when the painter's top is farther than the
/// true nearest surface by more than `abs + rel * z`.
#[derive(Clone, Copy, Debug)]
pub struct Tolerance {
    pub minor_abs: f32,
    pub minor_rel: f32,
    pub major_abs: f32,
    pub major_rel: f32,
}

impl Default for Tolerance {
    fn default() -> Self {
        Self {
            minor_abs: 1.0,
            minor_rel: 0.004,
            major_abs: 8.0,
            major_rel: 0.03,
        }
    }
}

/// A depth span of this many units on either polygon makes a pixel a `Span` case.
const SPAN_UNITS: f32 = 48.0;
/// Surfaces closer than this are `Thin`.
const THIN_UNITS: f32 = 8.0;

struct Packet {
    class: Class,
    info: Info,
    translucent: bool,
    z: [f32; 4],
    span: f32,
}

#[derive(Clone, Copy)]
struct Frag {
    order: u32,
    z: f32,
    id: u32,
}

/// Counts for one (painter class, true-nearest class) pair.
#[derive(Clone, Copy, Debug, Default)]
pub struct PairCount {
    pub minor: u64,
    pub major: u64,
    pub by_cause: [u64; 4],
}

#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    pub index: u32,
    pub label: String,
    pub packets: usize,
    pub unknown_packets: usize,
    /// Packets whose depths were not derivable and so use the flat key depth.
    pub approx_packets: usize,
    /// Packets whose depth came from their plane / from vertex depths / disagreed and fell back.
    pub plane_used: usize,
    pub plane_disagree: usize,
    pub covered: u64,
    pub unknown_covered: u64,
    pub wrong_minor: u64,
    pub wrong_major: u64,
    pub translucent_wrong: u64,
    pub by_cause: [u64; 4],
    /// Keyed by (painter-top class group, nearest class group).
    pub pairs: BTreeMap<(&'static str, &'static str), PairCount>,
    /// Wrong pixels attributed to each brush submodel (as painter or as victim).
    pub brush: BTreeMap<u16, u64>,
    /// Most-offending face pairs: (painter kind, painter face, victim kind, victim face) -> pixels.
    pub face_pairs: BTreeMap<(u8, u16, u8, u16), u64>,
}

/// Rendered panes for a frame.
pub struct Images {
    /// RGB, `WIDTH * HEIGHT * 3`.
    pub painter: Vec<u8>,
    pub depth: Vec<u8>,
    pub mask: Vec<u8>,
}

fn color_of(class: Class, id: u32) -> [u8; 3] {
    let h = id.wrapping_mul(2654435761);
    let v = 90 + (h >> 24) as u8 % 140;
    let w = 60 + (h >> 16) as u8 % 120;
    match class {
        Class::World => [w / 2, w, v],
        Class::Brush(_) => [v, v / 2 + 40, w / 3],
        Class::Model => [v, w / 3, v],
        Class::Beam => [w / 3, v, v],
        Class::Unknown => [120, 120, 120],
    }
}

/// What the census knows about the map being analysed.
pub struct Context<'a> {
    /// Names the surface class of a packet (world or brush submodel).
    pub class_of: &'a dyn Fn(u8, u16) -> Class,
    /// The plane a face lies on, (normal, distance) in the game's axes, shifted by the brush
    /// offset; lets the oracle compute exact per-pixel depth instead of interpolating vertex
    /// depths the guest could only partly recover.
    pub plane_of: &'a dyn Fn(u16, [i32; 3]) -> Option<([f64; 3], f64)>,
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Analyse one frame.
pub fn analyze(
    frame: &Frame,
    tol: Tolerance,
    ctx: &Context<'_>,
    want_images: bool,
) -> (FrameStats, Option<Images>) {
    let class_of = ctx.class_of;
    let mut stats = FrameStats {
        index: frame.index,
        label: frame.label.clone(),
        packets: frame.walk.len(),
        ..FrameStats::default()
    };
    let npix = (WIDTH * HEIGHT) as usize;
    let mut packets: Vec<Option<Packet>> = Vec::with_capacity(frame.walk.len());
    let mut opaque: Vec<Vec<Frag>> = vec![Vec::new(); npix];
    let mut trans: Vec<Vec<Frag>> = vec![Vec::new(); npix];
    let mut unknown = vec![false; npix];
    for (order, walked) in frame.walk.iter().enumerate() {
        let poly = raster::decode(&walked.words);
        let (Some(poly), Some(info)) = (poly, walked.info) else {
            // No depth, or not a polygon: mark its pixels so the mask can show them.
            stats.unknown_packets += 1;
            if let Some(poly) = poly {
                raster::polygon(&poly, [1.0; 4], |x, y, _| {
                    unknown[(y * WIDTH + x) as usize] = true;
                    stats.unknown_covered += 1;
                });
            }
            packets.push(None);
            continue;
        };
        // A blocked-edge skirt (a one pixel strip keyed at the far end of the table so that it
        // only shows where nothing else was drawn) is deliberately behind everything, however
        // near its own plane is: it fills cracks and is not an occluder. The guest tags it
        // `kind::SKIRT`; logs from before the tag are recognised by shape.
        let thin_strip = {
            let xs = poly.verts[..poly.n].iter().map(|v| v.0);
            let ys = poly.verts[..poly.n].iter().map(|v| v.1);
            let w = xs.clone().max().unwrap_or(0) - xs.min().unwrap_or(0);
            let h = ys.clone().max().unwrap_or(0) - ys.min().unwrap_or(0);
            w.min(h) <= 2
        };
        if info.kind == kind::SKIRT || (info.mode == 0 && info.key >= 639 && thin_strip) {
            packets.push(None);
            continue;
        }
        let reliable = matches!(info.mode, 1 | 2) && info.kind != kind::MODEL;
        let mut z = [0.0f32; 4];
        if reliable || info.kind == kind::MODEL {
            for (dst, src) in z.iter_mut().zip(info.z) {
                *dst = src as f32;
            }
        } else {
            stats.approx_packets += 1;
            z = [(info.key as f32) * 4.0 + 2.0; 4];
        }
        for k in 0..poly.n {
            if z[k] < 1.0 {
                z[k] = 1.0;
            }
        }
        // Exact depth from the face plane when the face is known and, where the guest recorded
        // trustworthy vertex depths, agrees with them (a rotating brush moves off its plane).
        let plane_fn = if info.kind == kind::MODEL || info.kind == kind::BEAM {
            None
        } else {
            let offset = if info.brush != 0 {
                [
                    i32::from(info.offset[0]),
                    i32::from(info.offset[1]),
                    i32::from(info.offset[2]),
                ]
            } else {
                [0; 3]
            };
            (ctx.plane_of)(info.face, offset).and_then(|(n, d)| {
                let nc = [
                    dot3(frame.rot[0], n),
                    dot3(frame.rot[1], n),
                    dot3(frame.rot[2], n),
                ];
                let eye = [
                    f64::from(frame.eye[0]),
                    f64::from(frame.eye[1]),
                    f64::from(frame.eye[2]),
                ];
                let dp = d - dot3(n, eye);
                let h = frame.h;
                let f = move |x: f64, y: f64| -> f32 {
                    let den = dot3(nc, [(x - 160.0) / h, (y - 120.0) / h, 1.0]);
                    if den.abs() < 1e-9 {
                        return f32::MAX;
                    }
                    ((dp / den) as f32).clamp(1.0, 60000.0)
                };
                let vz: Vec<f32> = (0..poly.n)
                    .map(|k| f(f64::from(poly.verts[k].0), f64::from(poly.verts[k].1)))
                    .collect();
                // World faces always take the plane (the guest's per-vertex depths for patch cells
                // are the parent's); a brush face keeps vertex depths when the plane is off.
                if reliable && info.brush != 0 {
                    let agree = (0..poly.n).all(|k| (vz[k] - z[k]).abs() <= 4.0 + 0.03 * z[k]);
                    if !agree {
                        stats.plane_disagree += 1;
                        return None;
                    }
                }
                if reliable && info.brush == 0 {
                    let agree = (0..poly.n).all(|k| (vz[k] - z[k]).abs() <= 4.0 + 0.03 * z[k]);
                    if !agree {
                        stats.plane_disagree += 1;
                    }
                }
                if vz.iter().any(|&v| v >= 59999.0) {
                    return None;
                }
                for k in 0..poly.n {
                    z[k] = vz[k];
                }
                stats.plane_used += 1;
                Some(f)
            })
        };
        let zs = &z[..poly.n];
        let span =
            zs.iter().cloned().fold(0.0, f32::max) - zs.iter().cloned().fold(f32::MAX, f32::min);
        let class = match info.kind {
            kind::MODEL => Class::Model,
            kind::BEAM => Class::Beam,
            k => class_of(k, info.face),
        };
        let translucent = poly.semi || info.policy & 0x03 != 0;
        let id = packets.len() as u32;
        raster::polygon(&poly, z, |x, y, depth| {
            let depth = match &plane_fn {
                Some(f) => f(f64::from(x) + 0.5, f64::from(y) + 0.5),
                None => depth,
            };
            let frag = Frag {
                order: order as u32,
                z: depth,
                id,
            };
            let p = (y * WIDTH + x) as usize;
            if translucent {
                trans[p].push(frag);
            } else {
                opaque[p].push(frag);
            }
        });
        packets.push(Some(Packet {
            class,
            info,
            translucent,
            z,
            span,
        }));
    }
    let mut images = want_images.then(|| Images {
        painter: vec![0; npix * 3],
        depth: vec![0; npix * 3],
        mask: vec![0; npix * 3],
    });
    for p in 0..npix {
        let frags = &opaque[p];
        let any = !frags.is_empty() || !trans[p].is_empty();
        if any {
            stats.covered += 1;
        }
        let mut verdict: Option<(bool, Cause)> = None;
        if let Some(&top) = frags.last() {
            let mut near = frags[0];
            for &f in frags.iter() {
                if f.z <= near.z {
                    near = f;
                }
            }
            if let Some(images) = images.as_mut() {
                if let Some(pk) = &packets[top.id as usize] {
                    images.painter[p * 3..p * 3 + 3].copy_from_slice(&color_of(pk.class, top.id));
                }
                if let Some(pk) = &packets[near.id as usize] {
                    images.depth[p * 3..p * 3 + 3].copy_from_slice(&color_of(pk.class, near.id));
                }
            }
            let gap = top.z - near.z;
            if top.id != near.id && gap > tol.minor_abs + tol.minor_rel * near.z {
                let major = gap > tol.major_abs + tol.major_rel * near.z;
                if let (Some(a), Some(b)) = (&packets[top.id as usize], &packets[near.id as usize])
                {
                    let cause = if a.info.key == b.info.key {
                        Cause::Tie
                    } else if a.span >= SPAN_UNITS || b.span >= SPAN_UNITS {
                        Cause::Span
                    } else if gap < THIN_UNITS {
                        Cause::Thin
                    } else {
                        Cause::Inverted
                    };
                    stats.wrong_minor += 1;
                    stats.wrong_major += major as u64;
                    let ci = CAUSES.iter().position(|&c| c == cause).unwrap_or(0);
                    stats.by_cause[ci] += 1;
                    let key = match (a.class, b.class) {
                        (Class::Brush(x), Class::Brush(y)) if x == y && x != 0 => {
                            ("brush-self", "brush-self")
                        }
                        _ => (a.class.group(), b.class.group()),
                    };
                    let pair = stats.pairs.entry(key).or_default();
                    pair.minor += 1;
                    pair.major += major as u64;
                    pair.by_cause[ci] += 1;
                    for class in [a.class, b.class] {
                        if let Class::Brush(m) = class {
                            *stats.brush.entry(m).or_default() += 1;
                        }
                    }
                    *stats
                        .face_pairs
                        .entry((a.info.kind, a.info.face, b.info.kind, b.info.face))
                        .or_default() += 1;
                    verdict = Some((major, cause));
                }
            }
        }
        let mut trans_wrong = false;
        for &t in &trans[p] {
            let tz = t.z;
            let tol_t = tol.minor_abs + tol.minor_rel * tz;
            trans_wrong |= frags.iter().any(|o| {
                (o.z < tz - tol_t && o.order < t.order) || (o.z > tz + tol_t && o.order > t.order)
            });
        }
        if trans_wrong {
            stats.translucent_wrong += 1;
        }
        if let Some(images) = images.as_mut() {
            let color = match (verdict, trans_wrong, unknown[p], any) {
                (Some((true, _)), _, _, _) => [255, 40, 40],
                (Some((false, _)), _, _, _) => [255, 220, 60],
                (None, true, _, _) => [255, 60, 255],
                (None, false, true, _) => [60, 90, 255],
                (None, false, false, true) => [50, 50, 50],
                _ => [0, 0, 0],
            };
            images.mask[p * 3..p * 3 + 3].copy_from_slice(&color);
        }
    }
    (stats, images)
}

/// Depth range (near, far) of a packet from its face plane at its vertices, for packets whose
/// plane is known and trustworthy (world faces).
pub fn plane_depth_range(
    frame: &Frame,
    walked: &crate::log::Walked,
    ctx: &Context<'_>,
) -> Option<(f32, f32)> {
    let info = walked.info?;
    let poly = raster::decode(&walked.words)?;
    let (n, d) = (ctx.plane_of)(info.face, [0; 3])?;
    let nc = [
        dot3(frame.rot[0], n),
        dot3(frame.rot[1], n),
        dot3(frame.rot[2], n),
    ];
    let eye = [
        f64::from(frame.eye[0]),
        f64::from(frame.eye[1]),
        f64::from(frame.eye[2]),
    ];
    let dp = d - dot3(n, eye);
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for k in 0..poly.n {
        let u = [
            (f64::from(poly.verts[k].0) - 160.0) / frame.h,
            (f64::from(poly.verts[k].1) - 120.0) / frame.h,
            1.0,
        ];
        let den = dot3(nc, u);
        if den.abs() < 1e-9 {
            return None;
        }
        let z = (dp / den) as f32;
        lo = lo.min(z);
        hi = hi.max(z);
    }
    Some((lo.max(1.0), hi.max(1.0)))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{Frame, Info, Walked};

    fn word(x: i32, y: i32) -> u32 {
        (x as u32 & 0x7ff) | ((y as u32 & 0x7ff) << 16)
    }

    /// A flat-coloured quad packet (words after the tag) covering (x0,y0)-(x1,y1).
    fn quad(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<u32> {
        vec![
            0x2880_8080,
            word(x0, y0),
            word(x1, y0),
            word(x0, y1),
            word(x1, y1),
        ]
    }

    fn packet(words: Vec<u32>, depth: u16, key: u16) -> Walked {
        Walked {
            addr: 0,
            words,
            info: Some(Info {
                kind: 4,
                mode: 2,
                n: 4,
                key,
                face: 0,
                policy: 0,
                z: [depth; 4],
                brush: 0,
                offset: [0; 3],
            }),
        }
    }

    fn analyse(walk: Vec<Walked>) -> FrameStats {
        let frame = Frame {
            walk,
            ..Frame::default()
        };
        let class_of = |_: u8, _: u16| Class::World;
        let plane_of = |_: u16, _: [i32; 3]| None;
        let ctx = Context {
            class_of: &class_of,
            plane_of: &plane_of,
        };
        analyze(&frame, Tolerance::default(), &ctx, false).0
    }

    #[test]
    fn far_then_near_is_right_and_near_then_far_is_wrong() {
        let near = packet(quad(0, 0, 8, 8), 100, 25);
        let far = packet(quad(0, 0, 8, 8), 300, 75);
        let right = analyse(vec![far.clone(), near.clone()]);
        assert_eq!(right.wrong_minor, 0);
        let wrong = analyse(vec![near, far]);
        assert_eq!(wrong.wrong_minor, 64);
        assert_eq!(wrong.wrong_major, 64);
    }

    #[test]
    fn coplanar_noise_inside_the_tolerance_is_not_an_error() {
        let a = packet(quad(0, 0, 8, 8), 200, 50);
        let b = packet(quad(0, 0, 8, 8), 200, 50);
        assert_eq!(analyse(vec![a, b]).wrong_minor, 0);
    }
}
