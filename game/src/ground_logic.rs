//! This port's placed-prop policy over the shared GoldSrc ground rules.
//!
//! The movement, slide and actor-sweep arithmetic lives in
//! `psx_goldsrc::ground_logic`; only which prop kinds drop to the floor and
//! which take the solid-world occlusion probe belong to the game.

pub use psx_goldsrc::ground_logic::*;

/// Select GoldSrc's map-load floor handling for a placed prop kind.
pub const fn prop_spawn_floor_mode(kind: u8, is_item: bool) -> u8 {
    if is_item {
        SPAWN_FLOOR_DEEP
    } else if matches!(
        kind,
        // scientist, Barney, headcrab, terrestrial hostiles, roach/G-Man,
        // grounded bosses, assassin and scripted humans.
        0 | 1 | 2 | 5 | 6 | 7 | 8 | 9 | 10 | 14 | 15 | 16 | 18 | 51 | 52 | 53 | 54 | 55 | 56
    ) {
        SPAWN_FLOOR_STANDARD
    } else {
        SPAWN_FLOOR_AUTHORED
    }
}

/// Whether an actor should be rejected by the port's render-only solid-world
/// line-of-sight optimization.
///
/// Hazard Course holograms are deliberately presented through display glass,
/// grates and narrow projector openings. GoldSrc does not ray-occlude studio
/// actors this way, so applying the optimization to type 56 makes a visible
/// Holo pop out as the camera ray crosses one of those brushes. PVS and sphere
/// culling still constrain it to the visible part of the room.
/// Dispenser cans are only a few units tall. The actor probe samples above
/// their visible mesh and hits the dispensing recess even when the can itself
/// is visible. Retain PVS/sphere rejection and normal GPU ordering for them.
pub const fn prop_uses_occlusion_probe(kind: u8) -> bool {
    kind != 56 && kind != 75
}

#[cfg(test)]
mod tests {
    use super::{
        prop_spawn_floor_mode, prop_uses_occlusion_probe, SPAWN_FLOOR_AUTHORED, SPAWN_FLOOR_DEEP,
    };

    #[test]
    fn sitting_scientists_and_hev_suits_drop_to_authored_supports() {
        assert_eq!(
            prop_spawn_floor_mode(25, false),
            SPAWN_FLOOR_AUTHORED,
            "the cooker preserves CSittingScientist short-hull support"
        );
        assert_eq!(prop_spawn_floor_mode(4, true), SPAWN_FLOOR_DEEP);
        assert_eq!(prop_spawn_floor_mode(3, true), SPAWN_FLOOR_DEEP);
        assert_eq!(prop_spawn_floor_mode(11, false), SPAWN_FLOOR_AUTHORED);
        assert_eq!(prop_spawn_floor_mode(12, false), SPAWN_FLOOR_AUTHORED);
    }

    #[test]
    fn holograms_and_dispenser_cans_bypass_the_solid_world_occlusion_optimization() {
        assert!(!prop_uses_occlusion_probe(56));
        assert!(!prop_uses_occlusion_probe(75));
        assert!(prop_uses_occlusion_probe(0));
        assert!(prop_uses_occlusion_probe(1));
    }
}
