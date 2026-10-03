//! Node-graph navigation: nearest visible node, route availability, the
//! first shortcut into a route, and following it to the goal.
//!
//! The test graph is a few nodes on a flat floor (Y-up) with walls as planar
//! boxes that block both sight lines and walking. The cooked next-hop table
//! is computed here by breadth-first search over the listed links.
#![cfg(test)]

use crate::nav_graph::*;

struct Graph {
    nodes: Vec<([i32; 3], bool)>,
    links: Vec<(usize, usize)>,
    /// Planar boxes (min x, min z, max x, max z) that block everything.
    walls: Vec<[i32; 4]>,
    cooked: bool,
    sight_checks: u32,
}

fn crosses(from: [i32; 3], to: [i32; 3], w: [i32; 4]) -> bool {
    // Sample the segment finely; walls in these tests are at least 8 thick.
    let steps = 256;
    (0..=steps).any(|i| {
        let x = from[0] as f64 + (to[0] - from[0]) as f64 * i as f64 / steps as f64;
        let z = from[2] as f64 + (to[2] - from[2]) as f64 * i as f64 / steps as f64;
        x > w[0] as f64 && x < w[2] as f64 && z > w[1] as f64 && z < w[3] as f64
    })
}

impl Graph {
    fn clear(&self, from: [i32; 3], to: [i32; 3]) -> bool {
        !self.walls.iter().any(|&w| crosses(from, to, w))
    }
    fn hop(&self, src: usize, dst: usize) -> usize {
        // BFS from dst; the hop from src is its neighbour nearest dst.
        let n = self.nodes.len();
        let mut dist = vec![usize::MAX; n];
        dist[dst] = 0;
        let mut queue = std::collections::VecDeque::from([dst]);
        while let Some(a) = queue.pop_front() {
            for &(x, y) in &self.links {
                for (p, q) in [(x, y), (y, x)] {
                    if p == a && dist[q] == usize::MAX {
                        dist[q] = dist[a] + 1;
                        queue.push_back(q);
                    }
                }
            }
        }
        let mut best = src;
        for &(x, y) in &self.links {
            for (p, q) in [(x, y), (y, x)] {
                if p == src && dist[q] < dist[best] {
                    best = q;
                }
            }
        }
        best
    }
}

impl NavGraph for Graph {
    fn node_count(&self) -> usize {
        self.nodes.len()
    }
    fn node_pos(&self, node: usize) -> [i32; 3] {
        self.nodes[node].0
    }
    fn is_land(&self, node: usize) -> bool {
        self.nodes[node].1
    }
    fn cooked_next(&self, src: usize, dst: usize) -> Option<usize> {
        self.cooked.then(|| self.hop(src, dst))
    }
    fn line_clear(&mut self, from: [i32; 3], to: [i32; 3]) -> bool {
        self.sight_checks += 1;
        self.clear(from, to)
    }
    fn walkable(&mut self, from: [i32; 3], to: [i32; 3]) -> bool {
        self.clear(from, to)
    }
}

/// A U-shaped corridor around a wall at x 100..140, z -400..200:
///
/// ```text
///   0 (0,0)      3 (240,0)
///   |            |
///   1 (0,300) -- 2 (240,300)
/// ```
fn corridor() -> Graph {
    Graph {
        nodes: vec![
            ([0, 0, 0], true),
            ([0, 0, 300], true),
            ([240, 0, 300], true),
            ([240, 0, 0], true),
            ([120, 0, 500], false),
        ],
        links: vec![(0, 1), (1, 2), (2, 3)],
        walls: vec![[100, -400, 140, 200]],
        cooked: true,
        sight_checks: 0,
    }
}

#[test]
fn the_nearest_node_is_the_closest_visible_land_node_measured_to_its_probe_point() {
    let mut g = corridor();
    assert_eq!(nearest_node(&mut g, [20, 0, 10], NEAREST_RANGE2), 0);
    // Node 3 is closer to this point but behind the wall.
    assert_eq!(nearest_node(&mut g, [90, 0, 0], NEAREST_RANGE2), 0);
    assert_eq!(nearest_node(&mut g, [150, 0, 0], NEAREST_RANGE2), 3);
    // Water/air nodes never count.
    assert_eq!(nearest_node(&mut g, [120, 0, 480], NEAREST_RANGE2), 1);
    // The probe point is 8 units above the node: a point 8 above node 0 is
    // at distance zero from it, a point at the node's own height is not.
    let mut g = Graph {
        nodes: vec![([0, 0, 0], true), ([0, 16, 0], true)],
        links: vec![],
        walls: vec![],
        cooked: true,
        sight_checks: 0,
    };
    assert_eq!(nearest_node(&mut g, [0, 8, 0], NEAREST_RANGE2), 0);
    assert_eq!(
        nearest_node(&mut g, [0, 16, 0], NEAREST_RANGE2),
        0,
        "tie: lower index"
    );
    assert_eq!(nearest_node(&mut g, [0, 25, 0], NEAREST_RANGE2), 1);
    assert_eq!(nearest_node(&mut g, [0, 0, 0], 65), 0);
    assert_eq!(
        nearest_node(&mut g, [0, 0, 0], 64),
        NODE_NONE,
        "range is strict"
    );
}

#[test]
fn a_route_exists_between_the_nodes_nearest_each_end_and_starts_at_the_source_node() {
    let mut g = corridor();
    let mut cache = RouteCache::EMPTY;
    assert!(route_available(
        &mut g,
        &mut cache,
        [10, 0, 10],
        [230, 0, 10]
    ));
    assert_eq!(
        cache,
        RouteCache {
            src: 0,
            dst: 3,
            next: NODE_NONE
        }
    );
    // No graph at all.
    let mut empty = Graph {
        nodes: vec![],
        links: vec![],
        walls: vec![],
        cooked: true,
        sight_checks: 0,
    };
    let mut cache = RouteCache {
        src: 1,
        dst: 2,
        next: 3,
    };
    assert!(!route_available(&mut empty, &mut cache, [0; 3], [9, 0, 9]));
    assert_eq!(cache, RouteCache::EMPTY);
    // No cooked route table: no hop, no route.
    let mut g = corridor();
    g.cooked = false;
    let mut cache = RouteCache::EMPTY;
    assert!(!route_available(
        &mut g,
        &mut cache,
        [10, 0, 10],
        [230, 0, 10]
    ));
    // Same node at both ends counts as a route.
    let mut g = corridor();
    let mut cache = RouteCache::EMPTY;
    assert!(route_available(
        &mut g,
        &mut cache,
        [10, 0, 10],
        [10, 0, 20]
    ));
}

#[test]
fn the_first_shortcut_skips_the_source_node_when_the_next_is_walkable_else_tries_the_halfway_point()
{
    let mut g = corridor();
    // From beside node 0, node 1 is directly walkable: no waypoint, hop kept.
    let mut cache = RouteCache {
        src: 0,
        dst: 3,
        next: NODE_NONE,
    };
    assert_eq!(simplified_entry(&mut g, &mut cache, [10, 0, 10]), None);
    assert_eq!(cache.next, 1);
    // A wall hides node 2 from the start but not the halfway point to it.
    let mut g = corridor();
    g.walls.push([160, 280, 200, 320]);
    let mut cache = RouteCache {
        src: 1,
        dst: 3,
        next: NODE_NONE,
    };
    assert_eq!(
        simplified_entry(&mut g, &mut cache, [-20, 0, 300]),
        Some([120, 0, 300])
    );
    assert_eq!(cache.next, 2);
    // Neither: wait at the source node.
    let mut g = corridor();
    g.walls.push([-60, 100, 60, 120]);
    let mut cache = RouteCache {
        src: 0,
        dst: 3,
        next: NODE_NONE,
    };
    assert_eq!(simplified_entry(&mut g, &mut cache, [10, 0, 10]), None);
    assert_eq!(
        cache,
        RouteCache {
            src: 0,
            dst: 3,
            next: NODE_NONE
        }
    );
}

#[test]
fn changing_the_source_forgets_a_hop_computed_for_the_old_one() {
    let mut c = RouteCache {
        src: 1,
        dst: 3,
        next: 2,
    };
    c.set_src(1);
    assert_eq!(
        c,
        RouteCache {
            src: 1,
            dst: 3,
            next: 2
        }
    );
    c.set_src(0);
    assert_eq!(
        c,
        RouteCache {
            src: 0,
            dst: NODE_NONE,
            next: NODE_NONE
        }
    );
}

#[test]
fn a_scripted_route_that_cannot_continue_falls_back_to_the_goal_unless_detouring() {
    let mut g = corridor();
    g.cooked = false;
    let goal = [230, 0, 10];
    let mut f = Follower {
        pos: [2, 0, 2],
        eye: [2, 64, 2],
        routed: true,
        detour: false,
    };
    let mut cache = RouteCache {
        src: 0,
        dst: 3,
        next: NODE_NONE,
    };
    assert_eq!(
        waypoint_towards(&mut g, &mut cache, &mut f, goal),
        Some(goal)
    );
    assert!(!f.routed);
    assert_eq!(cache, RouteCache::EMPTY);
    let mut f = Follower {
        routed: true,
        detour: true,
        ..f
    };
    let mut cache = RouteCache {
        src: 0,
        dst: 3,
        next: NODE_NONE,
    };
    assert_eq!(waypoint_towards(&mut g, &mut cache, &mut f, goal), None);
    assert!(f.routed);
}

/// Walk `f` toward each returned waypoint by up to `stride` units per call
/// and log every distinct waypoint with the call it first appeared on.
fn follow(
    g: &mut Graph,
    f: &mut Follower,
    cache: &mut RouteCache,
    goal: [i32; 3],
    stride: i32,
    calls: usize,
) -> Vec<(usize, Option<[i32; 3]>, bool, RouteCache)> {
    let mut log: Vec<(usize, Option<[i32; 3]>, bool, RouteCache)> = Vec::new();
    for call in 0..calls {
        let wp = waypoint_towards(g, cache, f, goal);
        if log.last().map(|l| (l.1, l.2, l.3)) != Some((wp, f.routed, *cache)) {
            log.push((call, wp, f.routed, *cache));
        }
        let Some(w) = wp else { break };
        if w == goal {
            // The caller walks the last leg straight.
            break;
        }
        let (dx, dz) = (w[0] - f.pos[0], w[2] - f.pos[2]);
        let len = ((dx * dx + dz * dz) as f64).sqrt() as i32;
        if len <= stride {
            f.pos = [w[0], f.pos[1], w[2]];
        } else {
            f.pos[0] += dx * stride / len;
            f.pos[2] += dz * stride / len;
        }
        f.eye = [f.pos[0], f.pos[1] + 64, f.pos[2]];
        if f.pos == [goal[0], f.pos[1], goal[2]] {
            break;
        }
    }
    log
}

/// Recorded from ac83da7 behaviour. A scripted actor that chose the graph at
/// assignment walks the U-shaped corridor from beside node 0 to a goal beside
/// node 3, 20 units per call. At node 2 the goal is directly walkable, so the
/// route is dropped there and the goal handed back for a straight last leg.
#[test]
fn golden_scripted_route_around_the_wall() {
    let mut g = corridor();
    let goal = [230, 0, 10];
    let mut cache = RouteCache::EMPTY;
    assert!(route_available(&mut g, &mut cache, [10, 0, 10], goal));
    let mut f = Follower {
        pos: [10, 0, 10],
        eye: [10, 64, 10],
        routed: true,
        detour: false,
    };
    let log = follow(&mut g, &mut f, &mut cache, goal, 20, 200);
    let expected: &[(usize, Option<[i32; 3]>, bool, RouteCache)] = &[
        (
            0,
            Some([0, 0, 0]),
            true,
            RouteCache {
                src: 0,
                dst: 3,
                next: 255,
            },
        ),
        (
            1,
            Some([0, 0, 300]),
            true,
            RouteCache {
                src: 0,
                dst: 3,
                next: 1,
            },
        ),
        (
            16,
            Some([240, 0, 300]),
            true,
            RouteCache {
                src: 1,
                dst: 3,
                next: 2,
            },
        ),
        (
            28,
            Some([230, 0, 10]),
            false,
            RouteCache {
                src: 255,
                dst: 255,
                next: 255,
            },
        ),
    ];
    assert_eq!(log, expected, "{log:#?}");
}

/// Recorded from ac83da7 behaviour. An ordinary (unscripted) monster blocked
/// beside node 0 asks for a waypoint toward a point beyond the wall: it
/// finds its own nearest node, steers through the graph and reaches its
/// last node within 48 units, 20 units per call; at the destination node it is
/// handed the goal.
#[test]
fn golden_unscripted_follow() {
    let mut g = corridor();
    let goal = [230, 0, 10];
    let mut cache = RouteCache::EMPTY;
    let mut f = Follower {
        pos: [30, 0, 60],
        eye: [30, 64, 60],
        routed: false,
        detour: false,
    };
    let log = follow(&mut g, &mut f, &mut cache, goal, 20, 200);
    let expected: &[(usize, Option<[i32; 3]>, bool, RouteCache)] = &[
        (
            0,
            Some([0, 0, 0]),
            false,
            RouteCache {
                src: 0,
                dst: 255,
                next: 255,
            },
        ),
        (
            2,
            Some([0, 0, 300]),
            false,
            RouteCache {
                src: 0,
                dst: 3,
                next: 1,
            },
        ),
        (
            14,
            Some([240, 0, 300]),
            false,
            RouteCache {
                src: 1,
                dst: 3,
                next: 2,
            },
        ),
        (
            24,
            Some([240, 0, 0]),
            false,
            RouteCache {
                src: 2,
                dst: 3,
                next: 3,
            },
        ),
        (
            37,
            Some([230, 0, 10]),
            false,
            RouteCache {
                src: 255,
                dst: 255,
                next: 255,
            },
        ),
    ];
    assert_eq!(log, expected, "{log:#?}");
}
