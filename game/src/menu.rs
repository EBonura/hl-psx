//! Boot menu styled after Half-Life's original (WON) main menu, built
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
//! orange armed bar. Returns the chosen streamed room id.

use crate::hltext;
use psx_font::{BitOrder, BitmapFont, FontAtlas};
use psx_gpu::material::TextureMaterial;
use psx_gpu::{self as gpu, framebuf::FrameBuffer};
use psx_pad::{button, poll_port1};
use psx_rt::interrupts;
use psx_vram::{upload_bytes, Clut, TexDepth, Tpage, VramRect};

const FONT_TPAGE: Tpage = Tpage::new(320, 0, TexDepth::Bit4);
const FONT_CLUT: Clut = Clut::new(320, 256);

// Exact colors from valve/640_textscheme.txt "Primary Button Text".
const WHITE: (u8, u8, u8) = (238, 238, 230);
const ITEM: (u8, u8, u8) = (255, 170, 0); // FgColor
const ITEM_SEL: (u8, u8, u8) = (255, 255, 255); // FgColorArmed (selected = white)
const ARMED: (u8, u8, u8) = (75, 53, 10); // BgColorArmed 255 170 0 @67 over the dark bg
const DIM: (u8, u8, u8) = (110, 110, 104);

// Keep the runnable map order in sync with Makefile's MAPLIST.
pub const MAPS: [&str; 96] = [
    "c0a0", "c0a0a", "c0a0b", "c0a0c", "c0a0d", "c0a0e", "c1a0", "c1a0a", "c1a0b", "c1a0c",
    "c1a0d", "c1a0e", "c1a1", "c1a1a", "c1a1b", "c1a1c", "c1a1d", "c1a1f", "c1a2", "c1a2a",
    "c1a2b", "c1a2c", "c1a2d", "c1a3", "c1a3a", "c1a3b", "c1a3c", "c1a3d", "c1a4", "c1a4b",
    "c1a4d", "c1a4e", "c1a4f", "c1a4g", "c1a4i", "c1a4j", "c1a4k", "c2a1", "c2a1a", "c2a1b",
    "c2a2", "c2a2a", "c2a2b1", "c2a2b2", "c2a2c", "c2a2d", "c2a2e", "c2a2f", "c2a2g", "c2a2h",
    "c2a3", "c2a3a", "c2a3b", "c2a3c", "c2a3d", "c2a3e", "c2a4", "c2a4a", "c2a4b", "c2a4c",
    "c2a4d", "c2a4e", "c2a4f", "c2a4g", "c2a5", "c2a5a", "c2a5b", "c2a5c", "c2a5d", "c2a5e",
    "c2a5f", "c2a5g", "c2a5w", "c2a5x", "c3a1", "c3a1a", "c3a1b", "c3a2", "c3a2a", "c3a2b",
    "c3a2c", "c3a2d", "c3a2e", "c3a2f", "c4a1", "c4a1a", "c4a1b", "c4a1c", "c4a1d", "c4a1e",
    "c4a1f", "c4a2", "c4a2a", "c4a2b", "c4a3", "c5a1",
];
const CHAPTERS: [&str; 19] = [
    "Black Mesa Inbound",
    "Anomalous Materials",
    "Unforeseen Consequences",
    "Office Complex",
    "We've Got Hostiles",
    "Blast Pit",
    "Power Up",
    "On A Rail",
    "Apprehension",
    "Residue Processing",
    "Questionable Ethics",
    "Surface Tension",
    "Forget About Freeman!",
    "Lambda Core",
    "Xen",
    "Interloper",
    "Gonarch's Lair",
    "Nihilanth",
    "Endgame",
];
const CHAPTER_ROOM: [i16; 19] = [
    0, 6, 12, 18, 23, 28, 37, 40, 50, 56, 60, 64, 74, 77, 84, 85, 91, 94, 95,
];
const VISIBLE_ROWS: i32 = 7;
const LIST_X: i16 = 20;
const LIST_Y: i16 = 84;
const ROW_H: i16 = 20;
const MAIN_X: i16 = 58;
const MAIN_Y: i16 = 88;
const FOOTER_Y: i16 = 224;

const MAIN_ITEMS: [&str; 4] = ["New Game", "Chapter Select", "Options", "Credits"];
const CREDIT_LINES: [(&str, (u8, u8, u8)); 8] = [
    ("Half-Life   Valve 1998", WHITE),
    ("PS1 Port   hl-psx", ITEM),
    ("Built with PSoXide GPL-2.0", (96, 170, 170)),
    ("PSoXide engine/runtime", (80, 130, 210)),
    ("Public Half-Life SDK source", DIM),
    ("Assets from your HL install", DIM),
    ("Unofficial fan port", DIM),
    ("All rights to original creators", DIM),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuScreen {
    Main,
    Chapters,
    Options,
    Credits,
}

#[derive(Clone, Copy)]
struct MenuEdges {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    ok: bool,
    back: bool,
}

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

fn draw_footer(font: &FontAtlas) {
    let ver = "hl-psx  v0.1";
    font.draw_text(300 - font.text_width(ver) as i16, FOOTER_Y, ver, DIM);
}

fn draw_centered(font: &FontAtlas, y: i16, text: &str, color: (u8, u8, u8)) {
    let x = 160 - (font.text_width(text) as i16 / 2);
    font.draw_text(x, y, text, color);
}

fn draw_select_bar(x: i16, y: i16, w: i16) {
    gpu::draw_quad_flat(
        [(x, y - 1), (x + w, y - 1), (x, y + 19), (x + w, y + 19)],
        ARMED.0,
        ARMED.1,
        ARMED.2,
    );
}

fn draw_simple_list(font: &FontAtlas, items: &[&str], sel: usize, x: i16, y0: i16, bar_w: i16) {
    let mut y = y0;
    let mut i = 0usize;
    while i < items.len() {
        if i == sel {
            draw_select_bar(x - 6, y, bar_w);
            font.draw_text(x, y, items[i], ITEM_SEL);
        } else {
            font.draw_text(x, y, items[i], ITEM);
        }
        y += ROW_H;
        i += 1;
    }
}

fn draw_loading(
    fb: &mut FrameBuffer,
    font: &FontAtlas,
    bg: TextureMaterial,
    logo: TextureMaterial,
    lw: u16,
    lh: u16,
    label: &str,
) {
    const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
    let _ = (logo, lw, lh); // loading stays unbranded (wordmark only on the menu)
    let mut frame = 0usize;
    while frame < 8 {
        fb.clear(0, 0, 0);
        draw_bg(bg);
        draw_centered(font, 146, "Loading", ITEM_SEL);
        let x = 160 + font.text_width("Loading") as i16 / 2 + 8;
        font.draw_text(x, 146, SPINNER[frame & 3], WHITE);
        draw_centered(font, 170, label, WHITE);
        gpu::draw_sync();
        interrupts::wait_vblank();
        fb.swap();
        frame += 1;
    }
}

fn draw_shell(
    fb: &mut FrameBuffer,
    font: &FontAtlas,
    bg: TextureMaterial,
    logo: TextureMaterial,
    lw: u16,
    lh: u16,
    title: &str,
) {
    fb.clear(0, 0, 0);
    draw_bg(bg);
    draw_logo(logo, lw, lh);
    if !title.is_empty() {
        draw_centered(font, 58, title, DIM);
    }
}

fn wrap_move(sel: &mut i32, count: i32, input: MenuEdges) {
    if input.up {
        *sel = (*sel - 1 + count) % count;
    }
    if input.down {
        *sel = (*sel + 1) % count;
    }
}

fn poll_menu_edges(
    p_up: &mut bool,
    p_dn: &mut bool,
    p_lf: &mut bool,
    p_rt: &mut bool,
    p_ok: &mut bool,
    p_back: &mut bool,
) -> MenuEdges {
    let pad = poll_port1();
    let b = pad.buttons;
    let mut up = b.is_held(button::UP);
    let mut dn = b.is_held(button::DOWN);
    let mut lf = b.is_held(button::LEFT);
    let mut rt = b.is_held(button::RIGHT);
    if pad.is_analog() {
        let (lx, ly) = pad.sticks.left_centered();
        let dz = 48i16;
        if ly < -dz {
            up = true;
        }
        if ly > dz {
            dn = true;
        }
        if lx < -dz {
            lf = true;
        }
        if lx > dz {
            rt = true;
        }
    }
    let ok = b.is_held(button::CROSS) || b.is_held(button::START);
    let back = b.is_held(button::CIRCLE) || b.is_held(button::SELECT);
    let edges = MenuEdges {
        up: up && !*p_up,
        down: dn && !*p_dn,
        left: lf && !*p_lf,
        right: rt && !*p_rt,
        ok: ok && !*p_ok,
        back: back && !*p_back,
    };
    *p_up = up;
    *p_dn = dn;
    *p_lf = lf;
    *p_rt = rt;
    *p_ok = ok;
    *p_back = back;
    edges
}

fn draw_main_menu(font: &FontAtlas, sel: usize) {
    draw_simple_list(font, &MAIN_ITEMS, sel, MAIN_X, MAIN_Y, 220);
}

fn draw_chapter_menu(font: &FontAtlas, sel: i32) {
    let n = CHAPTERS.len() as i32;
    let max_first = (n - VISIBLE_ROWS).max(0);
    let first = (sel - VISIBLE_ROWS / 2).clamp(0, max_first);
    let last = (first + VISIBLE_ROWS).min(n);
    let mut y = LIST_Y;
    for i in first..last {
        let idx = i as usize;
        let here = i == sel;
        let enabled = idx < MAPS.len() && CHAPTER_ROOM[idx] >= 0;
        let ic = if !enabled {
            DIM
        } else if here {
            ITEM_SEL
        } else {
            ITEM
        };
        if here && enabled {
            draw_select_bar(14, y, 292);
        }
        font.draw_text(LIST_X, y, CHAPTERS[idx], ic);
        y += ROW_H;
    }

    if first > 0 {
        gpu::draw_tri_flat([(154, 70), (166, 70), (160, 64)], ITEM.0, ITEM.1, ITEM.2);
    }
    if last < n {
        gpu::draw_tri_flat([(154, 224), (166, 224), (160, 230)], ITEM.0, ITEM.1, ITEM.2);
    }
}

const OPT_LABELS: [&str; 4] = ["Screen X", "Screen Y", "Music", "SFX"];
pub const N_OPTIONS: i32 = 5; // 4 sliders + Back

/// Format a signed int into `buf`, returning the slice as a str.
fn fmt_num(v: i32, buf: &mut [u8; 12]) -> &str {
    let neg = v < 0;
    let mut n = v.unsigned_abs();
    let mut i = buf.len();
    if n == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    if neg {
        i -= 1;
        buf[i] = b'-';
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("")
}

/// 8-segment volume bar; filled segments up to `v` are lit, the rest dim.
fn draw_vol_bar(x: i16, y: i16, v: i32, bright: bool) {
    let lit = if bright { ITEM_SEL } else { ITEM };
    for i in 0..8i16 {
        let sx = x + i * 13;
        let (r, g, b) = if (i as i32) < v { lit } else { (56, 56, 56) };
        gpu::draw_quad_flat(
            [(sx, y + 2), (sx + 10, y + 2), (sx, y + 15), (sx + 10, y + 15)],
            r,
            g,
            b,
        );
    }
}

fn draw_options_menu(font: &FontAtlas, sel: usize) {
    let x_label = 40i16;
    let x_val = 172i16;
    let mut y = 88i16;
    for i in 0..4usize {
        let is_sel = i == sel;
        if is_sel {
            draw_select_bar(x_label - 8, y, 120);
        }
        let col = if is_sel { ITEM_SEL } else { ITEM };
        font.draw_text(x_label, y, OPT_LABELS[i], col);
        if i < 2 {
            let mut buf = [0u8; 12];
            let s = fmt_num(crate::settings::value(i), &mut buf);
            font.draw_text(x_val + 8, y, "<", col);
            font.draw_text(x_val + 44, y, s, col);
            font.draw_text(x_val + 96, y, ">", col);
        } else {
            draw_vol_bar(x_val, y, crate::settings::value(i), is_sel);
        }
        y += 22;
    }
    y += 8;
    let is_sel = sel == 4;
    if is_sel {
        draw_select_bar(x_label - 8, y, 120);
    }
    font.draw_text(x_label, y, "Back", if is_sel { ITEM_SEL } else { ITEM });
    hltext::draw_centered_scaled(212, "Left / Right to adjust", hltext::SMALL_Q8, DIM);
}

fn draw_credits_menu(font: &FontAtlas) {
    let mut y = 76i16;
    let mut i = 0usize;
    while i < CREDIT_LINES.len() {
        hltext::draw_centered_scaled(y, CREDIT_LINES[i].0, hltext::SMALL_Q8, CREDIT_LINES[i].1);
        y += hltext::line_height_scaled(hltext::SMALL_Q8);
        i += 1;
    }
    draw_select_bar(58, 208, 204);
    draw_centered(font, 208, "Back", ITEM_SEL);
}

/// The campaign end card: white-in from the c5a1 fade, the wordmark, THE END,
/// and the credit lines. Waits for a button, then returns (to the main menu).
pub fn ending(fb: &mut FrameBuffer) {
    let font = FontAtlas::upload(unsafe { hl_font() }, FONT_TPAGE, FONT_CLUT);
    let (logo, lw, lh) = upload_tex(LOGO_BLOB, LOGO_VRAM_X);
    let mut prev_any = true; // swallow the button that ended the fade
    let mut frame = 0u32;
    loop {
        fb.clear(0, 0, 0);
        draw_logo(logo, lw, lh);
        draw_centered(&font, 88, "THE END", ITEM_SEL);
        let mut y = 110i16;
        let mut i = 0usize;
        while i < CREDIT_LINES.len() {
            hltext::draw_centered_scaled(y, CREDIT_LINES[i].0, hltext::SMALL_Q8, CREDIT_LINES[i].1);
            y += hltext::line_height_scaled(hltext::SMALL_Q8);
            i += 1;
        }
        if frame > 40 && (frame / 16) & 1 == 0 {
            draw_centered(&font, 214, "Press any button", DIM);
        }
        gpu::draw_sync();
        interrupts::wait_vblank();
        fb.swap();
        let pad = poll_port1();
        let any = pad.buttons.bits() != 0;
        if any && !prev_any && frame > 40 {
            unsafe { crate::sfx::play(crate::sfx::BUTTON) };
            return;
        }
        prev_any = any;
        frame += 1;
    }
}

/// Run the menu until the player confirms a launch; returns its room id.
pub fn run(fb: &mut FrameBuffer) -> usize {
    let font = FontAtlas::upload(unsafe { hl_font() }, FONT_TPAGE, FONT_CLUT);
    let (logo, lw, lh) = upload_tex(LOGO_BLOB, LOGO_VRAM_X);
    let (bg, _, _) = upload_tex(BG_BLOB, BG_VRAM_X);
    let mut screen = MenuScreen::Main;
    let mut main_sel = 0i32;
    let mut chapter_sel = 0i32;
    let mut options_sel = 0i32;
    let (mut p_up, mut p_dn, mut p_ok, mut p_back) = (true, true, true, true);
    let (mut p_lf, mut p_rt) = (true, true);

    loop {
        let input = poll_menu_edges(
            &mut p_up, &mut p_dn, &mut p_lf, &mut p_rt, &mut p_ok, &mut p_back,
        );
        match screen {
            MenuScreen::Main => {
                wrap_move(&mut main_sel, MAIN_ITEMS.len() as i32, input);
                if input.ok {
                    unsafe { crate::sfx::play(crate::sfx::BUTTON) };
                    match main_sel {
                        0 => {
                            draw_loading(fb, &font, bg, logo, lw, lh, CHAPTERS[0]);
                            return CHAPTER_ROOM[0] as usize;
                        }
                        1 => screen = MenuScreen::Chapters,
                        2 => screen = MenuScreen::Options,
                        _ => screen = MenuScreen::Credits,
                    }
                }
            }
            MenuScreen::Chapters => {
                wrap_move(&mut chapter_sel, CHAPTERS.len() as i32, input);
                if input.back {
                    screen = MenuScreen::Main;
                } else if input.ok {
                    let idx = chapter_sel as usize;
                    if idx < MAPS.len() && CHAPTER_ROOM[idx] >= 0 {
                        draw_loading(fb, &font, bg, logo, lw, lh, CHAPTERS[idx]);
                        return CHAPTER_ROOM[idx] as usize;
                    }
                }
            }
            MenuScreen::Options => {
                wrap_move(&mut options_sel, N_OPTIONS, input);
                let idx = options_sel as usize;
                if idx < 4 && (input.left || input.right) {
                    let step = if idx < 2 { 2 } else { 1 }; // screen +-2 px, volume +-1
                    crate::settings::adjust(idx, if input.right { step } else { -step });
                    unsafe { crate::sfx::play(crate::sfx::BUTTON) }; // click = live feedback
                }
                if input.back || (input.ok && idx == 4) {
                    screen = MenuScreen::Main;
                }
            }
            MenuScreen::Credits => {
                if input.back || input.ok {
                    screen = MenuScreen::Main;
                }
            }
        }

        match screen {
            MenuScreen::Main => {
                draw_shell(fb, &font, bg, logo, lw, lh, "");
                draw_main_menu(&font, main_sel as usize);
            }
            MenuScreen::Chapters => {
                draw_shell(fb, &font, bg, logo, lw, lh, "Chapter Select");
                draw_chapter_menu(&font, chapter_sel);
            }
            MenuScreen::Options => {
                draw_shell(fb, &font, bg, logo, lw, lh, "Options");
                draw_options_menu(&font, options_sel as usize);
            }
            MenuScreen::Credits => {
                draw_shell(fb, &font, bg, logo, lw, lh, "Credits");
                draw_credits_menu(&font);
            }
        }
        draw_footer(&font);

        gpu::draw_sync();
        interrupts::wait_vblank();
        fb.swap();
    }
}

pub fn room_for_map_name(name: &str) -> Option<usize> {
    let mut i = 0usize;
    while i < MAPS.len() {
        if MAPS[i].as_bytes() == name.as_bytes() {
            return Some(i);
        }
        i += 1;
    }
    None
}
