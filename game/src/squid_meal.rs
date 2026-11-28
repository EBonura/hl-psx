//! The bullsquid's meals: `SCHED_SQUID_EAT` and `SCHED_SQUID_SNIFF_AND_EAT`.
//!
//! A monster that finishes dying leaves a carcass scent for 30 s within 384 units
//! (`bits_SOUND_CARCASS`, inserted when its death task ends). A hungry bullsquid with no
//! enemy that smells one goes to it: straight there when the food is in its view cone and in
//! sight, else after the `sniff` sequence (ACT_DETECT_SCENT). It walks to the food, plays
//! `eat` three times (ACT_EAT) and is not hungry again for 50 s (TASK_EAT 50). Setting off
//! already counts as eating for 10 s (TASK_EAT 10), so a squid that cannot reach the food
//! does not try again at once. Damage or an enemy breaks the meal (the caller's reset).

/// Carcass scent radius and lifetime (`CSoundEnt::InsertSound(bits_SOUND_CARCASS, .., 384, 30)`).
pub const SMELL_RANGE: i32 = 384;
pub const CARCASS_TICKS: u16 = 600;
/// sniff: 57 frames at 15 fps.
pub const SNIFF_TICKS: u16 = 75;
/// eat, three times: 101 frames at 30 fps each.
pub const EAT_TICKS: u16 = 200;
/// TASK_EAT 50 after the meal, TASK_EAT 10 when it sets off.
pub const FULL_TICKS: u16 = 1_000;
pub const SET_OFF_TICKS: u16 = 200;
/// A walk that has not reached the food by now gives up.
pub const WALK_MAX_TICKS: u16 = 200;
/// Close enough to eat: the squid's mouth over the food, its body against the carcass's.
pub const REACH: i32 = 96;
/// The walk sequence's speed: 256 units over 149 frames at 30 fps, about 51 u/s.
pub const WALK_SPEED: i32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    None,
    Sniff,
    Walk,
    Eat,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Meal {
    pub phase: Phase,
    pub ticks: u16,
    /// The tick the squid is hungry again (`m_flHungryTime`).
    pub hungry_at: u16,
    pub food: [i32; 3],
}

impl Meal {
    pub const NONE: Meal = Meal {
        phase: Phase::None,
        ticks: 0,
        hungry_at: 0,
        food: [0; 3],
    };
}

/// Whether tick `now` has reached `at`, across the u16 wrap (within half the range).
pub const fn reached(now: u16, at: u16) -> bool {
    now.wrapping_sub(at) < 0x8000
}

/// One tick of a meal. `scent` is the nearest carcass in smell range and whether it is in
/// the view cone and in sight; `at_food` whether the squid has reached its food.
pub const fn step(m: Meal, now: u16, scent: Option<([i32; 3], bool)>, at_food: bool) -> Meal {
    let ticks = m.ticks.saturating_add(1);
    match m.phase {
        Phase::None => match scent {
            Some((food, seen)) if reached(now, m.hungry_at) => Meal {
                phase: if seen { Phase::Walk } else { Phase::Sniff },
                ticks: 0,
                hungry_at: now.wrapping_add(SET_OFF_TICKS),
                food,
            },
            _ => m,
        },
        Phase::Sniff if ticks >= SNIFF_TICKS => Meal {
            phase: Phase::Walk,
            ticks: 0,
            ..m
        },
        Phase::Walk if at_food => Meal {
            phase: Phase::Eat,
            ticks: 0,
            ..m
        },
        // The path fails: the meal schedule fails, and TASK_EAT 10 keeps it from setting off
        // again for ten seconds.
        Phase::Walk if ticks >= WALK_MAX_TICKS => Meal {
            phase: Phase::None,
            ticks: 0,
            hungry_at: now.wrapping_add(SET_OFF_TICKS),
            ..m
        },
        Phase::Eat if ticks >= EAT_TICKS => Meal {
            phase: Phase::None,
            ticks: 0,
            hungry_at: now.wrapping_add(FULL_TICKS),
            ..m
        },
        _ => Meal { ticks, ..m },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seen_carcass_is_walked_to_and_eaten_then_the_squid_is_full() {
        let food = [100, 0, 0];
        let mut m = step(Meal::NONE, 10, Some((food, true)), false);
        assert_eq!((m.phase, m.food), (Phase::Walk, food));
        m = step(m, 11, Some((food, true)), true);
        assert_eq!(m.phase, Phase::Eat);
        let mut now = 11;
        while m.phase == Phase::Eat {
            now += 1;
            m = step(m, now, Some((food, true)), true);
        }
        assert_eq!(now, 11 + EAT_TICKS);
        assert_eq!(m.hungry_at, now + FULL_TICKS);
        // Full: the same carcass is left alone.
        assert_eq!(
            step(m, now + 1, Some((food, true)), false).phase,
            Phase::None
        );
        assert_eq!(
            step(m, now + FULL_TICKS, Some((food, true)), false).phase,
            Phase::Walk
        );
    }

    #[test]
    fn an_unseen_carcass_is_sniffed_first_and_an_unreachable_one_given_up() {
        let mut m = step(Meal::NONE, 0, Some(([0; 3], false)), false);
        assert_eq!(m.phase, Phase::Sniff);
        for now in 1..=SNIFF_TICKS {
            m = step(m, now, None, false);
        }
        assert_eq!(m.phase, Phase::Walk);
        for now in 0..WALK_MAX_TICKS {
            m = step(m, SNIFF_TICKS + 1 + now, None, false);
        }
        assert_eq!(m.phase, Phase::None);
        // A failed walk holds the squid off for ten seconds (TASK_EAT 10).
        assert_eq!(m.hungry_at, SNIFF_TICKS + WALK_MAX_TICKS + SET_OFF_TICKS);
        assert!(reached(65_535, 65_530) && !reached(65_530, 2));
    }
}
