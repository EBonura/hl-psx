//! Tau Cannon secondary fire: holding it spends one cell to start, spins up
//! for 0.5 s, spends one cell per 0.3 s until full charge at 4 s, fires by
//! force when the cells run out, hurts the player for 50 if held 10 s, and on
//! release fires 200 x the charge fraction, shoving the player back by five
//! times the damage horizontally. Under water it discharges harmlessly.

use crate::player_rules::{
    tau_secondary_tick, tau_shove, TauEvent, TauInput, TauState, TAU_CELL_TICKS,
    TAU_FULL_CHARGE_TICKS, TAU_OVERCHARGE_DAMAGE, TAU_OVERCHARGE_TICKS, TAU_SPIN_TICKS,
};

/// Hold secondary fire for the first `hold` ticks of `total`, starting with
/// `cells` and no cooldown. Between ticks the weapon cooldown counts down by
/// one, as the game's weapon clock does. Returns every non-idle event with
/// its tick, and the final state.
fn session(cells: u16, hold: usize, total: usize) -> (Vec<(usize, TauEvent)>, TauState) {
    let mut s = TauState {
        age: 0,
        cooldown: 0,
        cells,
    };
    let mut events = vec![];
    for t in 0..total {
        let e = tau_secondary_tick(
            &mut s,
            TauInput {
                selected: true,
                held: t < hold,
                underwater: false,
                busy: false,
            },
        );
        if e != TauEvent::Idle {
            events.push((t, e));
        }
        s.cooldown = s.cooldown.saturating_sub(1);
    }
    (events, s)
}

fn fire_damage(events: &[(usize, TauEvent)]) -> Option<(usize, u8)> {
    events.iter().find_map(|&(t, e)| match e {
        TauEvent::Fire { damage } => Some((t, damage)),
        _ => None,
    })
}

#[test]
fn timings_and_amounts() {
    assert_eq!(TAU_FULL_CHARGE_TICKS, 80);
    assert_eq!(TAU_SPIN_TICKS, 10);
    assert_eq!(TAU_CELL_TICKS, 6);
    assert_eq!(TAU_OVERCHARGE_TICKS, 200);
    assert_eq!(TAU_OVERCHARGE_DAMAGE, 50);
}

#[test]
fn starting_spends_a_cell_and_spins_after_half_a_second() {
    let (events, _) = session(100, 30, 30);
    assert_eq!(events[0], (0, TauEvent::Start));
    assert_eq!(events[1], (9, TauEvent::Spin));
    let mut s = TauState {
        age: 0,
        cooldown: 0,
        cells: 5,
    };
    tau_secondary_tick(
        &mut s,
        TauInput {
            selected: true,
            held: true,
            ..TauInput::default()
        },
    );
    assert_eq!(s.cells, 4);
}

#[test]
fn a_full_charge_takes_four_seconds_and_14_cells() {
    // Released after 4 s: full damage.
    let (events, s) = session(100, 80, 100);
    assert_eq!(fire_damage(&events), Some((80, 200)));
    assert_eq!(s.cells, 100 - 14);
    // Holding longer costs nothing more and does no more damage.
    let (events, s) = session(100, 150, 200);
    assert_eq!(fire_damage(&events), Some((150, 200)));
    assert_eq!(s.cells, 100 - 14);
}

#[test]
fn release_damage_scales_with_charge() {
    for (hold, damage) in [(1usize, 2u8), (20, 50), (40, 100), (60, 150), (79, 197)] {
        let (events, _) = session(100, hold, hold + 5);
        assert_eq!(fire_damage(&events), Some((hold, damage)), "held {hold}");
    }
}

#[test]
fn running_out_of_cells_fires_at_once() {
    let (events, s) = session(3, 100, 101);
    assert_eq!(fire_damage(&events), Some((11, 30)));
    assert_eq!(s.cells, 0);
}

#[test]
fn holding_ten_seconds_hurts_the_player() {
    let (events, s) = session(100, 210, 210);
    assert!(events.contains(&(199, TauEvent::Overcharge)));
    assert!(fire_damage(&events[..3]).is_none());
    assert_eq!(s.age, 0);
}

#[test]
fn no_cells_gives_a_dry_click() {
    let (events, s) = session(0, 5, 10);
    assert_eq!(events, vec![(0, TauEvent::Dry), (4, TauEvent::Dry)]);
    assert_eq!(s.cells, 0);
}

#[test]
fn charging_under_water_discharges() {
    let mut s = TauState {
        age: 30,
        cooldown: 0,
        cells: 50,
    };
    let e = tau_secondary_tick(
        &mut s,
        TauInput {
            selected: true,
            held: false,
            underwater: true,
            busy: false,
        },
    );
    assert_eq!(e, TauEvent::Discharge);
    assert_eq!((s.age, s.cooldown, s.cells), (0, 10, 50));
    // Even pressing it fresh under water just zaps.
    let mut s = TauState {
        age: 0,
        cooldown: 0,
        cells: 50,
    };
    let e = tau_secondary_tick(
        &mut s,
        TauInput {
            selected: true,
            held: true,
            underwater: true,
            busy: false,
        },
    );
    assert_eq!(e, TauEvent::Discharge);
    assert_eq!(s.cells, 50);
}

#[test]
fn cannot_start_while_busy_or_cooling_down() {
    for (cooldown, busy) in [(3u8, false), (0, true)] {
        let mut s = TauState {
            age: 0,
            cooldown,
            cells: 50,
        };
        let e = tau_secondary_tick(
            &mut s,
            TauInput {
                selected: true,
                held: true,
                underwater: false,
                busy,
            },
        );
        assert_eq!(e, TauEvent::Idle);
        assert_eq!((s.age, s.cells), (0, 50));
    }
}

#[test]
fn switching_away_drops_the_charge_without_firing() {
    let mut s = TauState {
        age: 40,
        cooldown: 0,
        cells: 50,
    };
    let e = tau_secondary_tick(
        &mut s,
        TauInput {
            selected: false,
            held: false,
            underwater: false,
            busy: false,
        },
    );
    assert_eq!(e, TauEvent::Idle);
    assert_eq!((s.age, s.cells), (0, 50));
}

#[test]
fn a_shot_shoves_the_player_back_by_five_times_its_damage() {
    // Five times the damage per second is a quarter of it per 20 Hz tick,
    // straight back along the view and never vertical.
    assert_eq!(tau_shove(200, [0, 0, 4096]), [0, 0, -50]);
    assert_eq!(tau_shove(200, [4096, 0, 0]), [-50, 0, 0]);
    assert_eq!(tau_shove(200, [0, 4096, 0]), [0, 0, 0]);
}

/// Recorded from ac83da7 behaviour: (cells, ticks held) followed by the
/// non-idle events over 260 ticks and the cells left. The game's cooldown
/// counts down once per tick between steps.
#[test]
fn golden_sessions() {
    use TauEvent::*;
    let dry_from = |first: usize, n: usize| (0..n).map(move |i| (first + 4 * i, Dry));
    let mut out_of_three = vec![(0, Start), (9, Spin), (11, Fire { damage: 30 })];
    out_of_three.extend(dry_from(31, 18));
    let mut out_of_one = vec![(0, Start), (5, Fire { damage: 15 })];
    out_of_one.extend(dry_from(25, 19));
    let cases: Vec<((u16, usize), Vec<(usize, TauEvent)>, u16)> = vec![
        (
            (100, 100),
            vec![(0, Start), (9, Spin), (100, Fire { damage: 200 })],
            86,
        ),
        (
            (100, 40),
            vec![(0, Start), (9, Spin), (40, Fire { damage: 100 })],
            93,
        ),
        ((100, 1), vec![(0, Start), (1, Fire { damage: 2 })], 99),
        ((100, 2), vec![(0, Start), (2, Fire { damage: 5 })], 99),
        ((3, 100), out_of_three, 0),
        ((1, 100), out_of_one.clone(), 0),
        ((2, 100), out_of_one, 0),
        (
            (100, 250),
            vec![
                (0, Start),
                (9, Spin),
                (199, Overcharge),
                (219, Start),
                (228, Spin),
                (250, Fire { damage: 77 }),
            ],
            80,
        ),
        ((0, 5), vec![(0, Dry), (4, Dry)], 0),
        (
            (100, 79),
            vec![(0, Start), (9, Spin), (79, Fire { damage: 197 })],
            86,
        ),
        (
            (100, 80),
            vec![(0, Start), (9, Spin), (80, Fire { damage: 200 })],
            86,
        ),
        (
            (100, 81),
            vec![(0, Start), (9, Spin), (81, Fire { damage: 200 })],
            86,
        ),
    ];
    for ((cells, hold), expect, left) in cases {
        let (events, s) = session(cells, hold, 260);
        assert_eq!(events, expect, "cells {cells} held {hold}");
        assert_eq!(s.cells, left, "cells {cells} held {hold}");
    }
}

/// Recorded from ac83da7 behaviour: shove for several damages, looking along
/// +Z and along the +X-Z diagonal.
#[test]
fn golden_shoves() {
    let cases: [(u8, [i32; 3], [i32; 3]); 6] = [
        (1, [0, 0, 0], [0, 0, 0]),
        (3, [0, 0, 0], [0, 0, 0]),
        (4, [0, 0, -1], [-1, 0, 0]),
        (50, [0, 0, -12], [-9, 0, 8]),
        (100, [0, 0, -25], [-18, 0, 17]),
        (200, [0, 0, -50], [-36, 0, 35]),
    ];
    for (damage, along_z, diagonal) in cases {
        assert_eq!(tau_shove(damage, [0, 0, 4096]), along_z, "damage {damage}");
        assert_eq!(
            tau_shove(damage, [2896, 0, -2896]),
            diagonal,
            "damage {damage}"
        );
    }
}
