//! Scaled text: `psx_goldsrc::hltext` drawing through the overlay capture layer.

use crate::overlay::OverlayTail;
use psx_goldsrc::hltext as shared;

pub use shared::{
    line_height_scaled, text_width_gameplay_scaled, text_width_scaled, upload_gameplay_atlas,
    SMALL_Q8,
};

/// `psx_font::TextSink` over the in-game atlas.
pub type Sink = shared::Sink<OverlayTail>;

/// The in-game HUD/menu size.
pub const SMALL_SINK: Sink = Sink::new(SMALL_Q8);

#[inline(always)]
pub fn draw_text_gameplay_scaled(x: i16, y: i16, text: &str, scale_q8: u16, color: (u8, u8, u8)) {
    shared::draw_text_gameplay_scaled::<OverlayTail>(x, y, text, scale_q8, color);
}

#[inline(always)]
pub fn draw_text_scaled(x: i16, y: i16, text: &str, scale_q8: u16, color: (u8, u8, u8)) {
    shared::draw_text_scaled::<OverlayTail>(x, y, text, scale_q8, color);
}

#[inline(always)]
pub fn draw_centered_scaled(y: i16, text: &str, scale_q8: u16, color: (u8, u8, u8)) {
    shared::draw_centered_scaled::<OverlayTail>(y, text, scale_q8, color);
}
