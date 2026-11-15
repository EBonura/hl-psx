//! Shared HMD8 parsing with this port's generated projection-scratch budget.

pub use psx_goldsrc::model::*;

/// Reject a model before its vertices can exceed the game-owned scratch arena.
pub fn load(data: &'static [u8]) -> Model {
    psx_goldsrc::model::load(data, crate::room_budget::MAX_MODEL_VERTS)
}
