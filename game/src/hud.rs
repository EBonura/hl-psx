//! First-person HUD overlay: crosshair + health/ammo, drawn as immediate prims
//! on top of the world (after the OT submits). The numbers + health cross are
//! the REAL Half-Life HUD sprites (sprites/640hud7.spr), extracted by
//! tools/extract_menu.py into git-ignored data/menu/hud.tex (4bpp, amber baked
//! in, index 0 = transparent). Uploaded once to a free gameplay tpage; each
//! digit/icon is one textured quad sampling its sub-rect.

use psx_gpu::material::TextureMaterial;
use psx_gpu::{self as gpu};
use psx_vram::{upload_bytes, Clut, TexDepth, Tpage, VramRect};

const HUD_VRAM_X: u16 = 960; // band-1 last page; maps use far fewer pages, so it's free

static HUD_BLOB: &[u8] = include_bytes!("../../data/menu/hud.tex");

// Source layout inside hud.tex: digits 0-9 at u = d*20 (20x24), cross at (0,24,32,32).
const DW: u8 = 20;
const DH: u8 = 24;

/// Upload the HUD sprite sheet to a free gameplay tpage; returns its material.
pub fn upload() -> TextureMaterial {
    let w = u16::from_le_bytes([HUD_BLOB[0], HUD_BLOB[1]]);
    let h = u16::from_le_bytes([HUD_BLOB[2], HUD_BLOB[3]]);
    upload_bytes(VramRect::new(HUD_VRAM_X, 256, w / 4, h), &HUD_BLOB[36..]);
    upload_bytes(VramRect::new(HUD_VRAM_X, 504, 16, 1), &HUD_BLOB[4..36]);
    let tp = Tpage::new(HUD_VRAM_X, 256, TexDepth::Bit4);
    let cl = Clut::new(HUD_VRAM_X, 504);
    TextureMaterial::new(cl.uv_clut_word(), tp.uv_tpage_word(0))
}

/// Draw a sub-rect of the sheet (u,v,sw,sh source) scaled to (dw,dh) at (x,y).
fn sprite(mat: TextureMaterial, u: u8, v: u8, sw: u8, sh: u8, x: i16, y: i16, dw: i16, dh: i16) {
    gpu::draw_quad_textured_material(
        [(x, y), (x + dw, y), (x, y + dh), (x + dw, y + dh)],
        [
            (u, v),
            (u + sw - 1, v),
            (u, v + sh - 1),
            (u + sw - 1, v + sh - 1),
        ],
        mat,
    );
}

const GW: i16 = 16; // on-screen digit width
const GH: i16 = 19;
const GAP: i16 = 2;

fn digit(mat: TextureMaterial, d: u8, x: i16, y: i16) {
    sprite(mat, d * DW, 0, DW, DH, x, y, GW, GH);
}

fn digits(n: u16) -> ([u8; 5], usize) {
    let mut buf = [0u8; 5];
    let mut len = 0;
    let mut v = n;
    loop {
        buf[len] = (v % 10) as u8;
        v /= 10;
        len += 1;
        if v == 0 {
            break;
        }
    }
    (buf, len)
}

/// Left-aligned number at (x, y).
fn number_l(mat: TextureMaterial, n: u16, x: i16, y: i16) {
    let (buf, len) = digits(n);
    let mut cx = x;
    for i in (0..len).rev() {
        digit(mat, buf[i], cx, y);
        cx += GW + GAP;
    }
}

/// Right-aligned number ending at `right`.
fn number_r(mat: TextureMaterial, n: u16, right: i16, y: i16) {
    let (buf, len) = digits(n);
    let mut cx = right - len as i16 * (GW + GAP) + GAP;
    for i in (0..len).rev() {
        digit(mat, buf[i], cx, y);
        cx += GW + GAP;
    }
}

/// Full HUD: crosshair + health (cross icon + number, bottom-left) + ammo (bottom-right).
pub fn draw(mat: TextureMaterial, health: u16, ammo: u16) {
    sprite(mat, 40, DH, 24, 24, 160 - 11, 120 - 11, 22, 22); // real pistol crosshair
    let y = 240 - GH - 8;
    sprite(mat, 0, DH, 32, 32, 10, y - 4, 26, 26); // health cross icon
    number_l(mat, health, 42, y);
    number_r(mat, ammo, 308, y);
}
