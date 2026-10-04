//! Integer-only rules of the scripted set pieces (gargantua, func_tank,
//! osprey, apache, func_mortar_field), kept free of PS1 state so the host
//! runner can test them.
//!
//! Time: the port simulates at 20 Hz and monsters think every 0.1 s, so a
//! "think" here is two simulation ticks. Where a rule scales by the frame time
//! the port uses the 20 Hz reference frame (0.05 s), the same frame the
//! deterministic reference captures run at.

/// Signed angle from `b` to `a` in q12 turns, wrapped into -2048..2047.
#[inline(always)]
pub const fn angle_dist_q12(a: i32, b: i32) -> i32 {
    ((a - b + 2048) & 0xfff) - 2048
}

/// Turn `value` toward `target` along the short way by at most `speed`, in
/// q12 turns.
#[inline]
pub const fn approach_angle_q12(target: i32, value: i32, speed: i32) -> i32 {
    let d = angle_dist_q12(target, value);
    if d > speed {
        value + speed
    } else if d < -speed {
        value - speed
    } else {
        target
    }
}

/// The shock wave a gargantua's stomp sends along the ground. Fixed point:
/// world units x16.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stomp {
    /// Current speed, units per second x16.
    pub speed_q4: i32,
    /// Acceleration, units per second squared x16. It grows as the wave runs.
    pub accel_q4: i32,
    /// Distance still to travel, units x16.
    pub remaining_q4: i32,
    /// Position steps the clock is ahead of the wave.
    pub steps_owed: u8,
}

/// A wave starts at 30 units per second squared, and that acceleration itself
/// grows by 1500 units per second squared every second.
const STOMP_START_ACCEL_Q4: i32 = 30 * 16;
const STOMP_JERK_Q4: i32 = 1500 * 16;
/// Speed and acceleration integrate over the 20 Hz reference frame (0.05 s).
const STOMP_FRAME_HZ: i32 = 20;
/// The wave's position advances in steps of 0.025 s, four per think (a think
/// is two 20 Hz ticks).
const STOMP_STEPS_PER_SECOND: i32 = 40;
const STOMP_STEPS_PER_THINK: u8 = 4;

impl Stomp {
    /// A wave at rest that will travel `dist` units.
    pub const fn new(dist: i32) -> Self {
        Self {
            speed_q4: 0,
            accel_q4: STOMP_START_ACCEL_Q4,
            remaining_q4: dist * 16,
            steps_owed: 0,
        }
    }

    /// How far (x16) its leading edge reaches in one frame: the length of the
    /// damage sweep before a think.
    pub const fn sweep_q4(&self) -> i32 {
        self.speed_q4 / STOMP_FRAME_HZ
    }

    /// One think, after the damage sweep: speed up, then move. The first think
    /// (at spawn) only speeds up. Position runs one step behind the clock, so
    /// the second think moves three steps and every later one four.
    /// Returns the distance moved (x16) and whether the wave is spent.
    pub fn think(&mut self, first: bool) -> (i32, bool) {
        self.speed_q4 += self.accel_q4 / STOMP_FRAME_HZ;
        self.accel_q4 += STOMP_JERK_Q4 / STOMP_FRAME_HZ;
        if !first {
            self.steps_owed += STOMP_STEPS_PER_THINK;
        }
        let steps = self.steps_owed.saturating_sub(1);
        self.steps_owed -= steps;
        let stride = self.speed_q4 / STOMP_STEPS_PER_SECOND;
        let moved = (i32::from(steps) * stride).min(self.remaining_q4.max(0));
        self.remaining_q4 -= moved;
        (moved, self.remaining_q4 <= 0)
    }
}

/// Flame damage falloff, in tenths of a hit point: full damage
/// within 64 units of the flame line, then 0.4 less per unit. `None` when
/// the target is out of reach.
pub const fn flame_damage_tenths(damage: i32, dist: i32) -> Option<i32> {
    let tenths = if dist > 64 {
        damage * 10 - (dist - 64) * 4
    } else {
        damage * 10
    };
    if tenths > 0 {
        Some(tenths)
    } else {
        None
    }
}

/// Gargantua sequence timing from garg.mdl, in 20 Hz ticks: the event frame
/// over the sequence fps, and the sequence length (frames - 1) / fps.
/// `Attack` (seq 8): 55 frames at 36 fps, GARG_AE_SLASH_LEFT on frame 28.
pub const GARG_SWIPE_EVENT_TICKS: u8 = 16;
pub const GARG_SWIPE_TICKS: u8 = 30;
/// `stomp` (seq 9): 21 frames at 14 fps, GARG_AE_STOMP on frame 19.
pub const GARG_STOMP_EVENT_TICKS: u8 = 27;
pub const GARG_STOMP_TICKS: u8 = 29;

/// What a gargantua does about its enemy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GargChoice {
    Stomp,
    Swipe,
    Flame,
    Chase,
}

/// The attack a gargantua picks, or `Chase` to close in. `facing_q12` is the
/// 2D cosine between its facing and the enemy (4096 = 1.0), `dist` the
/// distance between them, and the flags say whether the stomp and flame
/// cooldowns have run out.
///
/// Facing roughly ahead (cosine of at least 0.7): within 80 units it swipes,
/// further out it stomps if its stomp is ready. Otherwise, between 80 and 330
/// units and facing tightly ahead (0.8) it flames if its flame is ready.
pub const fn garg_choice(
    facing_q12: i32,
    dist: i32,
    stomp_ready: bool,
    flame_ready: bool,
) -> GargChoice {
    const SWIPE_REACH: i32 = 80;
    const FLAME_REACH: i32 = 330;
    const AHEAD_Q12: i32 = 2867;
    const TIGHTLY_AHEAD_Q12: i32 = 3277;
    if facing_q12 >= AHEAD_Q12 {
        if dist <= SWIPE_REACH {
            return GargChoice::Swipe;
        }
        if stomp_ready {
            return GargChoice::Stomp;
        }
    }
    if flame_ready && facing_q12 >= TIGHTLY_AHEAD_Q12 && dist > SWIPE_REACH && dist <= FLAME_REACH {
        return GargChoice::Flame;
    }
    GargChoice::Chase
}

/// Screen shake amplitude a player `dist` units away feels, or 0
/// outside the radius (players off the ground feel nothing either).
pub const fn shake_amplitude(amplitude: i32, dist: i32, radius: i32) -> i32 {
    if dist >= radius {
        0
    } else {
        amplitude * (radius - dist) / radius
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stomp_accelerates_on_the_reference_frame() {
        // Speeds after each think at frametime 0.05: 1.5, 6.75, 15.75, ...
        let mut s = Stomp::new(1024);
        assert_eq!(s.sweep_q4(), 0);
        assert_eq!(s.think(true), (0, false));
        assert_eq!(s.speed_q4, 24); // 1.5 u/s
        let (moved, done) = s.think(false);
        assert!(!done);
        assert_eq!(s.speed_q4, 24 + (480 + 1200) / 20); // 6.75 u/s
        assert_eq!(moved, 3 * (s.speed_q4 / 40));
        let (moved, _) = s.think(false);
        assert_eq!(moved, 4 * (s.speed_q4 / 40));
    }

    #[test]
    fn stomp_crosses_a_flame_length_in_under_two_seconds() {
        // A flame length (330 units) away: the wave covers it in 16..18 thinks.
        let mut s = Stomp::new(1024);
        let mut travelled = 0;
        let mut thinks = 0;
        let mut first = true;
        while travelled < 330 * 16 {
            travelled += s.think(first).0;
            first = false;
            thinks += 1;
            assert!(thinks < 40);
        }
        assert!((16..=18).contains(&thinks), "{thinks} thinks");
    }

    #[test]
    fn stomp_ends_after_its_distance() {
        let mut s = Stomp::new(100);
        let mut first = true;
        let mut n = 0;
        loop {
            let (_, done) = s.think(first);
            first = false;
            n += 1;
            if done {
                break;
            }
            assert!(n < 100);
        }
        assert!(s.remaining_q4 <= 0);
    }

    #[test]
    fn flame_falloff_is_full_within_64_units_then_fades() {
        assert_eq!(flame_damage_tenths(3, 10), Some(30));
        assert_eq!(flame_damage_tenths(3, 64), Some(30));
        assert_eq!(flame_damage_tenths(3, 69), Some(10));
        assert_eq!(flame_damage_tenths(3, 71), Some(2));
        assert_eq!(flame_damage_tenths(3, 72), None);
    }

    #[test]
    fn garg_prefers_stomp_then_swipe_then_flame() {
        assert_eq!(garg_choice(4096, 200, true, true), GargChoice::Stomp);
        assert_eq!(garg_choice(4096, 60, true, true), GargChoice::Swipe);
        assert_eq!(garg_choice(4096, 200, false, true), GargChoice::Flame);
        assert_eq!(garg_choice(3000, 200, false, true), GargChoice::Chase);
        assert_eq!(garg_choice(4096, 400, false, true), GargChoice::Chase);
        assert_eq!(garg_choice(1000, 60, true, true), GargChoice::Chase);
    }

    #[test]
    fn angles_wrap_the_short_way() {
        assert_eq!(angle_dist_q12(100, 4000), 196);
        assert_eq!(angle_dist_q12(4000, 100), -196);
        assert_eq!(approach_angle_q12(1000, 0, 91), 91);
        assert_eq!(approach_angle_q12(-50, 0, 91), -50);
    }

    #[test]
    fn shake_fades_to_the_radius() {
        assert_eq!(shake_amplitude(12, 0, 1000), 12);
        assert_eq!(shake_amplitude(12, 500, 1000), 6);
        assert_eq!(shake_amplitude(12, 1000, 1000), 0);
    }
}
