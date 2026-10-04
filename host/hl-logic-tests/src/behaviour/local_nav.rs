//! Local navigation of a scripted walker: clear-path tests, the step-up
//! allowance, detour candidates, the straight/detour/graph choice, and the
//! moving lookahead.
//!
//! The test world is a set of 3-D boxes (Y-up) that block hull-centre
//! segments; a segment's impact fraction is the first of 256 evenly spaced
//! samples that falls inside a box. Other bodies use the shared actor sweep.
#![cfg(test)]

use crate::local_nav::*;
use crate::scripted_sequence::{Activity, RoutePlan, ScriptActor, ScriptMode, MODE_WALK, NO_CLIP};

#[derive(Default)]
struct Room {
    boxes: Vec<([i32; 3], [i32; 3])>,
    bodies: Vec<([i32; 3], [i32; 3])>,
    small: bool,
    hologram: bool,
    graph: bool,
    entry: Option<[i32; 3]>,
    graph_queries: Vec<[i32; 3]>,
    detours: Vec<(u8, bool, [i32; 3], u8)>,
}

fn inside(p: [f64; 3], b: &([i32; 3], [i32; 3])) -> bool {
    (0..3).all(|i| p[i] > b.0[i] as f64 && p[i] < b.1[i] as f64)
}

impl Room {
    fn frac(&self, from: [i32; 3], to: [i32; 3]) -> Option<i32> {
        (0..=256).find_map(|i| {
            let t = i as f64 / 256.0;
            let p: [f64; 3] =
                core::array::from_fn(|k| from[k] as f64 + (to[k] - from[k]) as f64 * t);
            self.boxes.iter().any(|b| inside(p, b)).then_some(i * 16)
        })
    }
}

impl LocalNavWorld for Room {
    fn small_hull(&self) -> bool {
        self.small
    }
    fn body_half_height(&self) -> i32 {
        if self.small {
            SMALL_HULL_HALF
        } else {
            HUMAN_HULL_HALF
        }
    }
    fn ignores_bodies(&self) -> bool {
        self.hologram
    }
    fn hull_blocked(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<i32> {
        self.frac(from, to)
    }
    fn hull_clear(&mut self, from: [i32; 3], to: [i32; 3]) -> bool {
        self.frac(from, to).is_none()
    }
    fn body_count(&self) -> usize {
        self.bodies.len()
    }
    fn body(&self, slot: usize) -> Option<([i32; 3], [i32; 3])> {
        Some(self.bodies[slot])
    }
    fn graph_route(&mut self, goal: [i32; 3]) -> bool {
        self.graph_queries.push(goal);
        self.graph
    }
    fn graph_entry(&mut self, _start: [i32; 3]) -> Option<[i32; 3]> {
        self.entry
    }
    fn trace_detour(&mut self, _blocked: i32, ring: u8, left: bool, point: [i32; 3], legs: u8) {
        self.detours.push((ring, left, point, legs));
    }
}

/// A pillar 80 wide (x) and 100 deep (z) straddling the straight path from
/// the origin to (0, 0, 400), floor to ceiling.
fn pillar_room() -> Room {
    Room {
        boxes: vec![([-40, -100, 150], [40, 300, 250])],
        ..Room::default()
    }
}

fn walker(pos: [i32; 3], goal: [i32; 3]) -> ScriptActor {
    ScriptActor {
        pos,
        alive: true,
        activity: Activity::Moving,
        yaw: 0,
        mode: ScriptMode {
            base: MODE_WALK,
            primed: false,
            routed: false,
            detour: false,
        },
        script: Some(0),
        goal: [goal[0] as i16, goal[1] as i16, goal[2] as i16],
        target_yaw: 0,
        residue: [3, -2],
        deadline: 500,
        play_clip: NO_CLIP,
        idle_clip: NO_CLIP,
        hold: 0,
        no_interrupt: false,
    }
}

#[test]
fn the_rule_constants() {
    assert_eq!(STEP_HEIGHT, 18);
    assert_eq!((SMALL_HULL_HALF, HUMAN_HULL_HALF), (18, 36));
    assert_eq!(FLOOR_CHANGE_MAX, 64);
    assert_eq!(LOOKAHEAD, 200);
    assert_eq!(DETOUR_RINGS, 8);
}

#[test]
fn a_small_hull_walks_over_a_lip_one_step_high_that_a_floor_level_probe_hits() {
    // A lip 30 units high across the path.
    let mut room = Room {
        boxes: vec![([-100, -10, 90], [100, 30, 110])],
        small: true,
        ..Room::default()
    };
    assert_eq!(world_blocked(&mut room, [0; 3], [0, 0, 200]), None);
    assert!(world_clear(&mut room, [0; 3], [0, 0, 200]));
    // Taller than a step: blocked, reporting the later of the two impacts.
    room.boxes = vec![([-100, -10, 90], [100, 50, 110])];
    assert_eq!(world_blocked(&mut room, [0; 3], [0, 0, 200]), Some(1856));
    // The human hull probes only at its own centre height.
    let mut human = Room {
        boxes: vec![([-100, 30, 90], [100, 40, 110])],
        ..Room::default()
    };
    assert_eq!(world_blocked(&mut human, [0; 3], [0, 0, 200]), Some(1856));
    assert!(!world_clear(&mut human, [0; 3], [0, 0, 200]));
}

#[test]
fn other_bodies_and_the_player_block_but_never_a_hologram() {
    let mut room = Room {
        bodies: vec![([-16, 0, 100], [16, 72, 132])],
        ..Room::default()
    };
    let hit = bodies_blocked(&mut room, [0; 3], [0, 0, 300], None);
    assert!(hit.is_some_and(|f| f > 0 && f < 4096), "{hit:?}");
    let mut empty = Room::default();
    let player = ([0, 36, 200], 36);
    assert!(bodies_blocked(&mut empty, [0; 3], [0, 0, 300], Some(player)).is_some());
    assert_eq!(bodies_blocked(&mut empty, [0; 3], [0, 0, 300], None), None);
    room.hologram = true;
    assert_eq!(
        bodies_blocked(&mut room, [0; 3], [0, 0, 300], Some(player)),
        None
    );
}

#[test]
fn blocked_distances_snap_down_to_whole_sixteen_unit_probes() {
    assert_eq!(blocked_distance(200, 4096), 192);
    assert_eq!(blocked_distance(200, 2048), 96);
    assert_eq!(blocked_distance(200, 1300), 48);
    assert_eq!(blocked_distance(200, 0), 0);
    assert_eq!(blocked_distance(200, 5000), 192);
}

#[test]
fn a_detour_tries_right_before_left_at_widening_offsets_past_the_obstruction() {
    let mut room = pillar_room();
    let apex = detour(&mut room, [0; 3], [0, 0, 400], 144, None).unwrap();
    // The first candidate, 96 units to the right of a point 32 past the
    // obstruction, clears the 80-wide pillar.
    assert_eq!(apex, [96, 0, 176]);
    assert_eq!(room.detours, [(0, false, [96, 0, 176], 3)]);
    // Wall off the right side: the left candidate of the same ring wins.
    let mut room = pillar_room();
    room.boxes.push(([40, -100, 0], [400, 300, 400]));
    let apex = detour(&mut room, [0; 3], [0, 0, 400], 144, None).unwrap();
    assert_eq!(apex, [-96, 0, 176]);
    assert_eq!(room.detours.len(), 2);
    // Nothing fits: every ring is tried on both sides.
    let mut room = pillar_room();
    room.boxes.push(([40, -100, 0], [2000, 300, 400]));
    room.boxes.push(([-2000, -100, 0], [-40, 300, 400]));
    assert_eq!(detour(&mut room, [0; 3], [0, 0, 400], 144, None), None);
    assert_eq!(room.detours.len(), 16);
}

#[test]
fn the_route_choice_goes_straight_detours_or_uses_the_graph() {
    // Clear and level: straight, no graph query.
    let mut room = Room {
        graph: true,
        ..Room::default()
    };
    assert_eq!(
        plan_route(&mut room, [0; 3], [0, 0, 400], true),
        RoutePlan::DIRECT
    );
    assert!(room.graph_queries.is_empty());
    // Clear but the mark is more than 64 units up or down: the graph.
    assert_eq!(
        plan_route(&mut room, [0; 3], [0, 64, 400], true),
        RoutePlan::DIRECT
    );
    let plan = plan_route(&mut room, [0; 3], [0, -65, 400], true);
    assert_eq!(
        plan,
        RoutePlan {
            waypoint: None,
            routed: true,
            detour: false
        }
    );
    room.entry = Some([50, 0, 50]);
    let plan = plan_route(&mut room, [0; 3], [0, 65, 400], true);
    assert_eq!(
        plan,
        RoutePlan {
            waypoint: Some([50, 0, 50]),
            routed: true,
            detour: true
        }
    );
    // Without a graph the floor change does not matter.
    assert_eq!(
        plan_route(&mut room, [0; 3], [0, 65, 400], false),
        RoutePlan::DIRECT
    );
    // Blocked: the detour first.
    let mut room = pillar_room();
    room.graph = true;
    let plan = plan_route(&mut room, [0; 3], [0, 0, 400], true);
    assert_eq!(plan.waypoint, Some([96, 0, 176]));
    assert!(plan.detour && !plan.routed);
    assert!(room.graph_queries.is_empty());
    // Blocked with no detour: the graph, else straight anyway.
    let mut room = Room {
        boxes: vec![([-3000, -100, 150], [3000, 300, 250])],
        graph: true,
        ..Room::default()
    };
    assert!(plan_route(&mut room, [0; 3], [0, 0, 400], true).routed);
    room.graph = false;
    assert_eq!(
        plan_route(&mut room, [0; 3], [0, 0, 400], true),
        RoutePlan::DIRECT
    );
}

#[test]
fn the_lookahead_checks_two_hundred_units_and_re_plans_when_blocked() {
    // The pillar starts 150 units ahead: inside the lookahead.
    let mut room = pillar_room();
    let mut a = walker([0; 3], [0, 0, 400]);
    a.hold = 3;
    let r = lookahead(&mut room, &mut a, [0, 0, 400], ([0, 36, -500], 28), false);
    assert_eq!(r, Replan::Detour);
    assert_eq!(a.goal, [96, 0, 176]);
    assert!(a.mode.detour);
    assert_eq!((a.residue, a.hold), ([0, 0], 0));
    // 250 units back the pillar is beyond the lookahead.
    let mut room = pillar_room();
    let mut a = walker([0, 0, -250], [0, 0, 400]);
    let before = a;
    let r = lookahead(&mut room, &mut a, [0, 0, 400], ([0, 36, -900], 28), false);
    assert_eq!(r, Replan::Clear);
    assert_eq!(a, before);
    // Standing on the waypoint: nothing to check.
    let mut a = walker([0, 0, 400], [0, 0, 400]);
    assert_eq!(
        lookahead(
            &mut pillar_room(),
            &mut a,
            [0, 0, 400],
            ([0, 36, 0], 28),
            false
        ),
        Replan::Clear
    );
}

#[test]
fn a_blocked_lookahead_without_a_detour_falls_back_to_the_graph() {
    let mut room = Room {
        boxes: vec![([-3000, -100, 150], [3000, 300, 250])],
        graph: true,
        entry: Some([0, 0, 120]),
        ..Room::default()
    };
    let mut a = walker([0; 3], [0, 0, 400]);
    let r = lookahead(&mut room, &mut a, [0, 0, 400], ([0, 36, -500], 28), true);
    assert_eq!(r, Replan::Graph);
    assert!(a.mode.routed && a.mode.detour);
    assert_eq!(a.goal, [0, 0, 120]);
    // No graph on the map: unchanged.
    let mut a = walker([0; 3], [0, 0, 400]);
    let before = a;
    let r = lookahead(&mut room, &mut a, [0, 0, 400], ([0, 36, -500], 28), false);
    assert_eq!(r, Replan::Unchanged);
    assert_eq!(a, before);
}

/// Recorded from ac83da7 behaviour. A walker heading diagonally past three
/// staggered pillars, with the player standing beside the path: the
/// lookahead result, the new waypoint, and every detour candidate tried, in
/// order, with its two-leg result bits.
#[test]
fn golden_detour_candidates_around_staggered_pillars() {
    let mut room = Room {
        boxes: vec![
            ([-60, -100, 150], [60, 300, 250]),
            ([60, -100, 120], [260, 300, 200]),
            ([-300, -100, 220], [-80, 300, 300]),
        ],
        ..Room::default()
    };
    let mut a = walker([40, 0, 0], [-40, 0, 420]);
    let r = lookahead(
        &mut room,
        &mut a,
        [-40, 0, 420],
        ([-140, 36, 100], 28),
        false,
    );
    let got = (r, a.goal, room.detours.clone());
    let expected: (Replan, [i16; 3], Vec<(u8, bool, [i32; 3], u8)>) = (
        Replan::Detour,
        [-87, 0, 156],
        vec![(0, false, [101, 0, 190], 0), (0, true, [-87, 0, 156], 3)],
    );
    assert_eq!(got, expected, "{got:?}");
}
