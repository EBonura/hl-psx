// SPDX-License-Identifier: GPL-2.0-or-later
//! Host runner for the pure-logic modules of the PlayStation crate.
//!
//! `game/` is `no_std` + `no_main` and only links for `mipsel-sony-psx`, so its
//! `#[cfg(test)] mod tests` blocks could never run: several modules carried
//! tests that nothing executed. This crate includes those modules by path and
//! runs them on the host, which is the only place a `cargo test` can happen.
//!
//! Only modules whose logic is arithmetic and layout -- no GPU, GTE, SPU or CD
//! -- can be included. Anything hardware-facing must be `cfg`-gated inside the
//! module itself, as `save` gates its card transport on `target_arch = "mips"`.
//!
//! Run with `cargo test --manifest-path host/hl-logic-tests/Cargo.toml`.

/// Mirrors of the arsenal dimensions the included modules expect from the game
/// crate root. `constants_match_the_game_crate` fails if these ever drift.
pub const N_WEAPONS: usize = 14;
pub const N_AMMO: usize = 13;

#[path = "../../../game/src/save.rs"]
pub mod save;

#[path = "../../../game/src/pickup_logic.rs"]
pub mod pickup_logic;

#[path = "../../../game/src/ground_logic.rs"]
pub mod ground_logic;

#[path = "../../../game/src/logic_state.rs"]
pub mod logic_state;

#[path = "../../../game/src/scientist_logic.rs"]
pub mod scientist_logic;

#[path = "../../../game/src/semantic_input.rs"]
pub mod semantic_input;

#[path = "../../../game/src/route_follow.rs"]
pub mod route_follow;

#[path = "../../../game/src/tram_logic.rs"]
pub mod tram_logic;

#[path = "../../../game/src/tram_follower.rs"]
pub mod tram_follower;

#[path = "../../../game/src/pushable.rs"]
pub mod pushable;

#[path = "../../../game/src/visibility_logic.rs"]
pub mod visibility_logic;

#[path = "../../../game/src/hitbox_logic.rs"]
pub mod hitbox_logic;

#[path = "../../../game/src/ladder_logic.rs"]
pub mod ladder_logic;

#[path = "../../../game/src/ordering.rs"]
pub mod ordering;

#[path = "../../../game/src/render.rs"]
pub mod render;

#[cfg(test)]
mod guard {
    /// The included modules resolve `crate::N_WEAPONS` / `crate::N_AMMO` to this
    /// crate's copies. If the game crate changes either, the save layout changes
    /// with it and these tests would silently be exercising a different format,
    /// so read the real values out of main.rs and compare.
    #[test]
    fn constants_match_the_game_crate() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../game/src/main.rs"
        ))
        .expect("game/src/main.rs is readable from the host runner");
        for (name, ours) in [("N_WEAPONS", super::N_WEAPONS), ("N_AMMO", super::N_AMMO)] {
            let needle = format!("const {name}: usize = ");
            let at = src
                .find(&needle)
                .unwrap_or_else(|| panic!("{name} not found in game/src/main.rs"));
            let rest = &src[at + needle.len()..];
            let end = rest.find(';').expect("constant is terminated");
            let theirs: usize = rest[..end]
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("{name} is not a plain integer literal"));
            assert_eq!(
                theirs, ours,
                "{name} drifted: game/src/main.rs says {theirs}, this runner mirrors {ours}"
            );
        }
    }
}

#[path = "../../../game/src/beverage.rs"]
pub mod beverage;
