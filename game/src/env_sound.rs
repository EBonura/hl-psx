//! Authored room ownership. Leaving a source clears its search range, while
//! the last audible room remains until another source supplies a room update.

pub const NONE: u16 = u16::MAX;
pub const NEAR_TICKS: u16 = 5;
pub const FAR_TICKS: u16 = 15;
pub fn due(now: u16,next: u16) -> bool {(now.wrapping_sub(next) as i16)>=0}
pub fn next(now: u16,delay: u16) -> u16 {now.wrapping_add(delay)}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listener {
    pub owner: u16,
    pub selected_room: u8,
    pub audible_room: u8,
    pub range_sq: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub next_ticks: u16,
    pub room_update: Option<u8>,
}
impl Listener {
    pub const NEW: Self = Self {
        owner: NONE, selected_room: 0, audible_room: 0, range_sq: 0,
    };
    pub fn observe(&mut self, source: u16, room: u8, range_sq: Option<u32>) -> Observation {
        if self.owner == source {
            if self.selected_room == 0 || self.range_sq == 0 {
                return Observation {next_ticks: FAR_TICKS, room_update: None};
            }
            if let Some(distance) = range_sq {
                self.range_sq = distance;
                return Observation {next_ticks: NEAR_TICKS, room_update: None};
            }
            self.range_sq = 0;
            self.selected_room = 0;
            return Observation {next_ticks: FAR_TICKS, room_update: None};
        }
        if let Some(distance) = range_sq {
            if self.range_sq == 0 || distance < self.range_sq {
                self.owner = source;
                self.selected_room = room;
                self.audible_room = room;
                self.range_sq = distance;
                return Observation {next_ticks: NEAR_TICKS, room_update: Some(room)};
            }
        }
        Observation {next_ticks: NEAR_TICKS, room_update: None}
    }
}

/// A point trace supplies visibility and whether the ray visits both open
/// and liquid contents. Actors are not sound occluders.
pub fn in_range(source_eye: [i32;3], listener_eye: [i32;3], radius: u16,
                visible: bool, crosses_liquid: bool) -> Option<u32> {
    if !visible || crosses_liquid {return None;}
    let mut distance = 0u64;
    for axis in 0..3 {
        let delta = (source_eye[axis] as i64 - listener_eye[axis] as i64).unsigned_abs();
        if delta > radius as u64 {return None;}
        distance += delta * delta;
    }
    (distance <= radius as u64 * radius as u64).then_some(distance as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_poll_deadlines_preserve_wrap_and_short_long_intervals() {
        assert_eq!(next(65534,NEAR_TICKS),3);
        assert!(!due(2,3));assert!(due(3,3));assert!(due(4,3));
        assert_eq!(next(65534,FAR_TICKS),13);
        assert!(!due(12,13));assert!(due(13,13));
    }
    #[test]
    fn radius_is_inclusive_and_actors_do_not_replace_world_visibility() {
        assert_eq!(in_range([0;3],[3,4,0],5,true,false),Some(25));
        assert_eq!(in_range([0;3],[3,4,1],5,true,false),None);
        assert_eq!(in_range([0;3],[3,4,0],5,false,false),None);
        assert_eq!(in_range([0;3],[3,4,0],5,true,true),None);
    }
    #[test]
    fn nearer_wins_but_equal_distance_preserves_owner() {
        let mut state=Listener::NEW;
        assert_eq!(state.observe(2,18,Some(100)).room_update,Some(18));
        assert_eq!(state.observe(3,19,Some(100)).room_update,None);
        assert_eq!(state.observe(3,19,Some(99)).room_update,Some(19));
        assert_eq!(state.owner,3);
    }
    #[test]
    fn owner_loss_preserves_audible_room_until_a_new_source_wins() {
        let mut state=Listener::NEW;
        state.observe(2,18,Some(100));
        assert_eq!(state.observe(2,18,None).next_ticks,FAR_TICKS);
        assert_eq!((state.owner,state.selected_room,state.audible_room,state.range_sq),(2,0,18,0));
        assert_eq!(state.observe(2,18,Some(25)).room_update,None);
        assert_eq!(state.observe(3,0,Some(900)).room_update,Some(0));
        assert_eq!(state.audible_room,0);
    }
    #[test]
    fn owned_zero_room_and_zero_range_keep_original_long_poll() {
        let mut state=Listener::NEW;
        state.observe(2,0,Some(100));
        assert_eq!(state.observe(2,0,Some(90)).next_ticks,FAR_TICKS);
        state=Listener::NEW;state.observe(3,18,Some(0));
        assert_eq!(state.observe(3,18,Some(10)).next_ticks,FAR_TICKS);
    }
    #[test]
    fn wide_coordinate_difference_does_not_wrap_into_radius() {
        assert_eq!(in_range([i32::MIN,0,0],[i32::MAX,0,0],1024,true,false),None);
    }
}
