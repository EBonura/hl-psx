// SPDX-License-Identifier: GPL-2.0-or-later
//! The small player rules of the retail movement and health code, as arithmetic.
//!
//! Everything here is a pure function of numbers so the host test crate can
//! check it; `phys.rs` and `main.rs` supply the traces and the world. Units:
//! velocities are Q6 world units per 20 Hz tick (`64` = one unit per tick =
//! 20 units per second), friction and angle fractions are Q12.
//!
//! The rules were written from the behaviour of the original player movement
//! and player code, not from its source text.

use psx_math::int32::isqrt_i32;

/// Q6 per world unit per tick.
pub const Q6: i32 = 64;
/// Ticks per second of the simulation.
pub const HZ: i32 = 20;

/// Convert units per second to Q6 units per tick, rounding to nearest.
pub const fn ups_to_q6(ups: i32) -> i32 {
    (ups * Q6 + HZ / 2) / HZ
}

// ---------------------------------------------------------------- friction

/// Acceleration step for one tick: `gap` is the speed still missing toward the
/// wish speed, `wish` the wish speed. The retail step is
/// `accel * dt * wish * friction` with `accel * dt = 0.5`, capped by the gap,
/// where `friction` is the toucher's friction scale (1.0 except inside a
/// `func_friction` volume).
pub const fn accel_step(gap: i32, wish: i32, friction_q12: i32) -> i32 {
    let mut step = (wish * friction_q12) >> 13;
    if step == 0 && friction_q12 > 0 {
        // one Q6 count is the finest push there is
        step = 1;
    }
    if gap < step {
        gap
    } else {
        step
    }
}

/// Ground friction scale with the edge rule: the original doubles friction
/// when the player is walking toward a drop so he stops at a ledge.
pub const fn edge_scaled(friction_q12: i32, over_drop: bool) -> i32 {
    if over_drop {
        friction_q12 * 2
    } else {
        friction_q12
    }
}

/// Where the edge probe starts and ends, relative to the origin of a player
/// moving with horizontal velocity `(vx, vz)` (Q6): 16 units ahead along the
/// velocity, at the feet, and 34 units below that. `None` when there is no
/// direction. The length uses the octagon approximation (within 4%), which is
/// plenty for a 16 unit probe and saves a square root.
#[cfg_attr(target_arch = "mips", optimize(size))]
#[inline(never)]
pub fn edge_probe(vx: i32, vz: i32, feet: i32) -> Option<([i32; 3], [i32; 3])> {
    let (ax, az) = (vx.abs(), vz.abs());
    let len = ax.max(az) + ax.min(az) * 3 / 8;
    if len == 0 {
        return None;
    }
    let ahead = |v: i32| v * 16 / len;
    let (x, z) = (ahead(vx), ahead(vz));
    Some(([x, -feet, z], [x, -feet - 34, z]))
}

/// Overbounce used when a walking player hits a wall while inside a friction
/// volume: `1 + bounce * (1 - friction)` with `sv_bounce` 1, in Q12. Floors
/// and slopes (up-component above 0.7) always clip at exactly 1.
pub const fn wall_overbounce_q12(friction_q12: i32, up_q12: i32) -> i32 {
    if up_q12 > 2867 || friction_q12 >= 4096 {
        4096
    } else {
        4096 + (4096 - friction_q12)
    }
}

// ------------------------------------------------------------- bunny hopping

/// Speed above which a ground jump is cut: 1.7 times the 320 u/s run speed.
pub const BUNNY_LIMIT_Q6: i32 = ups_to_q6(544);

/// The jump scales a horizontal velocity above the limit down to 65% of the
/// limit (`limit / speed * 0.65` of the velocity).
#[cfg_attr(target_arch = "mips", optimize(size))]
#[inline(never)]
pub fn bunny_cap(vx: i32, vz: i32) -> (i32, i32) {
    let speed = isqrt_i32(vx * vx + vz * vz);
    if speed <= BUNNY_LIMIT_Q6 {
        return (vx, vz);
    }
    let num = BUNNY_LIMIT_Q6 * 65;
    let den = speed * 100;
    ((vx * num) / den, (vz * num) / den)
}

// ------------------------------------------------------------------ long jump

/// Long jump launch speed: 350 * 1.6 u/s along the horizontal part of the
/// view direction.
pub const LONGJUMP_Q6: i32 = ups_to_q6(560);

/// Ticks after pressing duck during which the module can fire (1 s).
pub const LONGJUMP_WINDOW: u8 = 20;

/// Horizontal launch velocity (Q6) along the view: `sin_yaw`, `cos_yaw` and
/// `cos_pitch` are Q12. Looking up or down shortens the launch.
#[cfg_attr(target_arch = "mips", optimize(size))]
#[inline(never)]
pub fn longjump_velocity(sin_yaw: i32, cos_yaw: i32, cos_pitch: i32) -> (i32, i32) {
    let h = LONGJUMP_Q6 * cos_pitch >> 12;
    ((sin_yaw * h) >> 12, (cos_yaw * h) >> 12)
}

/// The module fires only for a ducked jump with duck held, inside the window
/// after the duck press, and while moving faster than 50 u/s.
pub fn longjump_ready(owned: bool, duck_held: bool, window: u8, vx: i32, vz: i32) -> bool {
    let min = ups_to_q6(50);
    owned && duck_held && window > 0 && vx * vx + vz * vz > min * min
}

/// Advance the post-press duck window one tick.
pub const fn duck_window_next(window: u8, pressed: bool, ducking: bool, requested: bool) -> u8 {
    if pressed && !ducking {
        LONGJUMP_WINDOW
    } else if !ducking && !requested {
        0
    } else if window > 0 {
        window - 1
    } else {
        0
    }
}

// ----------------------------------------------------------------- duck view

/// Ticks the duck takes on the ground.
pub const DUCK_TICKS: i32 = 8;
pub const VIEW_STAND: i32 = 28;
pub const VIEW_DUCK: i32 = 12;
/// Standing hull to ducking hull difference of the feet: 36 - 18.
pub const DUCK_DROP: i32 = 18;

/// The eye height above the origin while a grounded duck is still on its way
/// down, `elapsed` ticks after the press: the original eases the view with a
/// cubic ease in/out spline over 0.4 s toward the ducking view minus the
/// origin drop that the hull change applies at the end, so the eye is
/// continuous when the origin drops.
pub fn duck_view_height(elapsed: i32) -> i32 {
    let t = (elapsed.clamp(0, DUCK_TICKS) * 4096) / DUCK_TICKS;
    let t2 = (t * t) >> 12;
    let spline = 3 * t2 - ((2 * t2 * t) >> 12);
    let target = VIEW_DUCK - DUCK_DROP;
    (target * spline + VIEW_STAND * (4096 - spline) + 2048) >> 12
}

/// Eye rise per tick after standing up: the client smooths the 18 unit origin
/// step at 150 u/s.
pub const VIEW_RISE: i32 = 8;

/// Next eye height above the origin. `pending` is the ticks since the press
/// while a grounded duck is still on its way down; `crouch` the hull in use.
#[cfg_attr(target_arch = "mips", optimize(size))]
#[inline(never)]
pub fn view_height_next(cur: i32, crouch: bool, was_crouch: bool, pending: Option<i32>) -> i32 {
    if crouch {
        VIEW_DUCK
    } else if let Some(elapsed) = pending {
        duck_view_height(elapsed)
    } else if was_crouch {
        // The hull just stood up and the origin rose 18: the smoothed eye
        // starts 18 below its standing height and climbs back.
        VIEW_STAND - DUCK_DROP
    } else {
        (cur + VIEW_RISE).min(VIEW_STAND)
    }
}

// ---------------------------------------------------------------------- use

/// Ticks between pulses of a held use button (chargers, valve wheels).
pub const USE_PULSE_TICKS: u8 = 8;

/// One tick of the use button: `(edge, want, cooldown)`. `edge` is the press
/// (what a door, a button or a scientist reacts to, with no cooldown), `want`
/// whether a use is due (the press, or the held button's next pulse).
pub const fn use_phase(held: bool, was_held: bool, cooldown: u8) -> (bool, bool, u8) {
    let left = cooldown.saturating_sub(1);
    let edge = held && !was_held;
    let want = held && (left == 0 || edge);
    (edge, want, if want { USE_PULSE_TICKS } else { left })
}

// ------------------------------------------------------------------ ladders

/// Jump-off speed from a ladder: 270 u/s along the ladder normal.
pub const LADDER_DISMOUNT_Q6: i32 = ups_to_q6(270);

// ------------------------------------------------------------------- water

/// Water friction: each tick keeps `1 - 0.05 * 4` of the speed, times the
/// toucher's friction scale.
pub const fn water_keep_q12(friction_q12: i32) -> i32 {
    4096 - ((4096 * 4 / 20) * friction_q12 >> 12)
}

/// Swimming wish speed: 0.8 of the 320 u/s run speed.
pub const SWIM_WISH_Q6: i32 = ups_to_q6(256);
/// Idle drift: the original adds -60 u/s then scales the wish by 0.8.
pub const SWIM_SINK_Q6: i32 = ups_to_q6(48);

// ----------------------------------------------------------------- drowning

/// Lungs: 12 s of air.
pub const AIR_TICKS: u16 = 240;

/// Drowning and the recovery after it.
#[derive(Clone, Copy, Debug)]
pub struct Breath {
    /// Air left, in ticks.
    pub air: u16,
    /// Damage of the next drowning hit minus one (the original adds one before
    /// each hit, from 2, and stops at 5).
    pub dmg: u8,
    /// Ticks to the next drowning hit once air has run out.
    pub pain_clock: u8,
    /// Health taken by drowning so far, and how much of it was handed back.
    pub drowned: u16,
    pub restored: u16,
    /// Ticks to the next recovery pulse (the original checks every 2 s).
    pub recover_clock: u8,
}

impl Breath {
    pub const fn new() -> Self {
        Self {
            air: AIR_TICKS,
            dmg: 2,
            pain_clock: 0,
            drowned: 0,
            restored: 0,
            recover_clock: 0,
        }
    }

    /// One tick. Returns `(damage, heal)` for this tick. `under` is "the eyes
    /// are under water", `alive` whether the player still lives.
    #[cfg_attr(target_arch = "mips", optimize(size))]
    #[inline(never)]
    pub fn tick(&mut self, under: bool, alive: bool) -> (u16, u16) {
        if !alive {
            return (0, 0);
        }
        if under {
            self.recover_clock = 0;
            if self.air > 0 {
                self.air -= 1;
                return (0, 0);
            }
            if self.pain_clock > 0 {
                self.pain_clock -= 1;
                return (0, 0);
            }
            self.dmg = (self.dmg + 1).min(5);
            self.pain_clock = HZ as u8 - 1;
            self.drowned += self.dmg as u16;
            return (self.dmg as u16, 0);
        }
        self.air = AIR_TICKS;
        self.dmg = 2;
        self.pain_clock = 0;
        if self.drowned > self.restored {
            if self.recover_clock == 0 {
                self.recover_clock = 2 * HZ as u8 - 1;
                let give = (self.drowned - self.restored).min(10);
                self.restored += give;
                return (0, give);
            }
            self.recover_clock -= 1;
        }
        (0, 0)
    }

}

// ---------------------------------------------------------- time based damage

/// `DMG_POISON` is the only time-based damage that keeps hurting after the
/// player leaves its source: 2 health every 2 s for 5 checks.
pub const POISON_DAMAGE: u16 = 2;
pub const POISON_CHECKS: u8 = 5;

/// Time-based damage tracker (only poison deals damage in the original).
#[derive(Clone, Copy, Debug)]
pub struct Poison {
    /// Checks left; zero when not poisoned.
    pub left: u8,
    /// Ticks to the next check.
    pub clock: u8,
    armed: bool,
}

impl Poison {
    pub const fn new() -> Self {
        Self {
            left: 0,
            clock: 0,
            armed: false,
        }
    }

    /// A poisonous hit: the countdown restarts at the next check.
    pub fn poison(&mut self) {
        self.armed = true;
        self.left = 0;
        if self.clock == 0 {
            self.clock = 1;
        }
    }

    /// One tick; returns the damage for this tick (a check happens at most
    /// every 2 s).
    #[cfg_attr(target_arch = "mips", optimize(size))]
    #[inline(never)]
    pub fn tick(&mut self) -> u16 {
        if !self.armed {
            return 0;
        }
        if self.clock > 0 {
            self.clock -= 1;
            if self.clock > 0 {
                return 0;
            }
        }
        self.clock = 2 * HZ as u8;
        // The first check after a hit hurts and arms the duration; every
        // later one hurts and counts down.
        let dmg = POISON_DAMAGE;
        if self.left == 0 {
            self.left = POISON_CHECKS;
        } else {
            self.left -= 1;
            if self.left == 0 {
                self.armed = false;
                self.clock = 0;
            }
        }
        dmg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_convert_like_the_rest_of_the_port() {
        assert_eq!(ups_to_q6(320), 1024);
        assert_eq!(ups_to_q6(20), 64);
        assert_eq!(LADDER_DISMOUNT_Q6, 864);
        assert_eq!(BUNNY_LIMIT_Q6, 1741);
        assert_eq!(LONGJUMP_Q6, 1792);
    }

    #[test]
    fn friction_scales_the_acceleration_step() {
        // full friction: half the wish speed per tick, as before
        assert_eq!(accel_step(10_000, 1024, 4096), 512);
        // ice at 5%: a twentieth of that
        assert_eq!(accel_step(10_000, 1024, 205), 25);
        // zero friction never accelerates
        assert_eq!(accel_step(10_000, 1024, 0), 0);
        // the gap still caps the step
        assert_eq!(accel_step(100, 1024, 4096), 100);
    }

    #[test]
    fn edge_friction_doubles_only_over_a_drop() {
        assert_eq!(edge_scaled(4096, false), 4096);
        assert_eq!(edge_scaled(4096, true), 8192);
        assert_eq!(edge_scaled(2048, true), 4096);
    }

    #[test]
    fn edge_probe_looks_sixteen_ahead_and_thirty_four_down() {
        let (a, b) = edge_probe(640, 0, 36).unwrap();
        assert_eq!(a, [16, -36, 0]);
        assert_eq!(b, [16, -70, 0]);
        let (a, _) = edge_probe(0, -300, 18).unwrap();
        assert_eq!(a, [0, -18, -16]);
        assert!(edge_probe(0, 0, 36).is_none());
        // diagonal: within a unit of 16 / sqrt(2) on each axis
        let (a, _) = edge_probe(500, 500, 36).unwrap();
        assert!((a[0] - 11).abs() <= 1 && (a[2] - 11).abs() <= 1, "{a:?}");
    }

    #[test]
    fn walls_bounce_only_inside_a_friction_volume() {
        assert_eq!(wall_overbounce_q12(4096, 0), 4096);
        // ice 5%: nearly elastic, 1.95
        assert_eq!(wall_overbounce_q12(205, 0), 4096 + 3891);
        // floors and slopes keep the plain clip
        assert_eq!(wall_overbounce_q12(205, 3000), 4096);
    }

    #[test]
    fn mega_bunny_cut_only_above_the_limit() {
        assert_eq!(bunny_cap(1024, 0), (1024, 0));
        // whatever the speed above the limit, it comes out at 65% of the limit
        for fast in [1792, 2400, 3000] {
            let (x, z) = bunny_cap(fast, 0);
            let want = BUNNY_LIMIT_Q6 * 65 / 100;
            assert!((isqrt_i32(x * x + z * z) - want).abs() <= 2, "{fast}");
        }
        // direction is kept
        let (x, z) = bunny_cap(1000, 1500);
        assert!(x > 0 && z > 0 && (z * 2 - x * 3).abs() < 8);
    }

    #[test]
    fn long_jump_follows_the_view_and_shortens_with_pitch() {
        // facing +z (yaw 0): full 560 u/s straight ahead
        let (vx, vz) = longjump_velocity(0, 4096, 4096);
        assert_eq!((vx, vz), (0, LONGJUMP_Q6));
        // 60 degrees of pitch: cos = 0.5
        let (_, vz) = longjump_velocity(0, 4096, 2048);
        assert_eq!(vz, LONGJUMP_Q6 / 2);
        // facing +x
        let (vx, vz) = longjump_velocity(4096, 0, 4096);
        assert_eq!((vx, vz), (LONGJUMP_Q6, 0));
    }

    #[test]
    fn long_jump_needs_the_window_the_module_and_speed() {
        let fast = ups_to_q6(100);
        assert!(longjump_ready(true, true, 5, fast, 0));
        assert!(!longjump_ready(false, true, 5, fast, 0));
        assert!(!longjump_ready(true, false, 5, fast, 0));
        assert!(!longjump_ready(true, true, 0, fast, 0));
        assert!(!longjump_ready(true, true, 5, ups_to_q6(40), 0));
    }

    #[test]
    fn duck_window_opens_on_the_press_and_closes_after_a_second() {
        let mut w = 0;
        w = duck_window_next(w, true, false, true);
        assert_eq!(w, LONGJUMP_WINDOW);
        for _ in 0..LONGJUMP_WINDOW {
            w = duck_window_next(w, false, true, true);
        }
        assert_eq!(w, 0);
        // releasing before the hull is active closes it at once
        let w = duck_window_next(10, false, false, false);
        assert_eq!(w, 0);
        // a press while already ducked does not reopen it
        assert_eq!(duck_window_next(0, true, true, true), 0);
    }

    #[test]
    fn duck_view_eases_down_to_the_continuous_eye() {
        assert_eq!(duck_view_height(0), VIEW_STAND);
        // the end value is the ducking view minus the hull drop
        assert_eq!(duck_view_height(DUCK_TICKS), VIEW_DUCK - DUCK_DROP);
        // halfway the spline is at one half
        let mid = duck_view_height(DUCK_TICKS / 2);
        assert_eq!(mid, (VIEW_STAND + VIEW_DUCK - DUCK_DROP) / 2);
        // monotone
        let mut prev = VIEW_STAND + 1;
        for e in 0..=DUCK_TICKS {
            let h = duck_view_height(e);
            assert!(h < prev, "tick {e}: {h} !< {prev}");
            prev = h;
        }
    }

    #[test]
    fn the_eye_follows_the_duck_and_the_stand() {
        // pressing duck on the ground: spline down
        assert_eq!(view_height_next(28, false, false, Some(0)), 28);
        let mut cur = 28;
        for e in 0..DUCK_TICKS {
            cur = view_height_next(cur, false, false, Some(e));
        }
        assert_eq!(cur, duck_view_height(DUCK_TICKS - 1));
        // the hull switches: the eye is the ducking view and no jump of the
        // world eye (origin drops by the same amount the view number rises)
        let before = 36 + duck_view_height(DUCK_TICKS);
        let after = 18 + view_height_next(cur, true, false, None);
        assert_eq!(before, after);
        // standing up: starts 18 low and climbs by the smoothing rate
        let up = view_height_next(12, false, true, None);
        assert_eq!(up, 10);
        assert_eq!(view_height_next(up, false, false, None), 18);
        assert_eq!(view_height_next(26, false, false, None), 28);
        assert_eq!(view_height_next(28, false, false, None), 28);
    }

    #[test]
    fn use_edge_is_the_press_and_pulses_follow_the_hold() {
        // a tap: one edge, one want
        let (e, w, c) = use_phase(true, false, 0);
        assert!(e && w);
        assert_eq!(c, USE_PULSE_TICKS);
        // held on: no more edges, a pulse every USE_PULSE_TICKS
        let mut c = c;
        let mut pulses = 0;
        for _ in 0..40 {
            let (e, w, next) = use_phase(true, true, c);
            assert!(!e);
            if w {
                pulses += 1;
            }
            c = next;
        }
        assert_eq!(pulses, 5);
        // released: nothing
        let (e, w, _) = use_phase(false, true, 3);
        assert!(!e && !w);
        // a second tap inside the cooldown is still a press
        let (_, _, c) = use_phase(true, false, 0);
        let (_, _, c) = use_phase(false, true, c);
        let (e, w, _) = use_phase(true, false, c);
        assert!(e && w);
    }

    #[test]
    fn water_keeps_eighty_percent_per_tick() {
        assert_eq!(water_keep_q12(4096), 3277);
        assert_eq!(water_keep_q12(0), 4096);
        assert_eq!(SWIM_WISH_Q6, 819);
        assert_eq!(SWIM_SINK_Q6, 154);
    }

    #[test]
    fn drowning_starts_the_moment_the_air_ends_with_three_four_five_five() {
        let mut b = Breath::new();
        let mut hits = [0u16; 8];
        let mut first = 0;
        let mut n = 0;
        for tick in 1..=400 {
            let (d, _) = b.tick(true, true);
            if d > 0 {
                if n == 0 {
                    first = tick;
                }
                hits[n] = d;
                n += 1;
            }
        }
        // 12 s of air, then the first hit on the next tick
        assert_eq!(first, 241);
        assert_eq!(&hits[..5], &[3, 4, 5, 5, 5]);
        assert_eq!(b.drowned, 3 + 4 + 5 + 5 + 5 + 5 + 5 + 5);
    }

    #[test]
    fn drowning_hits_are_a_second_apart() {
        let mut b = Breath::new();
        let mut at = [0; 3];
        let mut n = 0;
        for tick in 1..=400 {
            if b.tick(true, true).0 > 0 && n < 3 {
                at[n] = tick;
                n += 1;
            }
        }
        assert_eq!(at, [241, 261, 281]);
    }

    #[test]
    fn surfacing_gives_the_health_back_ten_at_a_time_every_two_seconds() {
        let mut b = Breath::new();
        for _ in 0..300 {
            b.tick(true, true);
        }
        let owed = b.drowned;
        assert!(owed > 10);
        let mut healed = 0;
        let mut pulses = 0;
        let mut last_pulse = 0;
        for tick in 1..=400 {
            let (_, h) = b.tick(false, true);
            if h > 0 {
                assert!(h <= 10);
                if pulses > 0 {
                    assert_eq!(tick - last_pulse, 40);
                }
                last_pulse = tick;
                healed += h;
                pulses += 1;
            }
        }
        assert_eq!(healed, owed);
        // air and damage reset on the first breath
        assert_eq!(b.air, AIR_TICKS);
        assert_eq!(b.dmg, 2);
    }

    #[test]
    fn a_dead_player_neither_drowns_nor_heals() {
        let mut b = Breath::new();
        for _ in 0..500 {
            assert_eq!(b.tick(true, false), (0, 0));
        }
    }

    #[test]
    fn poison_hurts_every_two_seconds_for_five_checks_after_the_last_touch() {
        let mut p = Poison::new();
        assert_eq!(p.tick(), 0);
        p.poison();
        let mut total = 0;
        let mut hits = 0;
        for _ in 0..400 {
            let d = p.tick();
            if d > 0 {
                hits += 1;
                total += d;
            }
        }
        // the arming check plus five more, two health each
        assert_eq!(hits, 6);
        assert_eq!(total, 12);
        assert_eq!(p.tick(), 0);
    }

    #[test]
    fn a_fresh_poison_touch_restarts_the_countdown() {
        let mut p = Poison::new();
        p.poison();
        for _ in 0..70 {
            p.tick();
        }
        p.poison();
        let mut hits = 0;
        for _ in 0..400 {
            if p.tick() > 0 {
                hits += 1;
            }
        }
        assert_eq!(hits, 6);
    }
}
