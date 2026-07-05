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

// Tunables (world units per frame). HL feel-ish; adjust from captures.
const GRAVITY: i32 = 12;

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
const MOVE_SPEED: i32 = 18;
const JUMP: i32 = 64;
// Horizontal velocity ramps toward the target (accelerate) / toward 0 when idle
// (friction) instead of snapping -- HL's PM_Accelerate/PM_Friction feel. Fraction
// of the gap closed per sim tick, out of 16. Air is low so a jump keeps momentum.
const GROUND_ACCEL: i32 = 8; // ~half the gap/tick: reaches speed in ~4 ticks, stops in ~4
const AIR_ACCEL: i32 = 2; // gentle mid-air nudge; preserves jump momentum
const STEP_DOWN: i32 = 8; // ground probe depth
const CONTACT_NUDGE: i32 = 2;
const STOP_EPSILON: i32 = 1;
const MAX_CLIP_PLANES: usize = 5;

#[inline]
fn dot(n: [i16; 3], p: [i32; 3]) -> i32 {
    ((n[0] as i32 * p[0]) + (n[1] as i32 * p[1]) + (n[2] as i32 * p[2])) >> 12
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
        let t = dot(cn.n, p) - cn.dist;
        num = if t >= 0 { cn.c0 } else { cn.c1 };
    }
    num
}

fn recurse(
    map: &Map,
    num: i16,
    p1f: i32,
    p2f: i32,
    p1: [i32; 3],
    p2: [i32; 3],
    tr: &mut Trace,
    depth: u8,
) -> bool {
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
    let t1 = dot(cn.n, p1) - cn.dist;
    let t2 = dot(cn.n, p2) - cn.dist;
    if t1 >= 0 && t2 >= 0 {
        return recurse(map, cn.c0, p1f, p2f, p1, p2, tr, depth + 1);
    }
    if t1 < 0 && t2 < 0 {
        return recurse(map, cn.c1, p1f, p2f, p1, p2, tr, depth + 1);
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
    tr.normal = if side {
        [-(cn.n[0] as i32), -(cn.n[1] as i32), -(cn.n[2] as i32)]
    } else {
        [cn.n[0] as i32, cn.n[1] as i32, cn.n[2] as i32]
    };
    tr.frac = midf;
    false
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

/// A moving/brush collider: a submodel clip hull at a world offset.
#[derive(Clone, Copy)]
pub struct Mover {
    pub head: i32,
    pub head0: i32, // point-hull root (hitscans; 0 = fall back to head)
    pub off: [i32; 3],
    pub center: [i32; 3],
    pub radius: i32,
    pub id: i32, // owning brush-entity index (traces report it on hit)
}

const SWIM_SPEED: i32 = 6;
const SWIM_SINK: i32 = 1;

pub const NO_MOVER: Mover = Mover {
    head: 0,
    head0: 0,
    off: [0, 0, 0],
    center: [0, 0, 0],
    radius: 0,
    id: -1,
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
        let o = mv.off;
        let q1 = [p1[0] - o[0], p1[1] - o[1], p1[2] - o[2]];
        let q2 = [p2[0] - o[0], p2[1] - o[1], p2[2] - o[2]];
        let t = trace(map, head, q1, q2);
        if !t.startsolid && t.frac < 4096 {
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
    let t = trace(map, map.hull0_head, p1, p2);
    t.startsolid || t.frac >= 4096
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
        let o = mv.off;
        let q1 = [p1[0] - o[0], p1[1] - o[1], p1[2] - o[2]];
        let q2 = [p2[0] - o[0], p2[1] - o[1], p2[2] - o[2]];
        let t = trace(map, head, q1, q2);
        // NB: do NOT propagate a mover's startsolid. If the player ends up inside
        // a brush-entity hull (a non-solid func_illusionary, or slight
        // penetration), startsolid would make slide_move break and freeze them
        // forever. Movers still block ENTRY via frac; only the world hull's
        // startsolid counts as truly stuck.
        if t.frac < best.frac {
            best.frac = t.frac;
            best.normal = t.normal;
            best.mover = mv.id;
        }
    }
    best
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
}

impl Player {
    pub fn new(pos: [i32; 3]) -> Player {
        Player {
            pos,
            vel: [0, 0, 0],
            on_ground: false,
            ground_mover: -1,
            land_impact: 0,
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
            // Let go: push back off the ladder and resume normal physics.
            self.vel = [(-s * CLIMB_SPEED) >> 12, 0, (-c * CLIMB_SPEED) >> 12];
            self.on_ground = false;
            let head = map.hull1_head;
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
        let head = map.hull1_head;
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
            self.vel[1] = SWIM_SPEED; // paddle up (surfacing)
        } else if fwd == 0 && strafe == 0 {
            self.vel[1] -= SWIM_SINK; // idle: sink gently
        }
        let head = map.hull1_head;
        let (p, v) = slide_move(map, head, movers, self.pos, self.vel);
        self.pos = p;
        self.vel = v;
        self.on_ground = false;
        self.ground_mover = -1;
    }

    /// Advance the player one frame. `fwd`/`strafe` are analog deltas in
    /// `-128..=127` (D-pad sends ±127) relative to `yaw` (Q0.12); `jump`
    /// triggers when grounded.
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
        let wmag = ax.max(az) + ax.min(az) * 3 / 8;
        if wmag > MOVE_SPEED {
            wish_x = wish_x * MOVE_SPEED / wmag;
            wish_z = wish_z * MOVE_SPEED / wmag;
        }
        // Accelerate toward the wish velocity / decelerate toward it when idle.
        let accel = if self.on_ground { GROUND_ACCEL } else { AIR_ACCEL };
        self.vel[0] += ((wish_x - self.vel[0]) * accel) >> 4;
        self.vel[2] += ((wish_z - self.vel[2]) * accel) >> 4;
        // The >>4 decel is an arithmetic shift: a small NEGATIVE residual floors
        // at -1 and never reaches 0, so a released player would drift forever
        // (self-movement with no input). Snap tiny idle velocity to a dead stop.
        if wish_x == 0 && self.vel[0].abs() <= 2 {
            self.vel[0] = 0;
        }
        if wish_z == 0 && self.vel[2].abs() <= 2 {
            self.vel[2] = 0;
        }

        self.land_impact = 0;
        let was_air = !self.on_ground;

        if self.on_ground {
            if self.vel[1] < 0 {
                self.vel[1] = 0;
            }
            if jump {
                self.vel[1] = JUMP;
                unsafe {
                    if LONGJUMP && (self.vel[0].abs() + self.vel[2].abs()) > MOVE_SPEED {
                        // Long jump: launch along the move direction.
                        self.vel[0] = self.vel[0] * 5 / 2;
                        self.vel[2] = self.vel[2] * 5 / 2;
                    }
                }
                self.on_ground = false;
            }
        } else {
            self.vel[1] -= gravity_step();
        }

        // Move with stair-stepping: a plain slide, then (when grounded and
        // moving) an up/forward/down "step" -- keep whichever advanced further
        // along the ground, so the player climbs stairs/thresholds <= STEP_UP.
        let head = map.hull1_head;
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
