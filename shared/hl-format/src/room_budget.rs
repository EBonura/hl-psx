//! Resident room staging capacity shared by the guest build and host audits.

/// Keep the installed map window when the resident room format shrinks.
/// This is a capacity floor, not permission to reduce a stack or model guard.
///
/// The largest cooked room (c5a1, 695,220 B) needs 173,821 words staged. The
/// floor sits above it because the weapon cache in the arena tail is sized from
/// the same arena: at 180,168 words c5a1's tail margin was 13,472 B, and
/// `cargo hl-build audit` (weapon cache RAM audit) now reports 5,280 B at 178,120
/// words, the tightest map. Lowering the floor further spends that margin.
pub const INSTALLED_MAP_WORDS_FLOOR: usize = 178_120;

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
        assert_eq!(map_staging_words(712_480), INSTALLED_MAP_WORDS_FLOOR);
        assert_eq!(map_staging_words(800_001), 200_001);
    }
}
