//! First-person HUD overlay, queued as OT primitives after the weapon pass. The
//! numbers and status icons are generated from the real Half-Life HUD sprites at
//! 75% source size by
//! tools/extract_menu.py into git-ignored data/menu/hud.tex (4bpp, amber baked
//! in, index 0 = transparent). Uploaded once to a free gameplay tpage.

use psx_gpu::material::TextureMaterial;
use psx_gpu::ot::OrderingTable;
use psx_gpu::prim::QuadTexturedMaterial;
use psx_vram::{upload_bytes, Clut, TexDepth, Tpage, VramRect};

const HUD_VRAM_X: u16 = 960; // band-1 last page; maps use far fewer pages, so it's free
pub const DRAW_CAP: usize = 20;
pub const EMPTY_QUAD: QuadTexturedMaterial = QuadTexturedMaterial::with_material(
    [(0, 0), (0, 0), (0, 0), (0, 0)],
    [(0, 0), (0, 0), (0, 0), (0, 0)],
    TextureMaterial::opaque(0, 0, (128, 128, 128)),
);

static HUD_BLOB: &[u8] = include_bytes!("../../data/menu/hud.tex");

// Source layout inside hud.tex: digits 0-9 at u=d*15 (15x18), icons on row 18.
const DW: u8 = 15;
const DH: u8 = 18;
const ICON_V: u8 = DH;
const SUIT_FULL_U: u8 = 0;
const SUIT_EMPTY_U: u8 = 30;
const SUIT_W: u8 = 30;
const SUIT_H: u8 = 30;
const HEALTH_U: u8 = 60;
const HEALTH_W: u8 = 24;
const HEALTH_H: u8 = 24;
const AMMO_U: u8 = 84;
const AMMO_W: u8 = 18;
const AMMO_H: u8 = 18;
const CROSSHAIR_U: u8 = 108;
const CROSSHAIR_W: u8 = 18;
const CROSSHAIR_H: u8 = 18;
const DIVIDER_U: u8 = 132;
const DIVIDER_W: u8 = 2;
const DIVIDER_H: u8 = 30;
const BATTERY_U: u8 = 136;
const BATTERY_W: u8 = 24;
const BATTERY_H: u8 = 24;

pub const PICKUP_NONE: u8 = 0;
pub const PICKUP_SUIT: u8 = 1;
pub const PICKUP_BATTERY: u8 = 2;

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
fn sprite<const N: usize>(
    mat: TextureMaterial,
    ot: &mut OrderingTable<N>,
    prims: &mut [QuadTexturedMaterial; DRAW_CAP],
    count: &mut usize,
    u: u8,
    v: u8,
    sw: u8,
    sh: u8,
    x: i16,
    y: i16,
    dw: i16,
    dh: i16,
) {
    if *count >= DRAW_CAP {
        return;
    }
    prims[*count] = QuadTexturedMaterial::with_material(
        [(x, y), (x + dw, y), (x, y + dh), (x + dw, y + dh)],
        [
            (u, v),
            (u + sw - 1, v),
            (u, v + sh - 1),
            (u + sw - 1, v + sh - 1),
        ],
        mat,
    );
    ot.add(0, &mut prims[*count], QuadTexturedMaterial::WORDS);
    *count += 1;
}

const GW: i16 = 15; // 75% Half-Life HUD digits, drawn 1:1
const GH: i16 = 18;
const GAP: i16 = 2;
const HEALTH_DRAW_W: i16 = 24;
const HEALTH_DRAW_H: i16 = 24;
const SUIT_DRAW_W: i16 = 30;
const SUIT_DRAW_H: i16 = 30;
const AMMO_DRAW_W: i16 = 18;
const AMMO_DRAW_H: i16 = 18;
const CROSSHAIR_DRAW_W: i16 = 18;
const CROSSHAIR_DRAW_H: i16 = 18;
const DIVIDER_DRAW_W: i16 = 2;
const DIVIDER_DRAW_H: i16 = 30;
const PICKUP_ANIM_MAX_TICKS: i16 = 36;

fn digit<const N: usize>(
    mat: TextureMaterial,
    ot: &mut OrderingTable<N>,
    prims: &mut [QuadTexturedMaterial; DRAW_CAP],
    count: &mut usize,
    d: u8,
    x: i16,
    y: i16,
) {
    sprite(mat, ot, prims, count, d * DW, 0, DW, DH, x, y, GW, GH);
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
fn number_l<const N: usize>(
    mat: TextureMaterial,
    ot: &mut OrderingTable<N>,
    prims: &mut [QuadTexturedMaterial; DRAW_CAP],
    count: &mut usize,
    n: u16,
    x: i16,
    y: i16,
) {
    let (buf, len) = digits(n);
    let mut cx = x;
    for i in (0..len).rev() {
        digit(mat, ot, prims, count, buf[i], cx, y);
        cx += GW + GAP;
    }
}

/// Right-aligned number ending at `right`.
fn number_r<const N: usize>(
    mat: TextureMaterial,
    ot: &mut OrderingTable<N>,
    prims: &mut [QuadTexturedMaterial; DRAW_CAP],
    count: &mut usize,
    n: u16,
    right: i16,
    y: i16,
) {
    let (buf, len) = digits(n);
    let mut cx = right - len as i16 * (GW + GAP) + GAP;
    for i in (0..len).rev() {
        digit(mat, ot, prims, count, buf[i], cx, y);
        cx += GW + GAP;
    }
}

/// Full HUD: crosshair, health, armor, and pistol ammo cluster.
pub fn draw<const N: usize>(
    mat: TextureMaterial,
    suit_equipped: bool,
    health: u16,
    armor: u16,
    clip_ammo: u16,
    reserve_ammo: u16,
    pickup_kind: u8,
    pickup_ticks: u8,
    ot: &mut OrderingTable<N>,
    prims: &mut [QuadTexturedMaterial; DRAW_CAP],
) -> usize {
    let mut count = 0usize;
    sprite(
        mat,
        ot,
        prims,
        &mut count,
        CROSSHAIR_U,
        ICON_V,
        CROSSHAIR_W,
        CROSSHAIR_H,
        160 - CROSSHAIR_DRAW_W / 2,
        120 - CROSSHAIR_DRAW_H / 2,
        CROSSHAIR_DRAW_W,
        CROSSHAIR_DRAW_H,
    ); // real pistol crosshair
    // HEV HUD: no health/armor/ammo readout until the suit is equipped (HL shows
    // no HUD before the suit). On suit pickup it boots up by sliding in from below
    // over the pickup window; the crosshair above is always drawn (the weapon
    // works without the suit).
    if suit_equipped {
        let slide = if pickup_kind == PICKUP_SUIT {
            pickup_ticks as i16
        } else {
            0
        };
        let y = 240 - GH - 8 + slide;
        let icon_y = y - 3;
        sprite(
            mat,
            ot,
            prims,
            &mut count,
            HEALTH_U,
            ICON_V,
            HEALTH_W,
            HEALTH_H,
            10,
            icon_y,
            HEALTH_DRAW_W,
            HEALTH_DRAW_H,
        ); // health cross icon
        number_l(mat, ot, prims, &mut count, health, 38, y);

        let suit_u =
            if armor > 0 || (suit_equipped && pickup_kind == PICKUP_SUIT && (pickup_ticks & 2) == 0) {
                SUIT_FULL_U
            } else {
                SUIT_EMPTY_U
            };
        sprite(
            mat,
            ot,
            prims,
            &mut count,
            suit_u,
            ICON_V,
            SUIT_W,
            SUIT_H,
            104,
            y - 6,
            SUIT_DRAW_W,
            SUIT_DRAW_H,
        ); // HEV armor/suit icon
        number_l(mat, ot, prims, &mut count, armor, 139, y);

        sprite(
            mat,
            ot,
            prims,
            &mut count,
            AMMO_U,
            ICON_V,
            AMMO_W,
            AMMO_H,
            221,
            y,
            AMMO_DRAW_W,
            AMMO_DRAW_H,
        ); // pistol ammo icon
        number_l(mat, ot, prims, &mut count, reserve_ammo, 243, y);
        sprite(
            mat,
            ot,
            prims,
            &mut count,
            DIVIDER_U,
            ICON_V,
            DIVIDER_W,
            DIVIDER_H,
            277,
            y - 6,
            DIVIDER_DRAW_W,
            DIVIDER_DRAW_H,
        );
        number_r(mat, ot, prims, &mut count, clip_ammo, 308, y);
    }

    if pickup_kind != PICKUP_NONE && pickup_ticks > 0 {
        let age = PICKUP_ANIM_MAX_TICKS - (pickup_ticks as i16).min(PICKUP_ANIM_MAX_TICKS);
        let size = (42 - age).max(26);
        let y = 132 - (age / 2);
        let x = 160 - size / 2;
        let (u, sw, sh) = if pickup_kind == PICKUP_BATTERY {
            (BATTERY_U, BATTERY_W, BATTERY_H)
        } else {
            (SUIT_FULL_U, SUIT_W, SUIT_H)
        };
        sprite(
            mat, ot, prims, &mut count, u, ICON_V, sw, sh, x, y, size, size,
        );
    }
    count
}
