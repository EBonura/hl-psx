//! Resident room staging capacity shared by the guest build and host audits.

/// Keep the installed map window when the resident room format shrinks.
/// This is a capacity floor, not permission to reduce a stack or model guard.
///
/// The largest cooked room (c5a1, 695,220 B) needs 173,821 words staged. The
/// floor sits above it because the weapon cache in the arena tail is sized from
/// the same arena (`cargo hl-build audit`, weapon cache RAM audit: the minimum
/// margin is at c5a1, 5,280 B or more is kept). History: 180,168 words left
/// 13,472 B; 178,120 left 5,280 B; the actor tracks at 1.4 grid steps (hl-bsp
/// HMA1_NPC_MIN_TOLERANCE) and the authored HECU grenade clips together leave
/// 13,844 B at 179,400, so 177,280 words keeps 5,364 B with both. Lowering the
/// floor further spends that margin.
pub const INSTALLED_MAP_WORDS_FLOOR: usize = 177_280;

pub const fn map_staging_words(max_room_bytes: usize) -> usize {
    let cooked_words = max_room_bytes.div_ceil(4);
    if cooked_words > INSTALLED_MAP_WORDS_FLOOR {
        cooked_words
    } else {
        INSTALLED_MAP_WORDS_FLOOR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_keeps_installed_capacity_and_grows_for_larger_rooms() {
        assert_eq!(map_staging_words(690_520), INSTALLED_MAP_WORDS_FLOOR);
        assert_eq!(map_staging_words(709_120), INSTALLED_MAP_WORDS_FLOOR);
        assert_eq!(map_staging_words(800_001), 200_001);
    }
}
