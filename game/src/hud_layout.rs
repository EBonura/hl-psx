//! What the first-person HUD draws on one presented frame, as plain records.
//!
//! This module decides which HUD cells appear, where on the 320x240 screen,
//! how much of each is shown and how bright, from a plain [`HudInputs`]
//! snapshot plus the per-value flash memory in [`HudFade`]. It touches no GPU,
//! VRAM or globals, so the host runner can test it. `hud.rs` turns each
//! [`HudDraw`] into one textured sprite from the HUD atlas, in the order the
//! records arrive.
//!
//! Records arrive in submission order. The ordering table prepends, so the GPU
//! draws them in reverse: a record emitted earlier lands on top of one emitted
//! later. Every HUD draw is additive.

/// No item pickup is being shown.
pub const PICKUP_NONE: u8 = 0;
/// The HEV suit was picked up.
pub const PICKUP_SUIT: u8 = 1;
/// A suit battery was picked up.
pub const PICKUP_BATTERY: u8 = 2;
/// A health kit was picked up.
pub const PICKUP_HEALTHKIT: u8 = 3;
/// The long-jump module was picked up.
pub const PICKUP_LONGJUMP: u8 = 4;

/// How the held weapon's ammunition is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmmoDisplay {
    /// Melee weapons: no ammo block.
    None,
    /// Weapons without a magazine: one reserve count.
    ReserveOnly,
    /// Magazine weapons: clip count, divider, reserve count.
    ClipAndReserve,
}

/// Everything the HUD observes on one presented frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HudInputs {
    /// The HEV suit is worn; without it only the crosshair, weapon selection,
    /// train indicator and pickup history can appear.
    pub suit: bool,
    pub health: u16,
    pub armour: u16,
    /// Held weapon index (0 crowbar .. 13), `None` with nothing held.
    pub weapon: Option<u8>,
    pub ammo: AmmoDisplay,
    /// Rounds in the held weapon's magazine.
    pub clip: u16,
    /// Reserve rounds of the held weapon's ammo type.
    pub reserve: u16,
    /// MP5 grenades carried.
    pub secondary: u16,
    /// The view is zoomed (the crossbow scope).
    pub zoomed: bool,
    /// Ticks left on the weapon selection display; 0 hides it.
    pub selection_ticks: u8,
    pub flashlight_on: bool,
    /// Flashlight charge, 0..100.
    pub flashlight_charge: u8,
    /// Controllable train speed setting 1..5, or 0 for no indicator.
    pub train_setting: u8,
    /// One of the `PICKUP_*` kinds.
    pub pickup_kind: u8,
    /// Ticks left on the pickup history entry.
    pub pickup_ticks: u8,
}

/// Brightness and palette of one record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// The amber HUD colour at a brightness of 0..255 (255 = full art colour).
    Amber(u8),
    /// The amber art drawn red only, at a brightness of 0..255.
    Red(u8),
    /// The white palette at full brightness.
    White,
}

/// One cell of the HUD atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudCell {
    /// Number glyph 0..9 (12x16).
    Digit(u8),
    /// One-pixel separator bar (1x16).
    Divider,
    /// Health cross (16x16).
    HealthCross,
    /// Armour suit outline (20x20).
    SuitEmpty,
    /// Armour suit, filled (20x20).
    SuitFull,
    /// Ammo icon of a weapon 0..13 (18x18).
    AmmoIcon(u8),
    /// MP5 grenade icon (18x18).
    GrenadeAmmoIcon,
    /// Flashlight casing, empty (18x16).
    FlashlightEmpty,
    /// Flashlight casing, charged (18x16).
    FlashlightFull,
    /// Flashlight beam (6x16).
    FlashlightBeam,
    /// Train speed frame 0..4 (16x16).
    TrainFrame(u8),
    /// Weapon slot number 0..4 (12x12).
    SlotNumber(u8),
    /// Frame around the selected weapon's picture (80x20).
    SelectionFrame,
    /// Picture of a weapon 0..13 (80x20).
    WeaponPicture(u8),
    /// Crosshair of a weapon 0..13 that has one (24x24).
    Crosshair(u8),
    /// Crossbow scope reticle (104x16).
    ZoomReticle,
    /// Battery pickup icon (20x20).
    BatteryIcon,
    /// Health kit pickup icon (20x20).
    HealthKitIcon,
    /// Long-jump pickup icon (20x20).
    LongJumpIcon,
}

/// One textured rectangle on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HudDraw {
    pub cell: HudCell,
    /// Top-left screen position at 320x240.
    pub x: i16,
    pub y: i16,
    /// Size drawn, at most the cell's size.
    pub w: u8,
    pub h: u8,
    /// Columns and rows of the cell skipped before the drawn part starts, for
    /// cells shown only partly.
    pub skip_x: u8,
    pub skip_y: u8,
    pub ink: Ink,
}

/// Flash memory of the health, armour and ammo numbers between frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HudFade {
    prev_health: u16,
    prev_armour: u16,
    prev_clip: u16,
    prev_reserve: u16,
    prev_weapon: i16,
    health_fade: u8,
    armour_fade: u8,
    ammo_fade: u8,
}

impl HudFade {
    /// The memory before the first frame: nothing has changed yet.
    pub const fn new() -> Self {
        Self {
            prev_health: u16::MAX,
            prev_armour: u16::MAX,
            prev_clip: u16::MAX,
            prev_reserve: u16::MAX,
            prev_weapon: -2,
            health_fade: 0,
            armour_fade: 0,
            ammo_fade: 0,
        }
    }
}

impl Default for HudFade {
    fn default() -> Self {
        Self::new()
    }
}

/// Emit this frame's HUD records into `out`, in submission order, and advance
/// the flash memory by one frame.
pub fn layout(fade: &mut HudFade, inputs: &HudInputs, out: &mut impl FnMut(HudDraw)) {
    let weapon = inputs.weapon.map(|w| (w as usize).min(N_WEAPONS - 1));

    if let Some(w) = weapon {
        if inputs.zoomed && w == W_CROSSBOW {
            emit(
                out,
                HudCell::ZoomReticle,
                ZOOM_W,
                ZOOM_H,
                160 - ZOOM_W as i16 / 2,
                120 - ZOOM_H as i16 / 2,
                Ink::White,
            );
        } else if matches!(w, 1..=9) {
            emit(
                out,
                HudCell::Crosshair(w as u8),
                CROSS_W,
                CROSS_H,
                160 - CROSS_W as i16 / 2,
                120 - CROSS_H as i16 / 2,
                Ink::White,
            );
        }
        if inputs.selection_ticks > 0 {
            weapon_selection(out, w);
        }
    }

    let train = inputs.train_setting;
    if (1..=5).contains(&train) {
        emit(
            out,
            HudCell::TrainFrame(train - 1),
            TRAIN_W,
            TRAIN_H,
            320 / 3 + TRAIN_W as i16 / 4,
            240 - TRAIN_H as i16 - DH as i16,
            Ink::Amber(255),
        );
    }

    if inputs.suit {
        let health = inputs.health;
        let armor = inputs.armour;
        let clip_ammo = inputs.clip;
        let reserve_ammo = inputs.reserve;
        let (health_alpha, armor_alpha, ammo_alpha) = {
            if fade.prev_health == u16::MAX {
                fade.prev_health = health;
            } else if fade.prev_health != health {
                fade.prev_health = health;
                fade.health_fade = 100;
            }
            if fade.prev_armour == u16::MAX {
                fade.prev_armour = armor;
            } else if fade.prev_armour != armor {
                fade.prev_armour = armor;
                fade.armour_fade = 100;
            }
            let weapon_now = inputs.weapon.map_or(-1, |w| w as i16);
            if fade.prev_clip == u16::MAX {
                fade.prev_clip = clip_ammo;
                fade.prev_reserve = reserve_ammo;
                fade.prev_weapon = weapon_now;
            } else if fade.prev_clip != clip_ammo
                || fade.prev_reserve != reserve_ammo
                || fade.prev_weapon != weapon_now
            {
                fade.prev_clip = clip_ammo;
                fade.prev_reserve = reserve_ammo;
                fade.prev_weapon = weapon_now;
                fade.ammo_fade = 200;
            }
            let ha = if health <= 15 {
                255
            } else {
                100 + (fade.health_fade as u16 * 128 / 100) as u8
            };
            let ba = 100 + (fade.armour_fade as u16 * 128 / 100) as u8;
            let aa = fade.ammo_fade.max(100);
            fade.health_fade = fade.health_fade.saturating_sub(1);
            fade.armour_fade = fade.armour_fade.saturating_sub(1);
            fade.ammo_fade = fade.ammo_fade.saturating_sub(1);
            (ha, ba, aa)
        };

        let health_ink = if health <= 25 {
            Ink::Red(health_alpha)
        } else {
            Ink::Amber(health_alpha)
        };
        let armor_ink = Ink::Amber(armor_alpha);
        let ammo_ink = Ink::Amber(ammo_alpha);
        let base_y = 240 - DH as i16 - DH as i16 / 2; // 216
        let number_y = base_y;

        emit(
            out,
            HudCell::HealthCross,
            HEALTH_W,
            HEALTH_H,
            HEALTH_W as i16 / 2,
            base_y,
            health_ink,
        );
        let hx = number_3(out, health_ink, health, 22, number_y);
        emit(
            out,
            HudCell::Divider,
            1,
            DH,
            hx + DW as i16 / 2,
            number_y,
            health_ink,
        );

        let suit_y = base_y - SUIT_H as i16 / 6;
        emit(
            out,
            HudCell::SuitEmpty,
            SUIT_W,
            SUIT_H,
            SUIT_X,
            suit_y,
            armor_ink,
        );
        if armor > 0 {
            let cut = ((SUIT_H as u16 * (100 - armor.min(100))) / 100) as u8;
            out(HudDraw {
                cell: HudCell::SuitFull,
                x: SUIT_X,
                y: suit_y + cut as i16,
                w: SUIT_W,
                h: SUIT_H - cut,
                skip_x: 0,
                skip_y: cut,
                ink: armor_ink,
            });
        }
        number_3(out, armor_ink, armor, SUIT_X + SUIT_W as i16, number_y);

        if inputs.ammo != AmmoDisplay::None {
            if let Some(w) = weapon {
                let icon = HudCell::AmmoIcon(w as u8);
                let icon_y = number_y - AMMO_H as i16 / 8;
                if inputs.ammo == AmmoDisplay::ClipAndReserve {
                    let x = 320 - 8 * DW as i16 - AMMO_W as i16;
                    let end_clip = number_3(out, ammo_ink, clip_ammo, x, number_y);
                    let bar_x = end_clip + DW as i16 / 2;
                    emit(out, HudCell::Divider, 1, DH, bar_x, number_y, ammo_ink);
                    let reserve_x = bar_x + 1 + DW as i16 / 2;
                    let icon_x = number_3(out, ammo_ink, reserve_ammo, reserve_x, number_y);
                    emit(out, icon, AMMO_W, AMMO_H, icon_x, icon_y, ammo_ink);
                } else {
                    let x = 320 - 4 * DW as i16 - AMMO_W as i16;
                    let icon_x = number_3(out, ammo_ink, reserve_ammo, x, number_y);
                    emit(out, icon, AMMO_W, AMMO_H, icon_x, icon_y, ammo_ink);
                }

                if w == W_MP5 && inputs.secondary > 0 {
                    let sy = number_y - DH as i16 - DH as i16 / 4;
                    let x = 320 - 4 * DW as i16 - AMMO_W as i16;
                    let icon_x = number_3(out, ammo_ink, inputs.secondary, x, sy);
                    emit(
                        out,
                        HudCell::GrenadeAmmoIcon,
                        AMMO_W,
                        AMMO_H,
                        icon_x,
                        sy - AMMO_H as i16 / 8,
                        ammo_ink,
                    );
                }
            }
        }

        let flash_alpha = if inputs.flashlight_on { 225 } else { 100 };
        let flash_ink = if inputs.flashlight_charge < 20 {
            Ink::Red(flash_alpha)
        } else {
            Ink::Amber(flash_alpha)
        };
        let flash_x = 320 - FLASH_W as i16 - FLASH_W as i16 / 2;
        let flash_y = FLASH_H as i16 / 2;
        emit(
            out,
            HudCell::FlashlightEmpty,
            FLASH_W,
            FLASH_H,
            flash_x,
            flash_y,
            flash_ink,
        );
        if inputs.flashlight_on {
            emit(
                out,
                HudCell::FlashlightBeam,
                FLASH_BEAM_W,
                FLASH_H,
                320 - FLASH_W as i16 / 2,
                flash_y,
                flash_ink,
            );
        }
        let flash_offset =
            (FLASH_W as u16 * (100u16 - inputs.flashlight_charge.min(100) as u16) / 100) as u8;
        if flash_offset < FLASH_W {
            out(HudDraw {
                cell: HudCell::FlashlightFull,
                x: flash_x + flash_offset as i16,
                y: flash_y,
                w: FLASH_W - flash_offset,
                h: FLASH_H,
                skip_x: flash_offset,
                skip_y: 0,
                ink: flash_ink,
            });
        }
    }

    let pickup_kind = inputs.pickup_kind;
    if matches!(
        pickup_kind,
        PICKUP_BATTERY | PICKUP_HEALTHKIT | PICKUP_LONGJUMP
    ) && inputs.pickup_ticks > 0
    {
        let fade = (inputs.pickup_ticks as u16 * 4).min(255) as u8;
        let cell = match pickup_kind {
            PICKUP_HEALTHKIT => HudCell::HealthKitIcon,
            PICKUP_LONGJUMP => HudCell::LongJumpIcon,
            _ => HudCell::BatteryIcon,
        };
        emit(
            out,
            cell,
            ITEM_W,
            ITEM_H,
            320 - ITEM_W as i16 - 10,
            168,
            Ink::Amber(fade),
        );
    }
}

const DW: u8 = 12;
const DH: u8 = 16;
const SELECTION_W: u8 = 80;
const SELECTION_H: u8 = 20;
const SUIT_W: u8 = 20;
const SUIT_H: u8 = 20;
const SUIT_X: i16 = 320 / 5;
const HEALTH_W: u8 = 16;
const HEALTH_H: u8 = 16;
const ITEM_W: u8 = 20;
const ITEM_H: u8 = 20;
const BUCKET_W: u8 = 12;
const BUCKET_H: u8 = 12;
const AMMO_W: u8 = 18;
const AMMO_H: u8 = 18;
const CROSS_W: u8 = 24;
const CROSS_H: u8 = 24;
const FLASH_W: u8 = 18;
const FLASH_H: u8 = 16;
const FLASH_BEAM_W: u8 = 6;
const ZOOM_W: u8 = 104;
const ZOOM_H: u8 = 16;
const WEAPON_W: u8 = 80;
const WEAPON_H: u8 = 20;
const TRAIN_W: u8 = 16;
const TRAIN_H: u8 = 16;

const W_MP5: usize = 3;
const W_CROSSBOW: usize = 5;
const N_WEAPONS: usize = 14;

// Selection slot of each weapon, zero based.
const WEAPON_BUCKET: [u8; N_WEAPONS] = [0, 1, 1, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4];

#[inline(always)]
fn emit(out: &mut impl FnMut(HudDraw), cell: HudCell, w: u8, h: u8, x: i16, y: i16, ink: Ink) {
    out(HudDraw {
        cell,
        x,
        y,
        w,
        h,
        skip_x: 0,
        skip_y: 0,
        ink,
    });
}

fn digit(out: &mut impl FnMut(HudDraw), ink: Ink, value: u8, x: i16, y: i16) {
    emit(out, HudCell::Digit(value), DW, DH, x, y, ink);
}

fn number_3(out: &mut impl FnMut(HudDraw), ink: Ink, value: u16, x: i16, y: i16) -> i16 {
    let value = value.min(999);
    if value >= 100 {
        digit(out, ink, (value / 100) as u8, x, y);
    }
    if value >= 10 {
        digit(out, ink, ((value % 100) / 10) as u8, x + DW as i16, y);
    }
    digit(out, ink, (value % 10) as u8, x + 2 * DW as i16, y);
    x + 3 * DW as i16
}

fn weapon_selection(out: &mut impl FnMut(HudDraw), weapon: usize) {
    let active = WEAPON_BUCKET[weapon] as usize;
    let active_x = 10 + active as i16 * (BUCKET_W as i16 + 5);
    let mut x = 10i16;
    for bucket in 0..5usize {
        emit(
            out,
            HudCell::SlotNumber(bucket as u8),
            BUCKET_W,
            BUCKET_H,
            x,
            10,
            Ink::Amber(255),
        );
        x += if bucket == active {
            WEAPON_W as i16 + 5
        } else {
            BUCKET_W as i16 + 5
        };
    }
    emit(
        out,
        HudCell::SelectionFrame,
        SELECTION_W,
        SELECTION_H,
        active_x,
        22,
        Ink::Amber(255),
    );
    emit(
        out,
        HudCell::WeaponPicture(weapon as u8),
        WEAPON_W,
        WEAPON_H,
        active_x,
        22,
        Ink::Amber(255),
    );
}
