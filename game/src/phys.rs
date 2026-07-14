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
// per tick. GoldSrc applies half in PM_AddCorrectGravity before movement and
// half in PM_FixupGravityVelocity afterwards. Keep it in Q6 so trigger_gravity
// scales and jump-launch fractions survive without floats.
const GRAVITY_Q6: i32 = 2 * 64;

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
fn gravity_step_q6() -> i32 {
    unsafe { (GRAVITY_Q6 * GRAVITY_SCALE) >> 12 }
}
const MOVE_SPEED: i32 = 16; // sv_maxspeed 320 u/s (pm_shared.c:2865 clamp)
                            // sqrt(2*800*45)/20 in Q6. PM_Jump subtracts one half-gravity step before
                            // the first movement sweep, yielding GoldSrc's 12.416-unit launch displacement.
const JUMP_Q6: i32 = 859;
const STOP_SPEED: i32 = 5; // sv_stopspeed 100 u/s (PM_Friction floor)
                           // PM_AirAccelerate caps wishspd at 30 u/s = 1.5 u/tick. Keep the half-unit:
                           // rounding this to two materially over-accelerates repeated air-control ticks.
const AIR_WISH_CAP_Q6: i32 = 96;
// Long jump module (pm_shared.c:2580-93): 350*1.6 = 560 u/s forward,
// sqrt(2*800*56) = 299 u/s up, fired by a DUCKED jump while moving.
const LONGJUMP_FWD: i32 = 28;
const LONGJUMP_UP_Q6: i32 = 958;
// PM_CatagorizePosition traces the player origin exactly two units down after
// movement.  This is deliberately much shorter than the separate 18-unit
// PM_WalkMove stair descent: an eight-unit categorization probe snapped the
// ascending c1a1d player onto hanging crate 1 five ticks before GoldSrc.
const GROUND_PROBE_DOWN: i32 = 2;
const MAX_CLIP_PLANES: usize = 5;

#[inline]
fn dot(n: [i16; 3], p: [i32; 3]) -> i32 {
    ((n[0] as i32 * p[0]) + (n[1] as i32 * p[1]) + (n[2] as i32 * p[2])) >> 12
}

/// Plane projection in Q27.5. Keeping five fractional bits here lets the
/// existing `i32` cooked distance preserve GoldSrc's 1/32-unit trace epsilon.
#[inline(always)]
fn dot_q5(n: [i16; 3], p: [i32; 3]) -> i32 {
    ((n[0] as i32 * p[0]) + (n[1] as i32 * p[1]) + (n[2] as i32 * p[2])) >> 7
}

/// Exact wrapped equivalent of `(4096 * p) >> 12`, expressed as shifts so a
/// tagged positive axial plane never pays for the R3000's MULT/MFLO pair.
/// Keeping both shifts matters for out-of-range i32 inputs: returning `p`
/// directly would change the existing release-mode wrapping semantics.
#[inline(always)]
fn axial_dot_q5(p: i32) -> i32 {
    p.wrapping_shl(12) >> 7
}

#[inline(always)]
fn plane_delta(cn: &crate::map::ClipNode, p: [i32; 3]) -> i32 {
    let projection = if cn.axis == 0 {
        dot_q5(cn.n, p)
    } else {
        // `axis` comes directly from a two-bit tag, so the non-zero values are
        // exactly 1..=3. Avoid a switch/jump table in every tagged node visit.
        let coord = unsafe { *p.get_unchecked(cn.axis as usize - 1) };
        axial_dot_q5(coord)
    };
    projection.wrapping_sub(cn.dist_q5)
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

/// Exact active-player-hull result used only by the deep deterministic trace.
/// Keeping the BSP and mover result together lets a replay distinguish a bad
/// crouch hull from a stale brush mover without changing shipping telemetry.
#[cfg(feature = "deep-reference-trace")]
#[derive(Clone, Copy)]
pub struct PlayerHullProbe {
    pub frac: i32,
    pub normal: [i32; 3],
    pub startsolid: bool,
    pub mover: i32,
}

/// Deep-reference snapshot of GoldSrc's flat versus raised PM_WalkMove paths.
/// This type and every producer/consumer disappear from shipping builds.
#[cfg(feature = "deep-reference-trace")]
#[derive(Clone, Copy)]
pub struct PlayerStepProbe {
    pub head: i32,
    pub move_delta: [i32; 3],
    pub direct_frac: i32,
    pub direct_normal: [i32; 3],
    pub flat_pos: [i32; 3],
    pub up_frac: i32,
    pub up_startsolid: bool,
    pub up_pos: [i32; 3],
    pub raised_direct_frac: i32,
    pub raised_direct_normal: [i32; 3],
    pub raised_pos: [i32; 3],
    pub down_frac: i32,
    pub down_startsolid: bool,
    pub down_normal: [i32; 3],
    pub step_pos: [i32; 3],
    pub landed: bool,
    pub chose_step: bool,
}

#[cfg(feature = "deep-reference-trace")]
static mut PLAYER_STEP_TICK: u32 = 0;
#[cfg(feature = "deep-reference-trace")]
static mut PLAYER_SLIDE_CALL: u8 = 0;

#[cfg(feature = "deep-reference-trace")]
#[inline(always)]
pub fn set_player_step_tick(map_tick: u32) {
    unsafe {
        PLAYER_STEP_TICK = map_tick;
        PLAYER_SLIDE_CALL = 0;
    }
}

#[cfg(feature = "deep-reference-trace")]
#[inline(always)]
fn next_player_slide_call() -> u8 {
    unsafe {
        let call = PLAYER_SLIDE_CALL;
        PLAYER_SLIDE_CALL = PLAYER_SLIDE_CALL.wrapping_add(1);
        call
    }
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
        // Integer error diffusion can put an intended fractional endpoint
        // exactly on a blocking plane. GoldSrc's float endpoint is then just
        // across it and PM_FlyMove clips the velocity (c1a1f's ramp lip is the
        // first campaign example). Treat a solid far subtree at an exact
        // endpoint as contact: keep the integer endpoint, but return a fraction
        // one Q12 quantum short so slide_move applies the plane clip.
        if t2 == 0 && t1 != 0 {
            let side = t1 < 0;
            let far = if side { cn.c0 } else { cn.c1 };
            if point_contents(map, far, p2) == SOLID {
                let n = plane_normal(&cn);
                tr.normal = if side { [-n[0], -n[1], -n[2]] } else { n };
                tr.frac = (p2f - 1).max(p1f);
                tr.allsolid = false;
                return false;
            }
        }
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
        const EPS_Q5: i32 = 1;
        let denom = t1 - t2;
        let nudged = if t1 < 0 { t1 + EPS_Q5 } else { t1 - EPS_Q5 };
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
        let t1 = dot_q5(nd.n, p1).wrapping_sub(nd.dist_q5);
        let t2 = dot_q5(nd.n, p2).wrapping_sub(nd.dist_q5);
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

        // Hull 0 uses the same Q5 plane representation, but keeps the exact
        // split here: DROP_TO_FLOOR must land on the authored -216/-80 floors
        // rather than one whole unit above them.
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

        const EPS_Q5: i32 = 1;
        let denom = t1 - t2;
        let nudged = if t1 < 0 { t1 + EPS_Q5 } else { t1 - EPS_Q5 };
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
    // World axial rotation about `off` as the render matrix's own q12 cos/sin.
    // The axis is packed into spare high head0 bits (Y is zero/legacy) so the
    // full XYZ pendulum collision path does not grow this RAM-hot structure.
    // Identity = (4096, 0): the plain translated fast path.
    pub rc: i32,
    pub rs: i32,
}

pub const MOVER_VISUAL_DISABLED: i32 = i32::MIN;
const MOVER_ROT_AXIS_SHIFT: u32 = 29;
const MOVER_ROT_AXIS_MASK: i32 = 3 << MOVER_ROT_AXIS_SHIFT;

/// Pack runtime axis 0=Y, 1=X, 2=Z into a mover's point-head metadata.
#[inline(always)]
pub const fn mover_rotation_axis_tag(axis: u16) -> i32 {
    ((axis as i32) & 3) << MOVER_ROT_AXIS_SHIFT
}

impl Mover {
    #[inline]
    pub fn point_head(self) -> i32 {
        self.head0 & !(MOVER_VISUAL_DISABLED | MOVER_ROT_AXIS_MASK)
    }

    #[inline]
    fn visual_disabled(self) -> bool {
        self.head0 & MOVER_VISUAL_DISABLED != 0
    }

    #[inline(always)]
    fn rotation_axis(self) -> u16 {
        ((self.head0 & MOVER_ROT_AXIS_MASK) >> MOVER_ROT_AXIS_SHIFT) as u16
    }
}

/// Rotate a vector by the render's matching axial matrix. Axis 0 is the legacy
/// Y path used by doors/trams/fans; 1 and 2 add X/Z pendulum hulls.
// This transform is used by five trace paths. Keeping one shared copy saves
// several kilobytes of MIPS text while the call cost is paid only for a live
// rotated mover (the identity fast paths never reach it).
#[inline(never)]
fn rot_axis(p: [i32; 3], c: i32, s: i32, axis: u16) -> [i32; 3] {
    match axis {
        1 => [
            p[0],
            (c * p[1] - s * p[2]) >> 12,
            (s * p[1] + c * p[2]) >> 12,
        ],
        2 => [
            (c * p[0] - s * p[1]) >> 12,
            (s * p[0] + c * p[1]) >> 12,
            p[2],
        ],
        _ => [
            (c * p[0] + s * p[2]) >> 12,
            p[1],
            (-s * p[0] + c * p[2]) >> 12,
        ],
    }
}

/// Inverse (transpose) of [`rot_axis`]: world -> mover-local space.
#[inline]
fn rot_axis_inv(p: [i32; 3], c: i32, s: i32, axis: u16) -> [i32; 3] {
    rot_axis(p, c, -s, axis)
}

#[inline]
fn mover_local(mv: &Mover, p: [i32; 3]) -> [i32; 3] {
    let d = [p[0] - mv.off[0], p[1] - mv.off[1], p[2] - mv.off[2]];
    if mv.rs != 0 || mv.rc != 4096 {
        rot_axis_inv(d, mv.rc, mv.rs, mv.rotation_axis())
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
        rot_axis(mv.center, mv.rc, mv.rs, mv.rotation_axis())
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

/// True when an actor-sized segment does not enter any nearby brush collider.
///
/// Actor movement must use the submodel clip hull, not hull 0: a small actor's
/// centre can pass underneath a glass pane while the top of its bounding box
/// still intersects it (c1a0c's headcrab display is exactly that shape). A
/// mover which already contains the start point is deliberately ignored so a
/// spawned actor can escape an overlapping brush; every other mover continues
/// to block entry. This matches the start-solid convention used by `trace_all`.
pub fn actor_line_clear_movers(map: &Map, movers: &[Mover], p1: [i32; 3], p2: [i32; 3]) -> bool {
    for mv in movers {
        if mv.head <= 0 || !mover_may_touch_segment(mv, p1, p2) {
            continue;
        }
        let (q1, q2) = mover_local_segment(mv, p1, p2);
        let tr = trace(map, mv.head, q1, q2);
        if !tr.startsolid && tr.frac < 4096 {
            return false;
        }
    }
    true
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
        let t1 = dot_q5(nd.n, p1).wrapping_sub(nd.dist_q5);
        let t2 = dot_q5(nd.n, p2).wrapping_sub(nd.dist_q5);
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
        num = if dot_q5(nd.n, p).wrapping_sub(nd.dist_q5) >= 0 {
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
    let d = dot_q5(nd.n, p).wrapping_sub(nd.dist_q5);
    let radius_q5 = radius.wrapping_shl(5);
    if d > radius_q5 {
        return visual_sphere_solid_from(map, nd.c0, p, radius, depth + 1);
    }
    if d < -radius_q5 {
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
                rot_axis(t.normal, mv.rc, mv.rs, mv.rotation_axis())
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

/// Trace one translated brush entity's hull-0 subtree. Actor floor probes
/// already broad-phase the small set of brush entities touching their vertical
/// column, so walking each exact BSP segment is both cheaper and more reliable
/// than sampling points through the entity's bounding sphere. In particular,
/// wide four-unit func_wall floors can have a hundred-unit sphere radius and be
/// missed completely by sparse height samples.
pub fn trace_submodel_line(
    map: &Map,
    head0: i32,
    off: [i32; 3],
    p1: [i32; 3],
    p2: [i32; 3],
) -> Option<RayHit> {
    if head0 <= 0 {
        return None;
    }
    let q1 = [p1[0] - off[0], p1[1] - off[1], p1[2] - off[2]];
    let q2 = [p2[0] - off[0], p2[1] - off[1], p2[2] - off[2]];
    let t = trace_nodes(map, head0, q1, q2);
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
        mover: -1,
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
                rot_axis(hit.normal, mv.rc, mv.rs, mv.rotation_axis())
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
                rot_axis(t.normal, mv.rc, mv.rs, mv.rotation_axis()) // impact normal back to world space
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

/// Sign-symmetric nearest scaling used only as a recovery candidate for an
/// integer contact that landed inside a diagonal plane. Ordinary movement and
/// already-clear impacts retain the conservative arithmetic-shift path.
#[inline(always)]
fn scale12_round(v: [i32; 3], s: i32) -> [i32; 3] {
    [
        mul_q12_round(v[0], s),
        mul_q12_round(v[1], s),
        mul_q12_round(v[2], s),
    ]
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

#[inline(never)]
fn try_unstick(map: &Map, head: i32, movers: &[Mover], pos: [i32; 3]) -> Option<[i32; 3]> {
    // Actor sweeps deliberately do not report startsolid: an actor that walks
    // into the player must be escapable. Therefore this recovery path is only
    // entered for BSP penetration and needs the original BSP clearance test.
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
    #[cfg(feature = "deep-reference-trace")]
    let trace_call = next_player_slide_call();
    let mut planes = [[0i32; 3]; MAX_CLIP_PLANES];
    let mut nplanes = 0usize;
    let mut original_vel = vel;
    let primal_vel = vel;
    let mut time_left = 4096;
    for bump in 0..4 {
        if vel == [0, 0, 0] || time_left <= 0 {
            break;
        }
        let d = scale12(vel, time_left);
        let end = add(pos, d);
        let tr = trace_all(map, head, movers, pos, end);
        #[cfg(feature = "deep-reference-trace")]
        crate::reference_trace::player_slide(
            unsafe { PLAYER_STEP_TICK },
            trace_call,
            bump,
            pos,
            vel,
            d,
            end,
            time_left,
            tr.frac,
            tr.normal,
            tr.startsolid,
            tr.mover,
        );
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
            let impact_start = pos;
            let floor = add(impact_start, scale12(d, tr.frac));
            pos = floor;
            if tr.frac < 4096
                && crate::ground_logic::is_diagonal_contact_plane(tr.normal)
                && !clear_at(map, head, movers, floor)
            {
                // On a diagonal plane, independent negative-component floors
                // can turn the trace's clear backed-off fraction into an
                // integer startsolid point. Retry nearest rounding only for
                // that proven failure. This avoids the following bump's broad
                // try_unstick search without perturbing axial or clear paths.
                let nearest = add(impact_start, scale12_round(d, tr.frac));
                if clear_at(map, head, movers, nearest) {
                    pos = nearest;
                }
            }
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
const CLIMB_SPEED_Q6: i32 = 10 * PLANAR_FRAC_ONE; // MAX_CLIMB_SPEED / 20 Hz

pub struct Player {
    pub pos: [i32; 3],
    pub vel: [i32; 3],
    // Signed Q6 sub-unit horizontal velocity retained across 20 Hz ticks. The
    // two bytes occupy Player's former alignment padding, so this prevents
    // PM_Accelerate/friction from erasing small wall tangents at zero RAM cost.
    vel_frac_xz: [i8; 2],
    // Q6 error diffusion for the integer BSP origin. GoldSrc retains a float
    // origin, so a 15.875-unit velocity must not become 16 units every tick.
    move_frac_xz: [i8; 2],
    // Vertical velocity and origin residues keep GoldSrc's split-gravity and
    // fractional jump launch exact enough for deterministic hull-plane order.
    vel_frac_y: i8,
    move_frac_y: i8,
    pub on_ground: bool,
    // Cooked entity pools are far below i16::MAX. Narrowing this index funds
    // both vertical Q6 residues while preserving Player's 36-byte footprint.
    pub ground_mover: i16, // ent id under our feet (-1 world/none, -2 synthetic tram)
    // sv_maxvelocity caps this at 100 units/tick; u8 retains every legal fall.
    pub land_impact: u8, // downward speed absorbed the tick we touched down (0 = none)
    pub crouch: bool,    // hold-duck: trace the world against the shorter hull-3
}
const _: [(); 36] = [(); core::mem::size_of::<Player>()];

impl Player {
    pub fn new(pos: [i32; 3]) -> Player {
        Player {
            pos,
            vel: [0, 0, 0],
            vel_frac_xz: [0, 0],
            move_frac_xz: [0, 0],
            vel_frac_y: 0,
            move_frac_y: 0,
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
        self.move_frac_xz = [0, 0];
    }

    pub fn clear_velocity(&mut self) {
        self.vel = [0, 0, 0];
        self.vel_frac_xz = [0, 0];
        self.move_frac_xz = [0, 0];
        self.vel_frac_y = 0;
        self.move_frac_y = 0;
    }

    /// Replace a scripted vertical velocity and discard PM's old fractional
    /// state, matching a GoldSrc basevelocity/trigger assignment.
    pub fn set_vertical_velocity(&mut self, y: i32) {
        self.vel[1] = y;
        self.vel_frac_y = 0;
        self.move_frac_y = 0;
    }

    #[inline(always)]
    fn vertical_velocity_q6(&self) -> i32 {
        self.vel[1] * PLANAR_FRAC_ONE + self.vel_frac_y as i32
    }

    #[inline(always)]
    fn set_vertical_velocity_q6(&mut self, fine: i32) {
        (self.vel[1], self.vel_frac_y) = split_planar_round(fine);
    }

    #[cfg(feature = "deep-reference-trace")]
    pub fn hull_probe(&self, map: &Map, movers: &[Mover], delta: [i32; 3]) -> PlayerHullProbe {
        let end = add(self.pos, delta);
        let trace = trace_all(map, self.head(map), movers, self.pos, end);
        PlayerHullProbe {
            frac: trace.frac,
            normal: trace.normal,
            startsolid: trace.startsolid,
            mover: trace.mover,
        }
    }

    /// Exact next pre-move ballistic displacement for differential probes.
    #[cfg(feature = "deep-reference-trace")]
    pub fn next_fall_delta(&self) -> [i32; 3] {
        let fine_x = self.vel[0] * PLANAR_FRAC_ONE + self.vel_frac_xz[0] as i32;
        let fine_y = self.vertical_velocity_q6() - gravity_step_q6() / 2;
        let fine_z = self.vel[2] * PLANAR_FRAC_ONE + self.vel_frac_xz[1] as i32;
        [
            crate::ground_logic::integrate_planar_q6(fine_x, self.move_frac_xz[0]).0,
            crate::ground_logic::integrate_planar_q6(fine_y, self.move_frac_y).0,
            crate::ground_logic::integrate_planar_q6(fine_z, self.move_frac_xz[1]).0,
        ]
    }

    /// Stop one axis at a dynamic actor face and discard its sub-unit motion
    /// carry, just as a BSP plane clip inside `update` would.
    #[inline]
    pub fn block_actor_axis(&mut self, axis: usize) {
        self.vel[axis] = 0;
        if axis == 0 {
            self.vel_frac_xz[0] = 0;
            self.move_frac_xz[0] = 0;
        } else if axis == 2 {
            self.vel_frac_xz[1] = 0;
            self.move_frac_xz[1] = 0;
        } else if axis == 1 {
            self.vel_frac_y = 0;
            self.move_frac_y = 0;
        }
    }

    /// Reposition a ladder mount on the horizontal plane only when the active
    /// player hull is clear at the destination. This keeps mount assistance
    /// from pulling the player through an adjacent solid brush.
    pub fn try_set_planar_position(&mut self, map: &Map, movers: &[Mover], x: i32, z: i32) -> bool {
        let target = [x, self.pos[1], z];
        if clear_at(map, self.head(map), movers, target) {
            self.pos = target;
            self.vel[0] = 0;
            self.vel[2] = 0;
            self.vel_frac_xz = [0, 0];
            self.move_frac_xz = [0, 0];
            true
        } else {
            false
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

    #[inline(always)]
    fn half_height(&self) -> i32 {
        if self.crouch {
            18
        } else {
            36
        }
    }

    /// Duck/un-duck with HL's centred-hull origin rules
    /// (pm_shared.c:1920-2057). Grounded switches move the origin by the
    /// 18-unit half-height difference to keep the feet fixed; airborne
    /// switches keep the origin fixed. The caller owns GoldSrc's 0.4-second
    /// activation timer. Standing up is refused if the taller hull is stuck.
    pub fn set_crouch(&mut self, map: &Map, movers: &[Mover], want: bool) {
        if want == self.crouch {
            return;
        }
        let shift = crate::ground_logic::crouch_origin_shift(self.on_ground, want);
        if want {
            self.crouch = true;
            if shift != 0 {
                let shifted = [self.pos[0], self.pos[1] + shift, self.pos[2]];
                let head = self.head(map);
                let t = trace_all(map, head, movers, self.pos, shifted);
                self.pos[1] += (shift * t.frac) >> 12;
            }
        } else {
            let stand_pos = [self.pos[0], self.pos[1] + shift, self.pos[2]];
            if clear_at(map, map.hull1_head, movers, stand_pos) {
                self.crouch = false;
                self.pos = stand_pos;
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
        ladder_center: [i32; 3],
        ladder_half: [i32; 3],
    ) {
        let s = sincos::sin_q12(yaw);
        let c = sincos::sin_q12((yaw + 1024) & 0xFFF);
        let (normal_axis, normal_sign) =
            crate::ladder_logic::cardinal_normal(self.pos, ladder_center, ladder_half);
        if jump {
            // Let go: push back off the ladder (270 u/s, pm_shared.c:2131-35).
            const DISMOUNT: i32 = 14;
            self.vel = [0, 0, 0];
            self.vel[normal_axis] = normal_sign * DISMOUNT;
            self.vel_frac_xz = [0, 0];
            self.move_frac_xz = [0, 0];
            self.vel_frac_y = 0;
            self.move_frac_y = 0;
            self.on_ground = false;
            let head = self.head(map);
            let (p, v) = slide_move(map, head, movers, self.pos, self.vel);
            self.pos = p;
            self.vel = v;
            return;
        }
        let pitch_angle = (pitch as i32 & 0xFFF) as u16;
        let sp = sincos::sin_q12(pitch_angle);
        let cp = sincos::sin_q12((pitch_angle + 1024) & 0xFFF);
        let forward = [
            crate::ladder_logic::mul_q12_nearest(s, cp),
            sp,
            crate::ladder_logic::mul_q12_nearest(c, cp),
        ];
        let right = [c, 0, -s];
        let speed_q6 = if self.crouch {
            CLIMB_SPEED_Q6 / 3
        } else {
            CLIMB_SPEED_Q6
        };
        let fine = crate::ladder_logic::goldsrc_velocity_q6(
            forward,
            right,
            fwd,
            strafe,
            speed_q6,
            normal_axis,
            normal_sign,
            self.on_ground,
        );
        (self.vel[0], self.vel_frac_xz[0]) = split_planar_round(fine[0]);
        (self.vel[1], self.vel_frac_y) = split_planar_round(fine[1]);
        (self.vel[2], self.vel_frac_xz[1]) = split_planar_round(fine[2]);
        let physical_vel = self.vel;
        let (move_x, next_move_frac_x) =
            crate::ground_logic::integrate_planar_q6(fine[0], self.move_frac_xz[0]);
        let (move_y, next_move_frac_y) =
            crate::ground_logic::integrate_planar_q6(fine[1], self.move_frac_y);
        let (move_z, next_move_frac_z) =
            crate::ground_logic::integrate_planar_q6(fine[2], self.move_frac_xz[1]);
        let move_vel = [move_x, move_y, move_z];
        let head = self.head(map);
        let start = self.pos;
        #[cfg(feature = "deep-reference-trace")]
        let first = trace_all(map, head, movers, start, add(start, move_vel));
        let (mut p, mut v) = slide_move(map, head, movers, start, move_vel);
        if crate::ladder_logic::should_retry_vertical(start[1], p[1], move_y) {
            // A thin ladder beside an integer-quantized frame can make the
            // combined move hit a corner.  Gold's multi-plane float slide
            // keeps the vertical tangent; recover it without moving through
            // any solid by tracing the same hull vertically once.
            let (vertical_p, vertical_v) = slide_move(map, head, movers, start, [0, move_y, 0]);
            if vertical_p[1] != start[1] {
                p = vertical_p;
                v = vertical_v;
            }
        }
        #[cfg(feature = "deep-reference-trace")]
        crate::reference_trace::player_ladder(
            unsafe { PLAYER_STEP_TICK },
            start,
            fine,
            move_vel,
            first.frac,
            first.normal,
            first.startsolid,
            first.mover,
            p,
            v,
        );
        self.pos = p;
        self.vel = v;
        if v[0] == move_x {
            self.vel[0] = physical_vel[0];
            self.move_frac_xz[0] = next_move_frac_x;
        } else {
            self.vel_frac_xz[0] = 0;
            self.move_frac_xz[0] = 0;
        }
        if v[1] == move_y {
            self.vel[1] = physical_vel[1];
            self.move_frac_y = next_move_frac_y;
        } else {
            self.vel_frac_y = 0;
            self.move_frac_y = 0;
        }
        if v[2] == move_z {
            self.vel[2] = physical_vel[2];
            self.move_frac_xz[1] = next_move_frac_z;
        } else {
            self.vel_frac_xz[1] = 0;
            self.move_frac_xz[1] = 0;
        }
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
        self.move_frac_xz = [0, 0];
        self.vel_frac_y = 0;
        self.move_frac_y = 0;
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
    #[cfg_attr(feature = "deep-reference-trace", inline(never))]
    pub fn update(
        &mut self,
        map: &Map,
        movers: &[Mover],
        fwd: i32,
        strafe: i32,
        jump: bool,
        duck_jump: bool,
        yaw: u16,
    ) {
        self.land_impact = 0;
        let was_air = !self.on_ground;
        // GoldSrc records the fall speed and runs PM_AddCorrectGravity /
        // PM_Jump before choosing PM_WalkMove versus PM_AirMove. In
        // particular, a successful jump clears onground before friction and
        // acceleration, so its first horizontal command is capped to the
        // 30-u/s air wish speed. Doing horizontal movement first launched the
        // c1a1d crate jump at ground speed; the combined sweep hit the platform
        // edge and discarded part of the otherwise-correct vertical rise.
        let fall_speed_q6 = if was_air {
            (-self.vertical_velocity_q6()).max(0)
        } else {
            0
        };
        let gravity_q6 = gravity_step_q6();
        let gravity_pre_q6 = gravity_q6 / 2;
        let gravity_post_q6 = gravity_q6 - gravity_pre_q6;
        let mut vertical_q6 = self.vertical_velocity_q6();
        if self.on_ground {
            vertical_q6 = 0;
            if jump {
                let moving = self.vel[0].abs() + self.vel[2].abs() > 2;
                // PM_Jump accepts bInDuck as well as FL_DUCKING. On the first
                // frame of a ground duck+jump the standing collision hull is
                // intentionally still active, but the long-jump module must
                // already see the pending duck request.
                let longjumping = unsafe { LONGJUMP } && (self.crouch || duck_jump) && moving;
                if longjumping {
                    // Ducked jump with the module: launch along the move
                    // direction at 560 u/s, 299 u/s up (pm_shared.c:2580-93).
                    // This precedes PM_AirMove in the original code, so apply
                    // it before the horizontal acceleration block below.
                    let (vx, vz) = (self.vel[0], self.vel[2]);
                    let speed = vx.abs().max(vz.abs()) + vx.abs().min(vz.abs()) * 3 / 8;
                    self.vel[0] = vx * LONGJUMP_FWD / speed.max(1);
                    self.vel[2] = vz * LONGJUMP_FWD / speed.max(1);
                    self.vel_frac_xz = [0, 0];
                    vertical_q6 = LONGJUMP_UP_Q6;
                } else {
                    vertical_q6 = JUMP_Q6;
                }
                // PM_Jump calls PM_FixupGravityVelocity once before AirMove;
                // PlayerMove calls it again after movement below.
                vertical_q6 -= gravity_pre_q6;
                self.on_ground = false;
            }
        } else {
            vertical_q6 -= gravity_pre_q6;
        }
        self.set_vertical_velocity_q6(vertical_q6);

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
        let wishspeed_q6 = if wishmag_q12 > 0 {
            ((wishmag_q12 * MOVE_SPEED * PLANAR_FRAC_ONE + 2048) >> 12)
                .clamp(1, MOVE_SPEED * PLANAR_FRAC_ONE)
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
        if wishspeed_q6 > 0 {
            // PM_Accelerate (pm_shared.c:990) / PM_AirAccelerate (:1279):
            // current speed ALONG the wish direction; add at most
            // accel(10) * wishspeed * dt = wishspeed/2 per tick, never past
            // the target. In the air the target caps at 30 u/s but the accel
            // step still scales off the full wishspeed.
            let dirx = wx * 4096 / wishmag_q12;
            let dirz = wz * 4096 / wishmag_q12;
            let current_dot_q18 = fine_x * dirx + fine_z * dirz;
            let current_q6 = if current_dot_q18 >= 0 {
                (current_dot_q18 + 2048) >> 12
            } else {
                -((-current_dot_q18 + 2048) >> 12)
            };
            let target_q6 = if self.on_ground {
                wishspeed_q6
            } else {
                wishspeed_q6.min(AIR_WISH_CAP_Q6)
            };
            let addspeed_q6 = target_q6 - current_q6;
            if addspeed_q6 > 0 {
                let take_q6 = (wishspeed_q6 / 2).max(1).min(addspeed_q6);
                fine_x += mul_q12_round(take_q6, dirx);
                fine_z += mul_q12_round(take_q6, dirz);
            }
        }
        (self.vel[0], self.vel_frac_xz[0]) = split_planar_round(fine_x);
        (self.vel[2], self.vel_frac_xz[1]) = split_planar_round(fine_z);

        // Move with stair-stepping: a plain slide, then (when grounded and
        // moving) an up/forward/down "step" -- keep whichever advanced further
        // along the ground, so the player climbs stairs/thresholds <= STEP_UP.
        let head = self.head(map);
        let start = self.pos;
        // GoldSrc integrates its float origin by the float velocity.  Dither
        // the integer hull displacement with signed Q6 carries so 127/128
        // input travels 127 units every eight ticks and the 12.416-unit GoldSrc
        // jump launch does not become the same integer displacement forever.
        let motion_fine_x = self.vel[0] * PLANAR_FRAC_ONE + self.vel_frac_xz[0] as i32;
        let motion_fine_y = self.vertical_velocity_q6();
        let motion_fine_z = self.vel[2] * PLANAR_FRAC_ONE + self.vel_frac_xz[1] as i32;
        #[cfg(feature = "deep-reference-trace")]
        let prior_move_frac_y = self.move_frac_y;
        let (move_x, next_move_frac_x) =
            crate::ground_logic::integrate_planar_q6(motion_fine_x, self.move_frac_xz[0]);
        let (move_y, next_move_frac_y) =
            crate::ground_logic::integrate_planar_q6(motion_fine_y, self.move_frac_y);
        let (move_z, next_move_frac_z) =
            crate::ground_logic::integrate_planar_q6(motion_fine_z, self.move_frac_xz[1]);
        let physical_vel = self.vel;
        #[cfg(feature = "deep-reference-trace")]
        crate::reference_trace::player_motion(
            unsafe { PLAYER_STEP_TICK },
            start,
            was_air,
            jump,
            self.ground_mover as i32,
            [motion_fine_x, motion_fine_y, motion_fine_z],
            prior_move_frac_y,
            [move_x, move_y, move_z],
            next_move_frac_y,
        );
        // PM_WalkMove saves the velocity from before PM_FlyMove, restores it for
        // the raised alternative, and only then chooses the farther result.  In
        // particular, the flat pass is allowed to clip both horizontal axes to
        // zero at the face of a stair; using that clipped velocity for the step
        // pass makes the raised route motionless and wedges the player against
        // even an 8-unit threshold (the c0a0e tram-platform exit).
        let move_vel = [move_x, move_y, move_z];
        #[cfg(feature = "deep-reference-trace")]
        let direct = trace_all(
            map,
            head,
            movers,
            start,
            [start[0] + move_x, start[1], start[2] + move_z],
        );
        let (flat_pos, flat_vel) = slide_move(map, head, movers, start, move_vel);

        if self.on_ground && (move_vel[0] != 0 || move_vel[2] != 0) {
            // `pos` is the conservative integer ceiling of GoldSrc's float
            // ground contact. Apply its retained negative Q6 remainder when
            // selecting the integer raised endpoint; otherwise a contact at
            // -132.89 becomes -114 rather than -114.89 and steps onto a ledge
            // one tick early.
            // The raised collision probe must stay on the solid/downward side
            // of the fractional endpoint. If the real origin is -130.47,
            // GoldSrc probes from -112.47; integer -112 can clear a lip that
            // should clip the move, so use -113 (17 whole units from the
            // conservative -130 origin). Exact contacts still raise all 18.
            let step_up = STEP_UP - i32::from(self.move_frac_y < 0);
            let up_end = [start[0], start[1] + step_up, start[2]];
            let tup = trace_all(map, head, movers, start, up_end);
            let up_pos = [start[0], start[1] + ((step_up * tup.frac) >> 12), start[2]];
            #[cfg(feature = "deep-reference-trace")]
            let raised_direct = trace_all(
                map,
                head,
                movers,
                up_pos,
                [up_pos[0] + move_vel[0], up_pos[1], up_pos[2] + move_vel[2]],
            );
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
            #[cfg(feature = "deep-reference-trace")]
            crate::reference_trace::player_step(
                unsafe { PLAYER_STEP_TICK },
                PlayerStepProbe {
                    head,
                    move_delta: move_vel,
                    direct_frac: direct.frac,
                    direct_normal: direct.normal,
                    flat_pos,
                    up_frac: tup.frac,
                    up_startsolid: tup.startsolid,
                    up_pos,
                    raised_direct_frac: raised_direct.frac,
                    raised_direct_normal: raised_direct.normal,
                    raised_pos: sp,
                    down_frac: tdn.frac,
                    down_startsolid: tdn.startsolid,
                    down_normal: tdn.normal,
                    step_pos,
                    landed,
                    chose_step,
                },
            );
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
        // slide_move operates on this tick's integer displacement. Preserve
        // the independently retained Q6 physical velocity when an axis was
        // unobstructed; otherwise accept the clipped result and discard both
        // velocity and position residue on that axis.
        let moved_vel = self.vel;
        if moved_vel[0] != move_vel[0] {
            self.vel_frac_xz[0] = 0;
            self.move_frac_xz[0] = 0;
        } else {
            self.vel[0] = physical_vel[0];
            self.move_frac_xz[0] = next_move_frac_x;
        }
        if moved_vel[2] != move_vel[2] {
            self.vel_frac_xz[1] = 0;
            self.move_frac_xz[1] = 0;
        } else {
            self.vel[2] = physical_vel[2];
            self.move_frac_xz[1] = next_move_frac_z;
        }
        let mut next_vertical_q6 = if moved_vel[1] != move_vel[1] {
            self.vel_frac_y = 0;
            self.move_frac_y = 0;
            moved_vel[1] * PLANAR_FRAC_ONE
        } else {
            self.move_frac_y = next_move_frac_y;
            motion_fine_y
        };

        // Ground check: probe straight down a little.
        let down = [
            self.pos[0],
            self.pos[1] - GROUND_PROBE_DOWN,
            self.pos[2],
        ];
        let g = trace_all(map, head, movers, self.pos, down);
        self.on_ground = g.frac < 4096 && g.normal[1] > GROUND_NY;
        self.ground_mover = if self.on_ground { g.mover as i16 } else { -1 };
        if self.on_ground {
            // Snap onto the floor and consume the complete vertical component.
            // PM_WalkMove must not carry an upward component clipped from a
            // stair/ramp plane into the next frame: doing so made a grounded
            // c1a0e step launch the player for nine air-acceleration ticks.
            // A real downward touchdown still records fall damage/view dip.
            let ground_start_y = self.pos[1];
            (self.pos[1], self.move_frac_y) =
                crate::ground_logic::ground_contact_q6(ground_start_y, down[1], g.frac);
            self.set_vertical_velocity_q6(0);
            #[cfg(feature = "deep-reference-trace")]
            crate::reference_trace::player_ground(
                unsafe { PLAYER_STEP_TICK },
                ground_start_y,
                down[1],
                g.frac,
                g.normal,
                self.pos[1],
                self.move_frac_y,
            );
            if was_air && fall_speed_q6 > 0 {
                let impact = (fall_speed_q6 + PLANAR_FRAC_HALF) >> PLANAR_FRAC_BITS;
                self.land_impact = impact.min(u8::MAX as i32) as u8;
            }
        } else {
            // PM_FixupGravityVelocity: the second half-step is visible in the
            // velocity reported after the frame but not in this frame's origin.
            next_vertical_q6 -= gravity_post_q6;
            self.set_vertical_velocity_q6(next_vertical_q6);
        }
    }
}
