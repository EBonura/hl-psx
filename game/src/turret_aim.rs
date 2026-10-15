//! Turret gun aiming (turret.cpp CBaseTurret::ActiveThink, MoveTurret).
//!
//! The retail gun swings toward its target at TURRET_TURNRATE (30 degrees per
//! second), speeding up to ten times that while it is more than 30 degrees
//! off, and shoots along its *current* barrel direction with no spread, one
//! bullet per 0.1 s think once it points within 30 degrees of the target. A
//! target that crosses its line of fire faster than the barrel follows is
//! missed. The port keeps the gun yaw in the actor's own yaw and applies the
//! same limits per 20 Hz tick, stateless: the ramp of the retail turn rate is
//! folded into two rates.

/// Yaw units (4096 per turn).
pub const DEG30: u16 = 341;
/// Slew inside 30 degrees: about 45 degrees per second (30 to 60 in retail).
pub const TURN_NEAR: u16 = 26;
/// Slew outside 30 degrees: the ramp from 30 up to 300 degrees per second
/// turns 90 degrees in about 0.7 s, an average of 128 degrees per second.
pub const TURN_FAR: u16 = 73;
/// A bullet hits when it passes within this many units of the target axis
/// (the player's hull is 16 either side; Q12 radians cost 652 per unit).
pub const HIT_HALF_WIDTH: i32 = 18;
/// CTurret::SpinUpCall: 1.0 s before the barrel starts plus 12 thinks of 0.1 s.
pub const SPIN_UP_TICKS: u8 = 44;
/// m_flMaxSpin: the barrel spins down after 5 s without a target.
pub const SPIN_HOLD_TICKS: u8 = 100;
/// Retail turret that spins up (monster_turret); sentries and mini turrets
/// have the inline SpinUpCall and are ready at once.
pub const SPINNING_KIND: u8 = 21;

/// Shortest angular distance between two yaws.
#[inline(always)]
pub const fn yaw_error(cur: u16, want: u16) -> u16 {
    let d = (want.wrapping_sub(cur) & 0x0fff) as i32;
    (if d > 2048 { 4096 - d } else { d }) as u16
}

/// Yaw step for this tick given the error before it.
#[inline(always)]
pub const fn turn_step(err: u16) -> u16 {
    if err > DEG30 {
        TURN_FAR
    } else {
        TURN_NEAR
    }
}

/// ActiveThink only fires with the barrel within 30 degrees of the target
/// (dot product above 0.866).
#[inline(always)]
pub const fn can_fire(err: u16) -> bool {
    err <= DEG30
}

/// A zero-spread bullet along the barrel reaches a target `dist` units away
/// only if the yaw error leaves the axis inside the target's width.
#[inline(always)]
pub const fn bullet_hits(err: u16, dist: i32) -> bool {
    err as i32 * dist <= HIT_HALF_WIDTH * 652
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_error_takes_the_short_way() {
        assert_eq!(yaw_error(0, 100), 100);
        assert_eq!(yaw_error(100, 0), 100);
        assert_eq!(yaw_error(4000, 50), 146);
        assert_eq!(yaw_error(0, 2048), 2048);
    }

    #[test]
    fn a_strafing_target_outruns_the_barrel_up_close() {
        // The player strafes at 320 u/s = 16 units a tick at 300 units: the
        // bearing moves 16/300 rad = 0.053 rad = 35 yaw units a tick, more
        // than the 26 the barrel turns once it is close to the target.
        let dist = 300;
        let bearing_rate = 16 * 652 / dist;
        assert!(bearing_rate > TURN_NEAR as i32);
        // After a few ticks the barrel is far enough behind to miss.
        let mut err = 0i32;
        for _ in 0..6 {
            err = (err + bearing_rate - TURN_NEAR as i32).max(0);
        }
        assert!(!bullet_hits(err as u16, dist));
        // At 900 units the same strafe is only 11 units a tick and is tracked.
        assert!(16 * 652 / 900 < TURN_NEAR as i32);
    }

    #[test]
    fn standing_targets_are_always_hit_within_range() {
        assert!(bullet_hits(0, 1200));
        assert!(bullet_hits(9, 1200)); // 0.77 degrees of lag at full range
        assert!(!bullet_hits(20, 1200));
        assert!(bullet_hits(100, 100));
        assert!(!bullet_hits(DEG30 + 1, 600));
        assert!(can_fire(DEG30) && !can_fire(DEG30 + 1));
    }

    #[test]
    fn turn_rates_match_the_retail_ramp_ends() {
        // 30 degrees per second is 1.5 per tick = 17 units; the retail rate
        // oscillates 30..60 near the target and ramps to 300 far from it.
        assert!(TURN_NEAR >= 17 && TURN_NEAR <= 34);
        assert!(TURN_FAR as i32 * 20 * 360 / 4096 <= 300);
        assert_eq!(turn_step(DEG30), TURN_NEAR);
        assert_eq!(turn_step(DEG30 + 1), TURN_FAR);
    }
}
