//! Resident room staging capacity shared by the guest build and host audits.

/// Keep the installed 0170 map window when the resident room format shrinks.
/// This is a capacity floor, not permission to reduce a stack or model guard.
pub const INSTALLED_MAP_WORDS_FLOOR: usize = 180_168;

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
        assert_eq!(map_staging_words(720_604), INSTALLED_MAP_WORDS_FLOOR);
        assert_eq!(map_staging_words(800_001), 200_001);
    }
}
