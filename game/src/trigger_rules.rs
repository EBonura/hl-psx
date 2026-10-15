// SPDX-License-Identifier: GPL-2.0-or-later
//! Small rules of the map trigger entities, as arithmetic the host tests can
//! run. The game supplies the world; these decide who is touched, how often
//! and what a flag means.

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
