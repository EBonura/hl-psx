//! Fractional launch motion at the existing 20 Hz actor tick rate.
/// Ground-moving monster catalog entries, independent of combat AI state.
/// Includes passive G-Man/critter actors and scripted human/loader walkers;
/// excludes swimming/flying monsters, mounted turrets and decorative props.
pub fn walker(kind: u8) -> bool {
    matches!(kind, 0..=2 | 5..=10 | 14..=16 | 18 | 25 | 51..=55 | 59..=60)
}
/// Mounted turrets own bit 8 as inactivity. Keep every other class's existing
/// push/environment eligibility unchanged.
pub fn environment_eligible(kind: u8) -> bool { !matches!(kind, 20..=22) }
pub fn airborne(kind: u8, flags: u8) -> bool { environment_eligible(kind) && flags & 8 != 0 }
pub fn touches(trigger: [[i32;3];2], actor: [[i32;3];2]) -> bool {
    (0..3).all(|k| actor[0][k] <= trigger[1][k] && actor[1][k] >= trigger[0][k])
}
#[derive(Clone, Copy)]
pub struct State { pub active: bool, remainder: [i8; 3], pub velocity: [i16; 3] }
impl State {
    pub const EMPTY: Self = Self { active: false, remainder: [0; 3], velocity: [0; 3] };
    pub fn start(&mut self) { *self = Self { active: true, remainder: [0; 3], velocity: [0; 3] }; }
    pub fn launch(&mut self, impulse: [i16;3], grounded: bool) {
        self.velocity[0]=impulse[0];
        self.velocity[2]=impulse[2];
        if grounded { self.velocity[1]=impulse[1].saturating_neg(); }
    }
    pub fn step(&mut self, velocity_q4: [i16; 3]) -> [i32; 3] {
        core::array::from_fn(|axis| {
            let value = velocity_q4[axis] as i32 + self.remainder[axis] as i32;
            self.remainder[axis] = (value % 16) as i8;
            value / 16
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn airborne_contact_changes_horizontal_motion_without_vertical_reboost() {
        let mut s=State::EMPTY; s.start(); s.velocity=[16,-50,32];
        s.launch([4,102,7],false); assert_eq!(s.velocity,[4,-50,7]);
        s.launch([-4,102,7],true); assert_eq!(s.velocity,[-4,-102,7]);
    }
    #[test] fn eligibility_is_movement_based_including_idle_scripted_walkers() {
        for kind in [0,1,2,14,15,18,25,52,53,54,55,59,60] {
            assert!(walker(kind), "walker {kind}");
        }
        for kind in [3,4,11,12,13,17,19,20,21,22,23,24,50,56,57,58,61,67,75] {
            assert!(!walker(kind), "non-walker {kind}");
        }
    }
    #[test] fn retail_slab_touches_actor_box_without_origin_containment() {
        // c2a1 model56, remapped [HLx,HLz,HLy]. Headcrab box at
        // the edge of the pipe floor: origin is outside the slab's X range.
        let trigger=[[-1104,56,1304],[-1040,64,1392]];
        let origin=[-1028,64,1368];
        assert!(origin[0] > trigger[1][0]);
        let actor=[[origin[0]-12,origin[1],origin[2]-12],
                   [origin[0]+12,origin[1]+24,origin[2]+12]];
        assert!(touches(trigger,actor));
        let outside=[[-1039,64,1356],[-1015,88,1380]];
        assert!(!touches(trigger,outside));
    }
    #[test] fn retail_slow_launch_and_signed_residues_are_preserved() {
        let mut s = State::EMPTY; s.start();
        let mut distance = [0;3];
        for _ in 0..20 { let d=s.step([8, -102, -8]); for k in 0..3 { distance[k]+=d[k]; } }
        assert_eq!(distance,[10,-127,-10]);
        s.start(); assert_eq!(s.step([0;3]),[0;3]);
    }
}

#[cfg(test)]
mod packed_airborne_tests {
    #[test]
    fn inactive_turrets_are_not_airborne_walkers() {
        for kind in [20,21,22] { assert!(!super::airborne(kind,8)); }
        for kind in [0,1,2,8,11,15,19,23,24,54,58] {
            assert!(super::airborne(kind,8));
            assert!(!super::airborne(kind,0));
        }
    }
}
