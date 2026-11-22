//! Studio animation events the game acts on.
//!
//! A monster's attack lands on its sequence's events (`HandleAnimEvent`), not
//! on a timer of its own. The cooker (`hl-content anim-events`) writes every
//! event of every cooked clip as `type|slot|tick|event`, the tick being the
//! 20 Hz tick from the sequence's start of the 0.1 s think that handles it;
//! the game's build script keeps the clips named by [`RULES`] in one table
//! (`skill_table::ANIM_EVENTS`) that every species reads the same way.

/// The zombie's `attack1` (roster slot 2): ZOMBIE_AE_ATTACK_RIGHT (1) and
/// ZOMBIE_AE_ATTACK_LEFT (2).
pub const ZOMBIE_ATTACK: u8 = 2;
/// The vortigaunt's `attack1` (roster slot 5): ISLAVE_AE_CLAW (1), three times.
pub const SLAVE_CLAW: u8 = 5;

/// The alien grunt's `hornetattack` (roster slot 2): AGRUNT_AE_HORNET1 to 5.
pub const AGRUNT_HORNETS: u8 = 2;

/// (actor type, roster clip slot) pairs whose events are kept.
pub const RULES: &[(u8, u8)] = &[
    (5, ZOMBIE_ATTACK),
    (55, ZOMBIE_ATTACK),
    (9, SLAVE_CLAW),
    (10, AGRUNT_HORNETS),
];
