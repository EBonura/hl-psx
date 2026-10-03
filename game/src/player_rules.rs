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

/// Damage compass edge bits, one per screen-edge arrow.
pub const COMPASS_FRONT: u8 = 1 << 0;
pub const COMPASS_RIGHT: u8 = 1 << 1;
pub const COMPASS_REAR: u8 = 1 << 2;
pub const COMPASS_LEFT: u8 = 1 << 3;

/// Screen-edge arrows that show which way recent damage came from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DamageCompass {
    edges: u8,
    ticks: u8,
}

impl DamageCompass {
    pub const fn new() -> Self {
        Self { edges: 0, ticks: 0 }
    }

    /// Edge bits currently lit.
    pub const fn edges(&self) -> u8 {
        self.edges
    }

    /// Light the arrows facing a damage source. `forward` and `right` are the
    /// view's horizontal forward and screen-right axes in world space, as
    /// 1.0 = 4096 vectors.
    pub fn note_hit(
        &mut self,
        player: [i32; 3],
        forward: [i16; 3],
        right: [i16; 3],
        source: [i32; 3],
    ) {
        let dx = (source[0] - player[0]).clamp(-4096, 4096);
        let dy = (source[1] - player[1]).clamp(-4096, 4096);
        let dz = (source[2] - player[2]).clamp(-4096, 4096);
        let delta = [dx, dy, dz];
        let d2 = dx * dx + dy * dy + dz * dz;
        let mut edges = 0u8;
        if d2 <= 50 * 50 {
            edges = COMPASS_FRONT | COMPASS_RIGHT | COMPASS_REAR | COMPASS_LEFT;
        } else {
            let distance = psx_math::int32::isqrt_i32(d2);
            let ahead = dot_q12(forward, delta);
            let side = dot_q12(right, delta);
            let threshold = distance * 3;
            if ahead.abs() * 10 > threshold {
                edges |= if ahead > 0 {
                    COMPASS_FRONT
                } else {
                    COMPASS_REAR
                };
            }
            if side.abs() * 10 > threshold {
                edges |= if side > 0 {
                    COMPASS_RIGHT
                } else {
                    COMPASS_LEFT
                };
            }
        }
        self.edges |= edges;
        self.ticks = 10;
    }

    /// Advance one 20 Hz tick; the arrows go dark 10 ticks after the last hit.
    pub fn tick(&mut self) {
        if self.ticks > 0 {
            self.ticks -= 1;
            if self.ticks == 0 {
                self.edges = 0;
            }
        }
    }
}

/// Colour of the lit compass arrows (additive red, green, blue): amber while
/// health is above 25, red at 25 or below.
pub const fn compass_colour(health: u16) -> [u8; 3] {
    [128, if health > 25 { 40 } else { 0 }, 0]
}

#[inline(always)]
fn dot_q12(row: [i16; 3], e: [i32; 3]) -> i32 {
    ((row[0] as i32 * e[0]) + (row[1] as i32 * e[1]) + (row[2] as i32 * e[2])) >> 12
}

/// Radiation volumes further than this (world units) from the player never
/// make the Geiger counter click.
pub const GEIGER_RANGE: i32 = 800;

/// The Geiger counter samples once every this many 20 Hz ticks (0.25 s).
pub const GEIGER_SAMPLE_TICKS: u8 = 5;

/// A Geiger click to play, at 1/`volume_den` of full volume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeigerClick {
    pub volume_den: u16,
}

/// One Geiger sample: feed it the centre of every active radiation volume,
/// then roll for a click against the nearest one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeigerScan {
    nearest2: i32,
}

impl Default for GeigerScan {
    fn default() -> Self {
        Self::new()
    }
}

impl GeigerScan {
    pub const fn new() -> Self {
        Self {
            nearest2: (GEIGER_RANGE + 1) * (GEIGER_RANGE + 1),
        }
    }

    pub fn add_source(&mut self, player: [i32; 3], centre: [i32; 3]) {
        let lim = GEIGER_RANGE + 1;
        let dx = (centre[0] - player[0]).clamp(-lim, lim);
        let dy = (centre[1] - player[1]).clamp(-lim, lim);
        let dz = (centre[2] - player[2]).clamp(-lim, lim);
        self.nearest2 = self.nearest2.min(dx * dx + dy * dy + dz * dz);
    }

    /// Roll for a click. `rng` is drawn once, and a second time only when the
    /// first draw does not click (also when nothing is in range).
    pub fn sample(self, rng: &mut dyn FnMut() -> u32) -> Option<GeigerClick> {
        let range = psx_math::int32::isqrt_i32(self.nearest2);
        let chance: u32 = match range {
            601..=800 => 2,
            501..=600 => 4,
            301..=500 => 8,
            201..=300 => 28,
            151..=200 => 40,
            101..=150 => 60,
            76..=100 => 80,
            51..=75 => 90,
            0..=50 => 95,
            _ => 0,
        };
        if (rng() & 127) < chance || (rng() & 127) < chance {
            Some(GeigerClick {
                volume_den: if range > 400 {
                    3
                } else if range > 150 {
                    2
                } else {
                    1
                },
            })
        } else {
            None
        }
    }
}
