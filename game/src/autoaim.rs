//! Autoaim for the bullet weapons (`CBasePlayer::GetAutoaimVector` and
//! `AutoaimDeflection`). Pure integer arithmetic over Q12 direction vectors,
//! so the host logic tests run the exact code the console runs.
//!
//! The rule, in the original's terms: on Easy and Medium (never Hard) a shot
//! from the Glock, .357, MP5 or shotgun is steered toward the visible hostile
//! whose `|dir . right| + 0.5 |dir . up|` (stretched by 20 % over 8192 units)
//! is smallest and inside the weapon's cone: 10 degrees for the pistols, 5 for
//! the MP5 and shotgun. A shot that already lines up with a hostile is left
//! alone, so a deliberate head shot keeps its aim. The steering is clamped to
//! 25 degrees of pitch and 12 of yaw, then scaled: Medium takes 0.9 of it,
//! Easy 0.33 (the SDK keeps no history between shots, so the 0.67 it blends
//! in is always zero).

use psx_math::int32::isqrt_i32;

/// sin(10 degrees) in Q12: the Glock and .357 cone (`AUTOAIM_10DEGREES`).
pub const CONE_10: i32 = 711;
/// sin(5 degrees) in Q12: the MP5 and shotgun cone (`AUTOAIM_5DEGREES`).
pub const CONE_5: i32 = 357;
/// The range every autoaim candidate is weighed against.
const RANGE: i32 = 8192;
/// 25 degrees of pitch and 12 of yaw, in 4096-per-turn angle units.
const MAX_PITCH: i32 = 284;
const MAX_YAW: i32 = 136;

/// One candidate target seen from the shooter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Aim {
    /// The original's off-axis score (smaller is better), at most the cone.
    pub score: i32,
    /// Turn needed to face the target, in 4096-per-turn angle units: yaw
    /// positive toward the shooter's right, pitch positive upward.
    pub yaw: i32,
    pub pitch: i32,
}

/// Score a target for the cone: `None` when it is behind the shooter or
/// outside `cone`. `d` is the vector from the eye to the target; `fwd`,
/// `right` and `up` are the view axes in Q12.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub fn aim_at(
    d: [i32; 3],
    fwd: [i32; 3],
    right: [i32; 3],
    up: [i32; 3],
    cone: i32,
) -> Option<Aim> {
    let len = isqrt_i32(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
    if len == 0 || len > 2 * RANGE {
        return None;
    }
    let dot = |a: [i32; 3]| (d[0] * a[0] + d[1] * a[1] + d[2] * a[2]) / len;
    let ahead = dot(fwd);
    if ahead <= 0 {
        return None;
    }
    let (r, u) = (dot(right), dot(up));
    let score = r.abs() + u.abs() / 2;
    // Farther targets count for more: x (1 + 0.2 * distance / 8192).
    let score = score + score * len / (5 * RANGE);
    if score > cone {
        return None;
    }
    // Small angles: the turn is the offset over the forward part, 652 units
    // (4096 / 2 pi) to the radian.
    Some(Aim {
        score,
        yaw: r * 652 / ahead,
        pitch: u * 652 / ahead,
    })
}

/// The steering for a target that needs `yaw_off` / `pitch_off` angle units of
/// turn, at difficulty `skill` (0 Easy, 1 Medium, 2 Hard).
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub fn steer(yaw_off: i32, pitch_off: i32, skill: usize) -> (i32, i32) {
    let scale = match skill {
        0 => 33,
        1 => 90,
        _ => 0,
    };
    (
        yaw_off.clamp(-MAX_YAW, MAX_YAW) * scale / 100,
        pitch_off.clamp(-MAX_PITCH, MAX_PITCH) * scale / 100,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FWD: [i32; 3] = [0, 0, 4096];
    const RIGHT: [i32; 3] = [4096, 0, 0];
    const UP: [i32; 3] = [0, 4096, 0];

    fn at(d: [i32; 3], cone: i32) -> Option<Aim> {
        aim_at(d, FWD, RIGHT, UP, cone)
    }

    #[test]
    fn a_target_dead_ahead_scores_zero() {
        let a = at([0, 0, 500], CONE_10).unwrap();
        assert_eq!((a.score, a.yaw, a.pitch), (0, 0, 0));
    }

    #[test]
    fn the_cone_is_ten_degrees_for_pistols_and_five_for_the_mp5() {
        // 8 degrees to the right at 500 units: inside 10, outside 5.
        let d = [70, 0, 495];
        assert!(at(d, CONE_10).is_some());
        assert!(at(d, CONE_5).is_none());
        // 12 degrees is outside both.
        assert!(at([106, 0, 498], CONE_10).is_none());
    }

    #[test]
    fn vertical_offset_counts_half() {
        let side = at([60, 0, 495], CONE_10).unwrap().score;
        let above = at([0, 60, 495], CONE_10).unwrap().score;
        assert!(above < side);
        assert!((above * 2 - side).abs() < 12, "{above} {side}");
    }

    #[test]
    fn behind_the_shooter_is_never_a_target() {
        assert_eq!(at([0, 0, -300], CONE_10), None);
        assert_eq!(at([300, 0, 0], CONE_10), None);
    }

    #[test]
    fn distance_stretches_the_score() {
        let near = at([30, 0, 300], CONE_10).unwrap().score;
        let far = at([300, 0, 3000], CONE_10).unwrap().score;
        assert!(far > near);
    }

    #[test]
    fn the_turn_needed_is_the_angle_to_the_target() {
        // 6 degrees right (tan 0.105) and 3 degrees up: 68 and 34 angle units.
        let a = at([105, 52, 1000], CONE_10).unwrap();
        assert!((a.yaw - 68).abs() <= 3, "{a:?}");
        assert!((a.pitch - 34).abs() <= 3, "{a:?}");
        // Left and down are negative.
        let b = at([-105, -52, 1000], CONE_10).unwrap();
        assert_eq!((b.yaw, b.pitch), (-a.yaw, -a.pitch));
    }

    #[test]
    fn medium_takes_nine_tenths_and_easy_a_third_and_hard_none() {
        assert_eq!(steer(100, 200, 1), (90, 180));
        assert_eq!(steer(100, 200, 0), (33, 66));
        assert_eq!(steer(100, 200, 2), (0, 0));
    }

    #[test]
    fn steering_is_clamped_to_25_pitch_and_12_yaw() {
        // 40 degrees of yaw and pitch wanted: 12 and 25 degrees taken, x 0.9.
        assert_eq!(steer(455, 455, 1), (122, 255));
        assert_eq!(steer(-455, -455, 1), (-122, -255));
    }
}
