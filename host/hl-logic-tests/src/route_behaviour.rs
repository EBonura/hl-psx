//! Next-hop decoding of the retail node graph's compressed route table.
//!
//! Each node stores a byte stream that walks the destination indices in
//! order. A negative byte `-n` says the next `n` destinations are reached
//! directly (the next hop is the destination itself). A non-negative byte `n`
//! followed by a signed byte `d` says the next `n + 1` destinations all share
//! the next hop `(current + d) mod node_count`. A destination past the end of
//! the table is unreachable, reported as the current node.

use crate::map::Map;
use crate::map_fixture::{MapFixture, NavNodeFixture};

fn graph(routes: &[&[u8]]) -> Map {
    MapFixture {
        brushes: vec![],
        spawn: [0, 36, 0],
        nav: routes
            .iter()
            .enumerate()
            .map(|(i, r)| NavNodeFixture {
                origin: [i as i32 * 128, 0, 0],
                node_type: 1,
                routes: r.to_vec(),
            })
            .collect(),
    }
    .load()
}

const fn neg(n: i8) -> u8 {
    (-n) as u8
}

const fn delta(d: i8) -> u8 {
    d as u8
}

/// A five-node corridor 0-1-2-3-4, each node linked to its neighbours.
fn corridor() -> Map {
    graph(&[
        // node 0: dests 0,1 direct; 2,3,4 via +1
        &[neg(2), 2, delta(1)],
        // node 1: 0,1,2 direct; 3,4 via +1
        &[neg(3), 1, delta(1)],
        // node 2: 0,1 via -1; 2,3 direct; 4 via +1
        &[1, delta(-1), neg(2), 0, delta(1)],
        // node 3: 0,1,2 via -1; 3,4 direct
        &[2, delta(-1), neg(2)],
        // node 4: 0..=2 via -1 (wrapping as +4); 3,4 direct
        &[2, delta(4), neg(2)],
    ])
}

#[test]
fn the_graph_is_flagged_as_carrying_exact_routes() {
    let map = corridor();
    assert!(map.nav_has_exact_routes());
    assert_eq!(map.n_nav, 5);
}

#[test]
fn neighbours_and_self_are_reached_directly() {
    let map = corridor();
    for i in 0..5usize {
        assert_eq!(map.nav_route_next(i, i), i, "node {i} to itself");
        if i > 0 {
            assert_eq!(map.nav_route_next(i, i - 1), i - 1);
        }
        if i < 4 {
            assert_eq!(map.nav_route_next(i, i + 1), i + 1);
        }
    }
}

#[test]
fn far_destinations_share_the_run_next_hop() {
    let map = corridor();
    assert_eq!(map.nav_route_next(0, 4), 1);
    assert_eq!(map.nav_route_next(0, 3), 1);
    assert_eq!(map.nav_route_next(2, 0), 1);
    assert_eq!(map.nav_route_next(2, 4), 3);
    assert_eq!(map.nav_route_next(3, 0), 2);
}

#[test]
fn next_hop_offsets_wrap_around_the_node_count() {
    let map = corridor();
    // Node 4 encodes "one back" as +4 modulo five nodes.
    assert_eq!(map.nav_route_next(4, 0), 3);
    assert_eq!(map.nav_route_next(4, 2), 3);
}

#[test]
fn following_next_hops_always_reaches_the_destination() {
    let map = corridor();
    for src in 0..5usize {
        for dst in 0..5usize {
            let mut at = src;
            let mut hops = 0;
            while at != dst {
                let next = map.nav_route_next(at, dst);
                assert_ne!(next, at, "{src}->{dst} stalled at {at}");
                at = next;
                hops += 1;
                assert!(hops <= 4, "{src}->{dst} loops");
            }
            assert_eq!(hops, src.abs_diff(dst), "{src}->{dst} is shortest");
        }
    }
}

#[test]
fn running_off_the_end_of_the_table_is_unreachable() {
    // Streams sit end to end with no per-node length; a well-formed table
    // covers every destination. Here the last node only describes
    // destination 0, so asking it for 1 or 2 runs off the table.
    let map = graph(&[&[neg(3)], &[neg(3)], &[neg(1)]]);
    assert_eq!(map.nav_route_next(0, 2), 2);
    assert_eq!(map.nav_route_next(2, 0), 0);
    assert_eq!(map.nav_route_next(2, 1), 2, "past the end of the table");
    assert_eq!(map.nav_route_next(2, 2), 2);
}

#[test]
fn long_runs_cover_many_destinations() {
    // Twelve nodes; node 0 reaches 1 directly and everything else via 1,
    // written as one direct entry for itself, one for node 1, then a run of
    // ten sharing +1.
    let mut routes: Vec<Vec<u8>> = vec![vec![neg(2), 9, delta(1)]];
    for _ in 1..12 {
        routes.push(vec![]);
    }
    let refs: Vec<&[u8]> = routes.iter().map(Vec::as_slice).collect();
    let map = graph(&refs);
    for dst in 2..12 {
        assert_eq!(map.nav_route_next(0, dst), 1, "dest {dst}");
    }
}
