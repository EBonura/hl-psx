//! Turret gun timing and aiming (turret.cpp CBaseTurret: Deploy, ActiveThink,
//! MoveTurret).
//!
//! Everything runs on the retail 0.1 s think, which is two 20 Hz ticks.
//!
//! Deploy: an auto-activating turret sees its target on AutoSearchThink (0.3 s
//! cadence), waits 0.3 s, then plays its deploy clip (miniturret 31 frames at
//! 30 fps = 1.0 s, sentry 8 at 20 = 0.35 s, turret 51 at 30 = 1.67 s) one think
//! at a time, then needs a SearchThink and one ActiveThink to spin up before the
//! second ActiveThink fires. Measured in the retail oracle (hl-chain/ref/tur1,
//! tur4 and hl-parity/ref/tur1 to tur5, c1a2 miniturret): first sight to first
//! shot is 6 + clip + 6 ticks plus the 0 to 5 ticks until the next AutoSearch.
//!
//! Aim: the gun swings toward the target at TURRET_TURNRATE (30 degrees per
//! second), speeding up to ten times that while it is more than 30 degrees
//! off, and shoots along its *current* barrel direction with zero spread (the
//! source's TURRET_SPREAD is the zero vector), one bullet per think once it
//! points within 30 degrees of the target. FBoxVisible overwrites the aim point
//! with a random point of the target's box on every think, and the barrel moves
//! 3 or 6 degrees a think (the turn rate alternates 30 and 60 inside 30 degrees)
//! without pegging to the goal unless it is within 1.5 degrees, so a standing
//! player is hit on about one shot in three: retail took 100 HP from a standing
//! player in 86 to 129 ticks after the first hit (five oracle runs, one hit per
//! 5.6 ticks on average). The port keeps the gun yaw in the actor's own yaw and
//! steps it once per think with the same quantised steps.

/// Yaw units (4096 per turn).
pub const DEG30: u16 = 341;
/// ActiveThink period: 0.1 s.
pub const THINK_TICKS: u8 = 2;
/// MoveTurret snaps the barrel onto the goal inside 0.05 x TURRET_TURNRATE =
/// 1.5 degrees.
pub const SNAP: u16 = 17;
/// Yaw step of one think inside 30 degrees at the low rate: 3 degrees.
pub const STEP_LOW: u16 = 34;
/// The same at the high rate (the rate alternates 30 and 60): 6 degrees.
pub const STEP_HIGH: u16 = 68;
/// Yaw step of one think outside 30 degrees: the rate ramps 30 per think up to
/// 300, which turns 90 degrees in about 0.7 s, an average of 128 degrees per
/// second.
pub const STEP_FAR: u16 = 146;
/// A bullet hits when it passes within this many units of the target axis. The
/// player model's hitboxes are narrower than its 16-unit hull; 9 reproduces the
/// retail hit rate against the random aim point (see the module notes).
pub const HIT_HALF_WIDTH: i32 = 9;
/// The target box's half width: FBoxVisible picks the aim point inside it.
pub const BOX_HALF_WIDTH: i32 = 16;
/// CTurret::SpinUpCall: 1.0 s before the barrel starts plus 12 thinks of 0.1 s.
pub const SPIN_UP_TICKS: u8 = 44;
/// m_flMaxSpin: the barrel spins down after 5 s without a target.
pub const SPIN_HOLD_TICKS: u8 = 100;
/// Retail turret that spins up (monster_turret); sentries and mini turrets
/// have the inline SpinUpCall and are ready at once.
pub const SPINNING_KIND: u8 = 21;

/// Ticks of the deploy clip (sentry.mdl, miniturret.mdl and turret.mdl
/// sequence 3), rounded up to whole thinks.
#[inline(always)]
pub const fn deploy_ticks(kind: u8) -> u8 {
    match kind {
        20 => 8,
        21 => 34,
        _ => 20,
    }
}

/// Ticks from the end of the deploy clip to the first shot: SearchThink, the
/// spin-up ActiveThink and the firing one.
pub const DEPLOYED_SHOT_TICKS: u8 = 8;

/// Ticks a cold turret needs beyond `DEPLOYED_SHOT_TICKS` when it deploys on
/// sight instead of on a Use: the 6 ticks of the 0.3 s nextthink, the 2.5 the
/// target waits on average for the next AutoSearch (less the 1.5 the port's own
/// target search adds), and the 6 ticks the retail gun needs to swing its
/// pitch onto a target that is not level with it (the first hit came 9 ticks
/// after the first shot against a target 18 degrees below a ceiling gun).
pub const SIGHT_EXTRA_TICKS: u8 = 11;

/// Ticks from the start of deployment to the first shot, `extra` being 0 for a
/// Use (or a sentry's touch or damage) and `SIGHT_EXTRA_TICKS` for a sight.
#[inline(always)]
pub const fn shot_ticks(kind: u8, extra: u8) -> u8 {
    deploy_ticks(kind) + DEPLOYED_SHOT_TICKS + extra
}

/// Shortest angular distance between two yaws.
#[inline(always)]
pub const fn yaw_error(cur: u16, want: u16) -> u16 {
    let d = (want.wrapping_sub(cur) & 0x0fff) as i32;
    (if d > 2048 { 4096 - d } else { d }) as u16
}

/// Signed short-way yaw difference `goal - cur`, in -2048..=2048.
#[inline(always)]
const fn yaw_diff(cur: u16, goal: u16) -> i32 {
    let d = (goal.wrapping_sub(cur) & 0x0fff) as i32;
    if d > 2048 {
        d - 4096
    } else {
        d
    }
}

/// One MoveTurret yaw step. Inside 30 degrees the barrel moves a fixed 3 or 6
/// degrees (`high` alternates every think) and may overshoot the goal; only a
/// goal within 1.5 degrees is taken exactly. Outside 30 degrees it turns at the
/// ramped rate, which never reaches the goal in one think.
#[inline(always)]
pub const fn swing(cur: u16, goal: u16, high: bool) -> u16 {
    let d = yaw_diff(cur, goal);
    let dist = d.unsigned_abs() as u16;
    if dist < SNAP {
        return goal;
    }
    let step = (if dist > DEG30 {
        STEP_FAR
    } else if high {
        STEP_HIGH
    } else {
        STEP_LOW
    }) as i32;
    ((cur as i32 + if d < 0 { -step } else { step }) & 0x0fff) as u16
}

/// FBoxVisible stores a random point of the target's box as the aim point on
/// every think: a lateral offset of -16..=15 units from the box centre for the
/// think at `tick` of actor `pi`. A hash instead of a shared generator, so a
/// turret does not shift the random stream the rest of the game draws from.
#[inline(always)]
pub const fn box_offset(tick: u16, pi: u8) -> i32 {
    let mut h = (tick as u32).wrapping_mul(0x9E37_79B1) ^ (pi as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    ((h >> 16) & 31) as i32 - BOX_HALF_WIDTH
}

/// The yaw the barrel steers to: the bearing to the target's centre moved by
/// the lateral box offset seen from `dist` units away.
#[inline(always)]
pub const fn box_goal(want: u16, offset: i32, dist: i32) -> u16 {
    let dist = if dist < 64 { 64 } else { dist };
    ((want as i32 + offset * 652 / dist) & 0x0fff) as u16
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
    fn the_deploy_clips_are_sequence_three_of_the_models() {
        // (frames - 1) / fps seconds, rounded up to 0.1 s thinks of 2 ticks.
        assert_eq!(deploy_ticks(20), 8); // sentry: 7 / 20 = 0.35 s
        assert_eq!(deploy_ticks(22), 20); // miniturret: 30 / 30 = 1.0 s
        assert_eq!(deploy_ticks(21), 34); // turret: 50 / 30 = 1.67 s
    }

    #[test]
    fn a_cold_miniturret_waits_two_seconds_from_sight_to_first_hit() {
        // The oracle: 42 to 49 ticks from first sight to the first hit, of
        // which the first shot is 36 to 41 ticks after the sight.
        let shot = shot_ticks(22, SIGHT_EXTRA_TICKS) as i32;
        assert!((36..=41).contains(&shot), "{shot}");
        assert_eq!(shot_ticks(22, 0), 28);
        assert!(shot_ticks(20, 0) < shot_ticks(22, 0));
        assert!(shot_ticks(22, 0) < shot_ticks(21, 0));
    }

    #[test]
    fn the_barrel_moves_in_whole_steps_and_snaps_inside_one_and_a_half_degrees() {
        assert_eq!(swing(100, 100 + SNAP - 1, false), 100 + SNAP - 1);
        assert_eq!(swing(100, 100 + SNAP, false), 100 + STEP_LOW);
        assert_eq!(swing(100, 100 + SNAP, true), 100 + STEP_HIGH); // overshoots the goal
        assert_eq!(swing(100, 100 - 60, false), 100 - STEP_LOW);
        // Wraps the short way.
        assert_eq!(swing(4090, 20, true), 4090 + STEP_HIGH - 4096);
        // Outside 30 degrees it takes the ramped step.
        assert_eq!(swing(0, 1000, false), STEP_FAR);
        assert_eq!(swing(0, DEG30 + 5, true), STEP_FAR);
    }

    #[test]
    fn the_box_offset_covers_the_hull_and_is_deterministic() {
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        for t in 0..2000u16 {
            let o = box_offset(t, 3);
            assert_eq!(o, box_offset(t, 3));
            lo = lo.min(o);
            hi = hi.max(o);
        }
        assert_eq!((lo, hi), (-BOX_HALF_WIDTH, BOX_HALF_WIDTH - 1));
        assert_eq!(box_goal(1000, 0, 500), 1000);
        assert_eq!(box_goal(1000, 16, 652), 1016);
        assert_eq!(box_goal(1, -16, 652), 4081); // wraps
    }

    /// Plays 20 000 thinks against a standing player `dist` units away and
    /// returns how many shots hit.
    fn standing_hits(dist: i32) -> u32 {
        let mut cur = 300u16;
        let mut hits = 0;
        for think in 0..20_000u16 {
            let tick = think.wrapping_mul(2);
            if bullet_hits(yaw_error(cur, 0), dist) {
                hits += 1;
            }
            let goal = box_goal(0, box_offset(tick, 3), dist);
            cur = swing(cur, goal, think & 1 == 1);
        }
        hits
    }

    #[test]
    fn a_standing_player_is_hit_on_about_a_third_of_the_shots() {
        // Retail: one 5 HP hit per 5.6 ticks on average (five runs), that is
        // 0.36 hits per 2-tick think, at 500 to 600 units.
        for dist in [400, 560, 700] {
            let rate = standing_hits(dist) as i32; // per 20 000
            println!("{dist} units: {rate} hits in 20000 thinks");
            assert!((5_600..=9_000).contains(&rate), "{dist}: {rate}");
        }
    }

    #[test]
    fn a_strafing_target_outruns_the_barrel_up_close() {
        // The player strafes at 320 u/s = 16 units a tick at 300 units: the
        // bearing moves 16/300 rad = 0.053 rad = 35 yaw units a tick, 70 a
        // think, more than the 68 the barrel turns at its high rate.
        let dist = 300;
        let bearing_per_think = 2 * 16 * 652 / dist;
        assert!(bearing_per_think > STEP_HIGH as i32);
        let mut err = 0i32;
        for _ in 0..6 {
            err = (err + bearing_per_think - STEP_LOW as i32).max(0);
        }
        assert!(!bullet_hits(err as u16, dist));
        // At 900 units the same strafe is 23 yaw units a think and is tracked.
        assert!(2 * 16 * 652 / 900 < STEP_LOW as i32);
    }

    #[test]
    fn standing_targets_are_hit_when_the_barrel_is_on_them() {
        assert!(bullet_hits(0, 1200));
        assert!(bullet_hits(4, 1200));
        assert!(!bullet_hits(10, 1200));
        assert!(bullet_hits(100, 50));
        assert!(!bullet_hits(DEG30 + 1, 600));
        assert!(can_fire(DEG30) && !can_fire(DEG30 + 1));
    }
}
