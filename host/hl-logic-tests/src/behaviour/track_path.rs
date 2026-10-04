//! Tram waypoints: a train looks a distance ahead along its waypoints to set
//! its heading and keeps going straight past the last node. Firing a node
//! that has an alternate path switches paths (On = primary, Off = alternate);
//! firing any other node enables (On) or disables (Off) it, and a disabled
//! node stops a moving train though not the look-ahead. A node's speed key
//! replaces the train's speed.

use crate::world_rules::{
    track_lookahead_q8, track_speed_at_node, TrackGraph, TrackSwitches, TrackUse,
};

struct Graph {
    pos: Vec<[i32; 3]>,
    succ: Vec<Option<usize>>,
    alt: Vec<Option<usize>>,
    reverse_only: Vec<bool>,
    branching: bool,
}

impl TrackGraph for Graph {
    fn node_count(&self) -> usize {
        self.pos.len()
    }
    fn position(&self, n: usize) -> [i32; 3] {
        self.pos[n]
    }
    fn is_branching(&self) -> bool {
        self.branching
    }
    fn successor(&self, n: usize) -> Option<usize> {
        self.succ[n]
    }
    fn alternate(&self, n: usize) -> Option<usize> {
        self.alt[n]
    }
    fn alternate_reverse_only(&self, n: usize) -> bool {
        self.reverse_only[n]
    }
}

/// A branching track: 0 -> 1, then 1 -> 2 -> 4 (end) on the primary path or
/// 1 -> 3 -> 5 (end) on the alternate.
fn fork() -> Graph {
    Graph {
        pos: vec![
            [0, 0, 0],
            [300, 0, 0],
            [300, 0, 400],
            [600, 40, 0],
            [600, 40, 500],
            [900, 40, -100],
        ],
        succ: vec![Some(1), Some(2), Some(4), Some(5), None, None],
        alt: vec![None, Some(3), None, None, None, None],
        reverse_only: vec![false; 6],
        branching: true,
    }
}

/// A plain path: the four corners of a 100-unit square, in order.
fn square() -> Graph {
    Graph {
        pos: vec![[0, 0, 0], [100, 0, 0], [100, 0, 100], [0, 0, 100]],
        succ: vec![Some(1), Some(2), Some(3), None],
        alt: vec![None; 4],
        reverse_only: vec![false; 4],
        branching: false,
    }
}

fn q8(p: [i32; 3]) -> [i32; 3] {
    [p[0] << 8, p[1] << 8, p[2] << 8]
}

#[test]
fn a_plain_path_runs_its_nodes_in_order() {
    let g = square();
    let sw = TrackSwitches::new();
    assert_eq!(sw.next(&g, 0), Some(1));
    assert_eq!(sw.next(&g, 2), Some(3));
    assert_eq!(sw.next(&g, 3), None);
}

#[test]
fn look_ahead_follows_the_path_across_nodes() {
    let g = square();
    let sw = TrackSwitches::new();
    assert_eq!(track_lookahead_q8(&g, &sw, 0, 0, 50), q8([50, 0, 0]));
    assert_eq!(track_lookahead_q8(&g, &sw, 0, 0, 150), q8([100, 0, 50]));
    assert_eq!(track_lookahead_q8(&g, &sw, 0, 0, 250), q8([50, 0, 100]));
}

#[test]
fn look_ahead_continues_straight_past_the_last_node() {
    let g = square();
    let sw = TrackSwitches::new();
    // 100 units past the end of the final (2 -> 3) leg.
    assert_eq!(track_lookahead_q8(&g, &sw, 0, 0, 400), q8([-100, 0, 100]));
}

#[test]
fn switching_a_node_with_an_alternate_changes_the_path() {
    let g = fork();
    let mut sw = TrackSwitches::new();
    assert_eq!(sw.next(&g, 1), Some(2));
    sw.use_node(1, true, TrackUse::Off);
    assert_eq!(sw.next(&g, 1), Some(3));
    sw.use_node(1, true, TrackUse::On);
    assert_eq!(sw.next(&g, 1), Some(2));
    sw.use_node(1, true, TrackUse::Toggle);
    assert_eq!(sw.next(&g, 1), Some(3));
    sw.use_node(1, true, TrackUse::Toggle);
    assert_eq!(sw.next(&g, 1), Some(2));
    // Switching paths never disables anything.
    assert!(!sw.is_disabled(1));
}

#[test]
fn a_reverse_only_alternate_does_not_change_forward_travel() {
    let mut g = fork();
    g.reverse_only[1] = true;
    let mut sw = TrackSwitches::new();
    sw.use_node(1, true, TrackUse::Off);
    assert_eq!(sw.next(&g, 1), Some(2));
}

#[test]
fn firing_a_plain_node_enables_or_disables_it() {
    let g = fork();
    let mut sw = TrackSwitches::new();
    sw.use_node(2, false, TrackUse::Off);
    assert!(sw.is_disabled(2));
    sw.use_node(2, false, TrackUse::On);
    assert!(!sw.is_disabled(2));
    sw.use_node(2, false, TrackUse::Toggle);
    assert!(sw.is_disabled(2));
    // Disabling a node does not change where the path goes.
    assert_eq!(sw.next(&g, 1), Some(2));
}

#[test]
fn a_disabled_node_stops_the_train_but_not_the_look_ahead() {
    let g = fork();
    let mut sw = TrackSwitches::new();
    sw.disable(2);
    assert_eq!(sw.next_open(&g, 1), None);
    assert_eq!(sw.next_open(&g, 0), Some(1));
    // The heading still looks along 1 -> 2.
    assert_eq!(track_lookahead_q8(&g, &sw, 1, 0, 100), q8([300, 0, 100]));
}

#[test]
fn only_the_first_256_nodes_can_be_switched() {
    let mut sw = TrackSwitches::new();
    sw.use_node(256, false, TrackUse::Off);
    sw.disable(300);
    assert!(!sw.is_disabled(256) && !sw.is_disabled(300));
    sw.use_node(255, false, TrackUse::Off);
    assert!(sw.is_disabled(255));
}

#[test]
fn a_node_speed_key_replaces_the_speed_and_zero_keeps_it() {
    assert_eq!(track_speed_at_node(200, 400), 400);
    assert_eq!(track_speed_at_node(200, 0), 200);
    assert_eq!(track_speed_at_node(200, 50), 50);
}

#[test]
fn degenerate_tracks() {
    let sw = TrackSwitches::new();
    let empty = Graph {
        pos: vec![],
        succ: vec![],
        alt: vec![],
        reverse_only: vec![],
        branching: true,
    };
    assert_eq!(track_lookahead_q8(&empty, &sw, 0, 0, 50), [0; 3]);
    let single = Graph {
        pos: vec![[7, 8, 9]],
        succ: vec![None],
        alt: vec![None],
        reverse_only: vec![false],
        branching: true,
    };
    assert_eq!(track_lookahead_q8(&single, &sw, 0, 0, 50), q8([7, 8, 9]));
}

type Case = ((usize, i32, i32), [i32; 3]);

fn check(g: &Graph, sw: &TrackSwitches, cases: &[Case], label: &str) {
    for &((seg, dist, ahead), expect) in cases {
        assert_eq!(
            track_lookahead_q8(g, sw, seg, dist, ahead),
            expect,
            "{label}: node {seg} +{dist} ahead {ahead}"
        );
    }
}

/// Recorded from ac83da7 behaviour: look-ahead points (1/256 units) on the
/// fork with the primary path selected, as (node, distance past it, ahead).
/// Includes a branching dead end (node 4 stays put), a negative distance
/// ahead (treated as 0) and a starting distance beyond the segment end.
#[test]
fn golden_fork_primary() {
    let cases: [Case; 15] = [
        ((0, 0, 0), [0, 0, 0]),
        ((0, 0, 100), [25593, 0, 0]),
        ((0, 100, 250), [76800, 0, 12800]),
        ((0, 0, 700), [76800, 0, 102400]),
        ((1, 0, 100), [76800, 0, 25600]),
        ((1, 0, 399), [76800, 0, 102125]),
        ((1, 0, 400), [76800, 0, 102400]),
        ((1, 0, 401), [77025, 30, 102475]),
        ((1, 0, 1000), [221700, 19320, 150700]),
        ((2, 0, 0), [76800, 0, 102400]),
        ((2, 250, 250), [197550, 16100, 142650]),
        ((4, 0, 100), [153600, 10240, 128000]),
        ((4, 0, 0), [153600, 10240, 128000]),
        ((0, 0, -5), [0, 0, 0]),
        ((0, 400, 0), [102393, 0, 0]),
    ];
    check(&fork(), &TrackSwitches::new(), &cases, "primary");
}

/// Recorded from ac83da7 behaviour: the same fork after firing node 1 Off
/// (alternate path selected).
#[test]
fn golden_fork_alternate() {
    let mut sw = TrackSwitches::new();
    sw.use_node(1, true, TrackUse::Off);
    let cases: [Case; 6] = [
        ((0, 0, 700), [177412, 10240, -7938]),
        ((1, 0, 100), [102225, 3390, 0]),
        ((1, 0, 500), [201712, 10240, -16038]),
        ((1, 0, 1500), [444750, 10240, -97050]),
        ((3, 0, 100), [177900, 10240, -8100]),
        ((3, 0, 1000), [396637, 10240, -81013]),
    ];
    check(&fork(), &sw, &cases, "alternate");
}

/// Recorded from ac83da7 behaviour: look-ahead on the plain square path,
/// including starting nodes at or past the last one (treated as the start of
/// the final leg).
#[test]
fn golden_plain_path() {
    let cases: [Case; 8] = [
        ((0, 0, 50), [12800, 0, 0]),
        ((0, 0, 150), [25600, 0, 12800]),
        ((0, 0, 250), [12800, 0, 25600]),
        ((0, 0, 400), [-25600, 0, 25600]),
        ((2, 0, 300), [-51200, 0, 25600]),
        ((3, 0, 10), [23043, 0, 25600]),
        ((3, 50, 10), [10243, 0, 25600]),
        ((9, 0, 10), [23043, 0, 25600]),
    ];
    check(&square(), &TrackSwitches::new(), &cases, "plain");
}
