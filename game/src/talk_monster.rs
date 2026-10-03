//! Talk-monster behaviour shared by scientists and Barney: the one shared
//! dialogue channel, greetings, pre-disaster small talk and answers, the
//! player bump that makes an actor step aside, and the +use follow toggle.
//!
//! The runtime keeps every field of [`TalkActor`] packed into spare bits of
//! existing per-prop arrays; the game adaptor unpacks one actor, calls a step
//! here and packs the result back. Everything the step needs from the rest of
//! the world (line-of-sight, other actors, cooked voice ids) comes through
//! [`TalkWorld`], so the host runner can script it.
//!
//! Time is the 20 Hz simulation tick, a wrapping `u16`.

use crate::scientist_logic;
use psx_math::sincos;

/// No actor owns the dialogue channel.
pub const NO_SPEAKER: u8 = 0xff;
/// Voice-byte flag for a line the actor chose itself (greeting, small talk),
/// as opposed to an authored sentence or a +use reply.
pub const VOICE_AUTONOMOUS: u8 = 0x80;
pub const VOICE_ID_MASK: u8 = 0x3f;
/// Silence every actor must leave after the previous line ends (2 s).
pub const CONVERSATION_GAP_TICKS: u16 = 40;
/// The same actor waits this long between its own idle lines (60 s).
pub const IDLE_LINE_REPEAT_TICKS: u16 = 1_200;
/// A pending answer that could not start within this many ticks is dropped.
pub const ANSWER_LATE_TICKS: u16 = 40;
/// An answering actor may take its next turn this long after its answer ends.
pub const ANSWER_NEXT_TURN_TICKS: u16 = 48;
/// Step-aside walk speed in units per tick.
pub const MOVE_AWAY_SPEED: i32 = 3;
/// Planar squared distance at which the step-aside walk counts as arrived.
pub const MOVE_AWAY_ARRIVE_D2: i32 = 9;
/// Movement hold applied when the step-aside walk begins.
pub const MOVE_AWAY_START_HOLD_TICKS: u8 = 1;
/// How long the actor turns back toward the player after stepping aside.
pub const MOVE_AWAY_FACE_TICKS: u8 = 15;
/// Step-aside walk give-up time (4 s).
pub const MOVE_AWAY_TIMEOUT_TICKS: u8 = 80;
/// Height of a standing talk monster's hull top above its origin.
pub const ACTOR_HULL_TOP: i32 = 72;
/// Height of the standing player's hull top above its origin.
pub const PLAYER_HULL_TOP: i32 = 36;

#[inline(always)]
const fn reached(now: u16, at: u16) -> bool {
    now.wrapping_sub(at) < 0x8000
}

/// The single dialogue channel every talk monster shares.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TalkClock {
    /// Roster slot of the actor speaking, or [`NO_SPEAKER`].
    pub speaker: u8,
    /// Voice id, with [`VOICE_AUTONOMOUS`] set for self-chosen lines.
    pub voice: u8,
    /// Tick at which the current line ends.
    pub until: u16,
}

impl TalkClock {
    pub const SILENT: Self = Self {
        speaker: NO_SPEAKER,
        voice: 0,
        until: 0,
    };

    /// True once nobody has spoken, or the conversation gap after the last
    /// line has elapsed.
    #[inline(always)]
    pub fn gap_over(&self, now: u16) -> bool {
        self.speaker == NO_SPEAKER || reached(now, self.until.wrapping_add(CONVERSATION_GAP_TICKS))
    }

    /// True while `actor` is mid-line.
    #[inline(always)]
    pub fn is_talking(&self, actor: usize, now: u16) -> bool {
        self.speaker as usize == actor && !reached(now, self.until)
    }

    /// True while `actor` is mid-way through a line it did not choose itself
    /// (an authored sentence or a +use reply). Such an actor ignores +use.
    #[inline(always)]
    pub fn directed_line_playing(&self, actor: usize, now: u16) -> bool {
        self.speaker == actor as u8
            && self.voice & VOICE_AUTONOMOUS == 0
            && !reached(now, self.until)
    }

    /// Hand the channel to `actor` for `ticks` ticks from `now`.
    #[inline(always)]
    pub fn begin(&mut self, actor: usize, voice: u8, now: u16, ticks: u16) {
        self.speaker = actor as u8;
        self.voice = voice;
        self.until = now.wrapping_add(ticks);
    }
}

/// Coarse schedule of a talk monster as far as this module cares.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TalkState {
    Idle,
    /// Any other runtime state (moving, attacking, dead): owned elsewhere.
    Busy,
    /// Walking to the step-aside goal after a player bump.
    MoveAway,
    /// Turning back toward the player after stepping aside.
    FaceBack,
}

/// Persistent per-actor talk state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TalkActor {
    /// Roster slot; the id written to the dialogue channel.
    pub index: usize,
    pub alive: bool,
    pub state: TalkState,
    pub following: bool,
    pub provoked: bool,
    pub predisaster: bool,
    pub hello_said: bool,
    /// A qualifying player bump is waiting for the next think.
    pub client_push: bool,
    /// Another scientist asked this one a question.
    pub answer_pending: bool,
    /// Answer due tick while `answer_pending`, otherwise the earliest tick of
    /// this actor's next idle line.
    pub speech_due: u16,
    /// Step-aside give-up countdown.
    pub schedule_timer: u8,
    /// Movement hold, reused as the face-back countdown.
    pub hold_ticks: u8,
    /// Step-aside goal.
    pub move_goal: [i16; 3],
}

/// What a talk monster sees on a speech think.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpeechContext {
    pub now: u16,
    pub map_index: u16,
    /// False when the map carries no autonomous dialogue.
    pub voices_enabled: bool,
    pub player_in_pvs: bool,
    pub player_alive: bool,
    pub player_pos: [i32; 3],
    /// The player's eye point.
    pub player_eye: [i32; 3],
    pub actor_pos: [i32; 3],
    /// Current 12-bit facing.
    pub actor_yaw: u16,
    pub script_busy: bool,
    /// The actor is walking or running to a scripted mark.
    pub scripted_moving: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineKind {
    Hello,
    Answer,
    Question,
    Idle,
}

/// A line an actor just started on the shared channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Line {
    pub kind: LineKind,
    /// Voice id without the autonomous flag.
    pub voice: u8,
    pub ticks: u16,
    /// For a question: the roster slot that must answer when it ends.
    pub listener: Option<usize>,
}

/// World queries a talk monster makes while deciding to speak.
pub trait TalkWorld {
    /// Unobstructed sight line between two points.
    fn line_clear(&mut self, from: [i32; 3], to: [i32; 3]) -> bool;
    /// The speaking actor's sight origin.
    fn speaker_eye(&self) -> [i32; 3];
    /// Number of roster slots to scan for a listener.
    fn roster_len(&self) -> usize;
    /// Hull top of roster slot `slot` when it is a living, idle, pre-disaster
    /// scientist that could answer a question; `None` otherwise.
    fn idle_listener_top(&self, slot: usize) -> Option<[i32; 3]>;
    /// The speaker's position among the map's scientists.
    fn scientist_ordinal(&mut self) -> usize;
    /// Cooked voice id for the speaker's line of `kind`.
    fn voice_id(&mut self, kind: LineKind) -> u8;
}

/// One speech think: answer a pending question, else greet the player, else
/// start small talk. Returns the line started, if any.
pub fn take_turn(
    actor: &mut TalkActor,
    clock: &mut TalkClock,
    cx: &SpeechContext,
    world: &mut dyn TalkWorld,
) -> Option<Line> {
    if let Some(line) = try_answer(actor, clock, cx, world) {
        return Some(line);
    }
    if let Some(line) = try_hello(actor, clock, cx, world) {
        return Some(line);
    }
    try_small_talk(actor, clock, cx, world)
}

fn try_answer(
    actor: &mut TalkActor,
    clock: &mut TalkClock,
    cx: &SpeechContext,
    world: &mut dyn TalkWorld,
) -> Option<Line> {
    let now = cx.now;
    if !cx.voices_enabled
        || now & 1 != 0
        || !actor.answer_pending
        || !cx.player_in_pvs
        || !cx.player_alive
        || !actor.alive
        || !actor.predisaster
    {
        return None;
    }
    let due = actor.speech_due;
    if !reached(now, due) {
        return None;
    }
    let late = now.wrapping_sub(due);
    if late > ANSWER_LATE_TICKS && late < 0x8000 {
        actor.answer_pending = false;
        return None;
    }
    if !reached(now, clock.until) {
        return None;
    }
    let ordinal = world.scientist_ordinal();
    let voice = world.voice_id(LineKind::Answer);
    let ticks = scientist_logic::scientist_answer_duration_ticks(cx.map_index, ordinal);
    clock.begin(actor.index, voice | VOICE_AUTONOMOUS, now, ticks);
    actor.answer_pending = false;
    actor.speech_due = clock.until.wrapping_add(ANSWER_NEXT_TURN_TICKS);
    actor.hello_said = true;
    actor.client_push = false;
    Some(Line {
        kind: LineKind::Answer,
        voice,
        ticks,
        listener: None,
    })
}

fn try_hello(
    actor: &mut TalkActor,
    clock: &mut TalkClock,
    cx: &SpeechContext,
    world: &mut dyn TalkWorld,
) -> Option<Line> {
    let now = cx.now;
    if !cx.voices_enabled {
        return None;
    }
    if if cx.scripted_moving {
        now & 1 != 0
    } else {
        !scientist_logic::idle_hello_attempt(now)
    } {
        return None;
    }
    if !cx.player_in_pvs
        || !cx.player_alive
        || (!cx.scripted_moving && actor.state != TalkState::Idle)
        || actor.hello_said
        || actor.provoked
        || actor.following
        || (!cx.scripted_moving && cx.script_busy)
        || !clock.gap_over(now)
    {
        return None;
    }
    let pos = cx.actor_pos;
    let player = cx.player_pos;
    if !scientist_logic::friend_in_talk_range(
        [pos[0], pos[1] + ACTOR_HULL_TOP, pos[2]],
        [player[0], player[1] + PLAYER_HULL_TOP, player[2]],
    ) {
        return None;
    }
    let dx = player[0] - pos[0];
    let dz = player[2] - pos[2];
    let yaw = cx.actor_yaw;
    let forward_x = sincos::sin_q12(yaw);
    let forward_z = sincos::sin_q12((yaw + 1024) & 0x0fff);
    if !scientist_logic::wide_view_cone(dx, dz, forward_x, forward_z) {
        return None;
    }
    let eye = world.speaker_eye();
    if !world.line_clear(eye, cx.player_eye) {
        return None;
    }
    let ordinal = world.scientist_ordinal();
    let voice = world.voice_id(LineKind::Hello);
    let ticks = scientist_logic::hello_duration_ticks(cx.map_index, ordinal);
    clock.begin(actor.index, voice | VOICE_AUTONOMOUS, now, ticks);
    actor.hello_said = true;
    actor.client_push = false;
    Some(Line {
        kind: LineKind::Hello,
        voice,
        ticks,
        listener: None,
    })
}

fn nearest_listener(actor: &TalkActor, pos: [i32; 3], world: &mut dyn TalkWorld) -> Option<usize> {
    let from = [pos[0], pos[1] + ACTOR_HULL_TOP, pos[2]];
    let mut nearest = None;
    let mut nearest_d2 = scientist_logic::TALK_RANGE_MIN * scientist_logic::TALK_RANGE_MIN;
    let n = world.roster_len();
    let mut slot = 0usize;
    while slot < n {
        if slot != actor.index {
            if let Some(to) = world.idle_listener_top(slot) {
                if scientist_logic::friend_in_talk_range(from, to) {
                    let dx = to[0] - from[0];
                    let dy = to[1] - from[1];
                    let dz = to[2] - from[2];
                    let d2 = dx * dx + dy * dy + dz * dz;
                    if d2 < nearest_d2 && world.line_clear(from, to) {
                        nearest = Some(slot);
                        nearest_d2 = d2;
                    }
                }
            }
        }
        slot += 1;
    }
    nearest
}

fn try_small_talk(
    actor: &mut TalkActor,
    clock: &mut TalkClock,
    cx: &SpeechContext,
    world: &mut dyn TalkWorld,
) -> Option<Line> {
    let now = cx.now;
    if !cx.voices_enabled
        || now & 1 != 0
        || !cx.player_in_pvs
        || !cx.player_alive
        || !actor.alive
        || !actor.predisaster
        || actor.provoked
        || actor.answer_pending
        || (!cx.scripted_moving
            && (!actor.hello_said || actor.state != TalkState::Idle || cx.script_busy))
        || !reached(now, actor.speech_due)
        || !clock.gap_over(now)
    {
        return None;
    }
    let pos = cx.actor_pos;
    let listener = nearest_listener(actor, pos, world);
    if listener.is_none() {
        // Without a listener the line is addressed to the player, which an
        // actor only does after greeting them.
        if !actor.hello_said {
            return None;
        }
        let player = cx.player_pos;
        let player_top = [player[0], player[1] + PLAYER_HULL_TOP, player[2]];
        let actor_top = [pos[0], pos[1] + ACTOR_HULL_TOP, pos[2]];
        if !scientist_logic::friend_in_talk_range(actor_top, player_top)
            || !world.line_clear(actor_top, player_top)
        {
            return None;
        }
    }
    let ordinal = world.scientist_ordinal();
    let kind = if listener.is_some() {
        LineKind::Question
    } else {
        LineKind::Idle
    };
    let voice = world.voice_id(kind);
    let ticks = if kind == LineKind::Question {
        scientist_logic::predisaster_question_duration_ticks(cx.map_index, ordinal)
    } else {
        scientist_logic::predisaster_idle_duration_ticks(cx.map_index, ordinal)
    };
    clock.begin(actor.index, voice | VOICE_AUTONOMOUS, now, ticks);
    actor.speech_due = now.wrapping_add(IDLE_LINE_REPEAT_TICKS);
    actor.hello_said = true;
    actor.client_push = false;
    Some(Line {
        kind,
        voice,
        ticks,
        listener,
    })
}

/// Apply a question to its listener: it owes an answer when the question ends.
#[inline]
pub fn hear_question(listener: &mut TalkActor, question_ends: u16) {
    listener.answer_pending = true;
    listener.speech_due = question_ends;
}

/// Result of a player walking into a living human actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Bump {
    /// Too slow to count as a push.
    Ignored,
    /// A provoked actor will not yield.
    BlockedProvoked,
    /// An actor mid-line will not yield.
    BlockedTalking,
    /// The push is recorded for the next think. `ideal_yaw` (toward the
    /// player) is `None` while a script owns the actor's facing.
    Pushed { ideal_yaw: Option<u16> },
}

/// The player walked into `actor` with planar velocity `player_vel_xz`
/// (units per tick) and now stands at `player_pos`.
pub fn player_bump(
    actor: &mut TalkActor,
    clock: &TalkClock,
    now: u16,
    player_vel_xz: [i32; 2],
    player_pos: [i32; 3],
    actor_pos: [i32; 3],
    script_busy: bool,
) -> Bump {
    if !scientist_logic::client_push_speed(player_vel_xz[0], player_vel_xz[1]) {
        return Bump::Ignored;
    }
    if actor.provoked {
        return Bump::BlockedProvoked;
    }
    if clock.is_talking(actor.index, now) {
        return Bump::BlockedTalking;
    }
    actor.client_push = true;
    let ideal_yaw = if script_busy {
        None
    } else {
        Some(scientist_logic::precise_yaw_from_vec(
            player_pos[0] - actor_pos[0],
            player_pos[2] - actor_pos[2],
        ))
    };
    Bump::Pushed { ideal_yaw }
}

/// On a 10 Hz think (`move_tick` even), turn a recorded push into a
/// step-aside walk 100 units directly away from the player. `ideal_yaw` is
/// the latest facing toward the player. Returns the goal when it starts.
pub fn try_begin_move_away(
    actor: &mut TalkActor,
    move_tick: u16,
    actor_pos: [i32; 3],
    ideal_yaw: u16,
) -> Option<[i32; 3]> {
    if !actor.client_push || move_tick & 1 != 0 {
        return None;
    }
    actor.client_push = false;
    let away_yaw = ideal_yaw.wrapping_add(2048) & 0x0fff;
    let goal = scientist_logic::move_away_goal(
        actor_pos,
        sincos::sin_q12(away_yaw),
        sincos::sin_q12((away_yaw + 1024) & 0x0fff),
    );
    actor.move_goal = [
        goal[0].clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        goal[1].clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        goal[2].clamp(i16::MIN as i32, i16::MAX as i32) as i16,
    ];
    actor.state = TalkState::MoveAway;
    actor.hold_ticks = MOVE_AWAY_START_HOLD_TICKS;
    actor.schedule_timer = MOVE_AWAY_TIMEOUT_TICKS;
    Some(goal)
}

/// One tick of an active step-aside.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoveAwayStep {
    /// No step-aside is running; ordinary AI proceeds.
    Inactive,
    /// Walk toward `goal` at `speed` units per tick.
    Walk { goal: [i32; 3], speed: i32 },
    /// Reached the goal or gave up; turning back starts next tick.
    Arrived { goal: [i32; 3] },
    /// Turn toward `look_at`. When `done`, ordinary AI resumes this tick.
    FaceBack { look_at: [i32; 3], done: bool },
}

/// Advance an active step-aside by one tick.
pub fn continue_move_away(
    actor: &mut TalkActor,
    actor_pos: [i32; 3],
    player_eye: [i32; 3],
) -> MoveAwayStep {
    match actor.state {
        TalkState::MoveAway => {
            if actor.schedule_timer != 0 {
                actor.schedule_timer -= 1;
            }
            let g = actor.move_goal;
            let goal = [g[0] as i32, g[1] as i32, g[2] as i32];
            let dx = goal[0] - actor_pos[0];
            let dz = goal[2] - actor_pos[2];
            if actor.schedule_timer == 0 || dx * dx + dz * dz <= MOVE_AWAY_ARRIVE_D2 {
                actor.state = TalkState::FaceBack;
                actor.hold_ticks = MOVE_AWAY_FACE_TICKS;
                actor.schedule_timer = 0;
                return MoveAwayStep::Arrived { goal };
            }
            MoveAwayStep::Walk {
                goal,
                speed: MOVE_AWAY_SPEED,
            }
        }
        TalkState::FaceBack => {
            if actor.hold_ticks != 0 {
                actor.hold_ticks -= 1;
            }
            let done = actor.hold_ticks == 0;
            if done {
                actor.state = TalkState::Idle;
            }
            MoveAwayStep::FaceBack {
                look_at: player_eye,
                done,
            }
        }
        _ => MoveAwayStep::Inactive,
    }
}

/// A script takes the actor over: forget the push and abandon a step-aside.
pub fn script_takes_over(actor: &mut TalkActor) {
    actor.client_push = false;
    if matches!(actor.state, TalkState::MoveAway | TalkState::FaceBack) {
        actor.state = TalkState::Idle;
        actor.schedule_timer = 0;
    }
}

/// True when +use should consider this actor at all: it is not mid-way
/// through an authored line or a reply.
#[inline]
pub fn use_eligible(actor: &TalkActor, clock: &TalkClock, now: u16) -> bool {
    !clock.directed_line_playing(actor.index, now)
}

/// How the actor reacts to +use.
#[inline]
pub fn use_reaction(
    actor: &TalkActor,
    script_busy: bool,
    script_nointerrupt: bool,
) -> scientist_logic::FollowUse {
    scientist_logic::follow_use(
        actor.alive,
        actor.following,
        actor.predisaster,
        actor.provoked,
        script_busy,
        script_nointerrupt,
    )
}

/// Spoken reply to +use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UseReply {
    Started = 0,
    Stopped = 1,
    Declined = 2,
}

/// Side effects of a +use the caller applies to the rest of the world, in
/// field order: cancel the script, trim the other followers, refresh the
/// actor's path, then speak the reply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UseEffects {
    /// Cancel the actor's interruptible script without firing its outputs.
    pub cancel_script: bool,
    /// Keep only the first other follower in roster order; see
    /// [`followers_to_keep`].
    pub limit_followers: bool,
    /// Drop the actor's cached route.
    pub refresh_path: bool,
    pub reply: Option<UseReply>,
}

/// Apply +use to the actor's own state.
pub fn apply_use(
    actor: &mut TalkActor,
    reaction: scientist_logic::FollowUse,
    script_busy: bool,
) -> UseEffects {
    use scientist_logic::FollowUse;
    match reaction {
        FollowUse::Start => {
            actor.following = true;
            actor.hello_said = true;
            actor.client_push = false;
            if matches!(actor.state, TalkState::MoveAway | TalkState::FaceBack) {
                actor.state = TalkState::Idle;
                actor.schedule_timer = 0;
                actor.hold_ticks = 0;
            }
            UseEffects {
                cancel_script: script_busy,
                limit_followers: true,
                refresh_path: false,
                reply: Some(UseReply::Started),
            }
        }
        FollowUse::Stop => {
            actor.following = false;
            UseEffects {
                cancel_script: false,
                limit_followers: false,
                refresh_path: true,
                reply: Some(UseReply::Stopped),
            }
        }
        FollowUse::Decline => UseEffects {
            cancel_script: false,
            limit_followers: false,
            refresh_path: false,
            reply: Some(UseReply::Declined),
        },
        FollowUse::Ignore => UseEffects {
            cancel_script: false,
            limit_followers: false,
            refresh_path: false,
            reply: None,
        },
    }
}

/// Existing followers allowed to stay when a new one starts: at most one,
/// so the player never has more than two.
pub const FOLLOWERS_KEPT: usize = 1;

/// Walk the roster in order and decide, for each other actor currently
/// following, whether it keeps following. Call with `already_kept` = the
/// number of earlier slots that answered true.
#[inline]
pub fn followers_to_keep(already_kept: usize) -> bool {
    already_kept < FOLLOWERS_KEPT
}

/// An authored sentence (or other directed line) started from `actor`.
/// `ticks` is the sample length; zero means nothing played.
pub fn authored_line(
    actor: &mut TalkActor,
    clock: &mut TalkClock,
    voice: u8,
    now: u16,
    ticks: u16,
    human: bool,
) {
    actor.client_push = false;
    if ticks != 0 {
        clock.begin(actor.index, voice, now, ticks);
        if human {
            actor.hello_said = true;
        }
    }
}

/// A +use reply started from `actor`; it holds the channel for at least a tick.
pub fn reply_line(actor: &mut TalkActor, clock: &mut TalkClock, voice: u8, now: u16, ticks: u16) {
    clock.begin(actor.index, voice, now, ticks.max(1));
    actor.client_push = false;
}
