//! Transient matching-global brush state, consumed at the next room admission.

pub const CAPACITY: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Carry {
    pub relative_center: [i32; 3],
    pub id: u16,
    pub health: u16,
    /// Existing port velocity components, in units per simulation tick.
    pub velocity: [i8; 3],
    pub active: u8,
}

pub const EMPTY: Carry = Carry {
    relative_center: [0; 3],
    id: 0,
    health: 0,
    velocity: [0; 3],
    active: 0,
};

impl Carry {
    /// BSP model centres remain destination-owned. Only the live translation
    /// changes; rendering and collision consume that same translation.
    pub fn destination_offset(
        self,
        landmark: [i32; 3],
        model_center: [i32; 3],
    ) -> Option<[i32; 3]> {
        let mut offset = [0; 3];
        for axis in 0..3 {
            offset[axis] = landmark[axis]
                .checked_add(self.relative_center[axis])?
                .checked_sub(model_center[axis])?;
        }
        Some(offset)
    }
}

pub fn matching(records: &[Carry], id: u16) -> Option<Carry> {
    if id == 0 {
        return None;
    }
    records.iter().find(|record| record.id == id).copied()
}

pub fn take_count(count: &mut usize) -> usize {
    core::mem::replace(count, 0).min(CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_box_survives_forward_and_reverse_landmarks_on_destination_geometry() {
        let source_landmark = [100, 200, 300];
        let source_center = [140, 220, 350];
        let carry = Carry {
            relative_center: [40, 20, 50],
            id: 0x8001,
            health: 13,
            velocity: [2, -3, 1],
            active: 1,
        };
        let destination_landmark = [1000, 800, -200];
        let destination_center = [1020, 820, -160];
        let offset = carry
            .destination_offset(destination_landmark, destination_center)
            .unwrap();
        assert_eq!(offset, [20, 0, 10]);
        let live = [0, 1, 2].map(|axis| destination_center[axis] + offset[axis]);
        let returning = Carry {
            relative_center: [0, 1, 2].map(|axis| live[axis] - destination_landmark[axis]),
            ..carry
        };
        assert_eq!(
            returning.destination_offset(source_landmark, source_center),
            Some([0; 3])
        );
        assert_eq!(returning.health, 13);
        assert_eq!(returning.velocity, [2, -3, 1]);
    }

    #[test]
    fn broken_state_matches_only_same_global_and_transient_count_is_consumed() {
        let broken = Carry {
            id: 0x8002,
            active: 0,
            health: 0,
            ..EMPTY
        };
        assert_eq!(matching(&[broken], 0x8002), Some(broken));
        assert_eq!(matching(&[broken], 0x8003), None);
        assert_eq!(matching(&[EMPTY], 0), None);
        let mut count = 1;
        assert_eq!(take_count(&mut count), 1);
        assert_eq!(take_count(&mut count), 0);
    }

    #[test]
    fn bad_transform_is_rejected_and_resident_record_is_twenty_bytes() {
        let carry = Carry {
            relative_center: [1, 0, 0],
            ..EMPTY
        };
        assert_eq!(carry.destination_offset([i32::MAX, 0, 0], [0; 3]), None);
        assert_eq!(core::mem::size_of::<Carry>(), 20);
    }
}
