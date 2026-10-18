//! Gonarch (`monster_bigmomma`): the rules of her three attacks, as pure
//! arithmetic the host runner exercises (`hl-logic-tests`).
//!
//! Retail numbers (bigmomma.cpp, read against big_mom.mdl's event tables):
//! the leg slash reaches 170 units in front of her, the mortar 800, and
//! she lays at most 20 babies at once. The mortar is launched 2 to 15 s
//! apart; a baby is laid every 0.5 to 2.5 s for the first of a pair and every
//! 5 to 10 s for the second. All times are 20 Hz ticks.

/// The slash needs the enemy this close (BIG_ATTACKDIST) and within 0.7 of
/// her facing; the mortar within 800 and 0.5 of her facing.
pub const SLASH_RANGE: i32 = 170;
pub const SLASH_FACING_Q12: i32 = 2867;
pub const MORTAR_RANGE: i32 = 800;
pub const MORTAR_FACING_Q12: i32 = 2048;
/// BIG_MAXCHILDREN.
pub const MAX_CHILDREN: u8 = 20;
/// The mortar leaves her sack this far above her origin.
pub const MORTAR_MUZZLE_HEIGHT: i32 = 180;
/// The mortar's own gravity (`gravity = 1.0`: 800 u/s^2 is 2 units a tick
/// squared), as the half applied either side of a move in sixteenths.
pub const MORTAR_HALF_GRAVITY_Q4: i32 = 16;

/// big_mom.mdl moves at 130 units a second walking and 400 running.
pub const WALK_SPEED: i32 = 7;
pub const RUN_SPEED: i32 = 20;

/// The `claw` sequence 5 (41 frames at 35 fps) swings at frame 20 (event 10);
/// `spawn` (31 frames at 20 fps) lays at frame 8 (event 12) and `mortar`
/// (17 frames at 20 fps) fires at frame 10 (event 11).
pub const SLASH_TICKS: u8 = 23;
pub const SLASH_EVENT_TICK: u8 = 12;
pub const LAY_TICKS: u8 = 30;
pub const LAY_EVENT_TICK: u8 = 8;
pub const MORTAR_TICKS: u8 = 16;
pub const MORTAR_EVENT_TICK: u8 = 10;

/// The state that persists between her decisions.
#[derive(Clone, Copy)]
pub struct Brood {
    /// Earliest tick of the next mortar.
    pub mortar_at: u16,
    /// Earliest tick of the next baby.
    pub crab_at: u16,
    /// The baby just laid was the first of a pair.
    pub half_pair: bool,
    /// Babies alive.
    pub children: u8,
}

impl Brood {
    pub const NEW: Brood = Brood {
        mortar_at: 0,
        crab_at: 0,
        half_pair: false,
        children: 0,
    };

    /// Whether the mortar may be launched now at an enemy `dist` units off.
    pub fn mortar_ready(&self, now: u16, dist: i32, facing: bool) -> bool {
        facing && dist <= MORTAR_RANGE && time_after(now, self.mortar_at)
    }

    /// A mortar has been launched; the next comes 2 to 15 seconds later.
    pub fn launched(&mut self, now: u16, roll: u16) {
        self.mortar_at = now.wrapping_add(40 + roll % 261);
    }

    /// Whether another baby may be laid now (the area below her is clear).
    pub fn can_lay(&self, now: u16, area_clear: bool) -> bool {
        area_clear && self.children < MAX_CHILDREN && time_after(now, self.crab_at)
    }

    /// A baby has been laid. The first of a pair is followed by the second
    /// after 0.5 to 2.5 seconds, a second by the next pair after 5 to 10.
    pub fn laid(&mut self, now: u16, roll: u16) {
        self.children += 1;
        if self.half_pair {
            self.crab_at = now.wrapping_add(100 + roll % 101);
            self.half_pair = false;
        } else {
            self.crab_at = now.wrapping_add(10 + roll % 41);
            self.half_pair = true;
        }
    }

    /// One of her babies died.
    pub fn child_died(&mut self) {
        self.children = self.children.saturating_sub(1);
    }
}

/// Wrapping "now is after `at`".
pub fn time_after(now: u16, at: u16) -> bool {
    now.wrapping_sub(at) < 0x8000
}

/// Whether she slashes: the enemy is inside the reach and in front of her
/// (`facing` is the 0.7 cone check on her heading).
pub fn can_slash(facing: bool, dist: i32) -> bool {
    facing && dist <= SLASH_RANGE
}

/// The velocity (sixteenths of a unit a tick) of a lob from `start` to
/// `target` whose apex is `apex_y`, after VecCheckSplatToss: it climbs to the
/// apex under gravity and covers half the horizontal distance in that time.
/// `None` when the apex is not above the start. With g = 2 units a tick
/// squared the climb takes sqrt(height) ticks at 2 sqrt(height) a tick.
pub fn splat_toss(start: [i32; 3], target: [i32; 3], apex_y: i32) -> Option<[i32; 3]> {
    let height = apex_y - start[1] - 15;
    if height <= 0 {
        return None;
    }
    // 16 sqrt(height): the climb time in sixteenths of a tick
    let climb_q4 = psx_math::int32::isqrt_i32(height * 256).max(1);
    let half = |d: i32| d * 128 / climb_q4;
    Some([
        half(target[0] - start[0]),
        2 * climb_q4,
        half(target[2] - start[2]),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_needs_reach_and_facing() {
        assert!(can_slash(true, 170));
        assert!(!can_slash(true, 171));
        assert!(!can_slash(false, 100));
    }

    #[test]
    fn babies_come_in_pairs_and_stop_at_twenty() {
        let mut b = Brood::NEW;
        assert!(b.can_lay(1, true));
        b.laid(100, 0); // first of a pair: next in 0.5 s
        assert_eq!(b.crab_at, 110);
        assert!(!b.can_lay(105, true));
        assert!(b.can_lay(111, true));
        b.laid(111, 0); // second: next pair in 5 s
        assert_eq!(b.crab_at, 211);
        b.laid(300, 100); // first of the next pair: 0.5 to 2.5 s
        assert_eq!(b.crab_at, 300 + 10 + 100 % 41);
        b.children = MAX_CHILDREN;
        assert!(!b.can_lay(2000, true));
        b.child_died();
        assert!(b.can_lay(2000, true));
        assert!(!b.can_lay(2000, false));
    }

    #[test]
    fn the_mortar_waits_two_to_fifteen_seconds() {
        let mut b = Brood::NEW;
        assert!(b.mortar_ready(1, 800, true));
        assert!(!b.mortar_ready(1, 801, true));
        assert!(!b.mortar_ready(1, 100, false));
        b.launched(1000, 0);
        assert!(!b.mortar_ready(1039, 100, true));
        assert!(b.mortar_ready(1041, 100, true));
        b.launched(1000, 260);
        assert_eq!(b.mortar_at, 1300);
    }

    #[test]
    fn the_lob_comes_down_near_the_target() {
        use crate::proj_logic::integrate_q4;
        let start = [0, 180, 0];
        let target = [400, 180, 300];
        let apex = 180 + 300; // 300 above the start, over the midpoint
        let mut v = splat_toss(start, target, apex).unwrap();
        let mut rem = [0i8; 3];
        let mut p = start;
        let mut ticks = 0;
        while ticks < 300 {
            let step = integrate_q4(&mut v, &mut rem, MORTAR_HALF_GRAVITY_Q4);
            p = [p[0] + step[0], p[1] + step[1], p[2] + step[2]];
            ticks += 1;
            if p[1] <= start[1] && ticks > 4 {
                break;
            }
        }
        // back at launch height, about the full horizontal distance away
        assert!((p[0] - 400).abs() < 40, "{p:?}");
        assert!((p[2] - 300).abs() < 40, "{p:?}");
        assert!(splat_toss(start, target, 150).is_none());
    }
}
