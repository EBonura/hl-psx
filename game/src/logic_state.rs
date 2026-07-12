//! Allocation-free packing/state helpers for the runtime target graph.
//!
//! This module deliberately has no PSX dependencies, so its exact transition
//! semantics can be exercised with a host `rustc --test` build.

pub const CALLER_NONE: u16 = u16::MAX;

/// `RoomLaunch::carry_count` doubles as the cross-map tram motion byte. Actor
/// carry uses at most fifteen mailbox rows, leaving the high bit free without
/// growing the launch/change-request structs.
pub const CARRY_COUNT_MASK: u8 = 0x0f;
pub const CARRY_TRAM_ACTIVE_FLAG: u8 = 0x80;

#[inline(always)]
pub const fn carry_record_count(encoded: u8) -> usize {
    (encoded & CARRY_COUNT_MASK) as usize
}

#[inline(always)]
pub const fn carry_tram_active(encoded: u8) -> bool {
    encoded & CARRY_TRAM_ACTIVE_FLAG != 0
}

#[inline(always)]
pub const fn encode_carry_state(count: u8, tram_active: bool) -> u8 {
    (count & CARRY_COUNT_MASK)
        | if tram_active {
            CARRY_TRAM_ACTIVE_FLAG
        } else {
            0
        }
}

/// Cross-map target identity. Logic name ids are local to one cooked map, so
/// trigger_changelevel's `changetarget` travels as a stable, case-insensitive
/// hash and is resolved back to a destination-local id after the new map loads.
#[inline]
pub fn logic_name_hash32(name: &str) -> u32 {
    let name = name.trim();
    if name.is_empty() {
        return 0;
    }
    let mut hash = 0x811c_9dc5u32;
    for byte in name.bytes() {
        hash = (hash ^ byte.to_ascii_lowercase() as u32).wrapping_mul(0x0100_0193);
    }
    // Zero is the payload sentinel. FNV-1a can theoretically produce it, so
    // reserve that value without reducing ordinary hash width.
    if hash == 0 {
        1
    } else {
        hash
    }
}

/// RoomLaunch has no padding left, but its tracktrain seat only needs signed
/// 16-bit coordinates. Store the destination post-target hash and delay in the
/// otherwise-unused upper halves of those three existing words.
#[inline(always)]
pub const fn pack_changelevel_payload(
    seat: [i32; 3],
    target_hash: u32,
    delay_ticks: u16,
) -> [i32; 3] {
    [
        (((target_hash & 0xffff) << 16) | seat[0] as u16 as u32) as i32,
        ((target_hash & 0xffff_0000) | seat[1] as u16 as u32) as i32,
        (((delay_ticks as u32) << 16) | seat[2] as u16 as u32) as i32,
    ]
}

#[inline(always)]
pub const fn unpack_ride_seat(payload: [i32; 3]) -> [i32; 3] {
    [
        payload[0] as u16 as i16 as i32,
        payload[1] as u16 as i16 as i32,
        payload[2] as u16 as i16 as i32,
    ]
}

#[inline(always)]
pub const fn unpack_changelevel_payload(payload: [i32; 3]) -> (u32, u16) {
    let lo = (payload[0] as u32) >> 16;
    let hi = (payload[1] as u32) & 0xffff_0000;
    let delay = ((payload[2] as u32) >> 16) as u16;
    (hi | lo, delay)
}

/// Global func_train transition payload: campaign globals peak at speed 600
/// and have at most a 60-tick corner wait, so both values fit one existing
/// mailbox word (10-bit speed, 6-bit remaining wait) without resident state.
#[inline(always)]
pub const fn pack_train_speed_wait(speed: u16, wait_ticks: u16) -> u16 {
    let speed = if speed > 0x03ff { 0x03ff } else { speed };
    let wait_ticks = if wait_ticks > 0x003f {
        0x003f
    } else {
        wait_ticks
    };
    speed | (wait_ticks << 10)
}

#[inline(always)]
pub const fn unpack_train_speed_wait(payload: u16) -> (u16, u16) {
    (payload & 0x03ff, payload >> 10)
}

const EVENT_ACTIVE: u16 = 1 << 15;
const EVENT_USE_MASK: u16 = 0x3;
const EVENT_CALLER_SHIFT: u16 = 2;
const EVENT_CALLER_MASK: u16 = 0x1ff << EVENT_CALLER_SHIFT;

#[inline(always)]
pub const fn event_meta(active: bool, use_type: u8, caller: u16) -> u16 {
    // 0 means no logic caller; encoded logic indices are biased by one.
    // MAX_LOGIC=384, comfortably inside the packed nine-bit field.
    let caller_code = if caller == CALLER_NONE || caller >= 511 {
        0
    } else {
        caller + 1
    };
    (if active { EVENT_ACTIVE } else { 0 })
        | (use_type as u16 & EVENT_USE_MASK)
        | ((caller_code << EVENT_CALLER_SHIFT) & EVENT_CALLER_MASK)
}

#[inline(always)]
pub const fn event_active(meta: u16) -> bool {
    meta & EVENT_ACTIVE != 0
}

#[inline(always)]
pub const fn event_use_type(meta: u16) -> u8 {
    (meta & EVENT_USE_MASK) as u8
}

#[inline(always)]
pub const fn event_caller(meta: u16) -> u16 {
    let code = (meta & EVENT_CALLER_MASK) >> EVENT_CALLER_SHIFT;
    if code == 0 {
        CALLER_NONE
    } else {
        code - 1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultisourceToggle {
    pub bits: u32,
    pub accepted: bool,
    pub complete: bool,
    pub became_complete: bool,
}

#[inline(always)]
pub const fn multisource_complete(
    bits: u32,
    member_count: u8,
    global_required: bool,
    global_on: bool,
) -> bool {
    let count = if member_count > 32 { 32 } else { member_count };
    let mask = if count == 32 {
        u32::MAX
    } else if count == 0 {
        0
    } else {
        (1u32 << count) - 1
    };
    bits & mask == mask && (!global_required || global_on)
}

#[inline(always)]
pub const fn multisource_toggle(
    bits: u32,
    member_count: u8,
    member_index: u8,
    global_required: bool,
    global_on: bool,
) -> MultisourceToggle {
    let was_complete = multisource_complete(bits, member_count, global_required, global_on);
    if member_index >= member_count || member_index >= 32 {
        return MultisourceToggle {
            bits,
            accepted: false,
            complete: was_complete,
            became_complete: false,
        };
    }
    let next_bits = bits ^ (1u32 << member_index);
    let complete = multisource_complete(next_bits, member_count, global_required, global_on);
    MultisourceToggle {
        bits: next_bits,
        accepted: true,
        complete,
        became_complete: !was_complete && complete,
    }
}

// func_rotating reuses the runtime's existing door-state byte. Keeping the
// numeric values identical lets the game store the fan's complete state in
// LOGIC_STATE/LOGIC_NEXT/LOGIC_COUNTER plus the brush's existing ENT_PHASE;
// no fan-specific resident array is needed.
pub const ROTATING_STOPPED: u8 = 0;
pub const ROTATING_SPINNING_UP: u8 = 1;
pub const ROTATING_FULL_SPEED: u8 = 2;
pub const ROTATING_SPINNING_DOWN: u8 = 3;
pub const ROTATING_AUTO_START_WAIT: u8 = 4;
pub const ROTATING_THINK_TICKS: u16 = 2; // CFuncRotating thinks every 0.1 s at 20 Hz
                                         // Stateful fans use the c1a2 route-critical solid-pusher calibration. GoldSrc
                                         // rolls a blocked pusher's local time back, so this intentionally differs from
                                         // the unblocked cosmetic epoch below.
pub const ROTATING_AUTO_START_TICKS: u16 = 31;
// The reference client begins PostThink checkpoints with pushers already at
// ltime=.25. Their SDK nextthink=1.5 callback consequently fires at map tick26.
pub const ROTATING_COSMETIC_AUTO_START_TICKS: u32 = 26;

/// Untargeted rotating brushes have no runtime state slot. Only START_ON fans
/// animate on this cosmetic path; an ordinary unnamed rotator must remain at
/// its authored rest angle until something can target it (which requires a
/// named LogicEnt and therefore takes the stateful path instead). START_ON is
/// not immediate: GoldSrc waits 1.5 seconds, then optionally follows the same
/// 10 Hz fanfriction ramp as a targeted fan.
#[inline(always)]
pub fn rotating_cosmetic_phase_q16(
    map_tick: u32,
    velocity_q16: i16,
    start_on: bool,
    accelerate_decelerate: bool,
    fanfriction_percent: u16,
) -> i32 {
    if !start_on || map_tick <= ROTATING_COSMETIC_AUTO_START_TICKS {
        return 0;
    }

    let elapsed = map_tick - ROTATING_COSMETIC_AUTO_START_TICKS;
    let velocity = velocity_q16 as i32;
    if !accelerate_decelerate {
        return velocity.wrapping_mul((elapsed & 0xffff) as i32) & 0xffff;
    }

    // MOVETYPE_PUSH consumes the outer frame that runs SpinUp, then moves for
    // two 50 ms frames before the next 0.1 s think. The unblocked sequence is:
    //   think(f), f, f, think(2f), 2f, 2f, ... think(100), 100, 100, 100...
    // Sum it without walking elapsed ticks; fan phase is queried from both the
    // collision and render paths and must remain cheap on PS1.
    let friction = fanfriction_percent.clamp(1, 100) as u32;
    let magnitude = velocity.unsigned_abs();
    let full_step = (100 + friction - 1) / friction;
    let ramp_steps = full_step - 1;
    let completed_steps = ((elapsed.saturating_sub(2)) / 3).min(ramp_steps);
    let a = magnitude * friction;
    let mut phase = 2 * floor_sum_positive_multiples(completed_steps, a, 100);
    let partial_step = completed_steps + 1;
    if partial_step <= ramp_steps && elapsed >= 3 * partial_step + 1 {
        phase = phase.wrapping_add(a.wrapping_mul(partial_step) / 100);
    }
    let full_samples = elapsed.saturating_sub(3 * full_step);
    phase = phase.wrapping_add((full_samples & 0xffff).wrapping_mul(magnitude));
    let phase = (phase & 0xffff) as i32;
    if velocity < 0 {
        (-phase) & 0xffff
    } else {
        phase
    }
}

/// Sum floor(a*k/m), k=1..=count, in logarithmic time. Bounds on the fan
/// inputs keep every intermediate in u32 (count <= 99, m == 100).
#[inline(always)]
fn floor_sum_positive_multiples(count: u32, a: u32, m: u32) -> u32 {
    let mut n = count;
    let mut modulus = m;
    let mut slope = a;
    let mut intercept = a;
    let mut answer = 0u32;
    loop {
        if slope >= modulus {
            answer = answer
                .wrapping_add((n - n.min(1)).wrapping_mul(n).wrapping_mul(slope / modulus) / 2);
            slope %= modulus;
        }
        if intercept >= modulus {
            answer = answer.wrapping_add(n.wrapping_mul(intercept / modulus));
            intercept %= modulus;
        }
        let y = slope.wrapping_mul(n).wrapping_add(intercept);
        if y < modulus || slope == 0 {
            return answer;
        }
        n = y / modulus;
        intercept = y % modulus;
        core::mem::swap(&mut modulus, &mut slope);
    }
}

/// Persistent rotating-brush state. `phase_q16` is one turn in 65,536 units;
/// `speed_percent` is GoldSrc's exact 0..100 acceleration fraction. The final
/// signed angular velocity remains immutable in the cooked Ent record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RotatingState {
    pub phase_q16: i32,
    pub speed_percent: i16,
    pub state: u8,
    pub next_tick: u16,
}

#[inline(always)]
fn tick_reached(now: u16, at: u16) -> bool {
    now.wrapping_sub(at) < 0x8000
}

#[inline(always)]
fn approach_i16(value: i16, target: i16, step: i16) -> i16 {
    let value = value as i32;
    let target = target as i32;
    let step = (step as i32).max(1);
    if value < target {
        (value + step).min(target) as i16
    } else if value > target {
        (value - step).max(target) as i16
    } else {
        target as i16
    }
}

/// CFuncRotating::RotatingUse deliberately ignores USE_ON/OFF: a non-zero
/// angular velocity means "spin down", while a stopped brush means "start".
#[inline]
pub fn rotating_use(
    mut current: RotatingState,
    accelerate_decelerate: bool,
    now: u16,
) -> RotatingState {
    if current.speed_percent != 0 {
        current.state = ROTATING_SPINNING_DOWN;
        current.next_tick = now.wrapping_add(ROTATING_THINK_TICKS);
    } else if accelerate_decelerate {
        current.state = ROTATING_SPINNING_UP;
        current.next_tick = now.wrapping_add(ROTATING_THINK_TICKS);
    } else {
        current.speed_percent = 100;
        current.state = ROTATING_FULL_SPEED;
        current.next_tick = 0;
    }
    current
}

/// Advance one fixed 20 Hz sample, including GoldSrc's two-tick friction
/// cadence. Phase is retained when the fan reaches rest rather than reverting
/// to a map-tick-derived angle.
#[inline]
pub fn rotating_tick(
    mut current: RotatingState,
    max_velocity_q16: i16,
    fanfriction_percent: u16,
    accelerate_decelerate: bool,
    now: u16,
) -> RotatingState {
    if current.state == ROTATING_AUTO_START_WAIT {
        if tick_reached(now, current.next_tick) {
            return rotating_use(current, accelerate_decelerate, now);
        }
        return current;
    }
    if current.state == ROTATING_STOPPED {
        current.speed_percent = 0;
        return current;
    }

    current.phase_q16 =
        (current.phase_q16 + max_velocity_q16 as i32 * current.speed_percent as i32 / 100) & 0xffff;
    if current.state == ROTATING_FULL_SPEED {
        current.speed_percent = 100;
        return current;
    }
    if !tick_reached(now, current.next_tick) {
        return current;
    }

    if current.state == ROTATING_SPINNING_UP {
        current.speed_percent = if accelerate_decelerate {
            approach_i16(
                current.speed_percent,
                100,
                fanfriction_percent.max(1).min(i16::MAX as u16) as i16,
            )
        } else {
            100
        };
        if current.speed_percent == 100 {
            current.state = ROTATING_FULL_SPEED;
            current.next_tick = 0;
        } else {
            current.next_tick = now.wrapping_add(ROTATING_THINK_TICKS);
        }
    } else if current.state == ROTATING_SPINNING_DOWN {
        current.speed_percent = if accelerate_decelerate {
            approach_i16(
                current.speed_percent,
                0,
                fanfriction_percent.max(1).min(i16::MAX as u16) as i16,
            )
        } else {
            0
        };
        if current.speed_percent == 0 {
            current.state = ROTATING_STOPPED;
            current.next_tick = 0;
        } else {
            current.next_tick = now.wrapping_add(ROTATING_THINK_TICKS);
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carry_count_and_tram_motion_share_one_byte_without_aliasing() {
        for count in 0..=CARRY_COUNT_MASK {
            let stopped = encode_carry_state(count, false);
            assert_eq!(carry_record_count(stopped), count as usize);
            assert!(!carry_tram_active(stopped));

            let moving = encode_carry_state(count, true);
            assert_eq!(carry_record_count(moving), count as usize);
            assert!(carry_tram_active(moving));
            assert_eq!(moving ^ stopped, CARRY_TRAM_ACTIVE_FLAG);
        }
    }

    #[test]
    fn legacy_unflagged_carry_counts_remain_exact_and_stopped() {
        for old_count in 0..=15 {
            assert_eq!(carry_record_count(old_count), old_count as usize);
            assert!(!carry_tram_active(old_count));
        }
        // Reserved middle bits must never inflate a mailbox loop.
        assert_eq!(carry_record_count(0x7f), CARRY_COUNT_MASK as usize);
    }

    #[test]
    fn event_metadata_round_trips_every_runtime_edge() {
        for caller in [CALLER_NONE, 0, 1, 383] {
            for use_type in 0..=3 {
                let meta = event_meta(true, use_type, caller);
                assert!(event_active(meta));
                assert_eq!(event_use_type(meta), use_type);
                assert_eq!(event_caller(meta), caller);
            }
        }
        assert!(!event_active(event_meta(false, 3, 17)));
    }

    #[test]
    fn exact_members_toggle_and_only_completion_edges_fire() {
        let first = multisource_toggle(0, 2, 0, false, false);
        assert_eq!(first.bits, 1);
        assert!(!first.complete);
        assert!(!first.became_complete);

        let second = multisource_toggle(first.bits, 2, 1, false, false);
        assert_eq!(second.bits, 3);
        assert!(second.complete);
        assert!(second.became_complete);

        let off = multisource_toggle(second.bits, 2, 0, false, false);
        assert_eq!(off.bits, 2);
        assert!(!off.complete);
        assert!(!off.became_complete);

        let on_again = multisource_toggle(off.bits, 2, 0, false, false);
        assert!(on_again.complete);
        assert!(on_again.became_complete);
    }

    #[test]
    fn unknown_callers_are_ignored_and_global_is_an_and_gate() {
        let unknown = multisource_toggle(0, 2, 2, false, false);
        assert!(!unknown.accepted);
        assert_eq!(unknown.bits, 0);

        let all_inputs_global_off = multisource_toggle(1, 2, 1, true, false);
        assert_eq!(all_inputs_global_off.bits, 3);
        assert!(!all_inputs_global_off.complete);
        assert!(!all_inputs_global_off.became_complete);
        assert!(multisource_complete(3, 2, true, true));
    }

    #[test]
    fn zero_member_source_is_vacuously_complete() {
        assert!(multisource_complete(0, 0, false, false));
        assert!(!multisource_complete(0, 0, true, false));
        assert!(multisource_complete(0, 0, true, true));
    }

    #[test]
    fn changelevel_payload_round_trips_target_delay_and_signed_train_seat() {
        let seat = [-120, 70, -1];
        let hash = logic_name_hash32("WottaDrag");
        let packed = pack_changelevel_payload(seat, hash, 240);
        assert_eq!(unpack_ride_seat(packed), seat);
        assert_eq!(unpack_changelevel_payload(packed), (hash, 240));
    }

    #[test]
    fn cross_map_name_hash_is_case_insensitive_and_reserves_zero() {
        assert_eq!(
            logic_name_hash32("  EleStartMM "),
            logic_name_hash32("elestartmm")
        );
        assert_ne!(logic_name_hash32("elestartmm"), 0);
        assert_eq!(logic_name_hash32("   "), 0);
    }

    #[test]
    fn global_train_speed_and_wait_share_one_mailbox_word() {
        for (speed, wait) in [(32, 0), (150, 1), (600, 60), (1023, 63)] {
            assert_eq!(
                unpack_train_speed_wait(pack_train_speed_wait(speed, wait)),
                (speed, wait)
            );
        }
        assert_eq!(
            unpack_train_speed_wait(pack_train_speed_wait(2000, 100)),
            (1023, 63)
        );
    }

    #[test]
    fn c1a2_fan_auto_starts_then_spins_down_without_losing_phase() {
        // speed=400, reverse, at 20 Hz in Q16-turn units; fanfriction=2%.
        let max_velocity = 3641;
        let mut fan = RotatingState {
            phase_q16: 0,
            speed_percent: 0,
            state: ROTATING_AUTO_START_WAIT,
            next_tick: 31,
        };

        for now in 0..=140 {
            fan = rotating_tick(fan, max_velocity, 2, true, now);
            let expected = match now {
                20 => Some(0),
                40 => Some(1161),
                60 => Some(14259),
                80 => Some(41921),
                100 => Some(18611),
                120 => Some(9864),
                140 => Some(14521),
                _ => None,
            };
            if let Some(phase) = expected {
                assert_eq!(fan.phase_q16, phase, "reference checkpoint tick {now}");
            }
        }
        assert_eq!(fan.state, ROTATING_FULL_SPEED);
        assert_eq!(fan.speed_percent, 100);

        for now in 141..203 {
            fan = rotating_tick(fan, max_velocity, 2, true, now);
        }
        fan = rotating_use(fan, true, 203);
        assert_eq!(fan.state, ROTATING_SPINNING_DOWN);
        for now in 203..=320 {
            fan = rotating_tick(fan, max_velocity, 2, true, now);
        }
        assert_eq!(fan.state, ROTATING_STOPPED);
        assert_eq!(fan.speed_percent, 0);
        let stopped_phase = fan.phase_q16;
        let reference_phase = 36_409; // reflected -3439.998 degrees, modulo one turn
        let phase_error = (stopped_phase - reference_phase)
            .abs()
            .min(65_536 - (stopped_phase - reference_phase).abs());
        assert!(
            phase_error <= 90,
            "stopped phase drifts by more than 0.5 degree"
        );
        for now in 321..340 {
            fan = rotating_tick(fan, max_velocity, 2, true, now);
        }
        assert_eq!(
            fan.phase_q16, stopped_phase,
            "rest keeps the physical blade angle"
        );
    }

    #[test]
    fn ordinary_rotator_starts_immediately_and_stops_on_next_think() {
        let mut fan = RotatingState {
            phase_q16: 123,
            speed_percent: 0,
            state: ROTATING_STOPPED,
            next_tick: 0,
        };
        fan = rotating_use(fan, false, 10);
        assert_eq!((fan.state, fan.speed_percent), (ROTATING_FULL_SPEED, 100));
        fan = rotating_use(fan, false, 11);
        assert_eq!(fan.state, ROTATING_SPINNING_DOWN);
        fan = rotating_tick(fan, 512, 100, false, 12);
        assert_ne!(fan.speed_percent, 0);
        fan = rotating_tick(fan, 512, 100, false, 13);
        assert_eq!((fan.state, fan.speed_percent), (ROTATING_STOPPED, 0));
    }

    #[test]
    fn untargeted_cosmetic_rotator_obeys_start_on() {
        assert_eq!(rotating_cosmetic_phase_q16(80, 512, false, false, 100), 0);
        assert_eq!(
            rotating_cosmetic_phase_q16(ROTATING_COSMETIC_AUTO_START_TICKS, 512, true, false, 100,),
            0
        );
        assert_eq!(
            rotating_cosmetic_phase_q16(
                ROTATING_COSMETIC_AUTO_START_TICKS + 1,
                512,
                true,
                false,
                100,
            ),
            512
        );
    }

    #[test]
    fn cosmetic_closed_form_matches_unblocked_goldsrc_pusher_ramp_exactly() {
        for velocity in [3641i16, -1820, 512, -700] {
            for friction in [1u16, 2, 45, 100, 150] {
                let mut phase = 0i32;
                let mut percent = 0i32;
                let mut full_speed = false;
                for now in 0..=400u32 {
                    if now > ROTATING_COSMETIC_AUTO_START_TICKS {
                        let elapsed = now - ROTATING_COSMETIC_AUTO_START_TICKS;
                        if !full_speed && elapsed >= 3 && elapsed % 3 == 0 {
                            percent = (percent + friction.clamp(1, 100) as i32).min(100);
                            full_speed = percent == 100;
                        } else if percent != 0 {
                            phase = (phase + velocity as i32 * percent / 100) & 0xffff;
                        }
                    }
                    assert_eq!(
                        rotating_cosmetic_phase_q16(now, velocity, true, true, friction,),
                        phase,
                        "velocity={velocity} friction={friction} tick={now}",
                    );
                }
            }
        }
    }

    #[test]
    fn c1a1b_cosmetic_fans_match_reference_tick_40() {
        let q12 = |phase: i32| ((phase >> 4) as u16) & 0x0fff;
        assert_eq!(
            q12(rotating_cosmetic_phase_q16(40, 3641, true, false, 45)),
            3185,
        );
        assert_eq!(
            q12(rotating_cosmetic_phase_q16(40, -3641, true, false, 45)),
            910,
        );
        assert_eq!(
            q12(rotating_cosmetic_phase_q16(40, -1820, true, true, 100)),
            2844,
        );
    }
}
