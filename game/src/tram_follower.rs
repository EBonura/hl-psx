//! Allocation-free fixed-point primitives for GoldSrc-style track followers.
//!
//! GoldSrc's `CFuncTrackTrain::Next` does not advance a scalar distance along
//! the path.  It starts each lookahead at the train's actual origin and the
//! current `m_ppath`, prepares a chord toward a point 0.1 seconds ahead, then
//! performs the wheels lookahead from the same origin and the *old* pointer.
//! The pointer is committed only after both queries.  This module keeps that
//! ordering explicit while remaining independent of the PSX runtime and map
//! representation.

/// One world unit in the follower's position format.
pub const Q8_ONE: i32 = 1 << 8;

/// GoldSrc `SF_PATH_DISABLED`, represented independently of cooked-map bits.
pub const PATH_DISABLED: u8 = 1 << 0;

/// Optional cooked marker: stop after reaching this node and hand movement to
/// another phase (for example, a `func_trackchange`).
pub const PATH_PHASE_END: u8 = 1 << 1;

/// Sentinel used when a lookahead was given no valid starting node.
pub const NO_POINTER: usize = usize::MAX;

pub type Vec3Q8 = [i32; 3];

/// Minimal data returned by a path source.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PathPoint {
    pub position_q8: Vec3Q8,
    pub flags: u8,
}

/// Read-only path access. Implementations may decode points directly from a
/// packed room blob; the follower never allocates or retains a reference.
pub trait PathSource {
    fn len(&self) -> usize;
    fn point(&self, index: usize) -> PathPoint;
}

impl PathSource for [PathPoint] {
    #[inline(always)]
    fn len(&self) -> usize {
        <[PathPoint]>::len(self)
    }

    #[inline(always)]
    fn point(&self, index: usize) -> PathPoint {
        self[index]
    }
}

impl<const N: usize> PathSource for [PathPoint; N] {
    #[inline(always)]
    fn len(&self) -> usize {
        N
    }

    #[inline(always)]
    fn point(&self, index: usize) -> PathPoint {
        self[index]
    }
}

/// Controls which path flags stop a forward query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LookAheadPolicy {
    /// Flags on the candidate next node that make it invalid.
    pub blocked_next_flags: u8,
    /// Flags on the current node that stop traversal beyond that node.
    pub stop_after_flags: u8,
    /// Project along the final segment when no valid next node exists.
    pub project_past_end: bool,
}

/// Equivalent to GoldSrc `LookAhead(..., move = 1)` for a normal path.
pub const GOLDSRC_MOVE_POLICY: LookAheadPolicy = LookAheadPolicy {
    blocked_next_flags: PATH_DISABLED,
    stop_after_flags: 0,
    project_past_end: false,
};

/// Position policy for cooked paths that mark a distinct mover phase.
pub const PHASE_AWARE_MOVE_POLICY: LookAheadPolicy = LookAheadPolicy {
    blocked_next_flags: PATH_DISABLED,
    stop_after_flags: PATH_PHASE_END,
    project_past_end: false,
};

/// Equivalent to GoldSrc `LookAhead(..., move = 0)`: disabled nodes do not
/// stop a heading query and a terminal path projects its final direction.
pub const GOLDSRC_WHEELS_POLICY: LookAheadPolicy = LookAheadPolicy {
    blocked_next_flags: 0,
    stop_after_flags: 0,
    project_past_end: true,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LookAheadStop {
    /// The requested distance was consumed normally.
    Reached,
    /// The linear path had no next point.
    DeadEnd,
    /// Flags on the candidate next node made it invalid.
    BlockedNext(u8),
    /// The current node marks the end of this movement phase.
    StopAfter(u8),
    /// `start_pointer` did not name a point in the source.
    InvalidStart,
}

/// Result of one forward lookahead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LookAheadResult {
    pub point_q8: Vec3Q8,
    /// Matches the pointer returned by `CPathTrack::LookAhead`. A dead end or
    /// blocked path returns `None`, even when `point_q8` reached its terminal.
    pub returned_pointer: Option<usize>,
    /// Last valid node reached, useful for a subsequent dead-end phase.
    pub last_pointer: usize,
    pub stop: LookAheadStop,
    pub projected: bool,
}

/// Outputs prepared by the position and wheels queries of one `Next()` call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FollowerPreparation {
    pub old_pointer: usize,
    /// Pointer to commit after both lookaheads. It remains unchanged when the
    /// position query returned null.
    pub next_pointer: usize,
    /// GoldSrc fires only this final returned node when a query skips points.
    pub passed_pointer: Option<usize>,
    pub position: LookAheadResult,
    pub wheels: LookAheadResult,
    /// Displacement to apply on the following 20 Hz pusher frame.
    pub prepared_half_chord_q8: Vec3Q8,
    /// Wheels target relative to the actual origin; the caller may feed this
    /// into its high-precision atan/controller implementation.
    pub wheels_delta_q8: Vec3Q8,
}

/// Convert whole world units to Q8 without wrapping at extreme inputs.
#[inline(always)]
pub const fn units_to_q8(units: i32) -> i32 {
    units.saturating_mul(Q8_ONE)
}

/// Distance covered in one tenth of a second, in Q8, from an integer speed in
/// world units per second. Splitting quotient/remainder avoids `speed * 256`
/// overflow before the division.
#[inline(always)]
pub const fn tenth_second_distance_q8(speed: i32) -> i32 {
    if speed <= 0 {
        return 0;
    }
    let whole = speed / 10;
    let remainder = speed % 10;
    whole
        .saturating_mul(Q8_ONE)
        .saturating_add(remainder * Q8_ONE / 10)
}

/// Floor an unsigned ratio into Q16 without shifting the numerator out of
/// range and without an i64 division. `numerator` must not exceed
/// `denominator`; values above it are clamped to one.
pub const fn ratio_q16(numerator: u32, denominator: u32) -> u32 {
    if denominator == 0 || numerator >= denominator {
        return 1 << 16;
    }
    let mut remainder = numerator;
    let mut quotient = 0u32;
    let mut bit = 0;
    while bit < 16 {
        quotient <<= 1;
        // Overflow-free equivalent of `2 * remainder >= denominator`.
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

/// Exact floor of `value * fraction_q16 / 65536` using only 32-bit products.
/// The high/low split also handles negative values with arithmetic-floor
/// semantics, which is important for deterministic path-side bias.
pub const fn scale_i32_q16_floor(value: i32, fraction_q16: u32) -> i32 {
    let fraction = if fraction_q16 > (1 << 16) {
        1 << 16
    } else {
        fraction_q16
    };
    let high = value >> 16;
    let low = value as u32 & 0xffff;
    let high_part = high * fraction as i32;
    let low_part = ((low * fraction) >> 16) as i32;
    high_part + low_part
}

/// Return `(floor(value * fraction / 65536), remainder)` for unsigned input.
#[inline(always)]
const fn scale_u32_q16_parts(value: u32, fraction: u32) -> (u32, u32) {
    let high = value >> 16;
    let low_product = (value & 0xffff) * fraction;
    (high * fraction + (low_product >> 16), low_product & 0xffff)
}

/// Overflow-safe interpolation across the complete i32 range. This is equal
/// to `start + floor((end - start) * fraction / 65536)` without ever forming
/// the potentially overflowing signed difference or a 64-bit division.
pub const fn lerp_component_q16(start: i32, end: i32, fraction_q16: u32) -> i32 {
    let fraction = if fraction_q16 > (1 << 16) {
        1 << 16
    } else {
        fraction_q16
    };
    let biased_start = (start as u32) ^ 0x8000_0000;
    let biased_end = (end as u32) ^ 0x8000_0000;
    let biased_result = if biased_end >= biased_start {
        let distance = biased_end - biased_start;
        let (scaled, _) = scale_u32_q16_parts(distance, fraction);
        biased_start + scaled
    } else {
        let distance = biased_start - biased_end;
        let (floor, remainder) = scale_u32_q16_parts(distance, fraction);
        // floor(-x) = -ceil(x).
        let scaled = floor + if remainder != 0 { 1 } else { 0 };
        biased_start - scaled
    };
    (biased_result ^ 0x8000_0000) as i32
}

#[inline(always)]
pub const fn lerp_vec3_q16(start: Vec3Q8, end: Vec3Q8, fraction_q16: u32) -> Vec3Q8 {
    [
        lerp_component_q16(start[0], end[0], fraction_q16),
        lerp_component_q16(start[1], end[1], fraction_q16),
        lerp_component_q16(start[2], end[2], fraction_q16),
    ]
}

#[inline(always)]
const fn ordered_abs_diff(a: i32, b: i32) -> u32 {
    let aa = (a as u32) ^ 0x8000_0000;
    let bb = (b as u32) ^ 0x8000_0000;
    if aa >= bb {
        aa - bb
    } else {
        bb - aa
    }
}

/// Integer square root, rounded down. Shift/subtract only; no division.
const fn isqrt_u64(mut operand: u64) -> u64 {
    let mut result = 0u64;
    let mut bit = 1u64 << 62;
    while bit > operand {
        bit >>= 2;
    }
    while bit != 0 {
        if operand >= result + bit {
            operand -= result + bit;
            result = (result >> 1) + bit;
        } else {
            result >>= 1;
        }
        bit >>= 2;
    }
    result
}

/// Three-dimensional Q8 distance, rounded down and saturated to i32.
pub const fn distance_q8(a: Vec3Q8, b: Vec3Q8) -> i32 {
    let dx = ordered_abs_diff(a[0], b[0]) as u64;
    let dy = ordered_abs_diff(a[1], b[1]) as u64;
    let dz = ordered_abs_diff(a[2], b[2]) as u64;
    let squared = dx
        .saturating_mul(dx)
        .saturating_add(dy.saturating_mul(dy))
        .saturating_add(dz.saturating_mul(dz));
    let length = isqrt_u64(squared);
    if length > i32::MAX as u64 {
        i32::MAX
    } else {
        length as i32
    }
}

#[inline(always)]
const fn saturating_delta(to: i32, from: i32) -> i32 {
    to.saturating_sub(from)
}

#[inline(always)]
const fn vec_delta(to: Vec3Q8, from: Vec3Q8) -> Vec3Q8 {
    [
        saturating_delta(to[0], from[0]),
        saturating_delta(to[1], from[1]),
        saturating_delta(to[2], from[2]),
    ]
}

#[inline(always)]
const fn half_chord(to: Vec3Q8, from: Vec3Q8) -> Vec3Q8 {
    let delta = vec_delta(to, from);
    [delta[0] / 2, delta[1] / 2, delta[2] / 2]
}

fn project_past_pointer<P: PathSource + ?Sized>(
    path: &P,
    pointer: usize,
    fallback: Vec3Q8,
    distance: i32,
) -> (Vec3Q8, bool) {
    if pointer == 0 || pointer >= path.len() || distance <= 0 {
        return (fallback, false);
    }
    let start = path.point(pointer - 1).position_q8;
    let end = path.point(pointer).position_q8;
    let length = distance_q8(start, end);
    if length <= 0 {
        return (end, true);
    }
    let whole = distance / length;
    let remainder = distance % length;
    let fraction = ratio_q16(remainder as u32, length as u32);
    let direction = vec_delta(end, start);
    let mut out = end;
    let mut axis = 0;
    while axis < 3 {
        let full = direction[axis].saturating_mul(whole);
        let partial = scale_i32_q16_floor(direction[axis], fraction);
        out[axis] = out[axis].saturating_add(full).saturating_add(partial);
        axis += 1;
    }
    (out, true)
}

/// Forward GoldSrc-style lookahead from an actual origin and an old path
/// pointer. Segment zero is therefore `actual_origin -> next waypoint`, not
/// `current waypoint -> next waypoint`.
pub fn look_ahead_q8<P: PathSource + ?Sized>(
    path: &P,
    start_pointer: usize,
    actual_origin_q8: Vec3Q8,
    distance_ahead_q8: i32,
    policy: LookAheadPolicy,
) -> LookAheadResult {
    if path.len() == 0 || start_pointer >= path.len() {
        return LookAheadResult {
            point_q8: actual_origin_q8,
            returned_pointer: None,
            last_pointer: NO_POINTER,
            stop: LookAheadStop::InvalidStart,
            projected: false,
        };
    }

    let mut pointer = start_pointer;
    let mut current = actual_origin_q8;
    let mut remaining = distance_ahead_q8.max(0);
    let original_distance = remaining;

    while remaining > 0 {
        let current_stop = path.point(pointer).flags & policy.stop_after_flags;
        if current_stop != 0 {
            return LookAheadResult {
                point_q8: current,
                returned_pointer: None,
                last_pointer: pointer,
                stop: LookAheadStop::StopAfter(current_stop),
                projected: false,
            };
        }

        let next_pointer = pointer + 1;
        if next_pointer >= path.len() {
            let (point_q8, projected) = if policy.project_past_end {
                project_past_pointer(path, pointer, current, remaining)
            } else {
                (current, false)
            };
            return LookAheadResult {
                point_q8,
                returned_pointer: None,
                last_pointer: pointer,
                stop: LookAheadStop::DeadEnd,
                projected,
            };
        }

        let next = path.point(next_pointer);
        let blocked = next.flags & policy.blocked_next_flags;
        if blocked != 0 {
            let (point_q8, projected) = if policy.project_past_end {
                project_past_pointer(path, pointer, current, remaining)
            } else {
                (current, false)
            };
            return LookAheadResult {
                point_q8,
                returned_pointer: None,
                last_pointer: pointer,
                stop: LookAheadStop::BlockedNext(blocked),
                projected,
            };
        }

        let segment_length = distance_q8(current, next.position_q8);
        if segment_length == 0 {
            // Preserve CPathTrack's terminal-duplicate hack. At a zero-length
            // final link it returns null if no distance has yet been consumed,
            // otherwise it returns the current pointer without advancing into
            // the duplicate terminal node.
            let after_pointer = next_pointer + 1;
            let (after_valid, terminal_stop) = if after_pointer < path.len() {
                let after_flags = path.point(after_pointer).flags & policy.blocked_next_flags;
                (after_flags == 0, after_flags)
            } else {
                (false, 0)
            };
            if !after_valid {
                if remaining == original_distance {
                    return LookAheadResult {
                        point_q8: current,
                        returned_pointer: None,
                        last_pointer: pointer,
                        stop: if terminal_stop != 0 {
                            LookAheadStop::BlockedNext(terminal_stop)
                        } else {
                            LookAheadStop::DeadEnd
                        },
                        projected: false,
                    };
                }
                return LookAheadResult {
                    point_q8: current,
                    returned_pointer: Some(pointer),
                    last_pointer: pointer,
                    stop: LookAheadStop::Reached,
                    projected: false,
                };
            }
        }
        if segment_length > remaining {
            let fraction = ratio_q16(remaining as u32, segment_length as u32);
            return LookAheadResult {
                point_q8: lerp_vec3_q16(current, next.position_q8, fraction),
                returned_pointer: Some(pointer),
                last_pointer: pointer,
                stop: LookAheadStop::Reached,
                projected: false,
            };
        }

        // Exact endpoint equality advances the pointer, matching LookAhead's
        // strict `length > dist` comparison. Zero-length links also progress.
        remaining -= segment_length;
        pointer = next_pointer;
        current = next.position_q8;
    }

    LookAheadResult {
        point_q8: current,
        returned_pointer: Some(pointer),
        last_pointer: pointer,
        stop: LookAheadStop::Reached,
        projected: false,
    }
}

/// Prepare one forward follower update. Both queries intentionally receive
/// `old_pointer` and `actual_origin_q8`; committing `next_pointer` earlier is a
/// visible angular-controller regression at bends.
pub fn prepare_follower_q8<P: PathSource + ?Sized>(
    path: &P,
    old_pointer: usize,
    actual_origin_q8: Vec3Q8,
    position_lookahead_q8: i32,
    wheels_lookahead_q8: i32,
    position_policy: LookAheadPolicy,
) -> FollowerPreparation {
    let position = look_ahead_q8(
        path,
        old_pointer,
        actual_origin_q8,
        position_lookahead_q8,
        position_policy,
    );
    let wheels = look_ahead_q8(
        path,
        old_pointer,
        actual_origin_q8,
        wheels_lookahead_q8,
        GOLDSRC_WHEELS_POLICY,
    );
    let next_pointer = position.returned_pointer.unwrap_or(old_pointer);
    let passed_pointer = if position.returned_pointer.is_some() && next_pointer != old_pointer {
        Some(next_pointer)
    } else {
        None
    };
    FollowerPreparation {
        old_pointer,
        next_pointer,
        passed_pointer,
        position,
        wheels,
        prepared_half_chord_q8: half_chord(position.point_q8, actual_origin_q8),
        wheels_delta_q8: vec_delta(wheels.point_q8, actual_origin_q8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn p(x: i32, y: i32, z: i32) -> PathPoint {
        PathPoint {
            position_q8: [units_to_q8(x), units_to_q8(y), units_to_q8(z)],
            flags: 0,
        }
    }

    const fn flagged(x: i32, y: i32, z: i32, flags: u8) -> PathPoint {
        PathPoint {
            position_q8: [units_to_q8(x), units_to_q8(y), units_to_q8(z)],
            flags,
        }
    }

    fn assert_vec_within(actual: Vec3Q8, expected: Vec3Q8, tolerance: i32) {
        for axis in 0..3 {
            assert!(
                (actual[axis] - expected[axis]).abs() <= tolerance,
                "axis={} actual={:?} expected={:?} tolerance={}",
                axis,
                actual,
                expected,
                tolerance
            );
        }
    }

    #[test]
    fn straight_path_starts_at_actual_origin_not_current_waypoint() {
        let path = [p(0, 0, 0), p(100, 0, 0), p(200, 0, 0)];
        let actual = [units_to_q8(25), 0, 0];
        let prepared = prepare_follower_q8(
            &path,
            0,
            actual,
            units_to_q8(20),
            units_to_q8(40),
            GOLDSRC_MOVE_POLICY,
        );
        // Q16 interpolation floors a non-power-of-two ratio by at most one
        // Q8 quantum; pointer/chord semantics remain exact.
        assert_vec_within(prepared.position.point_q8, [units_to_q8(45), 0, 0], 1);
        assert_vec_within(prepared.prepared_half_chord_q8, [units_to_q8(10), 0, 0], 1);
        assert_vec_within(prepared.wheels.point_q8, [units_to_q8(65), 0, 0], 1);
        assert_eq!(prepared.next_pointer, 0);
        assert_eq!(prepared.passed_pointer, None);
    }

    #[test]
    fn bend_prepares_a_chord_and_wheels_still_use_the_old_pointer() {
        // The power-of-two second-segment length makes all expected Q8 values
        // exact while still exposing old-pointer ordering.
        let path = [p(0, 0, 0), p(10, 0, 0), p(10, 0, 8)];
        let actual = [units_to_q8(8), 0, 0];
        let prepared = prepare_follower_q8(
            &path,
            0,
            actual,
            units_to_q8(4),
            units_to_q8(6),
            GOLDSRC_MOVE_POLICY,
        );
        assert_eq!(
            prepared.position.point_q8,
            [units_to_q8(10), 0, units_to_q8(2)]
        );
        assert_eq!(
            prepared.prepared_half_chord_q8,
            [units_to_q8(1), 0, units_to_q8(1)]
        );
        assert_eq!(prepared.next_pointer, 1);
        assert_eq!(prepared.passed_pointer, Some(1));
        assert_eq!(
            prepared.wheels.point_q8,
            [units_to_q8(10), 0, units_to_q8(4)]
        );

        // Advancing the pointer before the wheels query instead aims directly
        // from the off-node actual origin toward node 2, producing a different
        // target. This guards the most important ordering rule in Next().
        let wrong = look_ahead_q8(
            &path,
            prepared.next_pointer,
            actual,
            units_to_q8(6),
            GOLDSRC_WHEELS_POLICY,
        );
        assert_ne!(wrong.point_q8, prepared.wheels.point_q8);
    }

    #[test]
    fn a_query_can_skip_nodes_but_only_returns_the_final_pointer() {
        let path = [p(0, 0, 0), p(10, 0, 0), p(20, 0, 0), p(30, 0, 0)];
        let prepared =
            prepare_follower_q8(&path, 0, [0; 3], units_to_q8(25), 0, GOLDSRC_MOVE_POLICY);
        assert_eq!(prepared.position.point_q8, [units_to_q8(25), 0, 0]);
        assert_eq!(prepared.position.returned_pointer, Some(2));
        assert_eq!(prepared.next_pointer, 2);
        assert_eq!(prepared.passed_pointer, Some(2));
        assert_eq!(
            prepared.prepared_half_chord_q8,
            [units_to_q8(12) + 128, 0, 0]
        );
    }

    #[test]
    fn dead_end_returns_null_at_the_terminal_but_wheels_project() {
        let path = [p(0, 0, 0), p(10, 0, 0)];
        let actual = [units_to_q8(8), 0, 0];
        let prepared = prepare_follower_q8(
            &path,
            0,
            actual,
            units_to_q8(30),
            units_to_q8(30),
            GOLDSRC_MOVE_POLICY,
        );
        assert_eq!(prepared.position.point_q8, [units_to_q8(10), 0, 0]);
        assert_eq!(prepared.position.returned_pointer, None);
        assert_eq!(prepared.position.last_pointer, 1);
        assert_eq!(prepared.position.stop, LookAheadStop::DeadEnd);
        assert_eq!(prepared.next_pointer, 0);
        assert_eq!(prepared.prepared_half_chord_q8, [units_to_q8(1), 0, 0]);
        assert_vec_within(prepared.wheels.point_q8, [units_to_q8(38), 0, 0], 1);
        assert_eq!(prepared.wheels.stop, LookAheadStop::DeadEnd);
        assert!(prepared.wheels.projected);
    }

    #[test]
    fn movement_respects_disabled_nodes_but_wheels_ignore_them() {
        let path = [p(0, 0, 0), flagged(10, 0, 0, PATH_DISABLED), p(20, 0, 0)];
        let prepared = prepare_follower_q8(
            &path,
            0,
            [0; 3],
            units_to_q8(5),
            units_to_q8(15),
            GOLDSRC_MOVE_POLICY,
        );
        assert_eq!(prepared.position.point_q8, [0; 3]);
        assert_eq!(
            prepared.position.stop,
            LookAheadStop::BlockedNext(PATH_DISABLED)
        );
        assert_eq!(prepared.position.returned_pointer, None);
        assert_eq!(prepared.wheels.point_q8, [units_to_q8(15), 0, 0]);
        assert_eq!(prepared.wheels.returned_pointer, Some(1));
    }

    #[test]
    fn phase_marker_allows_arrival_then_stops_outgoing_traversal() {
        let path = [p(0, 0, 0), flagged(10, 0, 0, PATH_PHASE_END), p(20, 0, 0)];
        let prepared = prepare_follower_q8(
            &path,
            0,
            [0; 3],
            units_to_q8(15),
            units_to_q8(15),
            PHASE_AWARE_MOVE_POLICY,
        );
        assert_eq!(prepared.position.point_q8, [units_to_q8(10), 0, 0]);
        assert_eq!(
            prepared.position.stop,
            LookAheadStop::StopAfter(PATH_PHASE_END)
        );
        assert_eq!(prepared.position.last_pointer, 1);
        assert_eq!(prepared.position.returned_pointer, None);
        assert_eq!(prepared.wheels.point_q8, [units_to_q8(15), 0, 0]);
    }

    #[test]
    fn exact_endpoint_advances_pointer_but_zero_distance_does_not() {
        let path = [p(0, 0, 0), p(10, 0, 0), p(20, 0, 0)];
        let exact = look_ahead_q8(&path, 0, [0; 3], units_to_q8(10), GOLDSRC_MOVE_POLICY);
        assert_eq!(exact.point_q8, [units_to_q8(10), 0, 0]);
        assert_eq!(exact.returned_pointer, Some(1));

        let actual = [units_to_q8(3), units_to_q8(-2), units_to_q8(7)];
        let zero = look_ahead_q8(&path, 0, actual, 0, GOLDSRC_MOVE_POLICY);
        assert_eq!(zero.point_q8, actual);
        assert_eq!(zero.returned_pointer, Some(0));
    }

    #[test]
    fn duplicate_points_progress_without_allocating_or_looping() {
        let path = [p(0, 0, 0), p(0, 0, 0), p(0, 0, 0), p(8, 0, 0)];
        let result = look_ahead_q8(&path, 0, [0; 3], units_to_q8(2), GOLDSRC_MOVE_POLICY);
        assert_eq!(result.point_q8, [units_to_q8(2), 0, 0]);
        assert_eq!(result.returned_pointer, Some(2));
    }

    #[test]
    fn terminal_duplicate_matches_goldsrcs_dead_end_hack() {
        let immediate = [p(0, 0, 0), p(0, 0, 0)];
        let result = look_ahead_q8(&immediate, 0, [0; 3], units_to_q8(2), GOLDSRC_MOVE_POLICY);
        assert_eq!(result.point_q8, [0; 3]);
        assert_eq!(result.returned_pointer, None);
        assert_eq!(result.last_pointer, 0);
        assert_eq!(result.stop, LookAheadStop::DeadEnd);

        let after_progress = [p(0, 0, 0), p(5, 0, 0), p(5, 0, 0)];
        let result = look_ahead_q8(
            &after_progress,
            0,
            [0; 3],
            units_to_q8(8),
            GOLDSRC_MOVE_POLICY,
        );
        assert_eq!(result.point_q8, [units_to_q8(5), 0, 0]);
        assert_eq!(result.returned_pointer, Some(1));
        assert_eq!(result.last_pointer, 1);
        assert_eq!(result.stop, LookAheadStop::Reached);
    }

    #[test]
    fn restoring_ratio_matches_an_exhaustive_small_integer_oracle() {
        for denominator in 1u32..=512 {
            for numerator in 0..=denominator {
                assert_eq!(
                    ratio_q16(numerator, denominator),
                    ((numerator as u64 * 65_536) / denominator as u64) as u32,
                    "{numerator}/{denominator}"
                );
            }
        }
    }

    #[test]
    fn interpolation_matches_wide_oracle_including_i32_extremes() {
        let values = [
            i32::MIN,
            i32::MIN + 1,
            -1_000_000,
            -1,
            0,
            1,
            1_000_000,
            i32::MAX - 1,
            i32::MAX,
        ];
        let fractions = [0, 1, 255, 256, 32_767, 32_768, 65_535, 65_536];
        for &start in &values {
            for &end in &values {
                for &fraction in &fractions {
                    let expected =
                        start as i128 + (((end as i128 - start as i128) * fraction as i128) >> 16);
                    assert_eq!(
                        lerp_component_q16(start, end, fraction),
                        expected as i32,
                        "start={start} end={end} fraction={fraction}"
                    );
                }
            }
        }

        // Dense coverage around zero catches signed-floor and carry mistakes.
        for start in -64..=64 {
            for end in -64..=64 {
                let mut fraction = 0u32;
                while fraction <= 65_536 {
                    let expected =
                        start as i64 + (((end as i64 - start as i64) * fraction as i64) >> 16);
                    assert_eq!(lerp_component_q16(start, end, fraction), expected as i32);
                    if fraction == 65_536 {
                        break;
                    }
                    fraction = (fraction + 257).min(65_536);
                }
            }
        }
    }

    #[test]
    fn straight_path_exhaustively_preserves_pointer_boundaries() {
        let path = [p(0, 0, 0), p(100, 0, 0), p(200, 0, 0)];
        let terminal = units_to_q8(200);
        for distance in 0..=units_to_q8(220) {
            let result = look_ahead_q8(&path, 0, [0; 3], distance, GOLDSRC_MOVE_POLICY);
            if distance <= terminal {
                // Q16 ratio flooring can place an interior point at most one
                // Q8 quantum behind the exact axial result.
                assert!(result.point_q8[0] <= distance);
                assert!(distance - result.point_q8[0] <= 1);
                let expected_pointer = if distance < units_to_q8(100) {
                    0
                } else if distance < terminal {
                    1
                } else {
                    2
                };
                assert_eq!(result.returned_pointer, Some(expected_pointer));
                assert_eq!(result.stop, LookAheadStop::Reached);
            } else {
                assert_eq!(result.point_q8[0], terminal);
                assert_eq!(result.returned_pointer, None);
                assert_eq!(result.last_pointer, 2);
                assert_eq!(result.stop, LookAheadStop::DeadEnd);
            }
        }
    }

    #[test]
    fn conversion_helpers_do_not_overflow() {
        assert_eq!(tenth_second_distance_q8(300), units_to_q8(30));
        assert_eq!(tenth_second_distance_q8(333), units_to_q8(33) + 76);
        assert_eq!(tenth_second_distance_q8(-1), 0);
        assert_eq!(units_to_q8(i32::MAX), i32::MAX);
        assert_eq!(units_to_q8(i32::MIN), i32::MIN);
    }
}
