//! Authored weaponbox ammunition and the retail touch admission rule.

/// Slots are the existing Arsenal ammo pools, including the distinct AR grenade pool.
pub fn ammo_slot(name: &str) -> Option<u16> {
    const KEYS: [(&str, u16); 12] = [
        ("9mm", 1), ("357", 2), ("buckshot", 3), ("bolts", 4),
        ("rockets", 5), ("uranium", 6), ("Hornets", 7),
        ("Hand Grenade", 8), ("Snarks", 9), ("Satchel Charge", 10),
        ("Trip Mine", 11), ("ARgrenades", 12),
    ];
    KEYS.iter().find(|(key, _)| name.eq_ignore_ascii_case(key)).map(|(_, slot)| *slot)
}

pub const fn can_touch(grounded: bool, player_alive: bool) -> bool {
    grounded && player_alive
}

pub fn touches(player: [i32; 3], half_height: i32, mins: [i32; 3], maxs: [i32; 3]) -> bool {
    let half = [16, half_height, 16];
    (0..3).all(|axis| player[axis] + half[axis] >= mins[axis]
        && player[axis] - half[axis] <= maxs[axis])
}

/// The box clears its named slots after GiveAmmo and then removes itself,
/// including when the player is already full. Excess is not retained.
pub const fn after_give(current: u16, amount: u16, cap: u16) -> u16 {
    let total = current.saturating_add(amount);
    if total < cap { total } else { cap }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airborne_boxes_or_dead_players_cannot_consume_a_box() {
        assert!(!can_touch(false, true));
        assert!(!can_touch(true, false));
        assert!(can_touch(true, true));
    }

    #[test]
    fn partial_and_full_caps_discard_excess_after_the_valid_touch() {
        assert_eq!(after_give(240, 50, 250), 250);
        assert_eq!(after_give(250, 50, 250), 250);
        assert_eq!(after_give(0, 60, 100), 60);
        assert_eq!(after_give(u16::MAX, 1, 250), 250);
    }

    #[test]
    fn named_ammunition_keeps_grenades_and_shared_pools_distinct() {
        assert_eq!(ammo_slot("9mm"), Some(1));
        assert_eq!(ammo_slot("ARgrenades"), Some(12));
        assert_eq!(ammo_slot("Hand Grenade"), Some(8));
        assert_eq!(ammo_slot("Satchel Charge"), Some(10));
        assert_eq!(ammo_slot("Trip Mine"), Some(11));
        assert_eq!(ammo_slot("weapon_mp5"), None);
    }

    #[test]
    fn original_box_bounds_require_real_player_hull_contact() {
        assert!(touches([32, 36, 0], 36, [-16, 0, -16], [16, 16, 16]));
        assert!(!touches([33, 36, 0], 36, [-16, 0, -16], [16, 16, 16]));
        assert!(!touches([0, 53, 0], 36, [-16, 0, -16], [16, 16, 16]));
    }
}
