//! Direct scaled text from the extracted Half-Life menu font. This is used for
//! modal overlays where uploading a second font atlas would trample gameplay
//! textures resident in VRAM.

use psx_gpu as gpu;

static HLFONT_BLOB: &[u8] = include_bytes!("../../data/menu/hlfont.bin");

pub const SMALL_Q8: u16 = 192; // 75%

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
