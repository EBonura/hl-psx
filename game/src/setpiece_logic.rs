//! Integer-only rules of the scripted set pieces (gargantua, func_tank,
//! osprey, apache, func_mortar_field), kept free of PS1 state so the host
//! runner can test them against the HLSDK arithmetic they reproduce.
//!
//! Time: the port simulates at 20 Hz and GoldSrc monsters think every 0.1 s,
//! so a "think" here is two simulation ticks. Where HLSDK scales by
//! `gpGlobals->frametime` the port uses the 20 Hz reference frame (0.05 s),
//! the same frame the deterministic Xash reference captures run at.

/// `UTIL_AngleDistance` in q12 turns: `a - b` wrapped into -2048..2047.
#[inline(always)]
pub const fn angle_dist_q12(a: i32, b: i32) -> i32 {
    ((a - b + 2048) & 0xfff) - 2048
}

/// `UTIL_ApproachAngle(target, value, speed)` in q12 turns.
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

/// CStomp (gargantua.cpp): the shock wave a gargantua's stomp sends along
/// the ground. Fixed point: world units x16.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stomp {
    /// `pev->speed`, units/s x16.
    pub speed_q4: i32,
    /// `pev->framerate`, which the effect uses as its acceleration, x16.
    pub rate_q4: i32,
    /// `pev->scale`: the distance still to travel, units x16.
    pub life_q4: i32,
    /// STOMP_INTERVAL (0.025 s) steps owed: `gpGlobals->time - pev->dmgtime`.
    pub owed: u8,
}

impl Stomp {
    /// `CStomp::StompCreate(origin, end, 0)`: framerate 30, speed 0, the
    /// whole start-to-end distance as its life.
    pub const fn new(dist: i32) -> Self {
        Self {
            speed_q4: 0,
            rate_q4: 30 * 16,
            life_q4: dist * 16,
            owed: 0,
        }
    }

    /// Length (x16) of this think's damage hull trace: `speed * frametime`.
    pub const fn sweep_q4(&self) -> i32 {
        self.speed_q4 / 20
    }

    /// One `CStomp::Think` after its damage trace: accelerate, then advance
    /// in 0.025 s steps. The first think runs at spawn with nothing owed;
    /// each later one owes four steps, and the loop only takes a step while
    /// more than one is owed (`time - dmgtime > STOMP_INTERVAL`), so the
    /// wave moves three steps on its second think and four after that.
    /// Returns the distance moved (x16) and whether the stomp is spent.
    pub fn think(&mut self, first: bool) -> (i32, bool) {
        self.speed_q4 += self.rate_q4 / 20;
        self.rate_q4 += 1500 * 16 / 20;
        if !first {
            self.owed += 4;
        }
        let mut moved = 0;
        while self.owed > 1 {
            let step = self.speed_q4 / 40;
            moved += step;
            self.owed -= 1;
            self.life_q4 -= step;
            if self.life_q4 <= 0 {
                return (moved, true);
            }
        }
        (moved, false)
    }
}

/// CGargantua::FlameDamage falloff, in tenths of a hit point: full damage
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

/// `CheckAttacks` for the gargantua. `dot_q12` is the 2D cosine between its
/// facing and the enemy, `dist` the origin-to-origin distance, and the two
/// flags whether `m_seeTime` and `m_flameTime` have passed. Returns the
/// schedule GoldSrc's combat state picks: range attack 1 (stomp) before
/// melee 1 (swipe) before melee 2 (flame), else chase. (GoldSrc's face
/// schedule needs range 1 or melee 1, which already won.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GargChoice {
    Stomp,
    Swipe,
    Flame,
    Chase,
}

pub const fn garg_choice(dot_q12: i32, dist: i32, see_passed: bool, flame_passed: bool) -> GargChoice {
    const ATTACKDIST: i32 = 80;
    const FLAME_LENGTH: i32 = 330;
    let range1 = see_passed && dot_q12 >= 2867 && dist > ATTACKDIST;
    let melee1 = dot_q12 >= 2867 && dist <= ATTACKDIST;
    let melee2 = flame_passed && dot_q12 >= 3277 && dist > ATTACKDIST && dist <= FLAME_LENGTH;
    if range1 {
        GargChoice::Stomp
    } else if melee1 {
        GargChoice::Swipe
    } else if melee2 {
        GargChoice::Flame
    } else {
        GargChoice::Chase
    }
}

/// `UTIL_ScreenShake` amplitude a player `dist` units away feels, or 0
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
    fn stomp_follows_the_reference_frame_acceleration() {
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
        // GARG_FLAME_LENGTH (330) away: the wave covers it in 16..18 thinks.
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
        assert!(s.life_q4 <= 0);
    }

    #[test]
    fn flame_falloff_matches_flamedamage() {
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
