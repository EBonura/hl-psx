//! This port's placed-prop policy over the shared GoldSrc ground rules.
//!
//! The movement, slide and actor-sweep arithmetic lives in
//! `psx_goldsrc::ground_logic`; only which prop kinds drop to the floor and
//! which take the solid-world occlusion probe belong to the game.

pub use psx_goldsrc::ground_logic::*;

/// How far the contact recovery search may reach from a buried hull origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryReach {
    /// Only the engine's contact offsets, at most 2 units away.
    Local,
    /// The local offsets, then rings out to [`WIDE_RECOVERY_RADIUS_MAX`] units.
    Wide,
}

/// Smallest radius of the wide recovery rings; smaller offsets are the local ones.
pub const WIDE_RECOVERY_RADIUS_MIN: i32 = 3;

/// Largest radius of the wide recovery rings, in world units. It stays under
/// the 33 units of any pre-expanded clip wall, so a hop never lands on the far
/// side of one.
pub const WIDE_RECOVERY_RADIUS_MAX: i32 = 18;

/// The engine's `CONTACT_RECOVERY_OFFSETS`, repeated here in one byte per axis. The
/// game ships no other copy of that table, and this one is a quarter of its size.
const LOCAL_RECOVERY_OFFSETS: [[i8; 3]; 14] = [
    [0, 1, 0],
    [0, 2, 0],
    [1, 0, 0],
    [-1, 0, 0],
    [0, 0, 1],
    [0, 0, -1],
    [2, 0, 0],
    [-2, 0, 0],
    [0, 0, 2],
    [0, 0, -2],
    [1, 1, 0],
    [-1, 1, 0],
    [0, 1, 1],
    [0, 1, -1],
];

/// Directions of one wide ring, nearest first: the six axes (up, the
/// horizontal pair, then down), then the twelve edge diagonals.
const WIDE_RECOVERY_DIRECTIONS: [[i8; 3]; 18] = [
    [0, 1, 0],
    [1, 0, 0],
    [-1, 0, 0],
    [0, 0, 1],
    [0, 0, -1],
    [0, -1, 0],
    [1, 1, 0],
    [-1, 1, 0],
    [0, 1, 1],
    [0, 1, -1],
    [1, 0, 1],
    [1, 0, -1],
    [-1, 0, 1],
    [-1, 0, -1],
    [1, -1, 0],
    [-1, -1, 0],
    [0, -1, 1],
    [0, -1, -1],
];

/// The `index`th offset to try when a hull origin is buried in solid, nearest
/// first, or `None` once `reach` is exhausted.
///
/// The engine's contact offsets only reach two units, and a player
/// that sank four to six units into a clip slab (a vent lip met while an actor
/// pushed the origin sideways) stayed frozen there for good: every slide leg
/// started solid and gave up. [`RecoveryReach::Wide`] continues past them with
/// rings of growing radius, so the search costs at most
/// `14 + 16 * 18` clearance tests and only while the hull is stuck. Integer
/// arithmetic only.
#[inline(always)]
#[optimize(size)]
pub fn recovery_offset(index: usize, reach: RecoveryReach) -> Option<[i32; 3]> {
    if let Some(offset) = LOCAL_RECOVERY_OFFSETS.get(index) {
        return Some(offset.map(i32::from));
    }
    if reach == RecoveryReach::Local {
        return None;
    }
    let wide = index - LOCAL_RECOVERY_OFFSETS.len();
    let ring = wide / WIDE_RECOVERY_DIRECTIONS.len();
    let radius = WIDE_RECOVERY_RADIUS_MIN + ring as i32;
    if radius > WIDE_RECOVERY_RADIUS_MAX {
        return None;
    }
    let direction = WIDE_RECOVERY_DIRECTIONS[wide % WIDE_RECOVERY_DIRECTIONS.len()];
    Some([
        radius * i32::from(direction[0]),
        radius * i32::from(direction[1]),
        radius * i32::from(direction[2]),
    ])
}

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
        prop_spawn_floor_mode, prop_uses_occlusion_probe, recovery_offset, RecoveryReach,
        CONTACT_RECOVERY_OFFSETS, SPAWN_FLOOR_AUTHORED, SPAWN_FLOOR_DEEP, WIDE_RECOVERY_RADIUS_MAX,
        WIDE_RECOVERY_RADIUS_MIN,
    };

    /// The search the game runs: the first offset whose point `is_clear`, with how many
    /// points were tested.
    fn first_clear(
        pos: [i32; 3],
        reach: RecoveryReach,
        mut is_clear: impl FnMut([i32; 3]) -> bool,
    ) -> (Option<[i32; 3]>, usize) {
        let mut tested = 0;
        let mut index = 0;
        while let Some(offset) = recovery_offset(index, reach) {
            let point = [pos[0] + offset[0], pos[1] + offset[1], pos[2] + offset[2]];
            tested += 1;
            if is_clear(point) {
                return (Some(point), tested);
            }
            index += 1;
        }
        (None, tested)
    }

    /// Hull origins below `top` on y, inside the slab footprint, are solid.
    fn slab_solid(p: [i32; 3], top: i32) -> bool {
        p[1] < top && (-40..=40).contains(&p[0]) && (-40..=40).contains(&p[2])
    }

    #[test]
    fn local_reach_is_the_engine_contact_table() {
        let all: Vec<_> = (0..)
            .map_while(|index| recovery_offset(index, RecoveryReach::Local))
            .collect();
        assert_eq!(all, CONTACT_RECOVERY_OFFSETS);
    }

    #[test]
    fn wide_reach_lifts_a_hull_sunk_five_units_into_a_slab() {
        let top = -170;
        let stuck = [0, top - 5, 0];
        let (out, tested) = first_clear(stuck, RecoveryReach::Wide, |p| !slab_solid(p, top));
        assert_eq!(out, Some([0, top, 0]));
        // 14 local offsets, rings 3 and 4 offer nothing clear, ring 5 finds the lift first.
        assert_eq!(tested, 14 + 2 * 18 + 1);
        let (local, _) = first_clear(stuck, RecoveryReach::Local, |p| !slab_solid(p, top));
        assert_eq!(
            local, None,
            "the contact table alone leaves the hull buried"
        );
    }

    #[test]
    fn wide_reach_prefers_the_axis_over_a_diagonal_of_the_same_ring() {
        let (out, _) = first_clear([0; 3], RecoveryReach::Wide, |p| {
            p == [4, 4, 0] || p == [0, 4, 0]
        });
        assert_eq!(out, Some([0, 4, 0]));
    }

    #[test]
    fn wide_reach_is_bounded_and_gives_up_past_its_radius() {
        let deep = [0, -200, 0];
        // Only a point 19 units above the stuck origin is clear.
        let (out, tested) = first_clear(deep, RecoveryReach::Wide, |p| {
            p[1] > deep[1] + WIDE_RECOVERY_RADIUS_MAX
        });
        assert_eq!(out, None);
        let rings = (WIDE_RECOVERY_RADIUS_MAX - WIDE_RECOVERY_RADIUS_MIN + 1) as usize;
        assert_eq!(tested, 14 + rings * 18);
    }

    #[test]
    fn wide_offsets_are_distinct_and_never_repeat_a_local_one() {
        let mut seen: Vec<_> = (0..)
            .map_while(|index| recovery_offset(index, RecoveryReach::Wide))
            .collect();
        let total = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), total);
        // Every wide offset is at least WIDE_RECOVERY_RADIUS_MIN away along some axis.
        for offset in seen
            .iter()
            .filter(|o| !CONTACT_RECOVERY_OFFSETS.contains(o))
        {
            let reach = offset
                .iter()
                .map(|c| c.abs())
                .max()
                .expect("an offset has three components");
            assert!((WIDE_RECOVERY_RADIUS_MIN..=WIDE_RECOVERY_RADIUS_MAX).contains(&reach));
        }
    }

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
