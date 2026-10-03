//! Synthetic cooked maps for host tests.
//!
//! Builds a minimal, valid `HLMH` room blob from axis-aligned solid boxes so
//! `map::Map::load` and the collision code can run without Half-Life assets.
//! The world is Y-up, in whole units, exactly like a cooked room.
//!
//! Each hull is a chain of box tests: a point is solid when it lies inside a
//! box grown by that hull's half extents. Every box contributes six planes;
//! leaving any plane's inner side jumps to the next box's chain, and the last
//! box falls through to empty space. That is a valid BSP (shared children are
//! fine for point and segment walks), and it keeps the fixture readable.

use hl_format::map as cooked;

/// One solid axis-aligned brush, `min` inclusive of the surface.
#[derive(Clone, Copy, Debug)]
pub struct Brush {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl Brush {
    pub const fn new(min: [i32; 3], max: [i32; 3]) -> Self {
        Self { min, max }
    }
}

/// One exact-route navigation node: origin plus its compressed route bytes.
#[derive(Clone, Debug)]
pub struct NavNodeFixture {
    pub origin: [i32; 3],
    pub node_type: u8,
    pub routes: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct MapFixture {
    pub brushes: Vec<Brush>,
    pub spawn: [i32; 3],
    pub nav: Vec<NavNodeFixture>,
}

/// Standing player hull half extents (x/z, y).
pub const STAND_HALF: [i32; 3] = [16, 36, 16];
/// Crouched player hull half extents.
pub const CROUCH_HALF: [i32; 3] = [16, 18, 16];

const SOLID_CONTENTS: i16 = -2;
const EMPTY_CONTENTS: i16 = -1;
const Q14_ONE: i16 = 1 << 14;

struct Planes {
    records: Vec<([i16; 3], i32)>,
}

impl Planes {
    fn add(&mut self, normal: [i16; 3], dist: i32) -> u16 {
        let dist_q5 = dist * 32;
        if let Some(i) = self
            .records
            .iter()
            .position(|&(n, d)| n == normal && d == dist_q5)
        {
            return i as u16;
        }
        self.records.push((normal, dist_q5));
        (self.records.len() - 1) as u16
    }
}

/// The six outward planes of a box: a point is outside when any plane's
/// signed distance is >= 0 (the front side).
fn box_planes(min: [i32; 3], max: [i32; 3]) -> [([i16; 3], i32); 6] {
    [
        ([-Q14_ONE, 0, 0], -min[0]),
        ([Q14_ONE, 0, 0], max[0]),
        ([0, -Q14_ONE, 0], -min[1]),
        ([0, Q14_ONE, 0], max[1]),
        ([0, 0, -Q14_ONE], -min[2]),
        ([0, 0, Q14_ONE], max[2]),
    ]
}

/// Clip hull for one set of half extents: returns (first node index, nodes),
/// nodes as (plane, front child, back child).
fn clip_hull(
    brushes: &[Brush],
    half: [i32; 3],
    planes: &mut Planes,
    base: usize,
) -> (i32, Vec<(u16, i16, i16)>) {
    let mut nodes = Vec::new();
    if brushes.is_empty() {
        return (EMPTY_CONTENTS as i32, nodes);
    }
    for (b, brush) in brushes.iter().enumerate() {
        let min = [
            brush.min[0] - half[0],
            brush.min[1] - half[1],
            brush.min[2] - half[2],
        ];
        let max = [
            brush.max[0] + half[0],
            brush.max[1] + half[1],
            brush.max[2] + half[2],
        ];
        let next_box = if b + 1 < brushes.len() {
            (base + (b + 1) * 6) as i16
        } else {
            EMPTY_CONTENTS
        };
        for (k, (n, d)) in box_planes(min, max).into_iter().enumerate() {
            let plane = planes.add(n, d);
            let inside = if k == 5 {
                SOLID_CONTENTS
            } else {
                (base + b * 6 + k + 1) as i16
            };
            nodes.push((plane, next_box, inside));
        }
    }
    (base as i32, nodes)
}

/// Render-node tree with the same chain shape. Leaf 0 is solid, leaf 1 is
/// open air (child encoding `-(leaf + 1)`).
fn render_tree(brushes: &[Brush], planes: &mut Planes) -> Vec<(u16, i16, i16)> {
    let mut nodes = Vec::new();
    let empty_leaf: i16 = -2; // leaf 1
    let solid_leaf: i16 = -1; // leaf 0
    if brushes.is_empty() {
        // One always-front node keeps the root valid.
        let plane = planes.add([0, Q14_ONE, 0], -1_000_000);
        nodes.push((plane, empty_leaf, empty_leaf));
        return nodes;
    }
    for (b, brush) in brushes.iter().enumerate() {
        let next_box = if b + 1 < brushes.len() {
            ((b + 1) * 6) as i16
        } else {
            empty_leaf
        };
        for (k, (n, d)) in box_planes(brush.min, brush.max).into_iter().enumerate() {
            let plane = planes.add(n, d);
            let inside = if k == 5 {
                solid_leaf
            } else {
                (b * 6 + k + 1) as i16
            };
            nodes.push((plane, next_box, inside));
        }
    }
    nodes
}

struct Blob(Vec<u8>);

impl Blob {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn align4(&mut self) {
        while self.0.len() % 4 != 0 {
            self.0.push(0);
        }
    }
    fn patch_u32(&mut self, at: usize, v: u32) {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}

impl MapFixture {
    /// Cook the fixture to a leaked, `'static` `HLMH` blob.
    pub fn cook(&self) -> &'static [u8] {
        let mut planes = Planes {
            records: Vec::new(),
        };
        let render = render_tree(&self.brushes, &mut planes);
        let (hull1_head, mut clip) = clip_hull(&self.brushes, STAND_HALF, &mut planes, 0);
        let (hull3_head, crouch) = clip_hull(&self.brushes, CROUCH_HALF, &mut planes, clip.len());
        clip.extend(crouch);
        assert!(planes.records.len() < 1 << 14, "fixture plane count");

        let mut b = Blob(Vec::new());
        b.0.extend_from_slice(&cooked::MAGIC_LATEST_BYTES);
        // n_verts, n_tris, n_texs, n_faces, then the eight section offsets and
        // the world-pipeline offset, patched below.
        for _ in 0..13 {
            b.u32(0);
        }
        assert_eq!(b.len(), cooked::HEADER_SIZE);
        // No render vertices; empty loop-vertex pool.
        b.u32(0);
        b.align4();
        // Light palette, then the dynamic-lightmap word (no faces, no alpha).
        for _ in 0..256 {
            b.u16(0);
        }
        b.u16(0);
        b.align4();

        let bsp_off = b.len();
        b.u32(planes.records.len() as u32);
        b.u32(0); // face groups
        b.u32(render.len() as u32);
        let n_leaves = 2u32;
        b.u32(n_leaves | ((n_leaves - 1) << 16));
        b.u32(0); // marks
        b.u32(0); // vis bytes
        for &(n, d) in &planes.records {
            b.i16(n[0]);
            b.i16(n[1]);
            b.i16(n[2]);
            b.i32(d);
        }
        for &(plane, c0, c1) in &render {
            b.u16(plane);
            b.i16(c0);
            b.i16(c1);
        }
        for _ in 0..n_leaves {
            b.i32(-1); // no visibility row
            b.u16(0);
            b.u16(0);
        }
        b.align4();
        b.align4();

        let clip_off = b.len();
        b.u32(clip.len() as u32);
        b.i32(-1);
        b.i32(hull1_head);
        b.i32(hull3_head);
        for axis in 0..3 {
            b.i32(self.spawn[axis]);
        }
        b.i32(0);
        for &(plane, c0, c1) in &clip {
            b.u16(plane); // untagged: generic plane
            b.i16(c0);
            b.i16(c1);
        }
        b.align4();

        let ent_off = b.len();
        b.u32(0); // models
        b.u32(0); // entities
        b.u32(0); // entity leaves
        b.align4();

        let tram_off = b.len();
        b.u16(0);
        b.u16(0);
        b.u32(0);
        b.i32(-1);
        for _ in 0..3 {
            b.i32(0);
        }

        let prop_off = b.len();
        b.u32(0);

        let nav_off = b.len();
        if self.nav.is_empty() {
            b.u16(0);
            b.u16(0);
        } else {
            let route_bytes: usize = self.nav.iter().map(|n| n.routes.len()).sum();
            assert!(route_bytes < 0x8000);
            b.u16(self.nav.len() as u16);
            b.u16(0x8000 | route_bytes as u16);
            let mut route_off = 0usize;
            for node in &self.nav {
                for axis in 0..3 {
                    b.i32(node.origin[axis]);
                }
                b.i16(1);
                b.u16(route_off as u16);
                b.u8(node.node_type);
                b.u8(0);
                route_off += node.routes.len();
            }
            for node in &self.nav {
                b.0.extend_from_slice(&node.routes);
            }
        }
        b.align4();

        let logic_off = b.len();
        b.u16(0);
        b.u16(0);
        b.u16(0);
        b.u16(0);
        b.align4();
        // Texture-animation chains: none.
        b.u16(0);
        b.u16(0);
        // Trailing padding so no optional end-of-file section tag matches.
        b.u32(0);
        b.u32(0);

        let header = |slot: usize| 4 + slot * 4;
        b.patch_u32(header(4), bsp_off as u32);
        b.patch_u32(header(5), clip_off as u32);
        b.patch_u32(header(6), ent_off as u32);
        b.patch_u32(header(7), tram_off as u32);
        b.patch_u32(header(8), prop_off as u32);
        b.patch_u32(header(9), u32::MAX); // no sky texture
        b.patch_u32(header(10), nav_off as u32);
        b.patch_u32(header(11), logic_off as u32);
        Box::leak(b.0.into_boxed_slice())
    }

    pub fn load(&self) -> crate::map::Map {
        crate::map::Map::load(self.cook())
    }
}
