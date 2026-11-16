//! The overlay capture layer (`psx_goldsrc::fx`) bound to this game's pending overlay tail.
//!
//! The layer calls back through [`OverlayTail`] when the packet arena runs out mid-pass, so the
//! frame's already-built overlay lists go to the GPU before the immediate fallback draws.

use psx_goldsrc::fx::{self, Tail};
use psx_gpu::material::{BlendMode, TextureMaterial};

/// This game's [`Tail`]: the frame's deferred overlay lists.
pub struct OverlayTail;

impl Tail for OverlayTail {
    #[inline(always)]
    unsafe fn issue_built() {
        crate::issue_deferred_overlays();
    }
}

/// A flat triangle through a translucent blend mode, captured when armed.
pub fn fx_tri_flat_blended(verts: [(i16, i16); 3], r: u8, g: u8, b: u8, blend_mode: BlendMode) {
    fx::tri_flat_blended::<OverlayTail>(verts, r, g, b, blend_mode);
}

/// A flat opaque triangle, captured when armed.
#[allow(dead_code)]
pub fn fx_tri_flat(verts: [(i16, i16); 3], r: u8, g: u8, b: u8) {
    fx::tri_flat::<OverlayTail>(verts, r, g, b);
}

/// A flat opaque quad, captured when armed.
pub fn fx_quad_flat(verts: [(i16, i16); 4], r: u8, g: u8, b: u8) {
    fx::quad_flat::<OverlayTail>(verts, r, g, b);
}

/// A textured quad, captured when armed.
pub fn fx_quad_textured_material(
    verts: [(i16, i16); 4],
    uvs: [(u8, u8); 4],
    material: TextureMaterial,
) {
    fx::quad_textured_material::<OverlayTail>(verts, uvs, material);
}
