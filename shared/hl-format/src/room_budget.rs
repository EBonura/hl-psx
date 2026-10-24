//! Resident room staging capacity shared by the guest build and host audits.

/// Floor of the shared map/model arena's map window, in words. The window must
/// hold the largest cooked room (c5a1 after the delta-chain PVS rows, 659,004 B
/// staged in 164,768 words), but it is kept higher than that on purpose: the
/// arena tail after the resident map and models feeds the weapon cache and the
/// actor packet stream (`use_model_tail` in the renderer chooses the larger of
/// that tail and the packet arena's remainder), and every word the window gives
/// up comes out of every map's tail. A map whose room shrank by less than the
/// window did would lose actor packet capacity, which can drop actor triangles
/// in dense frames. The floor therefore gives up no more than the smallest room
/// shrink among the maps with a small tail: 20,000 B (c1a2 saved 20,552 B when
/// the PVS rows became delta chains). Lowering it further needs those rooms to
/// shrink again; `cargo hl-build audit` still proves the weapon cache fits.
pub const INSTALLED_MAP_WORDS_FLOOR: usize = 172_280;

pub const fn map_staging_words(max_room_bytes: usize) -> usize {
    let cooked_words = max_room_bytes.div_ceil(4);
    if cooked_words > INSTALLED_MAP_WORDS_FLOOR {
        cooked_words
    } else {
        INSTALLED_MAP_WORDS_FLOOR
    }
}

/// Model render faces the arena's face region holds. The faces of the resident
/// actor models (a 4-byte packed-index word and a 6-byte UV payload each) are
/// baked into blocks at the top of the arena, below the end of the map and
/// model pool, instead of into fixed-size statics sized for the heaviest
/// roster in the campaign (c4a3, 8,027 faces: 80 KB of `.bss` that every other
/// map left mostly idle). The region is the roster of the heaviest map whose
/// weapon-cache tail is short enough for the actor packet stream to depend on
/// it (c4a1b, 7,179 faces): for every map up to that size the face region
/// leaves the tail exactly as the statics did, and a heavier roster (c4a3) takes
/// the rest from its own 200 KB of spare tail. The model-pool audit
/// (`cargo hl-build audit`) proves the fit of every map and transition.
pub const FACE_REGION_FACES: usize = 7_200;

/// Words one model's render faces take in the face region: one index word per
/// face, then the 6-byte payloads rounded up to a whole word.
pub const fn face_block_words(faces: usize) -> usize {
    faces + (faces * 6).div_ceil(4)
}

/// Words the arena adds for the face region.
pub const FACE_REGION_WORDS: usize = face_block_words(FACE_REGION_FACES);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_blocks_are_ten_bytes_a_face_to_the_word() {
        assert_eq!(face_block_words(0), 0);
        assert_eq!(face_block_words(1), 3);
        assert_eq!(face_block_words(2), 5);
        assert_eq!(FACE_REGION_WORDS * 4, FACE_REGION_FACES * 10);
    }

    #[test]
    fn staging_keeps_installed_capacity_and_grows_for_larger_rooms() {
        assert_eq!(map_staging_words(659_072), INSTALLED_MAP_WORDS_FLOOR);
        assert_eq!(map_staging_words(689_120), INSTALLED_MAP_WORDS_FLOOR);
        assert_eq!(map_staging_words(800_001), 200_001);
    }
}
