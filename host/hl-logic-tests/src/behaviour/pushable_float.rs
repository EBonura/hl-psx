//! Floating pushables: a box with a buoyancy factor settles in water where
//! gravity (800) balances buoyancy times the submerged depth, eases there
//! without bouncing, and floats only while its bottom is wet.

use crate::world_rules::{pushable_float_tick, FloatBody, FloatTick};

/// Water fills everything at or below `surface` (world Y is up).
fn pool(surface: i32) -> impl FnMut([i32; 3]) -> bool {
    move |p: [i32; 3]| p[1] <= surface
}

fn body(y: i32, half: i32, buoyancy: i32) -> FloatBody {
    FloatBody {
        center: [0, y, 0],
        half_height: half,
        buoyancy,
    }
}

/// Release a free box (no floor) at centre height `start_y` above a pool
/// whose surface is Y = 0 and run 20 Hz ticks the way the game does:
/// buoyancy while the bottom is wet, otherwise the ordinary fall of 2 units
/// per tick per tick capped at 32. Returns the centre Y after each tick.
fn simulate(start_y: i32, half: i32, buoyancy: i32, ticks: usize) -> Vec<i32> {
    let mut y = start_y;
    let mut vy = 0i32;
    let mut wet = pool(0);
    let mut out = Vec::with_capacity(ticks);
    for _ in 0..ticks {
        vy = match pushable_float_tick(body(y, half, buoyancy), vy, false, &mut wet) {
            Some(t) => t.vy,
            None => (vy - 2).max(-32),
        };
        y += vy;
        out.push(y);
    }
    out
}

/// Number of times the direction of travel flips along a trajectory.
fn direction_changes(path: &[i32]) -> usize {
    let mut last = 0i32;
    let mut flips = 0;
    for w in path.windows(2) {
        let d = (w[1] - w[0]).signum();
        if d != 0 {
            if last != 0 && d != last {
                flips += 1;
            }
            last = d;
        }
    }
    flips
}

#[test]
fn a_box_without_buoyancy_never_floats() {
    let r = pushable_float_tick(body(-50, 16, 0), -4, false, &mut pool(0));
    assert_eq!(r, None);
}

#[test]
fn a_box_with_a_dry_bottom_does_not_float() {
    // Bottom exactly on the surface: the sample just above the bottom is dry.
    assert_eq!(
        pushable_float_tick(body(16, 16, 80), 0, false, &mut pool(0)),
        None
    );
    // One unit lower and the bottom is wet.
    assert!(pushable_float_tick(body(15, 16, 80), 0, false, &mut pool(0)).is_some());
    // Top wet or not does not matter, only the bottom.
    assert_eq!(
        pushable_float_tick(body(100, 16, 80), 0, false, &mut pool(0)),
        None
    );
}

#[test]
fn a_gently_placed_box_settles_at_the_balance_depth() {
    // Balance depth is 800 / buoyancy whenever that fits inside the box.
    for &half in &[16, 32, 64] {
        for &b in &[40, 80, 100, 200] {
            let balance = 800 / b;
            if balance > 2 * half - 4 {
                continue;
            }
            let path = simulate(half + 1, half, b, 300);
            let y = *path.last().unwrap();
            let depth = -(y - half);
            let tolerance = 3 + (2 * half) / 32 + 1;
            assert!(
                (depth - balance).abs() <= tolerance,
                "half {half} buoyancy {b}: depth {depth} vs balance {balance}"
            );
            // At rest: the last ticks do not move.
            assert!(path[250..].iter().all(|&p| p == y));
        }
    }
}

#[test]
fn a_box_too_heavy_for_its_volume_sinks() {
    // Balance depth 80 is deeper than a 32-unit box, so it never stops.
    let path = simulate(17, 16, 10, 200);
    assert!(path.windows(2).all(|w| w[1] <= w[0]));
    assert!(path[199] < -1000);
}

#[test]
fn floating_eases_in_without_bouncing() {
    for &half in &[8, 16, 32] {
        for &b in &[40, 80, 100, 200] {
            for &start in &[half + 1, half + 64, -200] {
                let path = simulate(start, half, b, 300);
                assert!(
                    direction_changes(&path) <= 1,
                    "half {half} buoyancy {b} start {start}: {path:?}"
                );
            }
        }
    }
}

#[test]
fn grounded_box_stays_down_unless_lift_beats_gravity() {
    // Lift below gravity: still grounded and motionless.
    assert_eq!(
        pushable_float_tick(body(0, 16, 80), 0, true, &mut pool(0)),
        Some(FloatTick {
            vy: 0,
            grounded: true
        })
    );
    // A stored downward speed is ignored while grounded.
    assert_eq!(
        pushable_float_tick(body(0, 16, 80), -20, true, &mut pool(0)),
        Some(FloatTick {
            vy: 0,
            grounded: true
        })
    );
    // Lift beats gravity: the box leaves its floor and rises.
    let r = pushable_float_tick(body(-10, 16, 200), 0, true, &mut pool(0)).unwrap();
    assert!(!r.grounded && r.vy > 0);
}

/// Recorded from ac83da7 behaviour: final centre height after 300 ticks for
/// a box of the given half height and buoyancy, released (a) with its bottom
/// one unit under the surface, (b) from 64 units above that, (c) at rest
/// deep underwater at Y = -200. Surface at Y = 0.
#[test]
fn golden_settle_heights() {
    let table: &[(i32, i32, [i32; 3])] = &[
        (8, 40, [-300, -307, -200]),
        (8, 80, [1, -12, -200]),
        (8, 100, [3, -3, -3]),
        (8, 200, [6, 1, 1]),
        (16, 40, [-1, -8, -7]),
        (16, 80, [9, 3, 3]),
        (16, 100, [11, 5, 5]),
        (16, 200, [14, 9, 9]),
        (32, 40, [16, 8, 9]),
        (32, 80, [26, 19, 20]),
        (32, 100, [28, 21, 21]),
        (32, 200, [30, 25, 25]),
    ];
    for &(half, b, expect) in table {
        let starts = [half + 1, half + 64, -200];
        for (i, &s) in starts.iter().enumerate() {
            assert_eq!(
                simulate(s, half, b, 300)[299],
                expect[i],
                "half {half} buoyancy {b} start {s}"
            );
        }
    }
}

/// Recorded from ac83da7 behaviour: centre height per tick for a 32-unit box
/// with buoyancy 80 dropped from 64 units above the floating position. It
/// falls, dips under its rest height once, and eases back up.
#[test]
fn golden_drop_trajectory() {
    let expect = [
        78, 74, 68, 60, 50, 38, 24, 8, 0, -3, -3, -2, -1, 0, 1, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
        3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    ];
    assert_eq!(simulate(16 + 64, 16, 80, 40), expect);
}

/// Recorded from ac83da7 behaviour: single ticks with a 32-unit box in a pool
/// whose surface is Y = 0; (centre Y, buoyancy, stored speed, grounded).
#[test]
fn golden_single_ticks() {
    let cases: &[((i32, i32, i32, bool), Option<FloatTick>)] = &[
        (
            (0, 80, 0, true),
            Some(FloatTick {
                vy: 0,
                grounded: true,
            }),
        ),
        ((20, 80, 0, true), None),
        (
            (0, 40, 0, true),
            Some(FloatTick {
                vy: 0,
                grounded: true,
            }),
        ),
        (
            (-10, 200, 0, true),
            Some(FloatTick {
                vy: 2,
                grounded: false,
            }),
        ),
        (
            (0, 80, -6, false),
            Some(FloatTick {
                vy: -2,
                grounded: false,
            }),
        ),
        (
            (0, 80, 6, false),
            Some(FloatTick {
                vy: 3,
                grounded: false,
            }),
        ),
        ((16, 80, 0, false), None),
        (
            (15, 80, 0, false),
            Some(FloatTick {
                vy: -1,
                grounded: false,
            }),
        ),
    ];
    for &((y, b, vy, g), expect) in cases {
        assert_eq!(
            pushable_float_tick(body(y, 16, b), vy, g, &mut pool(0)),
            expect,
            "y {y} buoyancy {b} vy {vy} grounded {g}"
        );
    }
}
