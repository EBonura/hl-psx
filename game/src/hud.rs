//! GoldSrc-compatible first-person HUD, queued after the world/viewmodel pass.
//!
//! `host/hl-content` copies the native 320x240 Half-Life HUD rectangles into
//! `hud.tex` without resampling.  Runtime coordinates below follow the original
//! client DLL (`health.cpp`, `battery.cpp`, and `ammo.cpp`) so the 12x16 number
//! face, fixed three-digit fields, separators, ammo icons, and crosshairs land
//! on the same pixels as the PC game at 320x240.
//!
//! Layout follows the original SDK client: every bottom element, numbers
//! included, sits at y = ScreenHeight - 1.5 * FontHeight (216) and the suit
//! at ScreenWidth / 5. The 25th-anniversary SDK later lowered the numbers by
//! FontHeight / 5 and moved the suit to 3 * width; that is not what the
//! shipped game did, and the shared Counter-Strike client never had it.

use psx_gpu::material::{BlendMode, TextureMaterial};
use psx_gpu::ot::OrderingTable;
use crate::{max_reserve_for, Arsenal, AMMO_ARGREN, AMMO_NONE, WEAPON_DEFS};
use psx_gpu::prim::{RectFlat, Sprite};
use psx_vram::{upload_bytes, Clut, TextureDepth, TexturePage, VramRect};

const HUD_VRAM_X: u16 = 960; // fixed final band-1 page, excluded from room allocation
const HUD_AMBER_CLUT_X: u16 = 960;
const HUD_WHITE_CLUT_X: u16 = 992;
const HUD_CLUT_Y: u16 = 504;
const HUD_HEADER_BYTES: usize = 4 + 32 + 32;
const HUD_LEFT_W: usize = 160;
const HUD_LEFT_H: usize = 248;
const HUD_RIGHT_U: usize = 160;
const HUD_RIGHT_V: usize = 128;
const HUD_RIGHT_W: usize = 96;
const HUD_RIGHT_H: usize = 112;
const HUD_LEFT_BYTES: usize = HUD_LEFT_W * HUD_LEFT_H / 2;
const HUD_RIGHT_BYTES: usize = HUD_RIGHT_W * HUD_RIGHT_H / 2;
const HUD_BLOB_BYTES: usize = HUD_HEADER_BYTES + HUD_LEFT_BYTES + HUD_RIGHT_BYTES;

/// The native HUD's busiest possible frame is MP5 + secondary ammo + selection
/// strip + item history + active flashlight + three damage icons. Each packet explicitly restores
/// TextureWindow::NONE: world materials can leave a window active before the
/// HUD OT is submitted.
pub const DRAW_CAP: usize = 47;
/// Flat rectangles of the weapon list: a box for each weapon of the closed buckets (at most
/// thirteen) and the two halves of each ammo bar of the open one (at most eight).
pub const RECT_CAP: usize = 20;
pub const EMPTY_RECT: RectFlat = RectFlat::new(0, 0, 0, 0, 0, 0, 0);
pub const EMPTY_SPRITE: Sprite = Sprite::with_material(
    0,
    0,
    0,
    0,
    (0, 0),
    TextureMaterial::opaque(0, 0, (128, 128, 128)),
);

/// HUD atlas streamed once per map from WORLD.PAK. It remains in VRAM only;
/// the decompressed source is overwritten by the world stream.
pub const HUD_CHUNK_ID: u32 = 3001;

#[derive(Copy, Clone)]
pub struct Materials {
    amber: TextureMaterial,
    white: TextureMaterial,
}

// Native 320-res atlas layout. Keep in sync with host/hl-content::build_hud.
const DW: u8 = 12;
const DH: u8 = 16;
const SELECTION_U: u8 = 0;
const SELECTION_V: u8 = 16;
const SELECTION_W: u8 = 80;
const SELECTION_H: u8 = 20;
const SUIT_FULL_U: u8 = 80;
const SUIT_EMPTY_U: u8 = 100;
const SUIT_V: u8 = 16;
const SUIT_W: u8 = 20;
const SUIT_H: u8 = 20;
const SUIT_X: i16 = 320 / 5; // battery.cpp: x = ScreenWidth / 5, number after the icon width
const HEALTH_U: u8 = 120;
const HEALTH_V: u8 = 16;
const HEALTH_W: u8 = 16;
const HEALTH_H: u8 = 16;
const DIVIDER_U: u8 = 136;
const DIVIDER_V: u8 = 16;
const BATTERY_U: u8 = 138;
const BATTERY_V: u8 = 16;
const BATTERY_W: u8 = 20;
const BATTERY_H: u8 = 20;
const BUCKET_V: u8 = 36;
const BUCKET_W: u8 = 12;
const BUCKET_H: u8 = 12;
const AMMO_V: u8 = 48;
const AMMO_W: u8 = 18;
const AMMO_H: u8 = 18;
const MP5_SECONDARY_U: u8 = 126;
const CROSS_V: u8 = 84;
const CROSS_W: u8 = 24;
const CROSS_H: u8 = 24;
const HEALTHKIT_U: u8 = 72;
const LONGJUMP_U: u8 = 92;
const ITEM_V: u8 = 108;
const FLASH_FULL_U: u8 = 112;
const FLASH_EMPTY_U: u8 = 130;
const FLASH_BEAM_U: u8 = 148;
const FLASH_V: u8 = 108;
const FLASH_W: u8 = 18;
const FLASH_H: u8 = 16;
const FLASH_BEAM_W: u8 = 6;
const ZOOM_U: u8 = 0;
const ZOOM_V: u8 = 132;
const ZOOM_W: u8 = 104;
const ZOOM_H: u8 = 16;
const WEAPON_LEFT_V: u8 = 148;
const WEAPON_RIGHT_U: u8 = 160;
const WEAPON_RIGHT_V: u8 = 128;
const WEAPON_W: u8 = 80;
const WEAPON_H: u8 = 20;
const TRAIN_U: u8 = 240;
const TRAIN_V: u8 = 128;
const TRAIN_W: u8 = 16;
const TRAIN_H: u8 = 16;
// Damage-type icons (CHudHealth::DrawDamage): 32x32, radiation, drowning, acid along v=208.
const DMG_V: u8 = 208;
const DMG_SIZE: u8 = 32;
const DMG_SLOTS: usize = 3;
/// DMG_IMAGE_LIFE: an icon stays two seconds (40 ticks at 20 Hz) from its last hit.
const DMG_LIFE: u16 = 40;

const W_MP5: usize = 3;
const W_CROSSBOW: usize = 5;
const W_GRENADE: usize = 10;
const N_WEAPONS: usize = 14;

// Packed crosshair cell for each runtime weapon. GoldSrc defines no crosshair
// for melee or throwables; those entries deliberately remain absent.
const CROSS_INDEX: [i8; N_WEAPONS] = [-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, -1, -1, -1, -1];
// GoldSrc weapon buckets (zero based): melee, pistols, long guns, energy/heavy,
// and carried explosives.
const WEAPON_BUCKET: [u8; N_WEAPONS] = [0, 1, 1, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4];

pub const PICKUP_BATTERY: u8 = 2;
pub const PICKUP_HEALTHKIT: u8 = 3;
pub const PICKUP_LONGJUMP: u8 = 4;

static mut PREV_HEALTH: u16 = u16::MAX;
static mut PREV_ARMOR: u16 = u16::MAX;
static mut PREV_CLIP: u16 = u16::MAX;
static mut PREV_RESERVE: u16 = u16::MAX;
static mut PREV_WEAPON: i16 = -2;
static mut HEALTH_FADE: u8 = 0;
static mut ARMOR_FADE: u8 = 0;
static mut AMMO_FADE: u8 = 0;

/// Upload the sparse atlas and its amber/white additive CLUTs. The serialized
/// chunk omits u=160..255/v=0..127 (owned by gameplay text) and the unused
/// bottom-right tail. Avoiding those transparent pixels keeps this chunk below
/// the largest room stream, so improving the HUD cannot grow MAP_BUF.
pub fn upload(blob: &[u8]) -> Materials {
    if blob.len() >= HUD_BLOB_BYTES && &blob[..4] == b"HUD2" {
        upload_bytes(
            VramRect::new(HUD_VRAM_X, 256, (HUD_LEFT_W / 4) as u16, HUD_LEFT_H as u16),
            &blob[HUD_HEADER_BYTES..HUD_HEADER_BYTES + HUD_LEFT_BYTES],
        );
        upload_bytes(
            VramRect::new(
                HUD_VRAM_X + (HUD_RIGHT_U / 4) as u16,
                256 + HUD_RIGHT_V as u16,
                (HUD_RIGHT_W / 4) as u16,
                HUD_RIGHT_H as u16,
            ),
            &blob[HUD_HEADER_BYTES + HUD_LEFT_BYTES..HUD_BLOB_BYTES],
        );
        upload_bytes(
            VramRect::new(HUD_AMBER_CLUT_X, HUD_CLUT_Y, 16, 1),
            &blob[4..36],
        );
        upload_bytes(
            VramRect::new(HUD_WHITE_CLUT_X, HUD_CLUT_Y, 16, 1),
            &blob[36..68],
        );
    }
    let tp = TexturePage::new(HUD_VRAM_X, 256, TextureDepth::Bit4);
    let tpage = tp.uv_word(BlendMode::Add.texture_page_bits());
    Materials {
        amber: TextureMaterial::blended(
            Clut::new(HUD_AMBER_CLUT_X, HUD_CLUT_Y).uv_word(),
            tpage,
            (128, 128, 128),
            BlendMode::Add,
        ),
        white: TextureMaterial::blended(
            Clut::new(HUD_WHITE_CLUT_X, HUD_CLUT_Y).uv_word(),
            tpage,
            (128, 128, 128),
            BlendMode::Add,
        ),
    }
}

/// GP0 environment words (E1 draw mode, E2 texture window) carried as an
/// ordering-table packet instead of two immediate `write_gp0` stores.
///
/// `write_gp0` first spins on GPUSTAT "ready for command", which the GPU only
/// asserts once its whole raster backlog has drained. Issuing this state right
/// after the world/viewmodel lists therefore fenced the CPU against the entire
/// frame's rasterisation. As the head packet of the HUD chain it reaches GP0
/// through the same DMA walk, in the same position in draw order.
#[repr(C)]
pub struct GpuEnv {
    tag: u32,
    draw_mode: u32,
    tex_window: u32,
}

impl GpuEnv {
    /// Data-word count (the tag is not a data word).
    pub const WORDS: u8 = 2;

    pub const fn new() -> Self {
        Self {
            tag: 0,
            draw_mode: 0,
            tex_window: 0,
        }
    }
}

impl Default for GpuEnv {
    fn default() -> Self {
        Self::new()
    }
}

/// Restore the two pieces of global GPU state used by rectangle sprites once,
/// after the world and viewmodel have finished. Paying E1/E2 once per HUD is
/// substantially smaller in RAM and DMA traffic than embedding E2 in every
/// quad, while still preventing world texture windows from corrupting the HUD.
///
/// Call after every HUD packet has been queued: ordering-table insertion
/// prepends, so the last packet added to a slot is the first one drawn.
///
/// # Safety
///
/// `env` stays linked into `ot`: it must stay live and unmodified until
/// that table's walk has finished.
#[inline]
pub unsafe fn prepare<const N: usize>(
    mats: Materials,
    ot: &mut OrderingTable<N>,
    env: &mut GpuEnv,
) {
    // `TextureWindow::NONE.apply()` used to precede this; the material's own
    // E2 immediately overwrote it, so the pair reduces to these two words.
    env.draw_mode = mats.amber.draw_mode_word();
    env.tex_window = mats.amber.texture_window_word();
    ot.resume_frame()
        .add_raw(0, core::ptr::from_mut(env).cast(), GpuEnv::WORDS);
}

/// Link one sprite from `prims` into `ot`.
///
/// # Safety
///
/// `prims` stays linked into `ot`: it must stay live and unmodified until
/// that table's walk has finished.
#[allow(clippy::too_many_arguments)]
unsafe fn sprite<const N: usize>(
    mat: TextureMaterial,
    ot: &mut OrderingTable<N>,
    prims: &mut [Sprite; DRAW_CAP],
    count: &mut usize,
    u: u8,
    v: u8,
    w: u8,
    h: u8,
    x: i16,
    y: i16,
) {
    if *count >= DRAW_CAP || w == 0 || h == 0 {
        return;
    }
    prims[*count] = Sprite::with_material(x, y, w as u16, h as u16, (u, v), mat);
    ot.resume_frame().add_raw(
        0,
        core::ptr::from_mut(&mut prims[*count]).cast(),
        Sprite::WORDS,
    );
    *count += 1;
}

#[inline(always)]
fn alpha_mat(mat: TextureMaterial, alpha: u8) -> TextureMaterial {
    // PS1 texture modulation is texel*tint/128. The CLUT contains GoldSrc's
    // full RGB_YELLOWISH (255,160,0), so alpha/2 reproduces ScaleColors().
    let tint = ((alpha as u16 + 1) / 2) as u8;
    mat.with_tint((tint, tint, tint))
}

unsafe fn digit<const N: usize>(
    mat: TextureMaterial,
    ot: &mut OrderingTable<N>,
    prims: &mut [Sprite; DRAW_CAP],
    count: &mut usize,
    value: u8,
    x: i16,
    y: i16,
) {
    sprite(mat, ot, prims, count, value * DW, 0, DW, DH, x, y);
}

/// GoldSrc DrawHudNumber(DHN_3DIGITS | DHN_DRAWZERO): reserve all three slots,
/// omit leading zero sprites, and render a single zero in the ones slot.
unsafe fn number_3<const N: usize>(
    mat: TextureMaterial,
    ot: &mut OrderingTable<N>,
    prims: &mut [Sprite; DRAW_CAP],
    count: &mut usize,
    value: u16,
    x: i16,
    y: i16,
) -> i16 {
    let value = value.min(999);
    if value >= 100 {
        digit(mat, ot, prims, count, (value / 100) as u8, x, y);
    }
    if value >= 10 {
        digit(
            mat,
            ot,
            prims,
            count,
            ((value % 100) / 10) as u8,
            x + DW as i16,
            y,
        );
    }
    digit(
        mat,
        ot,
        prims,
        count,
        (value % 10) as u8,
        x + 2 * DW as i16,
        y,
    );
    x + 3 * DW as i16
}

#[inline]
fn weapon_icon_uv(weapon: usize) -> (u8, u8) {
    if weapon < W_GRENADE {
        (
            ((weapon & 1) * WEAPON_W as usize) as u8,
            WEAPON_LEFT_V + (weapon / 2) as u8 * WEAPON_H,
        )
    } else {
        (
            WEAPON_RIGHT_U,
            WEAPON_RIGHT_V + (weapon - W_GRENADE) as u8 * WEAPON_H,
        )
    }
}

/// `WeaponsResource::HasAmmo`: melee has no ammo type, anything else needs a
/// round in the magazine or the pool (the MP5's grenades count).
fn has_ammo(a: &Arsenal, w: usize) -> bool {
    let d = &WEAPON_DEFS[w];
    d.ammo == AMMO_NONE
        || a.clip[w] > 0
        || a.ammo[d.ammo] > 0
        || (w == W_MP5 && a.ammo[AMMO_ARGREN] > 0)
}

/// One flat additive box of the weapon list (`FillRGBA`).
unsafe fn flat<const N: usize>(
    ot: &mut OrderingTable<N>,
    rects: &mut [RectFlat; RECT_CAP],
    count: &mut usize,
    x: i16,
    y: i16,
    w: i16,
    h: u16,
    rgb: (u8, u8, u8),
) {
    if *count >= RECT_CAP || w <= 0 {
        return;
    }
    rects[*count] = RectFlat::new(x, y, w as u16, h, rgb.0, rgb.1, rgb.2);
    rects[*count].color_cmd |= 0x0200_0000; // semi-transparent: added to the frame
    ot.resume_frame()
        .add_raw(0, core::ptr::from_mut(&mut rects[*count]).cast(), RectFlat::WORDS);
    *count += 1;
}

/// `DrawAmmoBar` for one pool: a bright part for what is carried against the
/// weapon's cap, the rest of the 10 pixels dim. Nothing when the pool is empty.
unsafe fn ammo_bar<const N: usize>(
    a: &Arsenal,
    pool: usize,
    x: i16,
    y: i16,
    ot: &mut OrderingTable<N>,
    rects: &mut [RectFlat; RECT_CAP],
    count: &mut usize,
) -> i16 {
    let have = a.ammo[pool] as u32;
    if have == 0 {
        return x;
    }
    let fill = (have * 10 / max_reserve_for(pool).max(1) as u32).clamp(1, 10) as i16;
    flat(ot, rects, count, x, y, fill, 2, (0, 160, 0));
    flat(ot, rects, count, x + fill, y, 10 - fill, 2, (128, 80, 0));
    x + 10
}

/// CHudAmmo::DrawWList: the bucket numbers along the top; under the bucket the
/// selection is in, every weapon owned there (the selected one in full with its
/// frame, the others dimmer, red when empty) with its ammo bar; under the
/// others a box per weapon. Sprites are queued back to front: the frame last
/// drawn first, so it lands on the icon.
#[inline(never)]
#[optimize(size)]
unsafe fn draw_weapon_selection<const N: usize>(
    mat: TextureMaterial,
    weapon: usize,
    a: &Arsenal,
    ot: &mut OrderingTable<N>,
    prims: &mut [Sprite; DRAW_CAP],
    count: &mut usize,
    rects: &mut [RectFlat; RECT_CAP],
) {
    let active = WEAPON_BUCKET[weapon] as usize;
    let full = alpha_mat(mat, 255);
    let mut nrect = 0usize;
    let mut x = 10i16;
    for bucket in 0..5usize {
        sprite(
            full,
            ot,
            prims,
            count,
            bucket as u8 * BUCKET_W,
            BUCKET_V,
            BUCKET_W,
            BUCKET_H,
            x,
            10,
        );
        let mut y = 22i16;
        let mut w = 0usize;
        while w < N_WEAPONS {
            if WEAPON_BUCKET[w] as usize == bucket && a.owns(w) {
                let have = has_ammo(a, w);
                if bucket == active {
                    let (u, v) = weapon_icon_uv(w);
                    let tint = if have {
                        alpha_mat(mat, 192)
                    } else {
                        mat.with_tint((64, 0, 0))
                    };
                    if w == weapon {
                        sprite(full, ot, prims, count, SELECTION_U, SELECTION_V, SELECTION_W, SELECTION_H, x, y);
                        sprite(full, ot, prims, count, u, v, WEAPON_W, WEAPON_H, x, y);
                    } else {
                        sprite(tint, ot, prims, count, u, v, WEAPON_W, WEAPON_H, x, y);
                    }
                    let d = &WEAPON_DEFS[w];
                    if d.ammo != AMMO_NONE {
                        let bx = ammo_bar(a, d.ammo, x + 5, y, ot, rects, &mut nrect);
                        if w == W_MP5 && bx != x + 5 {
                            ammo_bar(a, AMMO_ARGREN, bx + 5, y, ot, rects, &mut nrect);
                        }
                    }
                    y += WEAPON_H as i16 + 5;
                } else {
                    let rgb = if have { (128, 80, 0) } else { (96, 6, 6) };
                    flat(ot, rects, &mut nrect, x, y, BUCKET_W as i16, BUCKET_H as u16, rgb);
                    y += BUCKET_H as i16 + 5;
                }
            }
            w += 1;
        }
        x += if bucket == active {
            WEAPON_W as i16 + 5
        } else {
            BUCKET_W as i16 + 5
        };
    }
}

/// # Safety
///
/// The sprites in `prims` stay linked into `ot`: they must stay live and
/// unmodified until that table's walk has finished.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub unsafe fn draw<const N: usize>(
    mats: Materials,
    suit_equipped: bool,
    flashlight_on: bool,
    flashlight_battery: u8,
    has_weapon: bool,
    zoomed: bool,
    health: u16,
    armor: u16,
    clip_ammo: u16,
    reserve_ammo: u16,
    secondary_ammo: u16,
    ammo_mode: u8, // 0 = melee, 1 = reserve-only, 2 = clip + reserve
    weapon_id: i32,
    selection_ticks: u8,
    train_position: u8,
    arsenal: &Arsenal,
    ot: &mut OrderingTable<N>,
    prims: &mut [Sprite; DRAW_CAP],
    rects: &mut [RectFlat; RECT_CAP],
) -> usize {
    let mut count = 0usize;
    let weapon = (weapon_id >= 0).then_some((weapon_id as usize).min(N_WEAPONS - 1));

    if has_weapon {
        if let Some(w) = weapon {
            if zoomed && w == W_CROSSBOW {
                sprite(
                    mats.white,
                    ot,
                    prims,
                    &mut count,
                    ZOOM_U,
                    ZOOM_V,
                    ZOOM_W,
                    ZOOM_H,
                    160 - ZOOM_W as i16 / 2,
                    120 - ZOOM_H as i16 / 2,
                );
            } else {
                let ci = CROSS_INDEX[w];
                if ci >= 0 {
                    let ci = ci as u8;
                    sprite(
                        mats.white,
                        ot,
                        prims,
                        &mut count,
                        (ci % 6) * CROSS_W,
                        CROSS_V + (ci / 6) * CROSS_H,
                        CROSS_W,
                        CROSS_H,
                        160 - CROSS_W as i16 / 2,
                        120 - CROSS_H as i16 / 2,
                    );
                }
            }
            if selection_ticks > 0 {
                draw_weapon_selection(mats.amber, w, arsenal, ot, prims, &mut count, rects);
            }
        }
    }

    if (1..=5).contains(&train_position) {
        // CHudTrain: right of the health/armor cluster and one font-height up.
        sprite(
            mats.amber,
            ot,
            prims,
            &mut count,
            TRAIN_U,
            TRAIN_V + (train_position - 1) * TRAIN_H,
            TRAIN_W,
            TRAIN_H,
            320 / 3 + TRAIN_W as i16 / 4,
            240 - TRAIN_H as i16 - DH as i16,
        );
    }

    draw_damage_icons(mats.amber, ot, prims, &mut count);

    if suit_equipped {
        // GoldSrc fades changed values from a bright flash back to MIN_ALPHA=100.
        let (health_alpha, armor_alpha, ammo_alpha) = unsafe {
            if PREV_HEALTH == u16::MAX {
                PREV_HEALTH = health;
            } else if PREV_HEALTH != health {
                PREV_HEALTH = health;
                HEALTH_FADE = 100;
            }
            if PREV_ARMOR == u16::MAX {
                PREV_ARMOR = armor;
            } else if PREV_ARMOR != armor {
                PREV_ARMOR = armor;
                ARMOR_FADE = 100;
            }
            let weapon_now = weapon_id as i16;
            if PREV_CLIP == u16::MAX {
                PREV_CLIP = clip_ammo;
                PREV_RESERVE = reserve_ammo;
                PREV_WEAPON = weapon_now;
            } else if PREV_CLIP != clip_ammo
                || PREV_RESERVE != reserve_ammo
                || PREV_WEAPON != weapon_now
            {
                PREV_CLIP = clip_ammo;
                PREV_RESERVE = reserve_ammo;
                PREV_WEAPON = weapon_now;
                AMMO_FADE = 200;
            }
            let ha = if health <= 15 {
                255
            } else {
                100 + (HEALTH_FADE as u16 * 128 / 100) as u8
            };
            let ba = 100 + (ARMOR_FADE as u16 * 128 / 100) as u8;
            let aa = AMMO_FADE.max(100);
            HEALTH_FADE = HEALTH_FADE.saturating_sub(1);
            ARMOR_FADE = ARMOR_FADE.saturating_sub(1);
            AMMO_FADE = AMMO_FADE.saturating_sub(1);
            (ha, ba, aa)
        };

        let health_mat = if health <= 25 {
            let red = ((health_alpha as u16 + 1) / 2) as u8;
            mats.amber.with_tint((red, 0, 0))
        } else {
            alpha_mat(mats.amber, health_alpha)
        };
        let armor_mat = alpha_mat(mats.amber, armor_alpha);
        let ammo_mat = alpha_mat(mats.amber, ammo_alpha);
        let base_y = 240 - DH as i16 - DH as i16 / 2; // 216
        let number_y = base_y; // numbers share the icon baseline

        // CHudHealth::Draw: cross at x=CrossWidth/2, then a fixed 3-digit field.
        sprite(
            health_mat,
            ot,
            prims,
            &mut count,
            HEALTH_U,
            HEALTH_V,
            HEALTH_W,
            HEALTH_H,
            HEALTH_W as i16 / 2,
            base_y,
        );
        let hx = number_3(health_mat, ot, prims, &mut count, health, 22, number_y);
        sprite(
            health_mat,
            ot,
            prims,
            &mut count,
            DIVIDER_U,
            DIVIDER_V,
            1,
            DH,
            hx + DW as i16 / 2,
            number_y,
        );

        // CHudBattery::Draw: empty outline plus the lower, vertically clipped
        // portion of suit_full. This preserves GoldSrc's continuous 0..100 fill.
        let suit_y = base_y - SUIT_H as i16 / 6;
        sprite(
            armor_mat,
            ot,
            prims,
            &mut count,
            SUIT_EMPTY_U,
            SUIT_V,
            SUIT_W,
            SUIT_H,
            SUIT_X,
            suit_y,
        );
        if armor > 0 {
            let cut = ((SUIT_H as u16 * (100 - armor.min(100))) / 100) as u8;
            sprite(
                armor_mat,
                ot,
                prims,
                &mut count,
                SUIT_FULL_U,
                SUIT_V + cut,
                SUIT_W,
                SUIT_H - cut,
                SUIT_X,
                suit_y + cut as i16,
            );
        }
        number_3(
            armor_mat,
            ot,
            prims,
            &mut count,
            armor,
            SUIT_X + SUIT_W as i16,
            number_y,
        );

        if ammo_mode != 0 {
            if let Some(w) = weapon {
                let ammo_u = (w % 7) as u8 * AMMO_W;
                let ammo_v = AMMO_V + (w / 7) as u8 * AMMO_H;
                let icon_y = number_y - AMMO_H as i16 / 8;
                if ammo_mode == 2 {
                    let x = 320 - 8 * DW as i16 - AMMO_W as i16; // ammo.cpp
                    let end_clip =
                        number_3(ammo_mat, ot, prims, &mut count, clip_ammo, x, number_y);
                    let bar_x = end_clip + DW as i16 / 2;
                    sprite(
                        ammo_mat, ot, prims, &mut count, DIVIDER_U, DIVIDER_V, 1, DH, bar_x,
                        number_y,
                    );
                    let reserve_x = bar_x + 1 + DW as i16 / 2;
                    let icon_x = number_3(
                        ammo_mat,
                        ot,
                        prims,
                        &mut count,
                        reserve_ammo,
                        reserve_x,
                        number_y,
                    );
                    sprite(
                        ammo_mat, ot, prims, &mut count, ammo_u, ammo_v, AMMO_W, AMMO_H, icon_x,
                        icon_y,
                    );
                } else {
                    let x = 320 - 4 * DW as i16 - AMMO_W as i16;
                    let icon_x =
                        number_3(ammo_mat, ot, prims, &mut count, reserve_ammo, x, number_y);
                    sprite(
                        ammo_mat, ot, prims, &mut count, ammo_u, ammo_v, AMMO_W, AMMO_H, icon_x,
                        icon_y,
                    );
                }

                // The MP5's M203 pool is the one secondary-ammo HUD line in HL.
                if w == W_MP5 && secondary_ammo > 0 {
                    let sy = number_y - DH as i16 - DH as i16 / 4;
                    let x = 320 - 4 * DW as i16 - AMMO_W as i16;
                    let icon_x = number_3(ammo_mat, ot, prims, &mut count, secondary_ammo, x, sy);
                    sprite(
                        ammo_mat,
                        ot,
                        prims,
                        &mut count,
                        MP5_SECONDARY_U,
                        AMMO_V,
                        AMMO_W,
                        AMMO_H,
                        icon_x,
                        sy - AMMO_H as i16 / 8,
                    );
                }
            }
        }

        // CHudFlashlight::Draw: exact SDK alpha, red below 20%, and a clipped
        // full sprite over the empty casing for the remaining 0..100 charge.
        let flash_alpha = if flashlight_on { 225 } else { 100 };
        let flash_mat = if flashlight_battery < 20 {
            let tint = ((flash_alpha as u16 + 1) / 2) as u8;
            mats.amber.with_tint((tint, 0, 0))
        } else {
            alpha_mat(mats.amber, flash_alpha)
        };
        let flash_x = 320 - FLASH_W as i16 - FLASH_W as i16 / 2;
        let flash_y = FLASH_H as i16 / 2;
        sprite(
            flash_mat,
            ot,
            prims,
            &mut count,
            FLASH_EMPTY_U,
            FLASH_V,
            FLASH_W,
            FLASH_H,
            flash_x,
            flash_y,
        );
        if flashlight_on {
            sprite(
                flash_mat,
                ot,
                prims,
                &mut count,
                FLASH_BEAM_U,
                FLASH_V,
                FLASH_BEAM_W,
                FLASH_H,
                320 - FLASH_W as i16 / 2,
                flash_y,
            );
        }
        let flash_offset =
            (FLASH_W as u16 * (100u16 - flashlight_battery.min(100) as u16) / 100) as u8;
        if flash_offset < FLASH_W {
            sprite(
                flash_mat,
                ot,
                prims,
                &mut count,
                FLASH_FULL_U + flash_offset,
                FLASH_V,
                FLASH_W - flash_offset,
                FLASH_H,
                flash_x + flash_offset as i16,
                flash_y,
            );
        }
    }

    draw_history(mats.amber, arsenal, ot, prims, &mut count);

    count
}

// ---- damage-type icons (CHudHealth::DrawDamage) ----

/// Icon cell of a damage kind, `None` for the kinds with no icon here.
fn damage_cell(kind: crate::hev::Kind) -> Option<u8> {
    match kind {
        crate::hev::Kind::Radiation => Some(0),
        crate::hev::Kind::Drown => Some(1),
        crate::hev::Kind::Acid => Some(2),
        _ => None,
    }
}

/// Active icons, oldest first: the cell and the tick the icon may leave at.
static mut DMG_ICONS: [(u8, u16); DMG_SLOTS] = [(0, 0); DMG_SLOTS];
static mut DMG_COUNT: usize = 0;

/// A hit of `kind` landed: show its icon, or keep it two seconds longer.
pub fn damage_kind(kind: crate::hev::Kind) {
    let Some(cell) = damage_cell(kind) else {
        return;
    };
    // SAFETY: the HUD state is only touched from the main loop.
    unsafe {
        let until = crate::SIM_NOW.wrapping_add(DMG_LIFE);
        let mut i = 0;
        while i < DMG_COUNT {
            if DMG_ICONS[i].0 == cell {
                DMG_ICONS[i].1 = until;
                return;
            }
            i += 1;
        }
        if DMG_COUNT < DMG_SLOTS {
            DMG_ICONS[DMG_COUNT] = (cell, until);
            DMG_COUNT += 1;
        }
    }
}

/// `fabs(sin(time * 2)) * 256`: a parabola over one 31-tick hump (the sine's
/// 1.57 s at 20 Hz), within a tenth of the sine.
fn damage_flash(now: u16) -> u16 {
    let x = (now % 31) as u32 * 256 / 31;
    ((x * (256 - x)) / 64).min(255) as u16
}

/// CHudHealth::DrawDamage's icons, out of line like the history.
#[inline(never)]
#[optimize(size)]
unsafe fn draw_damage_icons<const N: usize>(
    amber: TextureMaterial,
    ot: &mut OrderingTable<N>,
    prims: &mut [Sprite; DRAW_CAP],
    count: &mut usize,
) {
    // CHudHealth::DrawDamage: the active icons flash in the health colour from the lower left,
    // the newest lowest, and leave at the dark end of a flash once their time is up.
    {
        let now = unsafe { crate::SIM_NOW };
        let a = damage_flash(now);
        let mut i = 0usize;
        while i < unsafe { DMG_COUNT } {
            let (cell, until) = unsafe { DMG_ICONS[i] };
            if now.wrapping_sub(until) < 0x8000 && a < 40 {
                let mut j = i + 1;
                while j < unsafe { DMG_COUNT } {
                    unsafe { DMG_ICONS[j - 1] = DMG_ICONS[j] };
                    j += 1;
                }
                unsafe { DMG_COUNT -= 1 };
                continue;
            }
            let rank = unsafe { DMG_COUNT } - 1 - i;
            sprite(
                alpha_mat(amber, a as u8),
                ot,
                prims,
                count,
                160 + cell * DMG_SIZE,
                DMG_V,
                DMG_SIZE,
                DMG_SIZE,
                DMG_SIZE as i16 / 8,
                240 - 2 * DMG_SIZE as i16 - rank as i16 * DMG_SIZE as i16,
            );
            i += 1;
        }
    }
}

// ---- pickup history (CHudAmmoHistory) ----
const HIST_SLOTS: usize = 4;
const HIST_NONE: u8 = 0;
/// A weapon came into the arsenal (`id` = its weapon number).
pub const HIST_WEAPON: u8 = 1;
/// Rounds were taken (`id` = the ammo pool, `count` = how many).
pub const HIST_AMMO: u8 = 2;
/// A carried item (`id` = one of the `PICKUP_*` kinds).
pub const HIST_ITEM: u8 = 3;
/// Five seconds at the 20 Hz simulation.
const HIST_TICKS: u16 = 100;

#[derive(Copy, Clone)]
struct Hist {
    kind: u8,
    id: u8,
    count: u16,
    until: u16,
}
static mut HIST: [Hist; HIST_SLOTS] = [Hist {
    kind: HIST_NONE,
    id: 0,
    count: 0,
    until: 0,
}; HIST_SLOTS];
static mut HIST_NEXT: usize = 0;

/// Put a pickup on the history column. The entries fill the column from the
/// bottom while any is showing and start over when none is.
pub unsafe fn history_add(kind: u8, id: u8, count: u16) {
    let now = crate::SIM_NOW;
    let mut live = false;
    let mut i = 0;
    while i < HIST_SLOTS {
        let left = HIST[i].until.wrapping_sub(now);
        live |= HIST[i].kind != HIST_NONE && left != 0 && left <= HIST_TICKS;
        i += 1;
    }
    if !live || HIST_NEXT >= HIST_SLOTS {
        HIST_NEXT = 0;
    }
    HIST[HIST_NEXT] = Hist {
        kind,
        id,
        count,
        until: now.wrapping_add(HIST_TICKS),
    };
    HIST_NEXT += 1;
}

/// CHudAmmoHistory: what was just picked up stacks up the right edge. Out of line and sized for
/// space: it draws nothing most of the time.
#[inline(never)]
#[optimize(size)]
unsafe fn draw_history<const N: usize>(
    amber: TextureMaterial,
    arsenal: &Arsenal,
    ot: &mut OrderingTable<N>,
    prims: &mut [Sprite; DRAW_CAP],
    count: &mut usize,
) {
    // CHudAmmoHistory: what was just picked up stacks up the right edge, newest
    // last, each fading out over its five seconds (`remaining * 80`, so four alpha
    // steps per 20 Hz tick). Weapons show their sprite at the right edge (red when
    // they came without ammo), ammo the count then its icon, items their sprite.
    let now = unsafe { crate::SIM_NOW };
    let mut slot = 0usize;
    while slot < HIST_SLOTS {
        let h = unsafe { HIST[slot] };
        let y = 240 - (72 + 25 * slot as i16);
        slot += 1;
        // The simulation clock wraps (u16): an entry shows for the 100 ticks before `until`.
        let left = h.until.wrapping_sub(now);
        if h.kind == HIST_NONE || left == 0 || left > HIST_TICKS {
            continue;
        }
        let fade = (left * 4).min(255) as u8;
        let tint = alpha_mat(amber, fade);
        match h.kind {
            HIST_WEAPON => {
                let (u, v) = weapon_icon_uv(h.id as usize);
                let mat = if has_ammo(arsenal, h.id as usize) {
                    tint
                } else {
                    amber.with_tint((((fade as u16 + 1) / 2) as u8, 0, 0))
                };
                sprite(mat, ot, prims, count, u, v, WEAPON_W, WEAPON_H, 320 - WEAPON_W as i16, y);
            }
            HIST_AMMO => {
                // Icon of the first weapon that fires this pool (the MP5's grenades have their own).
                let (u, v) = if h.id as usize == AMMO_ARGREN {
                    (MP5_SECONDARY_U, AMMO_V)
                } else {
                    let mut w = 0usize;
                    while w + 1 < N_WEAPONS && WEAPON_DEFS[w].ammo != h.id as usize {
                        w += 1;
                    }
                    ((w % 7) as u8 * AMMO_W, AMMO_V + (w / 7) as u8 * AMMO_H)
                };
                sprite(tint, ot, prims, count, u, v, AMMO_W, AMMO_H, 320 - AMMO_W as i16, y);
                // The count, right-aligned 14 pixels left of the icon.
                let mut digits = 1i16;
                let mut n = h.count / 10;
                while n > 0 {
                    digits += 1;
                    n /= 10;
                }
                let mut at = 320 - AMMO_W as i16 - 14 - digits * DW as i16;
                let mut place = 1u16;
                let mut k = 1;
                while k < digits {
                    place *= 10;
                    k += 1;
                }
                while place > 0 {
                    digit(tint, ot, prims, count, ((h.count / place) % 10) as u8, at, y);
                    at += DW as i16;
                    place /= 10;
                }
            }
            _ => {
                let (u, v) = match h.id {
                    PICKUP_HEALTHKIT => (HEALTHKIT_U, ITEM_V),
                    PICKUP_LONGJUMP => (LONGJUMP_U, ITEM_V),
                    _ => (BATTERY_U, BATTERY_V),
                };
                sprite(tint, ot, prims, count, u, v, BATTERY_W, BATTERY_H, 320 - BATTERY_W as i16 - 10, y);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_hud_layout_stays_inside_reserved_tpage_regions() {
        assert_eq!(DW, 12);
        assert_eq!(DH, 16);
        assert!(WEAPON_LEFT_V as usize + 5 * WEAPON_H as usize <= 256);
        assert!(WEAPON_RIGHT_U as usize + WEAPON_W as usize <= 256);
        assert!(WEAPON_RIGHT_V as usize + 4 * WEAPON_H as usize <= 256);
        assert_eq!(HEALTHKIT_U as usize, CROSS_W as usize * 3);
        assert!(FLASH_BEAM_U as usize + FLASH_BEAM_W as usize <= HUD_LEFT_W);
        assert!(FLASH_V as usize + FLASH_H as usize <= 128);
        assert_eq!(DRAW_CAP, 44);
        // Gameplay text owns u=160..255 only for v=0..127.
        assert!(WEAPON_RIGHT_V >= 128);
        assert_eq!(HUD_BLOB_BYTES, 23_108);
    }

    #[test]
    fn every_weapon_mapping_is_bounded() {
        for weapon in 0..N_WEAPONS {
            let (u, v) = weapon_icon_uv(weapon);
            assert!(u as usize + WEAPON_W as usize <= 256);
            assert!(v as usize + WEAPON_H as usize <= 256);
            assert!(WEAPON_BUCKET[weapon] < 5);
            assert!(CROSS_INDEX[weapon] < 9);
        }
        assert_eq!(CROSS_INDEX[0], -1); // crowbar
        assert_eq!(CROSS_INDEX[W_GRENADE], -1);
    }
}
