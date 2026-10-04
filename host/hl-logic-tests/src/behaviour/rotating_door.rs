//! Rotating doors: a two-way door turning about the vertical axis opens away
//! from whoever activated it, judged by which side of the hinge they stand on
//! relative to the way they face. One-way doors (flag 16) and doors turning
//! about Z (64) or X (128) keep their authored direction, as does a chain
//! nobody started.

use crate::world_rules::{
    rotating_door_opens_reversed, DoorActivator, DOOR_ONE_WAY, DOOR_ROTATE_X, DOOR_ROTATE_Z,
};

const HINGE: [i32; 3] = [500, 0, -200];
const FACE_PLUS_Z: [i32; 2] = [0, 4096];
const FACE_PLUS_X: [i32; 2] = [4096, 0];

fn opens(spawnflags: u16, offset: [i32; 3], forward_xz: [i32; 2]) -> Option<bool> {
    rotating_door_opens_reversed(
        spawnflags,
        HINGE,
        Some(DoorActivator {
            pos: [
                HINGE[0] + offset[0],
                HINGE[1] + offset[1],
                HINGE[2] + offset[2],
            ],
            forward_xz,
        }),
    )
}

#[test]
fn exempt_doors_keep_their_direction() {
    for flags in [
        DOOR_ONE_WAY,
        DOOR_ROTATE_Z,
        DOOR_ROTATE_X,
        DOOR_ONE_WAY | 1,
        DOOR_ROTATE_X | 32,
    ] {
        assert_eq!(
            opens(flags, [100, 0, 0], FACE_PLUS_Z),
            None,
            "flags {flags}"
        );
        assert_eq!(
            opens(flags, [-100, 0, 0], FACE_PLUS_Z),
            None,
            "flags {flags}"
        );
    }
    assert_eq!((DOOR_ONE_WAY, DOOR_ROTATE_Z, DOOR_ROTATE_X), (16, 64, 128));
}

#[test]
fn other_flags_do_not_exempt() {
    for flags in [1u16, 2, 4, 8, 32, 256, 512, 1024] {
        assert!(
            opens(flags, [100, 0, 0], FACE_PLUS_Z).is_some(),
            "flags {flags}"
        );
    }
}

#[test]
fn nobody_activating_keeps_the_direction() {
    assert_eq!(rotating_door_opens_reversed(0, HINGE, None), None);
}

#[test]
fn stepping_to_the_other_side_of_the_hinge_flips_the_swing() {
    let a = opens(0, [100, 0, 0], FACE_PLUS_Z).unwrap();
    let b = opens(0, [-100, 0, 0], FACE_PLUS_Z).unwrap();
    assert_ne!(a, b);
    // Standing on +X of the hinge facing +Z keeps the authored direction.
    assert!(!a);
}

#[test]
fn turning_around_flips_the_swing() {
    let a = opens(0, [100, 0, 30], FACE_PLUS_Z).unwrap();
    let b = opens(0, [100, 0, 30], [0, -4096]).unwrap();
    assert_ne!(a, b);
}

#[test]
fn height_does_not_matter() {
    for dy in [-500, 0, 500] {
        assert_eq!(opens(0, [100, dy, 0], FACE_PLUS_Z), Some(false));
        assert_eq!(opens(0, [-100, dy, 0], FACE_PLUS_Z), Some(true));
    }
}

#[test]
fn standing_in_line_with_the_hinge_keeps_the_authored_direction() {
    // Directly ahead of or behind the hinge along the facing direction.
    assert_eq!(opens(0, [0, 0, 100], FACE_PLUS_Z), Some(false));
    assert_eq!(opens(0, [0, 0, -100], FACE_PLUS_Z), Some(false));
    assert_eq!(opens(0, [100, 0, 0], FACE_PLUS_X), Some(false));
}

#[test]
fn a_far_activator_still_decides() {
    let far = rotating_door_opens_reversed(
        0,
        [0, 0, 0],
        Some(DoorActivator {
            pos: [40000, 0, -40000],
            forward_xz: [4096, 4096],
        }),
    );
    assert_eq!(far, Some(false));
    let far_other_side = rotating_door_opens_reversed(
        0,
        [0, 0, 0],
        Some(DoorActivator {
            pos: [-40000, 0, 40000],
            forward_xz: [4096, 4096],
        }),
    );
    assert_eq!(far_other_side, Some(true));
}

/// Recorded from ac83da7 behaviour: hinge at (500, 0, -200), activator 30
/// units up at each listed offset, facing +Z, +X, -Z, -X and the +X+Z
/// diagonal; 1 = swings against the authored direction.
#[test]
fn golden_swing_table() {
    let facings = [[0, 4096], [4096, 0], [0, -4096], [-4096, 0], [2896, 2896]];
    let table: [([i32; 3], [u8; 5]); 8] = [
        ([100, 0, 0], [0, 0, 1, 0, 0]),
        ([100, 0, 100], [0, 1, 1, 0, 0]),
        ([0, 0, 100], [0, 1, 0, 0, 1]),
        ([-100, 0, 100], [1, 1, 0, 0, 1]),
        ([-100, 0, 0], [1, 0, 0, 0, 1]),
        ([-100, 0, -100], [1, 0, 0, 1, 0]),
        ([0, 0, -100], [0, 0, 0, 1, 0]),
        ([100, 0, -100], [0, 0, 1, 1, 0]),
    ];
    for (offset, expect) in table {
        for (i, f) in facings.iter().enumerate() {
            let got = opens(0, [offset[0], 30, offset[2]], *f).unwrap() as u8;
            assert_eq!(got, expect[i], "offset {offset:?} facing {f:?}");
        }
    }
}
