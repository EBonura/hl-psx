//! Integer-only rules of the player's suit, HUD feedback and weapons, kept
//! free of PS1 state so the host runner can pin their behaviour. The game
//! side gathers observations from its globals, calls these steps and applies
//! the results (sounds, view punch, drawing).

/// Player health and suit armour.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Vitals {
    pub health: u16,
    pub armor: u16,
}

/// How a hit reaches the player.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlayerDamageKind {
    /// Ordinary damage: the suit armour absorbs part of it.
    Generic,
    /// Falling: goes straight to health, the suit does not help.
    Fall,
}

/// Health and armour after the player takes `dmg` points of damage.
pub fn apply_player_damage(v: Vitals, dmg: u16, kind: PlayerDamageKind) -> Vitals {
    let Vitals { health, armor } = v;
    if kind == PlayerDamageKind::Fall || armor == 0 {
        return Vitals {
            health: health.saturating_sub(dmg),
            armor,
        };
    }
    // The suit leaves a fifth of the damage on health and pays for the rest
    // at one armour point per two damage points.
    let health_dmg = dmg / 5;
    let armor_cost = (dmg.saturating_sub(health_dmg).saturating_add(1)) / 2;
    if armor_cost <= armor {
        return Vitals {
            health: health.saturating_sub(health_dmg),
            armor: armor - armor_cost,
        };
    }
    let absorbed = armor.saturating_mul(2);
    Vitals {
        health: health.saturating_sub(dmg.saturating_sub(absorbed)),
        armor: 0,
    }
}
