//! Boot map selector styled after Half-Life's original (WON) main menu, built
//! from the install's own assets via `tools/extract_menu.py` (git-ignored
//! data/menu/, like every other extracted asset):
//!   - hlfont.bin  -- the menu font (Arial 16, per valve/640_textscheme.txt),
//!                    built into a `psx_font::BitmapFont` at boot.
//!   - logo.tex    -- the real HALF-LIFE wordmark (resource/logo.tga), 4bpp w/
//!                    a transparent background.
//!   - bg.tex      -- the console background (gfx/conback.lmp), desaturated +
//!                    darkened; the only full-screen art a Steam copy ships
//!                    (the WON splash.bmp isn't present).
//! Colors are the scheme's: items orange 255 170 0, selected white with a faint
//! orange armed bar, first letter underlined. Returns the chosen chunk id.

use psx_font::{BitOrder, BitmapFont, FontAtlas};
use psx_gpu::material::TextureMaterial;
use psx_gpu::{self as gpu, framebuf::FrameBuffer};
use psx_pad::{button, poll_port1};
use psx_vram::{upload_bytes, Clut, TexDepth, Tpage, VramRect};

const FONT_TPAGE: Tpage = Tpage::new(320, 0, TexDepth::Bit4);
const FONT_CLUT: Clut = Clut::new(320, 256);

// Exact colors from valve/640_textscheme.txt "Primary Button Text".
const WHITE: (u8, u8, u8) = (238, 238, 230);
const ITEM: (u8, u8, u8) = (255, 170, 0); // FgColor
const ITEM_SEL: (u8, u8, u8) = (255, 255, 255); // FgColorArmed (selected = white)
const ARMED: (u8, u8, u8) = (75, 53, 10); // BgColorArmed 255 170 0 @67 over the dark bg
const DESC: (u8, u8, u8) = (175, 175, 168); // grey description
const DIM: (u8, u8, u8) = (110, 110, 104);

pub const MAPS: [&str; 4] = ["c0a0", "c1a0", "c1a1a", "c1a3a"];
const CHAPTERS: [&str; 4] = [
    "Black Mesa Inbound",
    "Anomalous Materials",
    "Unforeseen Consequences",
    "We've Got Hostiles",
];
const DETAILS: [&str; 4] = [
    "Ride the tram in",
    "Experiment fails",
    "Cascade hits",
    "Military arrives",
];

// Assets extracted from the user's install (git-ignored). Each .tex blob is
// `u16 w | u16 h | u16 clut[16] | u8 pix4...`.
static HLFONT_BLOB: &[u8] = include_bytes!("../../data/menu/hlfont.bin");
static LOGO_BLOB: &[u8] = include_bytes!("../../data/menu/logo.tex");
static BG_BLOB: &[u8] = include_bytes!("../../data/menu/bg.tex");
const LOGO_VRAM_X: u16 = 640; // clear of framebuffers (X<320) and the font atlas (X320..)
const BG_VRAM_X: u16 = 704;

static mut HL_FONT: BitmapFont = BitmapFont {
    glyph_w: 16,
    glyph_h: 19,
    first_char: 32,
    glyph_count: 95,
    bitmap: &[],
    glyph_advances: None,
    advance_x: 16,
    line_height: 21,
    bit_order: BitOrder::Msb,
};

unsafe fn hl_font() -> &'static BitmapFont {
    let b = HLFONT_BLOB;
    let count = u16::from_le_bytes([b[2], b[3]]) as usize;
    HL_FONT = BitmapFont {
        glyph_w: b[0],
        glyph_h: b[1],
        first_char: u16::from_le_bytes([b[4], b[5]]),
        glyph_count: count as u16,
        bitmap: &b[8 + count..],
        glyph_advances: Some(&b[8..8 + count]),
        advance_x: b[0],
        line_height: b[1] + 2,
        bit_order: BitOrder::Msb,
    };
    &*core::ptr::addr_of!(HL_FONT)
}

/// Upload a `.tex` blob to a free tpage at VRAM X = `vx`; return its material + size.
fn upload_tex(blob: &[u8], vx: u16) -> (TextureMaterial, u16, u16) {
    let w = u16::from_le_bytes([blob[0], blob[1]]);
    let h = u16::from_le_bytes([blob[2], blob[3]]);
    upload_bytes(VramRect::new(vx, 0, w / 4, h), &blob[36..]); // 4bpp pixels
    upload_bytes(VramRect::new(vx, 256, 16, 1), &blob[4..36]); // CLUT
    let tp = Tpage::new(vx, 0, TexDepth::Bit4);
    let cl = Clut::new(vx, 256);
    (
        TextureMaterial::new(cl.uv_clut_word(), tp.uv_tpage_word(0)),
        w,
        h,
    )
}

/// The conback texture stretched to fill the 320x240 screen.
fn draw_bg(mat: TextureMaterial) {
    gpu::draw_quad_textured_material(
        [(0, 0), (320, 0), (0, 240), (320, 240)],
        [(0, 0), (255, 0), (0, 239), (255, 239)],
        mat,
    );
}

/// The textured HALF-LIFE wordmark, centered across the top.
fn draw_logo(mat: TextureMaterial, w: u16, h: u16) {
    let sw = 292i16;
    let sh = h as i16 * sw / w as i16;
    let x0 = 160 - sw / 2;
    let y0 = 14i16;
    let (uw, uh) = ((w - 1) as u8, (h - 1) as u8);
    gpu::draw_quad_textured_material(
        [(x0, y0), (x0 + sw, y0), (x0, y0 + sh), (x0 + sw, y0 + sh)],
        [(0, 0), (uw, 0), (0, uh), (uw, uh)],
        mat,
    );
}

/// Run the selector until the player confirms a choice; returns its chunk id.
pub fn run(fb: &mut FrameBuffer) -> usize {
    let font = FontAtlas::upload(unsafe { hl_font() }, FONT_TPAGE, FONT_CLUT);
    let (logo, lw, lh) = upload_tex(LOGO_BLOB, LOGO_VRAM_X);
    let (bg, _, _) = upload_tex(BG_BLOB, BG_VRAM_X);
    let n = MAPS.len() as i32;
    let mut sel = 0i32;
    let (mut p_up, mut p_dn, mut p_ok) = (true, true, true);
    let dz = 48i16;

    loop {
        let pad = poll_port1();
        let b = pad.buttons;
        let mut up = b.is_held(button::UP);
        let mut dn = b.is_held(button::DOWN);
        if pad.is_analog() {
            let (_, ly) = pad.sticks.left_centered();
            if ly < -dz {
                up = true;
            }
            if ly > dz {
                dn = true;
            }
        }
        let ok = b.is_held(button::CROSS) || b.is_held(button::START);
        if up && !p_up {
            sel = (sel - 1 + n) % n;
        }
        if dn && !p_dn {
            sel = (sel + 1) % n;
        }
        if ok && !p_ok {
            draw_bg(bg);
            draw_logo(logo, lw, lh);
            font.draw_text(40, 150, "Loading", ITEM_SEL);
            font.draw_text(124, 150, CHAPTERS[sel as usize], WHITE);
            gpu::vsync();
            fb.swap();
            return sel as usize;
        }
        p_up = up;
        p_dn = dn;
        p_ok = ok;

        draw_bg(bg);
        draw_logo(logo, lw, lh);

        let mut y = 94i16;
        for i in 0..MAPS.len() {
            let here = i as i32 == sel;
            let ic = if here { ITEM_SEL } else { ITEM };
            if here {
                gpu::draw_quad_flat(
                    [(10, y - 3), (310, y - 3), (10, y + 18), (310, y + 18)],
                    ARMED.0,
                    ARMED.1,
                    ARMED.2,
                );
            }
            font.draw_text(14, y, CHAPTERS[i], ic);
            let fw = font.text_width(&CHAPTERS[i][..1]) as i16; // underline first letter
            gpu::draw_quad_flat(
                [
                    (14, y + 16),
                    (14 + fw, y + 16),
                    (14, y + 17),
                    (14 + fw, y + 17),
                ],
                ic.0,
                ic.1,
                ic.2,
            );
            font.draw_text(208, y, DETAILS[i], DESC);
            y += 26;
        }
        font.draw_text(36, 224, "Cross  Play", DIM);
        let ver = "hl-psx  v0.1";
        font.draw_text(300 - font.text_width(ver) as i16, 224, ver, DIM);

        gpu::vsync();
        fb.swap();
    }
}
