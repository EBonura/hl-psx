// SPDX-License-Identifier: GPL-2.0-or-later
//
// The recursive hull trace and the multi-plane slide move in this file are
// fixed-point Rust adaptations of Quake's GPL-licensed SV_RecursiveHullCheck
// and SV_FlyMove (WinQuake/world.c, WinQuake/sv_phys.c), modified for this
// project's cooked GoldSrc hulls and integer arithmetic. Quake source:
// Copyright (C) 1996-1997 Id Software, Inc. See THIRD_PARTY_NOTICES.md.
// Modified for HL-PSX by Emanuele Bonura, 2025-2026.
//
//! Player physics on the BSP clip hull -- a fixed-point adaptation of Quake's
//! `SV_RecursiveHullCheck`, informed by GoldSrc movement behaviour, plus a
//! slide-move. The clip hull (`hull1`) is the BSP pre-expanded by the player
//! box, so we trace the player ORIGIN as a point.
//!
//! All world units are i32; plane normals are i16 in 1.3.12 (×4096); fractions
//! along a move are Q0.12 (4096 = full). No floats (no FPU on the PS1).

use psx_gte::math::Mat3I16;
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

/// Q14 plane projection in Q27.5. Keeping five fractional bits here lets the
/// existing `i32` cooked distance preserve GoldSrc's 1/32-unit trace epsilon.
#[inline(always)]
fn dot_q5(n: [i16; 3], p: [i32; 3]) -> i32 {
    ((n[0] as i32 * p[0]) + (n[1] as i32 * p[1]) + (n[2] as i32 * p[2]))
        >> (crate::map::PLANE_NORMAL_FRAC_BITS - 5)
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
        _ => plane_normal_q12(cn.n),
    }
}

/// Recover the exact legacy Q12 impact normal from a compatible Q14 plane.
/// Plane-side walks never pay this conversion; it runs only when a trace hits.
#[inline(always)]
fn plane_normal_q12(n: [i16; 3]) -> [i32; 3] {
    #[inline(always)]
    fn component(value: i16) -> i32 {
        let value = value as i32;
        if value < 0 {
            -((-value) >> (crate::map::PLANE_NORMAL_FRAC_BITS - 12))
        } else {
            value >> (crate::map::PLANE_NORMAL_FRAC_BITS - 12)
        }
    }
    [component(n[0]), component(n[1]), component(n[2])]
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
        // endpoint as contact, but only after walking the near subtree. An
        // eager return here lets an ancestor floor plane at the endpoint hide
        // an earlier stair-top impact deeper in that subtree (c1a0d).
        if t2 == 0 && t1 != 0 {
            let side = t1 < 0;
            let near = if side { cn.c1 } else { cn.c0 };
            let far = if side { cn.c0 } else { cn.c1 };
            if point_contents(map, far, p2) == SOLID {
                if !recurse(map, near, p1f, p2f, p1, p2, tr, depth + 1) {
                    return false;
                }
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
        let n = plane_normal_q12(nd.n);
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
        if t2 == 0 && t1 != 0 {
            let side = t1 < 0;
            let near = if side { cn.c1 } else { cn.c0 };
            let far = if side { cn.c0 } else { cn.c1 };
            if point_contents(map, far, p2) == SOLID {
                if !recurse_clear(map, near, p1, p2, tr, depth + 1) {
                    return false;
                }
                return false;
            }
        }
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
    // World-space centre of the conservative broad-phase sphere. It is
    // transformed once when the per-frame mover table is built, not once for
    // every actor/trace segment tested against this mover.
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
/// `Mover.rc` sentinel: `rs` then contains reflected pitch/yaw/roll q8 bytes.
/// This preserves the RAM-hot Mover layout while allowing composite authored
/// base transforms plus an animated rotation axis.
pub const MOVER_FULL_ROTATION: i32 = i32::MIN;
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

/// Matrix shared by renderer and collision for reflected GoldSrc Euler bytes.
#[inline(never)]
pub fn packed_euler_rotation(packed: u32) -> Mat3I16 {
    // Ry(yaw) * Rz(pitch) * Rx(roll), expanded here instead of constructing
    // and multiplying three generic matrices. Besides being cheaper on the
    // R3000A, this keeps the all-prop orientation path inside the PS1 image.
    let pitch = (packed & 0xff) as usize;
    let yaw = ((packed >> 8) & 0xff) as usize;
    let roll = ((packed >> 16) & 0xff) as usize;
    let sp = sincos::SIN_TABLE[pitch] as i32;
    let cp = sincos::SIN_TABLE[(pitch + 64) & 0xff] as i32;
    let sy = sincos::SIN_TABLE[yaw] as i32;
    let cy = sincos::SIN_TABLE[(yaw + 64) & 0xff] as i32;
    let sr = sincos::SIN_TABLE[roll] as i32;
    let cr = sincos::SIN_TABLE[(roll + 64) & 0xff] as i32;

    // First compose Ry * Rz. Keeping these intermediate shifts before the
    // final * Rx preserves Mat3I16::mul's fixed-point rounding exactly.
    let b00 = (cy * cp) >> 12;
    let b01 = (-cy * sp) >> 12;
    let b20 = (-sy * cp) >> 12;
    let b21 = (sy * sp) >> 12;

    Mat3I16 {
        m: [
            [
                b00 as i16,
                ((b01 * cr + sy * sr) >> 12) as i16,
                ((-b01 * sr + sy * cr) >> 12) as i16,
            ],
            [
                sp as i16,
                ((cp * cr) >> 12) as i16,
                ((-cp * sr) >> 12) as i16,
            ],
            [
                b20 as i16,
                ((b21 * cr + cy * sr) >> 12) as i16,
                ((-b21 * sr + cy * cr) >> 12) as i16,
            ],
        ],
    }
}

#[inline(never)]
fn mat_world(r: &Mat3I16, p: [i32; 3]) -> [i32; 3] {
    [
        ((r.m[0][0] as i32 * p[0]) + (r.m[0][1] as i32 * p[1]) + (r.m[0][2] as i32 * p[2])) >> 12,
        ((r.m[1][0] as i32 * p[0]) + (r.m[1][1] as i32 * p[1]) + (r.m[1][2] as i32 * p[2])) >> 12,
        ((r.m[2][0] as i32 * p[0]) + (r.m[2][1] as i32 * p[1]) + (r.m[2][2] as i32 * p[2])) >> 12,
    ]
}

#[inline(never)]
fn mat_local(r: &Mat3I16, p: [i32; 3]) -> [i32; 3] {
    [
        ((r.m[0][0] as i32 * p[0]) + (r.m[1][0] as i32 * p[1]) + (r.m[2][0] as i32 * p[2])) >> 12,
        ((r.m[0][1] as i32 * p[0]) + (r.m[1][1] as i32 * p[1]) + (r.m[2][1] as i32 * p[2])) >> 12,
        ((r.m[0][2] as i32 * p[0]) + (r.m[1][2] as i32 * p[1]) + (r.m[2][2] as i32 * p[2])) >> 12,
    ]
}

#[inline(never)]
fn mover_world_vector(mv: &Mover, p: [i32; 3]) -> [i32; 3] {
    if mv.rc == MOVER_FULL_ROTATION {
        mat_world(&packed_euler_rotation(mv.rs as u32), p)
    } else if mv.rs != 0 || mv.rc != 4096 {
        rot_axis(p, mv.rc, mv.rs, mv.rotation_axis())
    } else {
        p
    }
}

#[inline(never)]
fn mover_local(mv: &Mover, p: [i32; 3]) -> [i32; 3] {
    let d = [p[0] - mv.off[0], p[1] - mv.off[1], p[2] - mv.off[2]];
    if mv.rc == MOVER_FULL_ROTATION {
        mat_local(&packed_euler_rotation(mv.rs as u32), d)
    } else if mv.rs != 0 || mv.rc != 4096 {
        rot_axis_inv(d, mv.rc, mv.rs, mv.rotation_axis())
    } else {
        d
    }
}

/// Convert a world impact point and normal into the same entity-local space
/// used by the mover's hull-0 trace. This is a cold crowbar/material lookup,
/// kept out of the movement hot paths.
pub fn mover_local_impact(mv: &Mover, p: [i32; 3], n: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    let p = mover_local(mv, p);
    let n = if mv.rc == MOVER_FULL_ROTATION {
        mat_local(&packed_euler_rotation(mv.rs as u32), n)
    } else if mv.rs != 0 || mv.rc != 4096 {
        rot_axis_inv(n, mv.rc, mv.rs, mv.rotation_axis())
    } else {
        n
    };
    (p, n)
}

#[inline]
pub fn longjump_enabled() -> bool {
    unsafe { LONGJUMP }
}

/// Transform both endpoints together so the yaw/identity dispatch and its
/// sizeable fixed-point matrix path exist once instead of being cloned into
/// every point/clip trace loop twice.
#[inline(never)]
fn mover_local_segment(mv: &Mover, p1: [i32; 3], p2: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    (mover_local(mv, p1), mover_local(mv, p2))
}

#[inline]
pub fn mover_bounds_center(
    center: [i32; 3],
    off: [i32; 3],
    rc: i32,
    rs: i32,
    head0: i32,
) -> [i32; 3] {
    let local = if rc == MOVER_FULL_ROTATION {
        mat_world(&packed_euler_rotation(rs as u32), center)
    } else if rs != 0 || rc != 4096 {
        let axis = ((head0 & MOVER_ROT_AXIS_MASK) >> MOVER_ROT_AXIS_SHIFT) as u16;
        rot_axis(center, rc, rs, axis)
    } else {
        center
    };
    [local[0] + off[0], local[1] + off[1], local[2] + off[2]]
}

const SWIM_SPEED: i32 = 13; // water wishspeed = 0.8 * maxspeed = 256 u/s (pm_shared.c:1356)
const SWIM_SINK: i32 = 3; // idle sink -60 u/s (pm_shared.c:1341)
const SWIM_PADDLE: i32 = 5; // jump in water swims up 100 u/s (pm_shared.c:2513)
                            // PM_CheckWaterJump launches at 225 u/s. Keep the quarter-unit which the old
                            // whole-unit approximation lost: 225 / 20 Hz = 11.25 u/tick.
const WATERJUMP_UP_Q6: i32 = 225 * PLANAR_FRAC_ONE / 20;
const WATERJUMP_TICKS: u8 = 40; // GoldSrc waterjumptime is 2000 ms at 50 ms/tick
const WATERJUMP_PUSH_Q6: i32 = 50 * PLANAR_FRAC_ONE / 20;
const WATERJUMP_PROBE: i32 = 24;
const WATERJUMP_LOW_HEIGHT: i32 = 8;
const WATERJUMP_MAX_WALL_NY: i32 = 410; // fabs(normal.z) < 0.1 in GoldSrc
const WATERJUMP_MIN_FALL_Q6: i32 = -180 * PLANAR_FRAC_ONE / 20;

/// GoldSrc keeps this beside the player as `waterjumptime` + `movedir`.
/// Keeping it map-local preserves Player's deliberately fixed 36-byte layout.
pub struct WaterJumpState {
    ticks: u8,
    dir_x_q6: i16,
    dir_z_q6: i16,
}

impl WaterJumpState {
    pub const fn new() -> Self {
        Self {
            ticks: 0,
            dir_x_q6: 0,
            dir_z_q6: 0,
        }
    }

    #[inline(always)]
    pub fn active(&self) -> bool {
        self.ticks != 0
    }

    #[inline(always)]
    fn clear(&mut self) {
        self.ticks = 0;
    }
}

/// All-zero twin of [`NO_MOVER`], used only so mover tables land in `.bss`
/// rather than `.data`; the real default is installed at boot.
pub const ZERO_MOVER: Mover = Mover {
    head: 0,
    head0: 0,
    off: [0, 0, 0],
    center: [0, 0, 0],
    radius: 0,
    id: 0,
    rc: 0,
    rs: 0,
};

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
    let c = mv.center;
    let r = mv.radius;
    // Test the segment AABB without materialising min/max for every axis.
    // `(centre + radius) < both endpoints` and the mirrored high-side test are
    // exactly equivalent to the old expanded interval, but compile to straight
    // R3000 comparisons instead of three branch-heavy min/max diamonds.
    let overlaps = |axis: usize| {
        let low = c[axis] + r;
        let high = c[axis] - r;
        !((low < p1[axis] && low < p2[axis]) || (high > p1[axis] && high > p2[axis]))
    };
    overlaps(0) && overlaps(1) && overlaps(2)
}

#[inline(never)]
fn mover_may_touch_bounds(mv: &Mover, low: &[i32; 3], high: &[i32; 3]) -> bool {
    if mv.radius <= 0 {
        return true;
    }
    let c = mv.center;
    let r = mv.radius;
    let overlaps = |axis: usize| c[axis] + r >= low[axis] && c[axis] - r <= high[axis];
    overlaps(0) && overlaps(1) && overlaps(2)
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
        // `NAV_CHORD_REACHED`: a step that ends exactly on a brush's expanded
        // hull plane has arrived, not collided.
        if !tr.startsolid && tr.frac < NAV_CHORD_REACHED {
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
            let center = mv.center;
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
            best.normal = mover_world_vector(mv, t.normal);
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

/// Point sweep for a translated pushable face. Ordinary interaction/LOS rays
/// treat `startsolid` as no hit, but that lets a box sample already quantized
/// onto a wall advance farther through it. Here a startsolid sample blocks when
/// its endpoint remains in the same solid and is ignored only while escaping.
/// This is a cold func_pushable path and adds no work to player movement.
pub fn trace_pushable_sweep(
    map: &Map,
    movers: &[Mover],
    p1: [i32; 3],
    p2: [i32; 3],
) -> Option<RayHit> {
    let world = trace_nodes(map, 0, p1, p2);
    let world_end_solid = world.startsolid && node_point_solid_from(map, 0, p2);
    let mut best_frac =
        crate::pushable::blocking_sweep_fraction(world.startsolid, world_end_solid, world.frac);
    let mut best_normal = world.normal;
    let mut best_mover = -1;

    for mv in movers {
        if !mover_may_touch_segment(mv, p1, p2) {
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
        let end_solid = hit.startsolid
            && if point_head > 0 {
                node_point_solid_from(map, point_head, q2)
            } else {
                trace(map, mv.head, q2, q2).startsolid
            };
        let Some(frac) =
            crate::pushable::blocking_sweep_fraction(hit.startsolid, end_solid, hit.frac)
        else {
            continue;
        };
        if best_frac.map_or(true, |best| frac < best) {
            best_frac = Some(frac);
            best_normal = mover_world_vector(mv, hit.normal);
            best_mover = mv.id;
        }
    }

    let frac = best_frac?;
    Some(RayHit {
        frac,
        pos: [
            p1[0] + (((p2[0] - p1[0]) * frac) >> 12),
            p1[1] + (((p2[1] - p1[1]) * frac) >> 12),
            p1[2] + (((p2[2] - p1[2]) * frac) >> 12),
        ],
        normal: best_normal,
        mover: best_mover,
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
            best_normal = mover_world_vector(mv, hit.normal);
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
    // Player slide traces test the same segment against every active mover.
    // Resolve its bounds once instead of repeating two endpoint comparisons
    // per axis in every broad-phase call.
    let low = [p1[0].min(p2[0]), p1[1].min(p2[1]), p1[2].min(p2[2])];
    let high = [p1[0].max(p2[0]), p1[1].max(p2[1]), p1[2].max(p2[2])];
    for mv in movers {
        let head = mv.head;
        if head <= 0 {
            continue; // no clip hull for this submodel
        }
        if !mover_may_touch_bounds(mv, &low, &high) {
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
            best.normal = mover_world_vector(mv, t.normal); // impact normal back to world space
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

/// An impact this close to the chord end is the exact-endpoint contact rule in
/// `recurse` (`frac = p2f - 1`) rather than real obstruction: an authored goal
/// that sits flush against a brush lands exactly on that brush's expanded hull
/// plane. `SV_RecursiveHullCheck` puts a point exactly on a plane on the front
/// side and reports the move valid, and `CheckLocalMove` never even tests the
/// final partial step. Player movement keeps the stricter rule -- it is there
/// so integer error diffusion cannot skip a clip -- but a walker deciding
/// whether it can reach its mark must not read "touching the goal" as blocked.
/// c1a0b's retinal-scan scientist is the campaign case: his script mark is one
/// unit off the retinal button's hull, so he rejected the direct chord, failed
/// triangulation, livelocked on the node graph, and only reached the scanner
/// when the move timeout snapped him there -- eight seconds late, long after
/// the route had walked the player past the still-closed scanner door.
const NAV_CHORD_REACHED: i32 = 4095;

/// Fraction of the first world/brush impact on a scripted human-hull chord,
/// or `None` when it reaches the end. This is the allocation-free, cold-path
/// input to FTriangulate and avoids dozens of 16-unit floor probes on PS1.
#[inline(never)]
pub fn human_hull_blocked_fraction_movers(
    map: &Map,
    movers: &[Mover],
    from: [i32; 3],
    to: [i32; 3],
) -> Option<i32> {
    let tr = trace_all(map, map.hull1_head, movers, from, to);
    if tr.startsolid {
        Some(0)
    } else if tr.frac >= NAV_CHORD_REACHED {
        None
    } else {
        Some(tr.frac)
    }
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
        crate::ground_logic::clip_slide_component(v[0], n[0], proj),
        crate::ground_logic::clip_slide_component(v[1], n[1], proj),
        crate::ground_logic::clip_slide_component(v[2], n[2], proj),
    ]
}

fn clear_at(map: &Map, head: i32, movers: &[Mover], pos: [i32; 3]) -> bool {
    !trace_all(map, head, movers, pos, pos).startsolid
}

/// Validate a point against one mover's expanded player clip hull.
///
/// `trace_all` deliberately hides mover startsolid so an actor or brush that
/// overlaps the player cannot freeze them forever. A sweep that began clear
/// and just hit a mover is narrower: its rounded contact must remain on that
/// mover's clear side, or the next tangent bump would start inside the hull
/// and be allowed to pass through it. The tram rider path also uses this after
/// deliberate movement so a penetrated end wall is restored to the last safe
/// car-local seat instead of being mistaken for an open-side walk-off.
#[inline]
pub fn mover_clear_at(map: &Map, movers: &[Mover], mover_id: i32, pos: [i32; 3]) -> bool {
    for mv in movers {
        if mv.id != mover_id || mv.head <= 0 {
            continue;
        }
        let local = mover_local(mv, pos);
        return !trace(map, mv.head, local, local).startsolid;
    }
    true
}

#[inline]
fn impact_contact_clear_at(
    map: &Map,
    head: i32,
    movers: &[Mover],
    mover_id: i32,
    pos: [i32; 3],
) -> bool {
    clear_at(map, head, movers, pos)
        && (mover_id == -1 || mover_clear_at(map, movers, mover_id, pos))
}

#[inline(never)]
#[optimize(size)]
fn try_unstick(
    map: &Map,
    head: i32,
    movers: &[Mover],
    pos: [i32; 3],
    mover_id: i32,
) -> Option<[i32; 3]> {
    // World recovery passes -1 and retains its usual clearance test. A carried
    // player also checks the supporting mover, whose startsolid flag ordinary
    // movement traces intentionally ignore.
    if impact_contact_clear_at(map, head, movers, mover_id, pos) {
        return Some(pos);
    }
    let mut i = 0;
    while i < crate::ground_logic::CONTACT_RECOVERY_OFFSETS.len() {
        let p = add(pos, crate::ground_logic::CONTACT_RECOVERY_OFFSETS[i]);
        if impact_contact_clear_at(map, head, movers, mover_id, p) {
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
    // This is the complete integer displacement still available on each
    // axis. GoldSrc partitions one float displacement at impacts; rescaling
    // each integer partition independently makes negative tangents travel an
    // extra unit at every split (`-5 * 1/4096 >> 12 == -1`). Carry the exact
    // unused integer amount while clipping leaves an axis unchanged.
    let mut remaining_delta = vel;
    let mut last_zero_progress_plane: Option<[i32; 3]> = None;
    for _bump in 0..4 {
        if vel == [0, 0, 0] || time_left <= 0 {
            break;
        }
        let d = remaining_delta;
        if d == [0, 0, 0] {
            break;
        }
        let end = add(pos, d);
        let tr = trace_all(map, head, movers, pos, end);
        let mut advanced = [0i32; 3];
        #[cfg(feature = "deep-reference-trace")]
        crate::reference_trace::player_slide(
            unsafe { PLAYER_STEP_TICK },
            trace_call,
            _bump,
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
            match try_unstick(map, head, movers, pos, -1) {
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
            if tr.frac < 4096 {
                let diagonal = crate::ground_logic::is_diagonal_contact_plane(tr.normal);
                let rounded_into_mover =
                    tr.mover != -1 && !mover_clear_at(map, movers, tr.mover, floor);
                let rounded_into_diagonal_world = diagonal && !clear_at(map, head, movers, floor);
                if rounded_into_mover || rounded_into_diagonal_world {
                    if diagonal {
                        // On a diagonal plane, independent negative-component
                        // floors can turn the trace's clear backed-off fraction
                        // into an integer startsolid point. Reconstruct the
                        // impact at Q6 and quantize each axis toward the
                        // known-clear start. This keeps sub-unit precision
                        // without scaling the BSP/world, and fixes the recorded
                        // Hazard Course slope contact at -21.609375.
                        let end = add(impact_start, d);
                        let q6_contact = [
                            crate::ground_logic::slide_contact_axis_q6(
                                impact_start[0],
                                end[0],
                                tr.frac,
                            )
                            .0,
                            crate::ground_logic::slide_contact_axis_q6(
                                impact_start[1],
                                end[1],
                                tr.frac,
                            )
                            .0,
                            crate::ground_logic::slide_contact_axis_q6(
                                impact_start[2],
                                end[2],
                                tr.frac,
                            )
                            .0,
                        ];
                        // Never commit a contact point that the BSP or the
                        // impacting mover immediately rejects. `impact_start`
                        // came from the same successful trace and is the safe
                        // no-progress fallback if integer axes still combine
                        // unfavourably around a bevel.
                        pos = if impact_contact_clear_at(map, head, movers, tr.mover, q6_contact) {
                            q6_contact
                        } else {
                            impact_start
                        };
                    } else {
                        // Axial contacts have no fractional cross-axis point to
                        // recover. Stay at the known-clear sweep start instead
                        // of beginning the tangent bump inside the mover.
                        pos = impact_start;
                    }
                }
            }
            if tr.frac < 4096 {
                advanced = [
                    pos[0] - impact_start[0],
                    pos[1] - impact_start[1],
                    pos[2] - impact_start[2],
                ];
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

        let velocity_before_clip = vel;
        if found {
            vel = new_vel;
        } else if nplanes == 2 {
            let dir = cross12(planes[0], planes[1]);
            vel = scale12(dir, dot12_i32(vel, dir));
        } else {
            vel = [0, 0, 0];
            break;
        }

        remaining_delta = [
            crate::ground_logic::remaining_slide_axis(
                d[0],
                advanced[0],
                velocity_before_clip[0],
                vel[0],
                time_left,
            ),
            crate::ground_logic::remaining_slide_axis(
                d[1],
                advanced[1],
                velocity_before_clip[1],
                vel[1],
                time_left,
            ),
            crate::ground_logic::remaining_slide_axis(
                d[2],
                advanced[2],
                velocity_before_clip[2],
                vel[2],
                time_left,
            ),
        ];

        // Per-axis flooring can reverse the side of a nearly tangent vector.
        // Retry nearest rounding for the proven narrow cases, but accept it
        // only when the complete candidate clears every accumulated plane.
        let mut remainder_reenters = false;
        let mut plane_i = 0;
        while plane_i < nplanes {
            let into = dot_raw(remaining_delta, planes[plane_i]);
            if crate::ground_logic::slide_remainder_needs_nearest(
                tr.frac,
                advanced,
                planes[plane_i],
                into,
            ) {
                remainder_reenters = true;
            }
            plane_i += 1;
        }
        if remainder_reenters {
            // Keep the extra three multiplies and second plane scan off the
            // ordinary impact path; this branch is only taken for the narrow
            // sub-unit re-entry case above.
            let nearest_delta = [
                crate::ground_logic::remaining_slide_axis_nearest(
                    d[0],
                    advanced[0],
                    velocity_before_clip[0],
                    vel[0],
                    time_left,
                ),
                crate::ground_logic::remaining_slide_axis_nearest(
                    d[1],
                    advanced[1],
                    velocity_before_clip[1],
                    vel[1],
                    time_left,
                ),
                crate::ground_logic::remaining_slide_axis_nearest(
                    d[2],
                    advanced[2],
                    velocity_before_clip[2],
                    vel[2],
                    time_left,
                ),
            ];
            let mut nearest_clears = true;
            plane_i = 0;
            while plane_i < nplanes {
                if dot_raw(nearest_delta, planes[plane_i]) < 0 {
                    nearest_clears = false;
                    break;
                }
                plane_i += 1;
            }
            if nearest_clears {
                remaining_delta = nearest_delta;
            }
        }

        // A Q12 impact can be real while every integer contact component
        // remains at the clear start. GoldSrc keeps the fractional backed-off
        // contact and separates naturally on the next bump; repeating our
        // integer point instead traps the player while gravity accumulates.
        // Represent that sub-unit separation with one horizontal lattice step
        // toward the clear side, but only after the ordinary clipped retry also
        // made no progress. Trace the nudge so it cannot cross another hull.
        let repeated_zero_progress =
            advanced == [0, 0, 0] && tr.frac > 0 && last_zero_progress_plane == Some(tr.normal);
        if repeated_zero_progress && crate::ground_logic::is_diagonal_contact_plane(tr.normal) {
            let nudge = crate::ground_logic::horizontal_contact_nudge(tr.normal);
            if nudge != [0, 0, 0] {
                let nudge_end = add(pos, nudge);
                let nudge_trace = trace_all(map, head, movers, pos, nudge_end);
                if !nudge_trace.startsolid
                    && !nudge_trace.allsolid
                    && nudge_trace.frac >= 4096
                    && clear_at(map, head, movers, nudge_end)
                {
                    pos = nudge_end;
                }
            }
        }
        last_zero_progress_plane = if advanced == [0, 0, 0] && tr.frac > 0 {
            Some(tr.normal)
        } else {
            None
        };

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
    /// Keep a rotated rider outside both the lift and the surrounding world.
    /// The usual mover trace ignores startsolid, so repair quantization before
    /// the movement/ground traces can lose their supporting floor.
    #[inline(never)]
    #[optimize(size)]
    pub fn set_carried_position(
        &mut self,
        map: &Map,
        movers: &[Mover],
        mover_id: i32,
        pos: [i32; 3],
    ) {
        self.pos = if mover_clear_at(map, movers, mover_id, pos) {
            pos
        } else {
            try_unstick(map, self.head(map), movers, pos, mover_id).unwrap_or(pos)
        };
    }

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
    pub fn half_height(&self) -> i32 {
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
    /// slides along it. GoldSrc's `PM_LadderMove` checks the current
    /// `IN_JUMP` button directly, so a held jump detaches on contact; the
    /// old-button edge rule applies to ordinary ground jumping only.
    pub fn update_climb(
        &mut self,
        map: &Map,
        movers: &[Mover],
        fwd: i32,
        strafe: i32,
        jump_held: bool,
        yaw: u16,
        pitch: i16,
        ladder_center: [i32; 3],
        ladder_half: [i32; 3],
    ) {
        let s = sincos::sin_q12(yaw);
        let c = sincos::sin_q12((yaw + 1024) & 0xFFF);
        let (normal_axis, normal_sign) =
            crate::ladder_logic::cardinal_normal(self.pos, ladder_center, ladder_half);
        if jump_held {
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
        let fine = crate::ladder_logic::assisted_velocity_q6(
            forward,
            right,
            fwd,
            strafe,
            speed_q6,
            normal_axis,
            normal_sign,
            self.on_ground,
        );
        let fine = crate::ladder_logic::lock_small_tangent_q6(fine, normal_axis, strafe, speed_q6);
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
            if crate::ladder_logic::retry_improves_vertical(start[1], p[1], vertical_p[1], move_y) {
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
        // A clipped axis keeps its sub-unit origin carry: GoldSrc's float
        // origin never loses fractional progress at an impact, only the
        // blocked whole-unit motion.  Zeroing the carry here restarted the
        // error-diffusion stream whenever the climb brushed the frame, which
        // stuttered the whole-unit cadence tick to tick.
        self.move_frac_xz[0] = next_move_frac_x;
        self.move_frac_y = next_move_frac_y;
        self.move_frac_xz[1] = next_move_frac_z;
        if v[0] == move_x {
            self.vel[0] = physical_vel[0];
        } else {
            self.vel_frac_xz[0] = 0;
        }
        if v[1] == move_y {
            self.vel[1] = physical_vel[1];
        } else {
            self.vel_frac_y = 0;
        }
        if v[2] == move_z {
            self.vel[2] = physical_vel[2];
        } else {
            self.vel_frac_xz[1] = 0;
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
        water_level: u8,
        water_jump: &mut WaterJumpState,
        fwd: i32,
        strafe: i32,
        jump: bool,
        yaw: u16,
        pitch: i16,
    ) {
        let s = sincos::sin_q12(yaw);
        let c = sincos::sin_q12((yaw + 1024) & 0xFFF);
        let pitch_angle = (pitch as i32 & 0x0fff) as u16;
        let sp = sincos::sin_q12(pitch_angle);
        let cp = sincos::sin_q12((pitch_angle + 1024) & 0x0fff);
        let forward = [(s * cp) >> 12, sp, (c * cp) >> 12];
        let right = [c, 0, -s];

        // PM_PlayerMove handles an active water jump before ordinary water,
        // air or ground movement. PM_WaterJump reasserts the wall-normal
        // movedir every frame, so clipping one frame cannot erase the push
        // which carries the player over the lip.
        if water_jump.active() {
            water_jump.ticks -= 1;
            if water_level == 0 {
                water_jump.clear();
            }

            // PM_AddCorrectGravity and PM_FixupGravityVelocity both return
            // while `waterjumptime` is active. Preserve the launch velocity
            // verbatim; only collision may clip it.
            let fine_y = self.vertical_velocity_q6();

            let fine_x = water_jump.dir_x_q6 as i32;
            let fine_z = water_jump.dir_z_q6 as i32;
            let (move_x, next_move_frac_x) =
                crate::ground_logic::integrate_planar_q6(fine_x, self.move_frac_xz[0]);
            let (move_y, next_move_frac_y) =
                crate::ground_logic::integrate_planar_q6(fine_y, self.move_frac_y);
            let (move_z, next_move_frac_z) =
                crate::ground_logic::integrate_planar_q6(fine_z, self.move_frac_xz[1]);
            let (p, clipped) = slide_move(
                map,
                self.head(map),
                movers,
                self.pos,
                [move_x, move_y, move_z],
            );
            self.pos = p;

            // `movedir` survives PM_FlyMove's clipped result and is copied
            // back into velocity on the next frame. Retain it explicitly.
            (self.vel[0], self.vel_frac_xz[0]) = split_planar_round(fine_x);
            (self.vel[2], self.vel_frac_xz[1]) = split_planar_round(fine_z);
            self.move_frac_xz[0] = if clipped[0] == move_x {
                next_move_frac_x
            } else {
                0
            };
            self.move_frac_xz[1] = if clipped[2] == move_z {
                next_move_frac_z
            } else {
                0
            };
            if clipped[1] == move_y {
                (self.vel[1], self.vel_frac_y) = split_planar_round(fine_y);
                self.move_frac_y = next_move_frac_y;
            } else {
                self.vel[1] = clipped[1];
                self.vel_frac_y = 0;
                self.move_frac_y = 0;
            }
            self.on_ground = false;
            self.ground_mover = -1;
            return;
        }

        // PM_WaterMove keeps velocity from the air/previous swim tick, applies
        // five-percent water friction, then accelerates halfway toward the
        // requested 0.8*maxspeed each 50 ms tick. The old port replaced all
        // three axes outright, so entering a pool had no momentum and neutral
        // input teleported the velocity to a fixed sink rate.
        let mut fine = [
            self.vel[0] * PLANAR_FRAC_ONE + self.vel_frac_xz[0] as i32,
            self.vertical_velocity_q6(),
            self.vel[2] * PLANAR_FRAC_ONE + self.vel_frac_xz[1] as i32,
        ];
        let mut started_water_jump = false;

        // PM_CheckWaterJump runs automatically at waist depth (waterlevel 2);
        // the jump button is not part of the test. Both probes are point-hull
        // traces: one eight units above the origin must find a near-vertical
        // wall, while a second at the active player hull's top must be clear.
        // The previous full-hull, one-probe shortcut rejected valid authored
        // lips and then replaced the 225 u/s launch with a 100 u/s paddle on
        // the very next held-jump frame.
        if water_level == 2 && fine[1] >= WATERJUMP_MIN_FALL_Q6 {
            let planar_dot = fine[0] * s + fine[2] * c;
            if (fine[0] == 0 && fine[2] == 0) || planar_dot >= 0 {
                let low_start = [self.pos[0], self.pos[1] + WATERJUMP_LOW_HEIGHT, self.pos[2]];
                let low_end = [
                    low_start[0] + ((s * WATERJUMP_PROBE) >> 12),
                    low_start[1],
                    low_start[2] + ((c * WATERJUMP_PROBE) >> 12),
                ];
                let wall = trace_point_all(map, movers, low_start, low_end);
                if wall.frac < 4096 && (wall.normal[1] as i32).abs() < WATERJUMP_MAX_WALL_NY {
                    let high_start = [self.pos[0], self.pos[1] + self.half_height(), self.pos[2]];
                    let high_end = [
                        high_start[0] + ((s * WATERJUMP_PROBE) >> 12),
                        high_start[1],
                        high_start[2] + ((c * WATERJUMP_PROBE) >> 12),
                    ];
                    let clearance = trace_point_all(map, movers, high_start, high_end);
                    if !clearance.startsolid && !clearance.allsolid && clearance.frac == 4096 {
                        water_jump.ticks = WATERJUMP_TICKS;
                        water_jump.dir_x_q6 =
                            ((-(wall.normal[0] as i32) * WATERJUMP_PUSH_Q6) >> 12) as i16;
                        water_jump.dir_z_q6 =
                            ((-(wall.normal[2] as i32) * WATERJUMP_PUSH_Q6) >> 12) as i16;
                        fine[1] = WATERJUMP_UP_Q6;
                        started_water_jump = true;
                    }
                }
            }
        }

        // PM_Jump runs before PM_WaterMove. In water it first sets 100 u/s
        // upward, then water friction and wish-direction acceleration operate
        // on that velocity. Applying the paddle after acceleration erased the
        // extra vertical thrust from looking up + moving forward, which is
        // exactly how Hazard Course's second overhead exit is authored.
        if jump && !started_water_jump {
            fine[1] = SWIM_PADDLE * PLANAR_FRAC_ONE;
        }

        let speed = isqrt_i32(
            fine[0]
                .saturating_mul(fine[0])
                .saturating_add(fine[1].saturating_mul(fine[1]))
                .saturating_add(fine[2].saturating_mul(fine[2])),
        );
        if speed > 0 {
            let new_speed = (speed - speed / 20).max(0);
            fine[0] = fine[0] * new_speed / speed;
            fine[1] = fine[1] * new_speed / speed;
            fine[2] = fine[2] * new_speed / speed;
        }

        let input_mag = isqrt_i32(fwd * fwd + strafe * strafe).min(128);
        let (wish_dir, wish_speed_q6) = if input_mag == 0 {
            ([0, -4096, 0], SWIM_SINK * PLANAR_FRAC_ONE)
        } else {
            let wish = [
                (forward[0] * fwd + right[0] * strafe) / 128,
                (forward[1] * fwd) / 128,
                (forward[2] * fwd + right[2] * strafe) / 128,
            ];
            let mag = isqrt_i32(
                wish[0]
                    .saturating_mul(wish[0])
                    .saturating_add(wish[1].saturating_mul(wish[1]))
                    .saturating_add(wish[2].saturating_mul(wish[2])),
            )
            .max(1);
            (
                [
                    wish[0] * 4096 / mag,
                    wish[1] * 4096 / mag,
                    wish[2] * 4096 / mag,
                ],
                SWIM_SPEED * PLANAR_FRAC_ONE * input_mag / 128,
            )
        };
        let current = (fine[0] * wish_dir[0] + fine[1] * wish_dir[1] + fine[2] * wish_dir[2]) >> 12;
        let add_speed = wish_speed_q6 - current;
        if add_speed > 0 {
            let accel = add_speed.min((wish_speed_q6 / 2).max(1));
            fine[0] += (wish_dir[0] * accel) >> 12;
            fine[1] += (wish_dir[1] * accel) >> 12;
            fine[2] += (wish_dir[2] * accel) >> 12;
        }
        let physical_vel = [
            split_planar_round(fine[0]).0,
            split_planar_round(fine[1]).0,
            split_planar_round(fine[2]).0,
        ];
        let (move_x, next_move_frac_x) =
            crate::ground_logic::integrate_planar_q6(fine[0], self.move_frac_xz[0]);
        let (move_y, next_move_frac_y) =
            crate::ground_logic::integrate_planar_q6(fine[1], self.move_frac_y);
        let (move_z, next_move_frac_z) =
            crate::ground_logic::integrate_planar_q6(fine[2], self.move_frac_xz[1]);
        let move_vel = [move_x, move_y, move_z];
        let head = self.head(map);
        // PM_WaterMove first tries the intended destination from one
        // `stepsize + 1` above. This is the original cheap slope/step exit
        // path and is separate from the two-probe water-jump state.
        let dest = add(self.pos, move_vel);
        let raised = [dest[0], dest[1] + STEP_UP + 1, dest[2]];
        let down = trace_all(map, head, movers, raised, dest);
        let (p, v) = if !down.startsolid && !down.allsolid {
            (
                [
                    raised[0] + (((dest[0] - raised[0]) * down.frac) >> 12),
                    raised[1] + (((dest[1] - raised[1]) * down.frac) >> 12),
                    raised[2] + (((dest[2] - raised[2]) * down.frac) >> 12),
                ],
                move_vel,
            )
        } else {
            slide_move(map, head, movers, self.pos, move_vel)
        };
        self.pos = p;
        self.vel = v;
        if v[0] == move_x {
            self.vel[0] = physical_vel[0];
            self.vel_frac_xz[0] = split_planar_round(fine[0]).1;
            self.move_frac_xz[0] = next_move_frac_x;
        } else {
            self.vel_frac_xz[0] = 0;
            self.move_frac_xz[0] = 0;
        }
        if v[1] == move_y {
            self.vel[1] = physical_vel[1];
            self.vel_frac_y = split_planar_round(fine[1]).1;
            self.move_frac_y = next_move_frac_y;
        } else {
            self.vel_frac_y = 0;
            self.move_frac_y = 0;
        }
        if v[2] == move_z {
            self.vel[2] = physical_vel[2];
            self.vel_frac_xz[1] = split_planar_round(fine[2]).1;
            self.move_frac_xz[1] = next_move_frac_z;
        } else {
            self.vel_frac_xz[1] = 0;
            self.move_frac_xz[1] = 0;
        }
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
        // A chosen PM_WalkMove step already performed the authoritative
        // downward floor trace. Reuse that contact below instead of tracing
        // again from its integer-ceiled endpoint, which can be classified as
        // startsolid at an exact stair corner.
        let mut step_ground: Option<(i16, i8)> = None;

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
            // Keep the same Q6 contact precision as the post-move ground
            // probe. A 4095/4096 stair descent is 17.9956 units: truncating it
            // to 17 leaves the integer hull one unit above the floor, where a
            // following diagonal move can report startsolid and freeze. Q6
            // correctly rounds that contact to the 18-unit floor while still
            // keeping fractional slope contacts on their clear/upward side.
            let (step_y, step_residue) =
                crate::ground_logic::ground_contact_q6(sp[1], dn_end[1], tdn.frac);
            let step_pos = [sp[0], step_y, sp[2]];
            let landed =
                !tdn.startsolid && !tdn.allsolid && tdn.frac < 4096 && tdn.normal[1] > GROUND_NY;
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
                step_ground = Some((tdn.mover as i16, step_residue));
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

        if let Some((ground_mover, residue_q6)) = step_ground {
            // PM_WalkMove's down trace already proved this floor and supplied
            // the fractional GoldSrc contact. Besides being more faithful at
            // exact corners, reusing it removes a BSP trace on every accepted
            // stair step from the PS1 hot path.
            self.on_ground = true;
            self.ground_mover = ground_mover;
            self.move_frac_y = residue_q6;
            self.set_vertical_velocity_q6(0);
        } else {
            // Ground check: probe straight down a little.
            let down = [self.pos[0], self.pos[1] - GROUND_PROBE_DOWN, self.pos[2]];
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
}
