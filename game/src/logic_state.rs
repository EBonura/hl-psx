//! Allocation-free packing/state helpers for the runtime target graph.
//!
//! This module deliberately has no PSX dependencies, so its exact transition
//! semantics can be exercised with a host `rustc --test` build.

pub const CALLER_NONE: u16 = u16::MAX;

/// Per-20-Hz phase advance for GoldSrc AngularMove. `travel_q12` is the
/// absolute authored rotation where 4096 is one turn; speed is degrees/sec.
#[inline(always)]
pub fn angular_move_phase_step(speed: u16, travel_q12: i32) -> i32 {
    let travel = if travel_q12 == i32::MIN {
        i32::MAX as i64 + 1
    } else {
        travel_q12.abs().max(1) as i64
    };
    let numerator = speed.max(1) as i64 * 4096 * 4096;
    let denominator = 20i64 * 360 * travel;
    ((numerator + denominator - 1) / denominator).clamp(1, 4096) as i32
}

/// Advance a momentary control's normalized 0..4096 position by one 20 Hz
/// host frame. GoldSrc sends this same normalized value to every wheel sharing
/// the target and to the target entity through `USE_SET`.
#[inline(always)]
pub fn momentary_advance_phase(phase: i32, speed: u16, travel_q12: i32) -> i32 {
    phase
        .saturating_add(angular_move_phase_step(speed, travel_q12))
        .min(4096)
}

/// Return an auto-return momentary control toward its authored start angle.
#[inline(always)]
pub fn momentary_return_phase(phase: i32, return_speed: u16, travel_q12: i32) -> i32 {
    phase
        .saturating_sub(angular_move_phase_step(return_speed, travel_q12))
        .max(0)
}

/// `RoomLaunch::carry_count` doubles as the cross-map player/tram state byte.
/// Actor carry uses at most fifteen mailbox rows, leaving flag bits free
/// without growing the launch/change-request structs.
pub const CARRY_COUNT_MASK: u8 = 0x0f;
pub const CARRY_LONGJUMP_FLAG: u8 = 0x20;
pub const CARRY_PLAYER_CROUCHED_FLAG: u8 = 0x40;
pub const CARRY_TRAM_ACTIVE_FLAG: u8 = 0x80;

/// GoldSrc `CBreakable::TakeDamage`: club attacks (the crowbar) do double
/// damage to ordinary breakables, while `SF_BREAK_CROWBAR` makes that strike
/// destroy the brush immediately. Trigger-only immunity is handled by the
/// caller before reaching this damage transition.
#[inline(always)]
pub const fn breakable_hp_after_damage(
    hp: u16,
    damage: u8,
    club: bool,
    crowbar_sensitive: bool,
) -> u16 {
    if hp == 0 || (club && crowbar_sensitive) {
        return 0;
    }
    let scale = if club { 2 } else { 1 };
    hp.saturating_sub((damage as u16) * scale)
}

/// GoldSrc material 7 is `matUnbreakableGlass`. CBreakable::TakeDamage rejects
/// every damage type before changing health; direct trigger use may still call
/// Die(), which is why this remains a damage-path predicate.
#[inline(always)]
pub const fn breakable_accepts_damage(material: u16) -> bool {
    material != 7
}

/// GoldSrc radius damage falls off linearly from full damage at the blast
/// origin to zero at the edge of the authored radius.
#[inline(always)]
pub const fn radius_damage_at_distance(damage: u8, radius: i32, distance: i32) -> u8 {
    if radius <= 0 || distance >= radius {
        0
    } else {
        let distance = if distance < 0 { 0 } else { distance };
        ((damage as i32 * (radius - distance)) / radius) as u8
    }
}

#[inline(always)]
pub const fn carry_record_count(encoded: u8) -> usize {
    (encoded & CARRY_COUNT_MASK) as usize
}

#[inline(always)]
pub const fn carry_tram_active(encoded: u8) -> bool {
    encoded & CARRY_TRAM_ACTIVE_FLAG != 0
}

#[inline(always)]
pub const fn carry_player_crouched(encoded: u8) -> bool {
    encoded & CARRY_PLAYER_CROUCHED_FLAG != 0
}

#[inline(always)]
pub const fn carry_longjump(encoded: u8) -> bool {
    encoded & CARRY_LONGJUMP_FLAG != 0
}

#[inline(always)]
pub const fn encode_carry_state(
    count: u8,
    tram_active: bool,
    player_crouched: bool,
    longjump: bool,
) -> u8 {
    (count & CARRY_COUNT_MASK)
        | if longjump { CARRY_LONGJUMP_FLAG } else { 0 }
        | if player_crouched {
            CARRY_PLAYER_CROUCHED_FLAG
        } else {
            0
        }
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

/// Store a signed landmark-relative coordinate and signed per-tick velocity in
/// one existing `RoomLaunch` word. Transition offsets are local to a landmark
/// and GoldSrc BSP coordinates are signed 16-bit, so neither half needs the
/// former full i32. Keeping this packed avoids growing the cross-map launch
/// record merely to preserve player momentum.
pub const fn pack_landmark_axis(offset: i32, velocity: i32) -> i32 {
    let off = if offset < i16::MIN as i32 {
        i16::MIN
    } else if offset > i16::MAX as i32 {
        i16::MAX
    } else {
        offset as i16
    };
    let vel = if velocity < i16::MIN as i32 {
        i16::MIN
    } else if velocity > i16::MAX as i32 {
        i16::MAX
    } else {
        velocity as i16
    };
    (((vel as u16 as u32) << 16) | off as u16 as u32) as i32
}

pub const fn unpack_landmark_axis(packed: i32) -> (i32, i32) {
    (
        packed as u16 as i16 as i32,
        ((packed as u32 >> 16) as u16 as i16) as i32,
    )
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

/// GoldSrc's CMultiManager clears its Use callback while a non-threaded run
/// is active. A self-target (c1a1's gen_lightsmm2) therefore fires once as an
/// output but cannot recursively restart the manager until its final target
/// has completed. Threaded managers clone instead and remain callable.
#[inline(always)]
pub const fn multi_manager_accepts_use(running: bool, threaded: bool) -> bool {
    threaded || !running
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

/// Map a one-based SpinUp callback number to the host tick on which GoldSrc
/// runs it, relative to the automatic-use tick.  The first four 0.1-second
/// deadlines consume three 20 Hz host frames; once float `ltime` reaches 1.9,
/// the same deadlines settle to two host frames.  This is shared by every
/// campaign fanfriction profile and is cheaper than a callback table.
#[inline(always)]
fn rotating_ramp_callback_tick(step: u32) -> u32 {
    if step <= 4 {
        step * 3
    } else {
        step * 2 + 4
    }
}

/// Number of SpinUp callbacks which have run by a relative host tick.
#[inline(always)]
fn rotating_ramp_callbacks_by(tick: u32) -> u32 {
    if tick < 3 {
        0
    } else if tick < 14 {
        tick / 3
    } else {
        (tick - 4) / 2
    }
}

/// Convert host 20 Hz samples at full speed into the samples GoldSrc actually
/// integrates for a `func_rotating` pusher.
///
/// `CFuncRotating::Rotate` schedules a no-op think every ten local seconds.
/// `SV_Physics_Pusher` shortens the frame that reaches `nextthink` and discards
/// the remainder. Because `ltime += 0.05f` is single precision, some ten-second
/// spans need 201 host frames rather than 200; the extra frame advances by only
/// a tiny residue and is one whole-speed sample behind an ideal map-tick clock.
///
/// The slow single-precision ranges reached during a normal map lifetime are
/// 8.1..55.9 and 248.1..1015.9 local seconds. A rotating brush reaches full
/// speed at 1.5..11.5 seconds, so the affected heartbeat indices collapse to
/// the two compact ranges below. This is allocation-free and avoids software
/// floating point in the collision/render hot path.
#[inline(never)]
fn rotating_goldsrc_full_speed_samples(host_samples: u32, start_tenths: u32) -> u32 {
    // Segment starts are start_tenths + 100*n. Intersecting them with the two
    // slow float ranges yields two runs of 201-host-frame heartbeats. Store the
    // first completion and run length arithmetically: no table and no loop.
    let (first_a, count_a, first_b, count_b) = if start_tenths >= 81 {
        (201, 5, 5006, 77)
    } else if start_tenths >= 60 {
        (401, 4, 5205, 76)
    } else {
        (401, 5, 5206, 77)
    };
    let count = |first: u32, cap: u32| {
        if host_samples < first {
            0
        } else {
            ((host_samples - first) / 201 + 1).min(cap)
        }
    };
    host_samples - count(first_a, count_a) - count(first_b, count_b)
}

/// Untargeted rotating brushes have no runtime state slot. Only START_ON fans
/// animate on this cosmetic path; an ordinary unnamed rotator must remain at
/// its authored rest angle until something can target it (which requires a
/// named LogicEnt and therefore takes the stateful path instead). START_ON is
/// not immediate: GoldSrc waits 1.5 seconds, then optionally follows the same
/// 10 Hz fanfriction ramp as a targeted fan.
#[inline(never)]
pub fn rotating_cosmetic_phase_q16(
    map_tick: u32,
    velocity_q16: i16,
    start_on: bool,
    accelerate_decelerate: bool,
    fanfriction_percent: u16,
    full_speed_extra_think: bool,
    blocked_after_first_sample: bool,
) -> i32 {
    if !start_on || map_tick <= ROTATING_COSMETIC_AUTO_START_TICKS {
        return 0;
    }

    let elapsed = map_tick - ROTATING_COSMETIC_AUTO_START_TICKS;
    let velocity = velocity_q16 as i32;
    if !accelerate_decelerate {
        // Co-pivot solid overlay brushes in c1a1c move for one host sample,
        // then block one another and retain that angle indefinitely.
        if blocked_after_first_sample {
            return velocity & 0xffff;
        }
        let samples = rotating_goldsrc_full_speed_samples(elapsed, 15);
        return velocity.wrapping_mul((samples & 0xffff) as i32) & 0xffff;
    }

    // MOVETYPE_PUSH consumes or shortens the frame that runs SpinUp, then
    // integrates two useful 50 ms samples before the next 0.1 s callback. The
    // callback host ticks are 3,6,9,12,14,16,... rather than a uniform three;
    // sum the completed velocity steps without walking them in the hot path.
    let friction = fanfriction_percent.clamp(1, 100) as u32;
    let magnitude = velocity.unsigned_abs();
    let full_step = (100 + friction - 1) / friction;
    let ramp_steps = full_step - 1;
    let completed_steps = rotating_ramp_callbacks_by(elapsed.saturating_sub(2)).min(ramp_steps);
    let a = magnitude * friction;
    let mut phase = 2 * floor_sum_positive_multiples(completed_steps, a, 100);
    let partial_step = completed_steps + 1;
    if partial_step <= ramp_steps {
        let samples = elapsed
            .saturating_sub(rotating_ramp_callback_tick(partial_step))
            .min(2);
        phase = phase.wrapping_add(samples.wrapping_mul(a.wrapping_mul(partial_step) / 100));
    }
    let full_host_samples = elapsed.saturating_sub(rotating_ramp_callback_tick(full_step));
    let full_samples = rotating_goldsrc_full_speed_samples(
        full_host_samples,
        15 + full_step + u32::from(full_speed_extra_think),
    );
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

// CPendulum is another MOVETYPE_PUSH user, but unlike func_rotating it thinks
// every 0.1 local second. Keep its angular phase and the GoldSrc float-heartbeat
// index in the brush's existing i32 ENT_PHASE word: low 20 bits are a signed
// Q19 turn (sub-millidegree precision), high 12 bits are the think index. The
// velocity remains Q19-turn units per 20 Hz sample in LOGIC_COUNTER. This keeps
// every swinging brush deterministic without a pendulum-specific RAM array.
pub const PENDULUM_STOPPED: u8 = 0;
pub const PENDULUM_SWINGING: u8 = 2;
pub const PENDULUM_START_WAIT: u8 = 4;
pub const PENDULUM_AUTO_START_TICKS: u16 = 20;
const PENDULUM_PHASE_BITS: u32 = 20;
const PENDULUM_PHASE_MASK: u32 = (1 << PENDULUM_PHASE_BITS) - 1;
const PENDULUM_THINK_MAX: u16 = (1 << (32 - PENDULUM_PHASE_BITS)) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendulumState {
    pub packed_phase: i32,
    pub velocity_q19: i16,
    pub state: u8,
    pub next_tick: u16,
}

#[inline(always)]
pub fn pendulum_phase_q19(packed: i32) -> i32 {
    (packed << (32 - PENDULUM_PHASE_BITS)) >> (32 - PENDULUM_PHASE_BITS)
}

#[inline(always)]
fn pendulum_think_index(packed: i32) -> u16 {
    ((packed as u32) >> PENDULUM_PHASE_BITS) as u16
}

#[inline(always)]
fn pack_pendulum_phase(phase_q19: i32, think_index: u16) -> i32 {
    (((think_index.min(PENDULUM_THINK_MAX) as u32) << PENDULUM_PHASE_BITS)
        | (phase_q19 as u32 & PENDULUM_PHASE_MASK)) as i32
}

/// Host frames consumed by the next `ltime += 0.1f` pendulum heartbeat.
///
/// Xash's pusher integrates 0.05f frames until it reaches the SDK nextthink.
/// Single-precision addition changes whether that takes two or three host
/// frames at stable boundaries. These five arithmetic runs reproduce the
/// original sequence for more than six minutes without a table or software
/// floating point (the complete Half-Life campaign never leaves this range in
/// a normal room visit).
#[inline(always)]
fn pendulum_heartbeat_ticks(think_index: u16) -> u16 {
    match think_index {
        0..=6 => 3,
        7..=147 => 2,
        148..=306 => 3,
        307..=2547 => 2,
        _ => 3,
    }
}

#[inline(always)]
fn pendulum_accelerate(
    velocity_q19: i16,
    phase_q19: i32,
    center_q19: i32,
    max_velocity_q19: i16,
    accel_per_host_tick_q19: i16,
    host_ticks: u16,
) -> i16 {
    let accel = (accel_per_host_tick_q19 as i32).max(1);
    let delta = accel * host_ticks as i32;
    // Q19 rounding makes the integrated phase lag GoldSrc by at most one
    // 0.1-second acceleration quantum at a centre crossing. Bias by that
    // bounded quantum so the reversal happens on the same heartbeat instead
    // of slipping an entire oscillation half-cycle.
    let toward_negative = phase_q19 >= center_q19 - accel * 2;
    let next = velocity_q19 as i32 + if toward_negative { -delta } else { delta };
    let limit = (max_velocity_q19 as i32).abs().max(1);
    next.clamp(-limit, limit) as i16
}

/// GoldSrc PendulumUse toggles a moving pendulum to a dead stop, retaining its
/// angle. A stopped one schedules Swing for the next 0.1 local second.
#[inline]
pub fn pendulum_use(mut current: PendulumState, now: u16) -> PendulumState {
    if current.state == PENDULUM_SWINGING {
        current.velocity_q19 = 0;
        current.state = PENDULUM_STOPPED;
        current.next_tick = 0;
    } else {
        let phase = pendulum_phase_q19(current.packed_phase);
        current.packed_phase = pack_pendulum_phase(phase, 0);
        current.velocity_q19 = 0;
        current.state = PENDULUM_START_WAIT;
        current.next_tick = now.wrapping_add(3);
    }
    current
}

/// Advance one host sample of CPendulum/SV_Physics_Pusher. Parameters are
/// cooked once into the brush record, so this hot path uses only integer adds,
/// compares and one clamp per 0.1-second think.
#[inline]
pub fn pendulum_tick(
    mut current: PendulumState,
    center_q19: i32,
    max_velocity_q19: i16,
    accel_per_host_tick_q19: i16,
    now: u16,
) -> PendulumState {
    if current.state == PENDULUM_START_WAIT {
        if tick_reached(now, current.next_tick) {
            let phase = pendulum_phase_q19(current.packed_phase);
            current.velocity_q19 = pendulum_accelerate(
                current.velocity_q19,
                phase,
                center_q19,
                max_velocity_q19,
                accel_per_host_tick_q19,
                3,
            );
            current.state = PENDULUM_SWINGING;
            current.next_tick = now.wrapping_add(pendulum_heartbeat_ticks(0));
        }
        return current;
    }
    if current.state != PENDULUM_SWINGING {
        current.velocity_q19 = 0;
        return current;
    }

    let index = pendulum_think_index(current.packed_phase);
    let interval = pendulum_heartbeat_ticks(index);
    let due = tick_reached(now, current.next_tick);
    let mut phase = pendulum_phase_q19(current.packed_phase);
    // In a two-frame heartbeat the second 0.05 step reaches nextthink and Swing
    // runs in that same host frame. In a three-frame heartbeat the third frame
    // advances only a float residue, so it contributes no quantized phase.
    if !due || interval == 2 {
        phase = phase.wrapping_add(current.velocity_q19 as i32);
    }
    if due {
        let next_index = index.saturating_add(1).min(PENDULUM_THINK_MAX);
        current.velocity_q19 = pendulum_accelerate(
            current.velocity_q19,
            phase,
            center_q19,
            max_velocity_q19,
            accel_per_host_tick_q19,
            interval,
        );
        current.next_tick = now.wrapping_add(pendulum_heartbeat_ticks(next_index));
        current.packed_phase = pack_pendulum_phase(phase, next_index);
    } else {
        current.packed_phase = pack_pendulum_phase(phase, index);
    }
    current
}

/// A played scripted_sentence fires its output once, then cannot be reused
/// until duration + refire have elapsed. SF_SENTENCE_ONCE never rearms.
pub const fn sentence_rearm(now: u16, cooldown: u16, once: bool) -> Option<u16> {
    if once {
        None
    } else {
        Some(now.wrapping_add(if cooldown == 0 { 1 } else { cooldown }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_sentence_outputs_wait_before_rearming() {
        assert_eq!(sentence_rearm(222, 120, false), Some(342));
        assert_eq!(sentence_rearm(222, 120, true), None);
        assert_eq!(sentence_rearm(u16::MAX, 3, false), Some(2));
        assert_eq!(sentence_rearm(222, 0, false), Some(223));
    }

    #[test]
    fn angular_move_uses_the_authored_swing_not_a_nominal_angle() {
        // c1a0e motor_button_cover: 160 degrees at 120 degrees/sec.
        let travel = (160 * 4096 + 180) / 360;
        let step = angular_move_phase_step(120, travel);
        assert_eq!(step, 154);
        assert_eq!((4096 + step - 1) / step, 27);
        // A 90-degree door at the same speed must finish sooner.
        assert!(angular_move_phase_step(120, 1024) > step);
    }

    #[test]
    fn hazard_valve_uses_one_normalized_phase_for_turn_and_return() {
        // t0a0a's paired wheels: distance=115, speed=35, returnspeed=5.
        let travel = (115 * 4096 + 180) / 360;
        let turned = momentary_advance_phase(0, 35, travel);
        assert_eq!(turned, 63);
        assert_eq!(momentary_advance_phase(4090, 35, travel), 4096);
        assert_eq!(momentary_return_phase(turned, 5, travel), 54);
        assert_eq!(momentary_return_phase(4, 5, travel), 0);
    }

    #[test]
    fn breakables_take_goldsrc_club_damage() {
        // c1a1's observation panes have 15 HP: a 10-damage bullet leaves five,
        // while the same base crowbar damage is doubled and shatters the pane.
        assert_eq!(breakable_hp_after_damage(15, 10, false, false), 5);
        assert_eq!(breakable_hp_after_damage(15, 10, true, false), 0);
        assert_eq!(breakable_hp_after_damage(500, 10, true, false), 480);
        assert_eq!(breakable_hp_after_damage(500, 10, true, true), 0);
        assert_eq!(breakable_hp_after_damage(500, 10, false, true), 490);
        assert!(breakable_accepts_damage(0));
        assert!(breakable_accepts_damage(6));
        assert!(!breakable_accepts_damage(7));
    }

    #[test]
    fn radius_damage_uses_goldsrc_linear_falloff() {
        assert_eq!(radius_damage_at_distance(100, 250, 0), 100);
        assert_eq!(radius_damage_at_distance(100, 250, 125), 50);
        assert_eq!(radius_damage_at_distance(100, 250, 249), 0);
        assert_eq!(radius_damage_at_distance(100, 250, 250), 0);
    }

    #[test]
    fn carry_count_tram_motion_and_player_crouch_share_one_byte_without_aliasing() {
        for count in 0..=CARRY_COUNT_MASK {
            for tram_active in [false, true] {
                for player_crouched in [false, true] {
                    for longjump in [false, true] {
                        let encoded =
                            encode_carry_state(count, tram_active, player_crouched, longjump);
                        assert_eq!(carry_record_count(encoded), count as usize);
                        assert_eq!(carry_tram_active(encoded), tram_active);
                        assert_eq!(carry_player_crouched(encoded), player_crouched);
                        assert_eq!(carry_longjump(encoded), longjump);
                    }
                }
            }
        }
    }

    #[test]
    fn legacy_unflagged_carry_counts_remain_exact_and_stopped() {
        for old_count in 0..=15 {
            assert_eq!(carry_record_count(old_count), old_count as usize);
            assert!(!carry_tram_active(old_count));
            assert!(!carry_player_crouched(old_count));
            assert!(!carry_longjump(old_count));
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
    fn non_threaded_multi_manager_ignores_self_use_until_completion() {
        assert!(multi_manager_accepts_use(false, false));
        assert!(!multi_manager_accepts_use(true, false));
        assert!(multi_manager_accepts_use(true, true));
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
    fn landmark_axis_round_trips_signed_offset_and_velocity() {
        for (offset, velocity) in [(0, 0), (-143, -16), (32767, -32768), (-32768, 32767)] {
            assert_eq!(
                unpack_landmark_axis(pack_landmark_axis(offset, velocity)),
                (offset, velocity)
            );
        }
        assert_eq!(
            unpack_landmark_axis(pack_landmark_axis(40_000, -40_000)),
            (32767, -32768)
        );
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
        assert_eq!(
            rotating_cosmetic_phase_q16(80, 512, false, false, 100, false, false),
            0
        );
        assert_eq!(
            rotating_cosmetic_phase_q16(
                ROTATING_COSMETIC_AUTO_START_TICKS,
                512,
                true,
                false,
                100,
                false,
                false,
            ),
            0
        );
        assert_eq!(
            rotating_cosmetic_phase_q16(
                ROTATING_COSMETIC_AUTO_START_TICKS + 1,
                512,
                true,
                false,
                100,
                false,
                false,
            ),
            512
        );
    }

    #[test]
    fn cosmetic_closed_form_stays_in_wrapped_phase_range_for_all_profiles() {
        for velocity in [3641i16, -1820, 512, -700] {
            for friction in [1u16, 2, 45, 100, 150] {
                for now in 0..=400u32 {
                    let phase = rotating_cosmetic_phase_q16(
                        now, velocity, true, true, friction, false, false,
                    );
                    assert!((0..=0xffff).contains(&phase));
                }
            }
        }
    }

    #[test]
    fn goldsrc_rotator_heartbeat_reproduces_float_ltime_holes() {
        // Full speed at ltime=1.5: 1.5->11.5 takes 200 host frames, while
        // 11.5->21.5 and the next four spans each take 201. The extra frame
        // carries only the float residue, so it must not add a whole sample.
        assert_eq!(rotating_goldsrc_full_speed_samples(400, 15), 400);
        assert_eq!(rotating_goldsrc_full_speed_samples(401, 15), 400);
        assert_eq!(rotating_goldsrc_full_speed_samples(402, 15), 401);
        assert_eq!(rotating_goldsrc_full_speed_samples(601, 15), 600);
        assert_eq!(rotating_goldsrc_full_speed_samples(602, 15), 600);

        // A very slow fan can reach full speed at ltime=11.5, placing its
        // first ten-second heartbeat directly in a 201-frame float range.
        assert_eq!(rotating_goldsrc_full_speed_samples(200, 115), 200);
        assert_eq!(rotating_goldsrc_full_speed_samples(201, 115), 200);
    }

    #[test]
    fn c1a1b_cosmetic_fans_stall_on_goldsrc_rotate_heartbeat() {
        // brushes *6/*7 reach full speed at map tick 26. GoldSrc's second
        // Rotate heartbeat lands at tick 427 and advances by only 0.0267 deg.
        let p426 = rotating_cosmetic_phase_q16(426, 3641, true, false, 45, false, false);
        let p427 = rotating_cosmetic_phase_q16(427, 3641, true, false, 45, false, false);
        let p428 = rotating_cosmetic_phase_q16(428, 3641, true, false, 45, false, false);
        assert_eq!(p427, p426);
        assert_eq!(p428, (p426 + 3641) & 0xffff);

        // brush *8 reaches full speed three ticks later after its 100% ramp;
        // its corresponding near-zero heartbeat is map tick 430.
        let p429 = rotating_cosmetic_phase_q16(429, -1820, true, true, 100, false, false);
        let p430 = rotating_cosmetic_phase_q16(430, -1820, true, true, 100, false, false);
        let p431 = rotating_cosmetic_phase_q16(431, -1820, true, true, 100, false, false);
        assert_eq!(p430, p429);
        assert_eq!(p431, (p429 - 1820) & 0xffff);
    }

    #[test]
    fn c1a1b_cosmetic_fans_match_reference_tick_40() {
        let q12 = |phase: i32| ((phase >> 4) as u16) & 0x0fff;
        assert_eq!(
            q12(rotating_cosmetic_phase_q16(
                40, 3641, true, false, 45, false, false
            )),
            3185,
        );
        assert_eq!(
            q12(rotating_cosmetic_phase_q16(
                40, -3641, true, false, 45, false, false
            )),
            910,
        );
        assert_eq!(
            q12(rotating_cosmetic_phase_q16(
                40, -1820, true, true, 100, false, false
            )),
            2844,
        );
    }

    #[test]
    fn c1a1c_twenty_percent_ramp_matches_goldsrc_checkpoints() {
        let q12 = |phase: i32| ((phase >> 4) as u16) & 0x0fff;
        let expected = [
            (40, 363),
            (60, 2183),
            (80, 4003),
            (100, 1727),
            (120, 3547),
            (140, 1271),
        ];
        for (tick, phase) in expected {
            assert_eq!(
                q12(rotating_cosmetic_phase_q16(
                    tick, 1456, true, true, 20, true, false,
                )),
                phase,
                "c1a1c brush *31 at tick {tick}",
            );
        }
    }

    #[test]
    fn c1a1c_copivot_fans_retain_their_first_blocked_sample() {
        for tick in 27..=600 {
            assert_eq!(
                rotating_cosmetic_phase_q16(tick, -637, true, false, 100, false, true),
                (-637i32) & 0xffff,
            );
        }
    }

    #[test]
    fn pendulum_phase_word_keeps_signed_q19_angle_and_heartbeat_index() {
        for (phase, index) in [
            (0, 0),
            (123_456, 7),
            (-789, 148),
            (-(1 << 19), PENDULUM_THINK_MAX),
        ] {
            let packed = pack_pendulum_phase(phase, index);
            assert_eq!(pendulum_phase_q19(packed), phase);
            assert_eq!(pendulum_think_index(packed), index);
        }
    }

    #[test]
    fn c1a1b_pendulum_matches_goldsrc_float_pusher_checkpoints() {
        // brush *79: distance=3, speed=5. The cooker stores centre, maximum
        // velocity and acceleration in Q19-turn units; START_ON's first Swing
        // callback is map tick 20 in the deterministic SDK trace.
        let mut state = PendulumState {
            packed_phase: pack_pendulum_phase(0, 0),
            velocity_q19: 0,
            state: PENDULUM_START_WAIT,
            next_tick: PENDULUM_AUTO_START_TICKS,
        };
        let expected = [
            (20u16, 0),
            (40, 2549),
            (60, 5279),
            (100, -789),
            (320, 2245),
            (340, -152),
            (400, 1790),
            (500, -576),
            (580, 516),
        ];
        let mut next_expected = 0usize;
        for now in 0..=599u16 {
            state = pendulum_tick(state, 2185, 364, 15, now);
            if next_expected < expected.len() && expected[next_expected].0 == now {
                let error =
                    (pendulum_phase_q19(state.packed_phase) - expected[next_expected].1).abs();
                assert!(
                    error <= 80,
                    "tick {now}: Q19 phase error {error} exceeds 0.055 degree"
                );
                next_expected += 1;
            }
        }
        assert_eq!(next_expected, expected.len());
    }

    #[test]
    fn pendulum_use_stops_without_resetting_visible_angle() {
        let moving = PendulumState {
            packed_phase: pack_pendulum_phase(-1234, 77),
            velocity_q19: -90,
            state: PENDULUM_SWINGING,
            next_tick: 44,
        };
        let stopped = pendulum_use(moving, 45);
        assert_eq!(stopped.state, PENDULUM_STOPPED);
        assert_eq!(stopped.velocity_q19, 0);
        assert_eq!(pendulum_phase_q19(stopped.packed_phase), -1234);

        let restarting = pendulum_use(stopped, 80);
        assert_eq!(restarting.state, PENDULUM_START_WAIT);
        assert_eq!(restarting.next_tick, 83);
        assert_eq!(pendulum_phase_q19(restarting.packed_phase), -1234);
        assert_eq!(pendulum_think_index(restarting.packed_phase), 0);
    }
}
