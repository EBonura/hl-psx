//! Track-train follower rides with exact recorded outputs. Every 0.1 s the
//! train prepares a chord toward the point speed x 0.1 ahead and covers it
//! over two 50 ms frames; it faces the point 100 units ahead, both measured
//! from its actual origin and the not-yet-advanced waypoint. Disabled
//! waypoints and phase ends stop movement but not facing; past the last
//! waypoint facing extrapolates the last segment until the train reaches it.
//! These complement the follower module's own tests, which check the same
//! rules with tolerances, by pinning whole rides exactly.

use crate::tram_follower::{
    prepare_follower_q8, tenth_second_distance_q8, units_to_q8, LookAheadPolicy, LookAheadStop,
    PathPoint, Vec3Q8, GOLDSRC_MOVE_POLICY, PATH_DISABLED, PATH_PHASE_END, PHASE_AWARE_MOVE_POLICY,
};
use LookAheadStop::{BlockedNext, DeadEnd, Reached, StopAfter};

const fn p(x: i32, y: i32, z: i32, flags: u8) -> PathPoint {
    PathPoint {
        position_q8: [units_to_q8(x), units_to_q8(y), units_to_q8(z)],
        flags,
    }
}

/// One update: (pointer after, passed pointer, origin after the two frames,
/// wheels target relative to the origin before, position stop, wheels stop,
/// wheels projected past the end).
type Row = (
    usize,
    Option<usize>,
    Vec3Q8,
    Vec3Q8,
    LookAheadStop,
    LookAheadStop,
    bool,
);

/// Start on waypoint 0 at `speed` units per second, 100-unit wheels.
fn ride(path: &[PathPoint], speed: i32, policy: LookAheadPolicy, updates: usize) -> Vec<Row> {
    let mut pointer = 0usize;
    let mut origin = path[0].position_q8;
    let mut rows = vec![];
    for _ in 0..updates {
        let f = prepare_follower_q8(
            path,
            pointer,
            origin,
            tenth_second_distance_q8(speed),
            units_to_q8(100),
            policy,
        );
        // The prepared half chord is applied on each of the two frames.
        for a in 0..3 {
            origin[a] += 2 * f.prepared_half_chord_q8[a];
        }
        pointer = f.next_pointer;
        rows.push((
            pointer,
            f.passed_pointer,
            origin,
            f.wheels_delta_q8,
            f.position.stop,
            f.wheels.stop,
            f.wheels.projected,
        ));
    }
    rows
}

const BEND: [PathPoint; 4] = [
    p(0, 0, 0, 0),
    p(60, 0, 0, 0),
    p(60, 0, 60, 0),
    p(120, 10, 60, 0),
];

/// Recorded from ac83da7 behaviour: 300 u/s around a right-angle bend and a
/// rising last leg, running into the end of the track.
#[test]
fn golden_ride_around_a_bend_to_the_end() {
    let expect: Vec<Row> = vec![
        (
            0,
            None,
            [7680, 0, 0],
            [15360, 0, 10239],
            Reached,
            Reached,
            false,
        ),
        (
            1,
            Some(1),
            [15360, 0, 0],
            [10205, 420, 15360],
            Reached,
            Reached,
            false,
        ),
        (
            1,
            None,
            [15360, 0, 7680],
            [10101, 1683, 15360],
            Reached,
            Reached,
            false,
        ),
        (
            2,
            Some(2),
            [15360, 0, 15360],
            [17677, 2946, 7680],
            Reached,
            DeadEnd,
            true,
        ),
        (
            2,
            None,
            [22934, 1262, 15360],
            [25252, 4208, 0],
            Reached,
            DeadEnd,
            true,
        ),
        (
            2,
            None,
            [30508, 2524, 15360],
            [25253, 4209, 0],
            Reached,
            DeadEnd,
            true,
        ),
        (
            2,
            None,
            [30720, 2560, 15360],
            [25252, 4209, 0],
            DeadEnd,
            DeadEnd,
            true,
        ),
        (
            2,
            None,
            [30720, 2560, 15360],
            [0, 0, 0],
            DeadEnd,
            DeadEnd,
            false,
        ),
        (
            2,
            None,
            [30720, 2560, 15360],
            [0, 0, 0],
            DeadEnd,
            DeadEnd,
            false,
        ),
        (
            2,
            None,
            [30720, 2560, 15360],
            [0, 0, 0],
            DeadEnd,
            DeadEnd,
            false,
        ),
        (
            2,
            None,
            [30720, 2560, 15360],
            [0, 0, 0],
            DeadEnd,
            DeadEnd,
            false,
        ),
        (
            2,
            None,
            [30720, 2560, 15360],
            [0, 0, 0],
            DeadEnd,
            DeadEnd,
            false,
        ),
    ];
    assert_eq!(ride(&BEND, 300, GOLDSRC_MOVE_POLICY, 12), expect);
}

/// Recorded from ac83da7 behaviour: the same track with the bend's second
/// waypoint disabled. The train stops on waypoint 1 but keeps facing along
/// the disabled leg.
#[test]
fn golden_ride_into_a_disabled_waypoint() {
    let mut path = BEND;
    path[2].flags = PATH_DISABLED;
    let expect: Vec<Row> = vec![
        (
            0,
            None,
            [7680, 0, 0],
            [15360, 0, 10239],
            Reached,
            Reached,
            false,
        ),
        (
            1,
            Some(1),
            [15360, 0, 0],
            [10205, 420, 15360],
            Reached,
            Reached,
            false,
        ),
        (
            1,
            None,
            [15360, 0, 0],
            [10101, 1683, 15360],
            BlockedNext(1),
            Reached,
            false,
        ),
        (
            1,
            None,
            [15360, 0, 0],
            [10101, 1683, 15360],
            BlockedNext(1),
            Reached,
            false,
        ),
        (
            1,
            None,
            [15360, 0, 0],
            [10101, 1683, 15360],
            BlockedNext(1),
            Reached,
            false,
        ),
        (
            1,
            None,
            [15360, 0, 0],
            [10101, 1683, 15360],
            BlockedNext(1),
            Reached,
            false,
        ),
    ];
    assert_eq!(ride(&path, 300, GOLDSRC_MOVE_POLICY, 6), expect);
}

/// Recorded from ac83da7 behaviour: waypoint 1 marks the end of a movement
/// phase; with the phase-aware policy the train arrives there and stops.
#[test]
fn golden_ride_to_a_phase_end() {
    let mut path = BEND;
    path[1].flags = PATH_PHASE_END;
    let expect: Vec<Row> = vec![
        (
            0,
            None,
            [7680, 0, 0],
            [15360, 0, 10239],
            Reached,
            Reached,
            false,
        ),
        (
            1,
            Some(1),
            [15360, 0, 0],
            [10205, 420, 15360],
            Reached,
            Reached,
            false,
        ),
        (
            1,
            None,
            [15360, 0, 0],
            [10101, 1683, 15360],
            StopAfter(2),
            Reached,
            false,
        ),
        (
            1,
            None,
            [15360, 0, 0],
            [10101, 1683, 15360],
            StopAfter(2),
            Reached,
            false,
        ),
        (
            1,
            None,
            [15360, 0, 0],
            [10101, 1683, 15360],
            StopAfter(2),
            Reached,
            false,
        ),
        (
            1,
            None,
            [15360, 0, 0],
            [10101, 1683, 15360],
            StopAfter(2),
            Reached,
            false,
        ),
    ];
    assert_eq!(ride(&path, 300, PHASE_AWARE_MOVE_POLICY, 6), expect);
}

/// Recorded from ac83da7 behaviour: 230 u/s onto a track whose last
/// waypoint is duplicated; the train stops on the first copy.
#[test]
fn golden_ride_onto_a_duplicated_last_waypoint() {
    let path = [p(0, 0, 0, 0), p(50, 0, 0, 0), p(50, 0, 0, 0)];
    let expect: Vec<Row> = vec![
        (
            0,
            None,
            [5886, 0, 0],
            [12800, 0, 0],
            Reached,
            Reached,
            false,
        ),
        (
            0,
            None,
            [11772, 0, 0],
            [6914, 0, 0],
            Reached,
            Reached,
            false,
        ),
        (
            1,
            Some(1),
            [12800, 0, 0],
            [1028, 0, 0],
            Reached,
            Reached,
            false,
        ),
        (1, None, [12800, 0, 0], [0, 0, 0], DeadEnd, DeadEnd, false),
        (1, None, [12800, 0, 0], [0, 0, 0], DeadEnd, DeadEnd, false),
    ];
    assert_eq!(ride(&path, 230, GOLDSRC_MOVE_POLICY, 5), expect);
}

/// Recorded from ac83da7 behaviour: 170 u/s over closely spaced waypoints;
/// one update skips three of them and reports only the last as passed.
#[test]
fn golden_ride_skipping_waypoints() {
    let path = [
        p(0, 0, 0, 0),
        p(5, 0, 0, 0),
        p(10, 0, 0, 0),
        p(15, 0, 0, 0),
        p(40, 0, 0, 0),
    ];
    let expect: Vec<Row> = vec![
        (
            3,
            Some(3),
            [4350, 0, 0],
            [25599, 0, 0],
            Reached,
            DeadEnd,
            true,
        ),
        (3, None, [8700, 0, 0], [25599, 0, 0], Reached, DeadEnd, true),
        (
            3,
            None,
            [10240, 0, 0],
            [25599, 0, 0],
            DeadEnd,
            DeadEnd,
            true,
        ),
        (3, None, [10240, 0, 0], [0, 0, 0], DeadEnd, DeadEnd, false),
    ];
    assert_eq!(ride(&path, 170, GOLDSRC_MOVE_POLICY, 4), expect);
}

/// Recorded from ac83da7 behaviour: the tenth-second look-ahead distance in
/// 1/256 units for assorted speeds.
#[test]
fn golden_tenth_second_distances() {
    let cases = [
        (-50, 0),
        (0, 0),
        (1, 25),
        (9, 230),
        (10, 256),
        (100, 2560),
        (230, 5888),
        (300, 7680),
        (i32::MAX, i32::MAX),
    ];
    for (speed, q8) in cases {
        assert_eq!(tenth_second_distance_q8(speed), q8, "speed {speed}");
    }
}
