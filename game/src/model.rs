//! Shared HMD8 parsing with this port's generated projection-scratch budget.

pub use psx_asset::hmd8::*;

/// Reject a model before its vertices can exceed the game-owned scratch arena.
pub fn load(data: &'static [u8]) -> Model {
    Model::load_with_vertex_cap(data, crate::room_budget::MAX_MODEL_VERTS)
}
