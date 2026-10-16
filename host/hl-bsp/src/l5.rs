//! Lane L5 cook: map-painted `infodecal`s (decals.wad splats, signs and grime)
//! and the per-map section that carries them.
//!
//! GoldSrc applies a static infodecal when the map loads: `CDecal::StaticDecal`
//! hands its origin to the engine, whose `R_DecalShoot` walks the BSP, takes
//! every surface within `DECAL_DISTANCE` (4 units) of the origin and projects
//! the decal onto it along the surface's texture axes, clipped to the polygon.
//! The port draws each as one flat quad on the dominant host surface instead,
//! slid inward so it stays inside the polygon's texture-space bounds, in the
//! world order queue the runtime already uses for bullet and blood marks.
//!
//! Output: the "L5X1" section (written where the retired census-only world
//! pipeline header used to sit, see `HEADER_WORLD_PIPELINE_OFFSET`) and one
//! extra 128x128 4 bit texture appended to the room's texture chunk, holding
//! every distinct decal this map paints.

use crate::{
    f32le, i32le, is_tool_texture, miptex_name, parse_vec3, point_leaf_from, read_palette, u16le,
    u32le, CookedTex, WadIndex, SZ_EDGE, SZ_FACE, SZ_PLANE, SZ_SURFEDGE, SZ_TEXINFO,
};

use hl_format::map as cooked;
pub const SECTION_MAGIC: [u8; 4] = cooked::L5_SECTION_MAGIC;
pub const SECTION_HEADER: usize = cooked::L5_SECTION_HEADER;
pub const DEF_BYTES: usize = cooked::L5_DEF_BYTES;
pub const DECAL_BYTES: usize = cooked::L5_DECAL_BYTES;
pub const ATLAS: usize = 128;
/// A record's code keeps the definition index in its low five bits.
pub const MAX_DEFS: usize = 32;
/// Distance a decal's host surface may sit from the entity origin
/// (`DECAL_DISTANCE` in the engine's decal code).
const HOST_DISTANCE: f64 = 4.0;
/// How far outside the polygon the origin may sit and still pick that surface.
const HOST_SLACK: f64 = 24.0;
/// Lift off the surface, in world units (the runtime marks lift the same).
const LIFT: f64 = 2.0;
/// Vertex tint = palette tint * light / 2, with `light` the surface's own
/// lightmap level where the decal sits (1.0 = the neutral 128 modulation),
/// averaged over the map's uses of the texture. The CLUT is white, so the
/// PS1's 128 = 1.0x modulation doubles the value; GoldSrc lights a decal by
/// the lightmap of the surface it lies on, which one flat vertex colour per
/// texture approximates. At a typical 1.5 a retail blood (22..52) lands in
/// the same dark red the runtime's own blood marks use.
const TINT_FLOOR: f64 = 0.25;

pub struct Def {
    pub name: String,
    /// Source texels.
    pub src: (usize, usize),
    /// Atlas cell origin and size, texels.
    pub cell: (usize, usize, usize, usize),
    /// Vertex colour (modulates the white CLUT).
    pub tint: [u8; 3],
}

pub struct Rec {
    pub pos: [i16; 3],
    pub leaf: u16,
    /// Definition (bits 0..4), right direction (5..9), down direction (10..14).
    pub code: u16,
}

#[derive(Default)]
pub struct Stats {
    pub placed: usize,
    pub unknown_texture: usize,
    pub no_host: usize,
    pub on_movers: usize,
    pub overflow_defs: usize,
    pub dropped_budget: usize,
}

pub struct DecalCook {
    pub defs: Vec<Def>,
    pub half: Vec<[u8; 2]>,
    pub recs: Vec<Rec>,
    pub stats: Stats,
}

impl DecalCook {
    /// Bytes this cook adds to the resident room (header excluded).
    pub fn bytes(&self) -> usize {
        self.defs.len() * DEF_BYTES + self.recs.len() * DECAL_BYTES
    }
}

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn scale3(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm3(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l < 1e-9 {
        [0.0, 0.0, 0.0]
    } else {
        scale3(a, 1.0 / l)
    }
}

/// HL (x, y, z up) to the runtime's world (x, z, y).
fn to_world3(p: V3) -> V3 {
    [p[0], p[2], p[1]]
}

/// A direction as one of the 26 neighbours of a cube: ternary digits (x, y, z)
/// in -1..=1, code = (x + 1) + 3 (y + 1) + 9 (z + 1), 13 (all zero) unused.
/// Surfaces decals sit on are axial or chamfered, which these cover exactly;
/// the runtime decodes a code with a handful of integer operations
/// (`dir26` in game/src/main.rs).
pub fn decode_dir(code: u8) -> V3 {
    let c = code as i32;
    let d = [(c % 3) - 1, ((c / 3) % 3) - 1, (c / 9) - 1];
    norm3([d[0] as f64, d[1] as f64, d[2] as f64])
}

/// The code of the neighbour direction closest to `v`.
pub fn encode_dir(v: V3) -> u8 {
    let mut best = (f64::MIN, 0u8);
    for c in 0..27u8 {
        if c == 13 {
            continue;
        }
        let s = dot(v, decode_dir(c));
        if s > best.0 {
            best = (s, c);
        }
    }
    best.1
}

struct DecalImage {
    w: usize,
    h: usize,
    alpha: Vec<u8>,
    tint: (u8, u8, u8),
}

fn load_decal(wads: &WadIndex, name: &str) -> Option<DecalImage> {
    let lump = wads.miptex(name)?;
    let w = u32le(lump, 16)? as usize;
    let h = u32le(lump, 20)? as usize;
    let off0 = u32le(lump, 24)? as usize;
    if w == 0 || h == 0 || off0 == 0 {
        return None;
    }
    let alpha = lump.get(off0..off0 + w * h)?.to_vec();
    let pal = read_palette(lump, 0, w, h)?;
    Some(DecalImage {
        w,
        h,
        alpha,
        tint: pal[255],
    })
}

/// One `cw`x`ch` cell of decal coverage in the three PS1 classes the port's
/// own decals use: clear (0), half (1, semi-transparent) and full (8).
fn coverage_cell(img: &DecalImage, cw: usize, ch: usize) -> Vec<u8> {
    let mut cell = vec![0u8; cw * ch];
    for y in 0..ch {
        for x in 0..cw {
            let x0 = x * img.w / cw;
            let x1 = ((x + 1) * img.w / cw).max(x0 + 1);
            let y0 = y * img.h / ch;
            let y1 = ((y + 1) * img.h / ch).max(y0 + 1);
            let mut sum = 0usize;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    sum += img.alpha[sy * img.w + sx] as usize;
                }
            }
            let mean = sum / ((x1 - x0) * (y1 - y0));
            cell[y * cw + x] = if mean >= 150 {
                8
            } else if mean >= 50 {
                1
            } else {
                0
            };
        }
    }
    cell
}

/// Shelf-pack `sizes` into the atlas; None if they do not fit.
fn pack(sizes: &[(usize, usize)]) -> Option<Vec<(usize, usize)>> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| {
        (
            std::cmp::Reverse(sizes[i].1),
            std::cmp::Reverse(sizes[i].0),
            i,
        )
    });
    let mut at = vec![(0usize, 0usize); sizes.len()];
    let (mut x, mut y, mut row_h) = (0usize, 0usize, 0usize);
    for i in order {
        let (w, h) = sizes[i];
        if w > ATLAS || h > ATLAS {
            return None;
        }
        if x + w > ATLAS {
            x = 0;
            y += row_h;
            row_h = 0;
        }
        if y + h > ATLAS {
            return None;
        }
        at[i] = (x, y);
        x += w;
        row_h = row_h.max(h);
    }
    Some(at)
}

/// Cell sizes: native texels, halved (largest first) until everything packs.
fn cell_sizes(src: &[(usize, usize)]) -> Option<Vec<(usize, usize)>> {
    let mut cells: Vec<(usize, usize)> = src.to_vec();
    loop {
        if pack(&cells).is_some() {
            return Some(cells);
        }
        let (i, _) = cells
            .iter()
            .enumerate()
            .filter(|(_, &(w, h))| w > 4 && h > 4)
            .max_by_key(|(_, &(w, h))| w * h)?;
        cells[i] = (cells[i].0.div_ceil(2).max(4), cells[i].1.div_ceil(2).max(4));
    }
}

struct Poly {
    pts: Vec<V3>,
    n: V3,
    s_axis: V3,
    t_axis: V3,
}

struct Host {
    face: usize,
    poly: Poly,
    dist: f64,
    outside: f64,
}

/// Distance from `p` (in the polygon plane) to the polygon, 0 when inside.
fn plane_point_outside(poly: &Poly, p: V3) -> f64 {
    let n = poly.pts.len();
    let mut inside = true;
    let mut best = f64::MAX;
    for i in 0..n {
        let a = poly.pts[i];
        let b = poly.pts[(i + 1) % n];
        let edge = sub(b, a);
        let len2 = dot(edge, edge).max(1e-9);
        let side = dot(cross(poly.n, edge), sub(p, a));
        if side > 0.0 {
            inside = false;
        }
        let t = (dot(sub(p, a), edge) / len2).clamp(0.0, 1.0);
        let q = add(a, scale3(edge, t));
        let d = sub(p, q);
        best = best.min(dot(d, d).sqrt());
    }
    if inside {
        0.0
    } else {
        best
    }
}

struct Lumps<'a> {
    verts: &'a [u8],
    edges: &'a [u8],
    surf: &'a [u8],
    faces: &'a [u8],
    planes: &'a [u8],
    texinfo: &'a [u8],
    tex_lump: &'a [u8],
    nodes: &'a [u8],
    leaves: &'a [u8],
    models: &'a [u8],
}

pub struct Geometry<'a> {
    pub verts: &'a [u8],
    pub edges: &'a [u8],
    pub surf: &'a [u8],
    pub faces: &'a [u8],
    pub planes: &'a [u8],
    pub texinfo: &'a [u8],
    pub textures: &'a [u8],
    pub nodes: &'a [u8],
    pub leaves: &'a [u8],
    pub models: &'a [u8],
    pub n_visleaves: usize,
    pub lighting: &'a [u8],
    pub start_styles: &'a [u32; 64],
}

fn face_poly(l: &Lumps, f: usize) -> Option<Poly> {
    let fo = f * SZ_FACE;
    let planenum = u16le(l.faces, fo)? as usize;
    let side = u16le(l.faces, fo + 2)? != 0;
    let firstedge = i32le(l.faces, fo + 4)?.max(0) as usize;
    let numedges = u16le(l.faces, fo + 8)? as usize;
    let ti = u16le(l.faces, fo + 10)? as usize;
    if numedges < 3 {
        return None;
    }
    let po = planenum * SZ_PLANE;
    let mut n = [
        f32le(l.planes, po)? as f64,
        f32le(l.planes, po + 4)? as f64,
        f32le(l.planes, po + 8)? as f64,
    ];
    if side {
        n = scale3(n, -1.0);
    }
    let mut pts = Vec::with_capacity(numedges);
    for j in 0..numedges {
        let se = i32le(l.surf, (firstedge + j) * SZ_SURFEDGE)?;
        let e = se.unsigned_abs() as usize;
        let eo = e * SZ_EDGE;
        let v = if se >= 0 {
            u16le(l.edges, eo)?
        } else {
            u16le(l.edges, eo + 2)?
        } as usize;
        let vo = v * 12;
        pts.push([
            f32le(l.verts, vo)? as f64,
            f32le(l.verts, vo + 4)? as f64,
            f32le(l.verts, vo + 8)? as f64,
        ]);
    }
    let to = ti * SZ_TEXINFO;
    let s_axis = [
        f32le(l.texinfo, to)? as f64,
        f32le(l.texinfo, to + 4)? as f64,
        f32le(l.texinfo, to + 8)? as f64,
    ];
    let t_axis = [
        f32le(l.texinfo, to + 16)? as f64,
        f32le(l.texinfo, to + 20)? as f64,
        f32le(l.texinfo, to + 24)? as f64,
    ];
    Some(Poly {
        pts,
        n,
        s_axis,
        t_axis,
    })
}

fn tex_name_of(l: &Lumps, ti: usize) -> String {
    let mt = i32le(l.texinfo, ti * SZ_TEXINFO + 32).unwrap_or(-1);
    let n = i32le(l.tex_lump, 0).unwrap_or(0);
    if mt < 0 || mt >= n {
        return String::new();
    }
    match i32le(l.tex_lump, 4 + mt as usize * 4) {
        Some(d) if d >= 0 => miptex_name(l.tex_lump, d as usize),
        _ => String::new(),
    }
}

/// A brush model a decal may land on. The world and `func_wall` /
/// `func_illusionary` pieces that never move are drawn where they stand;
/// every other brush entity moves, and a decal on one would have to follow it.
struct HostModel {
    faces: std::ops::Range<usize>,
    /// The entity's `origin` key: submodel vertices are relative to it.
    offset: V3,
    fixed: bool,
}

/// The best host among the fixed models, and whether a moving model fits the
/// origin better (in which case the decal belongs to it and is not drawn).
fn find_host(l: &Lumps, o: V3, models: &[HostModel]) -> (Option<Host>, bool) {
    let mut best: Option<(f64, Host)> = None;
    let mut best_mover = f64::MAX;
    for model in models {
        for f in model.faces.clone() {
            let fo = f * SZ_FACE;
            let ti = u16le(l.faces, fo + 10).unwrap_or(0) as usize;
            let flags = i32le(l.texinfo, ti * SZ_TEXINFO + 36).unwrap_or(0);
            let name = tex_name_of(l, ti);
            // TEX_SPECIAL covers sky and liquids; tool textures are never drawn.
            if flags & 1 != 0
                || is_tool_texture(&name)
                || name.starts_with('!')
                || name.starts_with('*')
            {
                continue;
            }
            let Some(mut poly) = face_poly(l, f) else {
                continue;
            };
            for p in &mut poly.pts {
                *p = add(*p, model.offset);
            }
            let d0 = dot(poly.n, poly.pts[0]);
            let dist = dot(poly.n, o) - d0;
            if dist.abs() > HOST_DISTANCE + 0.5 {
                continue;
            }
            let p = sub(o, scale3(poly.n, dist));
            let outside = plane_point_outside(&poly, p);
            if outside > HOST_SLACK {
                continue;
            }
            let score = dist.abs() + outside * 2.0;
            if !model.fixed {
                best_mover = best_mover.min(score);
            } else if best.as_ref().is_none_or(|(s, _)| score < *s) {
                best = Some((
                    score,
                    Host {
                        face: f,
                        poly,
                        dist,
                        outside,
                    },
                ));
            }
        }
    }
    let mover_wins = best.as_ref().is_none_or(|(s, _)| best_mover < *s);
    (best.map(|(_, h)| h), mover_wins && best_mover < f64::MAX)
}

/// Every brush model with faces, in model order: the world first, then each
/// brush entity's submodel.
fn host_models(ents: &str, g: &Geometry) -> Vec<HostModel> {
    let n = g.models.len() / 64;
    let range = |mi: usize| {
        let first = u32le(g.models, mi * 64 + 56).unwrap_or(0) as usize;
        let count = u32le(g.models, mi * 64 + 60).unwrap_or(0) as usize;
        first..first + count
    };
    let mut out = vec![HostModel {
        faces: range(0),
        offset: [0.0; 3],
        fixed: true,
    }];
    for block in ents.split('{') {
        let Some(mi) = crate::ent_value(block, "model")
            .and_then(|m| m.strip_prefix('*'))
            .and_then(|m| m.parse::<usize>().ok())
        else {
            continue;
        };
        if mi == 0 || mi >= n {
            continue;
        }
        let class = crate::ent_value(block, "classname").unwrap_or("");
        let named = !crate::ent_value(block, "targetname")
            .unwrap_or("")
            .is_empty();
        let offset = crate::ent_value(block, "origin")
            .and_then(parse_vec3)
            .map_or([0.0; 3], |v| [v[0] as f64, v[1] as f64, v[2] as f64]);
        out.push(HostModel {
            faces: range(mi),
            offset,
            fixed: matches!(class, "func_wall" | "func_illusionary") && !named,
        });
    }
    out
}

/// Texture-space bounds of the polygon about the projected origin, along the
/// decal's own right/down axes.
fn polygon_extent(poly: &Poly, p: V3, r: V3, d: V3) -> ([f64; 2], [f64; 2]) {
    let (mut ru, mut rv) = ([f64::MAX, f64::MIN], [f64::MAX, f64::MIN]);
    for &q in &poly.pts {
        let rel = sub(q, p);
        let (u, v) = (dot(rel, r), dot(rel, d));
        ru = [ru[0].min(u), ru[1].max(u)];
        rv = [rv[0].min(v), rv[1].max(v)];
    }
    (ru, rv)
}

/// Slide an interval of half-width `h` around 0 so it lies inside `[lo, hi]`
/// when it can; otherwise centre it.
fn fit_interval(lo: f64, hi: f64, h: f64) -> f64 {
    if hi - lo <= 2.0 * h {
        return (lo + hi) / 2.0;
    }
    let c = 0.0f64;
    c.clamp(lo + h, hi - h)
}

/// The lightmap level of face `f` at the HL-space point `p`: 1.0 is the
/// neutral 128 vertex modulation. A face without a lightmap draws fullbright.
fn surface_level(g: &Geometry, poly: &Poly, f: usize, p: V3) -> f64 {
    let fo = f * SZ_FACE;
    let lightofs = i32le(g.faces, fo + 16).unwrap_or(-1);
    if lightofs < 0 || g.faces.get(fo + 12) == Some(&0xff) {
        return 1.0;
    }
    let ti = u16le(g.faces, fo + 10).unwrap_or(0) as usize;
    let to = ti * SZ_TEXINFO;
    let rd = |o: usize| f32le(g.texinfo, to + o).unwrap_or(0.0) as f64;
    let st = |axis: usize, q: V3| {
        rd(axis * 16) * q[0]
            + rd(axis * 16 + 4) * q[1]
            + rd(axis * 16 + 8) * q[2]
            + rd(axis * 16 + 12)
    };
    let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &q in &poly.pts {
        let (u, v) = (st(0, q), st(1, q));
        u0 = u0.min(u);
        u1 = u1.max(u);
        v0 = v0.min(v);
        v1 = v1.max(v);
    }
    let lmw = ((((u1 / 16.0).ceil() - (u0 / 16.0).floor()) as i64) + 1).clamp(1, 64) as usize;
    let lmh = ((((v1 / 16.0).ceil() - (v0 / 16.0).floor()) as i64) + 1).clamp(1, 64) as usize;
    let layers = crate::lit_layers_for(g.faces, f, g.start_styles);
    let Some(sum) = crate::plane_sums(
        g.lighting,
        lightofs,
        lmw,
        lmh,
        st(0, p) as f32,
        st(1, p) as f32,
        (u0 / 16.0).floor() as i32,
        (v0 / 16.0).floor() as i32,
        &layers,
    ) else {
        return 1.0;
    };
    let lit = |c: u32| crate::light_curve((c / 256).min(255) as u8) as f64;
    ((lit(sum[0]) + lit(sum[1]) + lit(sum[2])) / 3.0 / 128.0).clamp(TINT_FLOOR, 2.0)
}

/// What lane L5 may add to a room whose weapon-cache margin is nearly spent
/// (`cargo hl-build audit`, weapon cache RAM audit): the margin is the map
/// staging window minus the cooked room (plus the unused model pool), so every
/// byte a tight room grows is a byte the weapon cache loses.
pub struct RoomBudget {
    /// Largest the L5X1 section may be, header included.
    pub section: usize,
    /// Whether the room carries switchable/animated lightstyle planes. They
    /// cost about a hundred bytes per lit face; a tight room bakes its lights
    /// at their start state instead.
    pub lights: bool,
}

/// The budget of a tight room, `None` for every other. c5a1 and c2a4e keep
/// 580 B and 648 B of margin at b5a58b2, shared with every other lane that
/// grows those rooms or their models. The 48 B census header the section
/// replaces comes back to them; the budgets leave 300 B (c5a1) and 400 B
/// (c2a4e) of margin, and each 10 B more is one more decal. With lights,
/// c2a4e would grow by about 7.4 KB and c5a1 by about 4.2 KB.
pub fn room_budget(map: &str) -> Option<RoomBudget> {
    match map {
        "c5a1" => Some(RoomBudget {
            section: 48 + 280,
            lights: false,
        }),
        "c2a4e" => Some(RoomBudget {
            section: 48 + 248,
            lights: false,
        }),
        _ => None,
    }
}

/// Cook the static infodecals of one map. `budget` caps the bytes this adds to
/// the resident room (None = unlimited); records beyond it are dropped,
/// farthest from the spawn first.
pub fn cook_decals(
    ents: &str,
    wads: &WadIndex,
    g: &Geometry,
    scale: f32,
    budget: Option<usize>,
    spawn_hl: Option<[f32; 3]>,
) -> Option<(DecalCook, CookedTex)> {
    #[derive(Clone)]
    struct Placement {
        name: String,
        origin: V3,
    }
    let mut placements = Vec::new();
    for block in ents.split('{') {
        if crate::ent_value(block, "classname") != Some("infodecal") {
            continue;
        }
        if !crate::ent_value(block, "targetname")
            .unwrap_or("")
            .is_empty()
        {
            continue;
        }
        let (Some(tex), Some(o)) = (
            crate::ent_value(block, "texture"),
            crate::ent_value(block, "origin").and_then(parse_vec3),
        ) else {
            continue;
        };
        // entity_text drops the braces of quoted values, so the WAD name is
        // the texture key with its `{` put back.
        let name = format!("{{{}", tex.trim().trim_start_matches('{')).to_ascii_lowercase();
        placements.push(Placement {
            name,
            origin: [o[0] as f64, o[1] as f64, o[2] as f64],
        });
    }
    if placements.is_empty() {
        return None;
    }
    let mut stats = Stats::default();
    let lumps = Lumps {
        verts: g.verts,
        edges: g.edges,
        surf: g.surf,
        faces: g.faces,
        planes: g.planes,
        texinfo: g.texinfo,
        tex_lump: g.textures,
        nodes: g.nodes,
        leaves: g.leaves,
        models: g.models,
    };
    let hosts = host_models(ents, g);
    let head0 = crate::model_headnode(g.models, 0, 0).unwrap_or(0);

    // Definitions: one per distinct texture, in order of first appearance.
    let mut images: Vec<(String, DecalImage)> = Vec::new();
    let mut def_of: std::collections::HashMap<String, Option<usize>> = Default::default();
    for p in &placements {
        if def_of.contains_key(&p.name) {
            continue;
        }
        match load_decal(wads, &p.name) {
            Some(img) if images.len() < MAX_DEFS => {
                def_of.insert(p.name.clone(), Some(images.len()));
                images.push((p.name.clone(), img));
            }
            Some(_) => {
                stats.overflow_defs += 1;
                def_of.insert(p.name.clone(), None);
            }
            None => {
                def_of.insert(p.name.clone(), None);
            }
        }
    }
    let src: Vec<(usize, usize)> = images.iter().map(|(_, i)| (i.w, i.h)).collect();
    let cells = cell_sizes(&src)?;
    let at = pack(&cells)?;
    let mut pix = vec![0u8; ATLAS * ATLAS];
    let mut defs = Vec::new();
    let mut half = Vec::new();
    for (i, (name, img)) in images.iter().enumerate() {
        let (cw, ch) = cells[i];
        let (cx, cy) = at[i];
        let cell = coverage_cell(img, cw, ch);
        for y in 0..ch {
            for x in 0..cw {
                pix[(cy + y) * ATLAS + cx + x] = cell[y * cw + x];
            }
        }
        defs.push(Def {
            name: name.clone(),
            src: (img.w, img.h),
            cell: (cx, cy, cw, ch),
            tint: [img.tint.0, img.tint.1, img.tint.2],
        });
        let hw = ((img.w as f64 / 2.0) / scale as f64)
            .round()
            .clamp(1.0, 255.0) as u8;
        let hh = ((img.h as f64 / 2.0) / scale as f64)
            .round()
            .clamp(1.0, 255.0) as u8;
        half.push([hw, hh]);
    }

    let mut recs: Vec<(f64, Rec)> = Vec::new();
    let mut level_sum = vec![0.0f64; defs.len()];
    let mut level_n = vec![0usize; defs.len()];
    for p in &placements {
        let Some(Some(def)) = def_of.get(&p.name).copied() else {
            stats.unknown_texture += 1;
            continue;
        };
        let (host, on_mover) = find_host(&lumps, p.origin, &hosts);
        let Some(host) = host.filter(|_| !on_mover) else {
            if on_mover {
                stats.on_movers += 1;
            } else {
                stats.no_host += 1;
            }
            continue;
        };
        let _ = host.outside;
        level_sum[def] += surface_level(g, &host.poly, host.face, p.origin);
        level_n[def] += 1;
        // World-space geometry from here on.
        let proj = sub(p.origin, scale3(host.poly.n, host.dist));
        // Target frame: the surface's own texture axes.
        let s_dir = norm3(to_world3(sub(
            host.poly.s_axis,
            scale3(host.poly.n, dot(host.poly.s_axis, host.poly.n)),
        )));
        let t_dir = norm3(to_world3(sub(
            host.poly.t_axis,
            scale3(host.poly.n, dot(host.poly.t_axis, host.poly.n)),
        )));
        let (r, d) = (s_dir, t_dir);
        // Fit the quad inside the polygon's bounds along (r, d), in HL space.
        let (r_hl, d_hl) = (to_world3(r), to_world3(d)); // swap is its own inverse
        let (ru, rv) = polygon_extent(&host.poly, proj, r_hl, d_hl);
        let [hw, hh] = half[def];
        let (hwf, hhf) = (hw as f64 * scale as f64, hh as f64 * scale as f64);
        let (du, dv) = (
            fit_interval(ru[0], ru[1], hwf),
            fit_interval(rv[0], rv[1], hhf),
        );
        let centre = add(add(proj, scale3(r_hl, du)), scale3(d_hl, dv));
        let lifted = add(centre, scale3(host.poly.n, LIFT));
        let leaf = valid_leaf(&lumps, head0, lifted, host.poly.n, g.n_visleaves);
        let w = to_world3(lifted);
        let pos = [
            (w[0] / scale as f64).round().clamp(-32768.0, 32767.0) as i16,
            (w[1] / scale as f64).round().clamp(-32768.0, 32767.0) as i16,
            (w[2] / scale as f64).round().clamp(-32768.0, 32767.0) as i16,
        ];
        let code =
            def as u16 | ((encode_dir(s_dir) as u16) << 5) | ((encode_dir(t_dir) as u16) << 10);
        if std::env::var_os("L5_DUMP").is_some() {
            // A viewpoint in open space facing the decal, for probe galleries.
            let mut cam = [f64::NAN; 3];
            for d in [110.0, 90.0, 64.0, 48.0, 32.0, 20.0] {
                let c = add(p.origin, scale3(host.poly.n, d));
                let at = point_leaf_from(
                    head0,
                    [c[0] as f32, c[1] as f32, c[2] as f32],
                    lumps.nodes,
                    lumps.planes,
                ) as usize;
                if at > 0 && at <= g.n_visleaves {
                    cam = c;
                    break;
                }
            }
            eprintln!(
                "L5_DUMP {} {:.0} {:.0} {:.0} {:.2} {:.2} {:.2} leaf {} code {:#06x} cam {:.0} {:.0} {:.0}",
                p.name, p.origin[0], p.origin[1], p.origin[2], host.poly.n[0], host.poly.n[1], host.poly.n[2], leaf, code, cam[0], cam[1], cam[2]
            );
        }
        let from_spawn = spawn_hl
            .map(|s| {
                let d = sub(p.origin, [s[0] as f64, s[1] as f64, s[2] as f64]);
                dot(d, d)
            })
            .unwrap_or(0.0);
        recs.push((from_spawn, Rec { pos, leaf, code }));
    }
    for (i, def) in defs.iter_mut().enumerate() {
        let level = if level_n[i] == 0 {
            1.5
        } else {
            level_sum[i] / level_n[i] as f64
        };
        for c in &mut def.tint {
            *c = (*c as f64 * level / 2.0).round().clamp(0.0, 127.0) as u8;
        }
    }
    stats.placed = recs.len();
    if let Some(budget) = budget {
        // Keep the decals nearest the spawn; the definitions stay (they are
        // a few bytes each) so the survivors' indices still resolve.
        let fixed = SECTION_HEADER + defs.len() * DEF_BYTES;
        let keep = budget.saturating_sub(fixed) / DECAL_BYTES;
        if recs.len() > keep {
            recs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            stats.dropped_budget = recs.len() - keep;
            recs.truncate(keep);
            stats.placed = recs.len();
        }
    }
    // Stable order by leaf so the runtime's PVS test walks memory forward.
    recs.sort_by_key(|(_, r)| r.leaf);
    let recs: Vec<Rec> = recs.into_iter().map(|(_, r)| r).collect();
    if recs.is_empty() {
        return None;
    }

    // The atlas: 4 bit, white CLUT. Entry 0 is the transparent texel, 1..7
    // carry the semi-transparency bit (half coverage), 8..15 are opaque.
    let mut clut = [0u16; 16];
    for (i, e) in clut.iter_mut().enumerate().skip(1) {
        *e = if i < 8 { 0xffff } else { 0x7fff };
    }
    let mut pix4 = Vec::with_capacity(ATLAS * ATLAS / 2);
    for pair in pix.chunks(2) {
        pix4.push(pair[0] | (pair[1] << 4));
    }
    Some((
        DecalCook {
            defs,
            half,
            recs,
            stats,
        },
        CookedTex {
            w: ATLAS as u16,
            h: ATLAS as u16,
            clut,
            pix4,
            eight: None,
        },
    ))
}

/// The BSP leaf (== PVS bit + 1) holding `p`, nudged along the surface normal
/// when the first probe lands in solid. 0 means "no PVS test".
fn valid_leaf(l: &Lumps, head: i32, p: V3, n: V3, n_visleaves: usize) -> u16 {
    for extra in [0.0, 2.0, 6.0, 14.0] {
        let q = add(p, scale3(n, extra));
        let leaf = point_leaf_from(
            head,
            [q[0] as f32, q[1] as f32, q[2] as f32],
            l.nodes,
            l.planes,
        ) as usize;
        if leaf > 0 && leaf <= n_visleaves {
            return leaf as u16;
        }
    }
    let _ = l.leaves;
    0
}

/// One switchable or animated lightstyle slot of a room (slot k is record k-1).
pub struct StyleRecord {
    /// A targeted light: the runtime's switch mask decides whether it is on.
    /// Otherwise (the standard animated styles) it always plays.
    pub switchable: bool,
    /// Level at map start, 256 = "m"; what the baked lighting already holds.
    pub start: u32,
    /// Pattern letters, empty for steady "m".
    pub pattern: Vec<u8>,
}

/// The light-style table: `u8 n, 3 pad`, then per style `u8 flags (bit 0
/// switchable), u8 pattern length, u16 start level, u16 pattern offset` (from
/// the table start), then the pattern letters, padded to 4.
pub fn style_table(styles: &[StyleRecord]) -> Vec<u8> {
    if styles.is_empty() {
        return Vec::new();
    }
    let mut o = vec![styles.len() as u8, 0, 0, 0];
    let mut pat_off = 4 + styles.len() * 6;
    for s in styles {
        o.push(s.switchable as u8);
        o.push(s.pattern.len() as u8);
        o.extend_from_slice(&(s.start.min(u16::MAX as u32) as u16).to_le_bytes());
        o.extend_from_slice(&(pat_off as u16).to_le_bytes());
        pat_off += s.pattern.len();
    }
    for s in styles {
        o.extend_from_slice(&s.pattern);
    }
    while o.len() % 4 != 0 {
        o.push(0);
    }
    o
}

/// Serialise the section body: header, definitions, decals. `styles` is the
/// already-encoded light-style table (4-aligned) or empty.
pub fn write_section(decals: Option<&DecalCook>, atlas_tex: u8, styles: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(&SECTION_MAGIC);
    let n_decals = decals.map_or(0, |d| d.recs.len()) as u16;
    let n_defs = decals.map_or(0, |d| d.defs.len()) as u8;
    o.extend_from_slice(&n_decals.to_le_bytes());
    o.push(n_defs);
    o.push(if decals.is_some() { atlas_tex } else { 0xff });
    let style_off_pos = o.len();
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    if let Some(d) = decals {
        for r in &d.recs {
            for c in r.pos {
                o.extend_from_slice(&c.to_le_bytes());
            }
            o.extend_from_slice(&r.leaf.to_le_bytes());
            o.extend_from_slice(&r.code.to_le_bytes());
        }
        for (def, half) in d.defs.iter().zip(&d.half) {
            o.push(def.cell.0 as u8);
            o.push(def.cell.1 as u8);
            o.push(def.cell.2 as u8);
            o.push(def.cell.3 as u8);
            o.push(half[0]);
            o.push(half[1]);
            o.extend_from_slice(&def.tint);
        }
    }
    while o.len() % 4 != 0 {
        o.push(0);
    }
    if !styles.is_empty() {
        let off = o.len() as u16;
        o[style_off_pos..style_off_pos + 2].copy_from_slice(&off.to_le_bytes());
        o.extend_from_slice(styles);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 0.12)
    }

    #[test]
    fn axial_and_chamfer_directions_round_trip_exactly() {
        for v in [
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            norm3([1.0, 0.0, 1.0]),
            norm3([0.0, -1.0, 1.0]),
            norm3([-1.0, 1.0, -1.0]),
        ] {
            let d = decode_dir(encode_dir(v));
            assert!(close(d, v), "{v:?} -> {d:?}");
        }
    }

    #[test]
    fn the_runtime_direction_table_is_q6_of_decode_dir() {
        // game/src/main.rs DIR26 stores 64 / sqrt(nonzero digits) per digit:
        // [0, 64, 45, 37]. Keep that in step with the unit vectors above.
        for (k, len) in [(1, 64.0f64), (2, 45.0), (3, 37.0)] {
            assert!((64.0 / (k as f64).sqrt() - len).abs() < 0.6);
        }
        for code in (0..27u8).filter(|&c| c != 13) {
            let d = decode_dir(code);
            let digits = [
                (code % 3) as i32 - 1,
                ((code / 3) % 3) as i32 - 1,
                (code / 9) as i32 - 1,
            ];
            let k = digits.iter().filter(|&&x| x != 0).count();
            let q6 = [0, 64, 45, 37][k] as f64 / 64.0;
            for i in 0..3 {
                assert!(
                    (d[i] - digits[i] as f64 * q6).abs() < 0.01,
                    "code {code} axis {i}"
                );
            }
        }
    }

    #[test]
    fn odd_slants_snap_within_thirty_degrees() {
        // The worst gap between neighbour directions is under 30 degrees.
        for v in [
            norm3([0.8, 0.0, 0.6]),
            norm3([0.9, 0.3, 0.3]),
            norm3([-0.2, 0.7, 0.68]),
        ] {
            let d = decode_dir(encode_dir(v));
            assert!(dot(v, d) > 0.88, "{v:?} -> {d:?} dot {}", dot(v, d));
        }
    }

    #[test]
    fn packing_shrinks_until_it_fits() {
        let big: Vec<(usize, usize)> = vec![(96, 96); 6];
        let cells = cell_sizes(&big).unwrap();
        assert!(pack(&cells).is_some());
        assert!(cells.iter().any(|&(w, _)| w < 96));
    }

    #[test]
    fn interval_slides_inside_the_polygon() {
        assert_eq!(fit_interval(-100.0, 100.0, 10.0), 0.0);
        assert_eq!(fit_interval(-3.0, 100.0, 10.0), 7.0);
        assert_eq!(fit_interval(-100.0, 4.0, 10.0), -6.0);
        assert_eq!(fit_interval(-4.0, 6.0, 10.0), 1.0);
    }
}
