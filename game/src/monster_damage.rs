//! What a hit tells the monster that took it, for every species in one place.
//!
//! `CBaseMonster::TakeDamage` leaves its AI two things: the damage conditions
//! (`bits_COND_LIGHT_DAMAGE` for any damage, `bits_COND_HEAVY_DAMAGE` from 20
//! up) and `m_LastHitGroup`, the hit group of the bullet's trace, which picks
//! the flinch activity. [`conditions`] is that record for one hit: bits 0-2
//! the hit group (0 generic, 1 head, 2 chest, 3 stomach, 4-7 arms and legs;
//! the HECU helmet and the agrunt's plates read as generic), bit 3 light,
//! bit 4 heavy. The damage kinds stay with the hit (`DMG_ACID`, `DMG_BLAST`,
//! ... in main.rs).
//!
//! Today one thing reads it: a hit (light damage) can start the small flinch,
//! which holds the species' own think until the flinch clip has run; whether
//! this hit may flinch is the caller's (`prop_flinch_ticks`, GetSchedule's
//! rule). The flinch lives in
//! the actor's hit flash (`PROP_HIT_FLASH`): plain values are the 4-tick pain
//! pose every hit shows, [`FLINCH`] marks a flinch and its low bits count its
//! ticks. The record is not kept per actor; the schedule engine that will
//! read the conditions keeps its own per-actor state.

pub const GROUP: u8 = 7;
pub const LIGHT: u8 = 8;
pub const HEAVY: u8 = 16;
/// `flDamage >= 20` in CBaseMonster::TakeDamage.
pub const HEAVY_DAMAGE: u8 = 20;
/// A hit flash with this bit is a flinch.
pub const FLINCH: u8 = 0x80;

/// The conditions a hit of `dmg` (after its hit group and the species' own
/// scaling) on `group` sets.
pub const fn conditions(dmg: u8, group: u8) -> u8 {
    let mut out = group & GROUP;
    if dmg > 0 {
        out |= LIGHT;
    }
    if dmg >= HEAVY_DAMAGE {
        out |= HEAVY;
    }
    out
}

/// The hit flash after a hit with `cond`: a flinch of `flinch_ticks` (0 when
/// the actor does not flinch from this hit) on light damage, else the plain
/// `flash`. A running flinch is not restarted.
pub const fn flash_after_hit(current: u8, cond: u8, flinch_ticks: u8, flash: u8) -> u8 {
    if current & FLINCH != 0 {
        current
    } else if cond & LIGHT != 0 && flinch_ticks > 0 {
        FLINCH | if flinch_ticks > 127 { 127 } else { flinch_ticks }
    } else {
        flash
    }
}

/// One tick of the hit flash.
pub const fn flash_tick(flash: u8) -> u8 {
    if flash & !FLINCH <= 1 {
        0
    } else {
        flash - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_damage_is_light_and_twenty_is_heavy() {
        assert_eq!(conditions(0, 0), 0);
        assert_eq!(conditions(1, 0), LIGHT);
        assert_eq!(conditions(19, 1), LIGHT | 1);
        assert_eq!(conditions(20, 1), LIGHT | HEAVY | 1);
    }

    #[test]
    fn a_hit_that_may_flinch_plays_the_clip_and_any_other_flashes() {
        assert_eq!(flash_after_hit(0, conditions(40, 1), 13, 4), FLINCH | 13);
        assert_eq!(flash_after_hit(0, conditions(8, 1), 13, 4), FLINCH | 13);
        assert_eq!(flash_after_hit(0, conditions(0, 1), 13, 4), 4);
        // No flinch from this hit (the rule says no, a script holds it):
        // the plain flash.
        assert_eq!(flash_after_hit(0, conditions(40, 1), 0, 4), 4);
        // A second hit does not restart a running flinch.
        assert_eq!(flash_after_hit(FLINCH | 3, conditions(40, 0), 13, 4), FLINCH | 3);
        // The flinch counts down to nothing, as the plain flash does.
        let mut f = FLINCH | 13;
        let mut ticks = 0;
        while f != 0 {
            f = flash_tick(f);
            ticks += 1;
        }
        assert_eq!(ticks, 13);
        assert_eq!([flash_tick(4), flash_tick(1), flash_tick(0)], [3, 0, 0]);
    }
}
