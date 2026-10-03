//! Node-graph navigation over the retail `.nod` graph: the nearest usable
//! node, whether a graph route exists between two points, the first useful
//! shortcut into it, and the next point to steer for while following it.
//!
//! The runtime packs each actor's [`RouteCache`] into spare bytes of an
//! existing per-prop array; the game adaptor unpacks it, calls one query here
//! and packs it back. Map data and traces come through [`NavGraph`].
//! Coordinates are Y-up world units.

/// No node.
pub const NODE_NONE: u8 = 255;
/// Nodes further than this (squared) are never "nearest".
pub const NEAREST_RANGE2: i32 = 1024 * 1024;
/// An unscripted actor has reached a node within this planar radius (squared).
pub const NODE_REACHED_RANGE2: i32 = 48 * 48;
/// A scripted route node is reached within the 8-unit plant radius (squared).
pub const SCRIPT_NODE_REACHED_RANGE2: i32 = 8 * 8;
/// Largest floor difference to a node an actor will steer for directly.
pub const VERTICAL_MAX: i32 = 160;
/// Height of a land node's probe point above the node.
pub const NODE_PEEK_HEIGHT: i32 = 8;

#[inline]
fn dist2_xz(a: [i32; 3], b: [i32; 3]) -> i32 {
    let dx = b[0] - a[0];
    let dz = b[2] - a[2];
    dx * dx + dz * dz
}

/// One actor's remembered route: the source node, the destination node and
/// the next hop toward it (any may be [`NODE_NONE`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteCache {
    pub src: u8,
    pub dst: u8,
    pub next: u8,
}

impl RouteCache {
    pub const EMPTY: Self = Self {
        src: NODE_NONE,
        dst: NODE_NONE,
        next: NODE_NONE,
    };

    /// Change the source; a hop computed for another source is forgotten.
    #[inline]
    pub fn set_src(&mut self, src: u8) {
        if self.src != src {
            self.dst = NODE_NONE;
            self.next = NODE_NONE;
        }
        self.src = src;
    }

    #[inline]
    pub fn set_route(&mut self, src: u8, dst: u8, next: u8) {
        self.src = src;
        self.dst = dst;
        self.next = next;
    }
}

/// The map's node graph and the traces navigation needs.
pub trait NavGraph {
    /// Number of usable nodes.
    fn node_count(&self) -> usize;
    fn node_pos(&self, node: usize) -> [i32; 3];
    /// A land (walking) node.
    fn is_land(&self, node: usize) -> bool;
    /// The cooked next hop from `src` toward `dst`, or `None` when the map
    /// has no cooked route table.
    fn cooked_next(&self, src: usize, dst: usize) -> Option<usize>;
    /// Point line of sight.
    fn line_clear(&mut self, from: [i32; 3], to: [i32; 3]) -> bool;
    /// The actor's hull can walk straight from `from` to `to`.
    fn walkable(&mut self, from: [i32; 3], to: [i32; 3]) -> bool;
}

/// Nearest land node to `pos` whose probe point (8 units above it) is in
/// sight, ranked in 3-D; ties go to the lower index.
pub fn nearest_node(g: &mut dyn NavGraph, pos: [i32; 3], max_d2: i32) -> u8 {
    let n = g.node_count();
    let mut best = NODE_NONE;
    let mut best_d2 = max_d2;
    let mut i = 0usize;
    while i < n {
        let node = g.node_pos(i);
        if g.is_land(i) {
            let peek = [node[0], node[1] + NODE_PEEK_HEIGHT, node[2]];
            let dy = peek[1] - pos[1];
            let d2 = dist2_xz(pos, peek) + dy * dy;
            if d2 < best_d2 && g.line_clear(pos, peek) {
                best = i as u8;
                best_d2 = d2;
            }
        }
        i += 1;
    }
    best
}

/// Nearest land node within [`VERTICAL_MAX`] of `pos`, ranked by planar
/// distance, whose column is in sight from `from` at `from`'s height. The
/// cached source is re-verified first and kept unless a nearer node (or an
/// equally near lower-indexed one) is also in sight. Records the result as
/// the cache's source.
pub fn nearest_reachable(
    g: &mut dyn NavGraph,
    cache: &mut RouteCache,
    from: [i32; 3],
    pos: [i32; 3],
    max_d2: i32,
) -> u8 {
    let n = g.node_count();
    let cached = cache.src as usize;
    if cached < n && g.is_land(cached) {
        let node = g.node_pos(cached);
        let dy = (node[1] - pos[1]).abs();
        let d2 = dist2_xz(pos, node);
        if dy <= VERTICAL_MAX && d2 < max_d2 {
            let to = [node[0], from[1], node[2]];
            if g.line_clear(from, to) {
                let mut best = cached as u8;
                let mut best_d2 = d2;
                let mut i = 0usize;
                while i < n {
                    if i != cached && g.is_land(i) {
                        let candidate = g.node_pos(i);
                        let candidate_dy = (candidate[1] - pos[1]).abs();
                        if candidate_dy <= VERTICAL_MAX {
                            let candidate_d2 = dist2_xz(pos, candidate);
                            let can_beat = candidate_d2 < best_d2
                                || (candidate_d2 == best_d2 && i < best as usize);
                            if can_beat {
                                let candidate_to = [candidate[0], from[1], candidate[2]];
                                if g.line_clear(from, candidate_to) {
                                    best = i as u8;
                                    best_d2 = candidate_d2;
                                }
                            }
                        }
                    }
                    i += 1;
                }
                cache.set_src(best);
                return best;
            }
        }
    }
    let mut best = NODE_NONE;
    let mut best_d2 = max_d2;
    let mut i = 0usize;
    while i < n {
        let node = g.node_pos(i);
        let dy = (node[1] - pos[1]).abs();
        if g.is_land(i) && dy <= VERTICAL_MAX {
            let d2 = dist2_xz(pos, node);
            let to = [node[0], from[1], node[2]];
            if d2 < best_d2 && g.line_clear(from, to) {
                best = i as u8;
                best_d2 = d2;
            }
        }
        i += 1;
    }
    cache.set_src(best);
    best
}

/// Next hop from `src` toward `dst`, remembered in the cache.
pub fn next_node(g: &mut dyn NavGraph, cache: &mut RouteCache, src: u8, dst: u8) -> u8 {
    let n = g.node_count();
    let src_i = src as usize;
    let dst_i = dst as usize;
    if src_i >= n || dst_i >= n {
        cache.set_route(src, dst, NODE_NONE);
        return NODE_NONE;
    }
    if cache.src == src && cache.dst == dst {
        let cached = cache.next;
        // NONE also marks a route validated at assignment whose first hop is
        // only taken once the actor reaches the source node.
        if cached != NODE_NONE {
            return cached;
        }
    }
    if src == dst {
        cache.set_route(src, dst, src);
        return src;
    }
    let next = match g.cooked_next(src_i, dst_i) {
        Some(exact) if exact != src_i && exact < n => exact as u8,
        _ => NODE_NONE,
    };
    cache.set_route(src, dst, next);
    next
}

/// Whether the graph connects the node nearest `pos` to the node nearest
/// `goal`. On success the cache holds both nodes with no hop yet: the actor
/// first walks to the source node itself. On failure the cache is emptied.
pub fn route_available(
    g: &mut dyn NavGraph,
    cache: &mut RouteCache,
    pos: [i32; 3],
    goal: [i32; 3],
) -> bool {
    let src = nearest_node(g, pos, NEAREST_RANGE2);
    if src == NODE_NONE {
        *cache = RouteCache::EMPTY;
        return false;
    }
    let dst = nearest_node(g, goal, NEAREST_RANGE2);
    if dst == NODE_NONE {
        *cache = RouteCache::EMPTY;
        return false;
    }
    if next_node(g, cache, src, dst) == NODE_NONE {
        *cache = RouteCache::EMPTY;
        return false;
    }
    cache.set_route(src, dst, NODE_NONE);
    true
}

/// The first shortcut into a cached route from `start`: `None` with the hop
/// kept when the next node is directly walkable; the midpoint between the
/// source and next node when that is walkable instead; otherwise `None`
/// with the route left waiting at its source node.
pub fn simplified_entry(
    g: &mut dyn NavGraph,
    cache: &mut RouteCache,
    start: [i32; 3],
) -> Option<[i32; 3]> {
    let src = cache.src;
    let dst = cache.dst;
    let n = g.node_count();
    if src as usize >= n || dst as usize >= n {
        return None;
    }
    let next = next_node(g, cache, src, dst);
    if next == NODE_NONE || next as usize >= n {
        cache.set_route(src, dst, NODE_NONE);
        return None;
    }
    let next_pos = g.node_pos(next as usize);
    if g.walkable(start, next_pos) {
        return None;
    }
    let src_pos = g.node_pos(src as usize);
    let midpoint = [
        (src_pos[0] + next_pos[0]) / 2,
        (src_pos[1] + next_pos[1]) / 2,
        (src_pos[2] + next_pos[2]) / 2,
    ];
    if g.walkable(start, midpoint) {
        return Some(midpoint);
    }
    cache.set_route(src, dst, NODE_NONE);
    None
}

/// An actor following (or wanting) a graph route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Follower {
    pub pos: [i32; 3],
    /// Sight origin for the nearest-node search.
    pub eye: [i32; 3],
    /// A scripted move that chose the graph route.
    pub routed: bool,
    /// A scripted move heading for an intermediate waypoint.
    pub detour: bool,
}

fn fail_open(f: &mut Follower, cache: &mut RouteCache, goal: [i32; 3]) -> Option<[i32; 3]> {
    if f.routed && !f.detour {
        f.routed = false;
        *cache = RouteCache::EMPTY;
        Some(goal)
    } else {
        None
    }
}

/// The point to steer for on the way to `goal`, or `None` when the graph
/// offers nothing. A scripted route that ends (or cannot continue) hands
/// back the goal itself and clears `routed`.
pub fn waypoint_towards(
    g: &mut dyn NavGraph,
    cache: &mut RouteCache,
    f: &mut Follower,
    goal: [i32; 3],
) -> Option<[i32; 3]> {
    let n = g.node_count();
    if n == 0 {
        return None;
    }
    let pos = f.pos;
    let reached_range2 = if f.routed {
        SCRIPT_NODE_REACHED_RANGE2
    } else {
        NODE_REACHED_RANGE2
    };

    // Keep following the remembered hop until it is reached.
    let cached_next = cache.next as usize;
    if cached_next < n && g.is_land(cached_next) {
        let next_pos = g.node_pos(cached_next);
        let next_d2 = dist2_xz(pos, next_pos);
        let scripted = f.routed;
        if next_d2 > reached_range2
            && (scripted
                || ((pos[1] - next_pos[1]).abs() <= VERTICAL_MAX && next_d2 < NEAREST_RANGE2))
        {
            return Some(next_pos);
        }
        if next_d2 <= reached_range2 && (pos[1] - next_pos[1]).abs() <= VERTICAL_MAX {
            let dst = cache.dst;
            if dst as usize >= n || !g.is_land(dst as usize) {
                return fail_open(f, cache, goal);
            }
            // At the destination node, or once the goal itself is walkable,
            // a scripted route hands the last leg back to the straight mover.
            if cached_next as u8 == dst || (f.routed && g.walkable(pos, goal)) {
                f.routed = false;
                *cache = RouteCache::EMPTY;
                return Some(goal);
            }
            let following = next_node(g, cache, cached_next as u8, dst);
            return if following == NODE_NONE {
                fail_open(f, cache, goal)
            } else {
                Some(g.node_pos(following as usize))
            };
        }
    }

    // No hop yet: approach the remembered source node first.
    let cached_src = cache.src as usize;
    let mut src = NODE_NONE;
    if cached_src < n && g.is_land(cached_src) {
        let src_pos = g.node_pos(cached_src);
        let src_d2 = dist2_xz(pos, src_pos);
        if (pos[1] - src_pos[1]).abs() <= VERTICAL_MAX && src_d2 < NEAREST_RANGE2 {
            if src_d2 > reached_range2 {
                return Some(src_pos);
            }
            src = cached_src as u8;
        }
    }
    if src == NODE_NONE {
        src = nearest_reachable(g, cache, f.eye, pos, NEAREST_RANGE2);
    }
    if src == NODE_NONE {
        return fail_open(f, cache, goal);
    }
    let cached_dst = cache.dst;
    let dst = if f.routed && (cached_dst as usize) < n && g.is_land(cached_dst as usize) {
        cached_dst
    } else {
        nearest_node(g, goal, NEAREST_RANGE2)
    };
    if dst == NODE_NONE {
        return fail_open(f, cache, goal);
    }

    let src_pos = g.node_pos(src as usize);
    if dist2_xz(pos, src_pos) > reached_range2 || (pos[1] - src_pos[1]).abs() > VERTICAL_MAX {
        return Some(src_pos);
    }
    if src == dst {
        if f.routed {
            f.routed = false;
            *cache = RouteCache::EMPTY;
        }
        return Some(goal);
    }

    let next = next_node(g, cache, src, dst);
    if next == NODE_NONE {
        fail_open(f, cache, goal)
    } else {
        Some(g.node_pos(next as usize))
    }
}
