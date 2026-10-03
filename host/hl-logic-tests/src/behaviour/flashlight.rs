//! Suit lamp battery: 0 to 100, drains one unit per 1.2 s while on and
//! recharges one per 0.2 s while off, switches itself off with a click at 0,
//! shows 99 on a new game and 100 after the first tick, and needs the suit.

use crate::player_rules::{
    flashlight_tick, Flashlight, FlashlightInput, FLASH_CHARGE_TICKS, FLASH_DRAIN_TICKS,
};

/// (tick, battery, on, click, lit) for every tick where something changed.
type Change = (usize, u8, bool, bool, bool);

/// Run `ticks` 20 Hz ticks from `start`, pressing the button on the listed
/// ticks, dead from tick `dead_from` on.
fn run(
    start: Flashlight,
    ticks: usize,
    presses: &[usize],
    has_suit: bool,
    dead_from: usize,
) -> Vec<Change> {
    let mut s = start;
    let mut out = vec![];
    for t in 0..ticks {
        let (n, e) = flashlight_tick(
            s,
            FlashlightInput {
                toggle_pressed: presses.contains(&t),
                has_suit,
                dead: t >= dead_from,
            },
        );
        if n.battery != s.battery || n.on != s.on || e.click {
            out.push((t, n.battery, n.on, e.click, e.lit));
        }
        s = n;
    }
    out
}

fn press(s: Flashlight) -> (Flashlight, bool, bool) {
    let (n, e) = flashlight_tick(
        s,
        FlashlightInput {
            toggle_pressed: true,
            has_suit: true,
            dead: false,
        },
    );
    (n, e.click, e.lit)
}

#[test]
fn timings_are_one_point_two_and_point_two_seconds() {
    assert_eq!(FLASH_DRAIN_TICKS, 24);
    assert_eq!(FLASH_CHARGE_TICKS, 4);
}

#[test]
fn a_new_game_shows_99_then_100_on_the_first_tick() {
    assert_eq!(Flashlight::NEW_GAME.battery, 99);
    assert!(!Flashlight::NEW_GAME.on);
    assert_eq!(
        run(Flashlight::NEW_GAME, 50, &[], true, usize::MAX),
        vec![(0, 100, false, false, false)]
    );
}

#[test]
fn drains_one_unit_every_24_ticks_while_on() {
    let changes = run(Flashlight::NEW_GAME, 400, &[2], true, usize::MAX);
    let drops: Vec<usize> = changes
        .iter()
        .filter(|c| c.2 && !c.3)
        .map(|c| c.0)
        .collect();
    assert_eq!(drops[0], 2 + 24);
    assert!(drops.windows(2).all(|w| w[1] - w[0] == 24));
}

#[test]
fn recharges_one_unit_every_4_ticks_while_off_up_to_100() {
    let s = Flashlight {
        on: false,
        battery: 90,
        timer: 4,
    };
    let changes = run(s, 100, &[], true, usize::MAX);
    let ticks: Vec<usize> = changes.iter().map(|c| c.0).collect();
    assert_eq!(ticks, vec![3, 7, 11, 15, 19, 23, 27, 31, 35, 39]);
    assert_eq!(changes.last().unwrap().1, 100);
}

#[test]
fn an_empty_battery_switches_off_with_a_click() {
    let s = Flashlight {
        on: true,
        battery: 1,
        timer: 1,
    };
    let (n, e) = flashlight_tick(s, FlashlightInput::default());
    assert_eq!(n.battery, 0);
    assert!(!n.on && e.click && !e.lit);
    // It cannot be switched on at 0, and pressing makes no click.
    let empty = Flashlight {
        on: false,
        battery: 0,
        timer: 3,
    };
    let (n, click, lit) = press(empty);
    assert!(!n.on && !click && !lit);
}

#[test]
fn toggling_clicks_both_ways() {
    let s = Flashlight {
        on: false,
        battery: 100,
        timer: 0,
    };
    let (on, click, lit) = press(s);
    assert!(on.on && click && lit);
    assert_eq!(on.timer, FLASH_DRAIN_TICKS);
    let (off, click, lit) = press(on);
    assert!(!off.on && click && !lit);
    assert_eq!(off.timer, FLASH_CHARGE_TICKS);
}

#[test]
fn the_lamp_needs_the_suit() {
    assert_eq!(
        run(Flashlight::NEW_GAME, 20, &[3, 6, 9], false, usize::MAX),
        vec![(0, 100, false, false, false)]
    );
    // A lamp somehow on without the suit does not light anything.
    let (_, e) = flashlight_tick(
        Flashlight {
            on: true,
            battery: 50,
            timer: 10,
        },
        FlashlightInput {
            toggle_pressed: false,
            has_suit: false,
            dead: false,
        },
    );
    assert!(!e.lit);
}

#[test]
fn dying_switches_the_lamp_off_silently_and_blocks_the_button() {
    let changes = run(
        Flashlight {
            on: true,
            battery: 50,
            timer: 5,
        },
        20,
        &[8],
        true,
        4,
    );
    assert_eq!(changes[0], (4, 50, false, false, false));
    assert!(changes.iter().all(|c| !c.2 && !c.3));
}

/// Recorded from ac83da7 behaviour: a new game, lamp switched on at tick 2,
/// off at 100 and on again at 130, over 200 ticks.
#[test]
fn golden_on_off_session() {
    let expect: Vec<Change> = vec![
        (0, 100, false, false, false),
        (2, 100, true, true, true),
        (26, 99, true, false, true),
        (50, 98, true, false, true),
        (74, 97, true, false, true),
        (98, 96, true, false, true),
        (100, 96, false, true, false),
        (104, 97, false, false, false),
        (108, 98, false, false, false),
        (112, 99, false, false, false),
        (116, 100, false, false, false),
        (130, 100, true, true, true),
        (154, 99, true, false, true),
        (178, 98, true, false, true),
    ];
    assert_eq!(
        run(Flashlight::NEW_GAME, 200, &[2, 100, 130], true, usize::MAX),
        expect
    );
}

/// Recorded from ac83da7 behaviour: a nearly empty lamp left on runs out,
/// clicks off and recharges; in the second run it is switched back on at
/// tick 80 with 2 units.
#[test]
fn golden_run_out_and_recover() {
    let start = Flashlight {
        on: true,
        battery: 3,
        timer: 24,
    };
    let expect: Vec<Change> = vec![
        (23, 2, true, false, true),
        (47, 1, true, false, true),
        (71, 0, false, true, false),
        (75, 1, false, false, false),
        (79, 2, false, false, false),
        (83, 3, false, false, false),
        (87, 4, false, false, false),
        (91, 5, false, false, false),
        (95, 6, false, false, false),
        (99, 7, false, false, false),
    ];
    assert_eq!(run(start, 100, &[], true, usize::MAX), expect);
    let expect: Vec<Change> = vec![
        (23, 2, true, false, true),
        (47, 1, true, false, true),
        (71, 0, false, true, false),
        (75, 1, false, false, false),
        (79, 2, false, false, false),
        (80, 2, true, true, true),
    ];
    assert_eq!(run(start, 100, &[80], true, usize::MAX), expect);
}

/// Recorded from ac83da7 behaviour: from an empty lamp, presses at ticks 0, 5
/// and 10; a lamp on with 50 units when the player dies at tick 4 (a press
/// at 8 is ignored); and a suitless player pressing at tick 3.
#[test]
fn golden_edge_sessions() {
    let empty = Flashlight {
        on: false,
        battery: 0,
        timer: 0,
    };
    assert_eq!(
        run(empty, 20, &[0, 5, 10], true, usize::MAX),
        vec![
            (0, 1, true, true, true),
            (5, 1, false, true, false),
            (9, 2, false, false, false),
            (10, 2, true, true, true),
        ]
    );
    let lit = Flashlight {
        on: true,
        battery: 50,
        timer: 5,
    };
    assert_eq!(
        run(lit, 20, &[8], true, 4),
        vec![
            (4, 50, false, false, false),
            (7, 51, false, false, false),
            (11, 52, false, false, false),
            (15, 53, false, false, false),
            (19, 54, false, false, false),
        ]
    );
    assert_eq!(
        run(Flashlight::NEW_GAME, 20, &[3], false, usize::MAX),
        vec![(0, 100, false, false, false)]
    );
}
