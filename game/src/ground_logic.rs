//! Pure ground-contact state transitions shared by player physics and host tests.

/// Resolve vertical state after the downward probe confirms a walkable floor.
///
/// GoldSrc's walk move does not carry an upward component clipped from a
/// staircase plane into the following frame. Downward airborne velocity is
/// still captured for fall damage before every grounded vertical component is
/// consumed.
pub const fn settle_vertical(vertical: i32, was_airborne: bool) -> (i32, i32) {
    let landing_impact = if was_airborne && vertical < 0 {
        -vertical
    } else {
        0
    };
    (0, landing_impact)
}

/// GoldSrc's grounded duck interpolation lasts 0.4 seconds. The fixed player
/// simulation runs at 20 Hz, so the collision hull changes after eight whole
/// ticks. `PM_Duck` completes the change immediately while airborne, but it
/// runs before `PM_Jump`: a ground duck+jump therefore moves once with hull 1
/// and changes to hull 3 at the start of the following tick.
pub const DUCK_TRANSITION_TICKS: u8 = 8;

/// Advance the pending standing -> crouch collision-hull transition.
///
/// Returns `(next_timer, activate_crouch_hull_now)`. The timer is external to
/// `Player`: it is map-local input state, which preserves the player's strict
/// 36-byte RAM footprint.
pub const fn advance_duck_activation(
    crouch_active: bool,
    duck_requested: bool,
    on_ground: bool,
    pending_ticks: u8,
) -> (u8, bool) {
    if crouch_active || !duck_requested {
        return (0, false);
    }
    if !on_ground {
        return (0, true);
    }
    if pending_ticks == 0 {
        return (DUCK_TRANSITION_TICKS, false);
    }
    let next = pending_ticks - 1;
    (next, next == 0)
}

/// Vertical origin adjustment when switching between GoldSrc player hulls.
///
/// Hull 1 is 72 units tall and hull 3 is 36 units tall. On the ground,
/// GoldSrc moves the centred origin down while ducking and back up while
/// standing so the feet remain on the same floor. In the air it changes the
/// hull without moving the origin. Duck-jump timing is handled by
/// [`advance_duck_activation`], so an activation that happens while still on
/// the ground always keeps the feet planted.
pub const fn crouch_origin_shift(on_ground: bool, want_crouch: bool) -> i32 {
    const HALF_HEIGHT_DELTA: i32 = 18;
    if !on_ground {
        0
    } else if want_crouch {
        -HALF_HEIGHT_DELTA
    } else {
        HALF_HEIGHT_DELTA
    }
}

/// Whether a grounded actor may accept the floor found under its next step.
///
/// GoldSrc's walking monsters move through `WALK_MOVE`, which is bounded by
/// the server's 18-unit step size. Descents remain legal here; the floor probe
/// separately limits their depth and verifies that a floor actually exists.
pub const fn actor_floor_step_reachable(current_y: i32, next_y: i32) -> bool {
    next_y - current_y <= 18
}

/// Whether two vertical collision hull spans overlap with positive depth.
///
/// GoldSrc headcrabs deal damage from `LeapTouch`, and the other melee actors
/// use a hull trace. Merely being close in X/Z is therefore insufficient: an
/// actor on a balcony (or a monstermaker child still falling from above) cannot
/// strike through the floor. Face-only contact is not an overlap, matching the
/// engine's touch convention at the exact boundary.
pub const fn melee_vertical_hulls_overlap(
    attacker_min: i32,
    attacker_max: i32,
    target_min: i32,
    target_max: i32,
) -> bool {
    attacker_min < target_max && attacker_max > target_min
}

/// Test a walking actor's candidate hull against the centred player hull.
/// Actor movement and player movement run in separate phases; both sides must
/// reject overlap or two approaching slideboxes can swap places in one tick.
pub const fn actor_candidate_overlaps_player(
    actor_mins: [i32; 3],
    actor_maxs: [i32; 3],
    player_pos: [i32; 3],
    player_half_height: i32,
) -> bool {
    let player_mins = [
        player_pos[0] - 16,
        player_pos[1] - player_half_height,
        player_pos[2] - 16,
    ];
    let player_maxs = [
        player_pos[0] + 16,
        player_pos[1] + player_half_height,
        player_pos[2] + 16,
    ];
    actor_mins[0] <= player_maxs[0]
        && actor_maxs[0] >= player_mins[0]
        && actor_mins[1] <= player_maxs[1]
        && actor_maxs[1] >= player_mins[1]
        && actor_mins[2] <= player_maxs[2]
        && actor_maxs[2] >= player_mins[2]
}

/// Convert one Q6 planar velocity into this tick's integer displacement while
/// retaining the sub-unit position error for later ticks.
///
/// GoldSrc keeps both velocity and origin as floats.  The PS1 collision hull
/// uses integer origins, so merely retaining a fractional velocity still moves
/// 16 units every tick for a 15.875-unit velocity.  Error diffusion produces
/// seven 16-unit steps and one 15-unit step instead, preserving the exact
/// eight-tick distance without making the BSP tracer fractional.
pub const fn integrate_planar_q6(velocity_q6: i32, carry_q6: i8) -> (i32, i8) {
    const ONE: i32 = 64;
    // Nearest integer with exact half-units kept as residue. Ties-away would
    // make a stopped ±0.5 carry oscillate one unit forever.
    const BELOW_HALF: i32 = ONE / 2 - 1;
    let total = velocity_q6 + carry_q6 as i32;
    let whole = if total >= 0 {
        (total + BELOW_HALF) / ONE
    } else {
        -((-total + BELOW_HALF) / ONE)
    };
    (whole, (total - whole * ONE) as i8)
}

/// Preserve the sub-unit vertical origin reached by a downward Q0.12 ground
/// trace.
///
/// The collision hull itself remains integer for PS1 throughput, but GoldSrc
/// keeps a float origin after landing on a slope. Keep the integer hull on the
/// empty/upward side of that contact (ceil in Y-up space), and retain the
/// negative Q6 remainder for the next movement/step calculation. Nearest
/// rounding can put the integer point *inside* the supporting slope, turning a
/// legal upward PM_WalkMove probe into `startsolid`.
pub const fn ground_contact_q6(start_y: i32, end_y: i32, frac_q12: i32) -> (i32, i8) {
    const ONE: i32 = 64;
    const HALF: i32 = ONE / 2;
    let product_q12 = (end_y - start_y) * frac_q12;
    let delta_q6 = if product_q12 >= 0 {
        (product_q12 + HALF) / ONE
    } else {
        -((-product_q12 + HALF) / ONE)
    };
    let fine_y = start_y * ONE + delta_q6;
    let whole = if fine_y >= 0 {
        (fine_y + ONE - 1) / ONE
    } else {
        -((-fine_y) / ONE)
    };
    (whole, (fine_y - whole * ONE) as i8)
}

/// Whether a slide impact plane can need a nearest-rounded contact retry.
///
/// The BSP trace has already backed the impact fraction toward empty space.
/// Component-wise arithmetic shifts can nevertheless put the resulting
/// integer point inside a diagonal plane because every negative component is
/// rounded down independently. Axial planes do not have that ambiguity: their
/// conservative floor is the correct side of the exact integer boundary.
///
/// Callers must prove both that the conservative contact is blocked and that
/// the nearest-rounded candidate is clear before accepting the retry.
#[inline(always)]
pub const fn is_diagonal_contact_plane(normal: [i32; 3]) -> bool {
    let axes = (normal[0] != 0) as u8 + (normal[1] != 0) as u8 + (normal[2] != 0) as u8;
    axes > 1
}

/// Result of sweeping the player's centred box against one actor AABB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActorSweep {
    /// Q0.12 fraction along the requested move.
    pub frac: i32,
    /// Blocked runtime axis: 0=X, 1=Y-up, 2=Z.
    pub axis: u8,
}

/// Whether a centred player origin is strictly inside an actor-expanded box.
///
/// Runtime coordinates are X/Y-up/Z.  Actors are authored from their feet,
/// while the standing and crouching player origins are centred in 72- and
/// 36-unit hulls respectively.  Touching a face is deliberately not "inside":
/// the following inward sweep must report fraction zero so wall sliding keeps
/// working, while a tangent move along that face remains legal.
#[cfg(test)]
pub const fn player_inside_actor(
    point: [i32; 3],
    actor_mins: [i32; 3],
    actor_maxs: [i32; 3],
    player_half_height: i32,
) -> bool {
    let mins = [
        actor_mins[0] - 16,
        actor_mins[1] - player_half_height,
        actor_mins[2] - 16,
    ];
    let maxs = [
        actor_maxs[0] + 16,
        actor_maxs[1] + player_half_height,
        actor_maxs[2] + 16,
    ];
    point[0] > mins[0]
        && point[0] < maxs[0]
        && point[1] > mins[1]
        && point[1] < maxs[1]
        && point[2] > mins[2]
        && point[2] < maxs[2]
}

/// Sweep a centred player box against one GoldSrc `SOLID_SLIDEBOX` AABB.
///
/// This is the same Minkowski expansion used by GoldSrc's `PM_HullForBox`:
/// actor mins minus player maxs, actor maxs minus player mins.  Fractions use
/// the engine's Q0.12 convention.  The 1/32-unit entry backoff is GoldSrc's
/// `DIST_EPSILON`; retaining it in fraction space costs no fractional origins.
///
/// A move beginning strictly inside an actor is ignored so an actor that moved
/// into the player cannot freeze them forever.
#[inline(never)]
pub fn sweep_player_actor(
    start: [i32; 3],
    end: [i32; 3],
    actor_mins: [i32; 3],
    actor_maxs: [i32; 3],
    player_half_height: i32,
) -> Option<ActorSweep> {
    let mins = [
        actor_mins[0] - 16,
        actor_mins[1] - player_half_height,
        actor_mins[2] - 16,
    ];
    let maxs = [
        actor_maxs[0] + 16,
        actor_maxs[1] + player_half_height,
        actor_maxs[2] + 16,
    ];
    if start[0] > mins[0]
        && start[0] < maxs[0]
        && start[1] > mins[1]
        && start[1] < maxs[1]
        && start[2] > mins[2]
        && start[2] < maxs[2]
    {
        return None;
    }
    let mut enter = i32::MIN / 2;
    let mut exit = 4096;
    let mut hit_axis = 3u8;
    let mut axis = 0usize;
    while axis < 3 {
        let delta = end[axis] - start[axis];
        if delta == 0 {
            if start[axis] < mins[axis] || start[axis] > maxs[axis] {
                return None;
            }
            axis += 1;
            continue;
        }
        // A point exactly on one expanded face is touching, not embedded.
        // Motion away from that face immediately separates the two hulls and
        // must not be reported as a zero-fraction hit. Without this check the
        // c1a0c route freezes when the player strafes away from console_guy at
        // the shared Z=72 boundary (and the same trap affects tram NPCs).
        if (start[axis] == mins[axis] && delta < 0) || (start[axis] == maxs[axis] && delta > 0) {
            return None;
        }

        let (entry_distance, exit_distance, speed) = if delta > 0 {
            (mins[axis] - start[axis], maxs[axis] - start[axis], delta)
        } else {
            (start[axis] - maxs[axis], start[axis] - mins[axis], -delta)
        };
        // 1/32 world unit in Q12 is 128. Nearby actor coordinates are i16,
        // so these products remain comfortably inside i32 on the PS1.
        let axis_enter = (entry_distance * 4096 - 128) / speed;
        let axis_exit = (exit_distance * 4096) / speed;
        if axis_enter > enter {
            enter = axis_enter;
            hit_axis = axis as u8;
        }
        if axis_exit < exit {
            exit = axis_exit;
        }
        if enter > exit {
            return None;
        }
        axis += 1;
    }

    if hit_axis > 2 || exit < 0 || enter > 4096 {
        return None;
    }
    Some(ActorSweep {
        frac: enter.max(0),
        axis: hit_axis,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        actor_candidate_overlaps_player, actor_floor_step_reachable, advance_duck_activation,
        crouch_origin_shift, ground_contact_q6, integrate_planar_q6, melee_vertical_hulls_overlap,
        is_diagonal_contact_plane, player_inside_actor, settle_vertical, sweep_player_actor,
        ActorSweep,
    };

    #[test]
    fn grounded_hull_switch_keeps_the_players_feet_on_the_floor() {
        assert_eq!(crouch_origin_shift(true, true), -18);
        assert_eq!(crouch_origin_shift(true, false), 18);
    }

    #[test]
    fn airborne_hull_switches_do_not_move_the_origin() {
        assert_eq!(crouch_origin_shift(false, true), 0);
        assert_eq!(crouch_origin_shift(false, false), 0);
    }

    #[test]
    fn grounded_duck_waits_eight_ticks_before_switching_hulls() {
        let (mut timer, mut activate) = advance_duck_activation(false, true, true, 0);
        assert_eq!(timer, 8);
        assert!(!activate);
        for expected in (1..8).rev() {
            (timer, activate) = advance_duck_activation(false, true, true, timer);
            assert_eq!(timer, expected);
            assert!(!activate);
        }
        (timer, activate) = advance_duck_activation(false, true, true, timer);
        assert_eq!(timer, 0);
        assert!(activate);
    }

    #[test]
    fn duck_jump_switches_on_the_first_airborne_tick() {
        let (timer, activate) = advance_duck_activation(false, true, true, 0);
        assert_eq!((timer, activate), (8, false));
        assert_eq!(
            advance_duck_activation(false, true, false, timer),
            (0, true)
        );
    }

    #[test]
    fn releasing_duck_cancels_a_pending_transition() {
        assert_eq!(advance_duck_activation(false, false, true, 5), (0, false));
    }

    #[test]
    fn actor_floor_step_accepts_goldsrc_limit_and_descents() {
        assert!(actor_floor_step_reachable(-144, -126));
        assert!(actor_floor_step_reachable(-144, -145));
        assert!(actor_floor_step_reachable(-144, -304));
    }

    #[test]
    fn actor_floor_step_rejects_nineteen_unit_climb() {
        assert!(!actor_floor_step_reachable(-144, -125));
    }

    #[test]
    fn melee_requires_positive_vertical_hull_overlap() {
        assert!(melee_vertical_hulls_overlap(-72, -48, -108, -71));
        assert!(!melee_vertical_hulls_overlap(-72, -48, -144, -72));
        assert!(!melee_vertical_hulls_overlap(82, 106, -143, -71));
    }

    #[test]
    fn approaching_actor_cannot_cross_the_player_hull() {
        let actor_mins = [-2084, -288, -13];
        let actor_maxs = [-2051, -216, 19];
        assert!(actor_candidate_overlaps_player(
            actor_mins,
            actor_maxs,
            [-2100, -252, 15],
            36,
        ));
        assert!(!actor_candidate_overlaps_player(
            actor_mins,
            actor_maxs,
            [-2116, -252, 52],
            36,
        ));
    }

    #[test]
    fn grounded_step_does_not_launch_the_next_frame() {
        assert_eq!(settle_vertical(8, false), (0, 0));
    }

    #[test]
    fn airborne_downward_speed_becomes_landing_impact() {
        assert_eq!(settle_vertical(-13, true), (0, 13));
    }

    #[test]
    fn supported_downward_drift_is_consumed_without_false_impact() {
        assert_eq!(settle_vertical(-2, false), (0, 0));
    }

    #[test]
    fn q6_displacement_preserves_full_semantic_axis_distance() {
        // 127/128 of 16 units/tick = 15.875 = 1016/64.  Eight GoldSrc
        // frames therefore travel exactly 127 units, not 128.
        let mut carry = 0;
        let mut distance = 0;
        for _ in 0..8 {
            let (step, next) = integrate_planar_q6(1016, carry);
            distance += step;
            carry = next;
        }
        assert_eq!(distance, 127);
        assert_eq!(carry, 0);
    }

    #[test]
    fn q6_displacement_is_sign_symmetric() {
        let mut carry = 0;
        let mut distance = 0;
        for _ in 0..8 {
            let (step, next) = integrate_planar_q6(-1016, carry);
            distance += step;
            carry = next;
        }
        assert_eq!(distance, -127);
        assert_eq!(carry, 0);
    }

    #[test]
    fn stopped_velocity_does_not_spend_position_residue() {
        assert_eq!(integrate_planar_q6(0, 31), (0, 31));
        assert_eq!(integrate_planar_q6(0, 32), (0, 32));
        assert_eq!(integrate_planar_q6(0, -32), (0, -32));
    }

    #[test]
    fn ground_contact_keeps_fractional_slope_height() {
        // Eight-unit probe, 1479/4096 along it: -2.8887 units. The integer
        // collision origin stays above the slope while -57/64 records the
        // actual -132.890625 contact.
        assert_eq!(ground_contact_q6(-130, -138, 1479), (-132, -57));
        assert_eq!(ground_contact_q6(138, 130, 1479), (136, -57));
    }

    #[test]
    fn ground_contact_exact_integer_has_no_residue() {
        assert_eq!(ground_contact_q6(-130, -138, 1024), (-132, 0));
    }

    #[test]
    fn nearest_contact_retry_is_limited_to_multi_axis_planes() {
        assert!(is_diagonal_contact_plane([2697, 0, 3083]));
        assert!(is_diagonal_contact_plane([1024, 2048, 3072]));
        assert!(!is_diagonal_contact_plane([4096, 0, 0]));
        assert!(!is_diagonal_contact_plane([0, -4096, 0]));
    }

    #[test]
    fn console_scientist_stops_the_c1a0c_route_at_goldsrc_boundary() {
        // Runtime axes for Gold's console_guy: feet at (165,-144,40), human
        // hull +/-16 XZ and 72 high. The standing player moves toward -Z.
        let hit = sweep_player_actor(
            [154, -108, 84],
            [154, -108, 68],
            [149, -144, 24],
            [181, -72, 56],
            36,
        )
        .unwrap();
        assert_eq!(
            hit,
            ActorSweep {
                frac: 3064,
                axis: 2,
            }
        );
        let stopped_z = 84 + (((68 - 84) * hit.frac) >> 12);
        assert_eq!(stopped_z, 72);
    }

    #[test]
    fn parallel_move_outside_actor_does_not_hit() {
        assert_eq!(
            sweep_player_actor(
                [210, -108, 84],
                [210, -108, 68],
                [149, -144, 24],
                [181, -72, 56],
                36,
            ),
            None
        );
    }

    #[test]
    fn starting_inside_actor_can_escape() {
        assert!(player_inside_actor(
            [165, -108, 40],
            [149, -144, 24],
            [181, -72, 56],
            36
        ));
        assert_eq!(
            sweep_player_actor(
                [165, -108, 40],
                [165, -108, 84],
                [149, -144, 24],
                [181, -72, 56],
                36,
            ),
            None
        );
    }

    #[test]
    fn vertical_separation_rejects_horizontal_overlap() {
        assert_eq!(
            sweep_player_actor(
                [154, 40, 84],
                [154, 40, 68],
                [149, -144, 24],
                [181, -72, 56],
                36,
            ),
            None
        );
    }

    #[test]
    fn inward_move_from_surface_blocks_at_fraction_zero() {
        assert_eq!(
            sweep_player_actor(
                [154, -108, 72],
                [154, -108, 64],
                [149, -144, 24],
                [181, -72, 56],
                36,
            ),
            Some(ActorSweep { frac: 0, axis: 2 })
        );
    }

    #[test]
    fn outward_move_from_surface_is_not_blocked() {
        assert_eq!(
            sweep_player_actor(
                [154, -108, 72],
                [154, -108, 88],
                [149, -144, 24],
                [181, -72, 56],
                36,
            ),
            None
        );
        assert_eq!(
            sweep_player_actor(
                [154, -108, 8],
                [154, -108, -8],
                [149, -144, 24],
                [181, -72, 56],
                36,
            ),
            None
        );
    }
}
