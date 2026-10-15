//! Selection, exhaustion and ammo-pickup rules of the player's arsenal.
//!
//! Pure functions over small integers so the host runner can exercise them
//! (`hl-logic-tests` includes this file by path). They describe how the retail
//! weapon set decides what you may select, what the cycle keys step through,
//! what you are switched to when a thrown weapon runs dry, and where the rounds
//! of a duplicate pickup go.

use crate::combat_defs::*;

/// HUD bucket and position of each weapon id (the weapon list the retail
/// client builds), which is the order the next/previous keys walk.
pub const SLOT: [(u8, u8); N_WEAPONS] = [
    (0, 0), // crowbar
    (1, 0), // glock
    (1, 1), // .357
    (2, 0), // mp5
    (2, 1), // shotgun
    (2, 2), // crossbow
    (3, 0), // rpg
    (3, 1), // gauss
    (3, 2), // egon
    (3, 3), // hornet gun
    (4, 0), // hand grenade
    (4, 3), // snark
    (4, 2), // tripmine
    (4, 1), // satchel
];

/// Auto-switch weight (`iWeight`): when a thrown weapon is spent the heaviest
/// usable weapon takes over.
pub const WEIGHT: [i8; N_WEAPONS] = [0, 10, 15, 15, 15, 10, 20, 20, 20, 15, 5, 5, -10, -10];

/// Weapons whose last round removes them from the inventory.
pub const fn exhaustible(id: usize) -> bool {
    matches!(id, W_GRENADE | W_SNARK | W_TRIPMINE | W_SATCHEL)
}

/// Weapons the automatic switch never picks (the RPG, which needs a hand free
/// to guide it).
pub const fn no_auto_switch_to(id: usize) -> bool {
    id == W_RPG
}

/// Weapons that never start a reload on their own (the hornet gun refills by
/// itself and the thrown weapons have no magazine).
pub const fn no_auto_reload(id: usize) -> bool {
    id == W_HORNET
}

/// The weapon list in the order the cycle keys walk it, and each weapon's
/// place in it.
pub const HUD_ORDER: [u8; N_WEAPONS] = hud_order();
pub const HUD_POS: [u8; N_WEAPONS] = hud_positions();

const fn hud_order() -> [u8; N_WEAPONS] {
    let mut out = [0u8; N_WEAPONS];
    let mut n = 0;
    let mut slot = 0u8;
    while slot < 5 {
        let mut pos = 0u8;
        while pos < 4 {
            let mut id = 0;
            while id < N_WEAPONS {
                if SLOT[id].0 == slot && SLOT[id].1 == pos {
                    out[n] = id as u8;
                    n += 1;
                }
                id += 1;
            }
            pos += 1;
        }
        slot += 1;
    }
    out
}

const fn hud_positions() -> [u8; N_WEAPONS] {
    let order = hud_order();
    let mut out = [0u8; N_WEAPONS];
    let mut i = 0;
    while i < N_WEAPONS {
        out[order[i] as usize] = i as u8;
        i += 1;
    }
    out
}

/// Whether a weapon can be brought up: the crowbar always, a weapon with any
/// round in its magazine or pool, and the satchel while a charge is out to
/// detonate (`radio_ready`). The hornet gun refills by itself, so it always can.
pub const fn selectable(id: usize, clip: u16, pool: u16, radio_ready: bool) -> bool {
    if id == W_CROWBAR || id == W_HORNET {
        return true;
    }
    if id == W_SATCHEL && radio_ready {
        return true;
    }
    clip > 0 || pool > 0
}

/// The next (or previous) weapon after `current` in HUD order among those in
/// `mask` (the weapons owned and usable). `None` when nothing else qualifies.
#[inline(never)]
#[optimize(size)]
pub fn cycle_target(mask: u16, current: usize, forward: bool) -> Option<usize> {
    let mut i = HUD_POS[current] as usize;
    let mut n = 1;
    while n < N_WEAPONS {
        i = if forward {
            if i + 1 == N_WEAPONS { 0 } else { i + 1 }
        } else if i == 0 {
            N_WEAPONS - 1
        } else {
            i - 1
        };
        let w = HUD_ORDER[i] as usize;
        if mask >> w & 1 != 0 {
            return Some(w);
        }
        n += 1;
    }
    None
}

/// Where the player is sent when `current` (a thrown weapon) is spent, among
/// the weapons in `mask` (owned and usable): one of the same weight first, else
/// the heaviest that may be auto-selected.
#[inline(never)]
#[optimize(size)]
pub fn best_after_exhaust(mask: u16, current: usize) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut best_weight = -1i16;
    let mut id = 0;
    while id < N_WEAPONS {
        if id != current && mask >> id & 1 != 0 && !no_auto_switch_to(id) {
            if WEIGHT[id] == WEIGHT[current] {
                return Some(id);
            }
            if WEIGHT[id] as i16 > best_weight {
                best_weight = WEIGHT[id] as i16;
                best = Some(id);
            }
        }
        id += 1;
    }
    best
}

/// The rounds a duplicate pickup of a weapon already held adds: an empty
/// magazine fills first and the rest goes to the pool; otherwise everything
/// goes to the pool. Returns the new magazine and pool counts.
#[optimize(size)]
pub fn duplicate_pickup(clip: u16, clip_max: u16, pool: u16, pool_max: u16, give: u16) -> (u16, u16) {
    if clip_max == 0 {
        return (clip, pool.saturating_add(give).min(pool_max));
    }
    if clip == 0 {
        let to_clip = give.min(clip_max);
        return (to_clip, pool.saturating_add(give - to_clip).min(pool_max));
    }
    (clip, pool.saturating_add(give).min(pool_max))
}

/// Whether an empty magazine should refill on its own: no trigger held, the
/// attack timer spent, rounds left to load, and a weapon that reloads itself.
pub const fn wants_auto_reload(
    id: usize,
    clip: u16,
    clip_max: u16,
    pool: u16,
    trigger_held: bool,
    timer_spent: bool,
    busy: bool,
) -> bool {
    clip_max > 0
        && clip == 0
        && pool > 0
        && !trigger_held
        && timer_spent
        && !busy
        && !no_auto_reload(id)
}

/// A rocket the player guides may not be holstered while it flies: the weapon
/// cannot be put away with the laser on and a rocket in the air.
pub const fn can_holster(id: usize, laser_on: bool, rockets_in_flight: u8) -> bool {
    !(id == W_RPG && laser_on && rockets_in_flight != 0)
}

/// Length of the crowbar's first line trace.
pub const CROWBAR_LINE: i32 = 32;
/// Half-width of the swept hull the crowbar falls back to.
pub const CROWBAR_HULL: i32 = 16;

#[optimize(size)]

fn isqrt(v: i64) -> i32 {
    psx_math::int32::isqrt_i32(v.clamp(0, i32::MAX as i64) as i32) as i32
}

/// Where the crowbar's swept hull first touches an actor: `d` is the actor's
/// centre relative to the eye, `fwd` the unit view direction in q12, and the
/// actor a cylinder of radius `r` and half-height `hh`. The hull is a
/// 16-unit sphere carried along the 32-unit swing. Returns the distance along
/// the swing of the closest approach when it touches.
#[inline(never)]
#[optimize(size)]
pub fn crowbar_sweep_contact(d: [i32; 3], fwd: [i32; 3], r: i32, hh: i32) -> Option<i32> {
    let along = (d[0] as i64 * fwd[0] as i64 + d[1] as i64 * fwd[1] as i64 + d[2] as i64 * fwd[2] as i64) >> 12;
    let t = along.clamp(0, CROWBAR_LINE as i64) as i32;
    let px = d[0] as i64 - ((fwd[0] as i64 * t as i64) >> 12);
    let py = d[1] as i64 - ((fwd[1] as i64 * t as i64) >> 12);
    let pz = d[2] as i64 - ((fwd[2] as i64 * t as i64) >> 12);
    let horizontal = isqrt(px * px + pz * pz);
    if horizontal <= CROWBAR_HULL + r && (py.abs() as i32) <= CROWBAR_HULL + hh {
        Some(t)
    } else {
        None
    }
}

/// Ticks before the crowbar does full damage again: a swing lands full damage
/// only a second after the attack timer it set has expired (0.25 s after a
/// hit, 0.5 s after a miss).
pub const fn crowbar_full_damage_ticks(hit: bool) -> u8 {
    if hit {
        25
    } else {
        30
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_order_is_slot_major() {
        assert_eq!(
            HUD_ORDER.map(|w| w as usize),
            [
                W_CROWBAR, W_GLOCK, W_357, W_MP5, W_SHOTGUN, W_CROSSBOW, W_RPG, W_GAUSS, W_EGON,
                W_HORNET, W_GRENADE, W_SATCHEL, W_TRIPMINE, W_SNARK
            ]
        );
        for (i, w) in HUD_ORDER.iter().enumerate() {
            assert_eq!(HUD_POS[*w as usize] as usize, i);
        }
    }

    #[test]
    fn cycle_skips_unusable_weapons_and_wraps() {
        let mask = (1 << W_CROWBAR) | (1 << W_GLOCK) | (1 << W_SNARK);
        assert_eq!(cycle_target(mask, W_GLOCK, true), Some(W_SNARK));
        assert_eq!(cycle_target(mask, W_SNARK, true), Some(W_CROWBAR));
        assert_eq!(cycle_target(mask, W_CROWBAR, false), Some(W_SNARK));
        assert_eq!(cycle_target(1 << W_GLOCK, W_GLOCK, true), None);
    }

    #[test]
    fn selectable_rules() {
        assert!(selectable(W_CROWBAR, 0, 0, false));
        assert!(selectable(W_HORNET, 0, 0, false));
        assert!(!selectable(W_MP5, 0, 0, false));
        assert!(selectable(W_MP5, 0, 5, false));
        assert!(selectable(W_MP5, 3, 0, false));
        assert!(!selectable(W_SATCHEL, 0, 0, false));
        assert!(selectable(W_SATCHEL, 0, 0, true));
    }

    #[test]
    fn spent_grenade_falls_back_to_the_heaviest_usable_weapon() {
        let mask = (1 << W_CROWBAR) | (1 << W_GLOCK) | (1 << W_GAUSS) | (1 << W_RPG);
        // gauss and rpg weigh 20; the rpg is never auto-selected
        assert_eq!(best_after_exhaust(mask, W_GRENADE), Some(W_GAUSS));
        // a same-weight weapon wins outright (snark and grenade both weigh 5)
        assert_eq!(best_after_exhaust(mask | (1 << W_SNARK), W_GRENADE), Some(W_SNARK));
        // only a crowbar left
        assert_eq!(best_after_exhaust(1 << W_CROWBAR, W_GRENADE), Some(W_CROWBAR));
    }

    #[test]
    fn duplicate_pickup_fills_an_empty_magazine_first() {
        assert_eq!(duplicate_pickup(0, 17, 10, 250, 17), (17, 10));
        assert_eq!(duplicate_pickup(0, 17, 10, 250, 20), (17, 13));
        assert_eq!(duplicate_pickup(5, 17, 10, 250, 17), (5, 27));
        assert_eq!(duplicate_pickup(0, 0, 8, 10, 5), (0, 10));
        assert_eq!(duplicate_pickup(0, 6, 35, 36, 6), (6, 35));
    }

    #[test]
    fn auto_reload_waits_for_a_released_trigger() {
        assert!(wants_auto_reload(W_GLOCK, 0, 17, 40, false, true, false));
        assert!(!wants_auto_reload(W_GLOCK, 0, 17, 40, true, true, false));
        assert!(!wants_auto_reload(W_GLOCK, 0, 17, 40, false, false, false));
        assert!(!wants_auto_reload(W_GLOCK, 0, 17, 0, false, true, false));
        assert!(!wants_auto_reload(W_GAUSS, 0, 0, 40, false, true, false));
        assert!(!wants_auto_reload(W_HORNET, 0, 8, 40, false, true, false));
        assert!(!wants_auto_reload(W_GLOCK, 3, 17, 40, false, true, false));
    }

    #[test]
    fn a_guided_rocket_blocks_holstering() {
        assert!(!can_holster(W_RPG, true, 1));
        assert!(can_holster(W_RPG, false, 1));
        assert!(can_holster(W_RPG, true, 0));
        assert!(can_holster(W_GLOCK, true, 3));
    }

    #[test]
    fn crowbar_hull_reaches_a_little_past_the_line() {
        let fwd = [0, 0, 4096];
        // a standing human (radius 16, half-height 36) in front of the swing
        assert!(crowbar_sweep_contact([0, 0, 40], fwd, 16, 36).is_some());
        // 60 units out: 32 line + 16 hull + 16 radius = 64 is the limit
        assert!(crowbar_sweep_contact([0, 0, 60], fwd, 16, 36).is_some());
        assert!(crowbar_sweep_contact([0, 0, 66], fwd, 16, 36).is_none());
        // off to the side beyond hull + radius
        assert!(crowbar_sweep_contact([40, 0, 20], fwd, 16, 36).is_none());
        assert!(crowbar_sweep_contact([28, 0, 20], fwd, 16, 36).is_some());
        // above the head
        assert!(crowbar_sweep_contact([0, 80, 20], fwd, 16, 36).is_none());
        // a headcrab (radius 12) is easier to miss than a scientist
        assert!(crowbar_sweep_contact([0, 0, 62], fwd, 12, 12).is_none());
        assert!(crowbar_sweep_contact([0, 0, 58], fwd, 12, 12).is_some());
    }

    #[test]
    fn crowbar_full_damage_returns_a_second_after_the_attack_timer() {
        assert_eq!(crowbar_full_damage_ticks(true), 25);
        assert_eq!(crowbar_full_damage_ticks(false), 30);
    }
}
