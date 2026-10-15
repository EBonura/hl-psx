// SPDX-License-Identifier: GPL-2.0-or-later
//! Small rules of the map trigger entities, as arithmetic the host tests can
//! run. The game supplies the world; these decide who is touched, how often
//! and what a flag means.

// ------------------------------------------------------------ firing chains

/// How deep a zero-delay chain of fired targets may go. The original calls
/// straight through (a trigger fires an entity that fires another, without
/// limit); the port recurses on the PS1 stack, and a chain hop costs two depth
/// counts (one in the target scan, one in the entity's own use). The deepest
/// chain the inventory found is the c3a1 snark nests (trigger, breakable,
/// breakable, breakable, monstermaker), so twelve hops leave room to spare
/// while still stopping a map that fires itself in a loop.
pub const MAX_FIRE_DEPTH: u8 = 24;

// ------------------------------------------------------------- globalname

/// Entities that carry a `globalname` and are removed (a breakable that broke)
/// are remembered as dead, so they are not spawned again when the map is
/// entered a second time. Only the dead are kept, not every global entity, so
/// a few slots suffice; when they run out the newest are not remembered.
#[derive(Clone, Copy)]
pub struct DeadGlobals<const N: usize> {
    hashes: [u16; N],
    count: u8,
}

impl<const N: usize> DeadGlobals<N> {
    pub const fn new() -> Self {
        Self { hashes: [0; N], count: 0 }
    }
    pub fn contains(&self, hash: u16) -> bool {
        hash != 0 && self.hashes[..self.count as usize].contains(&hash)
    }
    pub fn kill(&mut self, hash: u16) {
        if hash != 0 && !self.contains(hash) && (self.count as usize) < N {
            self.hashes[self.count as usize] = hash;
            self.count += 1;
        }
    }
    pub fn clear(&mut self) {
        self.count = 0;
    }
}

// ----------------------------------------------------------- trigger_multiple

/// What a trigger_multiple does after it fires: wait that long before it may
/// fire again (`Some`), or, with a negative wait, never fire again (`None`).
/// The cook has already replaced an authored zero wait by 0.2 s.
pub const fn multiple_after_fire(wait_ticks: i16) -> Option<u16> {
    if wait_ticks < 0 {
        None
    } else {
        Some(wait_ticks as u16)
    }
}

// ----------------------------------------------------------------- env_shake

/// UTIL_ScreenShake: a shake reaches only a player standing on the ground, and
/// when the volume has a radius only one inside it; radius zero means anyone.
/// `distance_sq` and `radius` share a unit.
pub const fn shake_reaches(on_ground: bool, radius: i32, distance_sq: i64) -> bool {
    on_ground && (radius <= 0 || distance_sq < (radius as i64) * (radius as i64))
}

// ---------------------------------------------------------------- env_spark

pub const SF_SPARK_TOGGLE: u16 = 32;
pub const SF_SPARK_START_ON: u16 = 64;
/// `MaxDelay` default: 1.5 s.
pub const SPARK_DEFAULT_MAX_DELAY_TICKS: u16 = 30;

/// A spark sparks from the start unless it is a toggled one that spawns off.
pub const fn spark_starts_running(spawnflags: u16) -> bool {
    spawnflags & SF_SPARK_TOGGLE == 0 || spawnflags & SF_SPARK_START_ON != 0
}

/// What a Use does to a spark: a toggled one flips between running and
/// stopped (whatever the use type); an untoggled one ignores Use.
pub const fn spark_after_use(spawnflags: u16, running: bool) -> bool {
    if spawnflags & SF_SPARK_TOGGLE == 0 {
        running
    } else {
        !running
    }
}

/// Ticks to the next spark: 0.1 s plus a uniform draw from zero to `MaxDelay`
/// (`roll` is a uniform draw below `max_delay_ticks + 1`).
pub const fn spark_gap_ticks(roll: u16) -> u16 {
    2 + roll
}

// ------------------------------------------------------------- trigger_hurt

pub const SF_HURT_TARGET_ONCE: u16 = 1;
pub const SF_HURT_START_OFF: u16 = 2;
pub const SF_HURT_NO_CLIENTS: u16 = 8;
pub const SF_HURT_CLIENT_ONLY_FIRE: u16 = 16;
pub const SF_HURT_CLIENT_ONLY_TOUCH: u16 = 32;

/// A hurt volume damages anything that can take damage that touches it, with
/// two flags: `NO_CLIENTS` spares the player, `CLIENT_ONLY_TOUCH` spares
/// everything else.
pub const fn hurt_touches(spawnflags: u16, is_player: bool) -> bool {
    if is_player {
        spawnflags & SF_HURT_NO_CLIENTS == 0
    } else {
        spawnflags & SF_HURT_CLIENT_ONLY_TOUCH == 0
    }
}

/// The volume's target fires for each toucher it hurts, except that
/// `CLIENT_ONLY_FIRE` keeps it for the player.
pub const fn hurt_fires_target(spawnflags: u16, is_player: bool) -> bool {
    is_player || spawnflags & SF_HURT_CLIENT_ONLY_FIRE == 0
}

/// A hurt pulse is the volume's per-second damage for half a second.
pub const HURT_REPEAT_TICKS: u16 = 10;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_shipped_chain_fits_with_room_to_spare() {
        // c3a1: trigger_once -> breakable -> breakable -> breakable -> monstermaker
        let snark_nest_hops = 4;
        assert!(MAX_FIRE_DEPTH / 2 >= 2 * snark_nest_hops);
    }

    #[test]
    fn a_dead_global_stays_dead_until_cleared() {
        let mut dead = DeadGlobals::<4>::new();
        assert!(!dead.contains(77));
        dead.kill(77);
        dead.kill(77);
        assert!(dead.contains(77));
        // hash zero means "no globalname" and is never dead
        dead.kill(0);
        assert!(!dead.contains(0));
        // a full list drops the newest
        for h in 1..=10 {
            dead.kill(h);
        }
        assert!(dead.contains(77) && dead.contains(1) && dead.contains(2) && dead.contains(3));
        assert!(!dead.contains(10));
        dead.clear();
        assert!(!dead.contains(77));
    }

    #[test]
    fn a_multiple_with_a_negative_wait_fires_once() {
        assert_eq!(multiple_after_fire(-20), None);
        assert_eq!(multiple_after_fire(4), Some(4));
        assert_eq!(multiple_after_fire(0), Some(0));
    }

    #[test]
    fn shakes_reach_grounded_players_inside_the_radius() {
        assert!(shake_reaches(true, 0, 9_999_999));
        assert!(!shake_reaches(false, 0, 0));
        assert!(shake_reaches(true, 500, 499 * 499));
        assert!(!shake_reaches(true, 500, 500 * 500));
        assert!(!shake_reaches(false, 500, 1));
    }

    #[test]
    fn sparks_start_and_toggle_by_their_flags() {
        // plain sparks run from the start and ignore Use
        assert!(spark_starts_running(0));
        assert!(spark_after_use(0, true));
        // toggled and not START_ON: silent until used
        assert!(!spark_starts_running(SF_SPARK_TOGGLE));
        assert!(spark_after_use(SF_SPARK_TOGGLE, false));
        assert!(!spark_after_use(SF_SPARK_TOGGLE, true));
        // toggled and START_ON: running, and the first Use stops it
        assert!(spark_starts_running(SF_SPARK_TOGGLE | SF_SPARK_START_ON));
        assert!(!spark_after_use(SF_SPARK_TOGGLE | SF_SPARK_START_ON, true));
    }

    #[test]
    fn sparks_gap_is_a_tenth_of_a_second_plus_up_to_the_max_delay() {
        assert_eq!(spark_gap_ticks(0), 2);
        assert_eq!(spark_gap_ticks(SPARK_DEFAULT_MAX_DELAY_TICKS), 32);
        // mean over a uniform roll is 0.85 s for the default 1.5 s
        let sum: u32 = (0..=SPARK_DEFAULT_MAX_DELAY_TICKS).map(|r| spark_gap_ticks(r) as u32).sum();
        let mean_ticks = sum as f32 / (SPARK_DEFAULT_MAX_DELAY_TICKS as f32 + 1.0);
        assert!((mean_ticks - 17.0).abs() < 0.01, "{mean_ticks}");
    }

    #[test]
    fn plain_hurt_volumes_touch_the_player_and_monsters() {
        assert!(hurt_touches(0, true));
        assert!(hurt_touches(0, false));
        assert!(hurt_touches(SF_HURT_TARGET_ONCE | SF_HURT_START_OFF, false));
    }

    #[test]
    fn no_clients_spares_only_the_player() {
        assert!(!hurt_touches(SF_HURT_NO_CLIENTS, true));
        assert!(hurt_touches(SF_HURT_NO_CLIENTS, false));
    }

    #[test]
    fn client_only_touch_spares_monsters() {
        assert!(hurt_touches(SF_HURT_CLIENT_ONLY_TOUCH, true));
        assert!(!hurt_touches(SF_HURT_CLIENT_ONLY_TOUCH, false));
    }

    #[test]
    fn client_only_fire_keeps_the_target_for_the_player() {
        assert!(hurt_fires_target(SF_HURT_CLIENT_ONLY_FIRE, true));
        assert!(!hurt_fires_target(SF_HURT_CLIENT_ONLY_FIRE, false));
        assert!(hurt_fires_target(0, false));
    }
}
