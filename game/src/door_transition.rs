//! Transient matching global-door state, separate from actor/train admission.
pub const ANGULAR: u8 = 1;
pub const REVERSED: u8 = 2;
pub const CAPACITY: usize = 9; // maximum of the 63 retail placements

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Carry {
    pub id: u16,
    pub phase: u16,
    pub remaining: u16,
    pub speed: u16,
    pub relative_center: [i16; 3],
    pub movement: [i16; 3],
    pub state: u8,
    pub flags: u8,
}

impl Carry {
    pub const EMPTY: Self = Self { id: 0, phase: 0, remaining: 0, speed: 0,
        relative_center: [0;3], movement: [0;3], state: 0, flags: 0 };

    #[optimize(size)]
    pub fn destination_movement(self, mut authored: [i32;3]) -> [i32;3] {
        if self.flags & (ANGULAR|REVERSED) == (ANGULAR|REVERSED) { authored[0] = -authored[0]; }
        authored
    }

    #[optimize(size)]
    pub fn phase_at(self, landmark: [i32;3], base_center: [i32;3], movement: [i32;3]) -> Option<i32> {
        if self.phase > 4096 { return None; }
        let movement=self.destination_movement(movement);
        let expected = [0,1,2].map(|axis| landmark[axis] + self.relative_center[axis] as i32);
        let offset = [0,1,2].map(|axis| expected[axis] - base_center[axis]);
        if self.flags & ANGULAR != 0 {
            return (self.movement.map(i32::from)==movement && offset.iter().all(|&v|v.abs()<=1))
                .then_some(self.phase as i32);
        }
        let phase = if self.movement.map(i32::from) == movement {
            self.phase as i32
        } else {
            let length: i64 = movement.iter().map(|&v| v as i64*v as i64).sum();
            if length == 0 { return None; }
            let dot: i64 = (0..3).map(|axis| offset[axis] as i64*movement[axis] as i64).sum();
            ((dot*4096)/length).clamp(0,4096) as i32
        };
        // Do not silently place a mismatched global model somewhere along a
        // different path. One unit accounts for the renderer's integer phase.
        (0..3).all(|axis| (base_center[axis]+((movement[axis]*phase)>>12)-expected[axis]).abs() <= 1)
            .then_some(phase)
    }
}

#[optimize(size)]
pub fn remaining(now: u16, deadline: u16) -> u16 {
    let delta = deadline.wrapping_sub(now);
    if delta <= i16::MAX as u16 { delta } else { 0 }
}

#[optimize(size)]
pub fn take_count(count: &mut usize) -> usize { core::mem::take(count) }

#[optimize(size)]
pub fn matching(records: &[Carry], id: u16, landmark: [i32;3], center: [i32;3], movement: [i32;3]) -> Option<(Carry,i32)> {
    for &record in records {
        if record.id == id {
            if let Some(phase)=record.phase_at(landmark,center,movement) {return Some((record,phase));}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[optimize(size)]
    #[test]
    fn in_flight_landmark_translation_preserves_fraction_and_direction() {
        let source_landmark = [-1508,-2672,1404];
        let target_landmark = [2324,-512,-880];
        let mut carry = Carry { id: 1, phase: 1537, remaining: 7, speed: 150,
            relative_center: [-58,-16,38], movement: [-58,0,0], state: 1, flags: 0 };
        assert_eq!(carry.phase_at(target_landmark,[2288,-528,-842],[-58,0,0]),Some(1537));
        carry.state=3;
        assert_eq!(carry.phase_at(source_landmark,[-1544,-2688,1442],[-58,0,0]),Some(1537));
        assert_eq!(carry.speed,150);
    }
    #[optimize(size)]
    #[test]
    fn mismatched_destination_does_not_relocate_a_brush() {
        let carry=Carry { id: 1, phase: 4096, relative_center: [10,0,0], movement:[10,0,0], ..Carry::EMPTY };
        assert_eq!(carry.phase_at([100,200,300],[100,200,300],[10,0,0]),Some(4096));
        assert_eq!(carry.phase_at([100,200,300],[100,204,300],[10,0,0]),None);
        assert_eq!(carry.phase_at([100,200,300],[100,200,300],[0,0,0]),None);
    }
    #[optimize(size)]
    #[test]
    fn wait_preserves_remaining_time_across_clock_reset_and_wrap() {
        assert_eq!(remaining(65530,4),10);
        assert_eq!(remaining(100,110),10);
        assert_eq!(remaining(110,100),0);
        assert_eq!(remaining(100,100),0);
        assert_eq!(core::mem::size_of::<Carry>(),22);
    }
    #[optimize(size)]
    #[test]
    fn duplicate_authored_global_names_match_geometry_and_are_consumed_once() {
        let left=Carry {id:1,phase:4096,relative_center:[-20,0,0],movement:[-10,0,0],..Carry::EMPTY};
        let right=Carry {id:1,phase:1537,relative_center:[13,0,0],movement:[10,0,0],..Carry::EMPTY};
        let records=[left,right];
        assert_eq!(matching(&records,1,[100,0,0],[110,0,0],[10,0,0]),Some((right,1537)));
        assert_eq!(matching(&records,2,[100,0,0],[110,0,0],[10,0,0]),None);
        let mut count=records.len();
        let taken=take_count(&mut count);
        assert_eq!(taken,2);
        assert_eq!(count,0);
        assert_eq!(take_count(&mut count),0,"later restart must not replay transient state");
    }
    #[optimize(size)]
    #[test]
    fn rotating_global_preserves_phase_at_translated_pivot() {
        let carry=Carry {id:1,phase:1537,flags: ANGULAR,movement:[2048,0,0],relative_center:[4,8,12],..Carry::EMPTY};
        assert_eq!(carry.phase_at([100,200,300],[104,208,312],[2048,0,0]),Some(1537));
        assert_eq!(carry.phase_at([100,200,300],[104,208,312],[1024,0,0]),None);
        let reverse=Carry {movement:[-2048,0,0],flags:ANGULAR|REVERSED,state:3,..carry};
        assert_eq!(reverse.phase_at([100,200,300],[104,208,312],[2048,0,0]),Some(1537));
        assert_eq!(reverse.destination_movement([2048,0,0]),[-2048,0,0]);
        assert_eq!(reverse.state,3);

    }
}
