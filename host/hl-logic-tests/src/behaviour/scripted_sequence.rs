//! scripted_sequence lifecycle: claiming an actor by name or class, the busy
//! retry, walk/run/instant/in-place moves, planting on the mark, turning to
//! the authored yaw, playing, firing targets, repeatable and idle holds,
//! damage/death cancel, and where a play animation leaves its actor.
//!
//! The test world records every action the step asks for. Its mover mirrors
//! the game's half-rate walker: it acts only on the actor's own think ticks,
//! spends any movement hold first, then steps twice the requested per-tick
//! speed straight at the goal. Coordinates are Y-up world units.
#![cfg(test)]

use crate::scripted_sequence::*;

const SCIENTIST: ActorKind = ActorKind {
    scientist: true,
    barney: false,
    houndeye: false,
};
const BARNEY: ActorKind = ActorKind {
    scientist: false,
    barney: true,
    houndeye: false,
};

fn record(move_mode: u16, origin: [i32; 3], yaw: u16) -> ScriptRecord {
    ScriptRecord {
        actor_name: 7,
        class_kind: None,
        radius: 0,
        move_mode,
        origin,
        yaw,
        repeatable: false,
        no_interrupt: false,
        has_idle: false,
        has_play: true,
        targeted: true,
        play_clip: 3,
        idle_clip: NO_CLIP,
        root_offset: None,
    }
}

fn idle_actor(pos: [i32; 3]) -> ScriptActor {
    ScriptActor {
        pos,
        alive: true,
        activity: Activity::Idle,
        yaw: 0,
        mode: ScriptMode::NONE,
        script: None,
        goal: [0; 3],
        target_yaw: 0,
        residue: [0, 0],
        deadline: 0,
        play_clip: NO_CLIP,
        idle_clip: NO_CLIP,
        hold: 0,
        no_interrupt: false,
    }
}

struct Stage {
    records: Vec<ScriptRecord>,
    plan: RoutePlan,
    play_hold: u16,
    floor: Option<i32>,
    /// The mover refuses to move (a wedged actor).
    stuck: bool,
    slot: u16,
    move_tick: u16,
    now: u16,
    log: Vec<(u16, String)>,
    /// Scripts that a fired target assigns to the same actor (a chain).
    chain: Option<(usize, usize)>,
}

impl Stage {
    fn new(records: Vec<ScriptRecord>) -> Self {
        Self {
            records,
            plan: RoutePlan::DIRECT,
            play_hold: 30,
            floor: None,
            stuck: false,
            slot: 0,
            move_tick: 0,
            now: 0,
            log: Vec::new(),
            chain: None,
        }
    }
    fn note(&mut self, s: String) {
        let now = self.now;
        self.log.push((now, s));
    }
}

impl ScriptWorld for Stage {
    fn record(&self, script: usize) -> Option<ScriptRecord> {
        self.records.get(script).copied()
    }
    fn play_hold_ticks(&mut self, _actor: &ScriptActor) -> u16 {
        self.play_hold
    }
    fn talk_turn(&mut self, _actor: &mut ScriptActor) {
        self.note("talk".into());
    }
    fn local_replan(&mut self, _actor: &mut ScriptActor, waypoint: [i32; 3], _now: u16) {
        self.note(format!("lookahead {waypoint:?}"));
    }
    fn plan_route(&mut self, actor: &ScriptActor, goal: [i32; 3], _now: u16) -> RoutePlan {
        self.note(format!("plan {:?} -> {goal:?}", actor.pos));
        self.plan
    }
    fn move_toward(&mut self, actor: &mut ScriptActor, goal: [i32; 3], speed: i32) {
        if (self.slot ^ self.move_tick) & 1 != 0 {
            return;
        }
        if actor.hold > 0 {
            actor.hold -= 1;
            return;
        }
        if self.stuck {
            return;
        }
        let step = speed * 2;
        let (dx, dz) = (goal[0] - actor.pos[0], goal[2] - actor.pos[2]);
        let len = ((dx * dx + dz * dz) as f64).sqrt() as i32;
        if len <= step {
            actor.pos = [goal[0], actor.pos[1], goal[2]];
        } else {
            actor.pos[0] += dx * step / len;
            actor.pos[2] += dz * step / len;
        }
    }
    fn place(&mut self, actor: &mut ScriptActor, pos: [i32; 3]) {
        actor.pos = pos;
        self.note(format!("place {pos:?}"));
    }
    fn forget_route(&mut self) {}
    fn floor_below(&mut self, _actor: &ScriptActor, probe: [i32; 3], depth: i32) -> Option<i32> {
        self.note(format!("floor probe {probe:?} depth {depth}"));
        self.floor
    }
    fn studio_events(&mut self, _actor: &mut ScriptActor, idle: bool) {
        if idle {
            self.note("idle events".into());
        }
    }
    fn take_over_talker(&mut self, _actor: &mut ScriptActor) {
        self.note("talker taken".into());
    }
    fn script_started(&mut self, script: usize) {
        self.note(format!("started {script}"));
    }
    fn fire_targets(&mut self, actor: &mut ScriptActor, script: usize) {
        self.note(format!("fire {script}"));
        if let Some((from, next)) = self.chain {
            if from == script {
                let rec = self.records[next];
                let now = self.now;
                assign(actor, next, &rec, SCIENTIST, now, self);
            }
        }
    }
    fn remove_script(&mut self, _actor: &mut ScriptActor, script: usize) {
        self.note(format!("remove {script}"));
    }
    fn trace(&mut self, _a: i32, _b: i32, _code: u8) {}
}

/// Run ticks `from..=to`; returns run-length rows of (first tick, last tick,
/// state summary, position after the last tick) and stops early when the
/// script lets go of the actor.
fn run(
    actor: &mut ScriptActor,
    stage: &mut Stage,
    kind: ActorKind,
    from: u16,
    to: u16,
) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for now in from..=to {
        stage.now = now;
        stage.move_tick = now;
        let primed_hold = actor.mode.primed && primed_holds(actor, kind, now, stage);
        let cx = TickContext {
            now,
            move_tick: now,
            slot: stage.slot,
            kind,
            primed_hold,
        };
        let owned = tick(actor, &cx, stage);
        let s = format!(
            "owned={owned} mode={} detour={} goal={:?} act={:?} yaw={} hold={}",
            actor.mode.base, actor.mode.detour, actor.goal, actor.activity, actor.yaw, actor.hold
        );
        match rows.last_mut() {
            Some(r) if r.2 == s => {
                r.1 = now;
                r.3 = actor.pos;
            }
            _ => rows.push((now, now, s, actor.pos)),
        }
        if !owned {
            break;
        }
    }
    rows
}

type Row = (u16, u16, String, [i32; 3]);

fn rows_ref(rows: &[Row]) -> Vec<(u16, u16, &str, [i32; 3])> {
    rows.iter()
        .map(|(a, b, s, p)| (*a, *b, s.as_str(), *p))
        .collect()
}

fn log_ref(stage: &Stage) -> Vec<(u16, &str)> {
    stage.log.iter().map(|(t, s)| (*t, s.as_str())).collect()
}

// ---------------------------------------------------------------------------
// Claiming an actor
// ---------------------------------------------------------------------------

struct Crowd(Vec<Candidate>);

impl Roster for Crowd {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn candidate(&self, slot: usize) -> Candidate {
        self.0[slot]
    }
}

fn candidate(name: u16, kind: u8, pos: [i32; 3]) -> Candidate {
    Candidate {
        active: true,
        alive: true,
        item: false,
        name,
        kind,
        pos,
        busy: false,
        script: None,
        primed: false,
    }
}

#[test]
fn a_fired_sequence_claims_its_named_actor_before_any_class_match() {
    let mut rec = record(1, [0; 3], 0);
    rec.class_kind = Some(2);
    rec.radius = 500;
    let crowd = Crowd(vec![
        candidate(0, 2, [10, 0, 0]),
        candidate(7, 2, [400, 0, 0]),
    ]);
    assert_eq!(claim(5, &rec, &crowd), Claim::Assign(1));
}

#[test]
fn without_a_free_named_actor_the_first_class_match_in_roster_order_within_the_radius_wins() {
    let mut rec = record(1, [0; 3], 0);
    rec.actor_name = 99;
    rec.class_kind = Some(2);
    rec.radius = 150;
    let mut far = candidate(0, 2, [151, 0, 0]);
    far.name = 1;
    let crowd = Crowd(vec![
        far,
        candidate(0, 3, [5, 0, 0]),
        candidate(0, 2, [100, 0, 0]),
        candidate(0, 2, [20, 0, 0]),
    ]);
    assert_eq!(
        claim(5, &rec, &crowd),
        Claim::Assign(2),
        "roster order beats distance"
    );
    // The radius is inclusive and measured in 3-D from the mark.
    let crowd = Crowd(vec![candidate(0, 2, [90, 120, 0])]);
    assert_eq!(claim(5, &rec, &crowd), Claim::Assign(0));
}

#[test]
fn busy_dead_inactive_and_item_actors_are_never_claimed_and_the_fire_retries_in_one_second() {
    assert_eq!(RETRY_TICKS, 20);
    let rec = record(1, [0; 3], 0);
    let mut busy = candidate(7, 0, [0; 3]);
    busy.busy = true;
    let mut dead = candidate(7, 0, [0; 3]);
    dead.alive = false;
    let mut asleep = candidate(7, 0, [0; 3]);
    asleep.active = false;
    let mut item = candidate(7, 0, [0; 3]);
    item.item = true;
    let crowd = Crowd(vec![busy, dead, asleep, item]);
    assert_eq!(claim(5, &rec, &crowd), Claim::Retry);
}

#[test]
fn firing_a_sequence_that_is_already_playing_is_ignored_but_a_primed_actor_is_started() {
    let rec = record(1, [0; 3], 0);
    let mut playing = candidate(0, 0, [0; 3]);
    playing.script = Some(5);
    playing.busy = true;
    let crowd = Crowd(vec![candidate(7, 0, [0; 3]), playing]);
    assert_eq!(claim(5, &rec, &crowd), Claim::Ignore);
    let mut primed = playing;
    primed.primed = true;
    let crowd = Crowd(vec![candidate(7, 0, [0; 3]), primed]);
    assert_eq!(claim(5, &rec, &crowd), Claim::Assign(1));
    // A dead owner does not count.
    let mut dead = playing;
    dead.alive = false;
    let crowd = Crowd(vec![dead, candidate(7, 0, [0; 3])]);
    assert_eq!(claim(5, &rec, &crowd), Claim::Assign(1));
}

#[test]
fn map_start_primes_idle_scripts_and_runs_untargeted_ones() {
    let mut r = record(1, [0; 3], 0);
    r.targeted = false;
    r.has_idle = true;
    r.has_play = false;
    assert_eq!(
        spawn_action(&r),
        SpawnAction::Prime,
        "untargeted, idle only: hold forever"
    );
    r.has_play = true;
    assert_eq!(spawn_action(&r), SpawnAction::Fire);
    r.has_idle = false;
    assert_eq!(spawn_action(&r), SpawnAction::Fire);
    r.targeted = true;
    assert_eq!(spawn_action(&r), SpawnAction::Nothing);
    r.has_idle = true;
    assert_eq!(spawn_action(&r), SpawnAction::Prime);
}

// ---------------------------------------------------------------------------
// Moving to the mark
// ---------------------------------------------------------------------------

#[test]
fn authored_move_modes_map_to_wait_walk_run_and_instant() {
    let got: Vec<u8> = (0..=6).map(runtime_move_mode).collect();
    assert_eq!(
        got,
        [
            MODE_IN_PLACE,
            MODE_WALK,
            MODE_RUN,
            MODE_IN_PLACE,
            MODE_INSTANT,
            MODE_IN_PLACE,
            MODE_IN_PLACE
        ]
    );
}

#[test]
fn an_actor_within_eight_units_of_the_mark_is_planted_without_walking() {
    for (dx, walks) in [(8, false), (9, true)] {
        let rec = record(1, [dx, 0, 0], 0);
        let mut stage = Stage::new(vec![rec]);
        let mut a = idle_actor([0; 3]);
        assign(&mut a, 0, &rec, SCIENTIST, 0, &mut stage);
        a.hold = 0;
        let cx = TickContext {
            now: 2,
            move_tick: 2,
            slot: 0,
            kind: SCIENTIST,
            primed_hold: false,
        };
        tick(&mut a, &cx, &mut stage);
        assert_eq!(a.activity == Activity::Moving, walks, "{dx} units out");
        if !walks {
            assert_eq!(a.pos, [dx, 0, 0], "planted exactly on the mark");
        }
    }
}

#[test]
fn walk_and_run_deadlines_scale_with_distance_and_stay_between_five_and_sixty_seconds() {
    let deadline = |mode: u16, dx: i32, kind: ActorKind| {
        let rec = record(mode, [dx, 0, 0], 0);
        let mut stage = Stage::new(vec![rec]);
        let mut a = idle_actor([0; 3]);
        assign(&mut a, 0, &rec, kind, 1000, &mut stage);
        a.deadline.wrapping_sub(1000)
    };
    assert_eq!(deadline(1, 0, SCIENTIST), 100);
    assert_eq!(deadline(1, 600, SCIENTIST), 550);
    assert_eq!(deadline(2, 600, SCIENTIST), 157);
    assert_eq!(deadline(2, 600, BARNEY), 133);
    assert_eq!(deadline(1, 30_000, SCIENTIST), 1_200);
    assert_eq!(
        deadline(4, 600, SCIENTIST),
        0,
        "instant moves have no deadline"
    );
}

#[test]
fn a_walk_waits_five_thinks_before_its_first_step() {
    let rec = record(1, [200, 0, 0], 0);
    let mut stage = Stage::new(vec![rec]);
    let mut a = idle_actor([0; 3]);
    assign(&mut a, 0, &rec, SCIENTIST, 0, &mut stage);
    assert_eq!(a.hold, 5);
    let mut first_move = 0;
    for now in 1..=14 {
        run(&mut a, &mut stage, SCIENTIST, now, now);
        if a.pos != [0, 0, 0] {
            first_move = now;
            break;
        }
    }
    assert_eq!(
        first_move, 12,
        "five even-tick thinks spent holding (2..=10)"
    );
}

#[test]
fn walking_scientists_speak_and_look_ahead_only_once_moving_on_their_own_think() {
    let rec = record(1, [200, 0, 0], 0);
    let mut stage = Stage::new(vec![rec]);
    let mut a = idle_actor([0; 3]);
    assign(&mut a, 0, &rec, SCIENTIST, 0, &mut stage);
    stage.log.clear();
    run(&mut a, &mut stage, SCIENTIST, 1, 16);
    let talk: Vec<u16> = stage
        .log
        .iter()
        .filter(|e| e.1 == "talk")
        .map(|e| e.0)
        .collect();
    let look: Vec<u16> = stage
        .log
        .iter()
        .filter(|e| e.1.starts_with("lookahead"))
        .map(|e| e.0)
        .collect();
    assert_eq!(talk, [11, 12, 13, 14, 15, 16]);
    assert_eq!(look, [12, 14, 16]);
    // Barney walks without the scientists' small talk.
    let mut stage = Stage::new(vec![rec]);
    let mut b = idle_actor([0; 3]);
    assign(&mut b, 0, &rec, BARNEY, 0, &mut stage);
    stage.log.clear();
    run(&mut b, &mut stage, BARNEY, 1, 16);
    assert!(stage.log.iter().all(|e| e.1 != "talk"));
}

#[test]
fn a_routed_or_detouring_walk_skips_the_lookahead() {
    let rec = record(1, [200, 0, 0], 0);
    for plan in [
        RoutePlan {
            waypoint: None,
            routed: true,
            detour: false,
        },
        RoutePlan {
            waypoint: Some([50, 0, 80]),
            routed: false,
            detour: true,
        },
    ] {
        let mut stage = Stage::new(vec![rec]);
        stage.plan = plan;
        let mut a = idle_actor([0; 3]);
        assign(&mut a, 0, &rec, SCIENTIST, 0, &mut stage);
        run(&mut a, &mut stage, SCIENTIST, 1, 20);
        assert!(
            stage.log.iter().all(|e| !e.1.starts_with("lookahead")),
            "{plan:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Facing the authored yaw
// ---------------------------------------------------------------------------

#[test]
fn a_walk_with_an_idle_animation_turns_sixty_then_twenty_four_degrees_per_think_then_settles() {
    let deg = |d: u32| ((d * 4096 + 180) / 360) as u16 & 0x0fff;
    let mut rec = record(1, [4, 0, 0], deg(170));
    rec.has_idle = true;
    let mut stage = Stage::new(vec![rec]);
    let mut a = idle_actor([0; 3]);
    assign(&mut a, 0, &rec, SCIENTIST, 0, &mut stage);
    a.hold = 0;
    let mut yaws = Vec::new();
    let mut finished = None;
    for now in 2..60u16 {
        let cx = TickContext {
            now,
            move_tick: now,
            slot: 0,
            kind: SCIENTIST,
            primed_hold: false,
        };
        tick(&mut a, &cx, &mut stage);
        if a.mode.base == MODE_FACE && yaws.last() != Some(&a.yaw) {
            yaws.push(a.yaw);
        }
        if a.mode.base == MODE_NONE && finished.is_none() && now > 2 {
            finished = Some(now);
        }
    }
    let steps: Vec<i32> = yaws.windows(2).map(|w| w[1] as i32 - w[0] as i32).collect();
    assert_eq!(steps[0], 683, "60 degrees on the first think");
    assert!(
        steps[1..steps.len() - 1].iter().all(|&s| s == 273),
        "24 degrees after"
    );
    assert_eq!(*yaws.last().unwrap(), deg(170));
    // Reached the yaw on a think tick, then two settle ticks, then playing.
    assert!(finished.is_some());
}

// ---------------------------------------------------------------------------
// Playing, firing, releasing
// ---------------------------------------------------------------------------

#[test]
fn damage_cancels_unless_uninterruptible_and_death_always_cancels() {
    assert!(damage_cancels(true, false));
    assert!(!damage_cancels(true, true));
    assert!(damage_cancels(false, true));
    assert!(damage_cancels(false, false));
    assert_eq!((SF_REPEATABLE, SF_NO_INTERRUPT), (4, 32));
}

#[test]
fn releasing_forgets_the_script_without_touching_where_the_actor_is() {
    let rec = record(1, [200, 0, 0], 0);
    let mut stage = Stage::new(vec![rec]);
    let mut a = idle_actor([3, 4, 5]);
    assign(&mut a, 0, &rec, SCIENTIST, 0, &mut stage);
    a.no_interrupt = true;
    let before = a;
    release(&mut a);
    assert_eq!((a.mode, a.script), (ScriptMode::NONE, None));
    assert_eq!(
        (a.play_clip, a.idle_clip, a.no_interrupt),
        (NO_CLIP, NO_CLIP, false)
    );
    assert_eq!(
        (a.pos, a.goal, a.yaw),
        (before.pos, before.goal, before.yaw)
    );
}

#[test]
fn a_play_animation_that_carries_its_actor_leaves_it_at_the_offset_dropped_to_the_floor() {
    assert_eq!(root_motion_target([0, 0, 0], 0, [100, 0]), [0, 0, 100]);
    assert_eq!(root_motion_target([0, 0, 0], 1024, [100, 0]), [100, 0, 0]);
    assert_eq!(root_motion_target([10, 5, 10], 0, [0, 40]), [-30, 5, 10]);
}

#[test]
fn a_primed_walk_waits_one_second_for_its_start_then_moves_by_itself() {
    let mut rec = record(1, [100, 0, 0], 0);
    rec.has_idle = true;
    let mut stage = Stage::new(vec![rec]);
    let mut a = idle_actor([0; 3]);
    prime(&mut a, 0, &rec, SCIENTIST, &mut stage);
    assert!(a.mode.primed);
    assert_eq!(a.play_clip, NO_CLIP);
    assert_eq!(a.deadline, 19);
    let mut started = None;
    for now in 0..40u16 {
        let holds = primed_holds(&mut a, SCIENTIST, now, &stage);
        if !holds {
            started = Some(now);
            break;
        }
    }
    assert_eq!(started, Some(19));
    assert_eq!(a.activity, Activity::Moving);
}

// ---------------------------------------------------------------------------
// Golden scenarios
// ---------------------------------------------------------------------------

/// Recorded from ac83da7 behaviour. A scientist in roster slot 0 walks 100
/// units to a mark facing +X, plants, turns, plays a 30-tick gesture, fires
/// its target and is removed. The sequence is fired at tick 0.
#[test]
fn golden_walk_plant_play_fire() {
    let rec = record(1, [100, 0, 0], 1024);
    let mut stage = Stage::new(vec![rec]);
    let mut a = idle_actor([0; 3]);
    assign(&mut a, 0, &rec, SCIENTIST, 0, &mut stage);
    let rows = run(&mut a, &mut stage, SCIENTIST, 1, 200);
    let expected: &[(u16, u16, &str, [i32; 3])] = &[
        (
            1,
            1,
            "owned=true mode=1 detour=false goal=[100, 0, 0] act=Moving yaw=0 hold=5",
            [0, 0, 0],
        ),
        (
            2,
            3,
            "owned=true mode=1 detour=false goal=[100, 0, 0] act=Moving yaw=0 hold=4",
            [0, 0, 0],
        ),
        (
            4,
            5,
            "owned=true mode=1 detour=false goal=[100, 0, 0] act=Moving yaw=0 hold=3",
            [0, 0, 0],
        ),
        (
            6,
            7,
            "owned=true mode=1 detour=false goal=[100, 0, 0] act=Moving yaw=0 hold=2",
            [0, 0, 0],
        ),
        (
            8,
            9,
            "owned=true mode=1 detour=false goal=[100, 0, 0] act=Moving yaw=0 hold=1",
            [0, 0, 0],
        ),
        (
            10,
            42,
            "owned=true mode=1 detour=false goal=[100, 0, 0] act=Moving yaw=0 hold=0",
            [94, 0, 0],
        ),
        (
            43,
            72,
            "owned=true mode=0 detour=false goal=[100, 0, 0] act=Idle yaw=1024 hold=0",
            [100, 0, 0],
        ),
        (
            73,
            73,
            "owned=false mode=0 detour=false goal=[100, 0, 0] act=Idle yaw=1024 hold=0",
            [100, 0, 0],
        ),
    ];
    assert_eq!(rows_ref(&rows), expected, "{rows:#?}");
    let events: Vec<(u16, &str)> = log_ref(&stage)
        .into_iter()
        .filter(|e| e.1 != "talk" && !e.1.starts_with("lookahead"))
        .collect();
    let expected: &[(u16, &str)] = &[
        (0, "talker taken"),
        (0, "plan [0, 0, 0] -> [100, 0, 0]"),
        (43, "place [100, 0, 0]"),
        (73, "fire 0"),
        (73, "remove 0"),
    ];
    assert_eq!(events, expected, "{events:#?}");
}

/// Recorded from ac83da7 behaviour. A Barney in roster slot 1 runs to a mark
/// 300 units away whose sequence has an idle animation, so it turns to the
/// authored yaw (180 degrees) before playing a 10-tick gesture. The script is
/// repeatable and is not removed.
#[test]
fn golden_run_face_then_play_repeatable() {
    let mut rec = record(2, [0, 0, 300], 2048);
    rec.has_idle = true;
    rec.repeatable = true;
    let mut stage = Stage::new(vec![rec]);
    stage.slot = 1;
    stage.play_hold = 10;
    let mut a = idle_actor([0; 3]);
    assign(&mut a, 0, &rec, BARNEY, 0, &mut stage);
    let rows = run(&mut a, &mut stage, BARNEY, 1, 200);
    let expected: &[(u16, u16, &str, [i32; 3])] = &[
        (
            1,
            2,
            "owned=true mode=2 detour=false goal=[0, 0, 300] act=Moving yaw=0 hold=4",
            [0, 0, 0],
        ),
        (
            3,
            4,
            "owned=true mode=2 detour=false goal=[0, 0, 300] act=Moving yaw=0 hold=3",
            [0, 0, 0],
        ),
        (
            5,
            6,
            "owned=true mode=2 detour=false goal=[0, 0, 300] act=Moving yaw=0 hold=2",
            [0, 0, 0],
        ),
        (
            7,
            8,
            "owned=true mode=2 detour=false goal=[0, 0, 300] act=Moving yaw=0 hold=1",
            [0, 0, 0],
        ),
        (
            9,
            27,
            "owned=true mode=2 detour=false goal=[0, 0, 300] act=Moving yaw=0 hold=0",
            [0, 0, 300],
        ),
        (
            28,
            28,
            "owned=true mode=6 detour=false goal=[0, 0, 300] act=Idle yaw=0 hold=128",
            [0, 0, 300],
        ),
        (
            29,
            30,
            "owned=true mode=6 detour=false goal=[0, 0, 300] act=Idle yaw=683 hold=0",
            [0, 0, 300],
        ),
        (
            31,
            32,
            "owned=true mode=6 detour=false goal=[0, 0, 300] act=Idle yaw=956 hold=0",
            [0, 0, 300],
        ),
        (
            33,
            34,
            "owned=true mode=6 detour=false goal=[0, 0, 300] act=Idle yaw=1229 hold=0",
            [0, 0, 300],
        ),
        (
            35,
            36,
            "owned=true mode=6 detour=false goal=[0, 0, 300] act=Idle yaw=1502 hold=0",
            [0, 0, 300],
        ),
        (
            37,
            38,
            "owned=true mode=6 detour=false goal=[0, 0, 300] act=Idle yaw=1775 hold=0",
            [0, 0, 300],
        ),
        (
            39,
            39,
            "owned=true mode=6 detour=false goal=[0, 0, 300] act=Idle yaw=2048 hold=2",
            [0, 0, 300],
        ),
        (
            40,
            40,
            "owned=true mode=6 detour=false goal=[0, 0, 300] act=Idle yaw=2048 hold=1",
            [0, 0, 300],
        ),
        (
            41,
            50,
            "owned=true mode=0 detour=false goal=[0, 0, 300] act=Idle yaw=2048 hold=0",
            [0, 0, 300],
        ),
        (
            51,
            51,
            "owned=false mode=0 detour=false goal=[0, 0, 300] act=Idle yaw=2048 hold=0",
            [0, 0, 300],
        ),
    ];
    assert_eq!(rows_ref(&rows), expected, "{rows:#?}");
    let expected: &[(u16, &str)] = &[
        (0, "talker taken"),
        (0, "plan [0, 0, 0] -> [0, 0, 300]"),
        (11, "lookahead [0, 0, 300]"),
        (13, "lookahead [0, 0, 300]"),
        (15, "lookahead [0, 0, 300]"),
        (17, "lookahead [0, 0, 300]"),
        (19, "lookahead [0, 0, 300]"),
        (21, "lookahead [0, 0, 300]"),
        (23, "lookahead [0, 0, 300]"),
        (25, "lookahead [0, 0, 300]"),
        (27, "lookahead [0, 0, 300]"),
        (28, "place [0, 0, 300]"),
        (51, "fire 0"),
    ];
    assert_eq!(log_ref(&stage), expected, "{:#?}", stage.log);
}

/// Recorded from ac83da7 behaviour. A wedged scientist never moves; at its
/// deadline it is planted on the mark itself, even though it was detouring
/// toward an intermediate waypoint.
#[test]
fn golden_timeout_plants_on_the_mark() {
    let rec = record(1, [400, 0, 0], 0);
    let mut stage = Stage::new(vec![rec]);
    stage.stuck = true;
    stage.plan = RoutePlan {
        waypoint: Some([200, 0, 150]),
        routed: false,
        detour: true,
    };
    let mut a = idle_actor([0; 3]);
    assign(&mut a, 0, &rec, SCIENTIST, 0, &mut stage);
    let rows = run(&mut a, &mut stage, SCIENTIST, 1, 1500);
    let expected: &[(u16, u16, &str, [i32; 3])] = &[
        (
            1,
            1,
            "owned=true mode=1 detour=true goal=[200, 0, 150] act=Moving yaw=0 hold=5",
            [0, 0, 0],
        ),
        (
            2,
            3,
            "owned=true mode=1 detour=true goal=[200, 0, 150] act=Moving yaw=0 hold=4",
            [0, 0, 0],
        ),
        (
            4,
            5,
            "owned=true mode=1 detour=true goal=[200, 0, 150] act=Moving yaw=0 hold=3",
            [0, 0, 0],
        ),
        (
            6,
            7,
            "owned=true mode=1 detour=true goal=[200, 0, 150] act=Moving yaw=0 hold=2",
            [0, 0, 0],
        ),
        (
            8,
            9,
            "owned=true mode=1 detour=true goal=[200, 0, 150] act=Moving yaw=0 hold=1",
            [0, 0, 0],
        ),
        (
            10,
            465,
            "owned=true mode=1 detour=true goal=[200, 0, 150] act=Moving yaw=0 hold=0",
            [0, 0, 0],
        ),
        (
            466,
            495,
            "owned=true mode=0 detour=false goal=[200, 0, 150] act=Idle yaw=0 hold=0",
            [400, 0, 0],
        ),
        (
            496,
            496,
            "owned=false mode=0 detour=false goal=[200, 0, 150] act=Idle yaw=0 hold=0",
            [400, 0, 0],
        ),
    ];
    assert_eq!(rows_ref(&rows), expected, "{rows:#?}");
    let events: Vec<(u16, &str)> = log_ref(&stage)
        .into_iter()
        .filter(|e| e.1 != "talk")
        .collect();
    let expected: &[(u16, &str)] = &[
        (0, "talker taken"),
        (0, "plan [0, 0, 0] -> [400, 0, 0]"),
        (466, "place [400, 0, 0]"),
        (466, "place [400, 0, 0]"),
        (496, "fire 0"),
        (496, "remove 0"),
    ];
    assert_eq!(events, expected, "{events:#?}");
}

/// Recorded from ac83da7 behaviour. A detour plan sends the walker to an
/// intermediate waypoint first, then on to the mark.
#[test]
fn golden_detour_then_mark() {
    let rec = record(1, [240, 0, 0], 0);
    let mut stage = Stage::new(vec![rec]);
    stage.plan = RoutePlan {
        waypoint: Some([120, 0, 90]),
        routed: false,
        detour: true,
    };
    let mut a = idle_actor([0; 3]);
    assign(&mut a, 0, &rec, SCIENTIST, 0, &mut stage);
    assert_eq!(a.goal, [120, 0, 90]);
    let rows = run(&mut a, &mut stage, SCIENTIST, 1, 300);
    let expected: &[(u16, u16, &str, [i32; 3])] = &[
        (
            1,
            1,
            "owned=true mode=1 detour=true goal=[120, 0, 90] act=Moving yaw=0 hold=5",
            [0, 0, 0],
        ),
        (
            2,
            3,
            "owned=true mode=1 detour=true goal=[120, 0, 90] act=Moving yaw=0 hold=4",
            [0, 0, 0],
        ),
        (
            4,
            5,
            "owned=true mode=1 detour=true goal=[120, 0, 90] act=Moving yaw=0 hold=3",
            [0, 0, 0],
        ),
        (
            6,
            7,
            "owned=true mode=1 detour=true goal=[120, 0, 90] act=Moving yaw=0 hold=2",
            [0, 0, 0],
        ),
        (
            8,
            9,
            "owned=true mode=1 detour=true goal=[120, 0, 90] act=Moving yaw=0 hold=1",
            [0, 0, 0],
        ),
        (
            10,
            68,
            "owned=true mode=1 detour=true goal=[120, 0, 90] act=Moving yaw=0 hold=0",
            [115, 0, 86],
        ),
        (
            69,
            124,
            "owned=true mode=1 detour=false goal=[240, 0, 0] act=Moving yaw=0 hold=0",
            [236, 0, 3],
        ),
        (
            125,
            154,
            "owned=true mode=0 detour=false goal=[240, 0, 0] act=Idle yaw=0 hold=0",
            [240, 0, 0],
        ),
        (
            155,
            155,
            "owned=false mode=0 detour=false goal=[240, 0, 0] act=Idle yaw=0 hold=0",
            [240, 0, 0],
        ),
    ];
    assert_eq!(rows_ref(&rows), expected, "{rows:#?}");
}

/// Recorded from ac83da7 behaviour. Instant and in-place sequences: the
/// first teleports to the mark and faces the authored yaw; the second plays
/// where the actor stands. Both carry a root offset, so the actor ends at
/// the offset dropped to a floor 6 units down.
#[test]
fn golden_instant_and_in_place_with_root_motion() {
    type Rows = &'static [(u16, u16, &'static str, [i32; 3])];
    type Log = &'static [(u16, &'static str)];
    let cases: [(u16, Rows, Log); 2] = [
        (
            4,
            &[
                (
                    0,
                    5,
                    "owned=true mode=0 detour=false goal=[64, 10, -32] act=Idle yaw=512 hold=0",
                    [64, 10, -32],
                ),
                (
                    6,
                    6,
                    "owned=false mode=0 detour=false goal=[64, 10, -32] act=Idle yaw=512 hold=0",
                    [109, 4, -10],
                ),
            ],
            &[
                (0, "talker taken"),
                (0, "place [64, 10, -32]"),
                (0, "place [64, 10, -32]"),
                (6, "floor probe [109, 11, -10] depth 256"),
                (6, "place [109, 4, -10]"),
                (6, "fire 0"),
                (6, "remove 0"),
            ],
        ),
        (
            0,
            &[
                (
                    0,
                    5,
                    "owned=true mode=0 detour=false goal=[64, 10, -32] act=Idle yaw=3000 hold=0",
                    [0, 10, 0],
                ),
                (
                    6,
                    6,
                    "owned=false mode=0 detour=false goal=[64, 10, -32] act=Idle yaw=3000 hold=0",
                    [-50, 4, 10],
                ),
            ],
            &[
                (0, "talker taken"),
                (6, "floor probe [-50, 11, 10] depth 256"),
                (6, "place [-50, 4, 10]"),
                (6, "fire 0"),
                (6, "remove 0"),
            ],
        ),
    ];
    for (mode, expected_rows, expected_log) in cases {
        let mut rec = record(mode, [64, 10, -32], 512);
        rec.root_offset = Some([48, -16]);
        let mut stage = Stage::new(vec![rec]);
        stage.play_hold = 6;
        stage.floor = Some(4);
        let mut a = idle_actor([0, 10, 0]);
        a.yaw = 3000;
        assign(&mut a, 0, &rec, SCIENTIST, 0, &mut stage);
        let rows = run(&mut a, &mut stage, SCIENTIST, 0, 50);
        assert_eq!(rows_ref(&rows), expected_rows, "mode {mode}: {rows:#?}");
        assert_eq!(
            log_ref(&stage),
            expected_log,
            "mode {mode}: {:#?}",
            stage.log
        );
    }
}

/// Recorded from ac83da7 behaviour. A play-in-place sequence fires, when it
/// completes, a second sequence (with an idle pose) for the same actor. The
/// actor is released before the targets fire, so the chained script takes it
/// within the same tick and then runs to completion on its own.
#[test]
fn golden_completion_chains_into_the_next_script() {
    let first = record(3, [0; 3], 0);
    let mut second = record(3, [0; 3], 0);
    second.idle_clip = 5;
    second.play_clip = NO_CLIP;
    let mut stage = Stage::new(vec![first, second]);
    stage.play_hold = 4;
    stage.chain = Some((0, 1));
    let mut a = idle_actor([0; 3]);
    assign(&mut a, 0, &first, SCIENTIST, 0, &mut stage);
    let rows = run(&mut a, &mut stage, SCIENTIST, 0, 12);
    let expected: &[(u16, u16, &str, [i32; 3])] = &[
        (
            0,
            3,
            "owned=true mode=0 detour=false goal=[0, 0, 0] act=Idle yaw=0 hold=0",
            [0, 0, 0],
        ),
        (
            4,
            4,
            "owned=true mode=3 detour=false goal=[0, 0, 0] act=Idle yaw=0 hold=0",
            [0, 0, 0],
        ),
        (
            5,
            5,
            "owned=true mode=0 detour=false goal=[0, 0, 0] act=Idle yaw=0 hold=0",
            [0, 0, 0],
        ),
        (
            6,
            6,
            "owned=false mode=0 detour=false goal=[0, 0, 0] act=Idle yaw=0 hold=0",
            [0, 0, 0],
        ),
    ];
    assert_eq!(rows_ref(&rows), expected, "{rows:#?}");
    let expected: &[(u16, &str)] = &[
        (0, "talker taken"),
        (4, "fire 0"),
        (4, "talker taken"),
        (4, "remove 0"),
        (6, "fire 1"),
        (6, "remove 1"),
    ];
    assert_eq!(log_ref(&stage), expected, "{:#?}", stage.log);
}
