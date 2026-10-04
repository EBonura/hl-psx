//! Local navigation for an actor walking or running to a scripted mark:
//! whether the straight hull path is clear, the detour point it picks when
//! blocked, the choice between straight, detour and node graph, and the
//! short lookahead that re-plans while it moves.
//!
//! Traces, other actors' boxes and node-graph queries come through
//! [`LocalNavWorld`]. Coordinates are Y-up world units with foot origins;
//! fractions are Q12 (4096 = the whole chord).

use crate::scripted_sequence::{RoutePlan, ScriptActor};
use psx_goldsrc::ground_logic::sweep_player_actor;
use psx_math::int32::isqrt_i32;

/// Monster step height.
pub const STEP_HEIGHT: i32 = 18;
/// Half height of the small hull used by monsters 36 units tall or less.
pub const SMALL_HULL_HALF: i32 = 18;
/// Half height of the human hull.
pub const HUMAN_HULL_HALF: i32 = 36;
/// A mark more than this far above or below takes the node graph even when
/// the straight path is clear.
pub const FLOOR_CHANGE_MAX: i32 = 64;
/// Lookahead distance while moving.
pub const LOOKAHEAD: i32 = 200;
/// Detour candidates on each side.
pub const DETOUR_RINGS: i32 = 8;
/// Width of the human hull used for detour geometry.
pub const DETOUR_HULL_WIDTH: i32 = 32;
/// Blocked distances are reported in whole walking probes of this size.
pub const PROBE_STEP: i32 = 16;

/// What local navigation asks of the world for one actor.
pub trait LocalNavWorld {
    /// The actor moves in the small hull (and the map has one).
    fn small_hull(&self) -> bool;
    /// Half height of the actor's own body box against other bodies.
    fn body_half_height(&self) -> i32;
    /// The actor is never blocked by other bodies (a hologram).
    fn ignores_bodies(&self) -> bool;
    /// First impact fraction of the actor's nav hull between two hull-centre
    /// points, against the world and moving brushes; `None` when clear.
    fn hull_blocked(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<i32>;
    /// Cheaper yes/no human-hull test between two hull-centre points.
    fn hull_clear(&mut self, from: [i32; 3], to: [i32; 3]) -> bool;
    /// Number of roster slots that may hold other bodies.
    fn body_count(&self) -> usize;
    /// Box of another live solid body in `slot`, if any.
    fn body(&self, slot: usize) -> Option<([i32; 3], [i32; 3])>;
    /// The node graph links the actor's position to `goal` (and the actor's
    /// route cache now waits at the source node).
    fn graph_route(&mut self, goal: [i32; 3]) -> bool;
    /// First shortcut into that route from `start`.
    fn graph_entry(&mut self, start: [i32; 3]) -> Option<[i32; 3]>;
    /// Diagnostic trace of one detour candidate.
    fn trace_detour(&mut self, blocked_dist: i32, ring: u8, left: bool, point: [i32; 3], legs: u8);
}

/// The player as an obstacle: hull-centre position and half height.
pub type PlayerBox = ([i32; 3], i32);

/// Impact fraction of the actor's hull walking from `start` to `goal`
/// against the world. A small-hull actor that is blocked at floor height
/// still passes if the same path one step higher is clear.
pub fn world_blocked(w: &mut dyn LocalNavWorld, start: [i32; 3], goal: [i32; 3]) -> Option<i32> {
    let half = if w.small_hull() {
        SMALL_HULL_HALF
    } else {
        HUMAN_HULL_HALF
    };
    let mut best = 0;
    for step in 0..if half == SMALL_HULL_HALF { 2 } else { 1 } {
        let rise = half + STEP_HEIGHT * step;
        let from = [start[0], start[1] + rise, start[2]];
        let to = [goal[0], goal[1] + rise, goal[2]];
        match w.hull_blocked(from, to) {
            None => return None,
            Some(f) => best = best.max(f),
        }
    }
    Some(best)
}

/// The actor's hull can walk straight from `start` to `goal` (world only).
pub fn world_clear(w: &mut dyn LocalNavWorld, start: [i32; 3], goal: [i32; 3]) -> bool {
    if w.small_hull() {
        return world_blocked(w, start, goal).is_none();
    }
    let from = [start[0], start[1] + HUMAN_HULL_HALF, start[2]];
    let to = [goal[0], goal[1] + HUMAN_HULL_HALF, goal[2]];
    w.hull_clear(from, to)
}

/// Earliest impact with the player (when given) or another body.
pub fn bodies_blocked(
    w: &mut dyn LocalNavWorld,
    start: [i32; 3],
    goal: [i32; 3],
    player: Option<PlayerBox>,
) -> Option<i32> {
    if w.ignores_bodies() {
        return None;
    }
    let half = w.body_half_height();
    let from = [start[0], start[1] + half, start[2]];
    let to = [goal[0], goal[1] + half, goal[2]];
    let mut best = 4096;
    if let Some((p, ph)) = player {
        let mins = [p[0] - 16, p[1] - ph, p[2] - 16];
        let maxs = [p[0] + 16, p[1] + ph, p[2] + 16];
        if let Some(hit) = sweep_player_actor(from, to, mins, maxs, half) {
            best = best.min(hit.frac);
        }
    }
    let n = w.body_count();
    let mut slot = 0usize;
    while slot < n {
        if let Some((mins, maxs)) = w.body(slot) {
            if let Some(hit) = sweep_player_actor(from, to, mins, maxs, half) {
                best = best.min(hit.frac);
            }
        }
        slot += 1;
    }
    if best < 4096 {
        Some(best)
    } else {
        None
    }
}

fn leg_clear(
    w: &mut dyn LocalNavWorld,
    start: [i32; 3],
    goal: [i32; 3],
    player: Option<PlayerBox>,
) -> bool {
    world_clear(w, start, goal) && bodies_blocked(w, start, goal, player).is_none()
}

/// A detour point past an obstruction `blocked_dist` units along the path:
/// candidates sit beyond the obstruction at widening lateral offsets, right
/// before left, and the first whose two legs are both clear wins.
pub fn detour(
    w: &mut dyn LocalNavWorld,
    start: [i32; 3],
    goal: [i32; 3],
    blocked_dist: i32,
    player: Option<PlayerBox>,
) -> Option<[i32; 3]> {
    const SIDE_START: i32 = DETOUR_HULL_WIDTH * 3;
    const SIDE_STEP: i32 = DETOUR_HULL_WIDTH * 2;

    let delta = [goal[0] - start[0], goal[1] - start[1], goal[2] - start[2]];
    let length = isqrt_i32(
        delta[0]
            .saturating_mul(delta[0])
            .saturating_add(delta[1].saturating_mul(delta[1]))
            .saturating_add(delta[2].saturating_mul(delta[2])),
    )
    .max(1);
    let horizontal_length = isqrt_i32(
        delta[0]
            .saturating_mul(delta[0])
            .saturating_add(delta[2].saturating_mul(delta[2])),
    )
    .max(1);
    let forward = [
        delta[0] * 4096 / length,
        delta[1] * 4096 / length,
        delta[2] * 4096 / length,
    ];
    let side = [
        delta[2] * 4096 / horizontal_length,
        0,
        -delta[0] * 4096 / horizontal_length,
    ];
    let ahead = blocked_dist.max(0) + DETOUR_HULL_WIDTH;
    let base = [
        start[0] + ((forward[0] * ahead) >> 12),
        start[1] + ((forward[1] * ahead) >> 12),
        start[2] + ((forward[2] * ahead) >> 12),
    ];
    let mut ring = 0i32;
    while ring < DETOUR_RINGS {
        let lateral = SIDE_START + ring * SIDE_STEP;
        let offset = [(side[0] * lateral) >> 12, 0, (side[2] * lateral) >> 12];
        let right = [base[0] + offset[0], base[1], base[2] + offset[2]];
        let right_first = leg_clear(w, start, right, player);
        let right_second = right_first && leg_clear(w, right, goal, player);
        w.trace_detour(
            blocked_dist,
            ring as u8,
            false,
            right,
            right_first as u8 | ((right_second as u8) << 1),
        );
        if right_second {
            return Some(right);
        }
        let left = [base[0] - offset[0], base[1], base[2] - offset[2]];
        let left_first = leg_clear(w, start, left, player);
        let left_second = left_first && leg_clear(w, left, goal, player);
        w.trace_detour(
            blocked_dist,
            ring as u8,
            true,
            left,
            left_first as u8 | ((left_second as u8) << 1),
        );
        if left_second {
            return Some(left);
        }
        ring += 1;
    }
    None
}

#[inline]
fn min_blocked(world: Option<i32>, bodies: Option<i32>) -> Option<i32> {
    match (world, bodies) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Distance along a `distance`-long path to the start of the walking probe
/// that hit the obstruction at fraction `frac`.
#[inline]
pub fn blocked_distance(distance: i32, frac: i32) -> i32 {
    ((distance * frac.clamp(0, 4096)) >> 12) & !(PROBE_STEP - 1)
}

/// How a walk or run to `goal` from `pos` starts: straight when clear (but
/// via the graph if the mark is on another floor), else a detour, else the
/// node graph, else straight anyway.
pub fn plan_route(
    w: &mut dyn LocalNavWorld,
    pos: [i32; 3],
    goal: [i32; 3],
    has_graph: bool,
) -> RoutePlan {
    let mut plan = RoutePlan::DIRECT;
    let world = world_blocked(w, pos, goal);
    let bodies = bodies_blocked(w, pos, goal, None);
    if let Some(blocked_frac) = min_blocked(world, bodies) {
        let dx = goal[0] - pos[0];
        let dz = goal[2] - pos[2];
        let distance =
            isqrt_i32(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz))).max(0);
        let blocked_dist = blocked_distance(distance, blocked_frac);
        if let Some(apex) = detour(w, pos, goal, blocked_dist, None) {
            plan.waypoint = Some(apex);
            plan.detour = true;
        } else if has_graph && w.graph_route(goal) {
            plan.routed = true;
            if let Some(entry) = w.graph_entry(pos) {
                plan.waypoint = Some(entry);
                plan.detour = true;
            }
        }
    } else if (goal[1] - pos[1]).abs() > FLOOR_CHANGE_MAX && has_graph && w.graph_route(goal) {
        plan.routed = true;
        if let Some(entry) = w.graph_entry(pos) {
            plan.waypoint = Some(entry);
            plan.detour = true;
        }
    }
    plan
}

/// What the lookahead changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Replan {
    /// The path ahead is clear (or there is nothing to walk).
    Clear,
    /// Blocked, and neither a detour nor the graph helped.
    Unchanged,
    /// Heading for a new detour point; the cached graph route is stale.
    Detour,
    /// Switched to the node graph.
    Graph,
}

/// Re-check up to [`LOOKAHEAD`] units of the straight path to `waypoint` and
/// re-plan the actor's movement when it is blocked.
pub fn lookahead(
    w: &mut dyn LocalNavWorld,
    actor: &mut ScriptActor,
    waypoint: [i32; 3],
    player: PlayerBox,
    has_graph: bool,
) -> Replan {
    let start = actor.pos;
    let delta = [
        waypoint[0] - start[0],
        waypoint[1] - start[1],
        waypoint[2] - start[2],
    ];
    let waypoint_dist = isqrt_i32(
        delta[0]
            .saturating_mul(delta[0])
            .saturating_add(delta[2].saturating_mul(delta[2])),
    )
    .max(0);
    if waypoint_dist == 0 {
        return Replan::Clear;
    }
    // The probe direction is normalised in 3-D but its length is clamped by
    // the planar distance.
    let direction_len = isqrt_i32(
        delta[0]
            .saturating_mul(delta[0])
            .saturating_add(delta[1].saturating_mul(delta[1]))
            .saturating_add(delta[2].saturating_mul(delta[2])),
    )
    .max(1);
    let check_dist = waypoint_dist.min(LOOKAHEAD);
    let check_end = [
        start[0] + delta[0] * check_dist / direction_len,
        start[1] + delta[1] * check_dist / direction_len,
        start[2] + delta[2] * check_dist / direction_len,
    ];
    let world = world_blocked(w, start, check_end);
    let bodies = bodies_blocked(w, start, check_end, Some(player));
    let Some(blocked_frac) = min_blocked(world, bodies) else {
        return Replan::Clear;
    };
    if blocked_frac >= 4096 {
        return Replan::Clear;
    }
    let blocked_dist = blocked_distance(check_dist, blocked_frac);
    if let Some(apex) = detour(w, start, waypoint, blocked_dist, Some(player)) {
        actor.goal = [apex[0] as i16, apex[1] as i16, apex[2] as i16];
        actor.mode.detour = true;
        actor.residue = [0, 0];
        actor.hold = 0;
        return Replan::Detour;
    }
    if has_graph && w.graph_route(waypoint) {
        actor.mode.routed = true;
        if let Some(entry) = w.graph_entry(start) {
            actor.goal = [entry[0] as i16, entry[1] as i16, entry[2] as i16];
            actor.mode.detour = true;
        }
        actor.residue = [0, 0];
        actor.hold = 0;
        return Replan::Graph;
    }
    Replan::Unchanged
}
