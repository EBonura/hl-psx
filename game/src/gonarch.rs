//! Gonarch (`monster_bigmomma`) once her path is walked: the leg slash, the
//! acid mortar and the brood of baby headcrabs.
//!
//! Pure rules with no hardware, included by the host runner (`hl-logic-tests`).
//! Velocities of the mortar are in sixteenths of a unit per tick, the same
//! representation `proj_logic::integrate_q4` moves.

/// A slash reaches 170 units, in front of her (cosine 0.7 of the facing).
pub const SLASH_RANGE: i32 = 170;
pub const SLASH_FACING_Q12: i32 = 2867;
/// The mortar reaches 800 units.
pub const MORTAR_RANGE: i32 = 800;
/// At most this many of her babies live at once.
pub const MAX_CHILDREN: u8 = 20;
/// Where the mortar leaves her: this high above her origin.
pub const MORTAR_MUZZLE_HEIGHT: i32 = 180;
/// Gravity of the mortar (full: 800 units a second squared is 2 a tick squared),
/// half of it taken either side of a move in sixteenths.
pub const MORTAR_HALF_GRAVITY_Q4: i32 = 16;

/// Her speeds along a node path or in the chase, in units per tick: the model
/// walks at 130 units a second and runs at 400.
pub const WALK_SPEED: i32 = 7;
pub const RUN_SPEED: i32 = 20;

/// Ticks of each action: the slash and the lay run their animation to the
/// event, the mortar likewise (events at frame 20 of 35 fps, 8 of 20 and
/// 10 of 20).
pub const SLASH_TICKS: u8 = 16;
pub const SLASH_EVENT_TICK: u8 = 11;
pub const LAY_TICKS: u8 = 16;
pub const LAY_EVENT_TICK: u8 = 8;
pub const MORTAR_TICKS: u8 = 17;
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
    pub fn mortar_ready(&self, now: u16, dist: i32) -> bool {
        dist <= MORTAR_RANGE && time_after(now, self.mortar_at)
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
/// (`dot_q12` is the cosine of the angle off her facing, in 4096ths).
pub fn can_slash(dot_q12: i32, dist: i32) -> bool {
    dot_q12 >= SLASH_FACING_Q12 && dist <= SLASH_RANGE
}

/// The velocity (q4 units a tick) of a lob from `start` to `target` whose apex
/// is `apex_y` over the midpoint, after `VecCheckSplatToss`: it climbs to the
/// apex under gravity and covers half the horizontal distance in that time,
/// so it comes back down level with where it started. `None` when the apex is
/// not above the start.
pub fn splat_toss(start: [i32; 3], target: [i32; 3], apex_y: i32) -> Option<[i32; 3]> {
    let height = apex_y - start[1] - 15;
    if height <= 0 {
        return None;
    }
    // 2 units a tick squared: v_up = sqrt(2 * 2 * height) units a tick
    let v_up_q4 = isqrt(4 * height as i64 * 256) as i32; // (sqrt(4h) * 16)
    let time_ticks = (v_up_q4 / 2).max(1) as i64; // v_up / g, in q4 ticks (x16)
    let dx = (target[0] - start[0]) as i64;
    let dz = (target[2] - start[2]) as i64;
    // half the distance over the climb time: (d/2) / (time_q4/16) in q4 units
    let vx = (dx * 16 * 16 / (2 * time_ticks)) as i32;
    let vz = (dz * 16 * 16 / (2 * time_ticks)) as i32;
    Some([vx, v_up_q4, vz])
}

fn isqrt(v: i64) -> i64 {
    psx_math::int32::isqrt_i32(v.clamp(0, i32::MAX as i64) as i32) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_needs_reach_and_facing() {
        assert!(can_slash(4096, 170));
        assert!(!can_slash(4096, 171));
        assert!(can_slash(2867, 100));
        assert!(!can_slash(2866, 100));
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
        assert!(b.mortar_ready(1, 800));
        assert!(!b.mortar_ready(1, 801));
        b.launched(1000, 0);
        assert!(!b.mortar_ready(1039, 100));
        assert!(b.mortar_ready(1041, 100));
        b.launched(1000, 260);
        assert_eq!(b.mortar_at, 1300);
    }

    #[test]
    fn the_lob_comes_down_near_the_target() {
        use crate::proj_logic::integrate_q4;
        let start = [0, 180, 0];
        let target = [400, 0, 300];
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
