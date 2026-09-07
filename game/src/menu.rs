//! Boot menu styled after Half-Life's original (WON) main menu, built
//! from the install's own assets via `host/hl-content` (git-ignored
//! data/menu/, like every other extracted asset):
//!   - BASIC_8X16 -- PSoXide's clean fixed-width 8x16 bitmap font. The original
//!                    GoldSrc QFONT has deliberately segmented digits that
//!                    become ambiguous at 320x240 (notably `0` in map names).
//!   - logo.tex    -- the real HALF-LIFE wordmark (resource/logo.tga), 4bpp w/
//!                    a transparent background.
//!   - bg.tex      -- the console background (gfx/conback.lmp), desaturated +
//!                    darkened; the only full-screen art a Steam copy ships
//!                    (the WON splash.bmp isn't present).
//! Colors are the scheme's: items orange 255 170 0, selected white with a faint
//! orange armed bar. Returns the chosen streamed room id.

use crate::hltext;
use psx_font::{
    fonts::{BASIC_8X16, SPLEEN_5X8},
    FontAtlas,
};
use psx_gpu::material::TextureMaterial;
use psx_gpu::{self as gpu, framebuf::FrameBuffer};
use psx_math::fmt::{i32_dec, I32_DEC_MAX};
use psx_pad::{button, poll_port1, PadTracker};
use psx_rt::interrupts;
use psx_vram::{upload_bytes, Clut, TexDepth, Tpage, VramRect};

const FONT_TPAGE: Tpage = Tpage::new(320, 0, TexDepth::Bit4);
const FONT_CLUT: Clut = Clut::new(320, 256);

// Exact colors from valve/640_textscheme.txt "Primary Button Text".
const WHITE: (u8, u8, u8) = (238, 238, 230);
// The WON main menu is not text at retail: gfx/shell/btns_main.bmp holds a
// pre-rendered strip whose art is white, with a yellow frame for the armed row
// and the mnemonic letter underlined. A Steam install ships no such file, so
// Xash3D's CMenuPicButton falls back to drawing the label, which is what we do.
// Both item colours are Xash3D's: uiPromptTextColor for a normal row and
// uiPromptFocusColor for the armed one. The unarmed row is amber, not white --
// white is what the button art looks like at a glance, but the fallback the
// engine actually draws is this.
const ITEM: (u8, u8, u8) = (240, 180, 24);
const ITEM_ARMED: (u8, u8, u8) = (255, 255, 0);
const HINT: (u8, u8, u8) = (160, 160, 160);
const SHADOW: (u8, u8, u8) = (0, 0, 0);
const ITEM_SEL: (u8, u8, u8) = (255, 255, 255); // other screens' armed colour
const ARMED: (u8, u8, u8) = (75, 53, 10); // BgColorArmed 255 170 0 @67 over the dark bg
const DIM: (u8, u8, u8) = (110, 110, 104);

// Keep the runnable map order in sync with host/hl-content/map-list.txt.
// Hazard Course is appended so the campaign's stable 0..=95 room ids never move.
pub const CAMPAIGN_MAP_COUNT: usize = 96;
pub const TRAINING_START_ROOM: usize = CAMPAIGN_MAP_COUNT;
pub const MAPS: [&str; 103] = [
    "c0a0", "c0a0a", "c0a0b", "c0a0c", "c0a0d", "c0a0e", "c1a0", "c1a0a", "c1a0b", "c1a0c",
    "c1a0d", "c1a0e", "c1a1", "c1a1a", "c1a1b", "c1a1c", "c1a1d", "c1a1f", "c1a2", "c1a2a",
    "c1a2b", "c1a2c", "c1a2d", "c1a3", "c1a3a", "c1a3b", "c1a3c", "c1a3d", "c1a4", "c1a4b",
    "c1a4d", "c1a4e", "c1a4f", "c1a4g", "c1a4i", "c1a4j", "c1a4k", "c2a1", "c2a1a", "c2a1b",
    "c2a2", "c2a2a", "c2a2b1", "c2a2b2", "c2a2c", "c2a2d", "c2a2e", "c2a2f", "c2a2g", "c2a2h",
    "c2a3", "c2a3a", "c2a3b", "c2a3c", "c2a3d", "c2a3e", "c2a4", "c2a4a", "c2a4b", "c2a4c",
    "c2a4d", "c2a4e", "c2a4f", "c2a4g", "c2a5", "c2a5a", "c2a5b", "c2a5c", "c2a5d", "c2a5e",
    "c2a5f", "c2a5g", "c2a5w", "c2a5x", "c3a1", "c3a1a", "c3a1b", "c3a2", "c3a2a", "c3a2b",
    "c3a2c", "c3a2d", "c3a2e", "c3a2f", "c4a1", "c4a1a", "c4a1b", "c4a1c", "c4a1d", "c4a1e",
    "c4a1f", "c4a2", "c4a2a", "c4a2b", "c4a3", "c5a1", "t0a0", "t0a0a", "t0a0b", "t0a0b1",
    "t0a0b2", "t0a0c", "t0a0d",
];
pub const CHAPTERS: [&str; 19] = [
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
pub const CHAPTER_ROOM: [i16; 19] = [
    0, 6, 12, 18, 23, 28, 37, 40, 50, 56, 60, 64, 74, 77, 84, 85, 91, 94, 95,
];
/// Row index of the Hazard Course in the chapter list: its maps sit after the
/// campaign, so it behaves as one more chapter for sub-map selection.
pub const HAZARD_CHAPTER: usize = CHAPTERS.len();

/// The `MAPS` range a chapter row covers, `[start, end)`. Chapters are ordered
/// and contiguous, so a chapter owns every map up to the next chapter's start.
pub fn chapter_rooms(chapter: usize) -> (usize, usize) {
    if chapter >= HAZARD_CHAPTER {
        return (TRAINING_START_ROOM, MAPS.len());
    }
    let start = (CHAPTER_ROOM[chapter].max(0) as usize).min(CAMPAIGN_MAP_COUNT - 1);
    let end = match CHAPTER_ROOM.get(chapter + 1) {
        Some(&next) if next > 0 => (next as usize).min(CAMPAIGN_MAP_COUNT),
        _ => CAMPAIGN_MAP_COUNT,
    };
    (start, end.max(start + 1))
}

// The title film owns y=35..85, so every sub-screen starts below it. It used to
// start at 84 with the screen title at 58, which put the title across the
// wordmark and the scroll arrows at the very edges of the frame.
const PANEL_TITLE_Y: i16 = 92;
const VISIBLE_ROWS: i32 = 5;
const LIST_X: i16 = 20;
const LIST_Y: i16 = 114;
const ROW_H: i16 = 20;
const LIST_BOTTOM: i16 = LIST_Y + VISIBLE_ROWS as i16 * ROW_H;
/// Right-hand scroll track. A bar says how far through a list you are and costs
/// no vertical space; the up/down arrows it replaces cost a row at each end and
/// still collided with the wordmark and the bottom of the frame.
const SCROLL_X: i16 = 300;
const HINT_Y: i16 = 224;
// Retail puts the item column at 70/640 of the screen and the hint column at
// 235/640. Ours sits a little wider because an 8-pixel cell needs more room for
// the same label than Arial 17 does at twice the resolution.
const MAIN_X: i16 = 36;
const MAIN_Y: i16 = 88;
const HELP_X: i16 = 136;
/// Centres the 8-pixel hint line on the 16-pixel item.
const HELP_BASELINE: i16 = 4;
const UNDERLINE_Y: i16 = 14;
/// Retail sets these in Arial, which is narrower than an 8-pixel cell: at the
/// same point size "Configuration" runs 135 of 640 pixels there and 104 of 320
/// here, so the row reads as twice the weight it should. One pixel of negative
/// tracking brings it to 92 without letting any pair touch; two makes "Hazard
/// course" read as one word.
const MAIN_TRACKING: i8 = -1;

// Sentence case and retail's ordering: New game, Hazard course, Configuration,
// Load game, then ours. Retail's Multiplayer / Custom game / View readme /
// Previews / Quit have nothing to point at on a console.
const MAIN_ITEMS: [&str; 7] = [
    "New game",
    "Hazard course",
    "Configuration",
    "Load game",
    "Chapters",
    "Controls",
    "Credits",
];
/// The HelpText column, in the manner of resource/GameMenu.res.
const MAIN_HINTS: [&str; 7] = [
    "Start a new game.",
    "Learn how to play Half-Life.",
    "Change game settings.",
    "Load a previously saved game.",
    "Jump straight to any chapter.",
    "View the controller layout.",
    "About this port.",
];
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
    Maps,
    Options,
    Debug,
    Controls,
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

// Assets extracted from the user's install plus the Rust-embedded Bonnie
// Studios mark shared with the Celeste Collection. Each .tex blob is
// `u16 w | u16 h | u16 clut[16] | u8 pix4...`. The SDK font stays linked (it is
// used in-game by hltext too); the bg + logo art is menu-only, so it STREAMS from
// WORLD.PAK chunk MENU_CHUNK_ID instead of sitting in always-resident .rodata
// (~33 KiB reclaimed). `run`/`ending` take the streamed blob (see split_menu).
pub const MENU_CHUNK_ID: u32 = 3003;
const LOGO_VRAM_X: u16 = 640; // clear of framebuffers (X<320) and the font atlas (X320..)
/// The backdrop is 8bpp now and needs 128 VRAM columns rather than 64, which
/// does not fit where it used to sit between the logo and the Bonnie plate. It
/// moves into the gap between the logo animation's right half, which ends at
/// 464, and the logo at 640.
///
/// 512 and not 464, even though 464 is where the gap starts: a texture page X
/// is a four-bit field in units of 64 pixels, so Tpage asserts on anything
/// else. 464 is a multiple of 16, which is the CLUT rule, not this one -- and
/// the assert fires as a panic, which on a no_std guest is a black screen with
/// no other symptom.
///
/// Its CLUT is 256 entries and cannot share the y=256 row the 16-entry CLUTs
/// use, where the free run is only 240 pixels wide. It sits one row lower,
/// clear of its own pixels, which stop at y=240.
const BG_VRAM_X: u16 = 512;
const BG_CLUT_X: u16 = 512;
const BG_CLUT_Y: u16 = 257;
const BONNIE_VRAM_X: u16 = 768;
// The 8x16 font occupies the 4bpp page at X=320. GoldSrc's 640x100
// `media/logo.avi` is cooked to its native 320x50 menu geometry and uploaded
// at X=384. It spans two 4bpp pages: pixels 0..255 at X=384 and 256..319 at
// X=448.
const LOGO_ANIM_VRAM_X: u16 = 384;
const LOGO_ANIM_RIGHT_VRAM_X: u16 = 448;

/// Number of `.tex` sections in the streamed menu chunk. Order must match
/// `pack()` in host/hl-content/src/menu.rs.
const MENU_SECTIONS: usize = 4;
const SECTION_BG: usize = 0;
const SECTION_LOGO: usize = 1;
const SECTION_BONNIE: usize = 2;
const SECTION_LOGO_ANIM: usize = 3;

/// Split the streamed menu chunk into its `.tex` sections. A short or failed
/// read yields empty slices, which upload_tex turns into 0x0 materials that
/// every draw helper skips -- the menu still runs, just unbranded.
fn split_menu(blob: &[u8]) -> [&[u8]; MENU_SECTIONS] {
    let mut out = [&blob[0..0]; MENU_SECTIONS];
    let header = 4 * MENU_SECTIONS;
    if blob.len() < header {
        return out;
    }
    let mut at = header;
    for (index, slot) in out.iter_mut().enumerate() {
        let len = u32::from_le_bytes([
            blob[index * 4],
            blob[index * 4 + 1],
            blob[index * 4 + 2],
            blob[index * 4 + 3],
        ]) as usize;
        *slot = blob.get(at..at + len).unwrap_or(&[]);
        at += len;
    }
    out
}

/// Upload a `.tex` blob to a free tpage at VRAM X = `vx`; return its material + size.
/// A too-short blob (failed stream) yields a 0x0 material the draw helpers skip.
/// Upload an 8bpp section: `u16 w | u16 h | u16 clut[256] | u8 pixels`.
///
/// Only the backdrop uses this, because only the backdrop has to span the
/// narrow band above the console's black crush with anything resembling a
/// gradient.
fn upload_tex8(blob: &[u8], vx: u16) -> (TextureMaterial, u16, u16) {
    const HEADER: usize = 4 + 512;
    if blob.len() < HEADER {
        return (TextureMaterial::new(0, 0), 0, 0);
    }
    let w = u16::from_le_bytes([blob[0], blob[1]]);
    let h = u16::from_le_bytes([blob[2], blob[3]]);
    if w == 0 || h == 0 || blob.len() < HEADER + w as usize * h as usize {
        return (TextureMaterial::new(0, 0), 0, 0);
    }
    // Two pixels per VRAM halfword at 8bpp, against four at 4bpp.
    upload_bytes(VramRect::new(vx, 0, w / 2, h), &blob[HEADER..]);
    upload_bytes(
        VramRect::new(BG_CLUT_X, BG_CLUT_Y, 256, 1),
        &blob[4..HEADER],
    );
    let tp = Tpage::new(vx, 0, TexDepth::Bit8);
    let cl = Clut::new(BG_CLUT_X, BG_CLUT_Y);
    (
        TextureMaterial::new(cl.uv_clut_word(), tp.uv_tpage_word(0)),
        w,
        h,
    )
}

fn upload_tex(blob: &[u8], vx: u16) -> (TextureMaterial, u16, u16) {
    if blob.len() < 36 {
        return (TextureMaterial::new(0, 0), 0, 0);
    }
    let w = u16::from_le_bytes([blob[0], blob[1]]);
    let h = u16::from_le_bytes([blob[2], blob[3]]);
    if blob.len() < 36 + (w as usize * h as usize) / 2 || w == 0 || h == 0 {
        return (TextureMaterial::new(0, 0), 0, 0);
    }
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

// Ignore input briefly so a button still held from boot does not consume the
// studio splash.
const TITLE_SKIP_GRACE: i32 = 8;

const LOGO_ANIM_HEADER: usize = 8 + 32;

/// Upload a newly selected cooker frame from GoldSrc's `media/logo.avi`.
///
/// The custom blob is `u16 w | u16 h | u16 frames | u16 cycle_vblanks |
/// u16 clut[16] | packed 4bpp frames...`. Invalid streamed data leaves the
/// materials inert so the caller can draw the static wordmark instead.
fn update_logo_anim(
    blob: &[u8],
    phase: i32,
    current_frame: &mut u16,
    current: &mut (TextureMaterial, TextureMaterial, u16, u16),
) {
    if blob.len() < LOGO_ANIM_HEADER {
        return;
    }
    let word = |at: usize| u16::from_le_bytes([blob[at], blob[at + 1]]);
    let w = word(0);
    let h = word(2);
    let frames = word(4);
    let cycle = word(6);
    if w != 320 || h == 0 || h > 256 || frames == 0 || cycle == 0 {
        return;
    }
    let tick = phase.rem_euclid(cycle as i32) as usize;
    let frame = (tick * frames as usize / cycle as usize).min(frames as usize - 1) as u16;
    if frame == *current_frame {
        return;
    }
    let frame_bytes = w as usize * h as usize / 2;
    let at = LOGO_ANIM_HEADER + frame as usize * frame_bytes;
    let Some(pixels) = blob.get(at..at + frame_bytes) else {
        return;
    };

    upload_bytes(VramRect::new(LOGO_ANIM_VRAM_X, 0, w / 4, h), pixels);
    upload_bytes(
        VramRect::new(LOGO_ANIM_VRAM_X, 256, 16, 1),
        &blob[8..LOGO_ANIM_HEADER],
    );
    // The title is a light sweep over the backdrop, so add it rather than
    // paint it. logo.avi's frames are letterboxed in their own near-black,
    // which as an opaque quad lays a lighter rectangle across the whole width
    // wherever the backdrop is darker than the film's black. Additively that
    // background contributes nothing and the streaks read as light, which is
    // what they are. Needs the 0x8000 semi-transparency bit on the cooked
    // palette (see build_logo_animation).
    let clut = Clut::new(LOGO_ANIM_VRAM_X, 256);
    let blend = gpu::material::BlendMode::Add;
    let left = Tpage::new(LOGO_ANIM_VRAM_X, 0, TexDepth::Bit4);
    let right = Tpage::new(LOGO_ANIM_RIGHT_VRAM_X, 0, TexDepth::Bit4);
    let tint = (128, 128, 128);
    current.0 = TextureMaterial::blended(
        clut.uv_clut_word(),
        left.uv_tpage_word(blend.tpage_bits()),
        tint,
        blend,
    );
    current.1 = TextureMaterial::blended(
        clut.uv_clut_word(),
        right.uv_tpage_word(blend.tpage_bits()),
        tint,
        blend,
    );
    current.2 = w;
    current.3 = h;
    *current_frame = frame;
}

/// GoldSrc places the 640x100 movie at y=70 in a 640x480 menu. At half
/// resolution that is a 320x50 strip at y=35. The cooked 4bpp frame crosses
/// one texture-page boundary, so the rightmost 64 pixels use a second quad.
fn draw_logo_anim(left: TextureMaterial, right: TextureMaterial, w: u16, h: u16) {
    if w != 320 || h == 0 {
        return;
    }
    const Y: i16 = 35;
    let y1 = Y + h as i16;
    let v1 = (h - 1) as u8;
    gpu::draw_quad_textured_material(
        [(0, Y), (256, Y), (0, y1), (256, y1)],
        [(0, 0), (255, 0), (0, v1), (255, v1)],
        left,
    );
    gpu::draw_quad_textured_material(
        [(256, Y), (320, Y), (256, y1), (320, y1)],
        [(0, 0), (63, 0), (0, v1), (63, v1)],
        right,
    );
}

/// The textured HALF-LIFE wordmark, centered across the top.
fn draw_logo(mat: TextureMaterial, w: u16, h: u16) {
    draw_logo_tinted(mat, w, h, 14, (128, 128, 128));
}

#[inline(never)]
#[optimize(size)]
fn draw_logo_tinted(mat: TextureMaterial, w: u16, h: u16, y0: i16, tint: (u8, u8, u8)) {
    if w == 0 || h == 0 {
        return;
    }
    let sw = 292i16;
    let sh = h as i16 * sw / w as i16;
    let x0 = 160 - sw / 2;
    let (uw, uh) = ((w - 1) as u8, (h - 1) as u8);
    gpu::draw_quad_textured_material(
        [(x0, y0), (x0 + sw, y0), (x0, y0 + sh), (x0 + sw, y0 + sh)],
        [(0, 0), (uw, 0), (0, uh), (uw, uh)],
        mat.with_tint(tint),
    );
}

/// Shipping boot splash, matched to the Celeste Classic Collection cadence:
/// fade the project logo and "Built with PSoXide" treatment in, hold, then
/// fade out. A fresh face-button/Start press skips after the opening frames.
#[inline(never)]
#[optimize(size)]
pub fn intro(fb: &mut FrameBuffer, menu_blob: &[u8]) {
    let sections = split_menu(menu_blob);
    let (bonnie, bw, bh) = upload_tex(sections[SECTION_BONNIE], BONNIE_VRAM_X);
    let font = FontAtlas::upload(&BASIC_8X16, FONT_TPAGE, FONT_CLUT);

    const FADE_IN: i32 = 32;
    const HOLD: i32 = 74;
    const TOTAL: i32 = 150;
    const FADE_OUT: i32 = TOTAL - FADE_IN - HOLD;
    const TAG: &str = "Built with PSoXide";

    let any = |bits: u16| bits & (button::CROSS | button::CIRCLE | button::START) != 0;
    let mut frame = 0i32;
    while frame < TOTAL {
        // Held, not edge-triggered, past a short grace: holding Start from
        // boot should walk out of the splash.
        if frame > TITLE_SKIP_GRACE && any(poll_port1().buttons.bits()) {
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

        fb.clear(0, 0, 0);
        let logo_level = level as u8;
        draw_bonnie_tinted(
            bonnie,
            bw,
            bh,
            112,
            34,
            (logo_level, logo_level, logo_level),
        );
        draw_sheen(
            &font,
            160 - font.text_width(TAG) as i16 / 2,
            150,
            TAG,
            frame,
            level,
        );

        gpu::draw_sync();
        psx_rt::interrupts::wait_vblank();
        fb.swap();
        frame += 1;
    }
}

#[inline(never)]
#[optimize(size)]
fn draw_sheen(font: &FontAtlas, mut x: i16, y: i16, text: &str, frame: i32, brightness: i32) {
    let span = text.chars().count() as i32 + 18;
    let head = (frame / 2).rem_euclid(span);
    for (index, ch) in text.char_indices() {
        let glyph = &text[index..index + ch.len_utf8()];
        let amount = (18 - (index as i32 - head).abs() * 6).max(0);
        let channel = |base: i32| {
            let dim = base * brightness / 128;
            (dim + (brightness - dim) * amount / 18) as u8
        };
        font.draw_text(x, y, glyph, (channel(76), channel(108), channel(128)));
        x += font.text_width(glyph) as i16;
    }
}

/// Celeste Collection boot geometry: the 128px source logo is centered as a
/// 96x96 mark above the PSoXide line.
#[inline(never)]
#[optimize(size)]
fn draw_bonnie_tinted(mat: TextureMaterial, w: u16, h: u16, x: i16, y: i16, tint: (u8, u8, u8)) {
    if w == 0 || h == 0 {
        return;
    }
    const SIZE: i16 = 96;
    let (uw, uh) = ((w - 1) as u8, (h - 1) as u8);
    gpu::draw_quad_textured_material(
        [(x, y), (x + SIZE, y), (x, y + SIZE), (x + SIZE, y + SIZE)],
        [(0, 0), (uw, 0), (0, uh), (uw, uh)],
        mat.with_tint(tint),
    );
}

// ---------------------------------------------------------------------------
// Menu text glow, as valve/resource/TrackerScheme.res defines it
//
// A retail menu item is two draws of the same string. "MenuLarge" is the crisp
// pass: antialias 1, blur 0, dropshadow 1. "MenuBlurLarge" is the halo, the
// same face at a lighter weight with antialias 0, blur 5 and additive 1, in
// BlurMenuColor. So the halo is a blurred copy of the letterform composited
// additively underneath, and the shadow is what keeps the glyph legible on top
// of it.
//
// We cannot blur the framebuffer (no VRAM readback), so the halo is its own
// texture: every printable glyph is blurred once into a padded cell at menu
// entry and drawn as one additive quad per glyph. Per PS1 rules an additive
// *textured* primitive needs the 0x8000 semi-transparency bit on every non-zero
// CLUT entry -- the trap the sprite pack hit in M39 -- so the palette is built
// here rather than borrowed from the font atlas.
//
// The blur has to be a real average, not a distance splat from the ink. A splat
// sets every pixel within its radius to a high value, which fills the gaps
// between letters and the counters of o, e and a, and the word ends up sitting
// on a soft orange slab -- a highlight bar, the one thing the effect exists to
// replace. Averaging leaves a pixel surrounded by background dim, which is what
// makes it read as light.
//
// One 4bpp page at X=832 (clear of the framebuffers and every other menu
// texture), 96 cells of 16x24 in 16 columns.
const GLOW_VRAM_X: u16 = 832;
const GLOW_FIRST_CHAR: u8 = 0x20;
const GLOW_CHAR_COUNT: usize = 96;
const GLOW_COLS: usize = 16;
const GLOW_CELL_W: usize = 16; // 8-pixel glyph plus a 4-pixel halo each side
const GLOW_CELL_H: usize = 24;
const GLOW_PAD_X: i16 = 4;
const GLOW_PAD_Y: i16 = 4;
/// Retail blurs a 28-pixel face by 5. Two box passes is the same reach on a
/// 16-pixel one, and the average is left as it falls: a one-pixel stroke blurred
/// over 5x5 peaks around a fifth, which is the rim an unarmed row wants. Any
/// gain baked in here multiplies with the draw tint and turns every row into a
/// blob of its own colour.
const GLOW_BLUR_PASSES: usize = 2;
const GLOW_GAIN: u32 = 1;
/// TrackerScheme.res BlurMenuColor.
const GLOW_RGB: (u8, u8, u8) = (255, 178, 67);
/// Texture modulation is `texel * tint / 128`, so 255 is roughly double. Retail
/// separates the two states by colour alone, which works for antialiased Arial
/// at 28 pixels; our one-pixel strokes get swallowed by a halo of their own hue,
/// so the armed row carries the strong glow and an unarmed row only a rim.
const GLOW_TINT_NORMAL: u8 = 128;
const GLOW_TINT_ARMED: u8 = 255;

fn glow_material(level: u8) -> TextureMaterial {
    TextureMaterial::blended(
        Clut::new(GLOW_VRAM_X, 256).uv_clut_word(),
        Tpage::new(GLOW_VRAM_X, 0, TexDepth::Bit4)
            .uv_tpage_word(gpu::material::BlendMode::Add.tpage_bits()),
        (level, level, level),
        gpu::material::BlendMode::Add,
    )
}

/// Rasterize every printable glyph into its padded cell, blur it, and upload
/// the cell. Per-cell uploads keep the whole build inside one 384-byte buffer
/// instead of staging a 27 KB atlas; this runs once per menu entry, where the
/// cost is invisible beside the streamed textures.
fn upload_glow_atlas() {
    let font = &BASIC_8X16;
    let mut clut = [0u16; 16];
    for (level, entry) in clut.iter_mut().enumerate().skip(1) {
        let scale = |channel: u8| ((channel as usize * level) / 15) as u8;
        // 0x8000 marks the texel semi-transparent; without it the GPU draws an
        // additive textured primitive fully opaque.
        *entry = 0x8000 | bgr555(scale(GLOW_RGB.0), scale(GLOW_RGB.1), scale(GLOW_RGB.2));
    }
    upload_bytes(VramRect::new(GLOW_VRAM_X, 256, 16, 1), unsafe {
        core::slice::from_raw_parts(clut.as_ptr().cast::<u8>(), 32)
    });

    let mut alpha = [0u8; GLOW_CELL_W * GLOW_CELL_H];
    let mut blurred = [0u8; GLOW_CELL_W * GLOW_CELL_H];
    let mut packed = [0u8; GLOW_CELL_W * GLOW_CELL_H / 2];
    for index in 0..GLOW_CHAR_COUNT {
        alpha.fill(0);
        let glyph = GLOW_FIRST_CHAR as usize + index;
        for row in 0..font.glyph_h as usize {
            let bits = font.bitmap[glyph * font.glyph_h as usize + row];
            for column in 0..font.glyph_w as usize {
                if bits & (0x80 >> column) != 0 {
                    alpha[(row + GLOW_PAD_Y as usize) * GLOW_CELL_W
                        + column
                        + GLOW_PAD_X as usize] = 15;
                }
            }
        }
        for row in 0..font.glyph_h as usize {
            let bits = font.bitmap[glyph * font.glyph_h as usize + row];
            for column in 0..font.glyph_w as usize {
                if bits & (0x80 >> column) != 0 {
                    alpha[(row + GLOW_PAD_Y as usize) * GLOW_CELL_W
                        + column
                        + GLOW_PAD_X as usize] = 15;
                }
            }
        }
        for _ in 0..GLOW_BLUR_PASSES {
            blurred.copy_from_slice(&alpha);
            for y in 0..GLOW_CELL_H {
                for x in 0..GLOW_CELL_W {
                    let mut sum = 0u32;
                    for dy in 0..3 {
                        for dx in 0..3 {
                            let sy = (y + dy).wrapping_sub(1);
                            let sx = (x + dx).wrapping_sub(1);
                            if sy < GLOW_CELL_H && sx < GLOW_CELL_W {
                                sum += blurred[sy * GLOW_CELL_W + sx] as u32;
                            }
                        }
                    }
                    alpha[y * GLOW_CELL_W + x] = (sum / 9) as u8;
                }
            }
        }
        for level in alpha.iter_mut() {
            *level = ((*level as u32 * GLOW_GAIN).min(15)) as u8;
        }
        for (byte, pair) in packed.iter_mut().zip(alpha.chunks_exact(2)) {
            *byte = (pair[0] & 0x0f) | (pair[1] << 4);
        }
        let column = (index % GLOW_COLS) as u16;
        let row = (index / GLOW_COLS) as u16;
        upload_bytes(
            VramRect::new(
                GLOW_VRAM_X + column * (GLOW_CELL_W / 4) as u16,
                row * GLOW_CELL_H as u16,
                (GLOW_CELL_W / 4) as u16,
                GLOW_CELL_H as u16,
            ),
            &packed,
        );
    }
}

fn bgr555(r: u8, g: u8, b: u8) -> u16 {
    ((b as u16 >> 3) << 10) | ((g as u16 >> 3) << 5) | (r as u16 >> 3)
}

/// Lay the halo under `text`. Space carries no ink, so it is skipped rather
/// than drawn as an empty quad.
/// `tracking` must match whatever the crisp pass uses. The halo advances per
/// glyph independently, so a mismatch drifts it a pixel per character and by
/// the eighth letter the blur reads as a second, offset copy of the word.
#[inline(never)]
#[optimize(size)]
fn draw_text_glow(x: i16, y: i16, text: &str, level: u8, tracking: i8) {
    let material = glow_material(level);
    let mut cursor = x;
    for ch in text.chars() {
        let code = ch as usize;
        if code > GLOW_FIRST_CHAR as usize && code < GLOW_FIRST_CHAR as usize + GLOW_CHAR_COUNT {
            let index = code - GLOW_FIRST_CHAR as usize;
            let u = ((index % GLOW_COLS) * GLOW_CELL_W) as u8;
            let v = ((index / GLOW_COLS) * GLOW_CELL_H) as u8;
            let (x0, y0) = (cursor - GLOW_PAD_X, y - GLOW_PAD_Y);
            let (x1, y1) = (x0 + GLOW_CELL_W as i16, y0 + GLOW_CELL_H as i16);
            let (uw, vh) = (u + GLOW_CELL_W as u8 - 1, v + GLOW_CELL_H as u8 - 1);
            gpu::draw_quad_textured_material(
                [(x0, y0), (x1, y0), (x0, y1), (x1, y1)],
                [(u, v), (uw, v), (u, vh), (uw, vh)],
                material,
            );
        }
        cursor += BASIC_8X16.advance_x as i16 + tracking as i16;
    }
}

// Small proportional-ish face for the help column beside each menu item, in the
// 4bpp page after the glow atlas. The retail menu prints its hint at the same size as the
// item; at 320x240 our 8x16 cell would leave room for eleven characters, so the
// column gets a 5x8 face instead and the two-column proportions come out close
// to the original's.
const HELP_VRAM_X: u16 = 896;
const HELP_TPAGE: Tpage = Tpage::new(HELP_VRAM_X, 0, TexDepth::Bit4);
const HELP_CLUT: Clut = Clut::new(HELP_VRAM_X, 256);

fn draw_centered(font: &FontAtlas, y: i16, text: &str, color: (u8, u8, u8)) {
    let x = 160 - (font.text_width(text) as i16 / 2);
    font.draw_text(x, y, text, color);
}

/// Scroll indicator for a windowed list: dim track, amber thumb sized and
/// placed by the window. Drawn only when the list actually overflows.
#[inline(never)]
#[optimize(size)]
fn draw_scroll_bar(first: i32, visible: i32, total: i32) {
    if total <= visible || visible <= 0 {
        return;
    }
    let (y0, y1) = (LIST_Y - 2, LIST_BOTTOM - 2);
    let track = y1 - y0;
    gpu::draw_quad_flat(
        [
            (SCROLL_X, y0),
            (SCROLL_X + 4, y0),
            (SCROLL_X, y1),
            (SCROLL_X + 4, y1),
        ],
        26,
        22,
        14,
    );
    let thumb = ((track as i32 * visible / total) as i16).max(8);
    let span = track - thumb;
    let travel = (span as i32 * first / (total - visible)) as i16;
    let top = y0 + travel.clamp(0, span);
    gpu::draw_quad_flat(
        [
            (SCROLL_X, top),
            (SCROLL_X + 4, top),
            (SCROLL_X, top + thumb),
            (SCROLL_X + 4, top + thumb),
        ],
        ITEM.0,
        ITEM.1,
        ITEM.2,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowState {
    Normal,
    Armed,
    Disabled,
}

/// A list row in the main menu's manner: MenuBlurLarge halo, drop shadow, then
/// the glyph. The armed row was a filled brown bar, which is the WON armed
/// colour but fights the halo the rest of the menu is drawn with.
#[inline(never)]
#[optimize(size)]
fn draw_list_row(font: &FontAtlas, x: i16, y: i16, text: &str, state: RowState) {
    if state == RowState::Disabled {
        font.draw_text(x, y, text, DIM);
        return;
    }
    draw_text_glow(
        x,
        y,
        text,
        if state == RowState::Armed {
            GLOW_TINT_ARMED
        } else {
            GLOW_TINT_NORMAL
        },
        0,
    );
    font.draw_text(x + 1, y + 1, text, SHADOW);
    font.draw_text(
        x,
        y,
        text,
        if state == RowState::Armed {
            ITEM_ARMED
        } else {
            ITEM
        },
    );
}

#[inline(never)]
#[optimize(size)]
fn draw_select_bar(x: i16, y: i16, w: i16) {
    gpu::draw_quad_flat(
        [(x, y - 1), (x + w, y - 1), (x, y + 19), (x + w, y + 19)],
        ARMED.0,
        ARMED.1,
        ARMED.2,
    );
}

/// One WON main-menu row: hint column, then the item.
///
/// Xash3D's CMenuPicButton falls back to plain text when `gfx/shell/btns_main`
/// is absent, which it is in a Steam install: `UI_DrawString` with
/// `ETF_SHADOW`, colorBase normally and colorFocus when armed, and no halo of
/// any kind. Retail draws the same row from that bitmap strip, whose art is
/// white with a yellow focus frame and the mnemonic letter underlined. So the
/// row is a drop shadow, a colour, and a one-pixel rule under the first letter.
#[inline(never)]
#[optimize(size)]
fn draw_menu_row(font: &FontAtlas, help: &FontAtlas, y: i16, item: &str, hint: &str, armed: bool) {
    // uiColorHelp, 160 160 160.
    help.draw_text(HELP_X, y + HELP_BASELINE, hint, HINT);
    // MenuBlurLarge: the blurred, additive copy of the letterform, underneath.
    draw_text_glow(
        MAIN_X,
        y,
        item,
        if armed {
            GLOW_TINT_ARMED
        } else {
            GLOW_TINT_NORMAL
        },
        MAIN_TRACKING,
    );
    // ETF_SHADOW.
    font.draw_text_with_spacing(MAIN_X + 1, y + 1, item, MAIN_TRACKING, SHADOW);
    let color = if armed { ITEM_ARMED } else { ITEM };
    font.draw_text_with_spacing(MAIN_X, y, item, MAIN_TRACKING, color);
    // The mnemonic accent the button art carries, under the first glyph only.
    let width = font.text_width(&item[..1]) as i16 + MAIN_TRACKING as i16;
    gpu::draw_quad_flat(
        [
            (MAIN_X, y + UNDERLINE_Y),
            (MAIN_X + width, y + UNDERLINE_Y),
            (MAIN_X, y + UNDERLINE_Y + 1),
            (MAIN_X + width, y + UNDERLINE_Y + 1),
        ],
        color.0,
        color.1,
        color.2,
    );
}

#[inline(never)]
#[optimize(size)]
fn draw_simple_list(
    font: &FontAtlas,
    help: &FontAtlas,
    items: &[&str],
    hints: &[&str],
    sel: usize,
) {
    let mut y = MAIN_Y;
    let mut i = 0usize;
    while i < items.len() {
        draw_menu_row(
            font,
            help,
            y,
            items[i],
            hints.get(i).copied().unwrap_or(""),
            i == sel,
        );
        y += ROW_H;
        i += 1;
    }
}

struct Shell {
    bg: TextureMaterial,
    logo: TextureMaterial,
    lw: u16,
    lh: u16,
}

#[inline(never)]
#[optimize(size)]
fn draw_shell(
    fb: &mut FrameBuffer,
    font: &FontAtlas,
    shell: &Shell,
    anim: (TextureMaterial, TextureMaterial, u16, u16),
    title: &str,
) {
    fb.clear(0, 0, 0);
    draw_bg(shell.bg);
    if anim.2 == 0 {
        draw_logo(shell.logo, shell.lw, shell.lh);
    } else {
        draw_logo_anim(anim.0, anim.1, anim.2, anim.3);
    }
    if !title.is_empty() {
        draw_centered(font, PANEL_TITLE_Y, title, DIM);
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

#[inline]
fn play_menu_move() {
    unsafe { crate::sfx::play_vol(crate::sfx::MENU_MOVE, 3) };
}

#[inline]
fn play_menu_accept() {
    unsafe { crate::sfx::play_vol(crate::sfx::MENU_ACCEPT, 2) };
}

/// One pad poll folded into the tracker's active-high mask: when the pad is
/// analog, left-stick deflection past a +-48 deadzone maps onto the d-pad bits.
fn menu_buttons() -> u16 {
    let pad = poll_port1();
    let mut b = pad.buttons.bits();
    if pad.is_analog() {
        let (lx, ly) = pad.sticks.left_centered();
        const DZ: i16 = 48;
        if ly < -DZ {
            b |= button::UP;
        }
        if ly > DZ {
            b |= button::DOWN;
        }
        if lx < -DZ {
            b |= button::LEFT;
        }
        if lx > DZ {
            b |= button::RIGHT;
        }
    }
    b
}

/// Poll the pad and reduce it to per-frame menu edges. Every cursor step is a
/// fresh press (this menu has never auto-repeated; `pad.repeats(mask, delay,
/// rate)` is the one-line change if hold-to-scroll is ever wanted).
fn poll_menu_edges(pad: &mut PadTracker) -> MenuEdges {
    pad.update(menu_buttons());
    MenuEdges {
        up: pad.just_pressed(button::UP),
        down: pad.just_pressed(button::DOWN),
        left: pad.just_pressed(button::LEFT),
        right: pad.just_pressed(button::RIGHT),
        ok: pad.just_pressed(button::CROSS | button::START),
        back: pad.just_pressed(button::CIRCLE | button::SELECT),
    }
}

fn draw_main_menu(font: &FontAtlas, help: &FontAtlas, sel: usize) {
    draw_simple_list(font, help, &MAIN_ITEMS, &MAIN_HINTS, sel);
}

#[inline(never)]
#[optimize(size)]
fn draw_chapter_menu(font: &FontAtlas, sel: i32) {
    let n = CHAPTERS.len() as i32;
    let max_first = (n - VISIBLE_ROWS).max(0);
    let first = (sel - VISIBLE_ROWS / 2).clamp(0, max_first);
    let last = (first + VISIBLE_ROWS).min(n);
    let mut y = LIST_Y;
    for i in first..last {
        let idx = i as usize;
        let enabled = idx < MAPS.len() && CHAPTER_ROOM[idx] >= 0;
        let state = if !enabled {
            RowState::Disabled
        } else if i == sel {
            RowState::Armed
        } else {
            RowState::Normal
        };
        draw_list_row(font, LIST_X, y, CHAPTERS[idx], state);
        y += ROW_H;
    }
    draw_scroll_bar(first, VISIBLE_ROWS, n);
}

/// Row labels for the settings list, shared with the in-game pause menu so the
/// two cannot drift apart. The pause version shows exactly these rows: the
/// Debug row that follows them here is already a page of its own there.
pub const OPT_LABELS: [&str; 7] = [
    "Screen X",
    "Screen Y",
    "Music",
    "SFX",
    "Stick Deadzone",
    "Autosave",
    "Brightness",
];
pub const N_OPTIONS: i32 = 8; // 6 adjustable values + Autosave + Debug
/// Index of the brightness row inside [`OPT_LABELS`]; `settings::adjust` and
/// `settings::value` dispatch on the same number.
pub const OPT_BRIGHTNESS: usize = 6;

/// Title shown above a chapter's sub-map list.
pub fn chapter_title(chapter: usize) -> &'static str {
    if chapter >= HAZARD_CHAPTER {
        "Hazard Course"
    } else {
        CHAPTERS[chapter]
    }
}

/// The maps inside one chapter. Row 0 is the chapter's authored entry point and
/// says so; the rest exist to drop straight into a later section.
#[inline(never)]
#[optimize(size)]
fn draw_map_menu(font: &FontAtlas, help: &FontAtlas, chapter: usize, sel: i32) {
    let (start, end) = chapter_rooms(chapter);
    let n = (end - start) as i32;
    let max_first = (n - VISIBLE_ROWS).max(0);
    let first = (sel - VISIBLE_ROWS / 2).clamp(0, max_first);
    let last = (first + VISIBLE_ROWS).min(n);
    let mut y = LIST_Y;
    for i in first..last {
        let name = MAPS[start + i as usize];
        let armed = i == sel;
        draw_list_row(
            font,
            LIST_X,
            y,
            name,
            if armed {
                RowState::Armed
            } else {
                RowState::Normal
            },
        );
        if i == 0 {
            let x = LIST_X + font.text_width(name) as i16 + 8;
            help.draw_text(x, y + 4, "chapter start", HINT);
        }
        y += ROW_H;
    }
    draw_scroll_bar(first, VISIBLE_ROWS, n);
}

/// Session-only cheat toggles, shared with the in-game pause menu.
#[inline(never)]
#[optimize(size)]
fn draw_debug_menu(font: &FontAtlas, sel: usize) {
    let labels = crate::settings::DEBUG_LABELS;
    let x_label = 40i16;
    let mut y = 96i16;
    for (i, label) in labels.iter().enumerate() {
        let is_sel = i == sel;
        if is_sel {
            draw_select_bar(x_label - 8, y, 200);
        }
        let col = if is_sel { ITEM_SEL } else { ITEM };
        font.draw_text(x_label, y, label, col);
        let on = crate::settings::debug_on(1 << i);
        font.draw_text(
            x_label + 152,
            y,
            if on { "ON" } else { "OFF" },
            if on { col } else { DIM },
        );
        y += 22;
    }
    y += 8;
    let is_sel = sel == labels.len();
    if is_sel {
        draw_select_bar(x_label - 8, y, 200);
    }
    font.draw_text(x_label, y, "Back", if is_sel { ITEM_SEL } else { ITEM });
    hltext::draw_centered_scaled(212, "Cross toggles", hltext::SMALL_Q8, DIM);
}

/// 8-segment volume bar; filled segments up to `v` are lit, the rest dim.
#[inline(never)]
#[optimize(size)]
fn draw_vol_bar(x: i16, y: i16, v: i32, bright: bool) {
    let lit = if bright { ITEM_SEL } else { ITEM };
    for i in 0..8i16 {
        let sx = x + i * 13;
        let (r, g, b) = if (i as i32) < v { lit } else { (56, 56, 56) };
        gpu::draw_quad_flat(
            [
                (sx, y + 2),
                (sx + 10, y + 2),
                (sx, y + 15),
                (sx + 10, y + 15),
            ],
            r,
            g,
            b,
        );
    }
}

#[inline(never)]
#[optimize(size)]
fn draw_options_menu(font: &FontAtlas, help: &FontAtlas, sel: usize) {
    // Two columns on the list's own grid: label at LIST_X, control at the far
    // side of the panel. The old layout began at y=60, which put the first row
    // across the wordmark, and marked the armed row with a filled bar.
    let x_label = LIST_X;
    let x_val = 176i16;
    const VISIBLE: usize = 5;
    let first = sel
        .saturating_sub(VISIBLE / 2)
        .min(N_OPTIONS as usize - VISIBLE);
    let last = (first + VISIBLE).min(N_OPTIONS as usize);
    let mut y = LIST_Y - 6;
    for i in first..last {
        let is_sel = i == sel;
        let state = if is_sel {
            RowState::Armed
        } else {
            RowState::Normal
        };
        let label = if i < OPT_LABELS.len() {
            OPT_LABELS[i]
        } else {
            "Debug"
        };
        draw_list_row(font, x_label, y, label, state);
        let col = if is_sel { ITEM_ARMED } else { ITEM };
        if (2..4).contains(&i) {
            draw_vol_bar(x_val, y, crate::settings::value(i), is_sel);
        } else if i == 5 {
            font.draw_text(x_val, y, "<", col);
            let on = crate::settings::autosave_enabled();
            font.draw_text(x_val + 32, y, if on { "ON" } else { "OFF" }, col);
            font.draw_text(x_val + 88, y, ">", col);
        } else if i < OPT_LABELS.len() {
            // Screen X/Y, stick deadzone and brightness are all "< n >" rows.
            let mut buf = [0u8; I32_DEC_MAX];
            let value = i32_dec(&mut buf, crate::settings::value(i));
            font.draw_text(x_val, y, "<", col);
            font.draw_text(x_val + 40, y, value, col);
            font.draw_text(x_val + 88, y, ">", col);
        }
        y += ROW_H;
    }
    draw_scroll_bar(first as i32, VISIBLE as i32, N_OPTIONS);
    help.draw_text(
        LIST_X,
        HINT_Y + 6,
        "Left / Right adjusts, Circle goes back",
        HINT,
    );
}

/// Rows visible in the credits window. The lines are set in the small face, so
/// the same panel holds more of them than a chapter list.
const CREDIT_ROWS: i32 = 9;
const CREDIT_ROW_H: i16 = 12;

#[inline(never)]
#[optimize(size)]
fn draw_credits_menu(help: &FontAtlas, first: i32) {
    let n = CREDIT_LINES.len() as i32;
    let first = first.clamp(0, (n - CREDIT_ROWS).max(0));
    let last = (first + CREDIT_ROWS).min(n);
    let mut y = LIST_Y - 4;
    for i in first..last {
        let (text, color) = CREDIT_LINES[i as usize];
        let x = 160 - (help.text_width(text) as i16 / 2);
        help.draw_text(x, y, text, color);
        y += CREDIT_ROW_H;
    }
    draw_scroll_bar(first, CREDIT_ROWS, n);
    let hint = if n > CREDIT_ROWS {
        "Up / Down scrolls, Circle goes back"
    } else {
        "Circle goes back"
    };
    let x = 160 - (help.text_width(hint) as i16 / 2);
    help.draw_text(x, HINT_Y + 6, hint, HINT);
}

const CONTROL_LEFT: [(&str, &str); 6] = [
    ("L Stick", "Move"),
    ("D-Pad", "Move"),
    ("R Stick", "Look"),
    ("R2", "Primary Fire"),
    ("L2", "Secondary Fire"),
    ("L1/R1", "Change Weapon"),
];
const CONTROL_RIGHT: [(&str, &str); 6] = [
    ("Cross", "Jump"),
    ("Triangle", "Crouch"),
    ("Square", "Use"),
    ("Circle", "Reload"),
    ("L3", "Flashlight"),
    ("Start/Select", "Pause"),
];

#[inline(never)]
#[optimize(size)]
fn draw_control_column(font: &FontAtlas, rows: &[(&str, &str)], label_x: i16) {
    // Six rows below the screen title, on a tighter pitch than a list so the
    // last one clears the hint line.
    const CONTROL_ROW_H: i16 = 18;
    let mut y = LIST_Y - 4;
    let mut i = 0usize;
    while i < rows.len() {
        font.draw_text(label_x, y, rows[i].0, ITEM);
        font.draw_text(
            label_x + font.text_width(rows[i].0) as i16 + 8,
            y,
            rows[i].1,
            WHITE,
        );
        y += CONTROL_ROW_H;
        i += 1;
    }
}

#[inline(never)]
#[optimize(size)]
fn draw_controls_menu(font: &FontAtlas, help: &FontAtlas) {
    // Keep a real gutter between the two longest rows ("Change Weapon" and
    // "Start/Select Pause") while still fitting the right column at 320 px.
    draw_control_column(font, &CONTROL_LEFT, 8);
    draw_control_column(font, &CONTROL_RIGHT, 172);
    // No Back row: this screen has no other choice to make, so the button that
    // already leaves it is the whole interaction. Same for Credits.
    let hint = "Circle goes back";
    let x = 160 - (help.text_width(hint) as i16 / 2);
    help.draw_text(x, HINT_Y + 6, hint, HINT);
}

/// The campaign end card: white-in from the c5a1 fade, the wordmark, THE END,
/// and the credit lines. Waits for a button, then returns (to the main menu).
#[inline(never)]
#[optimize(size)]
pub fn ending(fb: &mut FrameBuffer, assets: &[u8]) {
    let font = FontAtlas::upload(&BASIC_8X16, FONT_TPAGE, FONT_CLUT);
    let (logo, lw, lh) = upload_tex(split_menu(assets)[SECTION_LOGO], LOGO_VRAM_X);
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
#[inline(never)]
#[optimize(size)]
pub fn run(fb: &mut FrameBuffer, assets: &[u8]) -> usize {
    let font = FontAtlas::upload(&BASIC_8X16, FONT_TPAGE, FONT_CLUT);
    let help_font = FontAtlas::upload(&SPLEEN_5X8, HELP_TPAGE, HELP_CLUT);
    upload_glow_atlas();
    let sections = split_menu(assets);
    let (logo, lw, lh) = upload_tex(sections[SECTION_LOGO], LOGO_VRAM_X);
    let (bg, _, _) = upload_tex8(sections[SECTION_BG], BG_VRAM_X);
    let shell = Shell { bg, logo, lw, lh };
    let mut title_phase = 0i32;
    let mut title_frame = u16::MAX;
    let mut title_anim = (
        TextureMaterial::new(0, 0),
        TextureMaterial::new(0, 0),
        0u16,
        0u16,
    );
    let mut screen = MenuScreen::Main;
    let mut main_sel = 0i32;
    let mut load_failed = false;
    let mut chapter_sel = 0i32;
    let mut options_sel = 0i32;
    let mut debug_sel = 0i32;
    let mut credits_first = 0i32;
    // Chapter row whose sub-map list is open, and the row inside it.
    let mut map_chapter = 0usize;
    let mut map_sel = 0i32;
    let mut pad = PadTracker::new();
    pad.update(menu_buttons());
    pad.prime(); // swallow buttons (or stick) still held at menu entry until re-pressed

    loop {
        let input = poll_menu_edges(&mut pad);
        if input.up || input.down {
            play_menu_move();
        }
        match screen {
            MenuScreen::Main => {
                wrap_move(&mut main_sel, MAIN_ITEMS.len() as i32, input);
                if input.ok {
                    play_menu_accept();
                    match main_sel {
                        0 => {
                            // No screen of our own here: play() overlays its
                            // loading strip on this very frame a moment later.
                            return CHAPTER_ROOM[0] as usize;
                        }
                        1 => {
                            map_chapter = HAZARD_CHAPTER;
                            map_sel = 0;
                            screen = MenuScreen::Maps;
                        }
                        2 => screen = MenuScreen::Options,
                        // Load game: the room comes from the card, and the
                        // restore is applied by play() after its normal setup.
                        3 => match crate::load_saved_room() {
                            Some(room) => return room,
                            None => load_failed = true,
                        },
                        4 => screen = MenuScreen::Chapters,
                        5 => screen = MenuScreen::Controls,
                        _ => screen = MenuScreen::Credits,
                    }
                }
            }
            MenuScreen::Chapters => {
                wrap_move(&mut chapter_sel, CHAPTERS.len() as i32, input);
                if input.back {
                    play_menu_accept();
                    screen = MenuScreen::Main;
                } else if input.ok {
                    let idx = chapter_sel as usize;
                    if CHAPTER_ROOM[idx] >= 0 && (CHAPTER_ROOM[idx] as usize) < MAPS.len() {
                        play_menu_accept();
                        map_chapter = idx;
                        map_sel = 0;
                        screen = MenuScreen::Maps;
                    }
                }
            }
            // Sub-map list: the chapter's own first map is the normal entry
            // point, every later one is a direct spawn for testing a section
            // without replaying the chapter up to it.
            MenuScreen::Maps => {
                let (start, end) = chapter_rooms(map_chapter);
                wrap_move(&mut map_sel, (end - start) as i32, input);
                if input.back {
                    play_menu_accept();
                    screen = if map_chapter >= HAZARD_CHAPTER {
                        MenuScreen::Main
                    } else {
                        MenuScreen::Chapters
                    };
                } else if input.ok {
                    let room = start + map_sel as usize;
                    if room < MAPS.len() {
                        play_menu_accept();
                        return room;
                    }
                }
            }
            MenuScreen::Options => {
                wrap_move(&mut options_sel, N_OPTIONS, input);
                let idx = options_sel as usize;
                if (idx < 5 || idx == OPT_BRIGHTNESS) && (input.left || input.right) {
                    let step = if idx < 2 {
                        2 // screen +-2 px
                    } else if idx == 4 {
                        4 // radial deadzone axis units
                    } else {
                        1 // volume, brightness level
                    };
                    crate::settings::adjust(idx, if input.right { step } else { -step });
                    play_menu_move(); // restrained live feedback for each adjustment
                }
                if idx == 5 && (input.left || input.right || input.ok) {
                    crate::settings::toggle_autosave();
                    play_menu_move();
                }
                if input.ok && idx == OPT_LABELS.len() {
                    play_menu_accept();
                    debug_sel = 0;
                    screen = MenuScreen::Debug;
                } else if input.back {
                    play_menu_accept();
                    screen = MenuScreen::Main;
                }
            }
            MenuScreen::Debug => {
                let rows = crate::settings::DEBUG_LABELS.len() as i32;
                wrap_move(&mut debug_sel, rows + 1, input);
                if input.ok && debug_sel < rows {
                    play_menu_move();
                    crate::settings::debug_toggle(debug_sel as usize);
                } else if input.back || input.ok {
                    play_menu_accept();
                    screen = MenuScreen::Options;
                }
            }
            MenuScreen::Controls => {
                if input.back || input.ok {
                    play_menu_accept();
                    screen = MenuScreen::Main;
                }
            }
            MenuScreen::Credits => {
                let overflow = (CREDIT_LINES.len() as i32 - CREDIT_ROWS).max(0);
                if input.down {
                    credits_first = (credits_first + 1).min(overflow);
                    play_menu_move();
                }
                if input.up {
                    credits_first = (credits_first - 1).max(0);
                    play_menu_move();
                }
                if input.back || input.ok {
                    play_menu_accept();
                    screen = MenuScreen::Main;
                }
            }
        }

        update_logo_anim(
            sections[SECTION_LOGO_ANIM],
            title_phase,
            &mut title_frame,
            &mut title_anim,
        );
        match screen {
            MenuScreen::Main => {
                draw_shell(fb, &font, &shell, title_anim, "");
                draw_main_menu(&font, &help_font, main_sel as usize);
                if load_failed {
                    hltext::draw_centered_scaled(
                        200,
                        "No save on the memory card",
                        hltext::SMALL_Q8,
                        DIM,
                    );
                }
            }
            MenuScreen::Chapters => {
                draw_shell(fb, &font, &shell, title_anim, "Chapter Select");
                draw_chapter_menu(&font, chapter_sel);
            }
            MenuScreen::Maps => {
                draw_shell(fb, &font, &shell, title_anim, chapter_title(map_chapter));
                draw_map_menu(&font, &help_font, map_chapter, map_sel);
            }
            MenuScreen::Options => {
                draw_shell(fb, &font, &shell, title_anim, "Configuration");
                draw_options_menu(&font, &help_font, options_sel as usize);
            }
            MenuScreen::Debug => {
                draw_shell(fb, &font, &shell, title_anim, "Debug");
                draw_debug_menu(&font, debug_sel as usize);
            }
            MenuScreen::Controls => {
                draw_shell(fb, &font, &shell, title_anim, "Controls");
                draw_controls_menu(&font, &help_font);
            }
            MenuScreen::Credits => {
                draw_shell(fb, &font, &shell, title_anim, "Credits");
                draw_credits_menu(&help_font, credits_first);
            }
        }
        title_phase = title_phase.wrapping_add(1);

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
