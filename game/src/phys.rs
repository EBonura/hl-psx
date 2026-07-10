//! Player physics on the BSP clip hull -- a fixed-point port of Quake/GoldSrc
//! `SV_RecursiveHullCheck` + a slide-move. The clip hull (`hull1`) is the BSP
//! pre-expanded by the player box, so we trace the player ORIGIN as a point.
//!
//! All world units are i32; plane normals are i16 in 1.3.12 (×4096); fractions
//! along a move are Q0.12 (4096 = full). No floats (no FPU on the PS1).

use psx_math::sincos;

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
const CONTACT_NUDGE: i32 = 2;
const STOP_EPSILON: i32 = 1;
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
        tr.normal = if side {
            [-n[0], -n[1], -n[2]]
        } else {
            n
        };
        tr.frac = midf;
        return false;
    }
}

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

#[inline]
fn trace_clear(map: &Map, head: i32, p1: [i32; 3], p2: [i32; 3]) -> bool {
    let mut tr = ClearTrace {
        startsolid: false,
    };
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
    pub head0: i32, // point-hull root (hitscans; 0 = fall back to head)
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

/// Rotate a vector by the render's rotate_y(c, s): x' = c·x + s·z, z' = −s·x + c·z.
#[inline]
fn rot_y(p: [i32; 3], c: i32, s: i32) -> [i32; 3] {
    [(c * p[0] + s * p[2]) >> 12, p[1], (-s * p[0] + c * p[2]) >> 12]
}

/// Inverse (transpose) of [`rot_y`]: world -> mover-local space.
#[inline]
fn rot_y_inv(p: [i32; 3], c: i32, s: i32) -> [i32; 3] {
    [(c * p[0] - s * p[2]) >> 12, p[1], (s * p[0] + c * p[2]) >> 12]
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

#[inline]
fn mover_may_touch_segment(mv: &Mover, p1: [i32; 3], p2: [i32; 3]) -> bool {
    if mv.radius <= 0 {
        return true;
    }
    let c = [
        mv.center[0] + mv.off[0],
        mv.center[1] + mv.off[1],
        mv.center[2] + mv.off[2],
    ];
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
        let head = if mv.head0 > 0 { mv.head0 } else { mv.head };
        if head <= 0 {
            continue;
        }
        if !mover_may_touch_segment(mv, p1, p2) {
            continue;
        }
        let q1 = mover_local(mv, p1);
        let q2 = mover_local(mv, p2);
        if !trace_clear(map, head, q1, q2) {
            return false;
        }
    }
    true
}

/// True when the segment does not hit the static world point hull.
pub fn line_clear_world(map: &Map, p1: [i32; 3], p2: [i32; 3]) -> bool {
    if map.hull0_head <= 0 {
        return true;
    }
    trace_clear(map, map.hull0_head, p1, p2)
}

/// Trace a point ray through static world and active mover hulls.
pub fn trace_line(map: &Map, movers: &[Mover], p1: [i32; 3], p2: [i32; 3]) -> Option<RayHit> {
    if map.hull0_head <= 0 {
        return None;
    }
    let t = trace_all(map, map.hull0_head, movers, p1, p2, true);
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

/// Snap a point origin onto floor geometry near it.
pub fn snap_to_ground(
    map: &Map,
    movers: &[Mover],
    pos: [i32; 3],
    probe_up: i32,
    probe_down: i32,
) -> Option<[i32; 3]> {
    if map.hull0_head <= 0 {
        return None;
    }
    let p1 = [pos[0], pos[1] + probe_up.max(0), pos[2]];
    let p2 = [pos[0], pos[1] - probe_down.max(0), pos[2]];
    let t = trace_all(map, map.hull0_head, movers, p1, p2, true);
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
fn trace_all(
    map: &Map,
    world_head: i32,
    movers: &[Mover],
    p1: [i32; 3],
    p2: [i32; 3],
    point: bool,
) -> Trace {
    let mut best = trace(map, world_head, p1, p2);
    for mv in movers {
        let head = if point && mv.head0 > 0 { mv.head0 } else { mv.head };
        if head <= 0 {
            continue; // no clip hull for this submodel
        }
        if !mover_may_touch_segment(mv, p1, p2) {
            continue;
        }
        let q1 = mover_local(mv, p1);
        let q2 = mover_local(mv, p2);
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
    let mut out = [
        v[0] - ((n[0] * proj) >> 12),
        v[1] - ((n[1] * proj) >> 12),
        v[2] - ((n[2] * proj) >> 12),
    ];
    let mut i = 0;
    while i < 3 {
        if out[i].abs() <= STOP_EPSILON {
            out[i] = 0;
        }
        i += 1;
    }
    out
}

fn nudge_out(pos: [i32; 3], n: [i32; 3]) -> [i32; 3] {
    add(pos, scale12(n, CONTACT_NUDGE))
}

fn clear_at(map: &Map, head: i32, movers: &[Mover], pos: [i32; 3]) -> bool {
    !trace_all(map, head, movers, pos, pos, false).startsolid
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
        let tr = trace_all(map, head, movers, pos, end, false);
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
        pos = nudge_out(pos, tr.normal);
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
    pub on_ground: bool,
    pub ground_mover: i32, // ent id of the mover under our feet (-1 = world/none)
    pub land_impact: i32,  // downward speed absorbed the tick we touched down (0 = none)
    pub crouch: bool,      // hold-duck: trace the world against the shorter hull-3
}

impl Player {
    pub fn new(pos: [i32; 3]) -> Player {
        Player {
            pos,
            vel: [0, 0, 0],
            on_ground: false,
            ground_mover: -1,
            land_impact: 0,
            crouch: false,
        }
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
                let t = trace_all(map, head, movers, self.pos, up, false);
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
            let blocked = trace_all(map, head, movers, self.pos, ahead, false).frac < 4096;
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
        let mut wish_x = (wx * MOVE_SPEED) >> 12;
        let mut wish_z = (wz * MOVE_SPEED) >> 12;
        // Clamp the wish speed to MOVE_SPEED so a full diagonal isn't ~1.41x fast
        // (octagonal |v| approximation, no sqrt needed).
        let (ax, az) = (wish_x.abs(), wish_z.abs());
        let mut wishspeed = ax.max(az) + ax.min(az) * 3 / 8;
        if wishspeed > MOVE_SPEED {
            wish_x = wish_x * MOVE_SPEED / wishspeed;
            wish_z = wish_z * MOVE_SPEED / wishspeed;
            wishspeed = MOVE_SPEED;
        }

        if self.on_ground {
            // PM_Friction (pm_shared.c:1202): drop = max(speed, stopspeed)
            // * friction(4) * dt(0.05) = max(speed, 5) / 5 per tick.
            let (vx, vz) = (self.vel[0], self.vel[2]);
            let speed = vx.abs().max(vz.abs()) + vx.abs().min(vz.abs()) * 3 / 8;
            if speed > 0 {
                let drop = speed.max(STOP_SPEED) / 5;
                let ns = (speed - drop.max(1)).max(0);
                self.vel[0] = vx * ns / speed;
                self.vel[2] = vz * ns / speed;
            }
        }
        if wishspeed > 0 {
            // PM_Accelerate (pm_shared.c:990) / PM_AirAccelerate (:1279):
            // current speed ALONG the wish direction; add at most
            // accel(10) * wishspeed * dt = wishspeed/2 per tick, never past
            // the target. In the air the target caps at 30 u/s but the accel
            // step still scales off the full wishspeed.
            let dirx = wish_x * 4096 / wishspeed;
            let dirz = wish_z * 4096 / wishspeed;
            let current = (self.vel[0] * dirx + self.vel[2] * dirz) >> 12;
            let target = if self.on_ground {
                wishspeed
            } else {
                wishspeed.min(AIR_WISH_CAP)
            };
            let addspeed = target - current;
            if addspeed > 0 {
                let take = (wishspeed / 2).max(1).min(addspeed);
                self.vel[0] += (dirx * take) >> 12;
                self.vel[2] += (dirz * take) >> 12;
            }
        }

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
        let (flat_pos, flat_vel) = slide_move(map, head, movers, start, self.vel);
        self.vel = flat_vel;

        if self.on_ground && (self.vel[0] != 0 || self.vel[2] != 0) {
            let up_end = [start[0], start[1] + STEP_UP, start[2]];
            let tup = trace_all(map, head, movers, start, up_end, false);
            let up_pos = [start[0], start[1] + ((STEP_UP * tup.frac) >> 12), start[2]];
            let (sp, _) = slide_move(map, head, movers, up_pos, [self.vel[0], 0, self.vel[2]]);
            let dn_end = [sp[0], sp[1] - STEP_UP * 2, sp[2]];
            let tdn = trace_all(map, head, movers, sp, dn_end, false);
            let step_pos = [sp[0], sp[1] - (((STEP_UP * 2) * tdn.frac) >> 12), sp[2]];
            let landed = tdn.frac < 4096 && tdn.normal[1] > GROUND_NY;
            if landed && dist_xz(start, step_pos) > dist_xz(start, flat_pos) {
                self.pos = step_pos;
            } else {
                self.pos = flat_pos;
            }
        } else {
            self.pos = flat_pos;
        }

        // Ground check: probe straight down a little.
        let down = [self.pos[0], self.pos[1] - STEP_DOWN, self.pos[2]];
        let g = trace_all(map, head, movers, self.pos, down, false);
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
