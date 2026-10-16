//! Compact, host-testable contracts shared by the cooked tram parser and the
//! runtime path/rotation code. Keeping these integer-only avoids pulling PSX
//! math dependencies into the host tests.

use crate::wide::Wide;

/// Marker for the compact speed/wheels/start motion word.
///
/// Bit 31 distinguishes it from the two legacy layouts. The remaining fields
/// fit every stock Half-Life tracktrain (speed <= 3000, wheels <= 700 and at
/// most 256 cooked path points) without growing the room blob:
/// `1xxx_xxss ssss_ssss ssww_wwww_wwww_vvvv_vvvv_vvvv` conceptually packs
/// speed[11:0], wheels[9:0], and start[7:0].
pub const TRAM_MOTION_V2: u32 = 0x8000_0000;

/// Decode the backward-compatible tram motion word as (speed, start, wheels).
///
/// The oldest rooms stored only a positive i32 speed. The previous format put
/// the authored waypoint in the high half and speed in the low half. V2 uses
/// the spare bits to retain GoldSrc's `wheels` look-ahead distance too.
#[inline(always)]
pub const fn decode_motion_word(word: u32, n_way: usize) -> (i32, usize, i32) {
    if word & TRAM_MOTION_V2 != 0 {
        let speed = (word & 0x0fff) as i32;
        let wheels = ((word >> 12) & 0x03ff) as i32;
        let candidate = ((word >> 22) & 0x00ff) as usize;
        let start = if candidate < n_way { candidate } else { 0 };
        (speed, start, wheels)
    } else {
        let speed = (word & 0xffff) as i32;
        let candidate = (word >> 16) as usize;
        let start = if candidate < n_way { candidate } else { 0 };
        // Stock CFuncTrackTrain substitutes 100 when `wheels` is absent/zero.
        (speed, start, 100)
    }
}

/// One full turn in the controller's high-precision angle representation.
/// Q28 leaves sixteen fractional bits below the public Q12 view while still
/// fitting a complete unsigned turn in a positive i32.
pub const TRAM_YAW_Q28_TURN: i32 = 1 << 28;

#[inline(always)]
pub const fn shortest_angle_delta_q28(current: i32, target: i32) -> i32 {
    let mask = TRAM_YAW_Q28_TURN - 1;
    let mut delta = (target.wrapping_sub(current)) & mask;
    if delta > TRAM_YAW_Q28_TURN / 2 {
        delta -= TRAM_YAW_Q28_TURN;
    }
    delta
}

/// Integrate GoldSrc's 20 Hz tracktrain angular controller.
///
/// `CFuncTrackTrain::Next` requests `AngleDistance(target,current) * 10` and
/// the pusher integrates it for 50 ms, so each physical frame closes exactly
/// half of the shortest angular error. Values are turns in Q28.
#[inline(always)]
pub const fn half_angle_step_q28(current: i32, target: i32) -> i32 {
    shortest_angle_delta_q28(current, target) / 2
}

/// GoldSrc/Xash `svc_addangle` conversion for one physical train-yaw step.
///
/// `MSG_WriteBitAngle(...,16)` floors the modulo-one-turn value. Arithmetic
/// shifting is the equivalent signed operation from Q28 to Q16: a tiny
/// negative turn becomes -1 rather than zero. PSX camera space has the
/// opposite sign after the Gold XY -> PSX XZ axis mapping.
#[inline(always)]
pub const fn camera_step_q16_from_physical_q28(physical_step: i32) -> i32 {
    -(physical_step >> 12)
}

#[inline(always)]
const fn round_shift15(value: i32) -> i32 {
    if value >= 0 {
        (value + (1 << 14)) >> 15
    } else {
        -((-value + (1 << 14)) >> 15)
    }
}

#[optimize(size)]
/// Floor `numerator / denominator` as an unsigned Q16 ratio without shifting
/// the numerator out of i32 range. The loop is a restoring binary divide; its
/// `denominator - remainder` comparison is the overflow-free form of
/// `2*remainder >= denominator`.
const fn ratio_q16(numerator: i32, denominator: i32) -> i32 {
    let mut remainder = numerator;
    let mut quotient = 0i32;
    let mut bit = 0;
    while bit < 16 {
        quotient <<= 1;
        if remainder >= denominator - remainder {
            remainder -= denominator - remainder;
            quotient |= 1;
        } else {
            remainder += remainder;
        }
        bit += 1;
    }
    quotient
}

#[optimize(size)]
/// High-precision, i32-only atan2 in Q28 turns.
///
/// A Q15 degree-nine minimax polynomial stays below half of one GoldSrc q16
/// packet unit across every opening-tram lookahead vector. It avoids floats,
/// i64 division, lookup-state RAM, and the Q12 target staircase that otherwise
/// accumulates visible camera drift over the five-map ride.
pub const fn atan2_q28(y: i32, x: i32) -> i32 {
    const QUARTER: i32 = 1 << 26;
    const EIGHTH: i32 = 1 << 25;
    const HALF: i32 = 1 << 27;
    const COEFF_Q16: [i32; 5] = [65_527, -21_647, 11_806, -5_579, 1_365];

    if x == 0 {
        return if y > 0 {
            QUARTER
        } else if y < 0 {
            TRAM_YAW_Q28_TURN - QUARTER
        } else {
            0
        };
    }
    if y == 0 {
        return if x < 0 { HALF } else { 0 };
    }
    let ax = x.abs();
    let ay = y.abs();
    let first_quadrant = if ax == ay {
        EIGHTH
    } else {
        let (small, large, reflect) = if ay < ax {
            (ay, ax, false)
        } else {
            (ax, ay, true)
        };
        let t_q15 = (ratio_q16(small, large) + 1) >> 1;
        let z_q15 = round_shift15(t_q15 * t_q15);
        let mut p_q16 = COEFF_Q16[4];
        let mut i = 4usize;
        while i > 0 {
            i -= 1;
            p_q16 = COEFF_Q16[i] + round_shift15(p_q16 * z_q15);
        }
        let radians_q16 = round_shift15(p_q16 * t_q15);
        // 904/355/256 approximates 1/(2*pi) while keeping every intermediate
        // inside i32. The final result is Q28 turns.
        let n = radians_q16 * 904;
        let octant = ((n / 355) << 8) + (((n % 355) << 8) / 355);
        if reflect {
            QUARTER - octant
        } else {
            octant
        }
    };
    if x > 0 {
        if y > 0 {
            first_quadrant
        } else {
            TRAM_YAW_Q28_TURN - first_quadrant
        }
    } else if y > 0 {
        HALF - first_quadrant
    } else {
        HALF + first_quadrant
    }
}

#[optimize(size)]
/// Scale a signed Q28 platform rotation across an integer travel distance
/// without overflowing `delta * distance`.
pub const fn scale_angle_offset_q28(delta: i32, distance: i32, length: i32) -> i32 {
    if length <= 0 {
        return 0;
    }
    let d = if distance < 0 {
        0
    } else if distance > length {
        length
    } else {
        distance
    };
    let whole = delta / length;
    let remainder = delta % length;
    whole * d + (remainder * d) / length
}

/// Add a fine Q16 camera delta to the public Q12 view while retaining the four
/// fractional bits that create the characteristic sub-Q12 GoldSrc drift.
#[inline(always)]
pub const fn add_camera_step_q16(yaw_q12: u16, frac_q16: u8, step_q16: i32) -> (u16, u8) {
    let fine = (yaw_q12 as i32) * 16 + frac_q16 as i32 + step_q16;
    (((fine >> 4) & 0x0fff) as u16, (fine & 0x0f) as u8)
}

/// Segment whose heading represents the train's authored, unrotated pose.
#[inline(always)]
pub const fn authored_heading_segment(start: usize, n_way: usize) -> usize {
    if n_way < 2 {
        0
    } else if start + 1 < n_way {
        start
    } else {
        n_way - 2
    }
}

/// Visible yaw of Half-Life's stock tracktrain brush from its path heading.
///
/// `CFuncTrackTrain::Find` and `Next` both compute the forward path angle and
/// then add 180 degrees because the authored train brush points west. That is
/// a fixed model-space basis, not the heading of the first path segment in the
/// current BSP. Using the latter appears correct in c0a0 (whose first segment
/// happens to run west) but rotates the carried car by 90 degrees in intro maps
/// whose local path starts north or south.
#[inline(always)]
pub const fn visible_yaw_q12(path_heading_q28: i32) -> u16 {
    (((path_heading_q28 >> 16) + 2048) & 0x0fff) as u16
}

/// GoldSrc's positive XY yaw becomes the inverse Y-up rotation after mapping
/// `(x,y,z)` to PSX world `(x,z,y)`. Truncate Q12 to the renderer's Q8 first,
/// then negate, so sub-step positive angles remain zero instead of rounding to
/// a full negative Q8 step. Camera travel yaw intentionally stays positive.
#[inline(always)]
pub const fn world_rotation_angle_q8(travel_yaw_q12: u16) -> u16 {
    0u16.wrapping_sub(travel_yaw_q12 >> 4) & 0x00ff
}

/// A fresh room honors the selected func_tracktrain's authored `startspeed`.
/// A transferred global train instead keeps the motion state carried from the
/// source room; the destination's delayed trigger_auto may still start it on
/// its authored tick.
#[inline(always)]
pub const fn should_apply_spawn_startspeed(riding_transfer: bool) -> bool {
    !riding_transfer
}

#[inline(always)]
const fn wide_diff(a: i32, b: i32) -> Wide {
    Wide::of(a).sub_w(Wide::of(b))
}

/// Whether a carried train lies just before the first point of its new path.
///
/// Nearest-point projection clamps such a position to waypoint zero. Preserve
/// it as an approach leg instead: otherwise every transition shaves off the
/// short overlap authored between adjacent BSPs. The degenerate-path guard
/// keeps a duplicate first waypoint from classifying every nearby point as an
/// approach.
#[inline(always)]
pub const fn is_upstream_of_first(car: [i32; 3], first: [i32; 3], second: [i32; 3]) -> bool {
    let (sx, sy, sz) = (wide_diff(second[0], first[0]), wide_diff(second[1], first[1]), wide_diff(second[2], first[2]));
    if sx.is_zero() && sy.is_zero() && sz.is_zero() {
        return false;
    }
    let (dx, dy, dz) = (wide_diff(car[0], first[0]), wide_diff(car[1], first[1]), wide_diff(car[2], first[2]));
    dx.mul_w(sx).add_w(dy.mul_w(sy)).add_w(dz.mul_w(sz)).is_negative()
}

/// Car-local passenger centre limit across the tram's narrow axis.
///
/// The brush is 150 units wide (half-width 75) and the player hull is 32 wide,
/// so a centre beyond 91 has nothing at all underfoot. The old 93 came from
/// the last grounded GoldSrc sample on the c0a0e exit walk, where the station
/// platform is holding the player up rather than the car: as a moving-ride
/// seat limit it pinned a rider outside the body instead of dropping them.
pub const RIDER_LOCAL_Z_LIMIT: i32 = 91;
/// Cross-BSP seat normalization. Gold reaches c0a0d/e at roughly -89 on the
/// narrow axis; keeping four units of walking margin reproduces the six
/// grounded exit steps instead of ejecting on the first input sample.
pub const RIDER_TRANSFER_Z_LIMIT: i32 = 89;
/// Expanded stopped-car support includes the final GoldSrc grounded sample.
pub const RIDER_STOPPED_Z_LIMIT: i32 = 98;

#[inline(always)]
pub const fn clamp_rider_local_z(z: i32) -> i32 {
    if z < -RIDER_LOCAL_Z_LIMIT {
        -RIDER_LOCAL_Z_LIMIT
    } else if z > RIDER_LOCAL_Z_LIMIT {
        RIDER_LOCAL_Z_LIMIT
    } else {
        z
    }
}

#[inline(always)]
pub const fn clamp_transfer_rider_local_z(z: i32) -> i32 {
    if z < -RIDER_TRANSFER_Z_LIMIT {
        -RIDER_TRANSFER_Z_LIMIT
    } else if z > RIDER_TRANSFER_Z_LIMIT {
        RIDER_TRANSFER_Z_LIMIT
    } else {
        z
    }
}

/// Symmetric nearest-integer conversion from a signed Q12 accumulator.
///
/// An arithmetic `>> 12` always rounds negative values down. A rider is
/// transformed car->world->car every moving tick, so that one-sided error
/// accumulated for hundreds of ticks until both seat axes hit their clamps.
/// Ties round away from zero, giving positive and negative seats identical
/// treatment without division or resident state.
#[inline(always)]
pub const fn round_q12(value: i32) -> i32 {
    if value >= 0 {
        (value + 2048) >> 12
    } else {
        -((-value + 2048) >> 12)
    }
}

/// Rotate a car-local XZ point into world XZ using Q12 cosine/sine.
#[inline(always)]
pub const fn rotate_rider_xz(c: i32, s: i32, x: i32, z: i32) -> [i32; 2] {
    [round_q12(c * x + s * z), round_q12(-s * x + c * z)]
}

/// Inverse of [`rotate_rider_xz`] for an orthogonal Y rotation.
#[inline(always)]
pub const fn inverse_rotate_rider_xz(c: i32, s: i32, x: i32, z: i32) -> [i32; 2] {
    [round_q12(c * x - s * z), round_q12(s * x + c * z)]
}

/// Support footprint for a rider on the MOVING car.
///
/// Only the car is underfoot out here, so the limit is the brush itself plus
/// the player's hull half-width. The stopped footprint below is deliberately
/// looser: at a station the platform shares the rider's floor.
#[inline(always)]
pub const fn rider_over_moving_tram(local_x: i32, local_z: i32) -> bool {
    local_x >= -162
        && local_x <= 162
        && local_z >= -RIDER_LOCAL_Z_LIMIT
        && local_z <= RIDER_LOCAL_Z_LIMIT
}

/// Tight stopped-tram support footprint in car-local coordinates.
///
/// This is deliberately separate from the broad moving-tram acquisition
/// radius: tracktrain pivots vary by hundreds of units, but once attached to a
/// stationary car the rider should detach at the actual expanded brush edge.
#[inline(always)]
pub const fn rider_over_tram_footprint(local_x: i32, local_z: i32) -> bool {
    local_x >= -162
        && local_x <= 162
        && local_z >= -RIDER_STOPPED_Z_LIMIT
        && local_z <= RIDER_STOPPED_Z_LIMIT
}

/// Result of deliberate player movement while the tram carry path is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RiderMotionDecision {
    /// The candidate penetrates the car's player hull; retain the prior seat.
    Restore,
    /// The candidate is clear and still supported by the car or station.
    Publish,
    /// The candidate is clear and outside support, so ordinary physics resumes.
    Detach,
}

/// Decide whether a moved tram rider publishes, restores, or walks off.
///
/// Collision clearance takes precedence over the rectangular support test.
/// Mover `startsolid` is deliberately suppressed by ordinary slide traces, so
/// a one-unit rounded penetration can otherwise advance through a solid end
/// wall until the support test incorrectly calls it an open-side departure.
#[inline(always)]
pub const fn rider_motion_decision(
    supported: bool,
    clear_of_tram_hull: bool,
) -> RiderMotionDecision {
    if !clear_of_tram_hull {
        RiderMotionDecision::Restore
    } else if supported {
        RiderMotionDecision::Publish
    } else {
        RiderMotionDecision::Detach
    }
}

/// Squared distance between two points, saturating just below `u32::MAX`
/// (which callers use as "no candidate yet"). Exact for any two points inside
/// a map, where the three squares sum to under 2^32.
#[inline]
pub fn squared_distance(a: [i32; 3], b: [i32; 3]) -> u32 {
    let mut sum = 0u32;
    let mut axis = 0;
    while axis < 3 {
        let d = a[axis].abs_diff(b[axis]);
        sum = sum.saturating_add(d.saturating_mul(d));
        axis += 1;
    }
    sum.min(u32::MAX - 1)
}

/// Largest component of a segment, or of the offset of a point from its start,
/// that `project_onto_segment` accepts. A map is at most 8192 units across, so
/// every real train segment and train position is inside a third of it; three
/// times the map is the largest limit that keeps the squared length and the
/// dot product (3 * limit^2, under 2^31) in `i32`.
pub const SEGMENT_LIMIT: u32 = 3 << 13;

/// Project `center` onto the segment `a..b`: the squared distance to the
/// closest point, the distance of that point along the segment (capped at
/// `i16::MAX`) and the point itself, in whole units with the division
/// truncated toward zero. `None` for a zero-length segment or one beyond
/// `SEGMENT_LIMIT`.
#[inline(never)]
pub fn project_onto_segment(
    a: [i32; 3],
    b: [i32; 3],
    center: [i32; 3],
) -> Option<(u32, i16, [i32; 3])> {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let rel = [
        center[0].saturating_sub(a[0]),
        center[1].saturating_sub(a[1]),
        center[2].saturating_sub(a[2]),
    ];
    if d.iter()
        .chain(rel.iter())
        .any(|v| v.unsigned_abs() > SEGMENT_LIMIT)
    {
        return None;
    }
    let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if len2 == 0 {
        return None;
    }
    let along = (rel[0] * d[0] + rel[1] * d[1] + rel[2] * d[2]).clamp(0, len2);
    let q = [
        a[0] + psx_math::int32::mul_div_i32(d[0], along, len2),
        a[1] + psx_math::int32::mul_div_i32(d[1], along, len2),
        a[2] + psx_math::int32::mul_div_i32(d[2], along, len2),
    ];
    let length = psx_math::int32::isqrt_u32(len2 as u32);
    let distance =
        psx_math::int32::mul_div_u32(length, along as u32, len2 as u32).min(i16::MAX as u32) as i16;
    Some((squared_distance(center, q), distance, q))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_projection_matches_the_wide_form() {
        // The former i64 body of `train_seek_position`, verbatim.
        let wide = |a: [i32; 3], b: [i32; 3], center: [i32; 3]| -> Option<(i64, i16, [i32; 3])> {
            let d = [
                (b[0] - a[0]) as i64,
                (b[1] - a[1]) as i64,
                (b[2] - a[2]) as i64,
            ];
            let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if len2 == 0 {
                return None;
            }
            let rel = [
                (center[0] - a[0]) as i64,
                (center[1] - a[1]) as i64,
                (center[2] - a[2]) as i64,
            ];
            let along_num = (rel[0] * d[0] + rel[1] * d[1] + rel[2] * d[2]).clamp(0, len2);
            let q = [
                a[0] as i64 + d[0] * along_num / len2,
                a[1] as i64 + d[1] * along_num / len2,
                a[2] as i64 + d[2] * along_num / len2,
            ];
            let error = (center[0] as i64 - q[0]).pow(2)
                + (center[1] as i64 - q[1]).pow(2)
                + (center[2] as i64 - q[2]).pow(2);
            let length = psx_math::int32::isqrt_u64(len2 as u64) as u64;
            let distance = (length * along_num as u64 / len2 as u64).min(i16::MAX as u64) as i16;
            Some((error, distance, [q[0] as i32, q[1] as i32, q[2] as i32]))
        };
        let mut state = 0x31d7_5a9bu32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let check = |a: [i32; 3], b: [i32; 3], c: [i32; 3]| {
            let new = project_onto_segment(a, b, c);
            let old = wide(a, b, c);
            assert_eq!(
                new.map(|(e, dist, q)| (e as i64, dist, q)),
                old,
                "{a:?} {b:?} {c:?}"
            );
        };
        let edges = [-8192, -4096, -1, 0, 1, 7, 4095, 8192];
        for &x in &edges {
            for &y in &edges {
                for &z in &edges {
                    for &c in &edges {
                        check([x, y, z], [y, z, x], [c, x, y]);
                        check([x, y, z], [x, y, z], [c, c, c]);
                        check([x, 0, 0], [c, 0, 0], [y, z, 0]);
                    }
                }
            }
        }
        for round in 0..600_000u32 {
            let range = if round % 4 == 0 { 8192 } else { 300 };
            let mut point = |next: &mut dyn FnMut() -> u32| -> [i32; 3] {
                let mut axis = || (next() % (2 * range as u32 + 1)) as i32 - range;
                [axis(), axis(), axis()]
            };
            let (a, b) = (point(&mut next), point(&mut next));
            // Mostly near the segment, sometimes anywhere in the map.
            let c = if round % 3 == 0 {
                point(&mut next)
            } else {
                let t = (next() % 1025) as i32;
                let on = [
                    a[0] + (b[0] - a[0]) * t / 1024,
                    a[1] + (b[1] - a[1]) * t / 1024,
                    a[2] + (b[2] - a[2]) * t / 1024,
                ];
                let j = point(&mut next);
                [on[0] + j[0] / 8, on[1] + j[1] / 8, on[2] + j[2] / 8]
            };
            check(a, b, c);
        }
        // Beyond the supported size there is no projection.
        assert!(project_onto_segment([0; 3], [24577, 0, 0], [0; 3]).is_none());
        assert!(project_onto_segment([0; 3], [1, 0, 0], [0, i32::MIN, 0]).is_none());
        assert_eq!(
            squared_distance([i32::MIN, 0, 0], [i32::MAX, 0, 0]),
            u32::MAX - 1
        );
        assert_eq!(squared_distance([3, 4, 12], [0, 0, 0]), 169);
    }

    #[optimize(size)]
    #[test]
    fn legacy_and_packed_motion_words_decode_without_a_format_version() {
        assert_eq!(decode_motion_word(300, 12), (300, 0, 100));
        assert_eq!(decode_motion_word((7 << 16) | 300, 12), (300, 7, 100));
        assert_eq!(decode_motion_word((99 << 16) | 150, 12), (150, 0, 100));
        let packed = TRAM_MOTION_V2 | 300 | (170 << 12) | (7 << 22);
        assert_eq!(decode_motion_word(packed, 12), (300, 7, 170));
    }

    #[optimize(size)]
    #[test]
    fn angular_controller_halves_the_shortest_q28_error() {
        let q = TRAM_YAW_Q28_TURN / 4;
        assert_eq!(half_angle_step_q28(0, q), q / 2);
        assert_eq!(half_angle_step_q28(q, 0), -(q / 2));
        assert_eq!(half_angle_step_q28(TRAM_YAW_Q28_TURN - 20, 20), 20);
        assert_eq!(half_angle_step_q28(20, TRAM_YAW_Q28_TURN - 20), -20);
    }

    #[optimize(size)]
    #[test]
    fn addangle_floors_negative_steps_and_preserves_q16_fraction() {
        assert_eq!(camera_step_q16_from_physical_q28(4095), 0);
        assert_eq!(camera_step_q16_from_physical_q28(4096), -1);
        assert_eq!(camera_step_q16_from_physical_q28(-1), 1);
        assert_eq!(camera_step_q16_from_physical_q28(-4096), 1);
        assert_eq!(add_camera_step_q16(1024, 0, 12), (1024, 12));
        assert_eq!(add_camera_step_q16(1024, 12, 12), (1025, 8));
        assert_eq!(add_camera_step_q16(0, 0, -1), (4095, 15));
    }

    #[optimize(size)]
    #[test]
    fn high_precision_atan_has_exact_axes_and_diagonals() {
        let q = TRAM_YAW_Q28_TURN / 4;
        assert_eq!(atan2_q28(0, 1), 0);
        assert_eq!(atan2_q28(1, 0), q);
        assert_eq!(atan2_q28(0, -1), q * 2);
        assert_eq!(atan2_q28(-1, 0), q * 3);
        assert_eq!(atan2_q28(1, 1), q / 2);
        assert_eq!(atan2_q28(1, -1), q + q / 2);
        assert_eq!(atan2_q28(-1, -1), q * 2 + q / 2);
        assert_eq!(atan2_q28(-1, 1), q * 3 + q / 2);
        assert!((atan2_q28(1, 2) - 19_808_338).abs() < 4096);
    }

    #[optimize(size)]
    #[test]
    fn trackchange_angle_scaling_is_signed_and_overflow_safe() {
        for &(delta, length) in &[(1 << 26, 1271), (-(1 << 26), 1582), (-(1 << 27), 46_340)] {
            assert_eq!(scale_angle_offset_q28(delta, -1, length), 0);
            assert_eq!(scale_angle_offset_q28(delta, 0, length), 0);
            assert_eq!(scale_angle_offset_q28(delta, length, length), delta);
            assert_eq!(scale_angle_offset_q28(delta, length + 1, length), delta);
            for distance in [1, length / 3, length - 1] {
                assert_eq!(
                    scale_angle_offset_q28(delta, distance, length),
                    ((delta as i64 * distance as i64) / length as i64) as i32
                );
            }
        }
    }

    #[optimize(size)]
    #[test]
    fn authored_heading_uses_start_segment_and_clamps_a_terminal_start() {
        assert_eq!(authored_heading_segment(0, 0), 0);
        assert_eq!(authored_heading_segment(0, 8), 0);
        assert_eq!(authored_heading_segment(5, 8), 5);
        assert_eq!(authored_heading_segment(7, 8), 6);
    }

    #[optimize(size)]
    #[test]
    fn tracktrain_uses_the_fixed_west_facing_brush_basis() {
        let q = TRAM_YAW_Q28_TURN / 4;
        assert_eq!(visible_yaw_q12(0), 2048); // eastward path -> car yaw 180
        assert_eq!(visible_yaw_q12(q), 3072); // northward path -> car yaw 270
        assert_eq!(visible_yaw_q12(q * 2), 0); // westward path -> car yaw 0
        assert_eq!(visible_yaw_q12(q * 3), 1024); // southward path -> car yaw 90
    }

    #[optimize(size)]
    #[test]
    fn psx_world_rotation_inverts_gold_path_yaw() {
        assert_eq!(world_rotation_angle_q8(0), 0);
        assert_eq!(world_rotation_angle_q8(1), 0);
        assert_eq!(world_rotation_angle_q8(15), 0);
        assert_eq!(world_rotation_angle_q8(16), 255);
        assert_eq!(world_rotation_angle_q8(1024), 192);
        assert_eq!(world_rotation_angle_q8(3072), 64);
        assert_eq!(world_rotation_angle_q8(4095), 1);
    }

    #[optimize(size)]
    #[test]
    fn transferred_tram_state_overrides_destination_startspeed_only_at_spawn() {
        assert!(should_apply_spawn_startspeed(false));
        assert!(!should_apply_spawn_startspeed(true));
    }

    #[optimize(size)]
    #[test]
    fn carried_train_positions_before_waypoint_zero_keep_their_overlap() {
        let c0a0c_first = [-126, 1328, 251];
        let c0a0c_second = [-289, 1328, 292];
        assert!(is_upstream_of_first(
            [-91, 1328, 251],
            c0a0c_first,
            c0a0c_second
        ));
        assert!(!is_upstream_of_first(
            c0a0c_first,
            c0a0c_first,
            c0a0c_second
        ));
        assert!(!is_upstream_of_first(
            [-207, 1328, 271],
            c0a0c_first,
            c0a0c_second
        ));
        assert!(is_upstream_of_first(
            [3495, 1328, 2190],
            [3495, 1328, 2168],
            [3495, 1328, 1962]
        ));
        assert!(!is_upstream_of_first([1, 2, 3], [0, 0, 0], [0, 0, 0]));
    }

    #[optimize(size)]
    #[test]
    fn rider_local_z_keeps_the_goldsrc_edge_seat() {
        assert_eq!(clamp_rider_local_z(-94), -91);
        assert_eq!(clamp_rider_local_z(-89), -89);
        assert_eq!(clamp_rider_local_z(0), 0);
        assert_eq!(clamp_rider_local_z(93), 91);
        assert_eq!(clamp_rider_local_z(94), 91);
        assert_eq!(clamp_transfer_rider_local_z(-93), -89);
        assert_eq!(clamp_transfer_rider_local_z(93), 89);
    }

    #[optimize(size)]
    #[test]
    fn signed_q12_rounding_has_no_negative_floor_bias() {
        assert_eq!(round_q12(-4096), -1);
        assert_eq!(round_q12(-4095), -1);
        assert_eq!(round_q12(-2048), -1);
        assert_eq!(round_q12(-2047), 0);
        assert_eq!(round_q12(0), 0);
        assert_eq!(round_q12(2047), 0);
        assert_eq!(round_q12(2048), 1);
        assert_eq!(round_q12(4095), 1);
        assert_eq!(round_q12(4096), 1);
    }

    #[optimize(size)]
    #[test]
    fn repeated_rider_rotation_round_trips_do_not_walk_into_the_clamps() {
        // Q12 sin/cos at 45 degrees. These are the non-cardinal turns that
        // exposed the old arithmetic-shift drift during the opening ride.
        let (c, s) = (2896, 2896);
        for initial in [[-119, -32], [101, 60], [-38, -93], [-53, -94]] {
            let mut local = initial;
            let mut i = 0;
            while i < 1000 {
                let world = rotate_rider_xz(c, s, local[0], local[1]);
                local = inverse_rotate_rider_xz(c, s, world[0], world[1]);
                i += 1;
            }
            assert_eq!(local, initial);
        }
        // Gold's stable edge seat quantizes by one unit once, then remains
        // stable rather than drifting toward +/-93 each tick.
        let mut edge = [32, -89];
        let mut i = 0;
        while i < 1000 {
            let world = rotate_rider_xz(c, s, edge[0], edge[1]);
            edge = inverse_rotate_rider_xz(c, s, world[0], world[1]);
            i += 1;
        }
        assert_eq!(edge, [33, -89]);
    }

    #[optimize(size)]
    #[test]
    fn stopped_tram_support_uses_the_expanded_brush_footprint() {
        assert!(rider_over_tram_footprint(162, 98));
        assert!(rider_over_tram_footprint(-162, -98));
        assert!(rider_over_tram_footprint(-38, -98));
        assert!(!rider_over_tram_footprint(163, 0));
        assert!(!rider_over_tram_footprint(0, 99));
        assert!(!rider_over_tram_footprint(-53, -99));
    }

    #[optimize(size)]
    #[test]
    fn solid_tram_hull_contact_restores_instead_of_detaching() {
        assert_eq!(
            rider_motion_decision(false, false),
            RiderMotionDecision::Restore
        );
        assert_eq!(
            rider_motion_decision(true, false),
            RiderMotionDecision::Restore
        );
        assert_eq!(
            rider_motion_decision(true, true),
            RiderMotionDecision::Publish
        );
        assert_eq!(
            rider_motion_decision(false, true),
            RiderMotionDecision::Detach
        );
    }
}
