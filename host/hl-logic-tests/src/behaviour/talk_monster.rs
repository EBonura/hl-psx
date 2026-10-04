//! Talk monsters (scientists and Barney): greeting the player, pre-disaster
//! small talk and answers, the shared dialogue channel, the player bump that
//! makes an actor step aside, and the +use follow toggle.
//!
//! Coordinates are Y-up world units. A standing actor's origin is on the
//! floor and its hull top 72 units above; the standing player's origin is the
//! hull centre, 36 units below its hull top. Time is the 20 Hz tick.
#![cfg(test)]

use crate::scientist_logic::{self, FollowUse};
use crate::talk_monster::*;

const MAP: u16 = 7;
/// Ticks on which an idle (not scripted) actor may try to greet.
const GREET_TICK: u16 = 12;

fn actor(index: usize) -> TalkActor {
    TalkActor {
        index,
        alive: true,
        state: TalkState::Idle,
        following: false,
        provoked: false,
        predisaster: true,
        hello_said: false,
        push_pending: false,
        answer_pending: false,
        speech_due: 0,
        schedule_timer: 0,
        hold_ticks: 0,
        move_goal: [0; 3],
    }
}

fn player_at(x: i32, z: i32) -> [i32; 3] {
    [x, 36, z]
}

fn context(now: u16, actor_pos: [i32; 3], actor_yaw: u16, player_pos: [i32; 3]) -> SpeechContext {
    SpeechContext {
        now,
        map_index: MAP,
        voices_enabled: true,
        player_in_pvs: true,
        player_alive: true,
        player_pos,
        player_eye: [player_pos[0], player_pos[1] + 28, player_pos[2]],
        actor_pos,
        actor_yaw,
        script_busy: false,
        scripted_moving: false,
    }
}

/// A scripted room. Voice ids are fake: kind base plus the speaker's slot.
struct View<'a> {
    me: usize,
    pos: &'a [[i32; 3]],
    others: &'a [TalkActor],
    sight: bool,
    traces: u32,
}

fn voice_for(kind: LineKind, me: usize) -> u8 {
    let base = match kind {
        LineKind::Hello => 0,
        LineKind::Answer => 8,
        LineKind::Question => 16,
        LineKind::Idle => 24,
    };
    base + me as u8
}

impl TalkWorld for View<'_> {
    fn line_clear(&mut self, _from: [i32; 3], _to: [i32; 3]) -> bool {
        self.traces += 1;
        self.sight
    }
    fn speaker_eye(&self) -> [i32; 3] {
        let p = self.pos[self.me];
        [p[0], p[1] + 64, p[2]]
    }
    fn roster_len(&self) -> usize {
        self.others.len()
    }
    fn idle_listener_top(&self, slot: usize) -> Option<[i32; 3]> {
        let a = &self.others[slot];
        if a.alive && a.predisaster && a.state == TalkState::Idle {
            let p = self.pos[slot];
            Some([p[0], p[1] + ACTOR_HULL_TOP, p[2]])
        } else {
            None
        }
    }
    fn scientist_ordinal(&mut self) -> usize {
        self.me
    }
    fn voice_id(&mut self, kind: LineKind) -> u8 {
        voice_for(kind, self.me)
    }
}

/// Run one actor's speech think in a room of `actors`.
fn think(
    actors: &mut [TalkActor],
    pos: &[[i32; 3]],
    clock: &mut TalkClock,
    me: usize,
    cx: &SpeechContext,
    sight: bool,
) -> Option<Line> {
    let mut a = actors[me];
    let snapshot: Vec<TalkActor> = actors.to_vec();
    let mut view = View {
        me,
        pos,
        others: &snapshot,
        sight,
        traces: 0,
    };
    let line = take_turn(&mut a, clock, cx, &mut view);
    actors[me] = a;
    if let Some(Line {
        listener: Some(l), ..
    }) = line
    {
        hear_question(&mut actors[l], clock.until);
    }
    line
}

fn lone_think(
    a: &mut TalkActor,
    clock: &mut TalkClock,
    cx: &SpeechContext,
    sight: bool,
) -> Option<Line> {
    let mut actors = [*a];
    let pos = [cx.actor_pos];
    let line = think(&mut actors, &pos, clock, 0, cx, sight);
    *a = actors[0];
    line
}

// ---------------------------------------------------------------------------
// Greeting
// ---------------------------------------------------------------------------

#[test]
fn greets_a_player_strictly_under_five_hundred_units_hull_top_to_hull_top() {
    for (z, greets) in [(499, true), (500, false)] {
        let mut a = actor(0);
        let mut clock = TalkClock::SILENT;
        let cx = context(GREET_TICK, [0, 0, 0], 0, player_at(0, z));
        let line = lone_think(&mut a, &mut clock, &cx, true);
        assert_eq!(line.is_some(), greets, "player {z} units ahead");
        if greets {
            assert_eq!(line.unwrap().kind, LineKind::Hello);
            assert!(a.hello_said);
        }
    }
    // The measure is hull top to hull top: a player standing 300 units
    // higher is out of range at 400 planar units but in range at 399.
    for (x, greets) in [(400, false), (399, true)] {
        let mut a = actor(0);
        let mut clock = TalkClock::SILENT;
        let cx = context(GREET_TICK, [0, 0, 0], 1024, [x, 336, 0]);
        assert_eq!(lone_think(&mut a, &mut clock, &cx, true).is_some(), greets);
    }
}

#[test]
fn greeting_needs_the_player_outside_the_rear_wedge_and_in_clear_sight() {
    // Facing +Z (yaw 0). Straight behind is the rear wedge; to the side is not.
    let mut a = actor(0);
    let mut clock = TalkClock::SILENT;
    let behind = context(GREET_TICK, [0, 0, 0], 0, player_at(0, -100));
    assert_eq!(lone_think(&mut a, &mut clock, &behind, true), None);
    let side = context(GREET_TICK, [0, 0, 0], 0, player_at(100, 0));
    let mut b = actor(0);
    assert!(lone_think(&mut b, &mut clock, &side, true).is_some());
    let mut c = actor(0);
    let mut clock = TalkClock::SILENT;
    let ahead = context(GREET_TICK, [0, 0, 0], 0, player_at(0, 100));
    assert_eq!(
        lone_think(&mut c, &mut clock, &ahead, false),
        None,
        "blocked sight"
    );
}

#[test]
fn greeting_happens_once_per_actor() {
    let mut a = actor(0);
    a.predisaster = false;
    let mut clock = TalkClock::SILENT;
    let cx = context(GREET_TICK, [0, 0, 0], 0, player_at(0, 100));
    assert!(lone_think(&mut a, &mut clock, &cx, true).is_some());
    let mut clock = TalkClock::SILENT;
    for now in GREET_TICK + 1..GREET_TICK + 400 {
        let cx = context(now, [0, 0, 0], 0, player_at(0, 100));
        assert_eq!(lone_think(&mut a, &mut clock, &cx, true), None);
    }
}

#[test]
fn an_idle_actor_tries_to_greet_every_sixteen_ticks_and_a_scripted_walker_every_other_tick() {
    let tried = |now: u16, scripted: bool| {
        let mut a = actor(0);
        a.predisaster = false;
        let mut clock = TalkClock::SILENT;
        let mut cx = context(now, [0, 0, 0], 0, player_at(0, 100));
        cx.scripted_moving = scripted;
        lone_think(&mut a, &mut clock, &cx, true).is_some()
    };
    let idle: Vec<u16> = (0..64).filter(|&t| tried(t, false)).collect();
    assert_eq!(idle, [12, 28, 44, 60]);
    let walking: Vec<u16> = (0..8).filter(|&t| tried(t, true)).collect();
    assert_eq!(walking, [0, 2, 4, 6]);
}

#[test]
fn greeting_is_blocked_by_provocation_following_scripts_and_activity() {
    let cx = context(GREET_TICK, [0, 0, 0], 0, player_at(0, 100));
    let mut cases: Vec<(&str, TalkActor, SpeechContext)> = Vec::new();
    let mut a = actor(0);
    a.provoked = true;
    cases.push(("provoked", a, cx));
    let mut a = actor(0);
    a.following = true;
    cases.push(("following", a, cx));
    let mut a = actor(0);
    a.state = TalkState::Busy;
    cases.push(("busy", a, cx));
    let mut a = actor(0);
    a.state = TalkState::MoveAway;
    cases.push(("stepping aside", a, cx));
    let mut busy = cx;
    busy.script_busy = true;
    cases.push(("script owned", actor(0), busy));
    let mut dead_player = cx;
    dead_player.player_alive = false;
    cases.push(("player dead", actor(0), dead_player));
    let mut away = cx;
    away.player_in_pvs = false;
    cases.push(("player not in view set", actor(0), away));
    let mut mute = cx;
    mute.voices_enabled = false;
    cases.push(("map has no voices", actor(0), mute));
    for (name, mut a, cx) in cases {
        let mut clock = TalkClock::SILENT;
        assert_eq!(lone_think(&mut a, &mut clock, &cx, true), None, "{name}");
    }
}

// ---------------------------------------------------------------------------
// The shared dialogue channel
// ---------------------------------------------------------------------------

#[test]
fn nobody_starts_a_line_until_two_seconds_after_the_previous_line_ends() {
    assert_eq!(CONVERSATION_GAP_TICKS, 40);
    let now = GREET_TICK + 160;
    for (ended_ago, greets) in [(39u16, false), (40, true)] {
        let mut a = actor(0);
        let mut clock = TalkClock {
            speaker: 5,
            voice: 3,
            until: now - ended_ago,
        };
        let cx = context(now, [0, 0, 0], 0, player_at(0, 100));
        assert_eq!(lone_think(&mut a, &mut clock, &cx, true).is_some(), greets);
    }
    assert!(TalkClock::SILENT.gap_over(0));
}

#[test]
fn a_started_line_holds_the_channel_for_its_duration() {
    let mut a = actor(3);
    a.predisaster = false;
    let mut clock = TalkClock::SILENT;
    let cx = context(GREET_TICK, [0, 0, 0], 0, player_at(0, 100));
    let mut actors = [actor(0), actor(1), actor(2), a];
    let pos = [[0; 3]; 4];
    let line = think(&mut actors, &pos, &mut clock, 3, &cx, true).unwrap();
    assert_eq!(clock.speaker, 3);
    assert_eq!(clock.voice, line.voice | VOICE_AUTONOMOUS);
    assert_eq!(clock.until, GREET_TICK + line.ticks);
    assert!(clock.is_talking(3, GREET_TICK + line.ticks - 1));
    assert!(!clock.is_talking(3, GREET_TICK + line.ticks));
    assert!(!clock.is_talking(2, GREET_TICK));
}

// ---------------------------------------------------------------------------
// Small talk and answers
// ---------------------------------------------------------------------------

#[test]
fn a_pre_disaster_scientist_asks_a_nearby_idle_scientist_who_answers_when_the_question_ends() {
    let pos = [[0, 0, 0], [200, 0, 0]];
    let mut actors = [actor(0), actor(1)];
    actors[0].hello_said = true;
    let mut clock = TalkClock::SILENT;
    let player = player_at(0, 2000);
    let q = think(
        &mut actors,
        &pos,
        &mut clock,
        0,
        &context(2, pos[0], 0, player),
        true,
    )
    .unwrap();
    assert_eq!(q.kind, LineKind::Question);
    assert_eq!(q.listener, Some(1));
    assert!(actors[1].answer_pending);
    let ends = 2 + q.ticks;
    // Not before the question ends ...
    for now in (4..ends).step_by(2) {
        assert_eq!(
            think(
                &mut actors,
                &pos,
                &mut clock,
                1,
                &context(now, pos[1], 0, player),
                true
            ),
            None
        );
    }
    // ... and right when it does, ignoring the conversation gap.
    let a = think(
        &mut actors,
        &pos,
        &mut clock,
        1,
        &context(ends, pos[1], 0, player),
        true,
    )
    .unwrap();
    assert_eq!(a.kind, LineKind::Answer);
    assert!(!actors[1].answer_pending);
    assert!(actors[1].hello_said);
}

#[test]
fn an_answer_that_cannot_start_within_two_seconds_is_dropped() {
    let pos = [[0, 0, 0]];
    for (late, answers) in [(40u16, true), (42, false)] {
        let mut actors = [actor(0)];
        actors[0].answer_pending = true;
        actors[0].speech_due = 100;
        actors[0].hello_said = true;
        let mut clock = TalkClock::SILENT;
        let cx = context(100 + late, pos[0], 0, player_at(0, 5000));
        let line = think(&mut actors, &pos, &mut clock, 0, &cx, true);
        assert_eq!(
            line.map(|l| l.kind),
            answers.then_some(LineKind::Answer),
            "late {late}"
        );
        assert!(!actors[0].answer_pending);
    }
}

#[test]
fn without_a_listener_an_idle_line_goes_to_a_greeted_visible_player_in_range() {
    let pos = [[0, 0, 0]];
    let near = player_at(0, 300);
    // Not yet greeted: no idle line (and on an odd-phase tick, no greeting).
    let mut actors = [actor(0)];
    let mut clock = TalkClock::SILENT;
    assert_eq!(
        think(
            &mut actors,
            &pos,
            &mut clock,
            0,
            &context(2, pos[0], 0, near),
            true
        ),
        None
    );
    // Greeted, in range, visible.
    actors[0].hello_said = true;
    let line = think(
        &mut actors,
        &pos,
        &mut clock,
        0,
        &context(2, pos[0], 0, near),
        true,
    )
    .unwrap();
    assert_eq!(line.kind, LineKind::Idle);
    assert_eq!(line.listener, None);
    // Out of range or hidden: nothing.
    let mut actors = [actor(0)];
    actors[0].hello_said = true;
    let mut clock = TalkClock::SILENT;
    assert_eq!(
        think(
            &mut actors,
            &pos,
            &mut clock,
            0,
            &context(2, pos[0], 0, player_at(0, 600)),
            true
        ),
        None
    );
    assert_eq!(
        think(
            &mut actors,
            &pos,
            &mut clock,
            0,
            &context(2, pos[0], 0, near),
            false
        ),
        None
    );
}

#[test]
fn the_same_actor_waits_sixty_seconds_between_idle_lines() {
    assert_eq!(IDLE_LINE_REPEAT_TICKS, 1_200);
    let pos = [[0, 0, 0]];
    let near = player_at(0, 300);
    let mut actors = [actor(0)];
    actors[0].hello_said = true;
    let mut clock = TalkClock::SILENT;
    let mut spoke = Vec::new();
    for now in (0..3_000u16).step_by(2) {
        if think(
            &mut actors,
            &pos,
            &mut clock,
            0,
            &context(now, pos[0], 0, near),
            true,
        )
        .is_some()
        {
            spoke.push(now);
        }
    }
    assert_eq!(spoke, [0, 1_200, 2_400]);
}

#[test]
fn small_talk_only_on_even_ticks_and_only_for_calm_pre_disaster_scientists() {
    let pos = [[0, 0, 0]];
    let near = player_at(0, 300);
    let speaks = |mutate: &dyn Fn(&mut TalkActor), now: u16| {
        let mut actors = [actor(0)];
        actors[0].hello_said = true;
        mutate(&mut actors[0]);
        let mut clock = TalkClock::SILENT;
        think(
            &mut actors,
            &pos,
            &mut clock,
            0,
            &context(now, pos[0], 0, near),
            true,
        )
        .is_some()
    };
    assert!(speaks(&|_| {}, 2));
    assert!(!speaks(&|_| {}, 3));
    assert!(!speaks(&|a| a.predisaster = false, 2));
    assert!(!speaks(&|a| a.provoked = true, 2));
    assert!(!speaks(&|a| a.alive = false, 2));
    assert!(!speaks(&|a| a.state = TalkState::Busy, 2));
}

// ---------------------------------------------------------------------------
// Player bump and step-aside
// ---------------------------------------------------------------------------

#[test]
fn a_bump_counts_only_above_fifty_units_per_second() {
    // Velocities are units per 20 Hz tick; |vx| + |vz| must reach 3.
    for (vel, pushes) in [
        ([2, 0], false),
        ([1, 1], false),
        ([3, 0], true),
        ([-2, 1], true),
    ] {
        let mut a = actor(0);
        let bump = player_bump(
            &mut a,
            &TalkClock::SILENT,
            0,
            vel,
            player_at(0, -20),
            [0; 3],
            false,
        );
        assert_eq!(matches!(bump, Bump::Pushed { .. }), pushes, "{vel:?}");
        assert_eq!(a.push_pending, pushes);
    }
}

#[test]
fn provoked_or_talking_actors_do_not_yield_and_scripts_keep_their_facing() {
    let mut a = actor(0);
    a.provoked = true;
    assert_eq!(
        player_bump(
            &mut a,
            &TalkClock::SILENT,
            0,
            [4, 0],
            player_at(0, -20),
            [0; 3],
            false
        ),
        Bump::BlockedProvoked
    );
    assert!(!a.push_pending);
    let mut a = actor(0);
    let talking = TalkClock {
        speaker: 0,
        voice: 1,
        until: 10,
    };
    assert_eq!(
        player_bump(
            &mut a,
            &talking,
            9,
            [4, 0],
            player_at(0, -20),
            [0; 3],
            false
        ),
        Bump::BlockedTalking
    );
    assert!(matches!(
        player_bump(
            &mut a,
            &talking,
            10,
            [4, 0],
            player_at(0, -20),
            [0; 3],
            false
        ),
        Bump::Pushed { .. }
    ));
    let mut a = actor(0);
    assert_eq!(
        player_bump(
            &mut a,
            &TalkClock::SILENT,
            0,
            [4, 0],
            player_at(0, -20),
            [0; 3],
            true
        ),
        Bump::Pushed { ideal_yaw: None }
    );
    assert!(a.push_pending);
}

#[test]
fn the_step_aside_goal_is_one_hundred_units_directly_away_from_the_player() {
    let mut a = actor(0);
    let Bump::Pushed {
        ideal_yaw: Some(yaw),
    } = player_bump(
        &mut a,
        &TalkClock::SILENT,
        0,
        [0, 4],
        player_at(10, -20),
        [10, 0, 30],
        false,
    )
    else {
        panic!("expected a push");
    };
    // Odd think: nothing yet; the push waits.
    assert_eq!(try_begin_move_away(&mut a, 1, [10, 0, 30], yaw), None);
    assert!(a.push_pending);
    let goal = try_begin_move_away(&mut a, 2, [10, 0, 30], yaw).unwrap();
    assert!(!a.push_pending);
    assert_eq!(a.state, TalkState::MoveAway);
    assert_eq!(a.schedule_timer, MOVE_AWAY_TIMEOUT_TICKS);
    assert_eq!(goal, [10, 0, 130], "player straight behind at -Z");
    assert_eq!(a.move_goal, [10, 0, 130]);

    // Off-axis pushes land on the same 100-unit circle, within rounding.
    for player in [player_at(-40, 10), player_at(25, 80), player_at(-7, -90)] {
        let mut a = actor(0);
        let Bump::Pushed {
            ideal_yaw: Some(yaw),
        } = player_bump(&mut a, &TalkClock::SILENT, 0, [4, 0], player, [0; 3], false)
        else {
            panic!("expected a push");
        };
        let goal = try_begin_move_away(&mut a, 0, [0; 3], yaw).unwrap();
        let len2 = goal[0] * goal[0] + goal[2] * goal[2];
        assert!((98 * 98..=101 * 101).contains(&len2), "{goal:?}");
        // Pointing away: the goal and the player are on opposite sides.
        assert!(goal[0] * player[0] + goal[2] * player[2] < 0, "{goal:?}");
    }
}

#[test]
fn the_step_aside_ends_on_arrival_within_three_units_or_after_four_seconds() {
    // Arrival.
    let mut a = actor(0);
    a.push_pending = true;
    try_begin_move_away(&mut a, 0, [0, 0, 0], 2048).unwrap();
    let goal = [a.move_goal[0] as i32, 0, a.move_goal[2] as i32];
    assert_eq!(
        continue_move_away(&mut a, [goal[0], 0, goal[2] - 4], [0; 3]),
        MoveAwayStep::Walk { goal, speed: 3 }
    );
    assert_eq!(
        continue_move_away(&mut a, [goal[0], 0, goal[2] - 3], [0; 3]),
        MoveAwayStep::Arrived { goal }
    );
    // Timeout: never moving, it gives up on the 80th tick.
    let mut a = actor(0);
    a.push_pending = true;
    try_begin_move_away(&mut a, 0, [0, 0, 0], 2048).unwrap();
    let mut gave_up = None;
    for tick in 1..=100 {
        if let MoveAwayStep::Arrived { .. } = continue_move_away(&mut a, [0; 3], [0; 3]) {
            gave_up = Some(tick);
            break;
        }
    }
    assert_eq!(gave_up, Some(80));
    assert_eq!(a.state, TalkState::FaceBack);
}

#[test]
fn after_stepping_aside_the_actor_faces_the_player_for_three_quarters_of_a_second() {
    let mut a = actor(0);
    a.state = TalkState::FaceBack;
    a.hold_ticks = MOVE_AWAY_FACE_TICKS;
    let eye = [5, 64, 5];
    let mut steps = Vec::new();
    loop {
        let step = continue_move_away(&mut a, [0; 3], eye);
        steps.push(step);
        if step
            != (MoveAwayStep::FaceBack {
                look_at: eye,
                done: false,
            })
        {
            break;
        }
    }
    assert_eq!(steps.len(), 15);
    assert_eq!(
        *steps.last().unwrap(),
        MoveAwayStep::FaceBack {
            look_at: eye,
            done: true
        }
    );
    assert_eq!(a.state, TalkState::Idle);
    assert_eq!(
        continue_move_away(&mut a, [0; 3], eye),
        MoveAwayStep::Inactive
    );
}

#[test]
fn a_script_taking_the_actor_forgets_the_push_and_any_step_aside() {
    let mut a = actor(0);
    a.push_pending = true;
    a.state = TalkState::MoveAway;
    a.schedule_timer = 30;
    script_takes_over(&mut a);
    assert!(!a.push_pending);
    assert_eq!(a.state, TalkState::Idle);
    assert_eq!(a.schedule_timer, 0);
    let mut b = actor(0);
    b.state = TalkState::Busy;
    b.schedule_timer = 30;
    script_takes_over(&mut b);
    assert_eq!((b.state, b.schedule_timer), (TalkState::Busy, 30));
}

// ---------------------------------------------------------------------------
// +use follow toggle
// ---------------------------------------------------------------------------

#[test]
fn use_toggles_following_and_pre_disaster_scientists_decline() {
    let mut a = actor(0);
    a.predisaster = false;
    let r = use_reaction(&a, false, false);
    assert_eq!(r, FollowUse::Start);
    let fx = apply_use(&mut a, r, false);
    assert!(a.following && a.hello_said);
    assert_eq!(fx.reply, Some(UseReply::Started));
    assert!(fx.limit_followers);
    let r = use_reaction(&a, false, false);
    assert_eq!(r, FollowUse::Stop);
    let fx = apply_use(&mut a, r, false);
    assert!(!a.following);
    assert_eq!(fx.reply, Some(UseReply::Stopped));
    assert!(fx.refresh_path);

    let mut p = actor(1);
    let r = use_reaction(&p, false, false);
    assert_eq!(r, FollowUse::Decline);
    assert_eq!(apply_use(&mut p, r, false).reply, Some(UseReply::Declined));
    assert!(!p.following);
}

#[test]
fn provoked_dead_or_uninterruptible_actors_ignore_use_but_an_interruptible_script_is_cancelled() {
    let mut a = actor(0);
    a.predisaster = false;
    a.provoked = true;
    assert_eq!(use_reaction(&a, false, false), FollowUse::Ignore);
    a.provoked = false;
    a.alive = false;
    assert_eq!(use_reaction(&a, false, false), FollowUse::Ignore);
    a.alive = true;
    assert_eq!(use_reaction(&a, true, true), FollowUse::Ignore);
    let r = use_reaction(&a, true, false);
    assert_eq!(r, FollowUse::Start);
    assert!(apply_use(&mut a, r, true).cancel_script);
    // A follower can always be dismissed, whatever else is true.
    let mut f = actor(0);
    f.following = true;
    f.provoked = true;
    assert_eq!(use_reaction(&f, true, true), FollowUse::Stop);
}

#[test]
fn starting_to_follow_cancels_a_step_aside() {
    let mut a = actor(0);
    a.predisaster = false;
    a.state = TalkState::FaceBack;
    a.hold_ticks = 9;
    a.schedule_timer = 4;
    a.push_pending = true;
    apply_use(&mut a, FollowUse::Start, false);
    assert_eq!(
        (a.state, a.hold_ticks, a.schedule_timer, a.push_pending),
        (TalkState::Idle, 0, 0, false)
    );
}

/// Apply the follower limit the way the game does: walk the roster in order,
/// skipping the newcomer.
fn limit(following: &mut [bool], newcomer: usize) {
    let mut kept = 0;
    for (i, f) in following.iter_mut().enumerate() {
        if i != newcomer && *f {
            if followers_to_keep(kept) {
                kept += 1;
            } else {
                *f = false;
            }
        }
    }
    following[newcomer] = true;
}

#[test]
fn the_player_never_has_more_than_two_followers() {
    let mut following = [false; 5];
    for newcomer in [3, 1, 4, 0] {
        limit(&mut following, newcomer);
        assert!(following.iter().filter(|&&f| f).count() <= 2);
        assert!(following[newcomer]);
    }
    // The earliest other follower in roster order is the one that stays.
    assert_eq!(following, [true, true, false, false, false]);
}

#[test]
fn a_directed_line_blocks_use_until_it_ends_but_autonomous_lines_do_not() {
    let mut a = actor(2);
    let mut clock = TalkClock::SILENT;
    authored_line(&mut a, &mut clock, 9, 100, 30, true);
    assert!(a.hello_said, "any spoken line counts as having greeted");
    assert!(!use_eligible(&a, &clock, 129));
    assert!(use_eligible(&a, &clock, 130));
    let auto = TalkClock {
        speaker: 2,
        voice: 9 | VOICE_AUTONOMOUS,
        until: 130,
    };
    assert!(use_eligible(&a, &auto, 100));
}

#[test]
fn authored_and_reply_lines_clear_the_push_and_hold_the_channel() {
    let mut a = actor(1);
    a.push_pending = true;
    let mut clock = TalkClock::SILENT;
    authored_line(&mut a, &mut clock, 7, 50, 0, true);
    assert!(
        !a.push_pending,
        "the push is forgotten even when nothing plays"
    );
    assert_eq!(clock, TalkClock::SILENT);
    assert!(!a.hello_said);
    let mut barney_like = actor(1);
    authored_line(&mut barney_like, &mut clock, 7, 50, 20, false);
    assert!(
        !barney_like.hello_said,
        "non-human speakers do not record a greeting"
    );
    assert_eq!((clock.speaker, clock.voice, clock.until), (1, 7, 70));

    let mut r = actor(4);
    r.push_pending = true;
    let mut clock = TalkClock::SILENT;
    reply_line(&mut r, &mut clock, 3, 200, 0);
    assert_eq!((clock.speaker, clock.voice, clock.until), (4, 3, 201));
    assert!(!r.push_pending);
}

// ---------------------------------------------------------------------------
// Cooked-voice contract shared with the content cooker
// ---------------------------------------------------------------------------

#[test]
fn greeting_and_small_talk_voice_choices_and_lengths_match_the_reference_run() {
    // Map 7 (the lobby): the first two pre-disaster scientists.
    assert_eq!(scientist_logic::hello_variant(7, 0, true), 5);
    assert_eq!(scientist_logic::hello_variant(7, 1, true), 0);
    assert_eq!(scientist_logic::hello_duration_ticks(7, 0), 61);
    assert_eq!(scientist_logic::hello_duration_ticks(7, 1), 64);
    assert_eq!(scientist_logic::predisaster_question_variant(7, 0), 8);
    assert_eq!(
        scientist_logic::predisaster_question_duration_ticks(7, 0),
        96
    );
    assert_eq!(scientist_logic::scientist_answer_variant(7, 1), 4);
    assert_eq!(scientist_logic::scientist_answer_duration_ticks(7, 1), 62);
    // Map 6: the observation-room question.
    assert_eq!(scientist_logic::predisaster_question_variant(6, 2), 14);
    assert_eq!(
        scientist_logic::predisaster_question_duration_ticks(6, 2),
        102
    );
    // Map 10: three idle statements.
    assert_eq!(scientist_logic::predisaster_idle_variant(10, 4), 5);
    assert_eq!(scientist_logic::predisaster_idle_duration_ticks(10, 4), 97);
    // Pre-disaster idle lines last 4.8 to 5.2 s; greetings 3.0 to 3.5 s.
    for map in 0..40u16 {
        for ordinal in 0..16 {
            let idle = scientist_logic::predisaster_idle_duration_ticks(map, ordinal);
            assert!((96..=104).contains(&idle));
            let hello = scientist_logic::hello_duration_ticks(map, ordinal);
            assert!((60..=70).contains(&hello));
        }
    }
}

// ---------------------------------------------------------------------------
// Golden scenarios
// ---------------------------------------------------------------------------

type LineLog = (u16, usize, LineKind, u8, u16, Option<usize>);

/// Recorded from ac83da7 behaviour. A lobby with two pre-disaster scientists
/// 200 units apart, both facing +Z, and a player walking toward them from
/// 900 units out at 5 units per tick until 300 units away. Every tick each
/// scientist thinks in roster order; sight is always clear.
#[test]
fn golden_lobby_greetings_question_and_answer() {
    let pos = [[0, 0, 0], [200, 0, 0]];
    let mut actors = [actor(0), actor(1)];
    let mut clock = TalkClock::SILENT;
    let mut log: Vec<LineLog> = Vec::new();
    for now in 0..900u16 {
        let z = (900 - 5 * now as i32).max(300);
        let player = player_at(100, z);
        for me in 0..2 {
            let cx = context(now, pos[me], 0, player);
            if let Some(l) = think(&mut actors, &pos, &mut clock, me, &cx, true) {
                log.push((now, me, l.kind, l.voice, l.ticks, l.listener));
            }
        }
    }
    let expected: &[LineLog] = &[
        (92, 0, LineKind::Hello, 0, 61, None),
        (194, 0, LineKind::Question, 16, 96, Some(1)),
        (290, 1, LineKind::Answer, 9, 62, None),
        (400, 1, LineKind::Question, 17, 97, Some(0)),
        (498, 0, LineKind::Answer, 8, 61, None),
        (608, 0, LineKind::Question, 16, 96, Some(1)),
        (704, 1, LineKind::Answer, 9, 62, None),
        (814, 1, LineKind::Question, 17, 97, Some(0)),
    ];
    assert_eq!(log, expected, "{log:#?}");
}

/// Recorded from ac83da7 behaviour. A player walking at about 4 units per
/// tick bumps a scientist at the origin from the lower-left. Each later tick
/// the scientist first continues any step-aside, then (when none is running)
/// tries to begin one; a test mover walks it straight toward its goal at the
/// requested speed. The log is run-length: (first tick, last tick, step,
/// position after the last tick).
#[test]
fn golden_bump_step_aside_and_face_back() {
    let mut a = actor(0);
    let mut pos = [0, 0, 0];
    let player = player_at(-33, -15);
    let eye = [player[0], player[1] + 28, player[2]];
    let bump = player_bump(&mut a, &TalkClock::SILENT, 0, [3, 1], player, pos, false);
    let Bump::Pushed {
        ideal_yaw: Some(yaw),
    } = bump
    else {
        panic!("{bump:?}");
    };
    assert_eq!(yaw, 2795);
    let mut log: Vec<(u16, u16, String, [i32; 3])> = Vec::new();
    for tick in 1..=120u16 {
        let step = continue_move_away(&mut a, pos, eye);
        let entry = match step {
            MoveAwayStep::Inactive => match try_begin_move_away(&mut a, tick, pos, yaw) {
                Some(goal) => format!("begin {goal:?}"),
                None => "waiting".to_string(),
            },
            MoveAwayStep::Walk { goal, speed } => {
                let (dx, dz) = (goal[0] - pos[0], goal[2] - pos[2]);
                let len = ((dx * dx + dz * dz) as f64).sqrt() as i32;
                if len <= speed {
                    pos = goal;
                } else {
                    pos[0] += dx * speed / len;
                    pos[2] += dz * speed / len;
                }
                format!("{step:?}")
            }
            _ => format!("{step:?}"),
        };
        match log.last_mut() {
            Some(last) if last.2 == entry => {
                last.1 = tick;
                last.3 = pos;
            }
            _ => log.push((tick, tick, entry, pos)),
        }
        if let MoveAwayStep::FaceBack { done: true, .. } = step {
            break;
        }
    }
    let expected: &[(u16, u16, &str, [i32; 3])] = &[
        (1, 1, "waiting", [0, 0, 0]),
        (2, 2, "begin [91, 0, 41]", [0, 0, 0]),
        (3, 45, "Walk { goal: [91, 0, 41], speed: 3 }", [91, 0, 41]),
        (46, 46, "Arrived { goal: [91, 0, 41] }", [91, 0, 41]),
        (
            47,
            60,
            "FaceBack { look_at: [-33, 64, -15], done: false }",
            [91, 0, 41],
        ),
        (
            61,
            61,
            "FaceBack { look_at: [-33, 64, -15], done: true }",
            [91, 0, 41],
        ),
    ];
    let got: Vec<(u16, u16, &str, [i32; 3])> = log
        .iter()
        .map(|(a, b, s, p)| (*a, *b, s.as_str(), *p))
        .collect();
    assert_eq!(got, expected, "{log:#?}");
}

/// Recorded from ac83da7 behaviour. Four post-disaster scientists; the player
/// uses 2, 0, 3, 0, 1 in turn. Logged: the reaction, the effects, and every
/// actor's follow flag afterwards.
#[test]
fn golden_follow_toggles_and_follower_limit() {
    let mut actors: Vec<TalkActor> = (0..4)
        .map(|i| {
            let mut a = actor(i);
            a.predisaster = false;
            a
        })
        .collect();
    let mut log = Vec::new();
    for who in [2usize, 0, 3, 0, 1] {
        let r = use_reaction(&actors[who], false, false);
        let fx = apply_use(&mut actors[who], r, false);
        if fx.limit_followers {
            let mut kept = 0;
            for i in 0..actors.len() {
                if i != who && actors[i].following {
                    if followers_to_keep(kept) {
                        kept += 1;
                    } else {
                        actors[i].following = false;
                    }
                }
            }
        }
        let flags: Vec<bool> = actors.iter().map(|a| a.following).collect();
        log.push((who, r, fx.reply, flags));
    }
    let started = Some(UseReply::Started);
    let expected: &[(usize, FollowUse, Option<UseReply>, Vec<bool>)] = &[
        (
            2,
            FollowUse::Start,
            started,
            vec![false, false, true, false],
        ),
        (0, FollowUse::Start, started, vec![true, false, true, false]),
        (3, FollowUse::Start, started, vec![true, false, false, true]),
        (
            0,
            FollowUse::Stop,
            Some(UseReply::Stopped),
            vec![false, false, false, true],
        ),
        (1, FollowUse::Start, started, vec![false, true, false, true]),
    ];
    assert_eq!(log, expected, "{log:#?}");
}
