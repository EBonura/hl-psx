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
///
/// With the suit charged, a fifth of every hit goes straight to health and
/// the other four fifths are soaked up by the armour at one armour point per
/// two points of damage. Armour that runs dry soaks what it can and the rest
/// lands on health. Falls skip the suit altogether.
pub fn apply_player_damage(v: Vitals, dmg: u16, kind: PlayerDamageKind) -> Vitals {
    let dmg = u32::from(dmg);
    let (health_loss, armor_loss) = match kind {
        PlayerDamageKind::Fall => (dmg, 0),
        PlayerDamageKind::Generic => {
            let suit_share = dmg - dmg / 5;
            let armor = u32::from(v.armor);
            // What the armour could soak is capped by what it holds.
            let soaked = suit_share.min(armor * 2);
            (dmg - soaked, soaked.div_ceil(2))
        }
    };
    Vitals {
        health: u32::from(v.health).saturating_sub(health_loss) as u16,
        armor: (u32::from(v.armor) - armor_loss) as u16,
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
    ///
    /// A source within 50 units lights every arrow. Otherwise an arrow lights
    /// when the source lies on its side of the view and its share of the
    /// full 3D distance on that axis is more than 0.3, so a source straight
    /// above lights nothing and a diagonal one lights two.
    pub fn note_hit(
        &mut self,
        player: [i32; 3],
        forward: [i16; 3],
        right: [i16; 3],
        source: [i32; 3],
    ) {
        self.ticks = 10;
        let mut gap = [0i32; 3];
        for axis in 0..3 {
            gap[axis] = source[axis].saturating_sub(player[axis]);
        }
        // Only the direction matters at long range: halve the gap until the
        // squared distances below fit comfortably.
        let mut scaled = false;
        while gap.iter().any(|g| g.abs() > 1023) {
            gap = gap.map(|g| g / 2);
            scaled = true;
        }
        let dist_sq = gap.iter().map(|g| g * g).sum::<i32>();
        if !scaled && dist_sq <= 50 * 50 {
            self.edges = COMPASS_FRONT | COMPASS_RIGHT | COMPASS_REAR | COMPASS_LEFT;
            return;
        }
        // Squared "share of the distance is above 0.3" for a signed distance
        // along an axis: (10 * along)^2 > 9 * dist^2.
        let strong = |along: i32| (10 * along) * (10 * along) > 9 * dist_sq;
        let ahead = dot_q12(forward, gap);
        let beside = dot_q12(right, gap);
        if strong(ahead) {
            self.edges |= if ahead > 0 {
                COMPASS_FRONT
            } else {
                COMPASS_REAR
            };
        }
        if strong(beside) {
            self.edges |= if beside > 0 {
                COMPASS_RIGHT
            } else {
                COMPASS_LEFT
            };
        }
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

/// Squared distance of the quiet range, past which the counter stays silent.
const GEIGER_RANGE_SQ: i32 = GEIGER_RANGE * GEIGER_RANGE;

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
        Self { nearest2: i32::MAX }
    }

    /// Note one radiation volume; only the closest one counts.
    pub fn add_source(&mut self, player: [i32; 3], centre: [i32; 3]) {
        // Anything past the range is silent, so clamp each axis to keep the
        // squared sum from overflowing.
        let mut sq = 0;
        for axis in 0..3 {
            let gap = (centre[axis] - player[axis]).clamp(-1000, 1000);
            sq += gap * gap;
        }
        self.nearest2 = self.nearest2.min(sq);
    }

    /// Roll for a click against the closest source. Consumes one random draw
    /// when something is in range and none otherwise.
    pub fn sample(self, rng: &mut dyn FnMut() -> u32) -> Option<GeigerClick> {
        if self.nearest2 > GEIGER_RANGE_SQ {
            return None;
        }
        if (rng() & 127) as i32 >= click_chance(self.nearest2) {
            return None;
        }
        let volume_den = if self.nearest2 <= 150 * 150 {
            1
        } else if self.nearest2 <= 400 * 400 {
            2
        } else {
            3
        };
        Some(GeigerClick { volume_den })
    }
}

/// Chance, out of 128, that a sample clicks when the nearest source is
/// `dist2` (squared units) away: about 1 in 64 at the edge of the range,
/// climbing along a cubic ease to nearly every sample at point blank.
fn click_chance(dist2: i32) -> i32 {
    // 0 at the edge of the range, 128 on top of the source.
    let closeness = (GEIGER_RANGE_SQ - dist2) / 5000;
    2 + ((125 * closeness * closeness * closeness) >> 21)
}

/// While the lamp is on the battery loses one unit every this many 20 Hz
/// ticks (1.2 s).
pub const FLASH_DRAIN_TICKS: u8 = 24;
/// While the lamp is off the battery gains one unit every this many 20 Hz
/// ticks (0.2 s).
pub const FLASH_CHARGE_TICKS: u8 = 4;

/// The suit lamp: on or off, battery 0..=100, and ticks to the next battery
/// step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Flashlight {
    pub on: bool,
    pub battery: u8,
    pub timer: u8,
}

impl Flashlight {
    /// A new game shows 99 and steps to 100 on the first tick.
    pub const NEW_GAME: Self = Self {
        on: false,
        battery: 99,
        timer: 1,
    };
}

/// What the player does to the lamp this tick.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlashlightInput {
    /// The flashlight button went down this tick.
    pub toggle_pressed: bool,
    pub has_suit: bool,
    pub dead: bool,
}

/// Results of one lamp tick.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlashlightTick {
    /// Play the lamp switch click (a toggle or the automatic switch-off).
    pub click: bool,
    /// The lamp actually lights the world this tick.
    pub lit: bool,
}

/// One 20 Hz tick of the suit lamp: death switches it off, the battery steps,
/// an empty battery switches it off with a click, then the button toggles it
/// (needs the suit; switching on needs charge).
#[inline(never)]
pub fn flashlight_tick(s: Flashlight, input: FlashlightInput) -> (Flashlight, FlashlightTick) {
    let mut on = s.on;
    let mut timer = s.timer;
    if input.dead && on {
        on = false;
        timer = FLASH_CHARGE_TICKS;
    }
    let (mut on, battery, mut timer, auto_off) = battery_step(on, s.battery, timer);
    let mut click = auto_off;
    if input.toggle_pressed && input.has_suit && !input.dead {
        if on {
            on = false;
            timer = FLASH_CHARGE_TICKS;
            click = true;
        } else if battery > 0 {
            on = true;
            timer = FLASH_DRAIN_TICKS;
            click = true;
        }
    }
    (
        Flashlight { on, battery, timer },
        FlashlightTick {
            click,
            lit: on && input.has_suit && !input.dead,
        },
    )
}

/// The battery clock, by value: keeping the three byte-wide fields out of
/// adjacent mutable references avoids a bad MIPS-I codegen alias on the
/// experimental target. The final bool reports automatic shutoff.
#[inline(never)]
fn battery_step(on: bool, mut battery: u8, mut timer: u8) -> (bool, u8, u8, bool) {
    if timer > 0 {
        timer -= 1;
        if timer > 0 {
            return (on, battery, timer, false);
        }
    }
    if on {
        if battery > 0 {
            battery -= 1;
        }
        if battery == 0 {
            return (false, 0, FLASH_CHARGE_TICKS, true);
        }
        timer = FLASH_DRAIN_TICKS;
    } else if battery < 100 {
        battery += 1;
        timer = if battery < 100 { FLASH_CHARGE_TICKS } else { 0 };
    }
    (on, battery, timer, false)
}

/// Tau Cannon secondary fire timings in 20 Hz ticks.
pub const TAU_FULL_CHARGE_TICKS: u8 = 80; // 4 s
pub const TAU_SPIN_TICKS: u8 = 10; // 0.5 s: the spin-up gives way to the spin loop
pub const TAU_CELL_TICKS: u8 = 6; // 0.3 s per uranium cell while charging
pub const TAU_OVERCHARGE_TICKS: u8 = 200; // 10 s, then it discharges into the player
/// Damage the player takes from an overcharge.
pub const TAU_OVERCHARGE_DAMAGE: u16 = 50;
/// Cooldown after a dry click with no cells.
pub const TAU_DRY_COOLDOWN_TICKS: u8 = 4;

/// The Tau Cannon's charge state plus the weapon fields it touches.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TauState {
    /// 0 when idle, otherwise ticks since the charge began.
    pub age: u8,
    /// Weapon cooldown in ticks.
    pub cooldown: u8,
    /// Uranium cells in reserve.
    pub cells: u16,
}

/// What the Tau Cannon sees this tick.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TauInput {
    /// The Tau Cannon is the current weapon.
    pub selected: bool,
    /// Secondary fire is held.
    pub held: bool,
    /// The player's eye is under water.
    pub underwater: bool,
    /// A weapon switch or reload is still in progress.
    pub busy: bool,
}

/// What happened this tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TauEvent {
    Idle,
    /// Tried to start with no cells: dry click.
    Dry,
    /// The charge began (one cell spent).
    Start,
    /// The spin-up became the spin loop.
    Spin,
    /// The charge was released or forced out: fire a shot of `damage`.
    Fire {
        damage: u8,
    },
    /// Charging under water discharges harmlessly with a zap.
    Discharge,
    /// Held too long: the charge hurts the player for
    /// `TAU_OVERCHARGE_DAMAGE`.
    Overcharge,
}

/// One 20 Hz tick of the Tau Cannon's hold-to-charge secondary fire.
pub fn tau_secondary_tick(s: &mut TauState, input: TauInput) -> TauEvent {
    if !input.selected {
        s.age = 0;
        return TauEvent::Idle;
    }
    if input.underwater && (input.held || s.age != 0) {
        s.age = 0;
        s.cooldown = 10;
        return TauEvent::Discharge;
    }
    if input.held {
        if s.age == 0 {
            if s.cooldown != 0 || input.busy {
                return TauEvent::Idle;
            }
            if s.cells == 0 {
                s.cooldown = TAU_DRY_COOLDOWN_TICKS;
                return TauEvent::Dry;
            }
            s.cells -= 1;
            s.age = 1;
            return TauEvent::Start;
        }
        s.age = s.age.saturating_add(1);
        let age = s.age;
        if age < TAU_FULL_CHARGE_TICKS && age % TAU_CELL_TICKS == 0 {
            if s.cells == 0 {
                return tau_release(s, age);
            }
            s.cells -= 1;
            if s.cells == 0 {
                return tau_release(s, age);
            }
        }
        if age >= TAU_OVERCHARGE_TICKS {
            s.age = 0;
            s.cooldown = 20;
            return TauEvent::Overcharge;
        }
        return if age == TAU_SPIN_TICKS {
            TauEvent::Spin
        } else {
            TauEvent::Idle
        };
    }
    if s.age != 0 {
        let age = s.age;
        return tau_release(s, age);
    }
    TauEvent::Idle
}

fn tau_release(s: &mut TauState, age: u8) -> TauEvent {
    let damage = (200u16 * age.min(TAU_FULL_CHARGE_TICKS) as u16 / TAU_FULL_CHARGE_TICKS as u16)
        .max(1) as u8;
    s.age = 0;
    s.cooldown = 20;
    TauEvent::Fire { damage }
}

/// Horizontal shove (world units per tick) a charged shot of `damage` gives
/// the player, backwards along the view `forward` axis (1.0 = 4096).
pub fn tau_shove(damage: u8, forward: [i16; 3]) -> [i32; 3] {
    let shove = -(damage as i32) * 5 / 20;
    [
        (forward[0] as i32 * shove) >> 12,
        0,
        (forward[2] as i32 * shove) >> 12,
    ]
}
