//! Suit armour: with armour the player keeps a fifth of a hit on health and
//! spends one armour point per two points of the rest; when armour runs out
//! the uncovered remainder lands on health; falls ignore armour.

use crate::player_rules::{apply_player_damage, PlayerDamageKind, Vitals};

fn hit(health: u16, armor: u16, dmg: u16) -> (u16, u16) {
    let v = apply_player_damage(Vitals { health, armor }, dmg, PlayerDamageKind::Generic);
    (v.health, v.armor)
}

fn fall(health: u16, armor: u16, dmg: u16) -> (u16, u16) {
    let v = apply_player_damage(Vitals { health, armor }, dmg, PlayerDamageKind::Fall);
    (v.health, v.armor)
}

#[test]
fn with_plenty_of_armour_a_fifth_reaches_health() {
    for dmg in [5u16, 10, 25, 50, 100] {
        let (h, a) = hit(200, 200, dmg);
        assert_eq!(200 - h, dmg / 5, "health loss for {dmg}");
        // Half of the remaining four fifths, rounded up.
        let rest = dmg - dmg / 5;
        assert_eq!(200 - a, rest.div_ceil(2), "armour spent for {dmg}");
    }
}

#[test]
fn without_armour_all_damage_reaches_health() {
    assert_eq!(hit(100, 0, 30), (70, 0));
    assert_eq!(hit(10, 0, 40), (0, 0));
}

#[test]
fn running_out_of_armour_lets_the_uncovered_part_through() {
    // 5 armour covers 10 damage; the other 40 of a 50 hit lands on health.
    assert_eq!(hit(100, 5, 50), (60, 0));
    // Armour is drained to zero, never negative.
    assert_eq!(hit(100, 1, 10).1, 0);
}

#[test]
fn falls_ignore_armour() {
    assert_eq!(fall(100, 100, 50), (50, 100));
    assert_eq!(fall(100, 0, 30), (70, 0));
    assert_eq!(fall(20, 100, 200), (0, 100));
}

#[test]
fn health_never_wraps_below_zero() {
    assert_eq!(hit(20, 100, 200).0, 0);
    assert_eq!(hit(100, 100, u16::MAX), (0, 0));
}

/// Recorded from ac83da7 behaviour: (health, armour, damage) followed by the
/// result of an ordinary hit and of a fall of the same size.
#[test]
fn golden_damage_table() {
    let table: &[((u16, u16, u16), (u16, u16), (u16, u16))] = &[
        ((100, 100, 10), (98, 96), (90, 100)),
        ((100, 100, 1), (100, 99), (99, 100)),
        ((100, 100, 2), (100, 99), (98, 100)),
        ((100, 100, 3), (100, 98), (97, 100)),
        ((100, 100, 7), (99, 97), (93, 100)),
        ((100, 100, 50), (90, 80), (50, 100)),
        ((100, 100, 100), (80, 60), (0, 100)),
        ((100, 5, 50), (60, 0), (50, 5)),
        ((100, 1, 10), (92, 0), (90, 1)),
        ((100, 0, 30), (70, 0), (70, 0)),
        ((20, 100, 200), (0, 20), (0, 100)),
        ((10, 0, 40), (0, 0), (0, 0)),
        ((100, 30, 60), (88, 6), (40, 30)),
        ((100, 25, 63), (87, 0), (37, 25)),
        ((100, 100, 65535), (0, 0), (0, 100)),
        ((100, 7, 15), (97, 1), (85, 7)),
    ];
    for &((h, a, d), generic, falling) in table {
        assert_eq!(hit(h, a, d), generic, "hit {h}/{a} by {d}");
        assert_eq!(fall(h, a, d), falling, "fall {h}/{a} by {d}");
    }
}
