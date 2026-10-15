//! Throw velocity and bounce rules for the lobbed weapons (hand grenade,
//! satchel, MP5 grenade). Pure integer arithmetic over units per tick, so the
//! host logic tests run the exact code the console runs.
//!
//! Reference behaviour, written down rather than copied:
//! * The hand grenade leaves the hand at `(90 + elevation) * 6.5` units/s,
//!   capped at 1000, along a direction pitched up by `10 + 0.889 * look-up`
//!   degrees (looking up) or `10 + 1.111 * look-down` (looking down), and the
//!   thrower's own velocity is added on top.
//! * Grenades and satchels are `MOVETYPE_BOUNCE` with gravity 0.5 and friction
//!   0.8. The engine's toss physics bounces a contact with backoff
//!   `2 - friction` = 1.2, so the part of the velocity along the surface normal
//!   comes back at 0.2 of its size and the sliding part is untouched. A floor
//!   contact that leaves under 30 units/s overall, or that is rolling on the
//!   ground, stops the body and clears its vertical velocity.
//! * A grenade already on the ground loses 20 % of its speed on every touch
//!   (`BounceTouch`); a satchel loses 5 % (`SatchelSlide`). Those fire once per
//!   server frame, so at the 20 Hz tick they are applied for the several frames
//!   a tick spans.

use psx_math::int32::isqrt_i32;
use psx_math::{atan2_q12, sin_q12, cos_q12};

/// 400 units/s^2 (gravity 0.5 of 800) at 20 ticks/s is one unit per tick^2.
pub const LOB_GRAVITY: i32 = 1;
/// Gravity 1.0 of 800 u/s^2, the satchel once it has touched anything.
pub const LOB_GRAVITY_FULL: i32 = 2;
/// 10 degrees in 4096-per-turn angle units.
const TEN_DEG: i32 = 114;
/// 1000 units/s in 1/16 unit per tick.
const MAX_SPEED_16: i32 = 800;
/// Floor normals steeper than 0.7 count as ground (Q12).
const GROUND_NORMAL_Y: i32 = 2867;
/// A body stops when it leaves a floor contact slower than 30 units/s; that is
/// 1.5 units/tick, 2.25 squared, and the integer velocity needs under 3.
const REST_SPEED_SQ: i32 = 3;
/// Vertical speed under one frame of gravity (40 units/s at 20 Hz): rolling.
const ROLL_VY: i32 = 2;

/// Friction per tick for a body that is already rolling: 0.8 per 1/72 s frame
/// over the 3.6 frames of a tick is 0.45, in 64ths.
pub const GRENADE_ROLL_64: i32 = 29;
/// 0.95 per frame over a tick, in 64ths.
pub const SATCHEL_ROLL_64: i32 = 53;

/// Launch velocity in units per tick for a hand grenade thrown along the
/// q12 view direction `fwd` (world y up) by a player at `player_vel`.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub fn hand_grenade_velocity(fwd: [i32; 3], yaw: u16, player_vel: [i32; 3]) -> [i32; 3] {
    let hlen = isqrt_i32(fwd[0] * fwd[0] + fwd[2] * fwd[2]);
    // Signed look angle, up positive.
    let mut look = atan2_q12(fwd[1], hlen) as i32;
    if look > 2048 {
        look -= 4096;
    }
    let throw = TEN_DEG
        + if look >= 0 {
            look * 8 / 9
        } else {
            look * 10 / 9
        };
    // (90 + degrees) * 6.5 u/s at 20 Hz in 1/16 units: 468 + 0.457 per angle unit.
    let speed16 = (468 + ((throw * 117) >> 8)).clamp(0, MAX_SPEED_16);
    let (hx, hz) = if hlen > 0 {
        ((fwd[0] << 12) / hlen, (fwd[2] << 12) / hlen)
    } else {
        (sin_q12(yaw), cos_q12(yaw))
    };
    let a = (throw & 0xFFF) as u16;
    let flat16 = (speed16 * cos_q12(a)) >> 12;
    let up16 = (speed16 * sin_q12(a)) >> 12;
    [
        player_vel[0] + ((((hx * flat16) >> 12) + 8) >> 4),
        player_vel[1] + ((up16 + 8) >> 4),
        player_vel[2] + ((((hz * flat16) >> 12) + 8) >> 4),
    ]
}

/// Satchel launch: 274 units/s along the view plus the thrower's velocity.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub fn satchel_velocity(fwd: [i32; 3], player_vel: [i32; 3]) -> [i32; 3] {
    // 274 u/s is 13.7 units/tick, 56,115 in Q12 per unit of fwd.
    let k = 56_115;
    [
        player_vel[0] + ((fwd[0] * k + (1 << 23)) >> 24),
        player_vel[1] + ((fwd[1] * k + (1 << 23)) >> 24),
        player_vel[2] + ((fwd[2] * k + (1 << 23)) >> 24),
    ]
}

/// Ticks after launch at which a rocket's motor lights (0.4 s) and the
/// first second of boost that follows ends.
pub const ROCKET_IGNITE_AGE: u8 = 8;
pub const ROCKET_BOOST_END_AGE: u8 = 28;

/// One 0.1 s `CRpgRocket::FollowThink`: the new velocity (units/tick) from the
/// old one, the unit heading `dir` (q12) and the rocket's age in ticks. A fifth
/// of the old velocity stays; for the first second after ignition the heading
/// gets 0.8 x speed + 400 u/s (20 units/tick), capped at 2000 u/s, afterwards
/// 0.798 x speed, and below 1500 u/s in the air the rocket is spent (`true`).
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub fn rocket_think(vel: [i32; 3], dir: [i32; 3], age: u8) -> ([i32; 3], bool) {
    let s = isqrt_i32(vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]);
    let boost = age < ROCKET_BOOST_END_AGE;
    let along = if boost { (s * 4 + 2) / 5 + 20 } else { (s * 798 + 500) / 1000 };
    let mut v = [0i32; 3];
    let mut a = 0;
    while a < 3 {
        v[a] = (vel[a] * 819 + dir[a] * along + 2048) >> 12;
        a += 1;
    }
    let len = isqrt_i32(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    if boost && len > 100 {
        return ([v[0] * 100 / len, v[1] * 100 / len, v[2] * 100 / len], false);
    }
    (v, !boost && len < 75)
}

/// What a bounce leaves behind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bounce {
    pub vel: [i32; 3],
    /// The body has stopped on the floor.
    pub rest: bool,
    /// The contact was ground (a floor facing up).
    pub ground: bool,
}

/// Clip `vel` against a contact with the unit q12 `normal`. `rolling` is true
/// when the body arrived already resting on the floor (its vertical velocity
/// was zero before gravity), `roll_64` the per-tick friction in 64ths that
/// then applies. A contact with no known normal (an actor) reflects the
/// velocity at the same 0.2 and keeps it flying.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub fn bounce(vel: [i32; 3], normal: [i32; 3], rolling: bool, roll_64: i32) -> Bounce {
    let mut v = vel;
    let ground = normal[1] > GROUND_NORMAL_Y;
    if rolling && ground {
        v = [(v[0] * roll_64) >> 6, (v[1] * roll_64) >> 6, (v[2] * roll_64) >> 6];
    }
    if normal == [0; 3] {
        return Bounce {
            vel: [-v[0] / 5, -v[1] / 5, -v[2] / 5],
            rest: false,
            ground: false,
        };
    }
    let along = (v[0] * normal[0] + v[1] * normal[1] + v[2] * normal[2]) >> 12;
    // backoff 1.2: remove 1.2 x the normal component.
    let cut = along * 6 / 5;
    let mut out = [
        v[0] - ((normal[0] * cut) >> 12),
        v[1] - ((normal[1] * cut) >> 12),
        v[2] - ((normal[2] * cut) >> 12),
    ];
    let mut rest = false;
    if ground {
        let speed_sq = out[0] * out[0] + out[1] * out[1] + out[2] * out[2];
        if out[1] < ROLL_VY {
            out[1] = 0;
        }
        if speed_sq < REST_SPEED_SQ {
            out = [0; 3];
            rest = true;
        }
    }
    Bounce {
        vel: out,
        rest,
        ground,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP: [i32; 3] = [0, 4096, 0];

    fn fwd_at(look_q12: i32) -> [i32; 3] {
        // View direction along +z pitched up by `look_q12` angle units.
        [0, sin_q12(look_q12 as u16), cos_q12(look_q12 as u16)]
    }

    /// Drop a body from `start` under `LOB_GRAVITY` the way tick_projectiles
    /// steps it (gravity first, then move) until it falls below y = 0.
    fn landing(start: [i32; 3], mut vel: [i32; 3]) -> ([i32; 3], i32) {
        let mut pos = start;
        let mut ticks = 0;
        while pos[1] > 0 && ticks < 400 {
            vel[1] -= LOB_GRAVITY;
            pos = [pos[0] + vel[0], pos[1] + vel[1], pos[2] + vel[2]];
            ticks += 1;
        }
        (pos, ticks)
    }

    #[test]
    fn level_throw_is_650_units_per_second_ten_degrees_up() {
        let v = hand_grenade_velocity(fwd_at(0), 0, [0; 3]);
        let speed_sq = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        // 650 u/s is 32.5 units/tick: 1056 squared, within a unit of rounding.
        assert!((speed_sq - 1056).abs() < 70, "speed^2 {speed_sq}");
        assert_eq!(v[0], 0);
        // 10 degrees up of 32.5 is 5.6.
        assert!(v[1] >= 5 && v[1] <= 6, "vy {}", v[1]);
    }

    #[test]
    fn level_throw_from_eye_height_lands_500_to_650_units_out() {
        let v = hand_grenade_velocity(fwd_at(0), 0, [0; 3]);
        let (pos, ticks) = landing([0, 64, 0], v);
        assert!(pos[2] >= 500 && pos[2] <= 650, "landed at {}", pos[2]);
        assert!(ticks > 15 && ticks < 22, "flight {ticks}");
    }

    #[test]
    fn looking_up_throws_harder_and_farther_capped_at_1000() {
        let level = hand_grenade_velocity(fwd_at(0), 0, [0; 3]);
        let up45 = hand_grenade_velocity(fwd_at(512), 0, [0; 3]);
        let straight_up = hand_grenade_velocity(UP, 0, [0; 3]);
        let sq = |v: [i32; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        assert!(sq(up45) > sq(level));
        // 1000 u/s is 50 units/tick, straight up.
        assert_eq!(straight_up[0], 0);
        assert_eq!(straight_up[2], 0);
        assert_eq!(straight_up[1], 50);
        let (far, _) = landing([0, 64, 0], up45);
        let (near, _) = landing([0, 64, 0], level);
        assert!(far[2] > near[2]);
    }

    #[test]
    fn looking_straight_down_drops_it() {
        let v = hand_grenade_velocity([0, -4096, 0], 0, [0; 3]);
        assert_eq!(v, [0; 3]);
    }

    #[test]
    fn thrower_velocity_is_added() {
        let v = hand_grenade_velocity(fwd_at(0), 0, [10, 0, 3]);
        let still = hand_grenade_velocity(fwd_at(0), 0, [0; 3]);
        assert_eq!(v[0], still[0] + 10);
        assert_eq!(v[2], still[2] + 3);
    }

    #[test]
    fn satchel_leaves_at_274_units_per_second() {
        let v = satchel_velocity([0, 0, 4096], [0; 3]);
        assert_eq!(v, [0, 0, 14]); // 13.7 units/tick
    }

    #[test]
    fn floor_bounce_keeps_the_slide_and_returns_a_fifth_of_the_drop() {
        let b = bounce([32, -14, 0], UP, false, GRENADE_ROLL_64);
        assert_eq!(b.vel[0], 32);
        assert_eq!(b.vel[2], 0);
        assert_eq!(b.vel[1], 2); // 0.2 of 14, truncated
        assert!(b.ground && !b.rest);
    }

    #[test]
    fn wall_bounce_reverses_a_fifth_of_the_normal_part() {
        let wall = [-4096, 0, 0];
        let b = bounce([30, -3, 10], wall, false, GRENADE_ROLL_64);
        assert_eq!(b.vel[0], -6);
        assert_eq!(b.vel[1], -3);
        assert_eq!(b.vel[2], 10);
        assert!(!b.ground && !b.rest);
    }

    #[test]
    fn a_slow_floor_contact_stops_the_body() {
        let b = bounce([1, -1, 0], UP, false, GRENADE_ROLL_64);
        assert!(b.rest);
        assert_eq!(b.vel, [0; 3]);
    }

    #[test]
    fn a_rolling_grenade_loses_speed_every_tick_and_stops() {
        let mut v = [20, 0, 0];
        let mut dist = 0;
        let mut ticks = 0;
        while v != [0; 3] && ticks < 50 {
            // gravity pulls it into the floor, the floor answers.
            v[1] -= LOB_GRAVITY;
            let b = bounce(v, UP, true, GRENADE_ROLL_64);
            v = b.vel;
            dist += v[0];
            ticks += 1;
        }
        assert_eq!(v, [0; 3], "never stopped");
        assert!(dist < 30, "rolled {dist}");
    }

    #[test]
    fn a_satchel_keeps_sliding_longer_than_a_grenade() {
        let roll = |scale| {
            let mut v = [20, 0, 0];
            let mut dist = 0;
            let mut ticks = 0;
            while v != [0; 3] && ticks < 80 {
                v[1] -= LOB_GRAVITY_FULL;
                let b = bounce(v, UP, true, scale);
                v = b.vel;
                dist += v[0];
                ticks += 1;
            }
            dist
        };
        assert!(roll(SATCHEL_ROLL_64) > roll(GRENADE_ROLL_64));
    }

    /// The HECU grenade (species.rs throw_grenade) aims a lob to land on a
    /// point `FLIGHT` ticks after release by solving for the launch velocity
    /// under the same gravity this module defines.
    #[test]
    fn the_grunt_lob_formula_lands_on_target_under_lob_gravity() {
        const FLIGHT: i32 = 10;
        let from = [0, 40, 0];
        for aim in [[300, 0, 0], [0, -20, 600], [-450, 10, 250]] {
            let vel = [
                (aim[0] - from[0]) / FLIGHT,
                (aim[1] - from[1] + LOB_GRAVITY * FLIGHT * (FLIGHT + 1) / 2) / FLIGHT,
                (aim[2] - from[2]) / FLIGHT,
            ];
            let mut v = vel;
            let mut pos = from;
            for _ in 0..FLIGHT {
                v[1] -= LOB_GRAVITY;
                pos = [pos[0] + v[0], pos[1] + v[1], pos[2] + v[2]];
            }
            for axis in 0..3 {
                assert!((pos[axis] - aim[axis]).abs() <= FLIGHT, "axis {axis}: {pos:?} vs {aim:?}");
            }
        }
    }


    #[test]
    fn a_rocket_gains_400_units_a_second_per_think_then_caps_at_2000() {
        let dir = [0, 0, 4096];
        let mut v = [0, 0, 12];
        let mut speeds = Vec::new();
        let mut age = ROCKET_IGNITE_AGE + 2;
        while age < ROCKET_BOOST_END_AGE {
            v = rocket_think(v, dir, age).0;
            speeds.push(v[2]);
            age += 2;
        }
        // 250 u/s -> about 650 -> 1050 -> 1450 -> 1850 -> 2000
        assert!((30..=34).contains(&speeds[0]), "{speeds:?}");
        assert!((50..=54).contains(&speeds[1]), "{speeds:?}");
        assert_eq!(*speeds.last().unwrap(), 100);
    }

    #[test]
    fn a_coasting_rocket_holds_its_speed_then_goes_off_below_1500() {
        let dir = [0, 0, 4096];
        // At full speed it loses almost nothing per think (0.998), so it stays
        // above 1500 u/s for the rest of its life ...
        let mut v = [0, 0, 100];
        for _ in 0..120 {
            let (nv, spent) = rocket_think(v, dir, 40);
            assert!(!spent);
            v = nv;
        }
        assert!(v[2] >= 75);
        // ... and one already under 1500 u/s (75 units/tick) is spent.
        assert!(rocket_think([0, 0, 70], dir, 40).1);
    }

    #[test]
    fn a_rocket_turns_toward_a_new_heading() {
        let (v, _) = rocket_think([0, 0, 40], [4096, 0, 0], 14);
        assert!(v[0] > v[2] * 3, "{v:?}");
    }

    #[test]
    fn an_actor_contact_throws_it_back_weakly() {
        let b = bounce([30, 0, 10], [0; 3], false, GRENADE_ROLL_64);
        assert_eq!(b.vel, [-6, 0, -2]);
        assert!(!b.rest);
    }
}
