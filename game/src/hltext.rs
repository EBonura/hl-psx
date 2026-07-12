//! Direct scaled text from the extracted Half-Life menu font. This is used for
//! modal overlays where uploading a second font atlas would trample gameplay
//! textures resident in VRAM.

use psx_gpu as gpu;
use psx_gpu::material::TextureMaterial;
use psx_vram::{upload_16bpp, upload_bytes, Clut, TexDepth, Tpage, VramRect};

static HLFONT_BLOB: &[u8] = include_bytes!("../../data/menu/hlfont.bin");

pub const SMALL_Q8: u16 = 192; // 75%

// The HUD occupies u=0..159, v=0..187 in the last 4-bit gameplay
// tpage. Its unused 96-texel right strip holds all 95 printable ASCII
// glyphs after exact SMALL_Q8 rasterisation: 8 columns x 12 rows of
// 12x15 cells. This replaces hundreds of tiny immediate flat quads in
// chapter cards with one transparent textured quad per character.
const GAME_TPAGE_X: u16 = 960;
const GAME_TPAGE_Y: u16 = 256;
const GAME_ATLAS_U: u8 = 160;
const GAME_CELL_W: usize = 12;
const GAME_CELL_H: usize = 15;
const GAME_COLS: usize = 8;
const GAME_ATLAS_W: usize = 96;
const GAME_ROW_BYTES: usize = GAME_ATLAS_W * GAME_CELL_H / 2;
const GAME_CLUT_X: u16 = 976; // immediately after the HUD's x=960 CLUT
const GAME_CLUT_Y: u16 = 504;

fn glyph_w() -> u8 {
    HLFONT_BLOB[0]
}

fn glyph_h() -> u8 {
    HLFONT_BLOB[1]
}

fn glyph_count() -> usize {
    u16::from_le_bytes([HLFONT_BLOB[2], HLFONT_BLOB[3]]) as usize
}

fn first_char() -> u16 {
    u16::from_le_bytes([HLFONT_BLOB[4], HLFONT_BLOB[5]])
}

fn advances() -> &'static [u8] {
    &HLFONT_BLOB[8..8 + glyph_count()]
}

fn bitmap() -> &'static [u8] {
    &HLFONT_BLOB[8 + glyph_count()..]
}

fn row_bytes() -> usize {
    (glyph_w() as usize).div_ceil(8)
}

fn glyph_index(ch: char) -> Option<usize> {
    let cp = ch as u32;
    let first = first_char() as u32;
    let end = first.saturating_add(glyph_count() as u32);
    if cp >= first && cp < end {
        Some((cp - first) as usize)
    } else {
        None
    }
}

fn glyph_advance(ch: char) -> u8 {
    glyph_index(ch)
        .and_then(|idx| advances().get(idx).copied())
        .unwrap_or(glyph_w())
}

fn scale_i16(v: i16, scale_q8: u16) -> i16 {
    ((v as i32 * scale_q8 as i32 + 128) >> 8) as i16
}

#[inline(always)]
fn set_4bpp_pixel(dst: &mut [u8], stride_px: usize, x: usize, y: usize) {
    let p = y * stride_px + x;
    let b = &mut dst[p >> 1];
    if p & 1 == 0 {
        *b |= 1;
    } else {
        *b |= 0x10;
    }
}

/// Upload the SMALL_Q8 gameplay font atlas into the HUD tpage's unused strip.
/// The temporary row is stack-only; resident main-RAM cost is zero.
pub fn upload_gameplay_atlas() {
    let gw = glyph_w();
    let gh = glyph_h();
    let src_row_bytes = row_bytes();
    let src = bitmap();
    let mut row_pixels = [0u8; GAME_ROW_BYTES];
    let rows = glyph_count().div_ceil(GAME_COLS);

    let mut atlas_row = 0usize;
    while atlas_row < rows {
        row_pixels.fill(0);
        let mut atlas_col = 0usize;
        while atlas_col < GAME_COLS {
            let gi = atlas_row * GAME_COLS + atlas_col;
            if gi >= glyph_count() {
                break;
            }
            let base = gi * src_row_bytes * gh as usize;
            let cell_x = atlas_col * GAME_CELL_W;
            let mut row = 0u8;
            while row < gh {
                let mut col = 0u8;
                while col < gw {
                    let byte = src[base + row as usize * src_row_bytes + col as usize / 8];
                    let mask = 0x80u8 >> (col & 7);
                    if byte & mask == 0 {
                        col += 1;
                        continue;
                    }
                    let run_start = col;
                    while col < gw {
                        let byte = src[base + row as usize * src_row_bytes + col as usize / 8];
                        let mask = 0x80u8 >> (col & 7);
                        if byte & mask == 0 {
                            break;
                        }
                        col += 1;
                    }
                    let x0 = scale_i16(run_start as i16, SMALL_Q8).max(0) as usize;
                    let mut x1 = scale_i16(col as i16, SMALL_Q8).max(0) as usize;
                    let y0 = scale_i16(row as i16, SMALL_Q8).max(0) as usize;
                    let mut y1 = scale_i16(row as i16 + 1, SMALL_Q8).max(0) as usize;
                    if x1 <= x0 {
                        x1 = x0 + 1;
                    }
                    if y1 <= y0 {
                        y1 = y0 + 1;
                    }
                    let mut py = y0;
                    while py < y1.min(GAME_CELL_H) {
                        let mut px = x0;
                        while px < x1.min(GAME_CELL_W) {
                            set_4bpp_pixel(&mut row_pixels, GAME_ATLAS_W, cell_x + px, py);
                            px += 1;
                        }
                        py += 1;
                    }
                }
                row += 1;
            }
            atlas_col += 1;
        }
        upload_bytes(
            VramRect::new(
                GAME_TPAGE_X + GAME_ATLAS_U as u16 / 4,
                GAME_TPAGE_Y + (atlas_row * GAME_CELL_H) as u16,
                (GAME_ATLAS_W / 4) as u16,
                GAME_CELL_H as u16,
            ),
            &row_pixels,
        );
        atlas_row += 1;
    }

    // Index 0 remains transparent. Index 1 is exact mid-grey (RGB5=16):
    // normal PS1 texture modulation by an 8-bit tint `g` yields
    // floor(16*g/128) == g>>3, exactly matching flat-primitive colour
    // conversion while keeping even a black fade endpoint nontransparent.
    let mut clut = [0u16; 16];
    clut[1] = 0x4210;
    upload_16bpp(VramRect::new(GAME_CLUT_X, GAME_CLUT_Y, 16, 1), &clut);
}

/// Draw SMALL_Q8 text through the pre-scaled gameplay atlas. Positions,
/// advances and the pixel mask match [`draw_text_scaled`]; only packet count
/// changes (one quad per non-space character instead of one per bitmap run).
pub fn draw_text_gameplay(x: i16, y: i16, text: &str, color: (u8, u8, u8)) {
    let tp = Tpage::new(GAME_TPAGE_X, GAME_TPAGE_Y, TexDepth::Bit4);
    let cl = Clut::new(GAME_CLUT_X, GAME_CLUT_Y);
    let mat = TextureMaterial::opaque(cl.uv_clut_word(), tp.uv_tpage_word(0), color);
    let mut cx = x;
    for ch in text.chars() {
        if let Some(index) = glyph_index(ch) {
            if ch != ' ' {
                let u0 = GAME_ATLAS_U as usize + (index % GAME_COLS) * GAME_CELL_W;
                let v0 = (index / GAME_COLS) * GAME_CELL_H;
                // The PS1 textured-polygon top/left sampling rule begins a
                // 1:1 quad one pixel after the equivalent flat-run polygon.
                // Bias the vertices back so the atlas mask lands bit-for-bit
                // on draw_text_scaled's original screen pixels.
                let x0 = cx - 1;
                let y0 = y - 1;
                let x1 = x0 + GAME_CELL_W as i16;
                let y1 = y0 + GAME_CELL_H as i16;
                let u1 = u0 + GAME_CELL_W - 1;
                let v1 = v0 + GAME_CELL_H - 1;
                gpu::draw_quad_textured_material(
                    [(x0, y0), (x1, y0), (x0, y1), (x1, y1)],
                    [
                        (u0 as u8, v0 as u8),
                        (u1 as u8, v0 as u8),
                        (u0 as u8, v1 as u8),
                        (u1 as u8, v1 as u8),
                    ],
                    mat,
                );
            }
        }
        cx += scale_i16(glyph_advance(ch) as i16, SMALL_Q8);
    }
}

pub fn line_height_scaled(scale_q8: u16) -> i16 {
    scale_i16(glyph_h() as i16 + 2, scale_q8).max(1)
}

pub fn text_width_scaled(text: &str, scale_q8: u16) -> i16 {
    let mut width = 0i16;
    for ch in text.chars() {
        width += scale_i16(glyph_advance(ch) as i16, scale_q8);
    }
    width
}

pub fn draw_text_scaled(x: i16, y: i16, text: &str, scale_q8: u16, color: (u8, u8, u8)) {
    let gw = glyph_w();
    let gh = glyph_h();
    let row_bytes = row_bytes();
    let bitmap = bitmap();
    let mut cx = x;

    for ch in text.chars() {
        if let Some(index) = glyph_index(ch) {
            let base = index * row_bytes * gh as usize;
            let mut row = 0u8;
            while row < gh {
                let mut col = 0u8;
                while col < gw {
                    let byte = bitmap[base + row as usize * row_bytes + (col as usize / 8)];
                    let mask = 0x80u8 >> (col & 7);
                    if byte & mask == 0 {
                        col += 1;
                        continue;
                    }

                    let run_start = col;
                    while col < gw {
                        let byte = bitmap[base + row as usize * row_bytes + (col as usize / 8)];
                        let mask = 0x80u8 >> (col & 7);
                        if byte & mask == 0 {
                            break;
                        }
                        col += 1;
                    }

                    let x0 = cx + scale_i16(run_start as i16, scale_q8);
                    let mut x1 = cx + scale_i16(col as i16, scale_q8);
                    let y0 = y + scale_i16(row as i16, scale_q8);
                    let mut y1 = y + scale_i16(row as i16 + 1, scale_q8);
                    if x1 <= x0 {
                        x1 = x0 + 1;
                    }
                    if y1 <= y0 {
                        y1 = y0 + 1;
                    }
                    gpu::draw_quad_flat(
                        [(x0, y0), (x1, y0), (x0, y1), (x1, y1)],
                        color.0,
                        color.1,
                        color.2,
                    );
                }
                row += 1;
            }
        }
        cx += scale_i16(glyph_advance(ch) as i16, scale_q8);
    }
}

pub fn draw_centered_scaled(y: i16, text: &str, scale_q8: u16, color: (u8, u8, u8)) {
    draw_text_scaled(
        160 - text_width_scaled(text, scale_q8) / 2,
        y,
        text,
        scale_q8,
        color,
    );
}
