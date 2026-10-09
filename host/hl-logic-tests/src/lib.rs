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
//!
//! Modules shared with the other GoldSrc port (pickup, ladder, pushable,
//! visibility, hitbox, ordering, semantic input and the core ground rules)
//! live in `psx-goldsrc` and are tested there.

/// The actor roster and arsenal tables, included from the game itself so the
/// modules below see the real arsenal dimensions and the parity audit reads the
/// real numbers.
#[path = "../../../game/src/combat_defs.rs"]
pub mod combat_defs;
pub use combat_defs::{N_AMMO, N_WEAPONS};

#[path = "../../../game/src/sfx.rs"]
pub mod sfx;

#[path = "../../../game/src/save.rs"]
pub mod save;

#[path = "../../../game/src/ground_logic.rs"]
pub mod ground_logic;

#[path = "../../../game/src/logic_state.rs"]
pub mod logic_state;

#[path = "../../../game/src/scientist_logic.rs"]
pub mod scientist_logic;

#[path = "../../../game/src/tram_logic.rs"]
pub mod tram_logic;

#[path = "../../../game/src/tram_follower.rs"]
pub mod tram_follower;

#[path = "../../../game/src/render.rs"]
pub mod render;

#[path = "../../../game/src/beverage.rs"]
pub mod beverage;

#[path = "../../../game/src/setpiece_logic.rs"]
pub mod setpiece_logic;
