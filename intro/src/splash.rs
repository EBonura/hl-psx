//! The studio splash that opens the disc: the Bonnie Studios mark and "Built
//! with PSoXide", faded in, held and faded out, before the startup movie.
//!
//! It is the same splash the game used to play once it had loaded (the game's
//! `menu::intro`, which an intro disc now skips): the mark's pixels are the
//! Rust-embedded ones the content compiler also uses (shared, not copied), and
//! the text sweeps a highlight across it as it did there. A button held for more
//! than a few frames walks out of it. A pad that is not a DualShock in analog mode
//! gets a two-line notice under the tag, here and not on a second splash after the
//! movie.

use psx_font::fonts::BASIC_8X16;
use psx_font::FontAtlas;
use psx_gpu::display::{DoubleBuffer, Resolution};
use psx_gpu::material::TextureMaterial;
use psx_gpu::prim::QuadTexturedMaterial;
use psx_gpu::Gpu;
use psx_pad::button;
use psx_rt::interrupts;
use psx_vram::{upload_bytes, Clut, TextureDepth, TexturePage, VramRect};

use crate::Skip;

#[path = "../../host/hl-content/src/bonnie_logo.rs"]
#[allow(dead_code)]
mod bonnie_logo;

/// The 16-colour palette the mark was drawn in (hl-content's `PICO8_CLUT`).
const PALETTE: [u16; 16] = [
    0x0421, 0x28A3, 0x288F, 0x2A00, 0x1955, 0x254B, 0x6318, 0x77DF, 0x241F, 0x029F, 0x13BF, 0x1B80,
    0x7EA5, 0x4DD0, 0x55DF, 0x573F,
];
const BONNIE_SIZE: u16 = 128;
/// The mark's page in VRAM, clear of the font atlas and the framebuffers.
const BONNIE_VRAM_X: u16 = 768;
const FONT_TPAGE: TexturePage = TexturePage::new(320, 0, TextureDepth::Bit4);
const FONT_CLUT: Clut = Clut::new(320, 256);

const FADE_IN: i32 = 32;
const HOLD: i32 = 74;
const TOTAL: i32 = 150;
const FADE_OUT: i32 = TOTAL - FADE_IN - HOLD;
const TAG: &str = "Built with PSoXide";
/// Ignore input briefly so a button still held from boot does not consume the
/// splash.
const SKIP_GRACE: i32 = 8;

fn bytes_of(words: &'static [u16]) -> &'static [u8] {
    // SAFETY: a `u16` slice viewed as its own bytes (little-endian target, as the
    // upload expects), no alignment requirement shrinks.
    unsafe { core::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 2) }
}

/// Play the splash on `gpu`, ending on a black picture. `notice` is two lines under the tag,
/// empty when the pad is fine (the game asks the pad for analog mode too, and used to
/// show this notice on a second splash after the movie).
pub fn play(gpu: &mut Gpu, skip: &mut Skip, notice: [&str; 2]) {
    let mut fb = DoubleBuffer::new(Resolution::R320X240);
    // 4bpp pixels, then the palette in the row below, as the game uploads it.
    upload_bytes(
        VramRect::new(BONNIE_VRAM_X, 0, BONNIE_SIZE / 4, BONNIE_SIZE),
        bytes_of(&bonnie_logo::COVER_BONNIE),
    );
    upload_bytes(VramRect::new(BONNIE_VRAM_X, 256, 16, 1), bytes_of(&PALETTE));
    let mark = TextureMaterial::new(
        Clut::new(BONNIE_VRAM_X, 256).uv_word(),
        TexturePage::new(BONNIE_VRAM_X, 0, TextureDepth::Bit4).uv_word(0),
    );
    let font = FontAtlas::upload(&BASIC_8X16, FONT_TPAGE, FONT_CLUT);

    let mut frame = 0i32;
    while frame < TOTAL {
        if frame > SKIP_GRACE
            && skip
                .buttons()
                .is_held(button::CROSS | button::CIRCLE | button::START)
        {
            break;
        }
        let level = if frame < FADE_IN {
            frame * 128 / FADE_IN
        } else if frame < FADE_IN + HOLD {
            128
        } else {
            (TOTAL - frame) * 128 / FADE_OUT
        }
        .clamp(0, 128);

        fb.clear(gpu, (0, 0, 0));
        let tint = level as u8;
        gpu.draw(&QuadTexturedMaterial::with_material(
            [(112, 34), (208, 34), (112, 130), (208, 130)],
            [(0, 0), (127, 0), (0, 127), (127, 127)],
            mark.with_tint((tint, tint, tint)),
        ));
        draw_sheen(
            &font,
            160 - font.text_width(TAG) as i16 / 2,
            150,
            frame,
            level,
        );
        let mut y = 190;
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
    // Leave both buffers black, so the movie starts from black.
    for _ in 0..2 {
        fb.clear(gpu, (0, 0, 0));
        gpu.wait_idle();
        interrupts::wait_vblank();
        fb.swap(gpu);
    }
}

/// The tag, with a highlight sweeping across its letters.
fn draw_sheen(font: &FontAtlas, mut x: i16, y: i16, frame: i32, brightness: i32) {
    let span = TAG.chars().count() as i32 + 18;
    let head = (frame / 2).rem_euclid(span);
    for (index, ch) in TAG.char_indices() {
        let glyph = &TAG[index..index + ch.len_utf8()];
        let amount = (18 - (index as i32 - head).abs() * 6).max(0);
        let channel = |base: i32| {
            let dim = base * brightness / 128;
            (dim + (brightness - dim) * amount / 18) as u8
        };
        font.draw_text(x, y, glyph, (channel(76), channel(108), channel(128)));
        x += font.text_width(glyph) as i16;
    }
}
