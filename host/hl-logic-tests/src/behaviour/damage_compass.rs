//! Damage compass: a hit lights the screen-edge arrows facing its source
//! (all four within 50 units), an edge lights when the source is more than
//! 0.3 of the way off the perpendicular toward it, the arrows go dark half a
//! second (10 ticks) after the last hit, and turn red at 25 health or below.

use crate::player_rules::{
    compass_colour, DamageCompass, COMPASS_FRONT, COMPASS_LEFT, COMPASS_REAR, COMPASS_RIGHT,
};

const ALL: u8 = COMPASS_FRONT | COMPASS_RIGHT | COMPASS_REAR | COMPASS_LEFT;
/// View looking down +Z with screen right along +X.
const FWD_Z: [i16; 3] = [0, 0, 4096];
const RIGHT_X: [i16; 3] = [4096, 0, 0];

fn hit_from(source: [i32; 3]) -> u8 {
    let mut c = DamageCompass::new();
    c.note_hit([0, 0, 0], FWD_Z, RIGHT_X, source);
    c.edges()
}

/// Source on a horizontal circle of `radius` around the origin, `deg`
/// degrees clockwise from straight ahead (toward screen right).
fn around(deg: i32, radius: f64) -> [i32; 3] {
    let a = (deg as f64).to_radians();
    [
        (a.sin() * radius).round() as i32,
        0,
        (a.cos() * radius).round() as i32,
    ]
}

#[test]
fn a_close_source_lights_all_four_edges() {
    assert_eq!(hit_from([50, 0, 0]), ALL);
    assert_eq!(hit_from([0, 0, -50]), ALL);
    assert_eq!(hit_from([28, 28, 28]), ALL);
    // Just beyond 50 units the direction counts again.
    assert_eq!(hit_from([51, 0, 0]), COMPASS_RIGHT);
    assert_eq!(hit_from([29, 29, 29]), COMPASS_FRONT | COMPASS_RIGHT);
}

#[test]
fn cardinal_directions_light_one_edge() {
    assert_eq!(hit_from(around(0, 300.0)), COMPASS_FRONT);
    assert_eq!(hit_from(around(90, 300.0)), COMPASS_RIGHT);
    assert_eq!(hit_from(around(180, 300.0)), COMPASS_REAR);
    assert_eq!(hit_from(around(270, 300.0)), COMPASS_LEFT);
}

#[test]
fn an_edge_lights_past_three_tenths_off_the_perpendicular() {
    // 15 degrees off ahead: sideways share 0.26, below 0.3.
    assert_eq!(hit_from(around(15, 1000.0)), COMPASS_FRONT);
    // 20 degrees: sideways share 0.34, above 0.3.
    assert_eq!(hit_from(around(20, 1000.0)), COMPASS_FRONT | COMPASS_RIGHT);
    // Diagonals light both neighbours.
    assert_eq!(hit_from(around(225, 1000.0)), COMPASS_REAR | COMPASS_LEFT);
}

#[test]
fn a_source_straight_above_lights_nothing() {
    assert_eq!(hit_from([0, 300, 0]), 0);
    // Height still counts toward the distance, so a little lean ahead is not
    // enough until it passes 0.3 of the full distance.
    assert_eq!(hit_from([0, 300, 90]), 0);
    assert_eq!(hit_from([0, 300, 100]), COMPASS_FRONT);
}

#[test]
fn arrows_go_dark_ten_ticks_after_the_last_hit() {
    let mut c = DamageCompass::new();
    c.note_hit([0, 0, 0], FWD_Z, RIGHT_X, around(0, 300.0));
    for _ in 0..9 {
        c.tick();
        assert_eq!(c.edges(), COMPASS_FRONT);
    }
    c.tick();
    assert_eq!(c.edges(), 0);
    // Further ticks keep it dark.
    c.tick();
    assert_eq!(c.edges(), 0);
}

#[test]
fn a_new_hit_adds_edges_and_restarts_the_timer() {
    let mut c = DamageCompass::new();
    c.note_hit([0, 0, 0], FWD_Z, RIGHT_X, around(0, 300.0));
    for _ in 0..5 {
        c.tick();
    }
    c.note_hit([0, 0, 0], FWD_Z, RIGHT_X, around(180, 300.0));
    assert_eq!(c.edges(), COMPASS_FRONT | COMPASS_REAR);
    for _ in 0..9 {
        c.tick();
    }
    assert_eq!(c.edges(), COMPASS_FRONT | COMPASS_REAR);
    c.tick();
    assert_eq!(c.edges(), 0);
}

#[test]
fn a_hit_lighting_nothing_still_restarts_the_timer() {
    let mut c = DamageCompass::new();
    c.note_hit([0, 0, 0], FWD_Z, RIGHT_X, around(90, 300.0));
    for _ in 0..9 {
        c.tick();
    }
    c.note_hit([0, 0, 0], FWD_Z, RIGHT_X, [0, 300, 0]);
    c.tick();
    assert_eq!(c.edges(), COMPASS_RIGHT);
}

#[test]
fn arrows_turn_red_at_25_health() {
    assert_eq!(compass_colour(100), [128, 40, 0]);
    assert_eq!(compass_colour(26), [128, 40, 0]);
    assert_eq!(compass_colour(25), [128, 0, 0]);
    assert_eq!(compass_colour(0), [128, 0, 0]);
}

/// Recorded from ac83da7 behaviour: lit edges for a source 200 units away at
/// each 15 degree step clockwise from straight ahead, view along +Z.
#[test]
fn golden_edges_around_the_player() {
    let expect: [(i32, u8); 24] = [
        (0, 1),
        (15, 1),
        (30, 3),
        (45, 3),
        (60, 3),
        (75, 2),
        (90, 2),
        (105, 2),
        (120, 6),
        (135, 6),
        (150, 6),
        (165, 4),
        (180, 4),
        (195, 4),
        (210, 12),
        (225, 12),
        (240, 12),
        (255, 8),
        (270, 8),
        (285, 8),
        (300, 9),
        (315, 9),
        (330, 9),
        (345, 1),
    ];
    for (deg, edges) in expect {
        assert_eq!(hit_from(around(deg, 200.0)), edges, "{deg} degrees");
    }
}

/// Recorded from ac83da7 behaviour: player at (100, 0, -50) looking along the
/// +X+Z diagonal (screen right along +X-Z); a source 1000 units away and 40
/// up at each 30 degree step clockwise from world +Z.
#[test]
fn golden_edges_for_a_diagonal_view() {
    let fwd = [2896, 0, 2896];
    let right = [2896, 0, -2896];
    let expect: [(i32, u8); 12] = [
        (0, 9),
        (30, 1),
        (60, 1),
        (90, 3),
        (120, 2),
        (150, 2),
        (180, 6),
        (210, 4),
        (240, 4),
        (270, 12),
        (300, 8),
        (330, 8),
    ];
    for (deg, edges) in expect {
        let p = around(deg, 1000.0);
        let source = [100 + p[0], 40, -50 + p[2]];
        let mut c = DamageCompass::new();
        c.note_hit([100, 0, -50], fwd, right, source);
        assert_eq!(c.edges(), edges, "{deg} degrees");
    }
}

/// Recorded from ac83da7 behaviour: assorted sources with the view along +Z,
/// including far sources beyond the 4096-unit clamp.
#[test]
fn golden_assorted_sources() {
    let cases: [([i32; 3], u8); 9] = [
        ([50, 0, 0], 15),
        ([51, 0, 0], 2),
        ([30, 30, 29], 3),
        ([30, 30, 30], 3),
        ([0, 300, 0], 0),
        ([0, 300, 100], 1),
        ([0, 300, 120], 1),
        ([9000, 0, 9000], 3),
        ([-9000, 0, 100], 8),
    ];
    for (s, edges) in cases {
        assert_eq!(hit_from(s), edges, "{s:?}");
    }
}
