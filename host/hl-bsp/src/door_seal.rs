//! Cook-time door occlusion: which world leaves a closed brush entity seals off.
//!
//! GoldSrc compiles doors as separate brush models, so the world's leaf PVS
//! cannot know that a closed blast door hides the room behind it. This pass
//! rebuilds the world's leaf portals from the hull-0 BSP, grafts one brush
//! model's own hull-0 tree into the world leaves its bounds touch, and floods
//! the resulting cell graph. A portal fragment inside the model's solid is
//! impassable, so when the model sits at its authored transform every sight
//! line from one flood component to another passes through it.
//!
//! The result is exact with respect to portal visibility, the notion the PVS
//! already uses: a leaf is hidden only when no chain of open portals reaches
//! it. Any gap between the door and its frame leaves a portal fragment open,
//! and then the door simply produces no record.

use std::collections::HashMap;

type V3 = [f64; 3];

const SZ_PLANE: usize = 20;
const SZ_NODE: usize = 24;
const SZ_LEAF: usize = 28;
const SZ_MODEL: usize = 64;
const SZ_FACE: usize = 20;
const SZ_TEXINFO: usize = 40;
const CONTENTS_SOLID: i32 = -2;

/// Vertices closer than this to a plane count as on it.
const ON_EPSILON: f64 = 0.01;
/// Portal fragments at or below this area connect nothing. Keeping every
/// larger sliver only ever adds connectivity, which is the safe direction.
const MIN_PORTAL_AREA: f64 = 0.001;
/// Offset from a portal fragment's centroid used to classify each side.
const SIDE_PROBE: f64 = 0.05;
/// Half-size of the initial winding on each splitting plane.
const BASE_WINDING: f64 = 65536.0;

#[derive(Clone, Copy)]
struct Plane {
    n: V3,
    d: f64,
}

impl Plane {
    fn dist(&self, p: V3) -> f64 {
        dot(self.n, p) - self.d
    }
    fn flipped(self) -> Plane {
        Plane {
            n: [-self.n[0], -self.n[1], -self.n[2]],
            d: -self.d,
        }
    }
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add_scaled(a: V3, b: V3, s: f64) -> V3 {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}
fn normalize(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

fn f32le(b: &[u8], o: usize) -> f64 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as f64
}
fn i32le(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn i16le(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

/// The raw BSP lumps this pass reads.
pub struct Lumps<'a> {
    pub planes: &'a [u8],
    pub nodes: &'a [u8],
    pub leaves: &'a [u8],
    pub models: &'a [u8],
    pub faces: &'a [u8],
    pub texinfo: &'a [u8],
    pub textures: &'a [u8],
}

/// Tree child reference: `Ok(node)` or `Err(leaf)`.
type Child = Result<usize, usize>;

impl Lumps<'_> {
    fn plane(&self, i: usize) -> Plane {
        let o = i * SZ_PLANE;
        Plane {
            n: [
                f32le(self.planes, o),
                f32le(self.planes, o + 4),
                f32le(self.planes, o + 8),
            ],
            d: f32le(self.planes, o + 12),
        }
    }
    fn node_plane(&self, node: usize) -> Plane {
        self.plane(i32le(self.nodes, node * SZ_NODE).max(0) as usize)
    }
    fn child(&self, node: usize, side: usize) -> Child {
        let c = i16le(self.nodes, node * SZ_NODE + 4 + side * 2) as i32;
        if c >= 0 {
            Ok(c as usize)
        } else {
            Err((-c - 1) as usize)
        }
    }
    fn n_leaves(&self) -> usize {
        self.leaves.len() / SZ_LEAF
    }
    fn contents(&self, leaf: usize) -> i32 {
        if leaf >= self.n_leaves() {
            return CONTENTS_SOLID;
        }
        i32le(self.leaves, leaf * SZ_LEAF)
    }
    fn solid(&self, leaf: usize) -> bool {
        self.contents(leaf) == CONTENTS_SOLID
    }
    fn model_bounds(&self, m: usize) -> (V3, V3) {
        let o = m * SZ_MODEL;
        (
            [
                f32le(self.models, o),
                f32le(self.models, o + 4),
                f32le(self.models, o + 8),
            ],
            [
                f32le(self.models, o + 12),
                f32le(self.models, o + 16),
                f32le(self.models, o + 20),
            ],
        )
    }
    fn model_head0(&self, m: usize) -> Child {
        let h = i32le(self.models, m * SZ_MODEL + 36);
        if h >= 0 {
            Ok(h as usize)
        } else {
            Err((-h - 1) as usize)
        }
    }
    fn texture_name(&self, texinfo: usize) -> String {
        let t = texinfo * SZ_TEXINFO;
        if t + SZ_TEXINFO > self.texinfo.len() {
            return String::new();
        }
        let miptex = i32le(self.texinfo, t + 32);
        if miptex < 0 || self.textures.len() < 4 {
            return String::new();
        }
        let count = i32le(self.textures, 0).max(0) as usize;
        let miptex = miptex as usize;
        if miptex >= count || 4 + miptex * 4 + 4 > self.textures.len() {
            return String::new();
        }
        let ofs = i32le(self.textures, 4 + miptex * 4);
        if ofs < 0 {
            return String::new();
        }
        let o = ofs as usize;
        let end = (o + 16).min(self.textures.len());
        let raw = &self.textures[o.min(end)..end];
        let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..len]).to_ascii_lowercase()
    }
    /// Every face of the model draws opaque: no cutout, liquid, sky or tool
    /// texture that could leave a hole where the solid is.
    fn model_opaque(&self, m: usize) -> bool {
        let o = m * SZ_MODEL;
        let first = i32le(self.models, o + 56).max(0) as usize;
        let count = i32le(self.models, o + 60).max(0) as usize;
        if count == 0 {
            return false;
        }
        for f in first..first + count {
            let fo = f * SZ_FACE;
            if fo + SZ_FACE > self.faces.len() {
                return false;
            }
            let name = self.texture_name(u16le(self.faces, fo + 10) as usize);
            if name.is_empty()
                || name.starts_with('{')
                || name.starts_with('!')
                || name.starts_with('*')
                || name.starts_with("sky")
                || name.starts_with("aaatrigger")
                || name.starts_with("clip")
                || name.starts_with("null")
                || name.starts_with("origin")
                || name.starts_with("hint")
                || name.starts_with("skip")
                || name.starts_with("black")
            {
                return false;
            }
        }
        true
    }
}

fn base_winding(p: Plane) -> Vec<V3> {
    let a = p.n.map(f64::abs);
    let up = if a[2] >= a[0] && a[2] >= a[1] {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let u = normalize(cross(up, p.n));
    let v = cross(p.n, u);
    let c = [p.n[0] * p.d, p.n[1] * p.d, p.n[2] * p.d];
    let s = BASE_WINDING;
    vec![
        add_scaled(add_scaled(c, u, -s), v, s),
        add_scaled(add_scaled(c, u, s), v, s),
        add_scaled(add_scaled(c, u, s), v, -s),
        add_scaled(add_scaled(c, u, -s), v, -s),
    ]
}

enum Split {
    Front,
    Back,
    On,
    Both(Vec<V3>, Vec<V3>),
}

fn split(w: &[V3], p: Plane) -> Split {
    let d: Vec<f64> = w.iter().map(|&v| p.dist(v)).collect();
    let front = d.iter().any(|&x| x > ON_EPSILON);
    let back = d.iter().any(|&x| x < -ON_EPSILON);
    match (front, back) {
        (false, false) => return Split::On,
        (true, false) => return Split::Front,
        (false, true) => return Split::Back,
        _ => {}
    }
    let mut f = Vec::new();
    let mut b = Vec::new();
    for i in 0..w.len() {
        let (p1, d1) = (w[i], d[i]);
        if d1 >= -ON_EPSILON {
            f.push(p1);
        }
        if d1 <= ON_EPSILON {
            b.push(p1);
        }
        let j = (i + 1) % w.len();
        let (p2, d2) = (w[j], d[j]);
        if (d1 > ON_EPSILON && d2 < -ON_EPSILON) || (d1 < -ON_EPSILON && d2 > ON_EPSILON) {
            let t = d1 / (d1 - d2);
            let mid = add_scaled(p1, sub(p2, p1), t);
            f.push(mid);
            b.push(mid);
        }
    }
    Split::Both(f, b)
}

/// Keep the part of `w` on the front of `p` (on-plane windings are kept).
fn clip_front(w: Vec<V3>, p: Plane) -> Option<Vec<V3>> {
    match split(&w, p) {
        Split::Front | Split::On => Some(w),
        Split::Back => None,
        Split::Both(f, _) => Some(f),
    }
}

fn area(w: &[V3]) -> f64 {
    let mut total = [0.0; 3];
    for i in 1..w.len().saturating_sub(1) {
        let c = cross(sub(w[i], w[0]), sub(w[i + 1], w[0]));
        total = [total[0] + c[0], total[1] + c[1], total[2] + c[2]];
    }
    0.5 * dot(total, total).sqrt()
}

fn centroid(w: &[V3]) -> V3 {
    let mut c = [0.0; 3];
    for v in w {
        c = [c[0] + v[0], c[1] + v[1], c[2] + v[2]];
    }
    let n = w.len() as f64;
    [c[0] / n, c[1] / n, c[2] / n]
}

/// A BSP tree over the shared node array, optionally translated.
#[derive(Clone, Copy)]
struct Tree<'a> {
    lumps: &'a Lumps<'a>,
    head: Child,
    offset: V3,
}

impl Tree<'_> {
    fn plane(&self, node: usize) -> Plane {
        let p = self.lumps.node_plane(node);
        Plane {
            n: p.n,
            d: p.d + dot(p.n, self.offset),
        }
    }
    fn leaf_at(&self, point: V3) -> usize {
        let mut c = self.head;
        let mut guard = 0;
        while let Ok(node) = c {
            guard += 1;
            if guard > 4096 {
                return 0;
            }
            let side = if self.plane(node).dist(point) >= 0.0 {
                0
            } else {
                1
            };
            c = self.lumps.child(node, side);
        }
        c.unwrap_err()
    }

    /// Split `w` into pieces that each lie in one leaf. `region` is a point
    /// direction telling which side of `w`'s own plane the caller's region is
    /// on, used when `w` is coplanar with a tree plane.
    fn filter(&self, c: Child, w: Vec<V3>, region: Option<V3>, out: &mut Vec<(usize, Vec<V3>)>) {
        match c {
            Err(leaf) => out.push((leaf, w)),
            Ok(node) => {
                let p = self.plane(node);
                match split(&w, p) {
                    Split::Front => self.filter(self.lumps.child(node, 0), w, region, out),
                    Split::Back => self.filter(self.lumps.child(node, 1), w, region, out),
                    Split::Both(f, b) => {
                        self.filter(self.lumps.child(node, 0), f, region, out);
                        self.filter(self.lumps.child(node, 1), b, region, out);
                    }
                    Split::On => match region {
                        Some(r) => {
                            let side = if dot(p.n, r) > 0.0 { 0 } else { 1 };
                            self.filter(self.lumps.child(node, side), w, region, out);
                        }
                        None => {
                            // Refine by both subtrees so each piece classifies
                            // uniformly from either side of the plane.
                            let mut front = Vec::new();
                            self.filter(self.lumps.child(node, 0), w, None, &mut front);
                            for (_, piece) in front {
                                self.filter(self.lumps.child(node, 1), piece, None, out);
                            }
                        }
                    },
                }
            }
        }
    }

    /// Portals between every pair of adjacent leaves, clipped to `bounds`.
    /// Each entry is (back leaf, front leaf, normal pointing back->front, winding).
    fn portals(&self, bounds: &[Plane]) -> Vec<(usize, usize, V3, Vec<V3>)> {
        let mut out = Vec::new();
        let mut path: Vec<Plane> = bounds.to_vec();
        if let Ok(node) = self.head {
            self.portals_r(node, &mut path, &mut out);
        }
        out
    }

    fn portals_r(
        &self,
        node: usize,
        path: &mut Vec<Plane>,
        out: &mut Vec<(usize, usize, V3, Vec<V3>)>,
    ) {
        let p = self.plane(node);
        let mut w = Some(base_winding(p));
        for &half in path.iter() {
            w = w.and_then(|w| clip_front(w, half));
            if w.is_none() {
                break;
            }
        }
        if let Some(w) = w.filter(|w| w.len() >= 3 && area(w) > MIN_PORTAL_AREA) {
            let mut front = Vec::new();
            self.filter(self.lumps.child(node, 0), w, Some(p.n), &mut front);
            for (fl, fw) in front {
                if self.lumps.solid(fl) {
                    continue;
                }
                let mut back = Vec::new();
                let neg = [-p.n[0], -p.n[1], -p.n[2]];
                self.filter(self.lumps.child(node, 1), fw, Some(neg), &mut back);
                for (bl, bw) in back {
                    if !self.lumps.solid(bl) && bw.len() >= 3 && area(&bw) > MIN_PORTAL_AREA {
                        out.push((bl, fl, p.n, bw));
                    }
                }
            }
        }
        for side in 0..2 {
            if let Ok(child) = self.lumps.child(node, side) {
                path.push(if side == 0 { p } else { p.flipped() });
                self.portals_r(child, path, out);
                path.pop();
            }
        }
    }

    /// For every leaf reached, the half-spaces bounding it (tree path only).
    fn leaf_paths(&self) -> HashMap<usize, Vec<(usize, u8)>> {
        let mut out = HashMap::new();
        let mut path = Vec::new();
        self.leaf_paths_r(self.head, &mut path, &mut out);
        out
    }
    fn leaf_paths_r(
        &self,
        c: Child,
        path: &mut Vec<(usize, u8)>,
        out: &mut HashMap<usize, Vec<(usize, u8)>>,
    ) {
        match c {
            Err(leaf) => {
                out.entry(leaf).or_insert_with(|| path.clone());
            }
            Ok(node) => {
                for side in 0..2u8 {
                    path.push((node, side));
                    self.leaf_paths_r(self.lumps.child(node, side as usize), path, out);
                    path.pop();
                }
            }
        }
    }
    fn path_planes(&self, path: &[(usize, u8)]) -> Vec<Plane> {
        path.iter()
            .map(|&(node, side)| {
                let p = self.plane(node);
                if side == 0 {
                    p
                } else {
                    p.flipped()
                }
            })
            .collect()
    }
}

fn leaves_in_box(lumps: &Lumps, c: Child, mins: V3, maxs: V3, out: &mut Vec<usize>) {
    match c {
        Err(leaf) => {
            if !out.contains(&leaf) {
                out.push(leaf);
            }
        }
        Ok(node) => {
            let p = lumps.node_plane(node);
            let mut front = 0.0;
            let mut back = 0.0;
            for i in 0..3 {
                let (hi, lo) = if p.n[i] >= 0.0 {
                    (maxs[i], mins[i])
                } else {
                    (mins[i], maxs[i])
                };
                front += p.n[i] * hi;
                back += p.n[i] * lo;
            }
            if front >= p.d {
                leaves_in_box(lumps, lumps.child(node, 0), mins, maxs, out);
            }
            if back < p.d {
                leaves_in_box(lumps, lumps.child(node, 1), mins, maxs, out);
            }
        }
    }
}

struct UnionFind(Vec<usize>);
impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind((0..n).collect())
    }
    fn find(&mut self, mut a: usize) -> usize {
        while self.0[a] != a {
            self.0[a] = self.0[self.0[a]];
            a = self.0[a];
        }
        a
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }
}

/// World portals, computed once per map.
pub struct WorldPortals {
    portals: Vec<(usize, usize, V3, Vec<V3>)>,
    bounds: Vec<Plane>,
    leaf_paths: HashMap<usize, Vec<(usize, u8)>>,
    n_visleaves: usize,
}

pub fn world_portals(lumps: &Lumps, n_visleaves: usize) -> WorldPortals {
    let (mins, maxs) = lumps.model_bounds(0);
    let pad = 64.0;
    let mut bounds = Vec::new();
    for i in 0..3 {
        let mut n = [0.0; 3];
        n[i] = 1.0;
        bounds.push(Plane {
            n,
            d: mins[i] - pad,
        });
        n[i] = -1.0;
        bounds.push(Plane {
            n,
            d: -(maxs[i] + pad),
        });
    }
    let world = Tree {
        lumps,
        head: Ok(0),
        offset: [0.0; 3],
    };
    WorldPortals {
        portals: world.portals(&bounds),
        bounds,
        leaf_paths: world.leaf_paths(),
        n_visleaves,
    }
}

/// A sealing record for one brush entity at its authored transform.
#[derive(Debug, Clone)]
pub struct DoorSeal {
    /// Brush entities that must all sit at their authored transform.
    pub entities: Vec<u16>,
    /// Leaves (1-based world leaf ids) reachable only on side A.
    pub only_a: Vec<u16>,
    /// Leaves reachable only on side B.
    pub only_b: Vec<u16>,
    /// Leaves holding both sides; never hidden, and never sealing.
    pub straddling: usize,
}

/// One brush model placed at `offset` (HL units).
#[derive(Clone, Copy, Debug)]
pub struct Member {
    pub entity: u16,
    pub model: usize,
    pub offset: V3,
}

/// Model bounds at its placement.
pub fn member_bounds(lumps: &Lumps, m: &Member) -> (V3, V3) {
    let (mins, maxs) = lumps.model_bounds(m.model);
    (
        add_scaled(mins, m.offset, 1.0),
        add_scaled(maxs, m.offset, 1.0),
    )
}

pub fn member_opaque(lumps: &Lumps, m: &Member) -> bool {
    m.model != 0 && lumps.model_opaque(m.model)
}

/// Leaf of every member tree at `p`, or None inside any member's solid.
fn classify(lumps: &Lumps, trees: &[Tree], p: V3) -> Option<Vec<usize>> {
    let mut sig = Vec::with_capacity(trees.len());
    for t in trees {
        let k = t.leaf_at(p);
        if lumps.solid(k) {
            return None;
        }
        sig.push(k);
    }
    Some(sig)
}

/// Split `w` by every tree except `skip`.
fn split_by(trees: &[Tree], skip: usize, w: Vec<V3>) -> Vec<Vec<V3>> {
    let mut pieces = vec![w];
    for (i, t) in trees.iter().enumerate() {
        if i == skip {
            continue;
        }
        let mut next = Vec::new();
        for piece in pieces {
            let mut out = Vec::new();
            t.filter(t.head, piece, None, &mut out);
            next.extend(out.into_iter().map(|(_, w)| w));
        }
        pieces = next;
    }
    pieces
        .into_iter()
        .filter(|w| w.len() >= 3 && area(w) > MIN_PORTAL_AREA)
        .collect()
}

/// Analyse the union of `members`, all at their authored placement.
pub fn analyse(lumps: &Lumps, world: &WorldPortals, members: &[Member]) -> Option<DoorSeal> {
    if members.is_empty() || !members.iter().all(|m| member_opaque(lumps, m)) {
        return None;
    }
    let trees: Vec<Tree> = members
        .iter()
        .map(|m| Tree {
            lumps,
            head: lumps.model_head0(m.model),
            offset: m.offset,
        })
        .collect();
    let mut grafted = Vec::new();
    for m in members {
        let (mins, maxs) = member_bounds(lumps, m);
        leaves_in_box(
            lumps,
            Ok(0),
            add_scaled(mins, [1.0; 3], -1.0),
            add_scaled(maxs, [1.0; 3], 1.0),
            &mut grafted,
        );
    }
    grafted.retain(|&l| l != 0 && !lumps.solid(l));
    grafted.sort_unstable();
    if grafted.is_empty() {
        return None;
    }
    let is_grafted = |l: usize| grafted.binary_search(&l).is_ok();

    // Graph nodes: world leaf ids, then one node per (grafted leaf, member
    // leaf signature) sub-cell.
    let n_leaves = lumps.n_leaves();
    let mut sub_ids: HashMap<(usize, Vec<usize>), usize> = HashMap::new();
    let mut edges: Vec<(usize, usize)> = Vec::new();
    let mut base_edges: Vec<(usize, usize)> = Vec::new();
    let node_of =
        |l: usize, sig: Vec<usize>, sub_ids: &mut HashMap<(usize, Vec<usize>), usize>| -> usize {
            if !is_grafted(l) {
                return l;
            }
            let next = n_leaves + sub_ids.len();
            *sub_ids.entry((l, sig)).or_insert(next)
        };

    for (a, b, n, w) in &world.portals {
        base_edges.push((*a, *b));
        if !is_grafted(*a) && !is_grafted(*b) {
            edges.push((*a, *b));
            continue;
        }
        for piece in split_by(&trees, usize::MAX, w.clone()) {
            let c = centroid(&piece);
            let (Some(sa), Some(sb)) = (
                classify(lumps, &trees, add_scaled(c, *n, -SIDE_PROBE)),
                classify(lumps, &trees, add_scaled(c, *n, SIDE_PROBE)),
            ) else {
                continue;
            };
            let na = node_of(*a, sa, &mut sub_ids);
            let nb = node_of(*b, sb, &mut sub_ids);
            edges.push((na, nb));
        }
    }

    // Connectivity inside each grafted leaf across the members' own planes:
    // every face between two union sub-cells lies on some member's portal.
    let world_tree = Tree {
        lumps,
        head: Ok(0),
        offset: [0.0; 3],
    };
    let halves: Vec<(usize, Vec<Plane>)> = grafted
        .iter()
        .filter_map(|&l| {
            world
                .leaf_paths
                .get(&l)
                .map(|path| (l, world_tree.path_planes(path)))
        })
        .collect();
    for (ti, t) in trees.iter().enumerate() {
        for (_, _, n, w) in t.portals(&world.bounds) {
            for piece in split_by(&trees, ti, w) {
                for (l, hs) in &halves {
                    let mut clipped = Some(piece.clone());
                    for &h in hs {
                        clipped = clipped.and_then(|w| clip_front(w, h));
                        if clipped.is_none() {
                            break;
                        }
                    }
                    let Some(cw) = clipped.filter(|w| w.len() >= 3 && area(w) > MIN_PORTAL_AREA)
                    else {
                        continue;
                    };
                    let c = centroid(&cw);
                    let (Some(sa), Some(sb)) = (
                        classify(lumps, &trees, add_scaled(c, n, -SIDE_PROBE)),
                        classify(lumps, &trees, add_scaled(c, n, SIDE_PROBE)),
                    ) else {
                        continue;
                    };
                    let na = node_of(*l, sa, &mut sub_ids);
                    let nb = node_of(*l, sb, &mut sub_ids);
                    edges.push((na, nb));
                }
            }
        }
    }

    let total = n_leaves + sub_ids.len();
    let mut base = UnionFind::new(n_leaves);
    for &(a, b) in &base_edges {
        base.union(a, b);
    }
    let mut cut = UnionFind::new(total);
    for &(a, b) in &edges {
        cut.union(a, b);
    }

    // Flood components of each world leaf.
    let last_vis = world.n_visleaves.min(n_leaves - 1);
    let mut leaf_comps: Vec<Vec<usize>> = vec![Vec::new(); n_leaves];
    for l in 1..=last_vis {
        if lumps.solid(l) || is_grafted(l) {
            continue;
        }
        leaf_comps[l].push(cut.find(l));
    }
    for ((l, _), &id) in &sub_ids {
        let c = cut.find(id);
        if !leaf_comps[*l].contains(&c) {
            leaf_comps[*l].push(c);
        }
    }

    // Only the baseline component holding the members matters.
    let base_comp = base.find(grafted[0]);
    let mut comp_leaves: HashMap<usize, usize> = HashMap::new();
    for l in 1..=last_vis {
        if lumps.solid(l) || base.find(l) != base_comp {
            continue;
        }
        for &c in &leaf_comps[l] {
            *comp_leaves.entry(c).or_default() += 1;
        }
    }
    if comp_leaves.len() < 2 {
        return None;
    }
    // Two groups: the largest component, and everything else. Merging
    // components only makes a camera side see more, never less.
    let largest = *comp_leaves
        .iter()
        .max_by_key(|&(c, n)| (*n, usize::MAX - *c))
        .unwrap()
        .0;
    let group = |c: usize| -> u8 {
        if c == largest {
            0
        } else {
            1
        }
    };

    let mut seal = DoorSeal {
        entities: members.iter().map(|m| m.entity).collect(),
        only_a: Vec::new(),
        only_b: Vec::new(),
        straddling: 0,
    };
    for l in 1..=last_vis {
        if lumps.solid(l) || base.find(l) != base_comp || leaf_comps[l].is_empty() {
            continue;
        }
        let mut groups = 0u8;
        for &c in &leaf_comps[l] {
            groups |= 1 << group(c);
        }
        match groups {
            1 => seal.only_a.push(l as u16),
            2 => seal.only_b.push(l as u16),
            // Both sides meet inside this leaf (it straddles the door):
            // never hidden, and a camera here leaves the record unsealed.
            _ => seal.straddling += 1,
        }
    }
    if seal.only_a.is_empty() || seal.only_b.is_empty() {
        return None;
    }
    Some(seal)
}

/// Collapse a sorted leaf list into (first, count) runs.
pub fn runs(leaves: &[u16]) -> Vec<(u16, u16)> {
    let mut out: Vec<(u16, u16)> = Vec::new();
    for &l in leaves {
        match out.last_mut() {
            Some((first, count)) if *first as u32 + *count as u32 == l as u32 => *count += 1,
            _ => out.push((l, 1)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two rooms joined by a doorway (x 100..200, y 25..75, z 0..75), and a
    /// door brush model in the doorway spanning y `door_y0`..75.
    fn doorway_map(door_y0: f32) -> Vec<Vec<u8>> {
        let mut planes = Vec::new();
        let mut plane = |n: [f32; 3], d: f32| -> i32 {
            for c in n {
                planes.extend_from_slice(&c.to_le_bytes());
            }
            planes.extend_from_slice(&d.to_le_bytes());
            planes.extend_from_slice(&0i32.to_le_bytes());
            (planes.len() / SZ_PLANE - 1) as i32
        };
        let x = [1.0, 0.0, 0.0];
        let nx = [-1.0, 0.0, 0.0];
        let y = [0.0, 1.0, 0.0];
        let ny = [0.0, -1.0, 0.0];
        let z = [0.0, 0.0, 1.0];
        let nz = [0.0, 0.0, -1.0];
        // (plane, front child, back child); children: node index, or -(leaf + 1).
        let solid = -1i16;
        let leaf = |l: i16| -(l + 1);
        let world = [
            (plane(x, 0.0), 1, solid),
            (plane(nx, -300.0), 2, solid),
            (plane(y, 0.0), 3, solid),
            (plane(ny, -100.0), 4, solid),
            (plane(z, 0.0), 5, solid),
            (plane(nz, -100.0), 6, solid),
            (plane(x, 100.0), 7, leaf(1)),
            (plane(x, 200.0), leaf(3), 8),
            (plane(y, 25.0), 9, solid),
            (plane(ny, -75.0), 10, solid),
            (plane(nz, -75.0), leaf(2), solid),
            // Door model, head node 11.
            (plane(x, 140.0), 12, leaf(4)),
            (plane(nx, -160.0), 13, leaf(5)),
            (plane(y, door_y0), 14, leaf(6)),
            (plane(ny, -75.0), 15, leaf(7)),
            (plane(z, 0.0), 16, leaf(8)),
            (plane(nz, -75.0), solid, leaf(9)),
        ];
        let mut nodes = Vec::new();
        for (p, f, b) in world {
            nodes.extend_from_slice(&p.to_le_bytes());
            nodes.extend_from_slice(&(f as i16).to_le_bytes());
            nodes.extend_from_slice(&(b as i16).to_le_bytes());
            nodes.extend_from_slice(&[0u8; 16]);
        }
        let mut leaves = Vec::new();
        for l in 0..10 {
            let contents: i32 = if l == 0 { CONTENTS_SOLID } else { -1 };
            leaves.extend_from_slice(&contents.to_le_bytes());
            leaves.extend_from_slice(&[0u8; SZ_LEAF - 4]);
        }
        let mut models = Vec::new();
        for (mins, maxs, head, face) in [
            ([0.0f32, 0.0, 0.0], [300.0f32, 100.0, 100.0], 0i32, 0i32),
            ([140.0, door_y0, 0.0], [160.0, 75.0, 75.0], 11, 0),
        ] {
            for c in mins.iter().chain(maxs.iter()).chain([0.0f32; 3].iter()) {
                models.extend_from_slice(&c.to_le_bytes());
            }
            for h in [head, 0, 0, 0, 0] {
                models.extend_from_slice(&h.to_le_bytes());
            }
            models.extend_from_slice(&face.to_le_bytes());
            models.extend_from_slice(&1i32.to_le_bytes());
        }
        let mut faces = vec![0u8; SZ_FACE];
        faces[10..12].copy_from_slice(&0u16.to_le_bytes());
        let mut texinfo = vec![0u8; SZ_TEXINFO];
        texinfo[32..36].copy_from_slice(&0i32.to_le_bytes());
        let mut textures = Vec::new();
        textures.extend_from_slice(&1i32.to_le_bytes());
        textures.extend_from_slice(&8i32.to_le_bytes());
        let mut name = [0u8; 16];
        name[..4].copy_from_slice(b"door");
        textures.extend_from_slice(&name);
        vec![planes, nodes, leaves, models, faces, texinfo, textures]
    }

    fn seal(door_y0: f32) -> Option<DoorSeal> {
        let l = doorway_map(door_y0);
        let lumps = Lumps {
            planes: &l[0],
            nodes: &l[1],
            leaves: &l[2],
            models: &l[3],
            faces: &l[4],
            texinfo: &l[5],
            textures: &l[6],
        };
        let world = world_portals(&lumps, 3);
        analyse(
            &lumps,
            &world,
            &[Member {
                entity: 7,
                model: 1,
                offset: [0.0; 3],
            }],
        )
    }

    #[test]
    fn a_door_filling_its_doorway_separates_the_rooms() {
        let s = seal(25.0).expect("the closed door seals");
        assert_eq!(s.entities, vec![7]);
        let mut sides = vec![s.only_a.clone(), s.only_b.clone()];
        sides.sort();
        assert_eq!(sides, vec![vec![1], vec![3]]);
        // The doorway leaf holds both sides: never hidden, never sealing.
        assert_eq!(s.straddling, 1);
    }

    #[test]
    fn a_one_unit_gap_beside_the_door_keeps_the_rooms_connected() {
        assert!(seal(26.0).is_none());
    }

    #[test]
    fn runs_collapse_consecutive_leaves() {
        assert_eq!(runs(&[1, 2, 3, 7, 9, 10]), vec![(1, 3), (7, 1), (9, 2)]);
    }
}
