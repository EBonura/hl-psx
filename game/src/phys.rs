//! Player physics on the BSP clip hull -- a fixed-point port of Quake/GoldSrc
//! `SV_RecursiveHullCheck` + a slide-move. The clip hull (`hull1`) is the BSP
//! pre-expanded by the player box, so we trace the player ORIGIN as a point.
//!
//! All world units are i32; plane normals are i16 in 1.3.12 (×4096); fractions
//! along a move are Q0.12 (4096 = full). No floats (no FPU on the PS1).

use psx_math::{int32::isqrt_i32, sincos};

use crate::map::Map;

const SOLID: i16 = -2; // CONTENTS_SOLID
const GROUND_NY: i32 = 2867; // floor if plane normal Y > ~0.7 (×4096)

// Movement constants are HL values converted to per-tick units at the 20 Hz
// sim (X u/s = X/20 u/tick; accelerations scale by dt = 0.05 twice). Every
// number cites its pm_shared.c / SDK source.
//
// sv_gravity 800 u/s^2 (world.cpp:482) -> dv = 800*0.05 = 40 u/s = 2 u/tick
// per tick. NB the discrete fall speed from height h is v = 2*sqrt(h) u/tick,
// exactly HL's sqrt(2*800*h)/20 -- the fall-damage threshold maps 1:1.
const GRAVITY: i32 = 2;

// trigger_gravity zones scale gravity (q12; 4096 = normal). Sticky until the
// next zone or map load, matching GoldSrc's sv_gravity behaviour.
static mut GRAVITY_SCALE: i32 = 4096;

pub fn set_gravity_scale(scale_q12: i32) {
    unsafe { GRAVITY_SCALE = scale_q12.clamp(0, 4096 * 4) };
}

// Long jump module: jumping while moving adds a strong horizontal boost
// (the Xen crossings need it). Persists for the session once picked up.
static mut LONGJUMP: bool = false;

pub fn set_longjump(on: bool) {
    unsafe { LONGJUMP = on };
}

#[inline]
fn gravity_step() -> i32 {
    unsafe { (GRAVITY * GRAVITY_SCALE) >> 12 }
}
const MOVE_SPEED: i32 = 16; // sv_maxspeed 320 u/s (pm_shared.c:2865 clamp)
const JUMP: i32 = 13; // sqrt(2*800*45) = 268 u/s (pm_shared.c:2596); apex ~49 u
const STOP_SPEED: i32 = 5; // sv_stopspeed 100 u/s (PM_Friction floor)
const AIR_WISH_CAP: i32 = 2; // PM_AirAccelerate caps wishspd at 30 u/s (pm_shared.c:1279)
                             // Long jump module (pm_shared.c:2580-93): 350*1.6 = 560 u/s forward,
                             // sqrt(2*800*56) = 299 u/s up, fired by a DUCKED jump while moving.
const LONGJUMP_FWD: i32 = 28;
const LONGJUMP_UP: i32 = 15;
const STEP_DOWN: i32 = 8; // ground probe depth
const MAX_CLIP_PLANES: usize = 5;

#[inline]
fn dot(n: [i16; 3], p: [i32; 3]) -> i32 {
    ((n[0] as i32 * p[0]) + (n[1] as i32 * p[1]) + (n[2] as i32 * p[2])) >> 12
}

/// Exact wrapped equivalent of `(4096 * p) >> 12`, expressed as shifts so a
/// tagged positive axial plane never pays for the R3000's MULT/MFLO pair.
/// Keeping both shifts matters for out-of-range i32 inputs: returning `p`
/// directly would change the existing release-mode wrapping semantics.
#[inline(always)]
fn axial_dot(p: i32) -> i32 {
    p.wrapping_shl(12) >> 12
}

#[inline(always)]
fn plane_delta(cn: &crate::map::ClipNode, p: [i32; 3]) -> i32 {
    let projection = if cn.axis == 0 {
        dot(cn.n, p)
    } else {
        // `axis` comes directly from a two-bit tag, so the non-zero values are
        // exactly 1..=3. Avoid a switch/jump table in every tagged node visit.
        let coord = unsafe { *p.get_unchecked(cn.axis as usize - 1) };
        axial_dot(coord)
    };
    projection.wrapping_sub(cn.dist)
}

/// Materialize a tagged axial normal only when a full trace actually impacts.
/// Point-content and LOS walks never need to fetch or construct one.
#[inline(always)]
fn plane_normal(cn: &crate::map::ClipNode) -> [i32; 3] {
    match cn.axis {
        1 => [4096, 0, 0],
        2 => [0, 4096, 0],
        3 => [0, 0, 4096],
        _ => [cn.n[0] as i32, cn.n[1] as i32, cn.n[2] as i32],
    }
}

struct Trace {
    frac: i32,        // Q0.12 along the move (4096 = reached end)
    normal: [i32; 3], // hit plane normal (×4096)
    allsolid: bool,
    startsolid: bool,
    mover: i32, // ent id of the mover hit (-1 = static world)
}

/// Public, allocation-free result for gameplay ray casts.
#[derive(Clone, Copy)]
pub struct RayHit {
    /// Q0.12 fraction along the segment.
    pub frac: i32,
    /// World-space impact point.
    pub pos: [i32; 3],
    /// Impact plane normal in Q0.12.
    pub normal: [i32; 3],
    /// Ent id of the brush entity hit, or -1 for static world.
    pub mover: i32,
}

fn point_contents(map: &Map, mut num: i16, p: [i32; 3]) -> i16 {
    let mut guard = 0;
    while num >= 0 {
        if num as usize >= map.n_clip || guard > 256 {
            return -1; // treat as empty on bad data
        }
        guard += 1;
        let cn = map.clipnode(num as usize);
        let t = plane_delta(&cn, p);
        num = if t >= 0 { cn.c0 } else { cn.c1 };
    }
    num
}

fn recurse(
    map: &Map,
    mut num: i16,
    p1f: i32,
    p2f: i32,
    p1: [i32; 3],
    p2: [i32; 3],
    tr: &mut Trace,
    mut depth: u8,
) -> bool {
    loop {
        if depth > 120 {
            return true;
        }
        if num < 0 {
            if num != SOLID {
                tr.allsolid = false;
            } else {
                tr.startsolid = true;
            }
            return true; // empty subtree -> no impact
        }
        if num as usize >= map.n_clip {
            tr.allsolid = false;
            return true;
        }
        let cn = map.clipnode(num as usize);
        let t1 = plane_delta(&cn, p1);
        let t2 = plane_delta(&cn, p2);
        // Most hull nodes put the complete segment on one side. Turn those
        // tail-recursive walks into a tight loop; recurse only at a real plane
        // crossing where the traversal must return to inspect the far side.
        if t1 >= 0 && t2 >= 0 {
            num = cn.c0;
            depth += 1;
            continue;
        }
        if t1 < 0 && t2 < 0 {
            num = cn.c1;
            depth += 1;
            continue;
        }
        // Crosses the plane -- split the segment. Back off by DIST_EPSILON (Quake's
        // trick) so we stop just SHORT of the plane instead of exactly on it, which
        // would leave the player startsolid (wedged) and unable to move next frame.
        const EPS: i32 = 1;
        let denom = t1 - t2;
        let nudged = if t1 < 0 { t1 + EPS } else { t1 - EPS };
        let frac = if denom == 0 {
            0
        } else {
            ((nudged * 4096) / denom).clamp(0, 4096)
        };
        let midf = p1f + (((p2f - p1f) * frac) >> 12);
        let mid = [
            p1[0] + (((p2[0] - p1[0]) * frac) >> 12),
            p1[1] + (((p2[1] - p1[1]) * frac) >> 12),
            p1[2] + (((p2[2] - p1[2]) * frac) >> 12),
        ];
        let side = t1 < 0; // true -> back side first
        let (near, far) = if side { (cn.c1, cn.c0) } else { (cn.c0, cn.c1) };
        if !recurse(map, near, p1f, midf, p1, mid, tr, depth + 1) {
            return false;
        }
        if point_contents(map, far, mid) != SOLID {
            return recurse(map, far, midf, p2f, mid, p2, tr, depth + 1);
        }
        if tr.allsolid {
            return false;
        }
        // Impact: the far side is solid at the split point.
        let n = plane_normal(&cn);
        tr.normal = if side { [-n[0], -n[1], -n[2]] } else { n };
        tr.frac = midf;
        return false;
    }
}

#[inline(never)]
fn trace(map: &Map, head: i32, p1: [i32; 3], p2: [i32; 3]) -> Trace {
    let mut tr = Trace {
        frac: 4096,
        normal: [0, 0, 0],
        allsolid: true,
        startsolid: false,
        mover: -1,
    };
    recurse(map, head as i16, 0, 4096, p1, p2, &mut tr, 0);
    tr
}

/// GoldSrc hull 0 is the render BSP node tree, not the pre-expanded clipnode
/// tree used by hulls 1/3. Point rays therefore need the same recursive walk
/// as `SV_RecursiveHullCheck`, but with node children ending in BSP leaves
/// (leaf 0 is solid).
fn recurse_node_trace(
    map: &Map,
    mut num: i32,
    p1f: i32,
    p2f: i32,
    p1: [i32; 3],
    p2: [i32; 3],
    tr: &mut Trace,
    mut depth: u8,
) -> bool {
    loop {
        if depth > 120 {
            return true;
        }
        if num < 0 {
            if (-num - 1) != 0 {
                tr.allsolid = false;
            } else {
                tr.startsolid = true;
            }
            return true;
        }
        if num as usize >= map.n_nodes {
            tr.allsolid = false;
            return true;
        }

        let nd = map.node(num as usize);
        let t1 = dot(nd.n, p1).wrapping_sub(nd.dist);
        let t2 = dot(nd.n, p2).wrapping_sub(nd.dist);
        if t1 >= 0 && t2 >= 0 {
            num = nd.c0;
            depth += 1;
            continue;
        }
        if t1 < 0 && t2 < 0 {
            num = nd.c1;
            depth += 1;
            continue;
        }

        // Player clip-hull movement backs away from a plane by one integer
        // world unit to avoid re-entering it next frame. GoldSrc point hull 0
        // uses DIST_EPSILON=1/32; at this runtime's integer precision that is
        // zero. Keeping the exact split is what makes DROP_TO_FLOOR land at
        // -216/-80 rather than one unit above those c1a1b surfaces.
        let denom = t1.wrapping_sub(t2);
        let frac = if denom == 0 {
            0
        } else {
            ((t1 * 4096) / denom).clamp(0, 4096)
        };
        let midf = p1f + (((p2f - p1f) * frac) >> 12);
        let mid = [
            p1[0] + (((p2[0] - p1[0]) * frac) >> 12),
            p1[1] + (((p2[1] - p1[1]) * frac) >> 12),
            p1[2] + (((p2[2] - p1[2]) * frac) >> 12),
        ];
        let side = t1 < 0;
        let (near, far) = if side { (nd.c1, nd.c0) } else { (nd.c0, nd.c1) };
        if !recurse_node_trace(map, near, p1f, midf, p1, mid, tr, depth + 1) {
            return false;
        }
        if !node_point_solid_from(map, far, mid) {
            return recurse_node_trace(map, far, midf, p2f, mid, p2, tr, depth + 1);
        }
        if tr.allsolid {
            return false;
        }
        let n = [nd.n[0] as i32, nd.n[1] as i32, nd.n[2] as i32];
        tr.normal = if side { [-n[0], -n[1], -n[2]] } else { n };
        tr.frac = midf;
        return false;
    }
}

#[inline(never)]
fn trace_nodes(map: &Map, head: i32, p1: [i32; 3], p2: [i32; 3]) -> Trace {
    let mut tr = Trace {
        frac: 4096,
        normal: [0, 0, 0],
        allsolid: true,
        startsolid: false,
        mover: -1,
    };
    if map.n_nodes != 0 && head >= 0 {
        recurse_node_trace(map, head, 0, 4096, p1, p2, &mut tr, 0);
    } else {
        tr.allsolid = false;
    }
    tr
}

/// Minimal state for a segment-visibility trace. LOS callers only ask whether
/// the segment was blocked; carrying Q12 fractions, impact normals, and mover
/// ids through every recursive split made their many AI probes pay for a full
/// gameplay ray cast.
struct ClearTrace {
    startsolid: bool,
}

/// Boolean twin of `recurse`. The split side, epsilon, midpoint rounding, bad
/// data guards, and start-solid convention intentionally match it exactly.
fn recurse_clear(
    map: &Map,
    mut num: i16,
    p1: [i32; 3],
    p2: [i32; 3],
    tr: &mut ClearTrace,
    mut depth: u8,
) -> bool {
    loop {
        if depth > 120 {
            return true;
        }
        if num < 0 {
            if num == SOLID {
                tr.startsolid = true;
            }
            return true;
        }
        if num as usize >= map.n_clip {
            return true;
        }
        let cn = map.clipnode(num as usize);
        let t1 = plane_delta(&cn, p1);
        let t2 = plane_delta(&cn, p2);
        if t1 >= 0 && t2 >= 0 {
            num = cn.c0;
            depth += 1;
            continue;
        }
        if t1 < 0 && t2 < 0 {
            num = cn.c1;
            depth += 1;
            continue;
        }

        const EPS: i32 = 1;
        let denom = t1 - t2;
        let nudged = if t1 < 0 { t1 + EPS } else { t1 - EPS };
        let frac = if denom == 0 {
            0
        } else {
            ((nudged * 4096) / denom).clamp(0, 4096)
        };
        let mid = [
            p1[0] + (((p2[0] - p1[0]) * frac) >> 12),
            p1[1] + (((p2[1] - p1[1]) * frac) >> 12),
            p1[2] + (((p2[2] - p1[2]) * frac) >> 12),
        ];
        let side = t1 < 0;
        let (near, far) = if side { (cn.c1, cn.c0) } else { (cn.c0, cn.c1) };
        if !recurse_clear(map, near, p1, mid, tr, depth + 1) {
            return false;
        }
        if point_contents(map, far, mid) != SOLID {
            return recurse_clear(map, far, mid, p2, tr, depth + 1);
        }
        return false;
    }
}

#[inline(never)]
fn trace_clear(map: &Map, head: i32, p1: [i32; 3], p2: [i32; 3]) -> bool {
    let mut tr = ClearTrace { startsolid: false };
    let no_impact = recurse_clear(map, head as i16, p1, p2, &mut tr, 0);
    // Preserve line_clear_world's historical convention: a ray beginning in
    // solid is treated as clear even when traversal reports an impact.
    tr.startsolid || no_impact
}

/// A moving/brush collider: a submodel clip hull at a world offset, optionally
/// rotated about that offset (the tram: its verts/hull are entity-local, so
/// `off` is both its world position and its rotation pivot).
#[derive(Clone, Copy)]
pub struct Mover {
    pub head: i32,
    // Point-hull root (hitscans; 0 = fall back to `head`). The high bit is a
    // zero-RAM tag that excludes a rotating brush from actor visual LOS while
    // preserving its collision root for gameplay traces.
    pub head0: i32,
    pub off: [i32; 3],
    pub center: [i32; 3],
    pub radius: i32,
    pub id: i32, // owning brush-entity index (traces report it on hit)
    // World yaw about `off` as the render matrix's own q12 cos/sin (extracted
    // from Mat3I16::rotate_y so hull and visual quantize identically).
    // Identity = (4096, 0): the plain translated fast path.
    pub rc: i32,
    pub rs: i32,
}

pub const MOVER_VISUAL_DISABLED: i32 = i32::MIN;

impl Mover {
    #[inline]
    pub fn point_head(self) -> i32 {
        self.head0 & !MOVER_VISUAL_DISABLED
    }

    #[inline]
    fn visual_disabled(self) -> bool {
        self.head0 & MOVER_VISUAL_DISABLED != 0
    }
}

/// Rotate a vector by the render's rotate_y(c, s): x' = c·x + s·z, z' = −s·x + c·z.
#[inline]
fn rot_y(p: [i32; 3], c: i32, s: i32) -> [i32; 3] {
    [
        (c * p[0] + s * p[2]) >> 12,
        p[1],
        (-s * p[0] + c * p[2]) >> 12,
    ]
}

/// Inverse (transpose) of [`rot_y`]: world -> mover-local space.
#[inline]
fn rot_y_inv(p: [i32; 3], c: i32, s: i32) -> [i32; 3] {
    [
        (c * p[0] - s * p[2]) >> 12,
        p[1],
        (s * p[0] + c * p[2]) >> 12,
    ]
}

#[inline]
fn mover_local(mv: &Mover, p: [i32; 3]) -> [i32; 3] {
    let d = [p[0] - mv.off[0], p[1] - mv.off[1], p[2] - mv.off[2]];
    if mv.rs != 0 || mv.rc != 4096 {
        rot_y_inv(d, mv.rc, mv.rs)
    } else {
        d
    }
}

/// Transform both endpoints together so the yaw/identity dispatch and its
/// sizeable fixed-point matrix path exist once instead of being cloned into
/// every point/clip trace loop twice.
#[inline(never)]
fn mover_local_segment(mv: &Mover, p1: [i32; 3], p2: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    (mover_local(mv, p1), mover_local(mv, p2))
}

#[inline]
fn mover_world_center(mv: &Mover) -> [i32; 3] {
    let local = if mv.rs != 0 || mv.rc != 4096 {
        rot_y(mv.center, mv.rc, mv.rs)
    } else {
        mv.center
    };
    [
        local[0] + mv.off[0],
        local[1] + mv.off[1],
        local[2] + mv.off[2],
    ]
}

const SWIM_SPEED: i32 = 13; // water wishspeed = 0.8 * maxspeed = 256 u/s (pm_shared.c:1356)
const SWIM_SINK: i32 = 3; // idle sink -60 u/s (pm_shared.c:1341)
const SWIM_PADDLE: i32 = 5; // jump in water swims up 100 u/s (pm_shared.c:2513)
const WATERJUMP_UP: i32 = 11; // hop-out boost 225 u/s (pm_shared.c:2617-79)

pub const NO_MOVER: Mover = Mover {
    head: 0,
    head0: 0,
    off: [0, 0, 0],
    center: [0, 0, 0],
    radius: 0,
    id: -1,
    rc: 4096,
    rs: 0,
};

#[inline(never)]
fn mover_may_touch_segment(mv: &Mover, p1: [i32; 3], p2: [i32; 3]) -> bool {
    if mv.radius <= 0 {
        return true;
    }
    let c = mover_world_center(mv);
    let r = mv.radius;
    let mut axis = 0;
    while axis < 3 {
        let lo = p1[axis].min(p2[axis]) - r;
        let hi = p1[axis].max(p2[axis]) + r;
        if c[axis] < lo || c[axis] > hi {
            return false;
        }
        axis += 1;
    }
    true
}

/// True when the segment does not hit any shifted mover hull.
pub fn line_clear_movers(map: &Map, movers: &[Mover], p1: [i32; 3], p2: [i32; 3]) -> bool {
    line_clear_movers_except(map, movers, p1, p2, i32::MIN)
}

/// Like [`line_clear_movers`] but ignores the mover whose id is `exclude_id`.
/// Aim-use traces end INSIDE the target button/charger/door brush, so that
/// brush's own hull would always report "blocked" -- exclude it so line of
/// sight to the thing you're pressing isn't blocked by the thing itself.
pub fn line_clear_movers_except(
    map: &Map,
    movers: &[Mover],
    p1: [i32; 3],
    p2: [i32; 3],
    exclude_id: i32,
) -> bool {
    for mv in movers {
        if mv.id == exclude_id {
            continue;
        }
        let point_head = mv.point_head();
        if point_head <= 0 && mv.head <= 0 {
            continue;
        }
        if !mover_may_touch_segment(mv, p1, p2) {
            continue;
        }
        let (q1, q2) = mover_local_segment(mv, p1, p2);
        let clear = if point_head > 0 {
            line_clear_visual_from(map, point_head, q1, q2)
        } else {
            trace_clear(map, mv.head, q1, q2)
        };
        if !clear {
            return false;
        }
    }
    true
}

/// True when the segment does not hit the static world point hull.
pub fn line_clear_world(map: &Map, p1: [i32; 3], p2: [i32; 3]) -> bool {
    line_clear_visual_from(map, 0, p1, p2)
}

struct VisualClearTrace {
    first_leaf: bool,
    startsolid: bool,
}

fn recurse_visual_clear(
    map: &Map,
    mut num: i32,
    p1: [i32; 3],
    p2: [i32; 3],
    tr: &mut VisualClearTrace,
    mut depth: u8,
) -> bool {
    loop {
        // Malformed/cyclic BSP data must fail open: this function is only a
        // render optimization, so bad data may cost draw time but never hide a
        // model. Valid GoldSrc trees are substantially shallower than this.
        if depth > 120 {
            return true;
        }
        if num < 0 {
            let solid = (-num - 1) == 0; // leaf 0 is the shared solid leaf
            if tr.first_leaf {
                tr.first_leaf = false;
                tr.startsolid = solid;
            }
            return !solid;
        }
        if num as usize >= map.n_nodes {
            return true;
        }

        let nd = map.node(num as usize);
        let t1 = dot(nd.n, p1).wrapping_sub(nd.dist);
        let t2 = dot(nd.n, p2).wrapping_sub(nd.dist);
        if t1 >= 0 && t2 >= 0 {
            num = nd.c0;
            depth += 1;
            continue;
        }
        if t1 < 0 && t2 < 0 {
            num = nd.c1;
            depth += 1;
            continue;
        }

        let denom = t1 - t2;
        let frac = if denom == 0 {
            0
        } else {
            ((t1 * 4096) / denom).clamp(0, 4096)
        };
        let mid = [
            p1[0] + (((p2[0] - p1[0]) * frac) >> 12),
            p1[1] + (((p2[1] - p1[1]) * frac) >> 12),
            p1[2] + (((p2[2] - p1[2]) * frac) >> 12),
        ];
        let (near, far) = if t1 < 0 {
            (nd.c1, nd.c0)
        } else {
            (nd.c0, nd.c1)
        };
        if !recurse_visual_clear(map, near, p1, mid, tr, depth + 1) {
            return false;
        }
        return recurse_visual_clear(map, far, mid, p2, tr, depth + 1);
    }
}

#[inline]
fn line_clear_visual_from(map: &Map, head: i32, p1: [i32; 3], p2: [i32; 3]) -> bool {
    if map.n_nodes == 0 || head < 0 {
        return true;
    }
    let mut tr = VisualClearTrace {
        first_leaf: true,
        startsolid: false,
    };
    let clear = recurse_visual_clear(map, head, p1, p2, &mut tr, 0);
    // Rendering must fail open if camera quantization puts the eye on the
    // solid side of a boundary; otherwise one transient classification can
    // hide every actor in view.
    tr.startsolid || clear
}

#[inline]
fn node_point_solid_from(map: &Map, mut num: i32, p: [i32; 3]) -> bool {
    let mut depth = 0u8;
    loop {
        if num < 0 {
            return (-num - 1) == 0;
        }
        if num as usize >= map.n_nodes || depth > 120 {
            return false;
        }
        let nd = map.node(num as usize);
        num = if dot(nd.n, p).wrapping_sub(nd.dist) >= 0 {
            nd.c0
        } else {
            nd.c1
        };
        depth += 1;
    }
}

fn visual_sphere_solid_from(map: &Map, num: i32, p: [i32; 3], radius: i32, depth: u8) -> bool {
    // This is a render-only exception test. Malformed/cyclic data must fail
    // open (report no overlap) so it can never make an unrelated occluder
    // disappear.
    if depth > 120 {
        return false;
    }
    if num < 0 {
        return (-num - 1) == 0;
    }
    if num as usize >= map.n_nodes {
        return false;
    }
    let nd = map.node(num as usize);
    let d = dot(nd.n, p).wrapping_sub(nd.dist);
    if d > radius {
        return visual_sphere_solid_from(map, nd.c0, p, radius, depth + 1);
    }
    if d < -radius {
        return visual_sphere_solid_from(map, nd.c1, p, radius, depth + 1);
    }
    visual_sphere_solid_from(map, nd.c0, p, radius, depth + 1)
        || visual_sphere_solid_from(map, nd.c1, p, radius, depth + 1)
}

/// True when a visual point segment stays outside the static world's solid
/// leaves. Unlike `line_clear_world`, this walks the render BSP node tree rooted
/// at node 0. GoldSrc's `headnode[0]` indexes that tree, but the legacy cooker
/// fed it to the clipnode remapper, where it can alias the expanded player hull
/// and falsely hide an actor that is plainly visible around a door frame.
#[inline]
pub fn line_clear_world_visual(map: &Map, p1: [i32; 3], p2: [i32; 3]) -> bool {
    line_clear_visual_from(map, 0, p1, p2)
}

/// Visual point trace through active brush entities (doors, plats, grates and
/// the tram). Submodel `head0` values index the same render-node array as the
/// world tree, so they must not be interpreted as clipnode roots either.
pub fn line_clear_movers_visual(
    map: &Map,
    movers: &[Mover],
    p1: [i32; 3],
    p2: [i32; 3],
    ignore_center: [i32; 3],
    ignore_radius: i32,
) -> bool {
    for mv in movers {
        // The synthetic tram has no cooked render-BSP root. Its inflated
        // collision hull remains valid for gameplay, but it has no accurate
        // windows and must not make actors (c0a0's torch Barney) flicker.
        if mv.id == -2 || mv.visual_disabled() {
            continue;
        }
        let point_head = mv.point_head();
        if ignore_radius > 0 && mv.radius > 0 && point_head > 0 {
            let center = mover_world_center(mv);
            let reach = mv.radius.saturating_add(ignore_radius);
            // Cheap broad phase, followed by an exact sphere-vs-submodel BSP
            // overlap. The old cube-only test ignored nearby walls and
            // breakables that did not actually touch Barney.
            if (center[0] - ignore_center[0]).abs() <= reach
                && (center[1] - ignore_center[1]).abs() <= reach
                && (center[2] - ignore_center[2]).abs() <= reach
                && visual_sphere_solid_from(
                    map,
                    point_head,
                    mover_local(mv, ignore_center),
                    ignore_radius,
                    0,
                )
            {
                continue;
            }
        }
        if !mover_may_touch_segment(mv, p1, p2) {
            continue;
        }
        let (q1, q2) = mover_local_segment(mv, p1, p2);
        let clear = if point_head > 0 {
            // An actor can be authored partly inside a door/frame brush. The
            // visible portion must not be rejected merely because this probe
            // ends inside that same mover (c0a0's torch Barney is canonical).
            if node_point_solid_from(map, point_head, q2) {
                continue;
            }
            line_clear_visual_from(map, point_head, q1, q2)
        } else if mv.head > 0 {
            // A non-tram mover without a cooked render-node root falls back to
            // its collision hull; the synthetic tram was skipped above.
            trace_clear(map, mv.head, q1, q2)
        } else {
            true
        };
        if !clear {
            return false;
        }
    }
    true
}

/// Trace the static world render-node tree plus every brush entity's render
/// hull-0 subtree. Brush records without a point subtree fall back to their
/// clip hull, but render-node indices are never interpreted as clipnode ids.
fn trace_point_all(map: &Map, movers: &[Mover], p1: [i32; 3], p2: [i32; 3]) -> Trace {
    let mut best = trace_nodes(map, 0, p1, p2);
    for mv in movers {
        if !mover_may_touch_segment(mv, p1, p2) {
            continue;
        }
        let point_head = mv.point_head();
        if point_head <= 0 && mv.head <= 0 {
            continue;
        }
        let (q1, q2) = mover_local_segment(mv, p1, p2);
        let t = if point_head > 0 {
            trace_nodes(map, point_head, q1, q2)
        } else {
            trace(map, mv.head, q1, q2)
        };
        // A non-solid/translated brush can contain the ray start. Match the
        // player trace convention: only the static world's startsolid state is
        // authoritative; movers still block entry through their hit fraction.
        if t.frac < best.frac {
            best.frac = t.frac;
            best.normal = if mv.rs != 0 || mv.rc != 4096 {
                rot_y(t.normal, mv.rc, mv.rs)
            } else {
                t.normal
            };
            best.mover = mv.id;
        }
    }
    best
}

/// Trace a point ray through static world and active mover hulls.
pub fn trace_line(map: &Map, movers: &[Mover], p1: [i32; 3], p2: [i32; 3]) -> Option<RayHit> {
    let t = trace_point_all(map, movers, p1, p2);
    if t.startsolid || t.frac >= 4096 {
        return None;
    }
    Some(RayHit {
        frac: t.frac,
        pos: [
            p1[0] + (((p2[0] - p1[0]) * t.frac) >> 12),
            p1[1] + (((p2[1] - p1[1]) * t.frac) >> 12),
            p1[2] + (((p2[2] - p1[2]) * t.frac) >> 12),
        ],
        normal: t.normal,
        mover: t.mover,
    })
}

/// Downward point probe used by a translated bbox to discover its floor.
/// Unlike ordinary rays, a start point already touching/quantized into the
/// supporting solid is a valid hit. Generic gameplay traces intentionally
/// treat startsolid as clear, so pushables need this narrow support query.
pub fn trace_down_support(
    map: &Map,
    movers: &[Mover],
    p1: [i32; 3],
    p2: [i32; 3],
    ignore_mover: i32,
) -> Option<RayHit> {
    if p2[1] >= p1[1] {
        return None;
    }
    let world = trace_nodes(map, 0, p1, p2);
    if world.startsolid {
        return Some(RayHit {
            frac: 0,
            pos: p1,
            normal: [0, 4096, 0],
            mover: -1,
        });
    }
    let mut best_frac = world.frac;
    let mut best_normal = world.normal;
    let mut best_mover = -1;
    for mv in movers {
        if mv.id == ignore_mover || !mover_may_touch_segment(mv, p1, p2) {
            continue;
        }
        let point_head = mv.point_head();
        if point_head <= 0 && mv.head <= 0 {
            continue;
        }
        let (q1, q2) = mover_local_segment(mv, p1, p2);
        let hit = if point_head > 0 {
            trace_nodes(map, point_head, q1, q2)
        } else {
            trace(map, mv.head, q1, q2)
        };
        if hit.startsolid {
            return Some(RayHit {
                frac: 0,
                pos: p1,
                normal: [0, 4096, 0],
                mover: mv.id,
            });
        }
        if hit.frac < best_frac {
            best_frac = hit.frac;
            best_normal = if mv.rs != 0 || mv.rc != 4096 {
                rot_y(hit.normal, mv.rc, mv.rs)
            } else {
                hit.normal
            };
            best_mover = mv.id;
        }
    }
    if best_frac >= 4096 {
        return None;
    }
    Some(RayHit {
        frac: best_frac,
        pos: [
            p1[0] + (((p2[0] - p1[0]) * best_frac) >> 12),
            p1[1] + (((p2[1] - p1[1]) * best_frac) >> 12),
            p1[2] + (((p2[2] - p1[2]) * best_frac) >> 12),
        ],
        normal: best_normal,
        mover: best_mover,
    })
}

/// Snap a point origin onto floor geometry near it.
pub fn snap_to_ground(
    map: &Map,
    movers: &[Mover],
    pos: [i32; 3],
    probe_up: i32,
    probe_down: i32,
) -> Option<[i32; 3]> {
    let p1 = [pos[0], pos[1] + probe_up.max(0), pos[2]];
    let p2 = [pos[0], pos[1] - probe_down.max(0), pos[2]];
    let t = trace_point_all(map, movers, p1, p2);
    if t.startsolid || t.frac >= 4096 || t.normal[1] <= GROUND_NY {
        return None;
    }
    Some([
        p1[0] + (((p2[0] - p1[0]) * t.frac) >> 12),
        p1[1] + (((p2[1] - p1[1]) * t.frac) >> 12),
        p1[2] + (((p2[2] - p1[2]) * t.frac) >> 12),
    ])
}

/// Trace the world hull plus every mover hull (each shifted by its offset);
/// return the nearest impact.
fn trace_all(map: &Map, world_head: i32, movers: &[Mover], p1: [i32; 3], p2: [i32; 3]) -> Trace {
    let mut best = trace(map, world_head, p1, p2);
    for mv in movers {
        let head = mv.head;
        if head <= 0 {
            continue; // no clip hull for this submodel
        }
        if !mover_may_touch_segment(mv, p1, p2) {
            continue;
        }
        let (q1, q2) = mover_local_segment(mv, p1, p2);
        let t = trace(map, head, q1, q2);
        // NB: do NOT propagate a mover's startsolid. If the player ends up inside
        // a brush-entity hull (a non-solid func_illusionary, or slight
        // penetration), startsolid would make slide_move break and freeze them
        // forever. Movers still block ENTRY via frac; only the world hull's
        // startsolid counts as truly stuck.
        if t.frac < best.frac {
            best.frac = t.frac;
            best.normal = if mv.rs != 0 || mv.rc != 4096 {
                rot_y(t.normal, mv.rc, mv.rs) // impact normal back to world space
            } else {
                t.normal
            };
            best.mover = mv.id;
        }
    }
    best
}

/// True if the STANDING hull (hull-1) has room at `pos` (a point trace that is
/// not startsolid). Lets the caller keep the player crouched when they can't
/// stand under a low ceiling (HL behaviour) -- else un-ducking inside a vent
/// switches to the startsolid hull-1 and wedges the player.
pub fn standing_fits(map: &Map, pos: [i32; 3]) -> bool {
    !trace(map, map.hull1_head, pos, pos).startsolid
}

/// Whole-path human-hull probe used when a scripted monster chooses its route.
/// GoldSrc tests the complete local move before falling back to its node graph;
/// a short point step cannot make that decision because it walks straight up
/// to a distant obstruction first. Callers pass hull-centre coordinates.
#[inline]
pub fn human_hull_line_clear(map: &Map, from: [i32; 3], to: [i32; 3]) -> bool {
    trace_clear(map, map.hull1_head, from, to)
}

#[inline]
fn dot12_i32(a: [i32; 3], b: [i32; 3]) -> i32 {
    ((a[0] * b[0]) + (a[1] * b[1]) + (a[2] * b[2])) >> 12
}

#[inline]
fn dot_raw(a: [i32; 3], b: [i32; 3]) -> i32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn scale12(v: [i32; 3], s: i32) -> [i32; 3] {
    [(v[0] * s) >> 12, (v[1] * s) >> 12, (v[2] * s) >> 12]
}

/// Sign-symmetric nearest-integer Q12 product. PM_Accelerate operates in
/// floats; always flooring a small positive tangent to zero while retaining
/// the corresponding negative value biases cardinal turns in integer space.
#[inline(always)]
fn mul_q12_round(a: i32, b: i32) -> i32 {
    let product = a * b;
    if product >= 0 {
        (product + 2048) >> 12
    } else {
        -((-product + 2048) >> 12)
    }
}

const PLANAR_FRAC_BITS: i32 = 6;
const PLANAR_FRAC_ONE: i32 = 1 << PLANAR_FRAC_BITS;
const PLANAR_FRAC_HALF: i32 = PLANAR_FRAC_ONE >> 1;

#[inline(always)]
fn split_planar_round(value: i32) -> (i32, i8) {
    let whole = if value >= 0 {
        (value + PLANAR_FRAC_HALF) >> PLANAR_FRAC_BITS
    } else {
        -((-value + PLANAR_FRAC_HALF) >> PLANAR_FRAC_BITS)
    };
    (whole, (value - whole * PLANAR_FRAC_ONE) as i8)
}

#[inline(always)]
fn q12_to_planar_round(value: i32) -> i32 {
    const SHIFT: i32 = 12 - PLANAR_FRAC_BITS;
    const HALF: i32 = 1 << (SHIFT - 1);
    if value >= 0 {
        (value + HALF) >> SHIFT
    } else {
        -((-value + HALF) >> SHIFT)
    }
}

#[inline]
fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn cross12(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [
        ((a[1] * b[2] - a[2] * b[1]) >> 12),
        ((a[2] * b[0] - a[0] * b[2]) >> 12),
        ((a[0] * b[1] - a[1] * b[0]) >> 12),
    ]
}

/// Remove the component of `v` along `n` (×4096) -- slide along a plane.
fn clip_velocity(v: [i32; 3], n: [i32; 3]) -> [i32; 3] {
    let proj = dot12_i32(v, n);
    // GoldSrc zeros components below 0.1 u/s. Our smallest non-zero integer
    // component is 1 u/tick = 20 u/s, so every representable component must
    // survive; an integer +/-1 is the tangent needed by c0a0e's exit turn.
    [
        v[0] - ((n[0] * proj) >> 12),
        v[1] - ((n[1] * proj) >> 12),
        v[2] - ((n[2] * proj) >> 12),
    ]
}

fn clear_at(map: &Map, head: i32, movers: &[Mover], pos: [i32; 3]) -> bool {
    !trace_all(map, head, movers, pos, pos).startsolid
}

fn try_unstick(map: &Map, head: i32, movers: &[Mover], pos: [i32; 3]) -> Option<[i32; 3]> {
    if clear_at(map, head, movers, pos) {
        return Some(pos);
    }
    const OFFSETS: [[i32; 3]; 14] = [
        [0, 1, 0],
        [0, 2, 0],
        [1, 0, 0],
        [-1, 0, 0],
        [0, 0, 1],
        [0, 0, -1],
        [2, 0, 0],
        [-2, 0, 0],
        [0, 0, 2],
        [0, 0, -2],
        [1, 1, 0],
        [-1, 1, 0],
        [0, 1, 1],
        [0, 1, -1],
    ];
    let mut i = 0;
    while i < OFFSETS.len() {
        let p = add(pos, OFFSETS[i]);
        if clear_at(map, head, movers, p) {
            return Some(p);
        }
        i += 1;
    }
    None
}

/// Slide `vel` from `pos` for one frame, sliding along walls (4 iterations).
/// Returns the new position and the wall-clipped velocity.
fn slide_move(
    map: &Map,
    head: i32,
    movers: &[Mover],
    mut pos: [i32; 3],
    mut vel: [i32; 3],
) -> ([i32; 3], [i32; 3]) {
    let mut planes = [[0i32; 3]; MAX_CLIP_PLANES];
    let mut nplanes = 0usize;
    let mut original_vel = vel;
    let primal_vel = vel;
    let mut time_left = 4096;
    for _ in 0..4 {
        if vel == [0, 0, 0] || time_left <= 0 {
            break;
        }
        let d = scale12(vel, time_left);
        let end = add(pos, d);
        let tr = trace_all(map, head, movers, pos, end);
        if tr.startsolid {
            match try_unstick(map, head, movers, pos) {
                Some(p) => {
                    pos = p;
                    continue;
                }
                None => {
                    vel = [0, 0, 0];
                    break;
                }
            }
        }
        if tr.frac > 0 {
            pos = add(pos, scale12(d, tr.frac));
            original_vel = vel;
            nplanes = 0;
        }
        if tr.frac >= 4096 {
            break;
        }
        // recurse() already backs the impact fraction off toward the clear
        // side by its integer DIST_EPSILON. A second two-unit nudge made every
        // wall/step stop three units early and could leave a turned player on
        // the wrong side of an adjacent hull plane.
        time_left = (time_left * (4096 - tr.frac)) >> 12;

        if nplanes >= MAX_CLIP_PLANES {
            vel = [0, 0, 0];
            break;
        }
        planes[nplanes] = tr.normal;
        nplanes += 1;

        let mut new_vel = [0, 0, 0];
        let mut found = false;
        let mut i = 0;
        while i < nplanes {
            new_vel = clip_velocity(original_vel, planes[i]);
            let mut ok = true;
            let mut j = 0;
            while j < nplanes {
                if i != j && dot12_i32(new_vel, planes[j]) < 0 {
                    ok = false;
                    break;
                }
                j += 1;
            }
            if ok {
                found = true;
                break;
            }
            i += 1;
        }

        if found {
            vel = new_vel;
        } else if nplanes == 2 {
            let dir = cross12(planes[0], planes[1]);
            vel = scale12(dir, dot12_i32(vel, dir));
        } else {
            vel = [0, 0, 0];
            break;
        }

        if dot_raw(vel, primal_vel) <= 0 {
            vel = [0, 0, 0];
            break;
        }
    }
    (pos, vel)
}

fn dist_xz(a: [i32; 3], b: [i32; 3]) -> i32 {
    let dx = b[0] - a[0];
    let dz = b[2] - a[2];
    dx * dx + dz * dz
}

const STEP_UP: i32 = 18; // max stair/ledge height the player climbs
const CLIMB_SPEED: i32 = 10; // ladder vertical units/tick at full stick
const LATERAL_CLIMB: i32 = 6; // slow xz drift while on a ladder
const CLIMB_PITCH_DOWN: i16 = 300; // pitch beyond this = looking down -> descend

pub struct Player {
    pub pos: [i32; 3],
    pub vel: [i32; 3],
    // Signed Q6 sub-unit horizontal velocity retained across 20 Hz ticks. The
    // two bytes occupy Player's former alignment padding, so this prevents
    // PM_Accelerate/friction from erasing small wall tangents at zero RAM cost.
    vel_frac_xz: [i8; 2],
    pub on_ground: bool,
    pub ground_mover: i32, // ent id of the mover under our feet (-1 = world/none)
    pub land_impact: i32,  // downward speed absorbed the tick we touched down (0 = none)
    pub crouch: bool,      // hold-duck: trace the world against the shorter hull-3
}
const _: [(); 36] = [(); core::mem::size_of::<Player>()];

impl Player {
    pub fn new(pos: [i32; 3]) -> Player {
        Player {
            pos,
            vel: [0, 0, 0],
            vel_frac_xz: [0, 0],
            on_ground: false,
            ground_mover: -1,
            land_impact: 0,
            crouch: false,
        }
    }

    /// External gameplay velocity replacement (pushables) must discard the
    /// prior PM sub-unit residue just like GoldSrc overwriting `velocity`.
    pub fn set_planar_velocity(&mut self, x: i32, z: i32) {
        self.vel[0] = x;
        self.vel[2] = z;
        self.vel_frac_xz = [0, 0];
    }

    pub fn clear_velocity(&mut self) {
        self.vel = [0, 0, 0];
        self.vel_frac_xz = [0, 0];
    }

    /// The world clip-hull headnode to trace against: the shorter crouch hull
    /// (hull-3, fits low vents) while ducking, else the standing hull-1. Falls
    /// back to hull-1 if the map has no crouch hull (headnode[3] empty -> < 0).
    #[inline]
    fn head(&self, map: &Map) -> i32 {
        if self.crouch && map.hull3_head >= 0 {
            map.hull3_head
        } else {
            map.hull1_head
        }
    }

    /// Duck/un-duck with HL's airborne origin shift (pm_shared.c:1920-2057):
    /// ducking mid-air pulls the feet UP 18 units (half the hull-height
    /// difference), which is what makes crouch-jumping clear taller ledges;
    /// un-ducking reverses it. On the ground the origin stays put, and
    /// standing up is refused while a ceiling would wedge the standing hull.
    pub fn set_crouch(&mut self, map: &Map, movers: &[Mover], want: bool) {
        if want == self.crouch {
            return;
        }
        const DUCK_SHIFT: i32 = 18; // (72 - 36) / 2: hull-1 vs hull-3 height
        if want {
            self.crouch = true;
            if !self.on_ground {
                let up = [self.pos[0], self.pos[1] + DUCK_SHIFT, self.pos[2]];
                let head = self.head(map);
                let t = trace_all(map, head, movers, self.pos, up);
                self.pos[1] += (DUCK_SHIFT * t.frac) >> 12;
            }
        } else {
            // Try to stand: feet drop back down in the air; blocked heads
            // stay crouched (the un-duck startsolid freeze from M48).
            let down_pos = if self.on_ground {
                self.pos
            } else {
                [self.pos[0], self.pos[1] - DUCK_SHIFT, self.pos[2]]
            };
            if !trace(map, map.hull1_head, down_pos, down_pos).startsolid {
                self.crouch = false;
                self.pos = down_pos;
            } else if !trace(map, map.hull1_head, self.pos, self.pos).startsolid {
                self.crouch = false;
            }
        }
    }

    /// Ladder-climb frame: gravity off, forward input runs up or down the
    /// ladder by view pitch (HL feel: look up + forward climbs up), strafe
    /// slides along it, jump lets go with a push away from the view.
    pub fn update_climb(
        &mut self,
        map: &Map,
        movers: &[Mover],
        fwd: i32,
        strafe: i32,
        jump: bool,
        yaw: u16,
        pitch: i16,
    ) {
        self.vel_frac_xz = [0, 0];
        let s = sincos::sin_q12(yaw);
        let c = sincos::sin_q12((yaw + 1024) & 0xFFF);
        if jump {
            // Let go: push back off the ladder (270 u/s, pm_shared.c:2131-35).
            const DISMOUNT: i32 = 14;
            self.vel = [(-s * DISMOUNT) >> 12, 0, (-c * DISMOUNT) >> 12];
            self.on_ground = false;
            let head = self.head(map);
            let (p, v) = slide_move(map, head, movers, self.pos, self.vel);
            self.pos = p;
            self.vel = v;
            return;
        }
        // Positive pitch = looking up (stick up). Forward climbs up unless the
        // player is looking clearly downward, then it descends (HL ladder feel).
        let up = if pitch >= -CLIMB_PITCH_DOWN { 1 } else { -1 };
        self.vel = [
            (s * fwd / 128 * LATERAL_CLIMB) >> 12,
            fwd * up * CLIMB_SPEED / 128,
            (c * fwd / 128 * LATERAL_CLIMB) >> 12,
        ];
        // Strafe slides sideways along the wall.
        self.vel[0] += (c * strafe / 128 * LATERAL_CLIMB) >> 12;
        self.vel[2] += (-s * strafe / 128 * LATERAL_CLIMB) >> 12;
        let head = self.head(map);
        let (p, v) = slide_move(map, head, movers, self.pos, self.vel);
        self.pos = p;
        self.vel = v;
        self.on_ground = false;
        self.ground_mover = -1;
    }

    /// Swim physics inside a func_water volume: move along the LOOK direction
    /// (pitch included), jump paddles straight up, and idle sinks slowly.
    pub fn update_swim(
        &mut self,
        map: &Map,
        movers: &[Mover],
        fwd: i32,
        strafe: i32,
        jump: bool,
        yaw: u16,
        pitch: i16,
    ) {
        self.vel_frac_xz = [0, 0];
        let s = sincos::sin_q12(yaw);
        let c = sincos::sin_q12((yaw + 1024) & 0xFFF);
        // Look-direction swim: split fwd into a horizontal part and a vertical
        // part by pitch (positive pitch = looking up).
        let vy_look = (fwd * pitch as i32) / 200; // gentle pitch-follow
        self.vel = [
            (s * fwd / 128 * SWIM_SPEED) >> 12,
            (vy_look * SWIM_SPEED / 128).clamp(-SWIM_SPEED, SWIM_SPEED),
            (c * fwd / 128 * SWIM_SPEED) >> 12,
        ];
        self.vel[0] += (c * strafe / 128 * SWIM_SPEED) >> 12;
        self.vel[2] += (-s * strafe / 128 * SWIM_SPEED) >> 12;
        if jump {
            // Paddle up (100 u/s); pressing into a low ledge fires the
            // waterjump hop-out (225 u/s) so pools are exitable without
            // step-up luck (pm_shared.c:2617-79, simplified probe).
            let ahead = [
                self.pos[0] + ((s * 24) >> 12),
                self.pos[1],
                self.pos[2] + ((c * 24) >> 12),
            ];
            let head = self.head(map);
            let blocked = trace_all(map, head, movers, self.pos, ahead).frac < 4096;
            self.vel[1] = if blocked && fwd > 0 {
                WATERJUMP_UP
            } else {
                SWIM_PADDLE
            };
        } else if fwd == 0 && strafe == 0 {
            self.vel[1] -= SWIM_SINK; // idle: sink gently
        }
        let head = self.head(map);
        let (p, v) = slide_move(map, head, movers, self.pos, self.vel);
        self.pos = p;
        self.vel = v;
        self.on_ground = false;
        self.ground_mover = -1;
    }

    /// Advance the player one frame. `fwd`/`strafe` are analog deltas in
    /// `-128..=127` (D-pad sends ±127) relative to `yaw` (Q0.12); `jump`
    /// triggers when grounded.
    ///
    /// The horizontal model is the real PM_Friction + PM_Accelerate /
    /// PM_AirAccelerate shape (pm_shared.c): friction drops speed toward zero
    /// on the ground, acceleration adds along the wish DIRECTION capped by the
    /// speed deficit, and air control can only add up to AIR_WISH_CAP along
    /// the stick -- it never brakes, so knockback/longjump momentum carries.
    pub fn update(
        &mut self,
        map: &Map,
        movers: &[Mover],
        fwd: i32,
        strafe: i32,
        jump: bool,
        yaw: u16,
    ) {
        // Forward = (sin yaw, 0, cos yaw); right = (cos yaw, 0, -sin yaw). sin/cos
        // are ×4096; dividing the ±127 input by 128 keeps a unit wish dir ≈ ×4096.
        let s = sincos::sin_q12(yaw);
        let c = sincos::sin_q12((yaw + 1024) & 0xFFF);
        let wx = (s * fwd + c * strafe) / 128;
        let wz = (c * fwd - s * strafe) / 128;
        // Keep direction in Q12 until acceleration. Quantizing wish_x/z to
        // whole units first erased the 1.1 u/tick tangent that Gold builds up
        // while turning along the c0a0e station wall.
        // PM_WalkMove normalizes the wish vector with VectorNormalize. The old
        // octagonal approximation overestimated this shallow angle by ~3.5%,
        // reducing c0a0e's wall-tangent acceleration below its one-unit
        // friction loss and making an otherwise valid deterministic route
        // arrive fourteen ticks late.
        let wishmag_q12 = isqrt_i32(wx * wx + wz * wz);
        let wishspeed = if wishmag_q12 > 0 {
            ((wishmag_q12 * MOVE_SPEED + 2048) >> 12).clamp(1, MOVE_SPEED)
        } else {
            0
        };
        let mut fine_x = self.vel[0] * PLANAR_FRAC_ONE + self.vel_frac_xz[0] as i32;
        let mut fine_z = self.vel[2] * PLANAR_FRAC_ONE + self.vel_frac_xz[1] as i32;

        if self.on_ground {
            // PM_Friction (pm_shared.c:1202): drop = max(speed, stopspeed)
            // * friction(4) * dt(0.05) = max(speed, 5) / 5 per tick. Scale
            // retained Q6 velocity so a small orthogonal component does not
            // truncate from 2 to 1 to 0 while the dominant axis is 16.
            let speed_fine = isqrt_i32(fine_x * fine_x + fine_z * fine_z);
            if speed_fine > 0 {
                let drop_fine = speed_fine.max(STOP_SPEED * PLANAR_FRAC_ONE) / 5;
                let new_speed_fine = (speed_fine - drop_fine).max(0);
                let scale_q12 = (new_speed_fine * 4096) / speed_fine;
                fine_x = mul_q12_round(fine_x, scale_q12);
                fine_z = mul_q12_round(fine_z, scale_q12);
            }
        }
        if wishspeed > 0 {
            // PM_Accelerate (pm_shared.c:990) / PM_AirAccelerate (:1279):
            // current speed ALONG the wish direction; add at most
            // accel(10) * wishspeed * dt = wishspeed/2 per tick, never past
            // the target. In the air the target caps at 30 u/s but the accel
            // step still scales off the full wishspeed.
            let dirx = wx * 4096 / wishmag_q12;
            let dirz = wz * 4096 / wishmag_q12;
            let current_dot = fine_x * dirx + fine_z * dirz;
            const CURRENT_SHIFT: i32 = PLANAR_FRAC_BITS + 12;
            const CURRENT_HALF: i32 = 1 << (CURRENT_SHIFT - 1);
            let current = if current_dot >= 0 {
                (current_dot + CURRENT_HALF) >> CURRENT_SHIFT
            } else {
                -((-current_dot + CURRENT_HALF) >> CURRENT_SHIFT)
            };
            let target = if self.on_ground {
                wishspeed
            } else {
                wishspeed.min(AIR_WISH_CAP)
            };
            let addspeed = target - current;
            if addspeed > 0 {
                let take = (wishspeed / 2).max(1).min(addspeed);
                fine_x += q12_to_planar_round(dirx * take);
                fine_z += q12_to_planar_round(dirz * take);
            }
        }
        (self.vel[0], self.vel_frac_xz[0]) = split_planar_round(fine_x);
        (self.vel[2], self.vel_frac_xz[1]) = split_planar_round(fine_z);

        self.land_impact = 0;
        let was_air = !self.on_ground;

        if self.on_ground {
            if self.vel[1] < 0 {
                self.vel[1] = 0;
            }
            if jump {
                let moving = self.vel[0].abs() + self.vel[2].abs() > 2;
                let longjumping = unsafe { LONGJUMP } && self.crouch && moving;
                if longjumping {
                    // Ducked jump with the module: launch along the move
                    // direction at 560 u/s, 299 u/s up (pm_shared.c:2580-93).
                    let (vx, vz) = (self.vel[0], self.vel[2]);
                    let speed = vx.abs().max(vz.abs()) + vx.abs().min(vz.abs()) * 3 / 8;
                    self.vel[0] = vx * LONGJUMP_FWD / speed.max(1);
                    self.vel[2] = vz * LONGJUMP_FWD / speed.max(1);
                    self.vel_frac_xz = [0, 0];
                    self.vel[1] = LONGJUMP_UP;
                } else {
                    self.vel[1] = JUMP;
                }
                self.on_ground = false;
            }
        } else {
            self.vel[1] -= gravity_step();
        }

        // Move with stair-stepping: a plain slide, then (when grounded and
        // moving) an up/forward/down "step" -- keep whichever advanced further
        // along the ground, so the player climbs stairs/thresholds <= STEP_UP.
        let head = self.head(map);
        let start = self.pos;
        // PM_WalkMove saves the velocity from before PM_FlyMove, restores it for
        // the raised alternative, and only then chooses the farther result.  In
        // particular, the flat pass is allowed to clip both horizontal axes to
        // zero at the face of a stair; using that clipped velocity for the step
        // pass makes the raised route motionless and wedges the player against
        // even an 8-unit threshold (the c0a0e tram-platform exit).
        let move_vel = self.vel;
        let (flat_pos, flat_vel) = slide_move(map, head, movers, start, move_vel);

        if self.on_ground && (move_vel[0] != 0 || move_vel[2] != 0) {
            let up_end = [start[0], start[1] + STEP_UP, start[2]];
            let tup = trace_all(map, head, movers, start, up_end);
            let up_pos = [start[0], start[1] + ((STEP_UP * tup.frac) >> 12), start[2]];
            let (sp, step_vel) =
                slide_move(map, head, movers, up_pos, [move_vel[0], 0, move_vel[2]]);
            // PM_WalkMove traces down exactly one sv_stepsize from the raised
            // slide endpoint. Two heights could select a floor below the
            // original one and turn a step-up candidate into a 36-unit drop.
            let dn_end = [sp[0], sp[1] - STEP_UP, sp[2]];
            let tdn = trace_all(map, head, movers, sp, dn_end);
            let step_pos = [sp[0], sp[1] - ((STEP_UP * tdn.frac) >> 12), sp[2]];
            let landed = tdn.frac < 4096 && tdn.normal[1] > GROUND_NY;
            let chose_step = landed && dist_xz(start, step_pos) > dist_xz(start, flat_pos);
            if chose_step {
                self.pos = step_pos;
                // GoldSrc keeps the raised pass's horizontal clipping but the
                // flat pass's vertical component (pm_shared.c:1279-80).
                self.vel = [step_vel[0], flat_vel[1], step_vel[2]];
            } else {
                self.pos = flat_pos;
                self.vel = flat_vel;
            }
        } else {
            self.pos = flat_pos;
            self.vel = flat_vel;
        }
        // A collision clips GoldSrc's complete floating velocity. Our public
        // integer clip result has no matching fractional component, so discard
        // residue only on axes the collision actually changed.
        if self.vel[0] != move_vel[0] {
            self.vel_frac_xz[0] = 0;
        }
        if self.vel[2] != move_vel[2] {
            self.vel_frac_xz[1] = 0;
        }

        // Ground check: probe straight down a little.
        let down = [self.pos[0], self.pos[1] - STEP_DOWN, self.pos[2]];
        let g = trace_all(map, head, movers, self.pos, down);
        self.on_ground = g.frac < 4096 && g.normal[1] > GROUND_NY;
        self.ground_mover = if self.on_ground { g.mover } else { -1 };
        if self.on_ground {
            // Snap onto the floor and kill downward speed. Capture the impact
            // speed on the touchdown tick (was airborne) for fall damage + a
            // landing view dip before it's zeroed.
            self.pos[1] += ((down[1] - self.pos[1]) * g.frac) >> 12;
            if self.vel[1] < 0 {
                if was_air {
                    self.land_impact = -self.vel[1];
                }
                self.vel[1] = 0;
            }
        }
    }
}
