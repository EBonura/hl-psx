//! Blood trails: a hit on a bleeding creature leaves 1, 2 or 4 blood decals
//! on surfaces up to 172 units past the wound along the shot direction,
//! jittered about 0.1, 0.2 or 0.3 per axis for damage under 10, under 25 and
//! above; red or yellow by species; nothing for zero damage or creatures that
//! do not bleed.

use crate::world_rules::{blood_trail, species_blood, BloodColour, BloodTrailWorld, BLEED_REACH};

/// A scripted world: a small fixed LCG for the random draws, recording each
/// draw's bound and each traced ray.
struct Script {
    seed: u32,
    /// None: use the LCG. Some(f): draw f(n) for a bound of n.
    fixed: Option<fn(u32) -> u32>,
    bounds: Vec<u32>,
    rays: Vec<([i32; 3], [i32; 3])>,
    /// Interleaving of draws ('r') and traces ('t').
    order: String,
}

impl Script {
    fn lcg() -> Self {
        Self {
            seed: 7,
            fixed: None,
            bounds: vec![],
            rays: vec![],
            order: String::new(),
        }
    }
    fn fixed(f: fn(u32) -> u32) -> Self {
        Self {
            fixed: Some(f),
            ..Self::lcg()
        }
    }
}

impl BloodTrailWorld for Script {
    fn random_below(&mut self, n: u32) -> u32 {
        self.bounds.push(n);
        self.order.push('r');
        if let Some(f) = self.fixed {
            return f(n);
        }
        self.seed = self.seed.wrapping_mul(1103515245).wrapping_add(12345);
        (self.seed >> 8) % n
    }
    fn trace_and_stamp(&mut self, from: [i32; 3], to: [i32; 3]) {
        self.order.push('t');
        self.rays.push((from, to));
    }
}

/// Shot along +X from the origin, 1000 units long, wound at (500, 10, 0).
fn x_shot(damage: u8, w: &mut Script) {
    blood_trail(
        [500, 10, 0],
        [0, 0, 0],
        [1000, 0, 0],
        1000,
        damage,
        BloodColour::Red,
        w,
    );
}

fn middle(n: u32) -> u32 {
    n / 2
}
fn lowest(_: u32) -> u32 {
    0
}
fn highest(n: u32) -> u32 {
    n - 1
}

#[test]
fn decal_count_follows_the_damage_band() {
    for (damage, count) in [
        (1u8, 1),
        (9, 1),
        (10, 2),
        (24, 2),
        (25, 4),
        (100, 4),
        (255, 4),
    ] {
        let mut w = Script::lcg();
        x_shot(damage, &mut w);
        assert_eq!(w.rays.len(), count, "damage {damage}");
    }
}

#[test]
fn zero_damage_or_no_blood_leaves_nothing() {
    let mut w = Script::lcg();
    x_shot(0, &mut w);
    assert!(w.rays.is_empty() && w.bounds.is_empty());
    let mut w = Script::lcg();
    blood_trail(
        [0, 0, 0],
        [0, 0, 0],
        [100, 0, 0],
        100,
        50,
        BloodColour::NoBlood,
        &mut w,
    );
    assert!(w.rays.is_empty() && w.bounds.is_empty());
}

#[test]
fn yellow_blood_trails_like_red() {
    let mut red = Script::lcg();
    x_shot(30, &mut red);
    let mut yellow = Script::lcg();
    blood_trail(
        [500, 10, 0],
        [0, 0, 0],
        [1000, 0, 0],
        1000,
        30,
        BloodColour::Yellow,
        &mut yellow,
    );
    assert_eq!(red.rays, yellow.rays);
}

#[test]
fn rays_start_at_the_wound_and_reach_172_units_on() {
    assert_eq!(BLEED_REACH, 172);
    let mut w = Script::fixed(middle);
    x_shot(30, &mut w);
    for (from, to) in &w.rays {
        assert_eq!(*from, [500, 10, 0]);
        assert_eq!(*to, [500 + 172, 10, 0]);
    }
}

#[test]
fn jitter_is_a_tenth_fifth_or_three_tenths_per_axis() {
    // (damage, jitter per axis in units at the 172-unit reach)
    for (damage, spread) in [(5u8, 17), (15, 34), (40, 51)] {
        let mut lo = Script::fixed(lowest);
        x_shot(damage, &mut lo);
        let mut hi = Script::fixed(highest);
        x_shot(damage, &mut hi);
        let lo_end = lo.rays[0].1;
        let hi_end = hi.rays[0].1;
        for a in 0..3 {
            let base = [672, 10, 0][a];
            assert!(
                (lo_end[a] - (base - spread)).abs() <= 1,
                "damage {damage} axis {a} low {lo_end:?}"
            );
            assert!(
                (hi_end[a] - (base + spread)).abs() <= 1,
                "damage {damage} axis {a} high {hi_end:?}"
            );
        }
    }
}

/// Recorded from ac83da7 behaviour: the random bound per draw is twice the
/// per-axis jitter in 1/4096 units plus one, three draws per decal, each
/// decal's draws immediately followed by its trace.
#[test]
fn golden_draw_bounds_and_order() {
    for (damage, bound, order) in [
        (5u8, 821u32, "rrrt"),
        (15, 1639, "rrrtrrrt"),
        (40, 2459, "rrrtrrrtrrrtrrrt"),
    ] {
        let mut w = Script::lcg();
        x_shot(damage, &mut w);
        assert!(w.bounds.iter().all(|&b| b == bound), "damage {damage}");
        assert_eq!(w.order, order, "damage {damage}");
    }
}

/// Recorded from ac83da7 behaviour: ray ends for four shots, using the
/// scripted LCG above (seed 7) as the random source. Each shot runs from
/// `start` to `end` over `len` and the wound sits at half of `end` plus 10 up.
#[test]
fn golden_ray_ends() {
    type Case = (u8, [i32; 3], [i32; 3], i32, &'static [[i32; 3]]);
    let cases: [Case; 4] = [
        (5, [0, 0, 0], [1000, 0, 0], 1000, &[[689, 18, 13]]),
        (
            15,
            [0, 64, 0],
            [300, -100, 400],
            512,
            &[[280, -84, 337], [258, -117, 354]],
        ),
        (
            40,
            [100, 0, 100],
            [-200, 50, -600],
            700,
            &[
                [-207, 89, -475],
                [-165, 75, -445],
                [-221, 64, -492],
                [-172, 34, -434],
            ],
        ),
        (
            255,
            [0, 0, 0],
            [0, 0, -90],
            90,
            &[
                [-33, 52, -220],
                [9, 38, -190],
                [-48, 27, -237],
                [1, -4, -179],
            ],
        ),
    ];
    for (damage, start, end, len, expect) in cases {
        let wound = [end[0] / 2, end[1] / 2 + 10, end[2] / 2];
        let mut w = Script::lcg();
        blood_trail(wound, start, end, len, damage, BloodColour::Red, &mut w);
        let ends: Vec<[i32; 3]> = w.rays.iter().map(|r| r.1).collect();
        assert_eq!(ends, expect, "damage {damage}");
        assert!(w.rays.iter().all(|r| r.0 == wound));
    }
}

/// Recorded from ac83da7 behaviour: actor type ids with yellow blood and
/// with none; every other id bleeds red.
#[test]
fn golden_species_colours() {
    let yellow = [
        2u8, 5, 6, 7, 9, 10, 11, 14, 16, 17, 18, 19, 24, 50, 55, 58, 59,
    ];
    let none = [
        13u8, 15, 20, 21, 22, 23, 56, 57, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74,
        75,
    ];
    for t in 0..=255u8 {
        let expect = if yellow.contains(&t) {
            BloodColour::Yellow
        } else if none.contains(&t) {
            BloodColour::NoBlood
        } else {
            BloodColour::Red
        };
        assert_eq!(species_blood(t), expect, "actor type {t}");
    }
}
