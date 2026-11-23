//! The studio splash that opens the disc, before the startup movie: the SDK's
//! animated one (`psx_splash`), the same picture and timing every PSoXide game
//! opens with, over "Built with PSoXide".
//!
//! A pad that is not a DualShock in analog mode gets a two-line notice after
//! it, here and not on a second splash after the movie. The splash itself has
//! no room for extra lines, so the notice is a short screen of its own.

use psx_display::Brightness;
use psx_font::fonts::BASIC_8X16;
use psx_font::FontAtlas;
use psx_gpu::display::{DoubleBuffer, Resolution};
use psx_gpu::Gpu;
use psx_pad::button;
use psx_rt::interrupts;
use psx_vram::{Clut, TextureDepth, TexturePage};

use crate::Skip;

const FONT_TPAGE: TexturePage = TexturePage::new(320, 0, TextureDepth::Bit4);
const FONT_CLUT: Clut = Clut::new(320, 256);
const TAG: &str = "Built with PSoXide";
/// The notice screen: frames in all, and fading in and out at each end.
const NOTICE: i32 = 150;
const NOTICE_FADE: i32 = 24;
/// Ignore input briefly so the press that ended the splash does not end the notice.
const SKIP_GRACE: i32 = 8;

/// Play the splash on `gpu`, ending on a black picture. `notice` is two lines shown after it,
/// empty when the pad is fine (the game asks the pad for analog mode too, and used to
/// show this notice on a second splash after the movie).
pub fn play(gpu: &mut Gpu, skip: &mut Skip, notice: [&str; 2]) {
    let mut fb = DoubleBuffer::new(Resolution::R320X240);
    // The intro reads no memory card, so the player's brightness is not known yet.
    psx_splash::show(gpu, &mut fb, TAG, Brightness::DEFAULT, || skip.buttons());
    if !notice[0].is_empty() {
        show_notice(gpu, &mut fb, skip, notice);
    }
    // Leave both buffers black, so the movie starts from black.
    for _ in 0..2 {
        fb.clear(gpu, (0, 0, 0));
        gpu.wait_idle();
        interrupts::wait_vblank();
        fb.swap(gpu);
    }
}

/// The pad notice, faded in, held and faded out; a press past the grace ends it.
fn show_notice(gpu: &mut Gpu, fb: &mut DoubleBuffer, skip: &mut Skip, notice: [&str; 2]) {
    let font = FontAtlas::upload(&BASIC_8X16, FONT_TPAGE, FONT_CLUT);
    let mut frame = 0i32;
    while frame < NOTICE {
        if frame > SKIP_GRACE
            && skip
                .buttons()
                .is_held(button::CROSS | button::CIRCLE | button::START)
        {
            break;
        }
        let level = (frame.min(NOTICE - frame) * 128 / NOTICE_FADE).clamp(0, 128);
        fb.clear(gpu, (0, 0, 0));
        let mut y = 104;
        for line in notice {
            let x = 160 - font.text_width(line) as i16 / 2;
            let tint = level as u8 / 2;
            font.draw_text(x, y, line, (tint + tint / 2, tint, tint / 2));
            y += 18;
        }
        gpu.wait_idle();
        interrupts::wait_vblank();
        fb.swap(gpu);
        frame += 1;
    }
}
