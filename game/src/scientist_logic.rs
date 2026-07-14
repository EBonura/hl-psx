//! Pure scientist-follow and scripted-sequence selection rules.
//!
//! The runtime keeps the actual flags in spare bits of `PROP_OCC_VIS` and the
//! transition mailbox's state byte.  Keeping the decisions here makes the
//! progression-critical rules host-testable without pulling in the PSX runtime.

pub const SCRIPT_SELECTOR_NONE: u8 = 0;
pub const SCRIPT_SELECTOR_MASK: u8 = 0x3F;
pub const CARRY_FOLLOW_BIT: u8 = 0x80;
pub const CARRY_PROVOKED_BIT: u8 = 0x40;
pub const CARRY_PREDISASTER_BIT: u8 = 0x20;
const CARRY_STATE_MASK: u8 = 0x1F;
pub const SCRIPT_MOVE_TIMEOUT_MIN: u16 = 100;
pub const SCRIPT_MOVE_TIMEOUT_MAX: u16 = 1_200;
pub const SCRIPT_PRIMED_BIT: u8 = 0x80;
pub const SCRIPT_ROUTE_BIT: u8 = 0x40;
pub const SCRIPT_PRIME_DELAY_TICKS: u16 = 19;
/// GoldSrc `CCineMonster::CineThink` retries an unavailable scripted actor one
/// second later. The fixed simulation runs at 20 Hz.
pub const SCRIPT_RETRY_TICKS: u16 = 20;
/// Script schedules spend several 10 Hz monster thinks selecting a route and
/// enabling movement before their first step. Reuse the existing failed-move
/// cooldown for five active thinks (about 0.5 seconds), with no new state.
pub const SCRIPT_MOVE_START_ACTIVE_TICKS: u8 = 5;
/// Internal actor mode used after TASK_PLANT_ON_SCRIPT while GoldSrc runs
/// TASK_FACE_SCRIPT/TASK_FACE_IDEAL. It is outside the authored m_fMoveTo
/// range and lives in the existing script-mode byte.
pub const SCRIPT_FACE_MODE: u8 = 6;
/// ChangeYaw's first fixed-rate update may consume the 0.25 s clamp because
/// m_flLastYawTime is unset. At 120 deg/s with the Xash yaw-speed fix this is
/// 60 degrees, then 24 degrees on each 10 Hz monster think.
pub const SCRIPT_FACE_FIRST_STEP_Q12: u16 = 683;
pub const SCRIPT_FACE_STEP_Q12: u16 = 273;
pub const SCRIPT_FACE_FIRST_PENDING: u8 = 0x80;
/// Two 20 Hz ticks cover TASK_ENABLE_SCRIPT and TASK_WAIT_FOR_SCRIPT after the
/// actor reaches its ideal yaw. The completion itself runs on the next tick.
pub const SCRIPT_FACE_SETTLE_TICKS: u8 = 2;
/// `TASK_PLANT_ON_SCRIPT` is an exact placement task, not an ordinary monster
/// path goal. Keep the tolerance small so a scripted actor does not begin its
/// sequence from the general AI's 32-unit waypoint acceptance radius.
pub const SCRIPT_PLANT_RADIUS: i32 = 8;

#[inline(always)]
pub const fn script_at_mark(distance_sq: i32, start_pending: bool) -> bool {
    !start_pending && distance_sq <= SCRIPT_PLANT_RADIUS * SCRIPT_PLANT_RADIUS
}

/// Rotate a 12-bit yaw toward its target by the short arc, matching the
/// clamped steps produced by GoldSrc ChangeYaw.
#[inline(always)]
pub const fn script_face_yaw_step(cur: u16, target: u16, first: bool) -> u16 {
    let diff = (target.wrapping_sub(cur) & 0x0fff) as i32;
    let signed = if diff > 2048 { diff - 4096 } else { diff };
    let rate = if first {
        SCRIPT_FACE_FIRST_STEP_Q12
    } else {
        SCRIPT_FACE_STEP_Q12
    } as i32;
    let step = if signed < -rate {
        -rate
    } else if signed > rate {
        rate
    } else {
        signed
    };
    ((cur as i32 + step) & 0x0fff) as u16
}

/// Deterministic actor-aware scripted movement. The source MDLs report
/// scientist walk/run at 58.750/275.351 u/s and Barney at 61.414/362.384.
/// Sixteen active 10 Hz thinks provide exact cheap walk cadences without
/// fractional actor state; run rounds to 280/360 u/s.
#[inline(always)]
pub const fn script_move_speed(mode: u8, move_tick: u16, barney: bool) -> u16 {
    if mode == 2 {
        if barney {
            18
        } else {
            14
        }
    } else if (move_tick >> 1) & 15 == 15 {
        if barney {
            4
        } else {
            2
        }
    } else {
        3
    }
}

/// Advance one horizontal component using a centered signed-Q4 remainder.
/// The returned remainder is always in -8..=7, so a four-bit field is enough
/// to retain GoldSrc's sub-unit motion without another per-actor array.
#[inline(always)]
pub const fn script_q4_component_step(
    component: i32,
    step: i32,
    len: i32,
    residue: i32,
) -> (i32, i32) {
    if len <= 0 || step <= 0 {
        return (0, residue);
    }
    let numerator = component * step * 16;
    let qstep = if numerator >= 0 {
        (numerator + len / 2) / len
    } else {
        (numerator - len / 2) / len
    };
    let q = qstep + residue;
    let delta = (q + 8) >> 4;
    (delta, q - (delta << 4))
}

#[inline(always)]
pub const fn script_q4_decode(bits: u16) -> i32 {
    ((bits & 0xF) as i32 ^ 8) - 8
}

#[inline(always)]
pub const fn script_q4_encode(residue: i32) -> u16 {
    residue as u16 & 0xF
}

#[inline(always)]
pub const fn script_timeout_speed(mode: u8, barney: bool) -> u16 {
    if mode == 2 {
        if barney {
            18
        } else {
            14
        }
    } else {
        3
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptOwnedUse {
    /// This script owns no actor yet, so it may search for a free candidate.
    Search,
    /// A startup-primed actor belongs to this script and Use should start it.
    AssignPrimed,
    /// The same script is already playing; GoldSrc ignores the duplicate Use.
    IgnorePlaying,
}

#[inline(always)]
pub const fn script_owned_use(has_owner: bool, owner_is_primed: bool) -> ScriptOwnedUse {
    if !has_owner {
        ScriptOwnedUse::Search
    } else if owner_is_primed {
        ScriptOwnedUse::AssignPrimed
    } else {
        ScriptOwnedUse::IgnorePlaying
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptPrimeGate {
    /// This is an ordinary fired script; priming does not constrain it.
    Execute,
    /// The actor is owned by the script but the one-second CineThink is pending,
    /// or it has reached its mark and must remain there until Use.
    Hold,
    /// A primed walk/run has reached CineThink and must arm its route timeout.
    StartMove,
}

/// A targeted scripted_sequence with m_iszIdle possesses and positions its
/// actor during map startup, then waits for Use. The high bit reuses the
/// existing mode byte so this waiting ownership costs no per-prop RAM.
#[inline(always)]
pub const fn script_primed_mode(mode: u8) -> u8 {
    SCRIPT_PRIMED_BIT | (mode & !SCRIPT_PRIMED_BIT)
}

#[inline(always)]
pub const fn script_is_primed(mode: u8) -> bool {
    mode & SCRIPT_PRIMED_BIT != 0
}

#[inline(always)]
pub const fn script_base_mode(mode: u8) -> u8 {
    mode & !(SCRIPT_PRIMED_BIT | SCRIPT_ROUTE_BIT)
}

#[inline(always)]
pub const fn script_route_mode(mode: u8, routed: bool) -> u8 {
    if routed {
        mode | SCRIPT_ROUTE_BIT
    } else {
        mode & !SCRIPT_ROUTE_BIT
    }
}

#[inline(always)]
pub const fn script_uses_route(mode: u8) -> bool {
    mode & SCRIPT_ROUTE_BIT != 0
}

/// Decide whether a targeted script with `m_iszIdle` may act this tick.
///
/// The existing actor state distinguishes a walk/run whose route deadline is
/// already live from one still waiting for GoldSrc's one-second CineThink. This
/// lets both deadlines reuse the gesture timer without another per-actor byte.
#[inline(always)]
pub const fn script_prime_gate(
    encoded_mode: u8,
    actor_is_moving: bool,
    start_deadline_reached: bool,
) -> ScriptPrimeGate {
    if !script_is_primed(encoded_mode) {
        return ScriptPrimeGate::Execute;
    }
    match script_base_mode(encoded_mode) {
        0 => ScriptPrimeGate::Hold,
        SCRIPT_FACE_MODE => ScriptPrimeGate::Execute,
        1 | 2 if actor_is_moving => ScriptPrimeGate::Execute,
        1 | 2 if start_deadline_reached => ScriptPrimeGate::StartMove,
        1 | 2 => ScriptPrimeGate::Hold,
        _ if start_deadline_reached => ScriptPrimeGate::Execute,
        _ => ScriptPrimeGate::Hold,
    }
}

/// Untargeted scripted_sequences with only an idle sequence possess their
/// actor forever in GoldSrc; they do not immediately complete and fire their
/// ordinary target. Authored-key flags are used instead of cooked clip
/// availability so an unsupported animation cannot change entity semantics.
#[inline(always)]
pub const fn script_untargeted_idle_holds(has_idle: bool, has_play: bool) -> bool {
    has_idle && !has_play
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FollowUse {
    Ignore,
    Start,
    Stop,
}

#[inline(always)]
pub const fn selector_kind(code: u8) -> Option<u8> {
    let code = code & SCRIPT_SELECTOR_MASK;
    if code == SCRIPT_SELECTOR_NONE {
        None
    } else {
        Some(code - 1)
    }
}

/// GoldSrc `FollowerUse` distilled to the state this port stores.
///
/// A current follower may always be dismissed.  A dead, pre-disaster, or
/// player-provoked scientist never starts following.  An interruptible script
/// may be cancelled by StartFollowing; a NOINTERRUPT script may not.
#[inline(always)]
pub const fn follow_use(
    alive: bool,
    following: bool,
    predisaster: bool,
    provoked: bool,
    scripted: bool,
    script_nointerrupt: bool,
) -> FollowUse {
    if !alive {
        FollowUse::Ignore
    } else if following {
        FollowUse::Stop
    } else if predisaster || provoked || (scripted && script_nointerrupt) {
        FollowUse::Ignore
    } else {
        FollowUse::Start
    }
}

#[inline]
fn angular_score(vx: i32, vy: i32, vz: i32) -> Option<i32> {
    if vz <= 0 {
        return None;
    }
    let ax = vx.saturating_abs();
    let ay = vy.saturating_abs();
    let side2 = (ax as i64)
        .saturating_mul(ax as i64)
        .saturating_add((ay as i64).saturating_mul(ay as i64));
    let forward2 = (vz as i64).saturating_mul(vz as i64).max(1);
    Some(((side2 << 16) / forward2).min(i32::MAX as i64) as i32)
}

/// Existing generous brush cone, scored by normalized view angle.
#[inline]
pub fn brush_use_score(vx: i32, vy: i32, vz: i32, reach: i32) -> Option<i32> {
    if vz <= 0 || vz > reach {
        return None;
    }
    let ax = vx.saturating_abs();
    let ay = vy.saturating_abs();
    if ax.saturating_mul(2) >= vz.saturating_mul(3) || ay.saturating_mul(2) >= vz.saturating_mul(3)
    {
        return None;
    }
    angular_score(vx, vy, vz)
}

/// GoldSrc `FIND_ENTITY_IN_SPHERE` measures from the search point to the
/// nearest point of each entity AABB, not to its origin/centre. The comparison
/// is strict (`distance^2 < radius^2`). Rejecting an axis outside the small
/// search cube first keeps all remaining squares safely inside i32.
#[inline]
pub fn aabb_in_search_radius(point: [i32; 3], mins: [i32; 3], maxs: [i32; 3], radius: i32) -> bool {
    if radius <= 0 {
        return false;
    }
    let mut distance_sq = 0i32;
    let mut axis = 0usize;
    while axis < 3 {
        let delta = if point[axis] < mins[axis] {
            mins[axis] - point[axis]
        } else if point[axis] > maxs[axis] {
            point[axis] - maxs[axis]
        } else {
            0
        };
        if delta >= radius {
            return false;
        }
        distance_sq += delta * delta;
        axis += 1;
    }
    distance_sq < radius * radius
}

/// Vector from `point` to the nearest point of an AABB. This is the exact
/// geometric result of GoldSrc's `VecBModelOrigin - eye` followed by
/// `UTIL_ClampVectorToBox(..., size * 0.5)` before normalization.
#[inline]
pub fn nearest_aabb_delta(point: [i32; 3], mins: [i32; 3], maxs: [i32; 3]) -> [i32; 3] {
    let mut out = [0i32; 3];
    let mut axis = 0usize;
    while axis < 3 {
        out[axis] = if point[axis] < mins[axis] {
            mins[axis] - point[axis]
        } else if point[axis] > maxs[axis] {
            maxs[axis] - point[axis]
        } else {
            0
        };
        axis += 1;
    }
    out
}

/// GoldSrc PlayerUse's normalized forward-dot gate (`VIEW_FIELD_NARROW =
/// 0.7`), scored so a smaller integer is a better-centred candidate.
#[inline]
pub fn player_use_score(vx: i32, vy: i32, vz: i32) -> Option<i32> {
    scientist_use_score(vx, vy, vz)
}

/// GoldSrc scientist PlayerUse view gate: normalized forward dot strictly >0.7.
#[inline]
pub fn scientist_use_score(vx: i32, vy: i32, vz: i32) -> Option<i32> {
    if vz <= 0 {
        return None;
    }
    let side2 = (vx as i64)
        .saturating_mul(vx as i64)
        .saturating_add((vy as i64).saturating_mul(vy as i64));
    let forward2 = (vz as i64).saturating_mul(vz as i64);
    // vz/sqrt(vz^2+side^2) > 7/10  <=>  51*vz^2 > 49*side^2.
    if forward2.saturating_mul(51) <= side2.saturating_mul(49) {
        return None;
    }
    angular_score(vx, vy, vz)
}

/// Vector from the player eye to the nearest point of a standing scientist's
/// GoldSrc 32x32x72 bbox, matching UTIL_ClampVectorToBox.
#[inline]
pub fn scientist_bbox_delta(eye: [i32; 3], actor_origin: [i32; 3]) -> [i32; 3] {
    let center = [actor_origin[0], actor_origin[1] + 36, actor_origin[2]];
    let half = [16, 36, 16];
    let mut out = [0; 3];
    let mut axis = 0usize;
    while axis < 3 {
        let delta = center[axis] - eye[axis];
        out[axis] = if delta > half[axis] {
            delta - half[axis]
        } else if delta < -half[axis] {
            delta + half[axis]
        } else {
            0
        };
        axis += 1;
    }
    out
}

/// Eligibility for classname selection. Runtime keeps the first viable actor in
/// cooked/engine order, exactly like UTIL_FindEntityInSphere enumeration.
#[inline]
pub fn script_candidate_eligible(
    kind: u8,
    wanted_kind: u8,
    active: bool,
    health: u8,
    script_busy: bool,
    delta: [i32; 3],
    radius: i32,
) -> bool {
    if kind != wanted_kind || !active || health == 0 || script_busy || radius < 0 {
        return false;
    }
    let d2 = (delta[0] as i64)
        .saturating_mul(delta[0] as i64)
        .saturating_add((delta[1] as i64).saturating_mul(delta[1] as i64))
        .saturating_add((delta[2] as i64).saturating_mul(delta[2] as i64));
    let r2 = (radius as i64).saturating_mul(radius as i64);
    d2 <= r2
}

/// Fail-safe deadline for an authored walk/run sequence. It gives normal path
/// finding 2.5x the straight-line travel time plus 2.5 seconds, then allows the
/// runtime to plant the actor on the script mark so a cinematic can never hold
/// its completion target forever. The deadline reuses the existing gesture
/// timer; it does not add actor state.
#[inline]
pub fn script_move_timeout_ticks(distance: u32, speed_per_tick: u16) -> u16 {
    let speed = speed_per_tick.max(1) as u32;
    let ticks = 50u32.saturating_add(distance.saturating_mul(5) / (speed * 2));
    ticks.clamp(
        SCRIPT_MOVE_TIMEOUT_MIN as u32,
        SCRIPT_MOVE_TIMEOUT_MAX as u32,
    ) as u16
}

#[inline(always)]
pub const fn pack_carry_state(
    base_state: u8,
    following: bool,
    provoked: bool,
    predisaster: bool,
) -> u8 {
    (base_state & CARRY_STATE_MASK)
        | if following { CARRY_FOLLOW_BIT } else { 0 }
        | if provoked { CARRY_PROVOKED_BIT } else { 0 }
        | if predisaster {
            CARRY_PREDISASTER_BIT
        } else {
            0
        }
}

#[inline(always)]
pub const fn unpack_carry_state(state: u8) -> (u8, bool, bool, bool) {
    (
        state & CARRY_STATE_MASK,
        state & CARRY_FOLLOW_BIT != 0,
        state & CARRY_PROVOKED_BIT != 0,
        state & CARRY_PREDISASTER_BIT != 0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follower_use_obeys_predisaster_provoked_dead_and_nointerrupt_rules() {
        assert_eq!(
            follow_use(true, false, false, false, false, false),
            FollowUse::Start
        );
        assert_eq!(
            follow_use(true, true, true, true, true, true),
            FollowUse::Stop
        );
        assert_eq!(
            follow_use(false, false, false, false, false, false),
            FollowUse::Ignore
        );
        assert_eq!(
            follow_use(true, false, true, false, false, false),
            FollowUse::Ignore
        );
        assert_eq!(
            follow_use(true, false, false, true, false, false),
            FollowUse::Ignore
        );
        assert_eq!(
            follow_use(true, false, false, false, true, true),
            FollowUse::Ignore
        );
        assert_eq!(
            follow_use(true, false, false, false, true, false),
            FollowUse::Start
        );
    }

    #[test]
    fn centred_brush_and_scientist_share_one_deterministic_score() {
        let centred_brush = brush_use_score(2, 1, 80, 120).unwrap();
        let off_axis_scientist = scientist_use_score(22, 8, 70).unwrap();
        assert!(centred_brush < off_axis_scientist);
        assert_eq!(brush_use_score(0, 0, 121, 120), None);
        assert_eq!(scientist_use_score(52, 0, 50), None);
        assert!(scientist_use_score(51, 0, 50).is_some());
    }

    #[test]
    fn retinal_class_selector_uses_first_eligible_living_scientist() {
        let candidates = [
            (0, true, 40, false, [100, 20, 0]),
            (0, true, 40, false, [30, 10, 0]),
        ];
        let selected = candidates
            .iter()
            .position(|&(kind, active, hp, busy, delta)| {
                script_candidate_eligible(kind, 0, active, hp, busy, delta, 150)
            });
        assert_eq!(selected, Some(0), "engine enumeration order beats distance");
        assert!(!script_candidate_eligible(
            1,
            0,
            true,
            40,
            false,
            [1, 0, 0],
            150
        ));
        assert!(!script_candidate_eligible(
            0,
            0,
            true,
            0,
            false,
            [1, 0, 0],
            150
        ));
        assert!(!script_candidate_eligible(
            0,
            0,
            true,
            40,
            true,
            [1, 0, 0],
            150
        ));
        assert!(!script_candidate_eligible(
            0,
            0,
            true,
            40,
            false,
            [151, 0, 0],
            150
        ));
    }

    #[test]
    fn scientist_use_clamps_to_nearest_bbox_point() {
        assert_eq!(scientist_bbox_delta([0, 28, 0], [40, 0, 0]), [24, 0, 0]);
        assert_eq!(scientist_bbox_delta([0, 28, 0], [0, 0, 40]), [0, 0, 24]);
    }

    #[test]
    fn player_use_searches_nearest_brush_bounds_with_strict_radius() {
        let mins = [100, -20, -10];
        let maxs = [108, 20, 10];
        assert!(aabb_in_search_radius([44, 0, 0], mins, maxs, 64));
        assert!(!aabb_in_search_radius([36, 0, 0], mins, maxs, 64));
        assert!(aabb_in_search_radius([104, 0, 0], mins, maxs, 64));
        assert_eq!(nearest_aabb_delta([150, 5, -30], mins, maxs), [-42, 0, 20]);
    }

    #[test]
    fn player_use_matches_goldsrc_point_seven_forward_dot() {
        assert!(player_use_score(7, 0, 8).is_some());
        assert!(player_use_score(8, 0, 7).is_none());
        let centred = player_use_score(1, 0, 20).unwrap();
        let off_axis = player_use_score(7, 0, 20).unwrap();
        assert!(centred < off_axis);
    }

    #[test]
    fn follow_bit_round_trips_through_existing_transition_state_byte() {
        let packed = pack_carry_state(2, true, true, false);
        assert_eq!(packed, 0xC2);
        assert_eq!(unpack_carry_state(packed), (2, true, true, false));
        assert_eq!(selector_kind(1), Some(0));
        assert_eq!(selector_kind(SCRIPT_SELECTOR_NONE), None);
        assert_eq!(
            selector_kind(0x80),
            None,
            "HAS_IDLE is not a class selector"
        );
        assert_eq!(selector_kind(0x82), Some(1));
    }

    #[test]
    fn script_timeout_scales_with_route_and_is_bounded() {
        assert_eq!(script_move_timeout_ticks(0, 4), SCRIPT_MOVE_TIMEOUT_MIN);
        assert_eq!(script_move_timeout_ticks(800, 4), 550);
        assert_eq!(script_move_timeout_ticks(800, 8), 300);
        assert_eq!(
            script_move_timeout_ticks(u32::MAX, 1),
            SCRIPT_MOVE_TIMEOUT_MAX
        );
    }

    #[test]
    fn scripted_arrival_is_the_source_pre_move_eight_unit_gate() {
        assert!(script_at_mark(64, false));
        assert!(!script_at_mark(65, false));
        assert!(!script_at_mark(0, true));
    }

    #[test]
    fn scripted_walk_and_run_match_each_source_actor_rate() {
        let mut scientist_walk = 0u16;
        let mut barney_walk = 0u16;
        for active_think in 0..16 {
            let tick = active_think * 2;
            scientist_walk += script_move_speed(1, tick, false);
            barney_walk += script_move_speed(1, tick, true);
        }
        assert_eq!(scientist_walk, 47, "58.75 units/s at 20 Hz");
        assert_eq!(barney_walk, 49, "61.25 units/s at 20 Hz");
        assert_eq!(script_move_speed(2, 0, false), 14);
        assert_eq!(script_move_speed(2, 0, true), 18);
        assert_eq!(script_timeout_speed(1, false), 3);
        assert_eq!(script_timeout_speed(2, false), 14);
        assert_eq!(script_timeout_speed(2, true), 18);
    }

    #[test]
    fn scripted_face_uses_goldsrc_first_step_and_short_arc() {
        let deg = |d: u16| ((d as u32 * 4096 + 180) / 360) as u16 & 0x0fff;
        let target = 0;
        let first = script_face_yaw_step(deg(164), target, true);
        assert_eq!(first, deg(104));
        assert_eq!(script_face_yaw_step(first, target, false), deg(80));
        assert_eq!(script_face_yaw_step(deg(350), target, true), target);
        assert_eq!(script_face_yaw_step(deg(10), target, true), target);
    }

    #[test]
    fn scripted_q4_motion_retains_exact_centered_remainders() {
        for residue in -8..=7 {
            assert_eq!(script_q4_decode(script_q4_encode(residue)), residue);
            for component in -64..=64 {
                for step in 1..=36 {
                    for len in step..=96 {
                        let numerator = component * step * 16;
                        let rounded = if numerator >= 0 {
                            (numerator + len / 2) / len
                        } else {
                            (numerator - len / 2) / len
                        };
                        let (delta, next) = script_q4_component_step(component, step, len, residue);
                        assert!((-8..=7).contains(&next));
                        assert_eq!(delta * 16 + next, rounded + residue);
                    }
                }
            }
        }
    }

    #[test]
    fn primed_script_ownership_round_trips_in_the_existing_mode_byte() {
        for mode in 0..=4 {
            let primed = script_primed_mode(mode);
            assert!(script_is_primed(primed));
            assert_eq!(script_base_mode(primed), mode);
        }
        assert!(!script_is_primed(4));
        assert_eq!(script_base_mode(4), 4);

        let routed = script_route_mode(1, true);
        assert!(script_uses_route(routed));
        assert_eq!(script_base_mode(routed), 1);
        let primed_routed = script_primed_mode(routed);
        assert!(script_is_primed(primed_routed));
        assert!(script_uses_route(primed_routed));
        assert_eq!(script_base_mode(primed_routed), 1);
    }

    #[test]
    fn every_primed_move_mode_waits_for_cinethink_before_acting() {
        assert_eq!(SCRIPT_PRIME_DELAY_TICKS, 19);
        for mode in 1..=4 {
            let primed = script_primed_mode(mode);
            assert_eq!(
                script_prime_gate(primed, false, false),
                ScriptPrimeGate::Hold,
                "mode {mode} moved before the one-second startup deadline"
            );
        }
        assert_eq!(
            script_prime_gate(script_primed_mode(1), false, true),
            ScriptPrimeGate::StartMove
        );
        assert_eq!(
            script_prime_gate(script_primed_mode(2), false, true),
            ScriptPrimeGate::StartMove
        );
        assert_eq!(
            script_prime_gate(script_primed_mode(3), false, true),
            ScriptPrimeGate::Execute
        );
        assert_eq!(
            script_prime_gate(script_primed_mode(4), false, true),
            ScriptPrimeGate::Execute
        );
    }

    #[test]
    fn primed_walk_route_deadline_reuses_the_same_timer_after_start() {
        for mode in [1, 2] {
            assert_eq!(
                script_prime_gate(script_primed_mode(mode), true, false),
                ScriptPrimeGate::Execute
            );
        }
        assert_eq!(
            script_prime_gate(script_primed_mode(0), false, true),
            ScriptPrimeGate::Hold,
            "an actor already planted at its idle mark must wait for Use"
        );
        assert_eq!(script_prime_gate(2, false, false), ScriptPrimeGate::Execute);
    }

    #[test]
    fn primed_actor_keeps_turning_after_it_reaches_the_mark() {
        assert_eq!(
            script_prime_gate(script_primed_mode(SCRIPT_FACE_MODE), false, false),
            ScriptPrimeGate::Execute
        );
    }

    #[test]
    fn scripted_use_retries_busy_actors_but_not_its_own_playing_actor() {
        assert_eq!(script_owned_use(false, false), ScriptOwnedUse::Search);
        assert_eq!(script_owned_use(true, true), ScriptOwnedUse::AssignPrimed);
        assert_eq!(script_owned_use(true, false), ScriptOwnedUse::IgnorePlaying);
        assert_eq!(SCRIPT_RETRY_TICKS, 20);
        assert_eq!(
            u16::MAX.wrapping_sub(5).wrapping_add(SCRIPT_RETRY_TICKS),
            14,
            "retry deadlines must retain the runtime's wrapping-tick contract"
        );
    }

    #[test]
    fn untargeted_idle_only_script_holds_without_firing_its_output() {
        assert!(script_untargeted_idle_holds(true, false));
        assert!(!script_untargeted_idle_holds(true, true));
        assert!(!script_untargeted_idle_holds(false, false));
    }
}
