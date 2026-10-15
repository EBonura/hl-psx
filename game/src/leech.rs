//! monster_leech: the little swimmer of c2a1a, c2a2a, c2a3, c3a1.
//!
//! The retail leech is a point-sized swimmer with two moods. Idle, it drifts
//! at 50 units per second inside a band of the water column and wanders its
//! heading now and then. Hunting, it speeds to 100 units per second whenever
//! it faces the prey, chases the prey's eye height, and bites from 40 units
//! away. It thinks ten times a second and only while the player can see its
//! room. Everything here is arithmetic over a [`World`], so the host test
//! crate can run it; `main.rs` supplies the traces and the water probe.
//!
//! Timers count 20 Hz simulation ticks and the think runs every second tick.

use psx_math::sincos;

/// Concurrent swimmers. Leeches only think while the player can see their
/// room, so a map's twenty rarely need more than a handful at once; the rest
/// hold their pose until a slot frees. See CHANGELOG (RAM).
pub const SLOTS: usize = 8;
/// A think is 0.1 s: every second simulation tick.
pub const THINK_TICKS: u32 = 2;
/// Sight range of `Look`.
pub const LOOK_RANGE: i32 = 750;
/// Bite reach from the leech to the prey's eye.
pub const BITE_REACH: i32 = 40;

const TICKS_PER_SECOND: i32 = 20;
const SWIM_SPEED: i32 = 50;
/// Units per second gained each 0.1 s think (LEECH_SWIM_ACCEL 80 x 0.1).
const ACCEL_STEP: i32 = 8;
/// Retained as tenths of a unit: LEECH_SWIM_DECEL 10 x 0.1 x obstacle ratio.
const DECEL_STEP_TENTHS: i32 = 10;
const TURN_RATE: i32 = 90;
const CHECK_DIST: i32 = 45;
const SIZE_X: i32 = 10;
/// One full turn of a 12-bit heading.
const FULL: i32 = 4096;
/// The attack sequence is 31 frames at 150 fps: 0.2067 s, in 1/4096 s.
const ATTACK_LOOP_Q12: i32 = 847;
const THINK_Q12: i32 = 410;

/// What the leech needs to know about the world and the prey.
pub trait World {
    /// Free fraction (0..=4096, 4096 = clear) of a trace between two points.
    fn free_frac(&mut self, from: [i32; 3], to: [i32; 3]) -> i32;
    /// Whether the point lies in water.
    fn wet(&mut self, p: [i32; 3]) -> bool;
    /// Uniform integer in `lo..=hi`.
    fn rand(&mut self, lo: i32, hi: i32) -> i32;
    /// The best enemy this leech can see now (`Look(750)` then
    /// `BestVisibleEnemy`), as its eye position and whether it stands in water.
    fn look(&mut self) -> Option<Enemy>;
    /// The current prey, if it still exists.
    fn enemy(&mut self) -> Option<Enemy>;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Enemy {
    pub eye: [i32; 3],
    /// `waterlevel != 0` for the prey.
    pub wet: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Swim {
    /// Owning prop; `FREE` when the slot is unused.
    pub prop: u8,
    pub hunting: bool,
    /// -1, 0 or +1 times the turn rate while an obstacle is being avoided.
    turning: i8,
    blocked: bool,
    /// `m_obstacle` in 1/16 units (the reciprocal of the clear fraction).
    obstacle16: u16,
    /// Units per second along the heading; vertical fall speed when dry.
    speed: i16,
    accel: i16,
    /// Degrees of dive (positive swims down).
    pitch: i8,
    height: i16,
    top: i16,
    bottom: i16,
    state_left: i16,
    water_left: i16,
    z_left: i16,
    side_left: i16,
    old: [i16; 3],
    biting: bool,
    attack_phase: i16,
    dry: bool,
    /// Velocity held between thinks (units per second) and the sub-tick
    /// remainders of moving and turning with it.
    vel: [i16; 3],
    rem: [i8; 3],
    yaw_rate: i16,
    yaw_rem: i16,
    fall: i16,
}

pub const FREE: u8 = 0xFF;

impl Swim {
    pub const EMPTY: Self = Self {
        prop: FREE,
        hunting: false,
        turning: 0,
        blocked: false,
        obstacle16: 16,
        speed: 0,
        accel: 0,
        pitch: 0,
        height: 0,
        top: 0,
        bottom: 0,
        state_left: 0,
        water_left: 0,
        z_left: 0,
        side_left: 0,
        old: [0; 3],
        biting: false,
        attack_phase: 0,
        dry: false,
        vel: [0; 3],
        rem: [0; 3],
        yaw_rate: 0,
        yaw_rem: 0,
        fall: 0,
    };

    /// CLeech::Spawn plus Activate: idle for one to five seconds, then look.
    pub fn spawn<W: World>(prop: u8, pos: [i32; 3], w: &mut W) -> Self {
        let mut s = Self::EMPTY;
        s.prop = prop;
        s.state_left = secs(w.rand(1, 5));
        s.old = clamp3(pos);
        s.recalculate_water(pos, w);
        s
    }

    /// RecalculateWaterlevel: the water column under and over the leech, its
    /// outer fifths trimmed off, and a random swimming height inside it.
    fn recalculate_water<W: World>(&mut self, pos: [i32; 3], w: &mut W) {
        let below = [pos[0], pos[1] - 400, pos[2]];
        let frac = w.free_frac(pos, below);
        // One unit above the floor that was hit, else the end of the probe.
        let bottom = if frac < 4096 {
            pos[1] - 400 * frac / 4096 + 1
        } else {
            below[1]
        };
        let top = water_surface(pos, w) - 1;
        let new_bottom = (bottom * 4 + top) / 5;
        let new_top = (bottom + top * 4) / 5;
        self.bottom = new_bottom as i16;
        self.top = new_top as i16;
        let (lo, hi) = (
            self.bottom.min(self.top) as i32,
            self.bottom.max(self.top) as i32,
        );
        self.height = w.rand(lo, hi) as i16;
        self.water_left = secs_range(w, 5, 7);
    }

    /// SwitchLeechState.
    fn switch_state<W: World>(&mut self, w: &mut W) {
        self.state_left = secs_range(w, 3, 6);
        if self.hunting {
            self.hunting = false;
            // We may be up against the prey: redo the side checks.
            self.side_left = 0;
        } else if let Some(e) = w.look() {
            if e.wet {
                self.hunting = true;
                self.state_left = secs_range(w, 18, 25);
            }
        }
    }

    /// One 0.1 s think. `yaw` is the 12-bit heading (0 = +Z, 1024 = +X).
    /// The result holds the velocity to integrate until the next think, the
    /// heading rate and the attack state for the animation.
    pub fn think<W: World>(&mut self, pos: [i32; 3], yaw: u16, wet: bool, w: &mut W) -> Motion {
        let dt = THINK_TICKS as i16;
        self.state_left -= dt;
        self.water_left -= dt;
        self.z_left -= dt;
        self.side_left -= dt;
        if !wet {
            return self.dry_motion();
        }
        if self.dry {
            // Back in the water: fly again and re-measure the column soon.
            self.dry = false;
            self.speed = 0;
            self.recalculate_water(pos, w);
            self.water_left = secs(2);
        }
        if self.water_left <= 0 {
            self.recalculate_water(pos, w);
        }
        if self.state_left <= 0 {
            self.switch_state(w);
        }

        let mut target_speed = SWIM_SPEED;
        let mut target_yaw = 0i32;
        let mut melee = false;
        let mut chasing = false;
        if self.hunting {
            match w.enemy() {
                None => self.switch_state(w),
                Some(e) => {
                    chasing = true;
                    self.height = ((e.eye[1] - 5).clamp(
                        self.bottom.min(self.top) as i32,
                        self.bottom.max(self.top) as i32,
                    )) as i16;
                    let to = [e.eye[0] - pos[0], e.eye[1] - pos[1], e.eye[2] - pos[2]];
                    if length(to) < BITE_REACH {
                        melee = true;
                    }
                    let diff = angle_diff_deg(heading_deg(to[0], to[2]), deg(yaw as i32));
                    let limit = TURN_RATE * 3 / 4;
                    if diff < -limit {
                        target_yaw = -limit;
                    } else if diff > limit {
                        target_yaw = limit;
                    } else {
                        target_yaw = diff;
                        target_speed *= 2;
                    }
                }
            }
        }
        if !self.hunting {
            if self.z_left <= 0 {
                let (lo, hi) = (
                    self.bottom.min(self.top) as i32,
                    self.bottom.max(self.top) as i32,
                );
                let fresh = w.rand(lo, hi);
                self.height = ((self.height as i32 + fresh) / 2) as i16;
                self.z_left = secs_range(w, 1, 4);
            }
            if w.rand(0, 100) < 10 {
                target_yaw = w.rand(-30, 30);
            }
            let moved = [
                pos[0] - self.old[0] as i32,
                pos[1] - self.old[1] as i32,
                pos[2] - self.old[2] as i32,
            ];
            if length(moved) < 1 {
                // Not moving: something blocks it, so retest the sides.
                self.side_left = 0;
            }
        }

        let forward = forward_vector(yaw, self.pitch as i32);
        let obstacle = self.obstacle_distance(pos, forward, yaw, chasing, w);
        self.old = clamp3(pos);
        let obstacle = obstacle.max(410); // 0.1 in 1/4096
        if obstacle >= 4096 {
            self.turning = 0;
            self.blocked = false;
            self.obstacle16 = 16;
            self.speed = approach(target_speed, self.speed as i32, ACCEL_STEP) as i16;
        } else {
            // m_obstacle = 1 / fraction, kept in 1/16.
            self.obstacle16 = (16 * 4096 / obstacle).min(u16::MAX as i32) as u16;
            self.blocked = true;
            if self.turning == 0 {
                let right = right_vector(yaw);
                let ahead = scale(forward_flat(yaw), CHECK_DIST);
                let probe = |side: i32| {
                    [
                        pos[0] + (right[0] * SIZE_X * side >> 12) + ahead[0],
                        pos[1],
                        pos[2] + (right[2] * SIZE_X * side >> 12) + ahead[2],
                    ]
                };
                let right_free = w.free_frac(pos, probe(1));
                let left_free = w.free_frac(pos, probe(-1));
                let delta = right_free - left_free;
                // 0.1 of a fraction is 410 of 4096.
                self.turning = if delta > 410 || (delta > -410 && w.rand(0, 100) < 50) {
                    -1
                } else {
                    1
                };
            }
            let step = DECEL_STEP_TENTHS * self.obstacle16 as i32 / 16;
            self.speed = approach_tenths(-(SWIM_SPEED / 2), self.speed as i32, step) as i16;
        }
        let ideal_yaw = self.turning as i32 * TURN_RATE + target_yaw;
        self.finish(pos, forward, ideal_yaw, melee && self.hunting, wet)
    }

    /// UpdateMotion: animation rate, heading rate, dive angle and bites.
    fn finish(
        &mut self,
        pos: [i32; 3],
        forward: [i32; 3],
        ideal_yaw: i32,
        melee: bool,
        wet: bool,
    ) -> Motion {
        let flap = (((self.speed as i32 - self.accel as i32).abs() * 16) / 10 + 16).clamp(8, 30);
        self.accel = ((self.accel as i32 * 4 + self.speed as i32) / 5) as i16;
        let rate = if self.blocked {
            ideal_yaw * self.obstacle16 as i32 / 16
        } else {
            ideal_yaw
        };
        // Lean toward the target height.
        let delta = self.height as i32 - pos[1];
        let target_pitch = if delta < -10 {
            30
        } else if delta > 10 {
            -30
        } else {
            0
        };
        self.pitch = approach(target_pitch, self.pitch as i32, 6) as i8;

        let mut bite = false;
        if melee {
            if !self.biting {
                self.biting = true;
                self.attack_phase = 0;
            } else {
                // Sequence time advances 0.1 s at the flap rate (16 = 1.0).
                let advance = THINK_Q12 * flap / 16;
                let phase = self.attack_phase as i32 + advance;
                if phase >= ATTACK_LOOP_Q12 {
                    bite = true;
                }
                self.attack_phase = (phase % ATTACK_LOOP_Q12) as i16;
            }
        } else {
            self.biting = false;
        }
        let _ = wet;
        let velocity = [
            forward[0] * self.speed as i32 >> 12,
            forward[1] * self.speed as i32 >> 12,
            forward[2] * self.speed as i32 >> 12,
        ];
        self.vel = [velocity[0] as i16, velocity[1] as i16, velocity[2] as i16];
        self.yaw_rate = rate.clamp(-i16::MAX as i32, i16::MAX as i32) as i16;
        Motion {
            velocity,
            yaw_rate_deg: rate,
            biting: melee,
            bite,
            dry: false,
        }
    }

    /// A leech out of the water drops, twitches and does nothing else.
    fn dry_motion(&mut self) -> Motion {
        if !self.dry {
            self.dry = true;
            self.speed = 0;
        }
        self.biting = false;
        self.pitch = 0;
        self.vel = [0; 3];
        self.yaw_rate = 0;
        Motion {
            velocity: [0; 3],
            yaw_rate_deg: 0,
            biting: false,
            bite: false,
            dry: true,
        }
    }

    /// What one 20 Hz tick moves it (units) and turns it (12-bit heading):
    /// the held velocity over 1/20 s with the remainders carried, or a fall
    /// at 800 units per second squared once out of the water.
    pub fn tick_displacement(&mut self) -> ([i32; 3], i32) {
        if self.dry {
            self.fall = (self.fall + 40).min(2000);
            self.vel = [0, -self.fall, 0];
        }
        let mut step = [0i32; 3];
        for axis in 0..3 {
            let total = self.rem[axis] as i32 + self.vel[axis] as i32;
            step[axis] = total / TICKS_PER_SECOND;
            self.rem[axis] = (total % TICKS_PER_SECOND) as i8;
        }
        // Degrees per second to 12-bit units per tick: x 4096 / 360 / 20.
        let turn_total = self.yaw_rem as i32 + self.yaw_rate as i32 * 512;
        let turn = turn_total / 900;
        self.yaw_rem = (turn_total % 900) as i16;
        (step, turn)
    }

    /// The last displacement ran into the floor or a wall: stop falling.
    pub fn land(&mut self) {
        self.fall = 0;
        self.rem = [0; 3];
    }

    /// A bite landed: `m_stateTime -= 2`.
    pub fn bit(&mut self) {
        self.state_left -= secs(2);
    }

    /// ObstacleDistance: the clear fraction (0..=4096) ahead and to both sides.
    fn obstacle_distance<W: World>(
        &mut self,
        pos: [i32; 3],
        forward: [i32; 3],
        yaw: u16,
        prey_ahead: bool,
        w: &mut W,
    ) -> i32 {
        let reach = |dir: [i32; 3], len: i32| {
            [
                pos[0] + (dir[0] * len >> 12),
                pos[1] + (dir[1] * len >> 12),
                pos[2] + (dir[2] * len >> 12),
            ]
        };
        let f = w.free_frac(pos, reach(forward, CHECK_DIST));
        if f < 4096 {
            // A wall in front only counts when it is not the prey, or when
            // the prey is not at swimming height yet (traces here never hit
            // the player, so "the prey" is the not-blocked case).
            if !prey_ahead || (self.height as i32 - pos[1]).abs() > 10 {
                return f;
            }
        }
        if self.side_left <= 0 {
            let right = right_vector(yaw);
            let flat = forward_flat(yaw);
            for side in [1i32, -1] {
                let p = [
                    pos[0] + (right[0] * SIZE_X * 2 * side >> 12) + (flat[0] * CHECK_DIST >> 12),
                    pos[1],
                    pos[2] + (right[2] * SIZE_X * 2 * side >> 12) + (flat[2] * CHECK_DIST >> 12),
                ];
                let f = w.free_frac(pos, p);
                if f < 4096 {
                    return f;
                }
            }
            // Clear on both sides: skip the side tests for 0.5 to 1 second.
            self.side_left = secs_range_tenths(w, 5, 10);
        }
        4096
    }
}

/// What a think asks of the world until the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// Units per second.
    pub velocity: [i32; 3],
    /// Degrees per second, positive turning toward +yaw.
    pub yaw_rate_deg: i32,
    /// Playing the attack sequence.
    pub biting: bool,
    /// The sequence's bite event fired on this think.
    pub bite: bool,
    /// Out of the water: falls and twitches.
    pub dry: bool,
}

/// Whether the prey is inside the front 240 degrees (`m_flFieldOfView -0.5`),
/// judged on the horizontal plane.
pub fn in_view(yaw: u16, dx: i32, dz: i32) -> bool {
    let f = forward_flat(yaw);
    let len = isqrt(dx * dx + dz * dz).max(1);
    // dot(f, d) / |d| > -0.5, f in Q12.
    (f[0] * dx + f[2] * dz) / len > -2048
}

/// Whether a bite connects: the prey is within 25 degrees of straight ahead
/// on the horizontal plane (dot > 0.9).
pub fn bite_connects(yaw: u16, dx: i32, dz: i32) -> bool {
    let f = forward_flat(yaw);
    let len = isqrt(dx * dx + dz * dz);
    if len == 0 {
        return false;
    }
    (f[0] * dx + f[2] * dz) / len > 3686
}

/// UTIL_WaterLevel from the leech's origin: the first non-water height in the
/// 400 units above, by bisection; the origin itself when it is already dry.
fn water_surface<W: World>(pos: [i32; 3], w: &mut W) -> i32 {
    if !w.wet(pos) {
        return pos[1];
    }
    let top = pos[1] + 400;
    if w.wet([pos[0], top, pos[2]]) {
        return top;
    }
    let (mut lo, mut hi) = (pos[1], top);
    for _ in 0..8 {
        let mid = (lo + hi) / 2;
        if w.wet([pos[0], mid, pos[2]]) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

fn secs(s: i32) -> i16 {
    (s * TICKS_PER_SECOND) as i16
}

fn secs_range<W: World>(w: &mut W, lo: i32, hi: i32) -> i16 {
    w.rand(lo * TICKS_PER_SECOND, hi * TICKS_PER_SECOND) as i16
}

/// A range in tenths of a second.
fn secs_range_tenths<W: World>(w: &mut W, lo: i32, hi: i32) -> i16 {
    (w.rand(lo, hi) * TICKS_PER_SECOND / 10) as i16
}

fn clamp3(p: [i32; 3]) -> [i16; 3] {
    [
        p[0].clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        p[1].clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        p[2].clamp(i16::MIN as i32, i16::MAX as i32) as i16,
    ]
}

fn approach(target: i32, value: i32, step: i32) -> i32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

/// `approach` with the step in tenths of a unit; speeds are whole units, so a
/// fractional step rounds up to one unit per think.
fn approach_tenths(target: i32, value: i32, step_tenths: i32) -> i32 {
    approach(target, value, ((step_tenths + 9) / 10).max(1))
}

fn scale(v: [i32; 3], by: i32) -> [i32; 3] {
    [v[0] * by >> 12, v[1] * by >> 12, v[2] * by >> 12]
}

fn length(v: [i32; 3]) -> i32 {
    isqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
}

fn isqrt(n: i32) -> i32 {
    let mut remainder = n.max(0) as u32;
    let mut root = 0u32;
    let mut bit = 1u32 << 30;
    while bit > remainder {
        bit >>= 2;
    }
    while bit != 0 {
        if remainder >= root + bit {
            remainder -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root as i32
}

fn deg(q12: i32) -> i32 {
    q12 * 360 / FULL
}

fn heading_deg(dx: i32, dz: i32) -> i32 {
    deg(psx_math::atan2_q12(dx, dz) as i32)
}

/// Shortest signed difference `target - current` in degrees, in (-180, 180].
fn angle_diff_deg(target: i32, current: i32) -> i32 {
    let d = (target - current).rem_euclid(360);
    if d > 180 {
        d - 360
    } else {
        d
    }
}

fn forward_flat(yaw: u16) -> [i32; 3] {
    [
        sincos::sin_q12(yaw),
        0,
        sincos::sin_q12((yaw.wrapping_add(1024)) & 0x0fff),
    ]
}

/// The heading's horizontal right, the side the heading turns toward as it
/// increases.
fn right_vector(yaw: u16) -> [i32; 3] {
    let f = forward_flat(yaw);
    [f[2], 0, -f[0]]
}

/// Forward vector in Q12 for a dive of `pitch_down` degrees.
fn forward_vector(yaw: u16, pitch_down: i32) -> [i32; 3] {
    let f = forward_flat(yaw);
    let p = (pitch_down * FULL / 360).rem_euclid(FULL) as u16;
    let sin = sincos::sin_q12(p);
    let cos = sincos::sin_q12((p + 1024) & 0x0fff);
    [f[0] * cos >> 12, -sin, f[2] * cos >> 12]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box of water 400 high over a floor at y = 0 with a surface at y = 400,
    /// walls at |x|,|z| = 500, and a scripted prey.
    struct Pool {
        prey: Option<Enemy>,
        sees: bool,
        roll: u32,
        floor: i32,
        surface: i32,
    }

    impl Pool {
        fn new() -> Self {
            Self {
                prey: None,
                sees: false,
                roll: 7,
                floor: 0,
                surface: 400,
            }
        }
    }

    impl World for Pool {
        fn free_frac(&mut self, from: [i32; 3], to: [i32; 3]) -> i32 {
            // Slab test against the six faces of the pool.
            let mut best = 4096i64;
            for axis in 0..3 {
                let (lo, hi) = match axis {
                    1 => (self.floor as i64, 1000i64),
                    _ => (-500, 500),
                };
                let (a, b) = (from[axis] as i64, to[axis] as i64);
                for wall in [lo, hi] {
                    if (a - wall) * (b - wall) < 0 || b == wall {
                        let t = (wall - a) * 4096 / (b - a);
                        if (0..best).contains(&t) {
                            best = t;
                        }
                    }
                }
            }
            best as i32
        }
        fn wet(&mut self, p: [i32; 3]) -> bool {
            p[1] >= self.floor && p[1] < self.surface
        }
        fn rand(&mut self, lo: i32, hi: i32) -> i32 {
            self.roll = self.roll.wrapping_mul(1664525).wrapping_add(1013904223);
            lo + ((self.roll >> 8) % ((hi - lo + 1) as u32)) as i32
        }
        fn look(&mut self) -> Option<Enemy> {
            if self.sees {
                self.prey
            } else {
                None
            }
        }
        fn enemy(&mut self) -> Option<Enemy> {
            self.prey
        }
    }

    fn run(
        world: &mut Pool,
        s: &mut Swim,
        pos: &mut [i32; 3],
        yaw: &mut u16,
        thinks: i32,
    ) -> Vec<Motion> {
        let mut out = Vec::new();
        for _ in 0..thinks {
            let wet = world.wet(*pos);
            let m = s.think(*pos, *yaw, wet, world);
            for axis in 0..3 {
                pos[axis] += m.velocity[axis] / 10;
            }
            let step = m.yaw_rate_deg * FULL / 360 / 10;
            *yaw = ((*yaw as i32 + step).rem_euclid(FULL)) as u16;
            out.push(m);
        }
        out
    }

    #[test]
    fn spawn_idles_one_to_five_seconds_then_looks() {
        let mut w = Pool::new();
        let s = Swim::spawn(3, [0, 200, 0], &mut w);
        assert!((20..=100).contains(&(s.state_left as i32)));
        assert!(!s.hunting);
        // The band is the water column with its outer fifths trimmed.
        assert!(
            s.bottom as i32 >= 70 && s.top as i32 <= 330,
            "{} {}",
            s.bottom,
            s.top
        );
    }

    #[test]
    fn idle_leech_cruises_at_fifty() {
        let mut w = Pool::new();
        let (mut pos, mut yaw) = ([0, 200, 0], 0u16);
        let mut s = Swim::spawn(0, pos, &mut w);
        s.state_left = 10_000;
        let motion = run(&mut w, &mut s, &mut pos, &mut yaw, 30);
        let last = motion.last().unwrap();
        let v = last.velocity;
        let speed = isqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
        assert!((45..=50).contains(&speed), "speed {speed}");
        assert!(w.wet(pos), "stays in the water: {pos:?}");
    }

    #[test]
    fn a_wet_visible_player_starts_the_hunt_a_dry_one_does_not() {
        let mut w = Pool::new();
        w.sees = true;
        w.prey = Some(Enemy {
            eye: [300, 250, 0],
            wet: false,
        });
        let (mut pos, mut yaw) = ([0, 200, 0], 1024u16);
        let mut s = Swim::spawn(0, pos, &mut w);
        s.state_left = 2;
        run(&mut w, &mut s, &mut pos, &mut yaw, 1);
        assert!(!s.hunting, "a player out of the water is ignored");
        w.prey = Some(Enemy {
            eye: [300, 250, 0],
            wet: true,
        });
        s.state_left = 2;
        run(&mut w, &mut s, &mut pos, &mut yaw, 1);
        assert!(s.hunting);
        assert!(
            (18 * 20..=25 * 20).contains(&(s.state_left as i32)),
            "{}",
            s.state_left
        );
    }

    #[test]
    fn hunting_doubles_speed_when_facing_and_closes_to_bite_range() {
        let mut w = Pool::new();
        w.prey = Some(Enemy {
            eye: [300, 250, 0],
            wet: true,
        });
        let (mut pos, mut yaw) = ([0, 245, 0], 1024u16); // facing +X
        let mut s = Swim::spawn(0, pos, &mut w);
        s.hunting = true;
        s.state_left = 10_000;
        let motion = run(&mut w, &mut s, &mut pos, &mut yaw, 80);
        let fastest = motion.iter().map(|m| m.velocity[0]).max().unwrap();
        assert!(fastest >= 95, "top speed {fastest}");
        assert!(
            motion.iter().any(|m| m.biting),
            "never reached the prey: {pos:?}"
        );
    }

    #[test]
    fn bite_cadence_follows_the_attack_loop() {
        let mut w = Pool::new();
        w.prey = Some(Enemy {
            eye: [20, 250, 0],
            wet: true,
        });
        let (mut pos, mut yaw) = ([0, 245, 0], 1024u16);
        let mut s = Swim::spawn(0, pos, &mut w);
        s.hunting = true;
        s.state_left = 10_000;
        s.speed = 0;
        let mut bites = 0;
        for _ in 0..50 {
            let m = s.think(pos, yaw, true, &mut w);
            if m.bite {
                bites += 1;
                s.bit();
            }
            let _ = (&mut pos, &mut yaw);
        }
        // 5 seconds of contact. The loop is 0.2 s at rate 1.0 and the retail
        // flap rate stretches it to 1.9x while the speed is still catching up
        // (the first second), so the count sits between 25 and 50 bites.
        assert!((20..=45).contains(&bites), "bites {bites}");
    }

    #[test]
    fn a_wall_ahead_makes_it_back_off_and_turn() {
        let mut w = Pool::new();
        let (mut pos, mut yaw) = ([480, 200, 0], 1024u16); // 20 from the +X wall
        let mut s = Swim::spawn(0, pos, &mut w);
        s.state_left = 10_000;
        s.height = 200;
        s.speed = 50;
        let first = s.think(pos, yaw, true, &mut w);
        assert!(s.blocked && s.turning != 0, "should be avoiding the wall");
        assert!(first.yaw_rate_deg.abs() >= 90);
        let motion = run(&mut w, &mut s, &mut pos, &mut yaw, 40);
        assert!(pos[0] < 500, "never leaves the pool: {pos:?}");
        assert!(motion.last().unwrap().velocity[0] <= 55);
    }

    #[test]
    fn hunting_ends_when_the_state_runs_out() {
        let mut w = Pool::new();
        w.prey = Some(Enemy {
            eye: [300, 250, 0],
            wet: true,
        });
        let (mut pos, mut yaw) = ([0, 245, 0], 1024u16);
        let mut s = Swim::spawn(0, pos, &mut w);
        s.hunting = true;
        s.state_left = 4;
        run(&mut w, &mut s, &mut pos, &mut yaw, 3);
        assert!(!s.hunting);
        assert!(
            (3 * 20..=6 * 20).contains(&(s.state_left as i32)),
            "{}",
            s.state_left
        );
    }

    #[test]
    fn out_of_the_water_it_stops_and_flops() {
        let mut w = Pool::new();
        let mut s = Swim::spawn(0, [0, 200, 0], &mut w);
        s.speed = 50;
        let m = s.think([0, 500, 0], 0, false, &mut w);
        assert!(m.dry && m.velocity == [0; 3] && !m.biting);
    }

    #[test]
    fn view_cone_is_240_degrees_and_bites_need_a_facing_prey() {
        // Facing +Z: behind is outside, the side is inside.
        assert!(in_view(0, 0, 100));
        assert!(in_view(0, 100, 0));
        assert!(in_view(0, 100, -40));
        assert!(!in_view(0, 0, -100));
        assert!(bite_connects(0, 5, 100));
        assert!(!bite_connects(0, 100, 100));
    }

    #[test]
    fn displacement_carries_remainders_so_a_second_moves_exactly_the_speed() {
        let mut w = Pool::new();
        let mut s = Swim::spawn(0, [0, 200, 0], &mut w);
        s.vel = [50, -30, 0];
        s.yaw_rate = 90;
        let (mut moved, mut turned) = ([0i32; 3], 0);
        for _ in 0..20 {
            let (step, turn) = s.tick_displacement();
            for axis in 0..3 {
                moved[axis] += step[axis];
            }
            turned += turn;
        }
        assert_eq!(moved, [50, -30, 0]);
        // 90 degrees is a quarter of the 4096-unit circle.
        assert!((1022..=1024).contains(&turned), "turned {turned}");
    }

    #[test]
    fn a_dry_leech_falls_faster_each_tick_until_it_lands() {
        let mut w = Pool::new();
        let mut s = Swim::spawn(0, [0, 200, 0], &mut w);
        s.think([0, 500, 0], 0, false, &mut w);
        let first = s.tick_displacement().0[1];
        let second = s.tick_displacement().0[1];
        assert!(first <= 0 && second <= first);
        s.land();
        assert_eq!(s.fall, 0);
    }
}
