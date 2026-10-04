//! Geiger counter: within 800 units of an active radiation volume (the
//! nearest volume centre, sampled every 0.25 s) it clicks with a probability
//! that rises as the player gets closer, rare at the edge and almost every
//! sample inside about 50 units; clicks get louder below 400 and 150 units.

use crate::player_rules::{GeigerScan, GEIGER_RANGE, GEIGER_SAMPLE_TICKS};

/// Clicks out of every pair of 7-bit random draws (128 x 128 = 16384) for a
/// player at the origin and one source `range` units away.
fn clicks_per_16384(range: i32) -> u32 {
    let mut n = 0;
    for a in 0..128u32 {
        for b in 0..128u32 {
            let mut s = GeigerScan::new();
            s.add_source([0, 0, 0], [range, 0, 0]);
            let mut seq = [a, b].into_iter();
            if s.sample(&mut || seq.next().unwrap_or(127)).is_some() {
                n += 1;
            }
        }
    }
    n
}

fn always_click_volume(range: i32) -> Option<u16> {
    let mut s = GeigerScan::new();
    s.add_source([0, 0, 0], [0, 0, range]);
    s.sample(&mut || 0).map(|c| c.volume_den)
}

#[test]
fn samples_every_quarter_second_within_800_units() {
    assert_eq!(GEIGER_RANGE, 800);
    // 20 Hz simulation ticks.
    assert_eq!(GEIGER_SAMPLE_TICKS, 5);
}

#[test]
fn never_clicks_beyond_800_units() {
    assert_eq!(clicks_per_16384(801), 0);
    assert_eq!(clicks_per_16384(5000), 0);
    assert!(clicks_per_16384(800) > 0);
    // Diagonal distance counts, not a single axis.
    let mut s = GeigerScan::new();
    s.add_source([0, 0, 0], [600, 0, 600]);
    assert_eq!(s.sample(&mut || 0), None);
}

#[test]
fn never_clicks_without_a_source() {
    assert_eq!(GeigerScan::new().sample(&mut || 0), None);
}

#[test]
fn clicks_get_more_likely_closer_in() {
    let mut last = 0;
    let mut range = 820;
    while range >= 0 {
        let r = clicks_per_16384(range);
        assert!(
            r >= last,
            "rate fell from {last} to {r} moving in to {range}"
        );
        last = r;
        range -= 7;
    }
    // Rare at the edge, nearly always close in.
    assert!(clicks_per_16384(800) * 20 < 16384);
    assert!(clicks_per_16384(50) * 10 > 16384 * 9);
    assert!(clicks_per_16384(0) * 10 > 16384 * 9);
}

#[test]
fn the_nearest_source_decides() {
    let mut s = GeigerScan::new();
    s.add_source([0, 0, 0], [700, 0, 0]);
    s.add_source([0, 0, 0], [0, 0, -100]);
    s.add_source([0, 0, 0], [0, 2000, 0]);
    assert_eq!(s.sample(&mut || 0).map(|c| c.volume_den), Some(1));
    // Player position matters, not the origin.
    let mut s = GeigerScan::new();
    s.add_source([1000, 50, 1000], [1000, 50, 1500]);
    assert_eq!(s.sample(&mut || 0).map(|c| c.volume_den), Some(3));
}

#[test]
fn clicks_get_louder_below_400_and_150_units() {
    // Volume is 1 / volume_den of full.
    assert_eq!(always_click_volume(800), Some(3));
    assert_eq!(always_click_volume(401), Some(3));
    assert_eq!(always_click_volume(400), Some(2));
    assert_eq!(always_click_volume(151), Some(2));
    assert_eq!(always_click_volume(150), Some(1));
    assert_eq!(always_click_volume(0), Some(1));
}

/// A sample uses one random draw when something is in range and none when
/// nothing is.
#[test]
fn a_sample_uses_at_most_one_random_draw() {
    let draws = |range: i32| {
        let mut n = 0;
        let mut s = GeigerScan::new();
        s.add_source([0, 0, 0], [range, 0, 0]);
        let _ = s.sample(&mut || {
            n += 1;
            0
        });
        n
    };
    assert_eq!(draws(10), 1);
    assert_eq!(draws(800), 1);
    assert_eq!(draws(900), 0);
    assert_eq!(GeigerScan::new().sample(&mut || unreachable!()), None);
}

#[test]
fn far_away_volumes_cannot_overflow_the_distance() {
    let mut s = GeigerScan::new();
    s.add_source([-2_000_000, 0, 0], [2_000_000, 0, 0]);
    assert_eq!(s.sample(&mut || 0), None);
}

/// Recorded from the rewritten curve: clicks per 16384 equally likely draw
/// pairs at a spread of ranges.
#[test]
fn click_rate_per_distance() {
    let expect: [(i32, u32); 9] = [
        (0, 16256),
        (50, 15872),
        (100, 15488),
        (200, 13312),
        (400, 6912),
        (600, 1536),
        (700, 384),
        (800, 256),
        (801, 0),
    ];
    for (range, n) in expect {
        assert_eq!(clicks_per_16384(range), n, "range {range}");
    }
}
