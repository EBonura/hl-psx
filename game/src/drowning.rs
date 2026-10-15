//! Drowning, as `CBasePlayer::WaterMove` and `CheckTimeBasedDamage` run it.
//!
//! The head under water spends a 12 s lungful (AIRTIME). When it is gone the
//! player takes `pev->dmg` straight to health (the suit does not absorb
//! DMG_DROWN), once at the moment the air runs out and then every second.
//! `dmg` is held at 2 whenever the head is out and gains one before each hit,
//! so the hits are 3, 4, 5, 5... Every point taken is remembered
//! (`m_idrowndmg`); on the surface it comes back, up to 10 points at each
//! two-second pass of `CheckTimeBasedDamage` (itbd_DrownRecover).

/// AIRTIME 12 s at the 20 Hz simulation.
pub const AIR_TICKS: u16 = 240;
/// `pain_finished = time + 1` between drowning hits.
const HURT_TICKS: u8 = 20;
/// CheckTimeBasedDamage only runs when two seconds have passed.
const RECOVER_TICKS: u8 = 40;
/// `pev->dmg` after the head was out of the water, before the first `+= 1`.
const FIRST_DAMAGE: u8 = 3;
const MAX_DAMAGE: u8 = 5;
/// TakeHealth in itbd_DrownRecover: `Q_min(m_idrowndmg - m_idrownrestored, 10)`.
const RECOVER_MAX: u16 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    None,
    /// Drowning damage to take this tick.
    Hurt(u8),
    /// Health given back this tick.
    Heal(u8),
}

pub struct Drowning {
    /// Air left, in ticks (the HUD bar).
    pub air: u16,
    damage: u8,
    tick: u8,
    taken: u16,
    under: bool,
}

impl Drowning {
    pub const fn new() -> Self {
        Self {
            air: AIR_TICKS,
            damage: FIRST_DAMAGE,
            tick: 0,
            taken: 0,
            under: false,
        }
    }

    /// One simulation tick. `eye_under` is the water level 3 test; a dead
    /// player never recovers (pass `alive = false`).
    pub fn step(&mut self, eye_under: bool, alive: bool) -> Event {
        if eye_under {
            self.under = true;
            if self.air > 0 {
                self.air -= 1;
                self.tick = 0;
                return Event::None;
            }
            let mut event = Event::None;
            if self.tick == 0 {
                self.tick = HURT_TICKS;
                let damage = self.damage.min(MAX_DAMAGE);
                self.damage = self.damage.saturating_add(1);
                self.taken = self.taken.saturating_add(damage as u16);
                event = Event::Hurt(damage);
            }
            self.tick -= 1;
            return event;
        }
        self.air = AIR_TICKS;
        self.damage = FIRST_DAMAGE;
        if self.under {
            // The two-second recovery pass counts from the breath.
            self.under = false;
            self.tick = 0;
        }
        if self.taken > 0 && alive {
            self.tick += 1;
            if self.tick >= RECOVER_TICKS {
                self.tick = 0;
                let back = self.taken.min(RECOVER_MAX);
                self.taken -= back;
                return Event::Heal(back as u8);
            }
        } else {
            self.tick = 0;
        }
        Event::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn under(d: &mut Drowning, ticks: u32) -> Vec<(u32, Event)> {
        let mut out = Vec::new();
        for t in 1..=ticks {
            let e = d.step(true, true);
            if e != Event::None {
                out.push((t, e));
            }
        }
        out
    }

    #[test]
    fn hits_are_3_4_5_5_from_the_moment_twelve_seconds_run_out() {
        let mut d = Drowning::new();
        let hits = under(&mut d, 240 + 20 * 3 + 1);
        // 240 ticks of air, then the first hit on the next tick (player.cpp
        // WaterMove: air_finished < time with pain_finished long past), then
        // one every second.
        assert_eq!(
            hits,
            vec![
                (241, Event::Hurt(3)),
                (261, Event::Hurt(4)),
                (281, Event::Hurt(5)),
                (301, Event::Hurt(5)),
            ]
        );
        // After 16 s the original has taken 3+4+5+5 = 17.
    }

    #[test]
    fn a_breath_resets_the_air_and_the_damage_ramp() {
        let mut d = Drowning::new();
        under(&mut d, 300);
        assert_eq!(d.step(false, true), Event::None);
        assert_eq!(d.air, AIR_TICKS);
        assert_eq!(under(&mut d, 242).first(), Some(&(241, Event::Hurt(3))));
    }

    #[test]
    fn the_surface_returns_up_to_ten_points_every_two_seconds() {
        let mut d = Drowning::new();
        under(&mut d, 240 + 20 * 5 + 1); // 3 + 4 + 5 + 5 + 5 + 5 = 27 taken
        let mut back = Vec::new();
        for t in 1..=200 {
            if let Event::Heal(n) = d.step(false, true) {
                back.push((t, n));
            }
        }
        assert_eq!(back, vec![(40, 10), (80, 10), (120, 7)]);
    }

    #[test]
    fn diving_again_stops_the_recovery_and_a_dead_player_gets_nothing() {
        let mut d = Drowning::new();
        under(&mut d, 241);
        for _ in 0..39 {
            assert_eq!(d.step(false, true), Event::None);
        }
        d.step(true, true);
        for _ in 0..39 {
            assert_eq!(d.step(false, true), Event::None);
        }
        let mut dead = Drowning::new();
        under(&mut dead, 241);
        for _ in 0..200 {
            assert_eq!(dead.step(false, false), Event::None);
        }
    }
}
