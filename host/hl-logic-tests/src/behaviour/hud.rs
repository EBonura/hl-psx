//! Behaviour of the first-person HUD layout (`game/src/hud_layout.rs`).
//!
//! Screen coordinates are at 320x240. Brightness is 0..255 (255 = the art's
//! full colour). Records come in submission order; the GPU draws them in
//! reverse, so an earlier record sits on top of a later one. One layout call
//! is one presented frame; the game presents at most 20 frames a second, the
//! rate the timings below are stated at.

use crate::hud::{atlas_uv, DRAW_CAP};
use crate::hud_layout::{
    layout, AmmoDisplay, HudCell, HudDraw, HudFade, HudInputs, Ink, PICKUP_BATTERY,
    PICKUP_HEALTHKIT, PICKUP_LONGJUMP, PICKUP_NONE, PICKUP_SUIT,
};

const CROWBAR: u8 = 0;
const GLOCK: u8 = 1;
const MAGNUM: u8 = 2;
const MP5: u8 = 3;
const SHOTGUN: u8 = 4;
const CROSSBOW: u8 = 5;
const RPG: u8 = 6;
const GAUSS: u8 = 7;
const EGON: u8 = 8;
const HORNET: u8 = 9;
const GRENADE: u8 = 10;
const SNARK: u8 = 11;
const TRIPMINE: u8 = 12;
const SATCHEL: u8 = 13;

/// Nothing shown: no suit, no weapon, no train, no pickup.
fn quiet() -> HudInputs {
    HudInputs {
        suit: false,
        health: 100,
        armour: 0,
        weapon: None,
        ammo: AmmoDisplay::None,
        clip: 0,
        reserve: 0,
        secondary: 0,
        zoomed: false,
        selection_ticks: 0,
        flashlight_on: false,
        flashlight_charge: 100,
        train_setting: 0,
        pickup_kind: PICKUP_NONE,
        pickup_ticks: 0,
    }
}

/// Suit on, 100 health, no armour, nothing held, flashlight off and full.
fn suited() -> HudInputs {
    HudInputs {
        suit: true,
        ..quiet()
    }
}

/// Suit on and holding `weapon` with the given ammo display.
fn armed(weapon: u8, ammo: AmmoDisplay, clip: u16, reserve: u16) -> HudInputs {
    HudInputs {
        weapon: Some(weapon),
        ammo,
        clip,
        reserve,
        ..suited()
    }
}

fn step(fade: &mut HudFade, inputs: &HudInputs) -> Vec<HudDraw> {
    let mut out = Vec::new();
    layout(fade, inputs, &mut |d| out.push(d));
    out
}

/// One frame with a fresh flash memory.
fn frame(inputs: &HudInputs) -> Vec<HudDraw> {
    step(&mut HudFade::new(), inputs)
}

fn show(d: &HudDraw) -> String {
    let mut s = format!("{:?} {},{} {}x{} {:?}", d.cell, d.x, d.y, d.w, d.h, d.ink);
    if d.skip_x != 0 || d.skip_y != 0 {
        s.push_str(&format!(" skip {},{}", d.skip_x, d.skip_y));
    }
    s
}

fn shown(records: &[HudDraw]) -> Vec<String> {
    records.iter().map(show).collect()
}

fn check(name: &str, actual: &[String], expected: &[&str]) {
    assert_eq!(actual, expected, "{name}");
}

fn find(records: &[HudDraw], cell: HudCell) -> Vec<HudDraw> {
    records.iter().copied().filter(|d| d.cell == cell).collect()
}

fn one(records: &[HudDraw], cell: HudCell) -> HudDraw {
    let found = find(records, cell);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one {cell:?} in {records:?}"
    );
    found[0]
}

fn digits_at(records: &[HudDraw], y: i16, x0: i16) -> Vec<(i16, u8)> {
    records
        .iter()
        .filter_map(|d| match d.cell {
            HudCell::Digit(n) if d.y == y && d.x >= x0 && d.x < x0 + 36 => Some((d.x, n)),
            _ => None,
        })
        .collect()
}

fn brightness(ink: Ink) -> u8 {
    match ink {
        Ink::Amber(b) | Ink::Red(b) => b,
        Ink::White => 255,
    }
}

// ---------------------------------------------------------------------------
// Spec properties
// ---------------------------------------------------------------------------

#[test]
fn numbers_use_a_three_cell_right_aligned_field_with_blank_leading_zeros() {
    // Health field starts at x=22, y=216; cells are 12x16.
    for (value, expected) in [
        (0u16, vec![(46, 0u8)]),
        (7, vec![(46, 7)]),
        (42, vec![(34, 4), (46, 2)]),
        (100, vec![(22, 1), (34, 0), (46, 0)]),
        (999, vec![(22, 9), (34, 9), (46, 9)]),
        (1234, vec![(22, 9), (34, 9), (46, 9)]),
        (65535, vec![(22, 9), (34, 9), (46, 9)]),
    ] {
        let r = frame(&HudInputs {
            health: value,
            ..suited()
        });
        assert_eq!(digits_at(&r, 216, 22), expected, "health {value}");
        for d in r.iter().filter(|d| matches!(d.cell, HudCell::Digit(_))) {
            assert_eq!((d.w, d.h), (12, 16));
        }
        // The field always occupies 36 pixels: the divider sits 6 past its end.
        assert_eq!(one(&r, HudCell::Divider).x, 64, "health {value}");
    }
}

#[test]
fn health_block_shows_only_with_the_suit() {
    let without = frame(&quiet());
    assert!(without.is_empty());
    let r = frame(&suited());
    let cross = one(&r, HudCell::HealthCross);
    assert_eq!((cross.x, cross.y, cross.w, cross.h), (8, 216, 16, 16));
    let divider = one(&r, HudCell::Divider);
    assert_eq!(
        (divider.x, divider.y, divider.w, divider.h),
        (64, 216, 1, 16)
    );
    assert_eq!(cross.ink, Ink::Amber(100));
    assert_eq!(divider.ink, Ink::Amber(100));
}

#[test]
fn health_low_values_turn_red_and_critical_values_draw_full_bright() {
    for (health, ink) in [
        (0u16, Ink::Red(255)),
        (15, Ink::Red(255)),
        (16, Ink::Red(100)),
        (25, Ink::Red(100)),
        (26, Ink::Amber(100)),
        (100, Ink::Amber(100)),
    ] {
        let r = frame(&HudInputs { health, ..suited() });
        assert_eq!(one(&r, HudCell::HealthCross).ink, ink, "health {health}");
    }
}

#[test]
fn a_changed_health_value_flashes_to_228_and_falls_back_to_100_over_five_seconds() {
    let mut fade = HudFade::new();
    let start = suited();
    let first = step(&mut fade, &start);
    assert_eq!(one(&first, HudCell::HealthCross).ink, Ink::Amber(100));
    let hurt = HudInputs {
        health: 90,
        ..start
    };
    let mut seen = Vec::new();
    for _ in 0..110 {
        let r = step(&mut fade, &hurt);
        seen.push(brightness(one(&r, HudCell::HealthCross).ink));
    }
    assert_eq!(seen[0], 228);
    // Monotonic fall, never below 100, back at 100 after 100 frames (5 s).
    assert!(seen.windows(2).all(|w| w[1] <= w[0]));
    assert!(seen.iter().all(|&b| b >= 100));
    assert_eq!(seen[100], 100);
    assert!(seen[99] > 100);
    // Linear within one brightness step: 128 over 100 frames.
    for (i, &b) in seen.iter().enumerate().take(100) {
        let ideal = 100.0 + 128.0 * (100 - i) as f32 / 100.0;
        assert!((b as f32 - ideal).abs() <= 1.0, "frame {i}: {b}");
    }
}

#[test]
fn armour_suit_fills_bottom_up_in_proportion_to_armour() {
    for armour in [0u16, 1, 5, 50, 99, 100, 150] {
        let r = frame(&HudInputs { armour, ..suited() });
        let outline = one(&r, HudCell::SuitEmpty);
        assert_eq!(
            (outline.x, outline.y, outline.w, outline.h),
            (64, 213, 20, 20)
        );
        let full = find(&r, HudCell::SuitFull);
        if armour == 0 {
            assert!(full.is_empty());
        } else {
            let hidden = (20 * (100 - armour.min(100)) / 100) as u8;
            let f = full[0];
            assert_eq!((f.x, f.w), (64, 20));
            assert_eq!(f.skip_y, hidden, "armour {armour}");
            assert_eq!(f.h, 20 - hidden);
            assert_eq!(f.y, 213 + hidden as i16);
        }
        // Number field at x=84.
        let field = digits_at(&r, 216, 84);
        assert!(!field.is_empty());
        assert_eq!(field.last().unwrap().0, 108);
    }
}

#[test]
fn armour_flashes_like_health_with_no_low_value_rule() {
    let mut fade = HudFade::new();
    step(&mut fade, &suited());
    let low = HudInputs {
        armour: 3,
        ..suited()
    };
    let r = step(&mut fade, &low);
    assert_eq!(one(&r, HudCell::SuitEmpty).ink, Ink::Amber(228));
    let mut last = 0;
    for _ in 0..100 {
        let r = step(&mut fade, &low);
        last = brightness(one(&r, HudCell::SuitEmpty).ink);
    }
    assert_eq!(last, 100);
    assert_eq!(one(&frame(&low), HudCell::SuitEmpty).ink, Ink::Amber(100));
}

#[test]
fn clip_weapons_lay_out_clip_divider_reserve_and_icon_on_the_bottom_row() {
    let r = frame(&armed(GLOCK, AmmoDisplay::ClipAndReserve, 123, 456));
    assert_eq!(digits_at(&r, 216, 206), vec![(206, 1), (218, 2), (230, 3)]);
    let dividers = find(&r, HudCell::Divider);
    assert!(dividers.iter().any(|d| (d.x, d.y) == (248, 216)));
    assert_eq!(digits_at(&r, 216, 255), vec![(255, 4), (267, 5), (279, 6)]);
    let icon = one(&r, HudCell::AmmoIcon(GLOCK));
    assert_eq!((icon.x, icon.y, icon.w, icon.h), (291, 214, 18, 18));
}

#[test]
fn reserve_only_weapons_lay_out_one_field_and_the_icon() {
    let r = frame(&armed(RPG, AmmoDisplay::ReserveOnly, 0, 5));
    assert_eq!(digits_at(&r, 216, 254), vec![(278, 5)]);
    let icon = one(&r, HudCell::AmmoIcon(RPG));
    assert_eq!((icon.x, icon.y, icon.w, icon.h), (290, 214, 18, 18));
    assert_eq!(
        find(&r, HudCell::Divider).len(),
        1,
        "only the health divider"
    );
}

#[test]
fn melee_weapons_show_no_ammo_block() {
    let r = frame(&armed(CROWBAR, AmmoDisplay::None, 0, 0));
    assert!(r.iter().all(|d| !matches!(d.cell, HudCell::AmmoIcon(_))));
    assert!(digits_at(&r, 216, 200).is_empty());
}

#[test]
fn mp5_grenades_show_a_second_row_only_when_carried() {
    let base = armed(MP5, AmmoDisplay::ClipAndReserve, 50, 100);
    let none = frame(&base);
    assert!(find(&none, HudCell::GrenadeAmmoIcon).is_empty());
    let some = frame(&HudInputs {
        secondary: 2,
        ..base
    });
    assert_eq!(digits_at(&some, 196, 254), vec![(278, 2)]);
    let icon = one(&some, HudCell::GrenadeAmmoIcon);
    assert_eq!((icon.x, icon.y, icon.w, icon.h), (290, 194, 18, 18));
    // Other weapons never show the second row.
    let glock = frame(&HudInputs {
        secondary: 2,
        ..armed(GLOCK, AmmoDisplay::ClipAndReserve, 5, 5)
    });
    assert!(find(&glock, HudCell::GrenadeAmmoIcon).is_empty());
}

#[test]
fn ammo_flashes_to_200_on_any_change_and_falls_one_step_per_frame_to_100() {
    let mut fade = HudFade::new();
    let base = armed(GLOCK, AmmoDisplay::ClipAndReserve, 17, 34);
    let r = step(&mut fade, &base);
    assert_eq!(one(&r, HudCell::AmmoIcon(GLOCK)).ink, Ink::Amber(100));
    let fired = HudInputs { clip: 16, ..base };
    let mut seen = Vec::new();
    for _ in 0..105 {
        let r = step(&mut fade, &fired);
        seen.push(brightness(one(&r, HudCell::AmmoIcon(GLOCK)).ink));
    }
    // 20 per second at 20 frames a second.
    assert_eq!(&seen[..4], &[200, 199, 198, 197]);
    assert_eq!(seen[100], 100);
    assert_eq!(seen[104], 100);
    // Reserve change and weapon switch flash too.
    for changed in [
        HudInputs {
            reserve: 1,
            ..fired
        },
        HudInputs {
            weapon: Some(MAGNUM),
            ..fired
        },
    ] {
        let mut fade = HudFade::new();
        step(&mut fade, &fired);
        let r = step(&mut fade, &changed);
        let icon = r
            .iter()
            .find(|d| matches!(d.cell, HudCell::AmmoIcon(_)))
            .unwrap();
        assert_eq!(icon.ink, Ink::Amber(200));
    }
}

#[test]
fn flashlight_casing_beam_and_charge_fill() {
    for on in [false, true] {
        for charge in [0u8, 1, 19, 20, 50, 99, 100] {
            let r = frame(&HudInputs {
                flashlight_on: on,
                flashlight_charge: charge,
                ..suited()
            });
            let level = if on { 225 } else { 100 };
            let ink = if charge < 20 {
                Ink::Red(level)
            } else {
                Ink::Amber(level)
            };
            let casing = one(&r, HudCell::FlashlightEmpty);
            assert_eq!((casing.x, casing.y, casing.w, casing.h), (293, 8, 18, 16));
            assert_eq!(casing.ink, ink);
            let beam = find(&r, HudCell::FlashlightBeam);
            if on {
                assert_eq!(
                    (beam[0].x, beam[0].y, beam[0].w, beam[0].h),
                    (311, 8, 6, 16)
                );
                assert_eq!(beam[0].ink, ink);
            } else {
                assert!(beam.is_empty());
            }
            let fill = find(&r, HudCell::FlashlightFull);
            let hidden = (18 * (100 - charge as u16) / 100) as u8;
            if hidden >= 18 {
                assert!(fill.is_empty(), "charge {charge}");
            } else {
                let f = fill[0];
                assert_eq!(f.skip_x, hidden);
                assert_eq!(f.w, 18 - hidden);
                assert_eq!((f.x, f.y, f.h), (293 + hidden as i16, 8, 16));
                assert_eq!(f.ink, ink);
            }
        }
    }
}

#[test]
fn flashlight_shows_only_with_the_suit() {
    let r = frame(&HudInputs {
        flashlight_on: true,
        ..quiet()
    });
    assert!(r.is_empty());
}

#[test]
fn train_indicator_shows_settings_one_to_five() {
    for setting in 0u8..=7 {
        let r = frame(&HudInputs {
            train_setting: setting,
            ..quiet()
        });
        if (1..=5).contains(&setting) {
            assert_eq!(r.len(), 1);
            let d = r[0];
            assert_eq!(d.cell, HudCell::TrainFrame(setting - 1));
            assert_eq!((d.x, d.y, d.w, d.h), (110, 208, 16, 16));
            assert_eq!(d.ink, Ink::Amber(255));
        } else {
            assert!(r.is_empty(), "setting {setting}");
        }
    }
}

#[test]
fn weapon_selection_strip_places_slots_and_the_held_weapons_picture() {
    let slot_of = [0u8, 1, 1, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4];
    for weapon in 0u8..14 {
        let r = frame(&HudInputs {
            weapon: Some(weapon),
            selection_ticks: 30,
            ..quiet()
        });
        let held = slot_of[weapon as usize] as i16;
        let mut x = 10;
        for slot in 0u8..5 {
            let d = one(&r, HudCell::SlotNumber(slot));
            assert_eq!((d.x, d.y, d.w, d.h), (x, 10, 12, 12), "weapon {weapon}");
            x += if slot as i16 == held { 85 } else { 17 };
        }
        let held_x = 10 + 17 * held;
        let picture = one(&r, HudCell::WeaponPicture(weapon));
        let frame_rec = one(&r, HudCell::SelectionFrame);
        assert_eq!(
            (picture.x, picture.y, picture.w, picture.h),
            (held_x, 22, 80, 20)
        );
        assert_eq!(
            (frame_rec.x, frame_rec.y, frame_rec.w, frame_rec.h),
            (held_x, 22, 80, 20)
        );
        // The frame is drawn over the picture (submitted earlier).
        let pos = |c| r.iter().position(|d| d.cell == c).unwrap();
        assert!(pos(HudCell::SelectionFrame) < pos(HudCell::WeaponPicture(weapon)));
        assert!(r
            .iter()
            .all(|d| d.ink == Ink::Amber(255) || d.ink == Ink::White));
    }
    let hidden = frame(&HudInputs {
        weapon: Some(GLOCK),
        selection_ticks: 0,
        ..quiet()
    });
    assert!(find(&hidden, HudCell::SelectionFrame).is_empty());
}

#[test]
fn item_pickups_show_a_fading_icon_for_five_seconds() {
    for (kind, cell) in [
        (PICKUP_BATTERY, HudCell::BatteryIcon),
        (PICKUP_HEALTHKIT, HudCell::HealthKitIcon),
        (PICKUP_LONGJUMP, HudCell::LongJumpIcon),
    ] {
        for ticks in [0u8, 1, 20, 63, 64, 100, 255] {
            let r = frame(&HudInputs {
                pickup_kind: kind,
                pickup_ticks: ticks,
                ..quiet()
            });
            if ticks == 0 {
                assert!(r.is_empty());
                continue;
            }
            assert_eq!(r.len(), 1);
            let d = r[0];
            assert_eq!(d.cell, cell);
            assert_eq!((d.x, d.y, d.w, d.h), (290, 168, 20, 20));
            // Remaining seconds x 80, capped at 255.
            let ideal = (ticks as u32 * 80 / 20).min(255) as u8;
            assert_eq!(d.ink, Ink::Amber(ideal), "kind {kind} ticks {ticks}");
        }
    }
    for kind in [PICKUP_NONE, PICKUP_SUIT, 5] {
        let r = frame(&HudInputs {
            pickup_kind: kind,
            pickup_ticks: 100,
            ..quiet()
        });
        assert!(r.is_empty(), "kind {kind}");
    }
}

#[test]
fn crosshair_per_weapon_and_the_crossbow_scope() {
    for weapon in 0u8..14 {
        let r = frame(&HudInputs {
            weapon: Some(weapon),
            ..quiet()
        });
        if weapon == CROWBAR || weapon >= GRENADE {
            assert!(r.is_empty(), "weapon {weapon}");
        } else {
            assert_eq!(r.len(), 1);
            let d = r[0];
            assert_eq!(d.cell, HudCell::Crosshair(weapon));
            assert_eq!((d.x, d.y, d.w, d.h), (148, 108, 24, 24));
            assert_eq!(d.ink, Ink::White);
        }
    }
    let scoped = frame(&HudInputs {
        weapon: Some(CROSSBOW),
        zoomed: true,
        ..quiet()
    });
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].cell, HudCell::ZoomReticle);
    assert_eq!(
        (scoped[0].x, scoped[0].y, scoped[0].w, scoped[0].h),
        (108, 112, 104, 16)
    );
    // Zoom only changes the crossbow's sight.
    let other = frame(&HudInputs {
        weapon: Some(GLOCK),
        zoomed: true,
        ..quiet()
    });
    assert_eq!(other[0].cell, HudCell::Crosshair(GLOCK));
}

#[test]
fn every_record_samples_inside_the_hud_atlas() {
    for inputs in matrix() {
        for d in frame(&inputs) {
            let (u, v) = atlas_uv(d.cell);
            let u0 = u as usize + d.skip_x as usize;
            let v0 = v as usize + d.skip_y as usize;
            assert!(u0 + d.w as usize <= 256, "{d:?}");
            assert!(v0 + d.h as usize <= 256, "{d:?}");
            // Gameplay text owns u=160..255 for v=0..127.
            assert!(u0 + d.w as usize <= 160 || v0 >= 128, "{d:?}");
        }
    }
}

#[test]
fn the_busiest_frame_overflows_the_sprite_budget_by_one() {
    // MP5 with three-digit counts, grenades, selection, pickup, flashlight on
    // and the train indicator: the layout asks for 35 sprites, the PS1 packet
    // store holds 34, so the last record (the pickup icon) is not drawn.
    let busiest = HudInputs {
        health: 100,
        armour: 100,
        secondary: 100,
        selection_ticks: 30,
        flashlight_on: true,
        flashlight_charge: 50,
        train_setting: 3,
        pickup_kind: PICKUP_HEALTHKIT,
        pickup_ticks: 20,
        ..armed(MP5, AmmoDisplay::ClipAndReserve, 100, 100)
    };
    let r = frame(&busiest);
    assert_eq!(DRAW_CAP, 34);
    assert_eq!(r.len(), 35);
    assert_eq!(r[34].cell, HudCell::HealthKitIcon);
    let no_train = frame(&HudInputs {
        train_setting: 0,
        ..busiest
    });
    assert_eq!(no_train.len(), DRAW_CAP);
}

/// A broad sweep of inputs used by the whole-frame checks.
fn matrix() -> Vec<HudInputs> {
    let mut all = Vec::new();
    for weapon in [
        None,
        Some(CROWBAR),
        Some(GLOCK),
        Some(MP5),
        Some(CROSSBOW),
        Some(SATCHEL),
    ] {
        for ammo in [
            AmmoDisplay::None,
            AmmoDisplay::ReserveOnly,
            AmmoDisplay::ClipAndReserve,
        ] {
            for suit in [false, true] {
                for value in [0u16, 7, 42, 100, 999, 1234] {
                    all.push(HudInputs {
                        suit,
                        health: value,
                        armour: value,
                        weapon,
                        ammo,
                        clip: value,
                        reserve: value,
                        secondary: value,
                        zoomed: value == 42,
                        selection_ticks: (value % 2) as u8 * 30,
                        flashlight_on: value >= 100,
                        flashlight_charge: (value % 101) as u8,
                        train_setting: (value % 7) as u8,
                        pickup_kind: (value % 5) as u8,
                        pickup_ticks: (value % 128) as u8,
                    });
                }
            }
        }
    }
    all
}

// ---------------------------------------------------------------------------
// Golden frames
// ---------------------------------------------------------------------------
//
// Every table below was recorded from ac83da7 behaviour: the layout records
// were captured by running the code, after the extraction was shown to give
// sprite packets identical to ac83da7's on a 57,000-frame input sweep.

fn label(text: String) -> String {
    format!("-- {text}")
}

fn only(records: &[HudDraw], keep: impl Fn(&HudCell) -> bool) -> Vec<String> {
    records.iter().filter(|d| keep(&d.cell)).map(show).collect()
}

#[test]
fn golden_suit_on_with_nothing_held() {
    // Fresh memory, suit on, 100 health, no armour, flashlight off and full.
    check("SUIT_ONLY", &shown(&frame(&suited())), SUIT_ONLY);
}

#[test]
fn golden_numbers_in_every_field() {
    // MP5 with health, armour, clip, reserve and grenades all set to one value.
    let mut lines = Vec::new();
    for value in [0u16, 7, 42, 100, 999, 1234] {
        lines.push(label(format!("value {value}")));
        let inputs = HudInputs {
            health: value,
            armour: value,
            secondary: value,
            ..armed(MP5, AmmoDisplay::ClipAndReserve, value, value)
        };
        lines.extend(shown(&frame(&inputs)));
    }
    check("NUMBERS", &lines, NUMBERS);
}

#[test]
fn golden_low_health_colours() {
    // Health block only, fresh memory, around the 15 and 25 thresholds.
    let mut lines = Vec::new();
    for health in [0u16, 1, 15, 16, 25, 26, 100] {
        lines.push(label(format!("health {health}")));
        let r = frame(&HudInputs { health, ..suited() });
        // The health cross, field and divider all sit at y=216, x<=64.
        lines.extend(r.iter().filter(|d| d.x <= 64 && d.y == 216).map(show));
    }
    check("LOW_HEALTH", &lines, LOW_HEALTH);
}

#[test]
fn golden_fade_after_changes() {
    // Glock, 100 health, 50 armour, 17/34 rounds. On the second frame health
    // drops to 80, armour to 40 and one round is fired; then nothing changes.
    // Each line: frames since the change, then health, armour, ammo inks.
    let base = HudInputs {
        armour: 50,
        ..armed(GLOCK, AmmoDisplay::ClipAndReserve, 17, 34)
    };
    let hit = HudInputs {
        health: 80,
        armour: 40,
        clip: 16,
        ..base
    };
    let mut fade = HudFade::new();
    let mut lines = Vec::new();
    let sample = |r: &[HudDraw], at: i32| {
        format!(
            "{at}: {:?} {:?} {:?}",
            one(r, HudCell::HealthCross).ink,
            one(r, HudCell::SuitEmpty).ink,
            one(r, HudCell::AmmoIcon(GLOCK)).ink
        )
    };
    lines.push(sample(&step(&mut fade, &base), -1));
    for at in 0..=160 {
        let r = step(&mut fade, &hit);
        if [0, 1, 2, 3, 10, 49, 50, 77, 98, 99, 100, 101, 150, 160].contains(&at) {
            lines.push(sample(&r, at));
        }
    }
    // A second hit to 12 health: the number stays full bright while critical.
    let critical = HudInputs { health: 12, ..hit };
    for at in 0..=3 {
        let r = step(&mut fade, &critical);
        lines.push(sample(&r, at));
    }
    // Suit off for 50 frames freezes the memory; the flash resumes after.
    let healed = HudInputs {
        health: 60,
        ..critical
    };
    step(&mut fade, &healed);
    for _ in 0..50 {
        step(
            &mut fade,
            &HudInputs {
                suit: false,
                ..healed
            },
        );
    }
    lines.push(sample(&step(&mut fade, &healed), 51));
    check("FADE", &lines, FADE);
}

#[test]
fn golden_armour_fill() {
    // Armour block only, fresh memory.
    let mut lines = Vec::new();
    for armour in [0u16, 1, 4, 5, 50, 94, 95, 99, 100, 150] {
        lines.push(label(format!("armour {armour}")));
        let r = frame(&HudInputs { armour, ..suited() });
        lines.extend(only(&r, |c| {
            matches!(c, HudCell::SuitEmpty | HudCell::SuitFull)
        }));
    }
    check("ARMOUR", &lines, ARMOUR);
}

#[test]
fn golden_ammo_layouts() {
    // Fresh memory, suit on, one frame per weapon and ammo display.
    let mut lines = Vec::new();
    for (weapon, ammo, clip, reserve, secondary) in [
        (CROWBAR, AmmoDisplay::None, 0u16, 0u16, 0u16),
        (GLOCK, AmmoDisplay::ClipAndReserve, 17, 68, 0),
        (MAGNUM, AmmoDisplay::ClipAndReserve, 6, 0, 0),
        (MP5, AmmoDisplay::ClipAndReserve, 50, 250, 0),
        (MP5, AmmoDisplay::ClipAndReserve, 50, 250, 10),
        (SHOTGUN, AmmoDisplay::ClipAndReserve, 8, 125, 0),
        (CROSSBOW, AmmoDisplay::ClipAndReserve, 5, 50, 0),
        (RPG, AmmoDisplay::ClipAndReserve, 1, 5, 0),
        (GAUSS, AmmoDisplay::ReserveOnly, 0, 100, 0),
        (EGON, AmmoDisplay::ReserveOnly, 0, 100, 0),
        (HORNET, AmmoDisplay::ReserveOnly, 0, 8, 0),
        (GRENADE, AmmoDisplay::ReserveOnly, 0, 10, 0),
        (SNARK, AmmoDisplay::ReserveOnly, 0, 15, 0),
        (TRIPMINE, AmmoDisplay::ReserveOnly, 0, 5, 0),
        (SATCHEL, AmmoDisplay::ReserveOnly, 0, 5, 0),
    ] {
        lines.push(label(format!(
            "weapon {weapon} {ammo:?} {clip}/{reserve}/{secondary}"
        )));
        let r = frame(&HudInputs {
            secondary,
            ..armed(weapon, ammo, clip, reserve)
        });
        lines.extend(only(&r, |c| {
            !matches!(
                c,
                HudCell::HealthCross
                    | HudCell::SuitEmpty
                    | HudCell::SuitFull
                    | HudCell::FlashlightEmpty
                    | HudCell::FlashlightFull
                    | HudCell::FlashlightBeam
            )
        }));
    }
    check("AMMO", &lines, AMMO);
}

#[test]
fn golden_flashlight() {
    // Flashlight records only, suit on, fresh memory.
    let mut lines = Vec::new();
    for on in [false, true] {
        for charge in [0u8, 1, 5, 6, 19, 20, 50, 94, 95, 99, 100, 101, 255] {
            lines.push(label(format!("on {on} charge {charge}")));
            let r = frame(&HudInputs {
                flashlight_on: on,
                flashlight_charge: charge,
                ..suited()
            });
            lines.extend(only(&r, |c| {
                matches!(
                    c,
                    HudCell::FlashlightEmpty | HudCell::FlashlightFull | HudCell::FlashlightBeam
                )
            }));
        }
    }
    check("FLASHLIGHT", &lines, FLASHLIGHT);
}

#[test]
fn golden_train_settings() {
    // No suit, nothing held; only the train setting varies.
    let mut lines = Vec::new();
    for setting in 0u8..=6 {
        lines.push(label(format!("setting {setting}")));
        lines.extend(shown(&frame(&HudInputs {
            train_setting: setting,
            ..quiet()
        })));
    }
    check("TRAIN", &lines, TRAIN);
}

#[test]
fn golden_selection_strip_for_each_weapon() {
    // No suit; each weapon held with the selection display running.
    let mut lines = Vec::new();
    for weapon in 0u8..14 {
        lines.push(label(format!("weapon {weapon}")));
        lines.extend(shown(&frame(&HudInputs {
            weapon: Some(weapon),
            selection_ticks: 30,
            ..quiet()
        })));
    }
    check("SELECTION", &lines, SELECTION);
}

#[test]
fn golden_pickup_history() {
    // No suit, nothing held; each pickup kind at several remaining times.
    let mut lines = Vec::new();
    for kind in 0u8..=5 {
        for ticks in [0u8, 1, 2, 20, 40, 63, 64, 100] {
            lines.push(label(format!("kind {kind} ticks {ticks}")));
            lines.extend(shown(&frame(&HudInputs {
                pickup_kind: kind,
                pickup_ticks: ticks,
                ..quiet()
            })));
        }
    }
    check("PICKUP", &lines, PICKUP);
}

#[test]
fn golden_sights() {
    // No suit; each weapon unzoomed and zoomed.
    let mut lines = Vec::new();
    for weapon in 0u8..14 {
        for zoomed in [false, true] {
            lines.push(label(format!("weapon {weapon} zoomed {zoomed}")));
            lines.extend(shown(&frame(&HudInputs {
                weapon: Some(weapon),
                zoomed,
                ..quiet()
            })));
        }
    }
    check("SIGHTS", &lines, SIGHTS);
}

#[test]
fn golden_busiest_frame() {
    // Every block at once, as in the sprite-budget test above.
    let busiest = HudInputs {
        health: 100,
        armour: 100,
        secondary: 100,
        selection_ticks: 30,
        flashlight_on: true,
        flashlight_charge: 50,
        train_setting: 3,
        pickup_kind: PICKUP_HEALTHKIT,
        pickup_ticks: 20,
        ..armed(MP5, AmmoDisplay::ClipAndReserve, 100, 100)
    };
    check("BUSIEST", &shown(&frame(&busiest)), BUSIEST);
}

/// The only test that drives `hud::draw`, whose flash memory is global.
#[test]
fn golden_submitted_sprite_packets() {
    // Three frames through the PS1 submit path, packets as GP0 words (colour
    // and command, position, texcoord and palette, size). Frame one is the
    // busiest frame, cut to the 34-sprite store; frame two drops health to 20
    // (red, flashing) and switches to the crossbow scope; frame three is the
    // suit-only frame with a long-jump pickup and the flashlight on and low.
    use crate::hud::{draw, upload, EMPTY_SPRITE};
    use psx_gpu::ot::OrderingTable;
    use psx_gpu::prim::Sprite;
    let busiest = HudInputs {
        health: 100,
        armour: 100,
        secondary: 100,
        selection_ticks: 30,
        flashlight_on: true,
        flashlight_charge: 50,
        train_setting: 3,
        pickup_kind: PICKUP_HEALTHKIT,
        pickup_ticks: 20,
        ..armed(MP5, AmmoDisplay::ClipAndReserve, 100, 100)
    };
    let hurt = HudInputs {
        health: 20,
        weapon: Some(CROSSBOW),
        zoomed: true,
        clip: 5,
        reserve: 50,
        selection_ticks: 0,
        train_setting: 0,
        pickup_kind: PICKUP_NONE,
        ..busiest
    };
    let plain = HudInputs {
        flashlight_on: true,
        flashlight_charge: 7,
        pickup_kind: PICKUP_LONGJUMP,
        pickup_ticks: 33,
        ..suited()
    };
    // An empty blob skips the VRAM upload and only builds the materials.
    let mats = upload(&[]);
    let mut ot: Box<OrderingTable<4>> = Box::new(OrderingTable::new());
    let mut prims: Box<[Sprite; DRAW_CAP]> = Box::new([EMPTY_SPRITE; DRAW_CAP]);
    let mut lines = Vec::new();
    for (name, inputs) in [("busiest", busiest), ("hurt", hurt), ("plain", plain)] {
        ot.clear();
        // SAFETY: the table is never walked; the packets outlive it.
        let n = unsafe { draw(mats, &inputs, &mut ot, &mut prims) };
        lines.push(label(format!("{name} {n}")));
        for p in &prims[..n] {
            lines.push(format!(
                "{:08x} {:08x} {:08x} {:08x}",
                p.color_cmd, p.xy, p.uv_clut, p.wh
            ));
        }
    }
    check("PACKETS", &lines, PACKETS);
}

const AMMO: &[&str] = &[
    "-- weapon 0 None 0/0/0",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "-- weapon 1 ClipAndReserve 17/68/0",
    "Crosshair(1) 148,108 24x24 White",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(1) 218,216 12x16 Amber(100)",
    "Digit(7) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(6) 267,216 12x16 Amber(100)",
    "Digit(8) 279,216 12x16 Amber(100)",
    "AmmoIcon(1) 291,214 18x18 Amber(100)",
    "-- weapon 2 ClipAndReserve 6/0/0",
    "Crosshair(2) 148,108 24x24 White",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(6) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(0) 279,216 12x16 Amber(100)",
    "AmmoIcon(2) 291,214 18x18 Amber(100)",
    "-- weapon 3 ClipAndReserve 50/250/0",
    "Crosshair(3) 148,108 24x24 White",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(5) 218,216 12x16 Amber(100)",
    "Digit(0) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(2) 255,216 12x16 Amber(100)",
    "Digit(5) 267,216 12x16 Amber(100)",
    "Digit(0) 279,216 12x16 Amber(100)",
    "AmmoIcon(3) 291,214 18x18 Amber(100)",
    "-- weapon 3 ClipAndReserve 50/250/10",
    "Crosshair(3) 148,108 24x24 White",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(5) 218,216 12x16 Amber(100)",
    "Digit(0) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(2) 255,216 12x16 Amber(100)",
    "Digit(5) 267,216 12x16 Amber(100)",
    "Digit(0) 279,216 12x16 Amber(100)",
    "AmmoIcon(3) 291,214 18x18 Amber(100)",
    "Digit(1) 266,196 12x16 Amber(100)",
    "Digit(0) 278,196 12x16 Amber(100)",
    "GrenadeAmmoIcon 290,194 18x18 Amber(100)",
    "-- weapon 4 ClipAndReserve 8/125/0",
    "Crosshair(4) 148,108 24x24 White",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(8) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(1) 255,216 12x16 Amber(100)",
    "Digit(2) 267,216 12x16 Amber(100)",
    "Digit(5) 279,216 12x16 Amber(100)",
    "AmmoIcon(4) 291,214 18x18 Amber(100)",
    "-- weapon 5 ClipAndReserve 5/50/0",
    "Crosshair(5) 148,108 24x24 White",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(5) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(5) 267,216 12x16 Amber(100)",
    "Digit(0) 279,216 12x16 Amber(100)",
    "AmmoIcon(5) 291,214 18x18 Amber(100)",
    "-- weapon 6 ClipAndReserve 1/5/0",
    "Crosshair(6) 148,108 24x24 White",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(1) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(5) 279,216 12x16 Amber(100)",
    "AmmoIcon(6) 291,214 18x18 Amber(100)",
    "-- weapon 7 ReserveOnly 0/100/0",
    "Crosshair(7) 148,108 24x24 White",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(1) 254,216 12x16 Amber(100)",
    "Digit(0) 266,216 12x16 Amber(100)",
    "Digit(0) 278,216 12x16 Amber(100)",
    "AmmoIcon(7) 290,214 18x18 Amber(100)",
    "-- weapon 8 ReserveOnly 0/100/0",
    "Crosshair(8) 148,108 24x24 White",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(1) 254,216 12x16 Amber(100)",
    "Digit(0) 266,216 12x16 Amber(100)",
    "Digit(0) 278,216 12x16 Amber(100)",
    "AmmoIcon(8) 290,214 18x18 Amber(100)",
    "-- weapon 9 ReserveOnly 0/8/0",
    "Crosshair(9) 148,108 24x24 White",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(8) 278,216 12x16 Amber(100)",
    "AmmoIcon(9) 290,214 18x18 Amber(100)",
    "-- weapon 10 ReserveOnly 0/10/0",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(1) 266,216 12x16 Amber(100)",
    "Digit(0) 278,216 12x16 Amber(100)",
    "AmmoIcon(10) 290,214 18x18 Amber(100)",
    "-- weapon 11 ReserveOnly 0/15/0",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(1) 266,216 12x16 Amber(100)",
    "Digit(5) 278,216 12x16 Amber(100)",
    "AmmoIcon(11) 290,214 18x18 Amber(100)",
    "-- weapon 12 ReserveOnly 0/5/0",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(5) 278,216 12x16 Amber(100)",
    "AmmoIcon(12) 290,214 18x18 Amber(100)",
    "-- weapon 13 ReserveOnly 0/5/0",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(5) 278,216 12x16 Amber(100)",
    "AmmoIcon(13) 290,214 18x18 Amber(100)",
];
const ARMOUR: &[&str] = &[
    "-- armour 0",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "-- armour 1",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,232 20x1 Amber(100) skip 0,19",
    "-- armour 4",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,232 20x1 Amber(100) skip 0,19",
    "-- armour 5",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,232 20x1 Amber(100) skip 0,19",
    "-- armour 50",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,223 20x10 Amber(100) skip 0,10",
    "-- armour 94",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,214 20x19 Amber(100) skip 0,1",
    "-- armour 95",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,214 20x19 Amber(100) skip 0,1",
    "-- armour 99",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,213 20x20 Amber(100)",
    "-- armour 100",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,213 20x20 Amber(100)",
    "-- armour 150",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,213 20x20 Amber(100)",
];
const BUSIEST: &[&str] = &[
    "Crosshair(3) 148,108 24x24 White",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 129,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 44,22 80x20 Amber(255)",
    "WeaponPicture(3) 44,22 80x20 Amber(255)",
    "TrainFrame(2) 110,208 16x16 Amber(255)",
    "HealthCross 8,216 16x16 Amber(100)",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,213 20x20 Amber(100)",
    "Digit(1) 84,216 12x16 Amber(100)",
    "Digit(0) 96,216 12x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(1) 206,216 12x16 Amber(100)",
    "Digit(0) 218,216 12x16 Amber(100)",
    "Digit(0) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(1) 255,216 12x16 Amber(100)",
    "Digit(0) 267,216 12x16 Amber(100)",
    "Digit(0) 279,216 12x16 Amber(100)",
    "AmmoIcon(3) 291,214 18x18 Amber(100)",
    "Digit(1) 254,196 12x16 Amber(100)",
    "Digit(0) 266,196 12x16 Amber(100)",
    "Digit(0) 278,196 12x16 Amber(100)",
    "GrenadeAmmoIcon 290,194 18x18 Amber(100)",
    "FlashlightEmpty 293,8 18x16 Amber(225)",
    "FlashlightBeam 311,8 6x16 Amber(225)",
    "FlashlightFull 302,8 9x16 Amber(225) skip 9,0",
    "HealthKitIcon 290,168 20x20 Amber(80)",
];
const FADE: &[&str] = &[
    "-1: Amber(100) Amber(100) Amber(100)",
    "0: Amber(228) Amber(228) Amber(200)",
    "1: Amber(226) Amber(226) Amber(199)",
    "2: Amber(225) Amber(225) Amber(198)",
    "3: Amber(224) Amber(224) Amber(197)",
    "10: Amber(215) Amber(215) Amber(190)",
    "49: Amber(165) Amber(165) Amber(151)",
    "50: Amber(164) Amber(164) Amber(150)",
    "77: Amber(129) Amber(129) Amber(123)",
    "98: Amber(102) Amber(102) Amber(102)",
    "99: Amber(101) Amber(101) Amber(101)",
    "100: Amber(100) Amber(100) Amber(100)",
    "101: Amber(100) Amber(100) Amber(100)",
    "150: Amber(100) Amber(100) Amber(100)",
    "160: Amber(100) Amber(100) Amber(100)",
    "0: Red(255) Amber(100) Amber(100)",
    "1: Red(255) Amber(100) Amber(100)",
    "2: Red(255) Amber(100) Amber(100)",
    "3: Red(255) Amber(100) Amber(100)",
    "51: Amber(226) Amber(100) Amber(100)",
];
const FLASHLIGHT: &[&str] = &[
    "-- on false charge 0",
    "FlashlightEmpty 293,8 18x16 Red(100)",
    "-- on false charge 1",
    "FlashlightEmpty 293,8 18x16 Red(100)",
    "FlashlightFull 310,8 1x16 Red(100) skip 17,0",
    "-- on false charge 5",
    "FlashlightEmpty 293,8 18x16 Red(100)",
    "FlashlightFull 310,8 1x16 Red(100) skip 17,0",
    "-- on false charge 6",
    "FlashlightEmpty 293,8 18x16 Red(100)",
    "FlashlightFull 309,8 2x16 Red(100) skip 16,0",
    "-- on false charge 19",
    "FlashlightEmpty 293,8 18x16 Red(100)",
    "FlashlightFull 307,8 4x16 Red(100) skip 14,0",
    "-- on false charge 20",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 307,8 4x16 Amber(100) skip 14,0",
    "-- on false charge 50",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 302,8 9x16 Amber(100) skip 9,0",
    "-- on false charge 94",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 294,8 17x16 Amber(100) skip 1,0",
    "-- on false charge 95",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
    "-- on false charge 99",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
    "-- on false charge 100",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
    "-- on false charge 101",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
    "-- on false charge 255",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
    "-- on true charge 0",
    "FlashlightEmpty 293,8 18x16 Red(225)",
    "FlashlightBeam 311,8 6x16 Red(225)",
    "-- on true charge 1",
    "FlashlightEmpty 293,8 18x16 Red(225)",
    "FlashlightBeam 311,8 6x16 Red(225)",
    "FlashlightFull 310,8 1x16 Red(225) skip 17,0",
    "-- on true charge 5",
    "FlashlightEmpty 293,8 18x16 Red(225)",
    "FlashlightBeam 311,8 6x16 Red(225)",
    "FlashlightFull 310,8 1x16 Red(225) skip 17,0",
    "-- on true charge 6",
    "FlashlightEmpty 293,8 18x16 Red(225)",
    "FlashlightBeam 311,8 6x16 Red(225)",
    "FlashlightFull 309,8 2x16 Red(225) skip 16,0",
    "-- on true charge 19",
    "FlashlightEmpty 293,8 18x16 Red(225)",
    "FlashlightBeam 311,8 6x16 Red(225)",
    "FlashlightFull 307,8 4x16 Red(225) skip 14,0",
    "-- on true charge 20",
    "FlashlightEmpty 293,8 18x16 Amber(225)",
    "FlashlightBeam 311,8 6x16 Amber(225)",
    "FlashlightFull 307,8 4x16 Amber(225) skip 14,0",
    "-- on true charge 50",
    "FlashlightEmpty 293,8 18x16 Amber(225)",
    "FlashlightBeam 311,8 6x16 Amber(225)",
    "FlashlightFull 302,8 9x16 Amber(225) skip 9,0",
    "-- on true charge 94",
    "FlashlightEmpty 293,8 18x16 Amber(225)",
    "FlashlightBeam 311,8 6x16 Amber(225)",
    "FlashlightFull 294,8 17x16 Amber(225) skip 1,0",
    "-- on true charge 95",
    "FlashlightEmpty 293,8 18x16 Amber(225)",
    "FlashlightBeam 311,8 6x16 Amber(225)",
    "FlashlightFull 293,8 18x16 Amber(225)",
    "-- on true charge 99",
    "FlashlightEmpty 293,8 18x16 Amber(225)",
    "FlashlightBeam 311,8 6x16 Amber(225)",
    "FlashlightFull 293,8 18x16 Amber(225)",
    "-- on true charge 100",
    "FlashlightEmpty 293,8 18x16 Amber(225)",
    "FlashlightBeam 311,8 6x16 Amber(225)",
    "FlashlightFull 293,8 18x16 Amber(225)",
    "-- on true charge 101",
    "FlashlightEmpty 293,8 18x16 Amber(225)",
    "FlashlightBeam 311,8 6x16 Amber(225)",
    "FlashlightFull 293,8 18x16 Amber(225)",
    "-- on true charge 255",
    "FlashlightEmpty 293,8 18x16 Amber(225)",
    "FlashlightBeam 311,8 6x16 Amber(225)",
    "FlashlightFull 293,8 18x16 Amber(225)",
];
const LOW_HEALTH: &[&str] = &[
    "-- health 0",
    "HealthCross 8,216 16x16 Red(255)",
    "Digit(0) 46,216 12x16 Red(255)",
    "Divider 64,216 1x16 Red(255)",
    "-- health 1",
    "HealthCross 8,216 16x16 Red(255)",
    "Digit(1) 46,216 12x16 Red(255)",
    "Divider 64,216 1x16 Red(255)",
    "-- health 15",
    "HealthCross 8,216 16x16 Red(255)",
    "Digit(1) 34,216 12x16 Red(255)",
    "Digit(5) 46,216 12x16 Red(255)",
    "Divider 64,216 1x16 Red(255)",
    "-- health 16",
    "HealthCross 8,216 16x16 Red(100)",
    "Digit(1) 34,216 12x16 Red(100)",
    "Digit(6) 46,216 12x16 Red(100)",
    "Divider 64,216 1x16 Red(100)",
    "-- health 25",
    "HealthCross 8,216 16x16 Red(100)",
    "Digit(2) 34,216 12x16 Red(100)",
    "Digit(5) 46,216 12x16 Red(100)",
    "Divider 64,216 1x16 Red(100)",
    "-- health 26",
    "HealthCross 8,216 16x16 Amber(100)",
    "Digit(2) 34,216 12x16 Amber(100)",
    "Digit(6) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "-- health 100",
    "HealthCross 8,216 16x16 Amber(100)",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
];
const NUMBERS: &[&str] = &[
    "-- value 0",
    "Crosshair(3) 148,108 24x24 White",
    "HealthCross 8,216 16x16 Red(255)",
    "Digit(0) 46,216 12x16 Red(255)",
    "Divider 64,216 1x16 Red(255)",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(0) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(0) 279,216 12x16 Amber(100)",
    "AmmoIcon(3) 291,214 18x18 Amber(100)",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
    "-- value 7",
    "Crosshair(3) 148,108 24x24 White",
    "HealthCross 8,216 16x16 Red(255)",
    "Digit(7) 46,216 12x16 Red(255)",
    "Divider 64,216 1x16 Red(255)",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,231 20x2 Amber(100) skip 0,18",
    "Digit(7) 108,216 12x16 Amber(100)",
    "Digit(7) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(7) 279,216 12x16 Amber(100)",
    "AmmoIcon(3) 291,214 18x18 Amber(100)",
    "Digit(7) 278,196 12x16 Amber(100)",
    "GrenadeAmmoIcon 290,194 18x18 Amber(100)",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
    "-- value 42",
    "Crosshair(3) 148,108 24x24 White",
    "HealthCross 8,216 16x16 Amber(100)",
    "Digit(4) 34,216 12x16 Amber(100)",
    "Digit(2) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,224 20x9 Amber(100) skip 0,11",
    "Digit(4) 96,216 12x16 Amber(100)",
    "Digit(2) 108,216 12x16 Amber(100)",
    "Digit(4) 218,216 12x16 Amber(100)",
    "Digit(2) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(4) 267,216 12x16 Amber(100)",
    "Digit(2) 279,216 12x16 Amber(100)",
    "AmmoIcon(3) 291,214 18x18 Amber(100)",
    "Digit(4) 266,196 12x16 Amber(100)",
    "Digit(2) 278,196 12x16 Amber(100)",
    "GrenadeAmmoIcon 290,194 18x18 Amber(100)",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
    "-- value 100",
    "Crosshair(3) 148,108 24x24 White",
    "HealthCross 8,216 16x16 Amber(100)",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,213 20x20 Amber(100)",
    "Digit(1) 84,216 12x16 Amber(100)",
    "Digit(0) 96,216 12x16 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "Digit(1) 206,216 12x16 Amber(100)",
    "Digit(0) 218,216 12x16 Amber(100)",
    "Digit(0) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(1) 255,216 12x16 Amber(100)",
    "Digit(0) 267,216 12x16 Amber(100)",
    "Digit(0) 279,216 12x16 Amber(100)",
    "AmmoIcon(3) 291,214 18x18 Amber(100)",
    "Digit(1) 254,196 12x16 Amber(100)",
    "Digit(0) 266,196 12x16 Amber(100)",
    "Digit(0) 278,196 12x16 Amber(100)",
    "GrenadeAmmoIcon 290,194 18x18 Amber(100)",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
    "-- value 999",
    "Crosshair(3) 148,108 24x24 White",
    "HealthCross 8,216 16x16 Amber(100)",
    "Digit(9) 22,216 12x16 Amber(100)",
    "Digit(9) 34,216 12x16 Amber(100)",
    "Digit(9) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,213 20x20 Amber(100)",
    "Digit(9) 84,216 12x16 Amber(100)",
    "Digit(9) 96,216 12x16 Amber(100)",
    "Digit(9) 108,216 12x16 Amber(100)",
    "Digit(9) 206,216 12x16 Amber(100)",
    "Digit(9) 218,216 12x16 Amber(100)",
    "Digit(9) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(9) 255,216 12x16 Amber(100)",
    "Digit(9) 267,216 12x16 Amber(100)",
    "Digit(9) 279,216 12x16 Amber(100)",
    "AmmoIcon(3) 291,214 18x18 Amber(100)",
    "Digit(9) 254,196 12x16 Amber(100)",
    "Digit(9) 266,196 12x16 Amber(100)",
    "Digit(9) 278,196 12x16 Amber(100)",
    "GrenadeAmmoIcon 290,194 18x18 Amber(100)",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
    "-- value 1234",
    "Crosshair(3) 148,108 24x24 White",
    "HealthCross 8,216 16x16 Amber(100)",
    "Digit(9) 22,216 12x16 Amber(100)",
    "Digit(9) 34,216 12x16 Amber(100)",
    "Digit(9) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "SuitFull 64,213 20x20 Amber(100)",
    "Digit(9) 84,216 12x16 Amber(100)",
    "Digit(9) 96,216 12x16 Amber(100)",
    "Digit(9) 108,216 12x16 Amber(100)",
    "Digit(9) 206,216 12x16 Amber(100)",
    "Digit(9) 218,216 12x16 Amber(100)",
    "Digit(9) 230,216 12x16 Amber(100)",
    "Divider 248,216 1x16 Amber(100)",
    "Digit(9) 255,216 12x16 Amber(100)",
    "Digit(9) 267,216 12x16 Amber(100)",
    "Digit(9) 279,216 12x16 Amber(100)",
    "AmmoIcon(3) 291,214 18x18 Amber(100)",
    "Digit(9) 254,196 12x16 Amber(100)",
    "Digit(9) 266,196 12x16 Amber(100)",
    "Digit(9) 278,196 12x16 Amber(100)",
    "GrenadeAmmoIcon 290,194 18x18 Amber(100)",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
];
const PICKUP: &[&str] = &[
    "-- kind 0 ticks 0",
    "-- kind 0 ticks 1",
    "-- kind 0 ticks 2",
    "-- kind 0 ticks 20",
    "-- kind 0 ticks 40",
    "-- kind 0 ticks 63",
    "-- kind 0 ticks 64",
    "-- kind 0 ticks 100",
    "-- kind 1 ticks 0",
    "-- kind 1 ticks 1",
    "-- kind 1 ticks 2",
    "-- kind 1 ticks 20",
    "-- kind 1 ticks 40",
    "-- kind 1 ticks 63",
    "-- kind 1 ticks 64",
    "-- kind 1 ticks 100",
    "-- kind 2 ticks 0",
    "-- kind 2 ticks 1",
    "BatteryIcon 290,168 20x20 Amber(4)",
    "-- kind 2 ticks 2",
    "BatteryIcon 290,168 20x20 Amber(8)",
    "-- kind 2 ticks 20",
    "BatteryIcon 290,168 20x20 Amber(80)",
    "-- kind 2 ticks 40",
    "BatteryIcon 290,168 20x20 Amber(160)",
    "-- kind 2 ticks 63",
    "BatteryIcon 290,168 20x20 Amber(252)",
    "-- kind 2 ticks 64",
    "BatteryIcon 290,168 20x20 Amber(255)",
    "-- kind 2 ticks 100",
    "BatteryIcon 290,168 20x20 Amber(255)",
    "-- kind 3 ticks 0",
    "-- kind 3 ticks 1",
    "HealthKitIcon 290,168 20x20 Amber(4)",
    "-- kind 3 ticks 2",
    "HealthKitIcon 290,168 20x20 Amber(8)",
    "-- kind 3 ticks 20",
    "HealthKitIcon 290,168 20x20 Amber(80)",
    "-- kind 3 ticks 40",
    "HealthKitIcon 290,168 20x20 Amber(160)",
    "-- kind 3 ticks 63",
    "HealthKitIcon 290,168 20x20 Amber(252)",
    "-- kind 3 ticks 64",
    "HealthKitIcon 290,168 20x20 Amber(255)",
    "-- kind 3 ticks 100",
    "HealthKitIcon 290,168 20x20 Amber(255)",
    "-- kind 4 ticks 0",
    "-- kind 4 ticks 1",
    "LongJumpIcon 290,168 20x20 Amber(4)",
    "-- kind 4 ticks 2",
    "LongJumpIcon 290,168 20x20 Amber(8)",
    "-- kind 4 ticks 20",
    "LongJumpIcon 290,168 20x20 Amber(80)",
    "-- kind 4 ticks 40",
    "LongJumpIcon 290,168 20x20 Amber(160)",
    "-- kind 4 ticks 63",
    "LongJumpIcon 290,168 20x20 Amber(252)",
    "-- kind 4 ticks 64",
    "LongJumpIcon 290,168 20x20 Amber(255)",
    "-- kind 4 ticks 100",
    "LongJumpIcon 290,168 20x20 Amber(255)",
    "-- kind 5 ticks 0",
    "-- kind 5 ticks 1",
    "-- kind 5 ticks 2",
    "-- kind 5 ticks 20",
    "-- kind 5 ticks 40",
    "-- kind 5 ticks 63",
    "-- kind 5 ticks 64",
    "-- kind 5 ticks 100",
];
const SELECTION: &[&str] = &[
    "-- weapon 0",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 95,10 12x12 Amber(255)",
    "SlotNumber(2) 112,10 12x12 Amber(255)",
    "SlotNumber(3) 129,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 10,22 80x20 Amber(255)",
    "WeaponPicture(0) 10,22 80x20 Amber(255)",
    "-- weapon 1",
    "Crosshair(1) 148,108 24x24 White",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 112,10 12x12 Amber(255)",
    "SlotNumber(3) 129,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 27,22 80x20 Amber(255)",
    "WeaponPicture(1) 27,22 80x20 Amber(255)",
    "-- weapon 2",
    "Crosshair(2) 148,108 24x24 White",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 112,10 12x12 Amber(255)",
    "SlotNumber(3) 129,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 27,22 80x20 Amber(255)",
    "WeaponPicture(2) 27,22 80x20 Amber(255)",
    "-- weapon 3",
    "Crosshair(3) 148,108 24x24 White",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 129,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 44,22 80x20 Amber(255)",
    "WeaponPicture(3) 44,22 80x20 Amber(255)",
    "-- weapon 4",
    "Crosshair(4) 148,108 24x24 White",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 129,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 44,22 80x20 Amber(255)",
    "WeaponPicture(4) 44,22 80x20 Amber(255)",
    "-- weapon 5",
    "Crosshair(5) 148,108 24x24 White",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 129,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 44,22 80x20 Amber(255)",
    "WeaponPicture(5) 44,22 80x20 Amber(255)",
    "-- weapon 6",
    "Crosshair(6) 148,108 24x24 White",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 61,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 61,22 80x20 Amber(255)",
    "WeaponPicture(6) 61,22 80x20 Amber(255)",
    "-- weapon 7",
    "Crosshair(7) 148,108 24x24 White",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 61,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 61,22 80x20 Amber(255)",
    "WeaponPicture(7) 61,22 80x20 Amber(255)",
    "-- weapon 8",
    "Crosshair(8) 148,108 24x24 White",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 61,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 61,22 80x20 Amber(255)",
    "WeaponPicture(8) 61,22 80x20 Amber(255)",
    "-- weapon 9",
    "Crosshair(9) 148,108 24x24 White",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 61,10 12x12 Amber(255)",
    "SlotNumber(4) 146,10 12x12 Amber(255)",
    "SelectionFrame 61,22 80x20 Amber(255)",
    "WeaponPicture(9) 61,22 80x20 Amber(255)",
    "-- weapon 10",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 61,10 12x12 Amber(255)",
    "SlotNumber(4) 78,10 12x12 Amber(255)",
    "SelectionFrame 78,22 80x20 Amber(255)",
    "WeaponPicture(10) 78,22 80x20 Amber(255)",
    "-- weapon 11",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 61,10 12x12 Amber(255)",
    "SlotNumber(4) 78,10 12x12 Amber(255)",
    "SelectionFrame 78,22 80x20 Amber(255)",
    "WeaponPicture(11) 78,22 80x20 Amber(255)",
    "-- weapon 12",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 61,10 12x12 Amber(255)",
    "SlotNumber(4) 78,10 12x12 Amber(255)",
    "SelectionFrame 78,22 80x20 Amber(255)",
    "WeaponPicture(12) 78,22 80x20 Amber(255)",
    "-- weapon 13",
    "SlotNumber(0) 10,10 12x12 Amber(255)",
    "SlotNumber(1) 27,10 12x12 Amber(255)",
    "SlotNumber(2) 44,10 12x12 Amber(255)",
    "SlotNumber(3) 61,10 12x12 Amber(255)",
    "SlotNumber(4) 78,10 12x12 Amber(255)",
    "SelectionFrame 78,22 80x20 Amber(255)",
    "WeaponPicture(13) 78,22 80x20 Amber(255)",
];
const SIGHTS: &[&str] = &[
    "-- weapon 0 zoomed false",
    "-- weapon 0 zoomed true",
    "-- weapon 1 zoomed false",
    "Crosshair(1) 148,108 24x24 White",
    "-- weapon 1 zoomed true",
    "Crosshair(1) 148,108 24x24 White",
    "-- weapon 2 zoomed false",
    "Crosshair(2) 148,108 24x24 White",
    "-- weapon 2 zoomed true",
    "Crosshair(2) 148,108 24x24 White",
    "-- weapon 3 zoomed false",
    "Crosshair(3) 148,108 24x24 White",
    "-- weapon 3 zoomed true",
    "Crosshair(3) 148,108 24x24 White",
    "-- weapon 4 zoomed false",
    "Crosshair(4) 148,108 24x24 White",
    "-- weapon 4 zoomed true",
    "Crosshair(4) 148,108 24x24 White",
    "-- weapon 5 zoomed false",
    "Crosshair(5) 148,108 24x24 White",
    "-- weapon 5 zoomed true",
    "ZoomReticle 108,112 104x16 White",
    "-- weapon 6 zoomed false",
    "Crosshair(6) 148,108 24x24 White",
    "-- weapon 6 zoomed true",
    "Crosshair(6) 148,108 24x24 White",
    "-- weapon 7 zoomed false",
    "Crosshair(7) 148,108 24x24 White",
    "-- weapon 7 zoomed true",
    "Crosshair(7) 148,108 24x24 White",
    "-- weapon 8 zoomed false",
    "Crosshair(8) 148,108 24x24 White",
    "-- weapon 8 zoomed true",
    "Crosshair(8) 148,108 24x24 White",
    "-- weapon 9 zoomed false",
    "Crosshair(9) 148,108 24x24 White",
    "-- weapon 9 zoomed true",
    "Crosshair(9) 148,108 24x24 White",
    "-- weapon 10 zoomed false",
    "-- weapon 10 zoomed true",
    "-- weapon 11 zoomed false",
    "-- weapon 11 zoomed true",
    "-- weapon 12 zoomed false",
    "-- weapon 12 zoomed true",
    "-- weapon 13 zoomed false",
    "-- weapon 13 zoomed true",
];
const SUIT_ONLY: &[&str] = &[
    "HealthCross 8,216 16x16 Amber(100)",
    "Digit(1) 22,216 12x16 Amber(100)",
    "Digit(0) 34,216 12x16 Amber(100)",
    "Digit(0) 46,216 12x16 Amber(100)",
    "Divider 64,216 1x16 Amber(100)",
    "SuitEmpty 64,213 20x20 Amber(100)",
    "Digit(0) 108,216 12x16 Amber(100)",
    "FlashlightEmpty 293,8 18x16 Amber(100)",
    "FlashlightFull 293,8 18x16 Amber(100)",
];
const TRAIN: &[&str] = &[
    "-- setting 0",
    "-- setting 1",
    "TrainFrame(0) 110,208 16x16 Amber(255)",
    "-- setting 2",
    "TrainFrame(1) 110,208 16x16 Amber(255)",
    "-- setting 3",
    "TrainFrame(2) 110,208 16x16 Amber(255)",
    "-- setting 4",
    "TrainFrame(3) 110,208 16x16 Amber(255)",
    "-- setting 5",
    "TrainFrame(4) 110,208 16x16 Amber(255)",
    "-- setting 6",
];

const PACKETS: &[&str] = &[
    "-- busiest 34",
    "66808080 006c0094 7e3e5430 00180018",
    "66808080 000a000a 7e3c2400 000c000c",
    "66808080 000a001b 7e3c240c 000c000c",
    "66808080 000a002c 7e3c2418 000c000c",
    "66808080 000a0081 7e3c2424 000c000c",
    "66808080 000a0092 7e3c2430 000c000c",
    "66808080 0016002c 7e3c1000 00140050",
    "66808080 0016002c 7e3ca850 00140050",
    "66808080 00d0006e 7e3ca0f0 00100010",
    "66323232 00d80008 7e3c1078 00100010",
    "66323232 00d80016 7e3c000c 0010000c",
    "66323232 00d80022 7e3c0000 0010000c",
    "66323232 00d8002e 7e3c0000 0010000c",
    "66323232 00d80040 7e3c1088 00100001",
    "66323232 00d50040 7e3c1064 00140014",
    "66323232 00d50040 7e3c1050 00140014",
    "66323232 00d80054 7e3c000c 0010000c",
    "66323232 00d80060 7e3c0000 0010000c",
    "66323232 00d8006c 7e3c0000 0010000c",
    "66323232 00d800ce 7e3c000c 0010000c",
    "66323232 00d800da 7e3c0000 0010000c",
    "66323232 00d800e6 7e3c0000 0010000c",
    "66323232 00d800f8 7e3c1088 00100001",
    "66323232 00d800ff 7e3c000c 0010000c",
    "66323232 00d8010b 7e3c0000 0010000c",
    "66323232 00d80117 7e3c0000 0010000c",
    "66323232 00d60123 7e3c3036 00120012",
    "66323232 00c400fe 7e3c000c 0010000c",
    "66323232 00c4010a 7e3c0000 0010000c",
    "66323232 00c40116 7e3c0000 0010000c",
    "66323232 00c20122 7e3c307e 00120012",
    "66717171 00080125 7e3c6c82 00100012",
    "66717171 00080137 7e3c6c94 00100006",
    "66717171 0008012e 7e3c6c79 00100009",
    "-- hurt 18",
    "66808080 0070006c 7e3e8400 00100068",
    "66000072 00d80008 7e3c1078 00100010",
    "66000072 00d80022 7e3c0018 0010000c",
    "66000072 00d8002e 7e3c0000 0010000c",
    "66000072 00d80040 7e3c1088 00100001",
    "66323232 00d50040 7e3c1064 00140014",
    "66323232 00d50040 7e3c1050 00140014",
    "66323232 00d80054 7e3c000c 0010000c",
    "66323232 00d80060 7e3c0000 0010000c",
    "66323232 00d8006c 7e3c0000 0010000c",
    "66646464 00d800e6 7e3c003c 0010000c",
    "66646464 00d800f8 7e3c1088 00100001",
    "66646464 00d8010b 7e3c003c 0010000c",
    "66646464 00d80117 7e3c0000 0010000c",
    "66646464 00d60123 7e3c305a 00120012",
    "66717171 00080125 7e3c6c82 00100012",
    "66717171 00080137 7e3c6c94 00100006",
    "66717171 0008012e 7e3c6c79 00100009",
    "-- plain 11",
    "66727272 00d80008 7e3c1078 00100010",
    "66727272 00d80016 7e3c000c 0010000c",
    "66727272 00d80022 7e3c0000 0010000c",
    "66727272 00d8002e 7e3c0000 0010000c",
    "66727272 00d80040 7e3c1088 00100001",
    "66727272 00d50040 7e3c1064 00140014",
    "66727272 00d8006c 7e3c0000 0010000c",
    "66000071 00080125 7e3c6c82 00100012",
    "66000071 00080137 7e3c6c94 00100006",
    "66000071 00080135 7e3c6c80 00100002",
    "66424242 00a80122 7e3c6c5c 00140014",
];
