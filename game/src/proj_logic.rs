//! Flight rules of the player's projectiles: the RPG rocket's ignition and
//! boost, the hive hand's tracking hornets, swept contact tests and the
//! bounce of a thrown charge.
//!
//! Pure integer arithmetic so the host runner can exercise it (`hl-logic-tests`
//! includes this file by path). Velocities are world units per 20 Hz tick
//! (retail units per second divided by 20); directions are q12 unit vectors.

/// A rocket's velocity is kept in sixteenths of a unit per tick (the same
/// representation the grunt's grenade uses) so its slow decay is not eaten by
/// whole-unit rounding.
///
/// Ticks a launched rocket coasts before its motor lights (0.4 s).
pub const ROCKET_IGNITE_TICKS: u32 = 8;
/// Ticks of motor boost after ignition (1.0 s).
pub const ROCKET_BOOST_TICKS: u32 = 20;
/// Launch speed (250 u/s).
pub const ROCKET_LAUNCH_SPEED_Q4: i32 = 200;
/// Boost per 0.1 s think (400 u/s) and the top speed (2000 u/s, 300 u/s in water).
pub const ROCKET_BOOST_STEP_Q4: i32 = 320;
pub const ROCKET_TOP_SPEED_Q4: i32 = 1600;
pub const ROCKET_TOP_SPEED_WATER_Q4: i32 = 240;
/// Below this speed (1500 u/s) a coasting rocket in the air detonates.
pub const ROCKET_STALL_SPEED_Q4: i32 = 1200;
/// Half of the per-tick gravity of the coasting rocket (gravity 0.5 of
/// 800 u/s^2 is one unit per tick squared), applied either side of the move.
pub const ROCKET_COAST_HALF_GRAVITY_Q4: i32 = 8;

#[optimize(size)]

fn isqrt64(v: i64) -> i64 {
    psx_math::int32::isqrt_i32(v.clamp(0, i32::MAX as i64) as i32) as i64
}

/// Length of a velocity.
#[optimize(size)]
pub fn speed(v: [i32; 3]) -> i32 {
    let s = v[0] as i64 * v[0] as i64 + v[1] as i64 * v[1] as i64 + v[2] as i64 * v[2] as i64;
    isqrt64(s) as i32
}

/// One 0.1 s think of a lit rocket: `v` is its velocity (q4), `dir` the unit
/// direction it steers to (the best laser spot, else the way it points),
/// `since_ignite` the ticks since ignition. Returns the new velocity and
/// whether the rocket has run out of speed and detonates.
///
/// During the first second the old velocity keeps a fifth of its weight and the
/// new heading takes the rest, at the old speed times 0.8 plus 400 u/s; after
/// that it only holds 0.798 of its speed and detonates in air below 1500 u/s.
#[inline(never)]
#[optimize(size)]
pub fn rocket_think(v: [i32; 3], dir: [i32; 3], since_ignite: u32, in_water: bool) -> ([i32; 3], bool) {
    let s = speed(v) as i64;
    let boost = since_ignite < ROCKET_BOOST_TICKS;
    let along = if boost {
        (s * 4 + 2) / 5 + ROCKET_BOOST_STEP_Q4 as i64
    } else {
        (s * 798 + 500) / 1000
    };
    let mut out = [0i32; 3];
    for a in 0..3 {
        // a fifth of the old velocity (819/4096) plus the new heading, rounded
        out[a] = ((v[a] as i64 * 819 + dir[a] as i64 * along + 2048) >> 12) as i32;
    }
    let cap = if in_water { ROCKET_TOP_SPEED_WATER_Q4 } else { ROCKET_TOP_SPEED_Q4 };
    let len = speed(out);
    if boost && len > cap {
        for a in 0..3 {
            out[a] = (out[a] as i64 * cap as i64 / len as i64) as i32;
        }
    }
    let stalled = !boost && !in_water && len < ROCKET_STALL_SPEED_Q4;
    (out, stalled)
}

/// Advance a position by a q4 velocity for one tick, carrying the sub-unit
/// remainder, with `half_gravity_q4` taken off the vertical speed either side
/// of the move. Returns the whole-unit step.
#[inline(never)]
#[optimize(size)]
pub fn integrate_q4(vel: &mut [i32; 3], rem: &mut [i8; 3], half_gravity_q4: i32) -> [i32; 3] {
    vel[1] -= half_gravity_q4;
    let mut step = [0i32; 3];
    for a in 0..3 {
        let total = vel[a] + rem[a] as i32;
        step[a] = total.div_euclid(16);
        rem[a] = total.rem_euclid(16) as i8;
    }
    vel[1] -= half_gravity_q4;
    step
}

/// The squared distance from `p` to the segment `a`..`b`.
#[cfg(test)]
pub fn segment_point_dist2(a: [i32; 3], b: [i32; 3], p: [i32; 3]) -> i64 {
    let ab = [(b[0] - a[0]) as i64, (b[1] - a[1]) as i64, (b[2] - a[2]) as i64];
    let ap = [(p[0] - a[0]) as i64, (p[1] - a[1]) as i64, (p[2] - a[2]) as i64];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t_num = (ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]).clamp(0, len2.max(0));
    let c = if len2 == 0 {
        [0, 0, 0]
    } else {
        [ab[0] * t_num / len2, ab[1] * t_num / len2, ab[2] * t_num / len2]
    };
    let d = [ap[0] - c[0], ap[1] - c[1], ap[2] - c[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// The point of the segment `a`..`b` nearest to `p`, and its position along
/// the segment (q12).
#[inline(never)]
pub fn segment_closest(a: [i32; 3], b: [i32; 3], p: [i32; 3]) -> ([i32; 3], i32) {
    let ab = [(b[0] - a[0]) as i64, (b[1] - a[1]) as i64, (b[2] - a[2]) as i64];
    let ap = [(p[0] - a[0]) as i64, (p[1] - a[1]) as i64, (p[2] - a[2]) as i64];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    if len2 == 0 {
        return (a, 0);
    }
    let t = (ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]).clamp(0, len2);
    let q12 = (t * 4096 / len2) as i32;
    (
        [
            a[0] + (ab[0] * t / len2) as i32,
            a[1] + (ab[1] * t / len2) as i32,
            a[2] + (ab[2] * t / len2) as i32,
        ],
        q12,
    )
}

/// Whether the actor cylinder (centre `c`, radius `r`, half-height `hh`) is
/// touched by a projectile of radius `pad` travelling `a`..`b`. Returns where
/// along the segment (q12) it first comes within reach.
#[inline(never)]
pub fn segment_hits_cylinder(a: [i32; 3], b: [i32; 3], c: [i32; 3], r: i32, hh: i32, pad: i32) -> Option<i32> {
    let (near, t) = segment_closest(a, b, c);
    let dx = (near[0] - c[0]) as i64;
    let dz = (near[2] - c[2]) as i64;
    let lim = (r + pad) as i64;
    if dx * dx + dz * dz <= lim * lim && ((near[1] - c[1]).abs()) <= hh + pad {
        Some(t)
    } else {
        None
    }
}

/// Velocity after a bounce off a surface with unit normal `n` (q12): the part
/// along the normal is reversed and scaled to `1 - keep_q8/256` of itself
/// (the retail clip with a backoff of 2 minus the entity's friction), the
/// sliding part is kept.
#[inline(never)]
pub fn bounce(v: [i32; 3], n: [i32; 3], friction_q8: i32) -> [i32; 3] {
    let dot = (v[0] as i64 * n[0] as i64 + v[1] as i64 * n[1] as i64 + v[2] as i64 * n[2] as i64) >> 12;
    // backoff = 2 - friction; the clip subtracts backoff * (v.n) * n
    let backoff_q8 = 2 * 256 - friction_q8 as i64;
    let mut out = [0i32; 3];
    for a in 0..3 {
        out[a] = (v[a] as i64 - ((n[a] as i64 * dot * backoff_q8) >> 20)) as i32;
    }
    out
}

/// Whether a bouncing charge that touched a floor-like surface (normal up
/// component above 0.7) comes to rest: its upward speed fell under 60 u/s.
pub fn comes_to_rest(n_up_q12: i32, vy_after: i32) -> bool {
    n_up_q12 > 2867 && vy_after < 3
}

/// Number of ticks a primary hornet flies before its motor kicks in; the
/// type decides the cruise speed (red 600 u/s, orange 800 u/s).
pub const HORNET_LAUNCH_SPEED: i32 = 15;
pub const HORNET_RED_SPEED: i32 = 30;
pub const HORNET_ORANGE_SPEED: i32 = 40;
/// Life of a tracking hornet (5 s) and of a dart (4 s), in ticks.
pub const HORNET_LIFE: u8 = 100;
pub const HORNET_DART_LIFE: u8 = 80;
/// A hornet sees a target within 512 units and a 25 degree half-angle (cos 0.9).
pub const HORNET_SIGHT: i32 = 512;
pub const HORNET_FOV_COS_Q12: i32 = 3686;

/// Steering of one hornet think: `flight` and `to_enemy` are unit directions
/// (q12) of the current flight and of the line to the target, `red` the hornet
/// type. Returns the new velocity. A red hornet slows in proportion to how
/// wide the turn is (never backwards); an orange one keeps its speed.
#[inline(never)]
pub fn hornet_think(flight: [i32; 3], to_enemy: [i32; 3], red: bool) -> [i32; 3] {
    let dot = ((flight[0] as i64 * to_enemy[0] as i64
        + flight[1] as i64 * to_enemy[1] as i64
        + flight[2] as i64 * to_enemy[2] as i64)
        >> 12) as i32;
    let sum = [
        flight[0] + to_enemy[0],
        flight[1] + to_enemy[1],
        flight[2] + to_enemy[2],
    ];
    let len = speed(sum).max(1) as i64;
    let cruise = if red { HORNET_RED_SPEED } else { HORNET_ORANGE_SPEED } as i64;
    let scale = if red {
        // a flat-out reversal still crawls at a quarter of the speed
        let d = if dot <= 0 { 1024 } else { dot } as i64;
        cruise * d / 4096
    } else {
        cruise
    };
    [
        (sum[0] as i64 * scale / len) as i32,
        (sum[1] as i64 * scale / len) as i32,
        (sum[2] as i64 * scale / len) as i32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rocket_picks_up_400_units_a_second_every_think_then_caps() {
        let mut v = [0, 0, ROCKET_LAUNCH_SPEED_Q4];
        let dir = [0, 0, 4096];
        let mut speeds = std::vec::Vec::new();
        for think in 0..10u32 {
            let (nv, stalled) = rocket_think(v, dir, think * 2, false);
            assert!(!stalled);
            v = nv;
            speeds.push(speed(v));
        }
        // 250 u/s -> 650 -> 1050 -> 1450 (the retail 400 u/s steps) -> cap 2000
        let ups: std::vec::Vec<i32> = speeds.iter().map(|q| q * 20 / 16).collect();
        assert_eq!(&ups[..4], &[650, 1050, 1450, 1850]);
        assert!(ups[4] <= 2000 && ups[4] >= 1990, "{ups:?}");
    }

    #[test]
    fn a_rocket_loses_speed_slowly_then_detonates_when_it_stalls() {
        let mut v = [0, 0, ROCKET_TOP_SPEED_Q4];
        let dir = [0, 0, 4096];
        let mut thinks = 0;
        loop {
            let (nv, stalled) = rocket_think(v, dir, 40, false);
            v = nv;
            thinks += 1;
            if stalled || thinks > 1000 {
                break;
            }
        }
        // 2000 u/s * 0.998^n falls under 1500 u/s after about 140 thinks (14 s)
        assert!((120..170).contains(&thinks), "thinks {thinks}");
        let (_, stalled_in_water) = rocket_think([0, 0, 100], dir, 40, true);
        assert!(!stalled_in_water);
    }

    #[test]
    fn q4_integration_carries_fractions_and_gravity() {
        let mut v = [0, 0, 200]; // 12.5 units per tick
        let mut r = [0i8; 3];
        let mut z = 0;
        for _ in 0..8 {
            z += integrate_q4(&mut v, &mut r, 0)[2];
        }
        assert_eq!(z, 100); // 8 ticks at 12.5
        // the coasting rocket falls 1 unit/tick^2: after 8 ticks about 32 units
        let mut v = [0, 0, 0];
        let mut r = [0i8; 3];
        let mut y = 0;
        for _ in 0..8 {
            y += integrate_q4(&mut v, &mut r, ROCKET_COAST_HALF_GRAVITY_Q4)[1];
        }
        assert!((-36..=-28).contains(&y), "{y}");
    }

    #[test]
    fn a_rocket_turns_most_of_the_way_to_the_spot_each_think() {
        let v = [0, 0, 800];
        let (nv, _) = rocket_think(v, [4096, 0, 0], 4, false);
        // 0.8 of the new heading, a fifth of the old one remains
        assert!(nv[0] > nv[2] * 3, "{nv:?}");
    }

    #[test]
    fn swept_distance_finds_a_target_between_two_samples() {
        let a = [0, 0, 0];
        let b = [0, 0, 100];
        assert_eq!(segment_point_dist2(a, b, [10, 0, 50]), 100);
        assert_eq!(segment_point_dist2(a, b, [0, 0, 150]), 50 * 50);
        assert_eq!(segment_point_dist2(a, b, [0, 0, -30]), 900);
        assert_eq!(segment_point_dist2(a, a, [3, 4, 0]), 25);
    }

    #[test]
    fn a_fast_projectile_cannot_skip_over_a_thin_target() {
        // 100 units a tick through a headcrab-sized cylinder
        let hit = segment_hits_cylinder([0, 0, 0], [0, 0, 100], [4, 2, 50], 12, 12, 2);
        assert_eq!(hit, Some(2048));
        assert!(segment_hits_cylinder([0, 0, 0], [0, 0, 100], [40, 0, 50], 12, 12, 2).is_none());
        assert!(segment_hits_cylinder([0, 0, 0], [0, 0, 100], [0, 0, 130], 12, 12, 2).is_none());
        assert!(segment_hits_cylinder([0, 0, 0], [0, 0, 100], [0, 0, 110], 12, 12, 2).is_some());
    }

    #[test]
    fn bounce_keeps_the_slide_and_gives_back_a_fifth_of_the_normal_speed() {
        // friction 0.8: backoff 1.2, so the normal part ends at -0.2 of itself
        let v = [10, -10, 0];
        let n = [0, 4096, 0];
        let out = bounce(v, n, 205);
        assert_eq!(out[0], 10);
        assert!(out[1] >= 1 && out[1] <= 3, "{out:?}");
        assert!(comes_to_rest(4096, out[1]));
        assert!(!comes_to_rest(1000, out[1]));
        // a wall: normal along -x, the satchel keeps falling
        let out = bounce([14, -3, 0], [-4096, 0, 0], 205);
        assert!(out[0] < 0 && out[0] >= -4 && out[1] == -3, "{out:?}");
    }

    #[test]
    fn red_hornets_slow_on_wide_turns_and_orange_ones_do_not() {
        let fwd = [0, 0, 4096];
        let straight = hornet_think(fwd, fwd, true);
        assert_eq!(speed(straight), HORNET_RED_SPEED);
        let turn = hornet_think(fwd, [4096, 0, 0], true);
        assert!(speed(turn) < HORNET_RED_SPEED);
        let back = hornet_think(fwd, [0, 0, -4096], true);
        assert!(speed(back) <= HORNET_RED_SPEED / 4 + 1);
        let orange = hornet_think(fwd, [4096, 0, 0], false);
        assert!((speed(orange) - HORNET_ORANGE_SPEED).abs() <= 1);
    }
}
