//! Mortar field (HV-07): drop point per mode, shell count and spread,
//! landing times and what each landing does.

use crate::behaviour::setpiece_rng::Lcg;
use crate::mortar_logic::{FieldUse, MortarState, MortarWorld, MAX_SHELLS};
use crate::setpiece_math::TraceHit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    Beam([i32; 3], [i32; 3]),
    Explode([i32; 3], u8, i32, bool),
    Shake(u16, u16),
}

#[derive(Clone, Copy)]
enum Pick {
    Low,
    High,
    Game(Lcg),
}

struct World {
    pick: Pick,
    draws: Vec<(u32, u32)>,
    controllers: [Option<i32>; 2],
    floor_y: Option<i32>,
    traces: Vec<([i32; 3], [i32; 3])>,
    events: Vec<(u16, Ev)>,
    now: u16,
}

impl World {
    fn new(pick: Pick) -> Self {
        Self {
            pick,
            draws: Vec::new(),
            controllers: [None; 2],
            floor_y: None,
            traces: Vec::new(),
            events: Vec::new(),
            now: 0,
        }
    }
}

impl MortarWorld for World {
    fn random_below(&mut self, n: u32) -> u32 {
        let v = match &mut self.pick {
            Pick::Low => 0,
            Pick::High => n.saturating_sub(1),
            Pick::Game(r) => r.below(n),
        };
        self.draws.push((n, v));
        v
    }
    fn controller(&mut self, axis: usize) -> Option<i32> {
        self.controllers[axis]
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        self.traces.push((from, to));
        let y = self.floor_y?;
        if from[1] > y && to[1] <= y {
            let frac = (from[1] - y) * 4096 / (from[1] - to[1]);
            Some(TraceHit {
                frac,
                pos: [from[0], y, from[2]],
                normal: [0, 4096, 0],
            })
        } else {
            None
        }
    }
    fn beam(&mut self, from: [i32; 3], to: [i32; 3]) {
        self.events.push((self.now, Ev::Beam(from, to)));
    }
    fn explode(&mut self, at: [i32; 3], damage: u8, radius: i32, by_player: bool) {
        self.events
            .push((self.now, Ev::Explode(at, damage, radius, by_player)));
    }
    fn set_shake(&mut self, amplitude: u16, ticks: u16) {
        self.events.push((self.now, Ev::Shake(amplitude, ticks)));
    }
}

fn field(mode: u8, count: u8, spread: i32) -> FieldUse {
    FieldUse {
        mins: [-200, 0, 100],
        maxs: [200, 300, 500],
        mode,
        count,
        spread,
        by_player: false,
        player_pos: [40, 0, 260],
    }
}

/// Run ticks `from..=to`, recording what lands and when.
fn run(s: &mut MortarState, w: &mut World, from: u16, to: u16, player: [i32; 3]) {
    let mut t = from;
    loop {
        w.now = t;
        s.tick(t, player, w);
        if t == to {
            break;
        }
        t = t.wrapping_add(1);
    }
}

#[test]
fn the_first_shell_lands_two_and_a_half_seconds_after_the_use() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Low);
    s.field_use(&field(0, 1, 0), 1000, &mut w);
    assert_eq!(s.pending().map(|(_, at)| at).collect::<Vec<_>>(), [1050]);
}

#[test]
fn each_next_shell_follows_two_to_five_tenths_of_a_second_later() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Low);
    s.field_use(&field(0, 3, 0), 1000, &mut w);
    let fast: Vec<u16> = s.pending().map(|(_, at)| at).collect();
    assert_eq!(fast, [1050, 1054, 1058]);
    let mut s = MortarState::new();
    let mut w = World::new(Pick::High);
    s.field_use(&field(0, 3, 0), 1000, &mut w);
    let slow: Vec<u16> = s.pending().map(|(_, at)| at).collect();
    assert_eq!(slow, [1050, 1060, 1070]);
}

#[test]
fn random_mode_drops_from_a_spot_in_the_footprint_at_its_top() {
    for (pick, x, z) in [(Pick::Low, -200, 100), (Pick::High, 200, 500)] {
        let mut s = MortarState::new();
        let mut w = World::new(pick);
        s.field_use(&field(0, 1, 0), 0, &mut w);
        assert_eq!(w.traces, [([x, 300, z], [x, 300 - 4096, z])]);
    }
}

#[test]
fn player_mode_drops_over_the_activating_player_only() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Low);
    let mut u = field(1, 1, 0);
    u.by_player = true;
    s.field_use(&u, 0, &mut w);
    assert_eq!(w.traces[0].0, [40, 300, 260]);
    // A trigger that was not the player leaves the random spot.
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Low);
    s.field_use(&field(1, 1, 0), 0, &mut w);
    assert_eq!(w.traces[0].0, [-200, 300, 100]);
}

#[test]
fn controller_mode_places_the_point_across_the_field() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Low);
    w.controllers = [Some(4096), Some(1024)];
    s.field_use(&field(2, 1, 0), 0, &mut w);
    assert_eq!(w.traces[0].0, [200, 300, 200]);
    // A missing controller leaves that axis at the random spot.
    let mut s = MortarState::new();
    let mut w = World::new(Pick::High);
    w.controllers = [Some(2048), None];
    s.field_use(&field(2, 1, 0), 0, &mut w);
    assert_eq!(w.traces[0].0, [0, 300, 500]);
}

#[test]
fn shells_scatter_within_the_spread_and_land_on_the_ground_below() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Low);
    w.floor_y = Some(-64);
    s.field_use(&field(0, 2, 48), 0, &mut w);
    let spots: Vec<[i32; 3]> = s.pending().map(|(p, _)| p).collect();
    assert_eq!(spots, [[-248, -64, 52], [-248, -64, 52]]);
    let mut s = MortarState::new();
    let mut w = World::new(Pick::High);
    s.field_use(&field(0, 1, 48), 0, &mut w);
    // No ground within 4096: the shell lands at the end of the drop.
    let spots: Vec<[i32; 3]> = s.pending().map(|(p, _)| p).collect();
    assert_eq!(spots, [[248, 300 - 4096, 548]]);
}

#[test]
fn a_landing_shell_shows_a_beam_blasts_200_out_to_500_and_shakes() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Low);
    w.floor_y = Some(0);
    s.field_use(&field(0, 1, 0), 10, &mut w);
    run(&mut s, &mut w, 10, 70, [-200, 0, 100]);
    assert_eq!(
        w.events,
        [
            (60, Ev::Beam([-200, 0, 100], [-200, 1024, 100])),
            (60, Ev::Explode([-200, 0, 100], 200, 500, false)),
            (60, Ev::Shake(25, 20)),
        ]
    );
    assert_eq!(s.pending().count(), 0);
}

#[test]
fn the_shake_fades_linearly_and_stops_at_750_units() {
    for (d, want) in [
        (0, Some(25)),
        (375, Some(12)),
        (720, Some(1)),
        (749, Some(0)),
        (750, None),
    ] {
        let mut s = MortarState::new();
        let mut w = World::new(Pick::Low);
        w.floor_y = Some(0);
        s.field_use(&field(0, 1, 0), 0, &mut w);
        run(&mut s, &mut w, 0, 50, [-200 + d, 0, 100]);
        let shake = w.events.iter().find_map(|e| match e.1 {
            Ev::Shake(a, t) => {
                assert_eq!(t, 20);
                Some(a)
            }
            _ => None,
        });
        assert_eq!(shake, want, "player {d} units away");
    }
}

#[test]
fn a_shell_from_the_players_use_is_credited_to_the_player() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Low);
    let mut u = field(0, 1, 0);
    u.by_player = true;
    s.field_use(&u, 0, &mut w);
    run(&mut s, &mut w, 0, 50, [5000, 0, 0]);
    assert!(w
        .events
        .iter()
        .any(|e| matches!(e.1, Ev::Explode(_, 200, 500, true))));
}

#[test]
fn at_most_twelve_shells_fall_at_once_but_timing_draws_continue() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Low);
    s.field_use(&field(0, 15, 0), 0, &mut w);
    assert_eq!(s.pending().count(), MAX_SHELLS);
    // Two spot draws, then two spread draws and one delay draw per shell.
    assert_eq!(w.draws.len(), 2 + 15 * 3);
    assert_eq!(w.traces.len(), 15);
}

#[test]
fn reset_forgets_falling_shells() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Low);
    s.field_use(&field(0, 4, 0), 0, &mut w);
    s.reset();
    assert_eq!(s.pending().count(), 0);
    run(&mut s, &mut w, 0, 200, [0; 3]);
    assert!(w.events.is_empty());
}

/// Recorded from ac83da7 behaviour: a random-mode use of five shells with a
/// spread of 64 over a floor at y = -100, drawn from the game's generator,
/// then every landing up to 5 s later with the player standing in the field.
#[test]
fn golden_random_volley_with_the_game_generator() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Game(Lcg(Lcg::GAME_SEED)));
    w.floor_y = Some(-100);
    s.field_use(&field(0, 5, 64), 300, &mut w);
    assert_eq!(
        w.draws,
        [
            (401, 296),
            (401, 19),
            (129, 28),
            (129, 16),
            (7, 4),
            (129, 89),
            (129, 65),
            (7, 5),
            (129, 125),
            (129, 44),
            (7, 4),
            (129, 19),
            (129, 121),
            (7, 6),
            (129, 117),
            (129, 79),
            (7, 1)
        ]
    );
    assert_eq!(
        s.pending().collect::<Vec<_>>(),
        [
            ([60, -100, 71], 350),
            ([121, -100, 120], 358),
            ([157, -100, 99], 367),
            ([51, -100, 176], 375),
            ([149, -100, 134], 385)
        ]
    );
    run(&mut s, &mut w, 300, 400, [0, -100, 300]);
    assert_eq!(
        w.events,
        [
            (350, Ev::Beam([60, -100, 71], [60, 924, 71])),
            (350, Ev::Explode([60, -100, 71], 200, 500, false)),
            (350, Ev::Shake(17, 20)),
            (358, Ev::Beam([121, -100, 120], [121, 924, 120])),
            (358, Ev::Explode([121, -100, 120], 200, 500, false)),
            (358, Ev::Shake(17, 20)),
            (367, Ev::Beam([157, -100, 99], [157, 924, 99])),
            (367, Ev::Explode([157, -100, 99], 200, 500, false)),
            (367, Ev::Shake(16, 20)),
            (375, Ev::Beam([51, -100, 176], [51, 924, 176])),
            (375, Ev::Explode([51, -100, 176], 200, 500, false)),
            (375, Ev::Shake(20, 20)),
            (385, Ev::Beam([149, -100, 134], [149, 924, 134])),
            (385, Ev::Explode([149, -100, 134], 200, 500, false)),
            (385, Ev::Shake(17, 20))
        ]
    );
}

/// Recorded from ac83da7 behaviour: a use just before the tick counter
/// wraps, so the volley lands across the wrap (a landing due on tick 0 is
/// moved to tick 1).
#[test]
fn golden_volley_across_the_tick_wrap() {
    let mut s = MortarState::new();
    let mut w = World::new(Pick::Game(Lcg(7)));
    w.floor_y = Some(0);
    let mut u = field(1, 3, 16);
    u.by_player = true;
    s.field_use(&u, 65486, &mut w);
    assert_eq!(
        w.draws,
        [
            (401, 17),
            (401, 240),
            (33, 26),
            (33, 20),
            (7, 5),
            (33, 23),
            (33, 24),
            (7, 2),
            (33, 18),
            (33, 21),
            (7, 3)
        ]
    );
    assert_eq!(
        s.pending().collect::<Vec<_>>(),
        [([50, 0, 264], 1), ([47, 0, 268], 9), ([42, 0, 265], 15)]
    );
    run(&mut s, &mut w, 65486, 40, [40, 0, 260]);
    assert_eq!(
        w.events,
        [
            (1, Ev::Beam([50, 0, 264], [50, 1024, 264])),
            (1, Ev::Explode([50, 0, 264], 200, 500, true)),
            (1, Ev::Shake(24, 20)),
            (9, Ev::Beam([47, 0, 268], [47, 1024, 268])),
            (9, Ev::Explode([47, 0, 268], 200, 500, true)),
            (9, Ev::Shake(24, 20)),
            (15, Ev::Beam([42, 0, 265], [42, 1024, 265])),
            (15, Ev::Explode([42, 0, 265], 200, 500, true)),
            (15, Ev::Shake(24, 20))
        ]
    );
}
