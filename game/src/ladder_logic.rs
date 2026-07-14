//! Small, allocation-free ladder mount correction.
//!
//! GoldSrc's hull contact leaves the player flush with the ladder face.  The
//! PS1's whole-unit movement can first touch a thin ladder volume from just
//! beyond a corner, which makes the following climb run into the frame around
//! the ladder.  On the first forward-moving contact, snap to the approached
//! face and clamp only the ladder's tangential axis to its authored width.

const PLAYER_HALF_WIDTH: i32 = 16;
const PLAYER_HALF_HEIGHT: i32 = 34;
// A whole-unit player origin can lead GoldSrc's float origin by nearly three
// units after a long ground approach.  Do not turn that shallow overlap into a
// ladder frame until the center has crossed the outer contact band.  This is
// entry hysteresis only: an already-mounted player retains the full hull test.
const NEW_MOUNT_NORMAL_INSET: i32 = 2;
const DESCEND_PITCH: i16 = -300;

#[inline]
pub fn wants_descend(pitch: i16) -> bool {
    pitch < DESCEND_PITCH
}

/// GoldSrc tests the ladder model against the active player hull. The old
/// 18-unit horizontal padding grabbed thin ladders two units before the
/// 16-unit standing hull actually reached them, changing ordinary floor input
/// into climb input beside the c1a0e shaft.
pub fn touches(pos: [i32; 3], center: [i32; 3], half: [i32; 3]) -> bool {
    (pos[0] - center[0]).abs() <= half[0] + PLAYER_HALF_WIDTH
        && (pos[1] - center[1]).abs() <= half[1] + PLAYER_HALF_HEIGHT
        && (pos[2] - center[2]).abs() <= half[2] + PLAYER_HALF_WIDTH
}

/// Whether a *new* ladder contact has enough overlap along its thin face to be
/// stable after integer quantization.  The strict comparison deliberately
/// excludes the two-unit boundary itself.  Tangential and vertical acceptance
/// remain the ordinary hull test (or the separate intentional-mount assist).
pub fn new_mount_has_stable_normal_overlap(
    pos: [i32; 3],
    center: [i32; 3],
    half: [i32; 3],
) -> bool {
    let normal_axis = if half[0] <= half[2] { 0 } else { 2 };
    (pos[normal_axis] - center[normal_axis]).abs()
        < half[normal_axis] + PLAYER_HALF_WIDTH - NEW_MOUNT_NORMAL_INSET
}

/// Two-unit PS1 quantization assist for an intentional mount from the lower
/// half of a ladder.  Widen only the tangential edge: widening the thin/normal
/// axis grabs the ladder before the real player hull reaches its face.  That
/// skipped GoldSrc's final wall-contact frame on both c1a1d ladders and changed
/// the second ladder's exit momentum enough to miss the hanging crates.
///
/// Keep this separate from ordinary contact: applying the wider box at the
/// upper landing captures floor sidesteps as climb input.
pub fn touches_mount_assist(
    pos: [i32; 3],
    center: [i32; 3],
    half: [i32; 3],
    allow_upper: bool,
) -> bool {
    if !allow_upper && pos[1] > center[1] {
        return false;
    }
    let dx = (pos[0] - center[0]).abs();
    let dz = (pos[2] - center[2]).abs();
    let vertical = (pos[1] - center[1]).abs() <= half[1] + PLAYER_HALF_HEIGHT;
    if half[0] <= half[2] {
        vertical && dx <= half[0] + PLAYER_HALF_WIDTH && dz <= half[2] + PLAYER_HALF_WIDTH + 2
    } else {
        vertical && dx <= half[0] + PLAYER_HALF_WIDTH + 2 && dz <= half[2] + PLAYER_HALF_WIDTH
    }
}

#[inline]
fn clamp_inside(value: i32, center: i32, half: i32) -> i32 {
    let inset = (half > 0) as i32;
    value.clamp(center - half + inset, center + half - inset)
}

/// Scale one signed semantic axis without the toward-zero bias that turns a
/// full +127 input into 9 when the authored speed is 10 units/tick.
pub fn axis_speed_nearest(axis: i32, speed: i32) -> i32 {
    let product = axis * speed;
    if product >= 0 {
        (product + 64) / 128
    } else {
        -((-product + 64) / 128)
    }
}

/// Q12 multiply with symmetric nearest rounding. Ladder movement often sits
/// on cardinal angles whose LUT values are one unit shy of 4096; truncating
/// those values turns an authored 10-unit sidestep into 9 every tick.
pub fn mul_q12_nearest(a: i32, b: i32) -> i32 {
    let product = a * b;
    if product >= 0 {
        (product + 2048) >> 12
    } else {
        -((-product + 2048) >> 12)
    }
}

/// Fixed-point form of GoldSrc's `PM_LadderMove` velocity decomposition.
///
/// `forward` and `right` are Q12 view vectors, `normal_axis` is the ladder's
/// thin horizontal axis (0 = X, 2 = Z), and `normal_sign` points from the
/// ladder towards the player. The result is Q6 world units per 20 Hz tick.
/// GoldSrc treats ladder input as buttons, so any non-zero semantic axis uses
/// the full authored speed; crouching supplies the smaller speed at the call
/// site.
pub fn goldsrc_velocity_q6(
    forward: [i32; 3],
    right: [i32; 3],
    fwd: i32,
    strafe: i32,
    speed_q6: i32,
    normal_axis: usize,
    normal_sign: i32,
    on_floor: bool,
) -> [i32; 3] {
    let fwd_speed = fwd.signum() * speed_q6;
    let right_speed = strafe.signum() * speed_q6;
    let mut intended = [0; 3];
    let mut axis = 0;
    while axis < 3 {
        intended[axis] =
            mul_q12_nearest(forward[axis], fwd_speed) + mul_q12_nearest(right[axis], right_speed);
        axis += 1;
    }

    // PM_LadderMove removes the component through the ladder face, then turns
    // that same component upward. For a vertical cardinal plane,
    // normal x (up x normal) is exactly world-up, so no cross products or
    // normalization are needed at runtime.
    let normal = intended[normal_axis] * normal_sign;
    intended[normal_axis] = 0;
    intended[1] -= normal;
    if on_floor && normal > 0 {
        // Walking away while grounded releases the player from the face.
        intended[normal_axis] = speed_q6 * normal_sign;
    }
    intended
}

/// Infer the cardinal face normal from the already-cooked ladder AABB. BSP
/// `func_ladder` brushes are vertical and axis-aligned in the Half-Life maps,
/// so their thinner horizontal half-extent identifies the plane at no RAM
/// cost. The sign is selected from the player's current side of the volume.
pub fn cardinal_normal(pos: [i32; 3], center: [i32; 3], half: [i32; 3]) -> (usize, i32) {
    if half[0] <= half[2] {
        (0, if pos[0] < center[0] { -1 } else { 1 })
    } else {
        (2, if pos[2] < center[2] { -1 } else { 1 })
    }
}

/// Fixed-point corner contact can reject a combined ladder move even though
/// the vertical tangent is clear.  GoldSrc's slide preserves that tangent;
/// request one vertical-only retry when none of the authored climb happened.
pub fn should_retry_vertical(start_y: i32, end_y: i32, wanted_y: i32) -> bool {
    wanted_y != 0 && end_y == start_y
}

pub fn mount_target(pos: [i32; 3], center: [i32; 3], half: [i32; 3]) -> [i32; 3] {
    let mut target = pos;
    if half[0] <= half[2] {
        // GoldSrc's origin sits one hull half-width from the ladder center,
        // rather than adding the thin brush thickness a second time. Keep the
        // tangential origin one integer unit inside the authored edge so the
        // expanded hull cannot quantize into the surrounding frame.
        target[0] = if pos[0] <= center[0] {
            center[0] - PLAYER_HALF_WIDTH
        } else {
            center[0] + PLAYER_HALF_WIDTH
        };
        target[2] = clamp_inside(pos[2], center[2], half[2]);
    } else {
        // Ladder is thin along Z: symmetric treatment for the other wall
        // orientation.
        target[2] = if pos[2] <= center[2] {
            center[2] - PLAYER_HALF_WIDTH
        } else {
            center[2] + PLAYER_HALF_WIDTH
        };
        target[0] = clamp_inside(pos[0], center[0], half[0]);
    }
    target
}

#[cfg(test)]
mod tests {
    use super::{
        axis_speed_nearest, cardinal_normal, goldsrc_velocity_q6, mount_target, mul_q12_nearest,
        new_mount_has_stable_normal_overlap, should_retry_vertical, touches, touches_mount_assist,
        wants_descend,
    };

    #[test]
    fn full_semantic_axis_reaches_authored_climb_speed() {
        assert_eq!(axis_speed_nearest(127, 10), 10);
        assert_eq!(axis_speed_nearest(-127, 10), -10);
        assert_eq!(axis_speed_nearest(64, 10), 5);
    }

    #[test]
    fn near_cardinal_lut_values_keep_authored_lateral_speed() {
        assert_eq!(mul_q12_nearest(4095, 10), 10);
        assert_eq!(mul_q12_nearest(-4095, 10), -10);
        assert_eq!(mul_q12_nearest(52, 10), 0);
    }

    #[test]
    fn ladder_contact_starts_at_the_real_player_hull_edge() {
        let center = [2183, -67, 723];
        let half = [2, 257, 16];
        assert!(!touches([2174, 133, 690], center, half));
        assert!(touches([2174, 133, 691], center, half));
        assert!(touches([2167, 132, 708], center, half));
        assert!(touches_mount_assist([2175, -267, 756], center, half, false,));
        assert!(!touches_mount_assist([2174, 133, 690], center, half, false));
        assert!(touches_mount_assist([2169, 133, 690], center, half, true));
        assert!(wants_descend(-1000));
        assert!(!wants_descend(0));
    }

    #[test]
    fn mount_assist_never_widens_the_ladder_face() {
        // c1a1d ladder 2 is thin along PSX Z. At Gold y=-423 the standing
        // hull is still one unit short of its -440 face and must first perform
        // the ordinary wall-contact move; the old +2 normal pad grabbed here.
        let center = [1496, 184, -442];
        let half = [12, 168, 2];
        assert!(!touches_mount_assist([1488, 52, -423], center, half, false,));
        assert!(touches_mount_assist([1488, 52, -424], center, half, false,));

        // Preserve the intended two-unit rescue along the wide/tangential
        // edge once the hull has genuinely reached the ladder face.
        assert!(touches_mount_assist([1526, 52, -424], center, half, false,));
        assert!(!touches_mount_assist([1527, 52, -424], center, half, false,));
    }

    #[test]
    fn new_mount_waits_out_the_integer_normal_ambiguity_band() {
        // First c1a1d ladder: PS1 reaches z=-314 one tick before GoldSrc's
        // float hull intersects.  Waiting until the following ground move
        // crosses this boundary aligns the start of PM_LadderMove.
        let first_center = [1152, -104, -330];
        let first_half = [12, 120, 2];
        assert!(touches([1149, -188, -314], first_center, first_half));
        assert!(!new_mount_has_stable_normal_overlap(
            [1149, -188, -314],
            first_center,
            first_half,
        ));
        assert!(new_mount_has_stable_normal_overlap(
            [1149, -188, -315],
            first_center,
            first_half,
        ));

        // The second ladder is already four units beyond the expanded face
        // when the route turns onto it, so its established mount tick remains.
        let second_center = [1496, 184, -442];
        let second_half = [12, 168, 2];
        assert!(new_mount_has_stable_normal_overlap(
            [1488, 52, -428],
            second_center,
            second_half,
        ));
    }

    #[test]
    fn mounts_west_face_and_clamps_tangent() {
        assert_eq!(
            mount_target([2174, -267, 756], [2183, -67, 723], [2, 257, 16]),
            [2167, -267, 738]
        );
    }

    #[test]
    fn mounts_east_face_without_moving_in_range_tangent() {
        assert_eq!(
            mount_target([2200, 10, 720], [2183, -67, 723], [2, 257, 16]),
            [2199, 10, 720]
        );
    }

    #[test]
    fn handles_ladders_thin_along_z() {
        assert_eq!(
            mount_target([95, 7, 80], [100, 0, 100], [12, 40, 2]),
            [95, 7, 84]
        );
    }

    #[test]
    fn retries_only_a_fully_blocked_vertical_ladder_move() {
        assert!(should_retry_vertical(143, 143, -10));
        assert!(should_retry_vertical(10, 10, 10));
        assert!(!should_retry_vertical(143, 142, -10));
        assert!(!should_retry_vertical(143, 143, 0));
    }

    #[test]
    fn goldsrc_turns_wallward_view_motion_into_vertical_climb() {
        // Facing straight into a +Z ladder while looking up 45 degrees. The
        // 7.08-unit view rise and 7.08-unit wallward component sum to the
        // characteristic 14.16-unit GoldSrc ladder step.
        assert_eq!(
            goldsrc_velocity_q6([0, 2896, -2896], [-4096, 0, 0], 127, 0, 640, 2, 1, false,),
            [0, 906, 0]
        );
    }

    #[test]
    fn c1a1d_ladder_keeps_only_the_small_authored_tangent() {
        // Approximate Q12 vectors at the captured yaw=-91.38, pitch=-49.71.
        // The old generic wall drift became -1 whole X unit every tick; the
        // GoldSrc decomposition retains -10/64 u/tick and error-diffuses it.
        assert_eq!(
            goldsrc_velocity_q6([-64, 3124, -2639], [-4095, 0, 99], 127, 0, 640, 2, 1, false,),
            [-10, 900, 0]
        );
        assert_eq!(
            cardinal_normal([1148, -188, -316], [1152, -104, -330], [12, 120, 2]),
            (2, 1)
        );
    }
}
