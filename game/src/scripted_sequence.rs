//! scripted_sequence lifecycle: claiming an actor, moving it to the mark
//! (walk, run or instant), planting it, turning it to the authored yaw,
//! playing, firing the sequence's targets and releasing the actor.
//!
//! The runtime packs [`ScriptActor`] into existing per-prop arrays; the game
//! adaptor unpacks one actor, runs a step here and packs it back. Every
//! [`ScriptWorld`] action that can move the actor or fire other logic is
//! bracketed by the adaptor: it writes `actor` to the world before the action
//! and re-reads it afterwards, so a step always sees the current state.
//!
//! Time is the 20 Hz simulation tick, a wrapping `u16`. Actors think at
//! 10 Hz on alternating ticks chosen by their roster slot.

use crate::scientist_logic;
use psx_math::int32::isqrt_i32;
use psx_math::sincos;

/// Runtime move modes (the authored mode is mapped by [`runtime_move_mode`]).
pub const MODE_NONE: u8 = 0;
pub const MODE_WALK: u8 = 1;
pub const MODE_RUN: u8 = 2;
/// Play in place: no move, no turn.
pub const MODE_IN_PLACE: u8 = 3;
/// Teleport to the mark.
pub const MODE_INSTANT: u8 = 4;
/// Turning to the authored yaw after planting.
pub const MODE_FACE: u8 = scientist_logic::SCRIPT_FACE_MODE;
/// No clip.
pub const NO_CLIP: u8 = 0xFF;
/// Authored spawnflags.
pub const SF_REPEATABLE: u16 = 4;
pub const SF_NO_INTERRUPT: u16 = 32;
/// Player half-height used for the walking actor's lookahead.
pub const ROOT_DROP_DEPTH: i32 = 256;

#[inline(always)]
const fn reached(now: u16, at: u16) -> bool {
    now.wrapping_sub(at) < 0x8000
}

/// What a scripted_sequence entity authored, decoded from the cooked record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScriptRecord {
    /// Targetname of the wanted actor, or zero.
    pub actor_name: u16,
    /// Class to fall back to, from the cooked selector, if any.
    pub class_kind: Option<u8>,
    /// Class search radius.
    pub radius: i32,
    /// Authored move mode (0..=5).
    pub move_mode: u16,
    /// The mark.
    pub origin: [i32; 3],
    /// Authored 12-bit yaw.
    pub yaw: u16,
    pub repeatable: bool,
    pub no_interrupt: bool,
    /// The sequence has an idle animation.
    pub has_idle: bool,
    /// The sequence has a play animation.
    pub has_play: bool,
    /// The sequence has a targetname (something must fire it).
    pub targeted: bool,
    /// Cooked clip slots, or [`NO_CLIP`].
    pub play_clip: u8,
    pub idle_clip: u8,
    /// Where the play animation leaves the actor: (forward, left) units.
    pub root_offset: Option<[i32; 2]>,
}

/// Decoded movement mode of an actor's current script.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScriptMode {
    /// One of the `MODE_*` values.
    pub base: u8,
    /// Possessed at map start by a targeted idle script, waiting for its Use.
    pub primed: bool,
    /// Following the node graph.
    pub routed: bool,
    /// Heading for an intermediate waypoint before the mark.
    pub detour: bool,
}

impl ScriptMode {
    pub const NONE: Self = Self {
        base: MODE_NONE,
        primed: false,
        routed: false,
        detour: false,
    };
}

/// The actor's coarse activity as far as scripts care.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Activity {
    Idle,
    Moving,
    /// Anything else; scripts never set it.
    Other,
}

/// Per-actor script state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScriptActor {
    /// Observed position; changes only through [`ScriptWorld`] actions.
    pub pos: [i32; 3],
    /// Observed; the actor has health left.
    pub alive: bool,
    pub activity: Activity,
    /// Current 12-bit facing.
    pub yaw: u16,
    pub mode: ScriptMode,
    /// The owning sequence, if any.
    pub script: Option<usize>,
    /// Current movement waypoint (the mark, or a detour/route waypoint).
    pub goal: [i16; 3],
    /// Yaw to face at the mark (also the step-aside ideal yaw when idle).
    pub target_yaw: u16,
    /// Sub-unit movement remainders (x, z), each -8..=7 sixteenths.
    pub residue: [i32; 2],
    /// Move deadline while moving; play end while playing; idle phase origin
    /// while holding.
    pub deadline: u16,
    pub play_clip: u8,
    pub idle_clip: u8,
    /// Movement hold, reused as the face-phase counter.
    pub hold: u8,
    pub no_interrupt: bool,
}

/// How a talk monster is described to the scripted step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActorKind {
    pub scientist: bool,
    pub barney: bool,
    pub houndeye: bool,
}

/// How a walk or run reaches its mark, chosen once at assignment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoutePlan {
    /// First waypoint when it is not the mark itself.
    pub waypoint: Option<[i32; 3]>,
    pub routed: bool,
    pub detour: bool,
}

impl RoutePlan {
    pub const DIRECT: Self = Self {
        waypoint: None,
        routed: false,
        detour: false,
    };
}

/// Everything a scripted actor asks of, or does to, the rest of the game.
pub trait ScriptWorld {
    /// The sequence entity `script`, or `None` when out of range.
    fn record(&self, script: usize) -> Option<ScriptRecord>;
    /// How long the actor's play gesture holds it at the mark.
    fn play_hold_ticks(&mut self, actor: &ScriptActor) -> u16;
    /// One talk-monster speech think while walking to the mark.
    fn talk_turn(&mut self, actor: &mut ScriptActor);
    /// Re-check the path ahead and re-plan when it is blocked.
    fn local_replan(&mut self, actor: &mut ScriptActor, waypoint: [i32; 3], now: u16);
    /// Choose how to reach `goal` from the actor's position.
    fn plan_route(&mut self, actor: &ScriptActor, goal: [i32; 3], now: u16) -> RoutePlan;
    /// One movement step toward `goal`.
    fn move_toward(&mut self, actor: &mut ScriptActor, goal: [i32; 3], speed: i32);
    /// Put the actor exactly at `pos` (no floor drop).
    fn place(&mut self, actor: &mut ScriptActor, pos: [i32; 3]);
    /// Drop the actor's cached graph route.
    fn forget_route(&mut self);
    /// Floor height below `probe`, searching `depth` units down.
    fn floor_below(&mut self, actor: &ScriptActor, probe: [i32; 3], depth: i32) -> Option<i32>;
    /// Fire the sequence's animation events for this tick.
    fn studio_events(&mut self, actor: &mut ScriptActor, idle: bool);
    /// The talk state forgets a push and any step-aside.
    fn take_over_talker(&mut self, actor: &mut ScriptActor);
    /// Mark the sequence entity as running.
    fn script_started(&mut self, script: usize);
    /// Fire the sequence's targets.
    fn fire_targets(&mut self, actor: &mut ScriptActor, script: usize);
    /// Remove the sequence entity.
    fn remove_script(&mut self, actor: &mut ScriptActor, script: usize);
    /// Diagnostic trace of a navigation step.
    fn trace(&mut self, a: i32, b: i32, code: u8);
}

/// Map the authored move mode to the runtime one.
#[inline]
pub const fn runtime_move_mode(authored: u16) -> u8 {
    match authored {
        0 | 5 => MODE_IN_PLACE,
        1 => MODE_WALK,
        2 => MODE_RUN,
        4 => MODE_INSTANT,
        _ => MODE_IN_PLACE,
    }
}

/// Planar distance left to walk: to the waypoint, plus waypoint to mark
/// while detouring.
fn remaining_distance(actor: &ScriptActor, world: &dyn ScriptWorld) -> u32 {
    let g = actor.goal;
    let waypoint = [g[0] as i32, g[1] as i32, g[2] as i32];
    let pos = actor.pos;
    let dx = waypoint[0] - pos[0];
    let dz = waypoint[2] - pos[2];
    let mut distance = isqrt_i32(dx * dx + dz * dz).max(0) as u32;
    if actor.mode.detour {
        if let Some(rec) = actor.script.and_then(|s| world.record(s)) {
            let ax = rec.origin[0] - waypoint[0];
            let az = rec.origin[2] - waypoint[2];
            distance = distance.saturating_add(isqrt_i32(ax * ax + az * az).max(0) as u32);
        }
    }
    distance
}

/// Give `actor` to the sequence `script` and start it.
pub fn assign(
    actor: &mut ScriptActor,
    script: usize,
    rec: &ScriptRecord,
    kind: ActorKind,
    now: u16,
    world: &mut dyn ScriptWorld,
) {
    world.take_over_talker(actor);
    let mode = runtime_move_mode(rec.move_mode);
    let mark = rec.origin;
    actor.goal = [mark[0] as i16, mark[1] as i16, mark[2] as i16];
    actor.target_yaw = rec.yaw & 0x0fff;
    actor.residue[0] = 0;
    world.forget_route();
    let mut plan = RoutePlan::DIRECT;
    if mode == MODE_WALK || mode == MODE_RUN {
        plan = world.plan_route(actor, mark, now);
        if let Some(w) = plan.waypoint {
            actor.goal = [w[0] as i16, w[1] as i16, w[2] as i16];
        }
    }
    actor.mode = ScriptMode {
        base: mode,
        primed: false,
        routed: plan.routed,
        detour: plan.detour,
    };
    actor.script = Some(script);
    actor.residue[1] = 0;
    if mode == MODE_WALK || mode == MODE_RUN {
        actor.hold = scientist_logic::SCRIPT_MOVE_START_ACTIVE_TICKS;
        let distance = remaining_distance(actor, world);
        let speed = scientist_logic::script_timeout_speed(mode, kind.barney);
        actor.deadline =
            now.wrapping_add(scientist_logic::script_move_timeout_ticks(distance, speed));
    } else {
        actor.hold = 0;
        actor.deadline = now;
    }
    actor.no_interrupt = rec.no_interrupt;
    actor.play_clip = rec.play_clip;
    actor.idle_clip = rec.idle_clip;
}

/// Possess `actor` at map start for a sequence that waits for its Use.
pub fn prime(
    actor: &mut ScriptActor,
    script: usize,
    rec: &ScriptRecord,
    kind: ActorKind,
    world: &mut dyn ScriptWorld,
) {
    assign(actor, script, rec, kind, 0, world);
    actor.mode.primed = true;
    actor.play_clip = NO_CLIP;
    actor.activity = Activity::Idle;
    actor.deadline = scientist_logic::SCRIPT_PRIME_DELAY_TICKS;
}

/// For a primed actor: true while it must keep waiting this tick.
pub fn primed_holds(
    actor: &mut ScriptActor,
    kind: ActorKind,
    now: u16,
    world: &dyn ScriptWorld,
) -> bool {
    let encoded = encode_mode(actor.mode);
    match scientist_logic::script_prime_gate(
        encoded,
        actor.activity == Activity::Moving,
        reached(now, actor.deadline),
    ) {
        scientist_logic::ScriptPrimeGate::Hold => true,
        scientist_logic::ScriptPrimeGate::Execute => false,
        scientist_logic::ScriptPrimeGate::StartMove => {
            let distance = remaining_distance(actor, world);
            let speed = scientist_logic::script_timeout_speed(actor.mode.base, kind.barney);
            actor.deadline =
                now.wrapping_add(scientist_logic::script_move_timeout_ticks(distance, speed));
            actor.activity = Activity::Moving;
            false
        }
    }
}

/// The packed mode byte for `mode` (base plus flag bits).
#[inline]
pub const fn encode_mode(mode: ScriptMode) -> u8 {
    let mut m = mode.base;
    if mode.primed {
        m = scientist_logic::script_primed_mode(m);
    }
    m = scientist_logic::script_route_mode(m, mode.routed);
    scientist_logic::script_detour_mode(m, mode.detour)
}

/// Inverse of [`encode_mode`].
#[inline]
pub const fn decode_mode(m: u8) -> ScriptMode {
    ScriptMode {
        base: scientist_logic::script_base_mode(m),
        primed: scientist_logic::script_is_primed(m),
        routed: scientist_logic::script_uses_route(m),
        detour: scientist_logic::script_uses_detour(m),
    }
}

/// Release the actor from its script without firing anything. The caller
/// also drops the actor's cached graph route.
pub fn release(actor: &mut ScriptActor) {
    actor.mode = ScriptMode::NONE;
    actor.script = None;
    actor.play_clip = NO_CLIP;
    actor.idle_clip = NO_CLIP;
    actor.deadline = 0;
    actor.no_interrupt = false;
}

/// Damage cancels the actor's script unless it is uninterruptible; death
/// always does.
#[inline]
pub const fn damage_cancels(alive_after: bool, no_interrupt: bool) -> bool {
    !alive_after || !no_interrupt
}

/// What firing a scripted_sequence does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Claim {
    /// Give the script to this roster slot.
    Assign(usize),
    /// Already playing: ignore the duplicate fire.
    Ignore,
    /// No actor available: try again in [`RETRY_TICKS`].
    Retry,
}

pub const RETRY_TICKS: u16 = scientist_logic::SCRIPT_RETRY_TICKS;

/// How an actor looks to the claim search.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub active: bool,
    pub alive: bool,
    /// Pickups and other non-actors.
    pub item: bool,
    pub name: u16,
    pub kind: u8,
    pub pos: [i32; 3],
    pub busy: bool,
    pub script: Option<usize>,
    pub primed: bool,
}

/// The actor roster as the claim search sees it.
pub trait Roster {
    fn len(&self) -> usize;
    fn candidate(&self, slot: usize) -> Candidate;
}

/// Pick a free actor: by name first, else the first of the wanted class
/// within the radius, in roster order.
pub fn find_actor(rec: &ScriptRecord, roster: &dyn Roster) -> Option<usize> {
    let n = roster.len();
    let mut slot = 0usize;
    while slot < n {
        let c = roster.candidate(slot);
        if c.active && c.alive && !c.item && c.name != 0 && c.name == rec.actor_name && !c.busy {
            return Some(slot);
        }
        slot += 1;
    }
    let wanted = rec.class_kind?;
    slot = 0;
    while slot < n {
        let c = roster.candidate(slot);
        let delta = [
            c.pos[0] - rec.origin[0],
            c.pos[1] - rec.origin[1],
            c.pos[2] - rec.origin[2],
        ];
        if scientist_logic::script_candidate_eligible(
            c.kind,
            wanted,
            c.active,
            c.alive as u8,
            c.busy,
            delta,
            rec.radius,
        ) {
            return Some(slot);
        }
        slot += 1;
    }
    None
}

/// The sequence `script` was fired.
pub fn claim(script: usize, rec: &ScriptRecord, roster: &dyn Roster) -> Claim {
    let n = roster.len();
    let mut slot = 0usize;
    while slot < n {
        let c = roster.candidate(slot);
        if c.active && c.alive && c.script == Some(script) {
            return match scientist_logic::script_owned_use(true, c.primed) {
                scientist_logic::ScriptOwnedUse::AssignPrimed => Claim::Assign(slot),
                scientist_logic::ScriptOwnedUse::IgnorePlaying => Claim::Ignore,
                scientist_logic::ScriptOwnedUse::Search => break,
            };
        }
        slot += 1;
    }
    match find_actor(rec, roster) {
        Some(slot) => Claim::Assign(slot),
        None => Claim::Retry,
    }
}

/// What a sequence does at map start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnAction {
    Nothing,
    /// Possess an actor now and wait.
    Prime,
    /// Run now, as if fired.
    Fire,
}

pub const fn spawn_action(rec: &ScriptRecord) -> SpawnAction {
    if !rec.targeted {
        if scientist_logic::script_untargeted_idle_holds(rec.has_idle, rec.has_play) {
            SpawnAction::Prime
        } else {
            SpawnAction::Fire
        }
    } else if rec.has_idle {
        SpawnAction::Prime
    } else {
        SpawnAction::Nothing
    }
}

/// Per-tick inputs of a scripted actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TickContext {
    pub now: u16,
    /// The half-rate movement phase counter.
    pub move_tick: u16,
    /// Roster slot; its parity picks the actor's 10 Hz think.
    pub slot: u16,
    pub kind: ActorKind,
    /// A primed actor that must keep waiting this tick.
    pub primed_hold: bool,
}

/// Where a finished play animation leaves the actor before the floor drop.
pub fn root_motion_target(pos: [i32; 3], yaw: u16, offset: [i32; 2]) -> [i32; 3] {
    let (f, l) = (offset[0], offset[1]);
    let s = sincos::sin_q12(yaw);
    let c = sincos::sin_q12((yaw + 1024) & 0x0fff);
    [
        pos[0] + ((s * f - c * l) >> 12),
        pos[1],
        pos[2] + ((c * f + s * l) >> 12),
    ]
}

fn root_motion(actor: &mut ScriptActor, rec: &ScriptRecord, world: &mut dyn ScriptWorld) {
    let Some(offset) = rec.root_offset else {
        return;
    };
    if !actor.alive {
        return;
    }
    let p = actor.pos;
    let mut pos = root_motion_target(p, actor.yaw, offset);
    if let Some(y) = world.floor_below(actor, [pos[0], p[1] + 1, pos[2]], ROOT_DROP_DEPTH) {
        pos[1] = y;
    }
    world.forget_route();
    world.place(actor, pos);
}

fn finish_face_phase(
    actor: &mut ScriptActor,
    primed: bool,
    cx: &TickContext,
    world: &mut dyn ScriptWorld,
) {
    if primed {
        // A walk or run that waited at map start begins its play as soon as
        // its actor has turned on the mark.
        if let Some(script) = actor.script {
            world.script_started(script);
            if let Some(rec) = world.record(script) {
                assign(actor, script, &rec, cx.kind, cx.now, world);
            }
        }
        return;
    }
    actor.mode = ScriptMode::NONE;
    actor.deadline = if actor.play_clip != NO_CLIP {
        cx.now.wrapping_add(world.play_hold_ticks(actor))
    } else {
        cx.now
    };
}

/// One tick of a script-owned actor. Returns true when the script consumed
/// the tick (ordinary AI must not run).
pub fn tick(actor: &mut ScriptActor, cx: &TickContext, world: &mut dyn ScriptWorld) -> bool {
    let now = cx.now;
    let base = actor.mode.base;
    if cx.primed_hold {
        actor.activity = Activity::Idle;
        if actor.mode.base == MODE_NONE {
            world.studio_events(actor, true);
        }
        return true;
    }
    let my_think = (cx.slot ^ cx.move_tick) & 1 == 0;
    if base != MODE_NONE {
        let walking = base == MODE_WALK || base == MODE_RUN;
        if walking && cx.kind.scientist && actor.hold == 0 {
            world.talk_turn(actor);
        }
        if walking
            && my_think
            && actor.hold == 0
            && !reached(now, actor.deadline)
            && !actor.mode.routed
            && !actor.mode.detour
        {
            let g = actor.goal;
            world.local_replan(actor, [g[0] as i32, g[1] as i32, g[2] as i32], now);
        }
        let g = actor.goal;
        let waypoint = [g[0] as i32, g[1] as i32, g[2] as i32];
        let mode_now = actor.mode;
        let primed = mode_now.primed;
        let detouring = mode_now.detour;
        let mode = base;
        if mode == MODE_FACE {
            actor.activity = Activity::Idle;
            let phase = actor.hold;
            if phase != 0 && phase <= scientist_logic::SCRIPT_FACE_SETTLE_TICKS {
                actor.hold = phase - 1;
                if phase == 1 {
                    finish_face_phase(actor, primed, cx, world);
                }
                return true;
            }
            if !my_think {
                return true;
            }
            let target = actor.target_yaw;
            let first = phase == scientist_logic::SCRIPT_FACE_FIRST_PENDING;
            world.trace(
                actor.yaw as i32,
                target as i32,
                if first { 0x61 } else { 0x60 },
            );
            actor.yaw = scientist_logic::script_face_yaw_step(actor.yaw, target, first);
            actor.hold = 0;
            if actor.yaw == target {
                actor.hold = scientist_logic::SCRIPT_FACE_SETTLE_TICKS;
            }
            return true;
        }
        let walking = mode == MODE_WALK || mode == MODE_RUN;
        let move_timed_out = walking && reached(now, actor.deadline);
        // At the deadline the actor is planted on the mark itself, not on an
        // intermediate waypoint.
        let goal = if move_timed_out && detouring && actor.script.is_some() {
            actor
                .script
                .and_then(|s| world.record(s))
                .map(|r| r.origin)
                .unwrap_or(waypoint)
        } else {
            waypoint
        };
        let arrived = if move_timed_out {
            world.trace(goal[0] - actor.pos[0], goal[2] - actor.pos[2], 0x40);
            world.forget_route();
            world.place(actor, goal);
            true
        } else if mode == MODE_IN_PLACE {
            true
        } else if mode == MODE_INSTANT {
            world.forget_route();
            world.place(actor, goal);
            true
        } else {
            let start_pending = actor.hold != 0;
            let dx = goal[0] - actor.pos[0];
            let dz = goal[2] - actor.pos[2];
            if scientist_logic::script_at_mark(dx * dx + dz * dz, start_pending) {
                true
            } else {
                let speed = if mode == MODE_RUN && cx.kind.houndeye {
                    20
                } else {
                    scientist_logic::script_move_speed(mode, cx.move_tick, cx.kind.barney) as i32
                };
                world.move_toward(actor, goal, speed);
                actor.activity = Activity::Moving;
                false
            }
        };
        if arrived && detouring && !move_timed_out {
            // The intermediate waypoint is done: head for the mark itself.
            if let Some(rec) = actor.script.and_then(|s| world.record(s)) {
                let m = rec.origin;
                actor.goal = [m[0] as i16, m[1] as i16, m[2] as i16];
            }
            actor.mode = ScriptMode {
                detour: false,
                ..mode_now
            };
            actor.residue = [0, 0];
            actor.hold = 0;
            actor.activity = Activity::Moving;
            return true;
        }
        if arrived {
            if mode != MODE_IN_PLACE {
                world.place(actor, goal);
                actor.residue = [0, 0];
            }
            actor.activity = Activity::Idle;
            let face_first = walking
                && actor
                    .script
                    .and_then(|s| world.record(s))
                    .is_some_and(|r| r.has_idle);
            if face_first {
                actor.mode = ScriptMode {
                    base: MODE_FACE,
                    primed,
                    routed: false,
                    detour: false,
                };
                actor.hold = scientist_logic::SCRIPT_FACE_FIRST_PENDING;
                return true;
            }
            if mode != MODE_IN_PLACE {
                actor.yaw = actor.target_yaw;
            }
            actor.mode = ScriptMode {
                base: MODE_NONE,
                primed,
                routed: false,
                detour: false,
            };
            if primed {
                // Planted and waiting for the Use; the deadline becomes the
                // idle animation's phase origin.
                actor.deadline = now;
                return true;
            }
            actor.deadline = if actor.play_clip != NO_CLIP {
                now.wrapping_add(world.play_hold_ticks(actor))
            } else {
                now
            };
        }
        return true;
    }

    if actor.script.is_some() && !reached(now, actor.deadline) {
        // Still playing: the script owns the actor for the whole gesture.
        world.studio_events(actor, false);
        actor.activity = Activity::Idle;
        return true;
    }

    if let Some(script) = actor.script {
        if reached(now, actor.deadline) {
            // Release before firing so a chained script can take the actor.
            release(actor);
            world.forget_route();
            if let Some(rec) = world.record(script) {
                root_motion(actor, &rec, world);
                world.fire_targets(actor, script);
                if !rec.repeatable {
                    world.remove_script(actor, script);
                }
            }
        }
    }
    if actor.idle_clip != NO_CLIP && actor.alive {
        actor.activity = Activity::Idle;
        return true;
    }
    false
}
