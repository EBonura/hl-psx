//! Shared campaign map registry and chapter ordering.

// Keep the runnable map order in sync with host/hl-content/map-list.txt.
// Hazard Course is appended so the campaign's stable 0..=95 room ids never move.
pub const CAMPAIGN_MAP_COUNT: usize = 96;
pub const TRAINING_START_ROOM: usize = CAMPAIGN_MAP_COUNT;
pub const MAPS: [&str; 103] = [
    "c0a0", "c0a0a", "c0a0b", "c0a0c", "c0a0d", "c0a0e", "c1a0", "c1a0a", "c1a0b", "c1a0c",
    "c1a0d", "c1a0e", "c1a1", "c1a1a", "c1a1b", "c1a1c", "c1a1d", "c1a1f", "c1a2", "c1a2a",
    "c1a2b", "c1a2c", "c1a2d", "c1a3", "c1a3a", "c1a3b", "c1a3c", "c1a3d", "c1a4", "c1a4b",
    "c1a4d", "c1a4e", "c1a4f", "c1a4g", "c1a4i", "c1a4j", "c1a4k", "c2a1", "c2a1a", "c2a1b",
    "c2a2", "c2a2a", "c2a2b1", "c2a2b2", "c2a2c", "c2a2d", "c2a2e", "c2a2f", "c2a2g", "c2a2h",
    "c2a3", "c2a3a", "c2a3b", "c2a3c", "c2a3d", "c2a3e", "c2a4", "c2a4a", "c2a4b", "c2a4c",
    "c2a4d", "c2a4e", "c2a4f", "c2a4g", "c2a5", "c2a5a", "c2a5b", "c2a5c", "c2a5d", "c2a5e",
    "c2a5f", "c2a5g", "c2a5w", "c2a5x", "c3a1", "c3a1a", "c3a1b", "c3a2", "c3a2a", "c3a2b",
    "c3a2c", "c3a2d", "c3a2e", "c3a2f", "c4a1", "c4a1a", "c4a1b", "c4a1c", "c4a1d", "c4a1e",
    "c4a1f", "c4a2", "c4a2a", "c4a2b", "c4a3", "c5a1", "t0a0", "t0a0a", "t0a0b", "t0a0b1",
    "t0a0b2", "t0a0c", "t0a0d",
];
// Retail's New Game chapter list, in play order. GoldSrc itself has no chapter
// select; each row starts where normal play arrives in that chapter, which is
// the map whose worldspawn carries the chapter title (C1A1TITLE is in c1a0c,
// C3A2TITLE in c3a2e) and not always the map the name suggests.
pub const CHAPTERS: [&str; 19] = [
    "Black Mesa Inbound",
    "Anomalous Materials",
    "Unforeseen Consequences",
    "Office Complex",
    "We've Got Hostiles",
    "Blast Pit",
    "Power Up",
    "On A Rail",
    "Apprehension",
    "Residue Processing",
    "Questionable Ethics",
    "Surface Tension",
    "Forget About Freeman!",
    "Lambda Core",
    "Xen",
    "Gonarch's Lair",
    "Interloper",
    "Nihilanth",
    "Endgame",
];
/// Every room in play order: the campaign as its changelevels chain it, then
/// the Hazard Course. `MAPS` stays in file order because room ids are chunk
/// ids; this is the order a player meets the maps in. Hub chapters (Blast
/// Pit, the c1a1 and c3a2 side maps) list a side map after the map it
/// branches from.
pub const PLAY_ORDER: [u8; 103] = [
    0, 1, 2, 3, 4, 5, // Black Mesa Inbound: c0a0..c0a0e
    6, 10, 7, 8, 11, // Anomalous Materials: c1a0 c1a0d c1a0a c1a0b c1a0e
    9, 12, 13, 17, 14, 15,
    16, // Unforeseen Consequences: c1a0c c1a1 c1a1a c1a1f c1a1b c1a1c c1a1d
    18, 19, 20, 21, 22, // Office Complex: c1a2..c1a2d
    23, 24, 25, 26, 27, // We've Got Hostiles: c1a3..c1a3d
    28, 36, 29, 32, 30, 31, 34, 33,
    35, // Blast Pit: c1a4 c1a4k c1a4b c1a4f c1a4d c1a4e c1a4i c1a4g c1a4j
    37, 38, 39, // Power Up: c2a1..c2a1b
    40, 41, 42, 43, 44, 45, 46, 47, 48, 49, // On A Rail: c2a2..c2a2h
    50, 51, 52, 53, 54, 55, // Apprehension: c2a3..c2a3e
    56, 57, 58, 59, // Residue Processing: c2a4..c2a4c
    60, 61, 62, 63, // Questionable Ethics: c2a4d..c2a4g
    64, 72, 73, 65, 66, 67, 68, 69, 70, 71, // Surface Tension: c2a5 c2a5w c2a5x c2a5a..c2a5g
    74, 75, 76, // Forget About Freeman!: c3a1..c3a1b
    82, 77, 78, 79, 80, 83, 81, // Lambda Core: c3a2e c3a2 c3a2a c3a2b c3a2c c3a2f c3a2d
    84, // Xen: c4a1
    91, 92, 93, // Gonarch's Lair: c4a2..c4a2b
    85, 86, 87, 88, 89, 90, // Interloper: c4a1a..c4a1f
    94, // Nihilanth: c4a3
    95, // Endgame: c5a1
    96, 97, 98, 99, 100, 101, 102, // Hazard Course: t0a0..t0a0d
];
/// Index into `PLAY_ORDER` of each chapter's first map.
const CHAPTER_START: [u8; 19] = [
    0, 6, 11, 18, 23, 28, 37, 40, 50, 56, 60, 64, 74, 77, 84, 85, 88, 94, 95,
];
/// Room each chapter starts in.
pub const CHAPTER_ROOM: [i16; 19] = {
    let mut rooms = [0i16; 19];
    let mut i = 0;
    while i < rooms.len() {
        rooms[i] = PLAY_ORDER[CHAPTER_START[i] as usize] as i16;
        i += 1;
    }
    rooms
};
/// Row index of the Hazard Course in the chapter list: its maps sit after the
/// campaign, so it behaves as one more chapter for sub-map selection.
pub const HAZARD_CHAPTER: usize = CHAPTERS.len();

/// The `PLAY_ORDER` range a chapter row covers, `[start, end)`; use
/// [`chapter_map`] to turn a row inside it into a room id.
pub fn chapter_rooms(chapter: usize) -> (usize, usize) {
    if chapter >= HAZARD_CHAPTER {
        return (CAMPAIGN_MAP_COUNT, PLAY_ORDER.len());
    }
    let start = CHAPTER_START[chapter] as usize;
    let end = match CHAPTER_START.get(chapter + 1) {
        Some(&next) => next as usize,
        None => CAMPAIGN_MAP_COUNT,
    };
    (start, end)
}

/// Room id of row `row` in a chapter's sub-map list.
#[cfg(not(feature = "semantic-input"))]
pub fn chapter_map(chapter: usize, row: usize) -> usize {
    let (start, end) = chapter_rooms(chapter);
    PLAY_ORDER[(start + row).min(end - 1)] as usize
}

/// Position of a room in `PLAY_ORDER`: how far into the game a player is when
/// they first reach it.
pub const fn play_rank(room: usize) -> usize {
    let mut i = 0;
    while i < PLAY_ORDER.len() {
        if PLAY_ORDER[i] as usize == room {
            return i;
        }
        i += 1;
    }
    PLAY_ORDER.len()
}

/// Diagnostic launches after the authored suit pickup inherit the suit.
/// Room IDs address chunks, so they must not define campaign progression.
pub const fn diagnostic_has_hev(room: usize, suit_room: usize) -> bool {
    room < CAMPAIGN_MAP_COUNT && play_rank(room) > play_rank(suit_room)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_equipment_follows_actual_anomalous_materials_order() {
        assert_eq!(&PLAY_ORDER[6..11], &[6, 10, 7, 8, 11]);
        assert_eq!(MAPS[10], "c1a0d");
        assert_eq!(MAPS[7], "c1a0a");
        assert_eq!(MAPS[8], "c1a0b");
        assert!(!diagnostic_has_hev(6, 10));
        assert!(!diagnostic_has_hev(10, 10));
        for room in [7, 8, 9, 11, 94] {
            assert!(diagnostic_has_hev(room, 10), "room {room}");
        }
    }

    #[test]
    fn training_and_invalid_rooms_do_not_receive_diagnostic_equipment() {
        for room in TRAINING_START_ROOM..MAPS.len() {
            assert!(!diagnostic_has_hev(room, 10));
        }
        assert!(!diagnostic_has_hev(MAPS.len(), 10));
        assert!(!diagnostic_has_hev(usize::MAX, 10));
    }
}

/// The chapter whose first map is `room`, if any.
pub fn chapter_starting_in(room: usize) -> Option<usize> {
    CHAPTER_ROOM.iter().position(|&r| r as usize == room)
}

const _: () = {
    // PLAY_ORDER is a permutation of every room, and each chapter start and
    // the Hazard Course begin where CHAPTER_START and TRAINING_START_ROOM say.
    let mut seen = [false; 103];
    let mut i = 0;
    while i < PLAY_ORDER.len() {
        assert!(!seen[PLAY_ORDER[i] as usize]);
        seen[PLAY_ORDER[i] as usize] = true;
        i += 1;
    }
    assert!(PLAY_ORDER[CAMPAIGN_MAP_COUNT] as usize == TRAINING_START_ROOM);
    assert!(CHAPTER_ROOM[2] == 9); // Unforeseen Consequences: c1a0c
    assert!(CHAPTER_ROOM[13] == 82); // Lambda Core: c3a2e
};
