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

use crate::hud_layout::{HudCell, HudDraw, HudFade, HudInputs, Ink};
pub use crate::hud_layout::{
    PICKUP_BATTERY, PICKUP_HEALTHKIT, PICKUP_LONGJUMP, PICKUP_NONE, PICKUP_SUIT,
};
use psx_gpu::material::{BlendMode, TextureMaterial};
use psx_gpu::ot::OrderingTable;
use psx_gpu::prim::Sprite;
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
const HUD_RIGHT_H: usize = 80;
const HUD_LEFT_BYTES: usize = HUD_LEFT_W * HUD_LEFT_H / 2;
const HUD_RIGHT_BYTES: usize = HUD_RIGHT_W * HUD_RIGHT_H / 2;
const HUD_BLOB_BYTES: usize = HUD_HEADER_BYTES + HUD_LEFT_BYTES + HUD_RIGHT_BYTES;

/// The native HUD's busiest possible frame is MP5 + secondary ammo + selection
/// strip + item history + active flashlight. Each packet explicitly restores
/// TextureWindow::NONE: world materials can leave a window active before the
/// HUD OT is submitted.
pub const DRAW_CAP: usize = 34;
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
const SELECTION_U: u8 = 0;
const SELECTION_V: u8 = 16;
const SUIT_FULL_U: u8 = 80;
const SUIT_EMPTY_U: u8 = 100;
const SUIT_V: u8 = 16;
const HEALTH_U: u8 = 120;
const HEALTH_V: u8 = 16;
const DIVIDER_U: u8 = 136;
const DIVIDER_V: u8 = 16;
const BATTERY_U: u8 = 138;
const BATTERY_V: u8 = 16;
const BUCKET_V: u8 = 36;
const BUCKET_W: u8 = 12;
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
const ZOOM_U: u8 = 0;
const ZOOM_V: u8 = 132;
const WEAPON_LEFT_V: u8 = 148;
const WEAPON_RIGHT_U: u8 = 160;
const WEAPON_RIGHT_V: u8 = 128;
const WEAPON_W: u8 = 80;
const WEAPON_H: u8 = 20;
const TRAIN_U: u8 = 240;
const TRAIN_V: u8 = 128;
const TRAIN_H: u8 = 16;

const W_GRENADE: usize = 10;
const N_WEAPONS: usize = 14;

// Packed crosshair cell for each runtime weapon; melee and throwables have
// none, and the layout never asks for theirs.
const CROSS_INDEX: [i8; N_WEAPONS] = [-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, -1, -1, -1, -1];

/// Flash memory of the numbers between presented frames.
static mut FADE: HudFade = HudFade::new();

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
    ot.insert(0, core::ptr::from_mut(env).cast(), GpuEnv::WORDS);
}

/// Top-left texel of a HUD cell inside the atlas texture page.
pub fn atlas_uv(cell: HudCell) -> (u8, u8) {
    match cell {
        HudCell::Digit(n) => (n * DW, 0),
        HudCell::Divider => (DIVIDER_U, DIVIDER_V),
        HudCell::HealthCross => (HEALTH_U, HEALTH_V),
        HudCell::SuitEmpty => (SUIT_EMPTY_U, SUIT_V),
        HudCell::SuitFull => (SUIT_FULL_U, SUIT_V),
        HudCell::AmmoIcon(w) => ((w % 7) * AMMO_W, AMMO_V + (w / 7) * AMMO_H),
        HudCell::GrenadeAmmoIcon => (MP5_SECONDARY_U, AMMO_V),
        HudCell::FlashlightEmpty => (FLASH_EMPTY_U, FLASH_V),
        HudCell::FlashlightFull => (FLASH_FULL_U, FLASH_V),
        HudCell::FlashlightBeam => (FLASH_BEAM_U, FLASH_V),
        HudCell::TrainFrame(f) => (TRAIN_U, TRAIN_V + f * TRAIN_H),
        HudCell::SlotNumber(s) => (s * BUCKET_W, BUCKET_V),
        HudCell::SelectionFrame => (SELECTION_U, SELECTION_V),
        HudCell::WeaponPicture(w) => weapon_icon_uv(w as usize),
        HudCell::Crosshair(w) => {
            let ci = CROSS_INDEX[(w as usize).min(N_WEAPONS - 1)].max(0) as u8;
            ((ci % 6) * CROSS_W, CROSS_V + (ci / 6) * CROSS_H)
        }
        HudCell::ZoomReticle => (ZOOM_U, ZOOM_V),
        HudCell::BatteryIcon => (BATTERY_U, BATTERY_V),
        HudCell::HealthKitIcon => (HEALTHKIT_U, ITEM_V),
        HudCell::LongJumpIcon => (LONGJUMP_U, ITEM_V),
    }
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

/// Link one HUD record from `prims` into `ot` as a textured sprite.
///
/// # Safety
///
/// `prims` stays linked into `ot`: it must stay live and unmodified until
/// that table's walk has finished.
unsafe fn submit<const N: usize>(
    mats: Materials,
    ot: &mut OrderingTable<N>,
    prims: &mut [Sprite; DRAW_CAP],
    count: &mut usize,
    d: HudDraw,
) {
    if *count >= DRAW_CAP || d.w == 0 || d.h == 0 {
        return;
    }
    // PS1 texture modulation is texel * tint / 128 and the amber CLUT holds
    // the full HUD colour, so brightness b maps to a tint of (b + 1) / 2.
    let mat = match d.ink {
        Ink::Amber(b) => {
            let t = ((b as u16 + 1) / 2) as u8;
            mats.amber.with_tint((t, t, t))
        }
        Ink::Red(b) => {
            let t = ((b as u16 + 1) / 2) as u8;
            mats.amber.with_tint((t, 0, 0))
        }
        Ink::White => mats.white,
    };
    let (u, v) = atlas_uv(d.cell);
    prims[*count] = Sprite::with_material(
        d.x,
        d.y,
        d.w as u16,
        d.h as u16,
        (u + d.skip_x, v + d.skip_y),
        mat,
    );
    ot.insert(
        0,
        core::ptr::from_mut(&mut prims[*count]).cast(),
        Sprite::WORDS,
    );
    *count += 1;
}

/// Queue this frame's HUD into `ot`, using `prims` as packet storage, and
/// return how many sprites were linked. Records past [`DRAW_CAP`] are dropped.
///
/// # Safety
///
/// The sprites in `prims` stay linked into `ot`: they must stay live and
/// unmodified until that table's walk has finished.
pub unsafe fn draw<const N: usize>(
    mats: Materials,
    inputs: &HudInputs,
    ot: &mut OrderingTable<N>,
    prims: &mut [Sprite; DRAW_CAP],
) -> usize {
    let mut count = 0usize;
    // SAFETY: the HUD is drawn from the single render path only.
    let fade = unsafe { &mut *core::ptr::addr_of_mut!(FADE) };
    crate::hud_layout::layout(fade, inputs, &mut |d| unsafe {
        submit(mats, ot, prims, &mut count, d)
    });
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_hud_layout_stays_inside_reserved_tpage_regions() {
        assert_eq!(DW, 12);
        assert!(WEAPON_LEFT_V as usize + 5 * WEAPON_H as usize <= 256);
        assert!(WEAPON_RIGHT_U as usize + WEAPON_W as usize <= 256);
        assert!(WEAPON_RIGHT_V as usize + 4 * WEAPON_H as usize <= 256);
        assert_eq!(HEALTHKIT_U as usize, CROSS_W as usize * 3);
        assert!(FLASH_BEAM_U as usize + 6 <= HUD_LEFT_W);
        assert!(FLASH_V as usize + 16 <= 128);
        assert_eq!(DRAW_CAP, 34);
        // Gameplay text owns u=160..255 only for v=0..127.
        assert!(WEAPON_RIGHT_V >= 128);
        assert_eq!(HUD_BLOB_BYTES, 23_748);
    }

    #[test]
    fn every_weapon_mapping_is_bounded() {
        for weapon in 0..N_WEAPONS {
            let (u, v) = weapon_icon_uv(weapon);
            assert!(u as usize + WEAPON_W as usize <= 256);
            assert!(v as usize + WEAPON_H as usize <= 256);
            assert!(CROSS_INDEX[weapon] < 9);
        }
        assert_eq!(CROSS_INDEX[0], -1); // crowbar
        assert_eq!(CROSS_INDEX[W_GRENADE], -1);
    }
}
