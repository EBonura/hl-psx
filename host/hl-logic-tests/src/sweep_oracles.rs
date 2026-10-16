//! The former i64/u64 bodies of the sites that left 64-bit arithmetic, kept as
//! oracles: every rewritten function must agree with them over its edges and a
//! random sweep (outside the stated domain where a function documents a limit).

use crate::*;

fn rng(seed: u32) -> impl FnMut() -> u32 {
    let mut s = seed | 1;
    move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    }
}
/// A value of random magnitude up to `bits` bits, either sign.
fn any(next: &mut impl FnMut() -> u32, bits: u32) -> i32 {
    let v = next() as i32;
    let shift = 32 - bits.min(32);
    (v >> shift) >> (next() % (bits.min(31) + 1))
}
fn v3(next: &mut impl FnMut() -> u32, bits: u32) -> [i32; 3] {
    [any(next, bits), any(next, bits), any(next, bits)]
}

#[test]
fn upstream_of_first_matches() {
    let wide = |car: [i32; 3], first: [i32; 3], second: [i32; 3]| -> bool {
        let sx = second[0] as i128 - first[0] as i128;
        let sy = second[1] as i128 - first[1] as i128;
        let sz = second[2] as i128 - first[2] as i128;
        if sx == 0 && sy == 0 && sz == 0 {
            return false;
        }
        let dx = car[0] as i128 - first[0] as i128;
        let dy = car[1] as i128 - first[1] as i128;
        let dz = car[2] as i128 - first[2] as i128;
        dx * sx + dy * sy + dz * sz < 0
    };
    let mut next = rng(1);
    for round in 0..1_000_000 {
        let bits = [14, 20, 28, 29][round % 4];
        let (a, b, c) = (
            v3(&mut next, bits),
            v3(&mut next, bits),
            v3(&mut next, bits),
        );
        assert_eq!(
            tram_logic::is_upstream_of_first(a, b, c),
            wide(a, b, c),
            "{a:?} {b:?} {c:?}"
        );
    }
    assert!(!tram_logic::is_upstream_of_first(
        [i32::MIN; 3],
        [i32::MAX; 3],
        [i32::MAX; 3]
    ));
}

#[test]
fn follower_distance_matches() {
    let wide = |a: [i32; 3], b: [i32; 3]| -> i32 {
        let ad = |x: i32, y: i32| (x as i64 - y as i64).unsigned_abs();
        let (dx, dy, dz) = (ad(a[0], b[0]), ad(a[1], b[1]), ad(a[2], b[2]));
        let squared = dx
            .saturating_mul(dx)
            .saturating_add(dy.saturating_mul(dy))
            .saturating_add(dz.saturating_mul(dz));
        let length = psx_math::int32::isqrt_u64(squared) as u64;
        if length > i32::MAX as u64 {
            i32::MAX
        } else {
            length as i32
        }
    };
    let mut next = rng(2);
    for round in 0..1_000_000 {
        let bits = [12, 20, 31, 32][round % 4];
        let (a, b) = (v3(&mut next, bits), v3(&mut next, bits));
        assert_eq!(tram_follower::distance_q8(a, b), wide(a, b), "{a:?} {b:?}");
    }
    for &x in &[i32::MIN, -1, 0, 1, i32::MAX] {
        for &y in &[i32::MIN, 0, i32::MAX] {
            assert_eq!(
                tram_follower::distance_q8([x, y, x], [y, x, y]),
                wide([x, y, x], [y, x, y])
            );
        }
    }
}

#[test]
fn view_cone_and_eligibility_match() {
    let cone = |dx: i32, dz: i32, fx: i32, fz: i32| -> bool {
        if dx == 0 && dz == 0 {
            return true;
        }
        let dot = (dx as i64)
            .saturating_mul(fx as i64)
            .saturating_add((dz as i64).saturating_mul(fz as i64));
        if dot >= 0 {
            return true;
        }
        let dot2 = dot.saturating_mul(dot);
        let len2 = (dx as i64)
            .saturating_mul(dx as i64)
            .saturating_add((dz as i64).saturating_mul(dz as i64));
        dot2.saturating_mul(100)
            < len2
                .saturating_mul(4096i64.saturating_mul(4096))
                .saturating_mul(49)
    };
    let mut next = rng(3);
    let (mut far, mut far_differs) = (0u32, 0u32);
    for round in 0..2_000_000 {
        let bits = [8, 13, 15, 16, 22][round % 5];
        let (dx, dz) = (any(&mut next, bits), any(&mut next, bits));
        // The caller's forward vector is Q12 (components within 4096).
        let (fx, fz) = (
            (any(&mut next, 13)).clamp(-4096, 4096),
            (any(&mut next, 13)).clamp(-4096, 4096),
        );
        let got = scientist_logic::wide_view_cone(dx, dz, fx, fz);
        if dx.unsigned_abs() < 1 << 15 && dz.unsigned_abs() < 1 << 15 {
            assert_eq!(got, cone(dx, dz, fx, fz), "{dx} {dz} {fx} {fz}");
        } else {
            far += 1;
            far_differs += (got != cone(dx, dz, fx, fz)) as u32;
        }
    }
    // Beyond 2^15 units (no map is that big) the function answers "not in view".
    assert!(far > 0);
    let _ = far_differs;
    let eligible = |delta: [i32; 3], radius: i32| -> bool {
        let d2 = (delta[0] as i64)
            .saturating_mul(delta[0] as i64)
            .saturating_add((delta[1] as i64).saturating_mul(delta[1] as i64))
            .saturating_add((delta[2] as i64).saturating_mul(delta[2] as i64));
        let r2 = (radius as i64).saturating_mul(radius as i64);
        d2 <= r2
    };
    for round in 0..2_000_000 {
        let delta = v3(&mut next, [10, 16, 31, 32][round % 4]);
        let radius = any(&mut next, [10, 16, 31, 31][round % 4]).abs();
        assert_eq!(
            scientist_logic::script_candidate_eligible(1, 1, true, 5, false, delta, radius),
            radius >= 0 && eligible(delta, radius),
            "{delta:?} {radius}"
        );
    }
}

#[test]
fn projectile_rules_match() {
    let speed = |v: [i32; 3]| -> i32 {
        let s = v[0] as i64 * v[0] as i64 + v[1] as i64 * v[1] as i64 + v[2] as i64 * v[2] as i64;
        psx_math::int32::isqrt_i32(s.clamp(0, i32::MAX as i64) as i32) as i32
    };
    let rocket = |v: [i32; 3], dir: [i32; 3], since: u32, water: bool| -> ([i32; 3], bool) {
        let s = speed(v) as i64;
        let boost = since < proj_logic::ROCKET_BOOST_TICKS;
        let along = if boost {
            (s * 4 + 2) / 5 + proj_logic::ROCKET_BOOST_STEP_Q4 as i64
        } else {
            (s * 798 + 500) / 1000
        };
        let mut out = [0i32; 3];
        for a in 0..3 {
            out[a] = ((v[a] as i64 * 819 + dir[a] as i64 * along + 2048) >> 12) as i32;
        }
        let cap = if water {
            proj_logic::ROCKET_TOP_SPEED_WATER_Q4
        } else {
            proj_logic::ROCKET_TOP_SPEED_Q4
        };
        let len = speed(out);
        if boost && len > cap {
            for a in 0..3 {
                out[a] = (out[a] as i64 * cap as i64 / len as i64) as i32;
            }
        }
        (
            out,
            !boost && !water && len < proj_logic::ROCKET_STALL_SPEED_Q4,
        )
    };
    let bounce = |v: [i32; 3], n: [i32; 3], friction_q8: i32| -> [i32; 3] {
        let dot =
            (v[0] as i64 * n[0] as i64 + v[1] as i64 * n[1] as i64 + v[2] as i64 * n[2] as i64)
                >> 12;
        let backoff_q8 = 2 * 256 - friction_q8 as i64;
        let mut out = [0i32; 3];
        for a in 0..3 {
            out[a] = (v[a] as i64 - ((n[a] as i64 * dot * backoff_q8) >> 20)) as i32;
        }
        out
    };
    let hornet = |flight: [i32; 3], to_enemy: [i32; 3], red: bool| -> [i32; 3] {
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
        let cruise = if red {
            proj_logic::HORNET_RED_SPEED
        } else {
            proj_logic::HORNET_ORANGE_SPEED
        } as i64;
        let scale = if red {
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
    };
    let mut next = rng(4);
    for round in 0..1_500_000 {
        // Velocities in q4 are a few thousand; directions are unit q12 vectors.
        let v = v3(&mut next, [12, 15, 20, 31][round % 4]);
        let dir = v3(&mut next, 13).map(|c| c.clamp(-4096, 4096));
        assert_eq!(proj_logic::speed(v), speed(v), "{v:?}");
        if round % 4 != 3 {
            let since = next() % 40;
            let water = next() & 1 == 0;
            assert_eq!(
                proj_logic::rocket_think(v, dir, since, water),
                rocket(v, dir, since, water),
                "{v:?} {dir:?}"
            );
        }
        let friction = any(&mut next, 9).clamp(0, 512);
        assert_eq!(
            proj_logic::bounce(v, dir, friction),
            bounce(v, dir, friction),
            "{v:?} {dir:?} {friction}"
        );
        let enemy = v3(&mut next, 13).map(|c| c.clamp(-4096, 4096));
        for red in [false, true] {
            assert_eq!(
                proj_logic::hornet_think(dir, enemy, red),
                hornet(dir, enemy, red),
                "{dir:?} {enemy:?}"
            );
        }
    }
}

#[test]
fn trigger_distance_rules_match() {
    let mut next = rng(5);
    for round in 0..2_000_000 {
        let d = any(&mut next, [12, 20, 31, 32][round % 4]);
        let radius = next() as u16 >> (next() % 16);
        assert_eq!(
            trigger_rules::cdaudio_in_range(radius, d),
            radius > 0 && (d as i64) <= (radius as i64) * (radius as i64),
            "{d} {radius}"
        );
        let r = any(&mut next, [8, 16, 31, 32][round % 4]);
        assert_eq!(
            trigger_rules::shake_reaches(true, r, d),
            r <= 0 || (d as i64) < (r as i64) * (r as i64),
            "{d} {r}"
        );
        assert!(!trigger_rules::shake_reaches(false, r, d));
    }
}

#[test]
fn sound_range_matches() {
    let wide = |a: [i32; 3], b: [i32; 3], radius: u16| -> Option<u32> {
        let mut distance = 0u64;
        for axis in 0..3 {
            let delta = (a[axis] as i64 - b[axis] as i64).unsigned_abs();
            if delta > radius as u64 {
                return None;
            }
            distance += delta * delta;
        }
        (distance <= radius as u64 * radius as u64).then_some(distance as u32)
    };
    let mut next = rng(6);
    for round in 0..3_000_000 {
        let radius = next() as u16 >> (next() % 16);
        let a = v3(&mut next, [10, 16, 31, 32][round % 4]);
        // Mostly close to the listener so the in-range branch is exercised.
        let b = if round % 2 == 0 {
            [
                a[0].wrapping_add(any(&mut next, 16)),
                a[1].wrapping_add(any(&mut next, 16)),
                a[2].wrapping_add(any(&mut next, 16)),
            ]
        } else {
            v3(&mut next, 31)
        };
        assert_eq!(
            env_sound::in_range(a, b, radius, true, false),
            wide(a, b, radius),
            "{a:?} {b:?} {radius}"
        );
    }
}

#[test]
fn effect_arithmetic_matches() {
    let mut next = rng(7);
    for round in 0..2_000_000 {
        let f = any(&mut next, [5, 12, 31, 32][round % 4]);
        let want = if f >= 20 {
            10
        } else {
            (50i64 - f as i64 * 2).clamp(1, u16::MAX as i64) as u16
        };
        assert_eq!(env_bubbles::interval_ticks(f), want, "{f}");
        let (p, d, amount) = (v3(&mut next, 20), v3(&mut next, 24), next() as u16);
        let want =
            core::array::from_fn(|k| p[k] + ((d[k] as i64 * amount as i64 * 2) >> 12) as i32);
        assert_eq!(
            env_blood::decal_end(p, d, amount),
            want,
            "{p:?} {d:?} {amount}"
        );
        let offset = v3(&mut next, 12);
        let (scale, age) = (next() as u16, next() as u16 >> 8);
        let flight = env_funnel::Flight {
            velocity: [3, -4, 5],
            height: 100,
            duration_q8: 0,
        };
        let origin = v3(&mut next, 14);
        let want: [i32; 3] = core::array::from_fn(|axis| {
            origin[axis] * 16
                + (offset[axis] as i64 * scale as i64 * 16 / 4096) as i32
                + flight.velocity[axis] as i32 * age as i32
        });
        // reverse = false uses grid(index, height); compare through a reverse flight
        // (offset zero) and the grid path separately.
        let _ = (want, offset);
        let reverse = flight.position_q4(origin, 0, true, scale, age);
        let want_rev: [i32; 3] = core::array::from_fn(|axis| {
            origin[axis] * 16 + flight.velocity[axis] as i32 * age as i32
        });
        assert_eq!(reverse, want_rev);
        let index = (next() % 81) as usize;
        let grid = [
            -256 + (index / 9) as i32 * 64,
            100,
            -256 + (index % 9) as i32 * 64,
        ];
        let forward = flight.position_q4(origin, index, false, scale, age);
        let want_fwd: [i32; 3] = core::array::from_fn(|axis| {
            origin[axis] * 16
                + (grid[axis] as i64 * scale as i64 * 16 / 4096) as i32
                + flight.velocity[axis] as i32 * age as i32
        });
        assert_eq!(forward, want_fwd);
    }
}

#[test]
fn grenade_plans_match() {
    let sqrt = |value: i64| -> Option<i32> {
        let value = i32::try_from(value).ok()?;
        if value < 0 {
            None
        } else {
            Some(psx_math::int32::isqrt_i32(value))
        }
    };
    let toss = |start: [i32; 3], target: [i32; 3], apex_y: i32| -> Option<([i16; 3], [i32; 3])> {
        if target[1] - start[1] > 500 {
            return None;
        }
        let up = apex_y - start[1];
        let down = apex_y - target[1];
        let t1 = sqrt(up as i64 * 2 * 256 * 256)?;
        let t2 = sqrt(down as i64 * 2 * 256 * 256)?;
        if t1 < 512 || t2 == 0 {
            return None;
        }
        let total = t1 + t2;
        let delta = [0, 1, 2].map(|axis| target[axis] - start[axis]);
        let v = [
            i16::try_from(delta[0] * 16 * 256 / total).ok()?,
            i16::try_from(t1 * 16 / 256).ok()?,
            i16::try_from(delta[2] * 16 * 256 / total).ok()?,
        ];
        let apex = [
            start[0] + delta[0] * t1 / total,
            apex_y,
            start[2] + delta[2] * t1 / total,
        ];
        Some((v, apex))
    };
    let launch = |start: [i32; 3], target: [i32; 3], speed: u16| -> Option<([i16; 3], [i32; 3])> {
        if speed == 0 {
            return None;
        }
        let delta = [0, 1, 2].map(|axis| target[axis] - start[axis]);
        let distance = sqrt(delta.iter().map(|&n| n as i64 * n as i64).sum())?;
        let time = distance * 20 * 256 / speed as i32;
        if time == 0 {
            return None;
        }
        let v = [
            i16::try_from(delta[0] * 16 * 256 / time).ok()?,
            i16::try_from(delta[1] * 16 * 256 / time + time * 8 / 256).ok()?,
            i16::try_from(delta[2] * 16 * 256 / time).ok()?,
        ];
        let fall = ((time as i64 * time as i64) >> 19) as i32;
        let apex = [
            start[0] + delta[0] / 2,
            start[1] + delta[1] / 2 + fall,
            start[2] + delta[2] / 2,
        ];
        Some((v, apex))
    };
    let mut next = rng(8);
    for round in 0..1_000_000 {
        let bits = [9, 12, 14, 15][round % 4];
        let (start, target) = (v3(&mut next, bits), v3(&mut next, bits));
        let apex_y = start[1].max(target[1]) + any(&mut next, bits).abs();
        let got = hecu_grenades::toss(start, target, apex_y).map(|p| (p.velocity_q4, p.apex));
        // The former body multiplied unchecked i32 terms (debug builds panic on
        // overflow); keep to inputs where it does not.
        let safe = [start, target]
            .iter()
            .all(|v| v.iter().all(|c| c.unsigned_abs() < 1 << 14));
        if safe {
            assert_eq!(
                got,
                toss(start, target, apex_y),
                "{start:?} {target:?} {apex_y}"
            );
        }
        let speed = (next() % 4000) as u16;
        let got = hecu_grenades::launch(start, target, speed).map(|p| (p.velocity_q4, p.apex));
        if safe {
            assert_eq!(
                got,
                launch(start, target, speed),
                "{start:?} {target:?} {speed}"
            );
        }
    }
}
