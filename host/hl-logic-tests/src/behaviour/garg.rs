//! Gargantua (HV-01): attack timings, swipe, stomp scheduling, the flame
//! sweep and its cancel rule, flame damage, footstep shakes, the damage
//! rule, pain cadence and the death timeline. The stomp wave's motion and
//! the attack choice are pinned in `setpiece_logic`'s own tests.

use crate::behaviour::setpiece_kit::{segment_box_frac, trace_walls, Lcg, Pick, Wall};
use crate::garg_logic::{
    scale_damage, GargBrain, GargInputs, GargModel, GargSound, GargState, GargWorld,
    GargWorldInputs,
};
use crate::setpiece_logic as sl;
use crate::setpiece_math::TraceHit;

const MODEL: GargModel = GargModel {
    run_per_tick: 18,
    flame_attach: [[111, -66, 86], [110, 64, 88]],
    swipe_event: sl::GARG_SWIPE_EVENT_TICKS,
    swipe_len: sl::GARG_SWIPE_TICKS,
    stomp_event: sl::GARG_STOMP_EVENT_TICKS,
    stomp_len: sl::GARG_STOMP_TICKS,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    State(GargState),
    ClearTarget,
    Hurt(u16, [i32; 3]),
    Punch(i32, i32),
    DamageTarget(u8),
    Shake(i32, u16),
    Sound(GargSound, [i32; 3]),
    Beam([i32; 3], [i32; 3], bool),
    Dust([i32; 3]),
    Fx([i32; 3], u8),
    Gib([i32; 3]),
    LoopMute,
    LoopStop,
}

struct World {
    pick: Pick,
    walls: Vec<Wall>,
    player: [i32; 3],
    alive: bool,
    target_box: Option<([i32; 3], [i32; 3])>,
    pos: [i32; 3],
    /// Ignore the walk, so range-dependent attacks can be set up.
    planted: bool,
    state: Option<GargState>,
    now: u16,
    events: Vec<(u16, Ev)>,
}

impl World {
    fn new(player: [i32; 3]) -> Self {
        Self {
            pick: Pick::Low,
            walls: Vec::new(),
            player,
            alive: true,
            target_box: None,
            pos: [0, 0, 0],
            planted: true,
            state: None,
            now: 0,
            events: Vec::new(),
        }
    }
    fn ev(&mut self, e: Ev) {
        self.events.push((self.now, e));
    }
    fn ticks_of(&self, f: impl Fn(&Ev) -> bool) -> Vec<u16> {
        self.events
            .iter()
            .filter(|e| f(&e.1))
            .map(|e| e.0)
            .collect()
    }
    fn sounds(&self, s: GargSound) -> Vec<u16> {
        self.ticks_of(|e| matches!(e, Ev::Sound(x, _) if *x == s))
    }
}

impl GargWorld for World {
    fn random_below(&mut self, n: u32) -> u32 {
        self.pick.below(n)
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        trace_walls(&self.walls, from, to)
    }
    fn player_box_frac(&mut self, from: [i32; 3], to: [i32; 3], pad: i32) -> Option<i32> {
        if !self.alive {
            return None;
        }
        let p = self.player;
        let (w, h) = (16 + pad, 36 + pad);
        segment_box_frac(
            from,
            to,
            [p[0] - w, p[1] - h, p[2] - w],
            [p[0] + w, p[1] + h, p[2] + w],
        )
    }
    fn target_box_hit(&mut self, from: [i32; 3], to: [i32; 3]) -> bool {
        self.target_box
            .is_some_and(|(mn, mx)| segment_box_frac(from, to, mn, mx).is_some())
    }
    fn set_state(&mut self, state: GargState) {
        if self.state != Some(state) {
            self.state = Some(state);
            self.ev(Ev::State(state));
        }
    }
    fn keep_target(&mut self) {}
    fn clear_target(&mut self) {
        self.ev(Ev::ClearTarget);
    }
    fn face(&mut self, _point: [i32; 3]) {}
    fn run_towards(&mut self, point: [i32; 3], step: i32) {
        if self.planted {
            return;
        }
        let d = [point[0] - self.pos[0], point[2] - self.pos[2]];
        let l = psx_math::int32::isqrt_i32(d[0] * d[0] + d[1] * d[1]);
        if l > step {
            self.pos[0] += d[0] * step / l;
            self.pos[2] += d[1] * step / l;
        }
    }
    fn hurt_player(&mut self, damage: u16, from: [i32; 3]) {
        self.ev(Ev::Hurt(damage, from));
    }
    fn view_punch(&mut self, pitch: i32, yaw: i32) {
        self.ev(Ev::Punch(pitch, yaw));
    }
    fn damage_target(&mut self, damage: u8) {
        self.ev(Ev::DamageTarget(damage));
    }
    fn shake(&mut self, amplitude: i32, ticks: u16) {
        self.ev(Ev::Shake(amplitude, ticks));
    }
    fn sound(&mut self, sound: GargSound, at: [i32; 3]) {
        self.ev(Ev::Sound(sound, at));
    }
    fn flame_loop(&mut self, _at: [i32; 3]) {}
    fn flame_loop_mute(&mut self) {
        self.ev(Ev::LoopMute);
    }
    fn flame_loop_stop(&mut self) {
        self.ev(Ev::LoopStop);
    }
    fn flame_beam(&mut self, from: [i32; 3], to: [i32; 3], core: bool) {
        self.ev(Ev::Beam(from, to, core));
    }
    fn stomp_dust(&mut self, at: [i32; 3]) {
        self.ev(Ev::Dust(at));
    }
    fn explosion_fx(&mut self, at: [i32; 3], mag: u8) {
        self.ev(Ev::Fx(at, mag));
    }
    fn gib(&mut self, at: [i32; 3]) {
        self.ev(Ev::Gib(at));
    }
}

/// The gargantua at the origin facing +x (yaw a quarter turn), the player
/// its visible target.
fn inputs(w: &World) -> GargInputs {
    GargInputs {
        now: w.now,
        visible: true,
        aim: Some([w.player[0], w.player[1] + 14, w.player[2]]),
        target_is_player: true,
        enemy_pos: w.player,
        pos: w.pos,
        yaw: 1024,
        player_pos: w.player,
        player_alive: w.alive,
        view_height: 28,
        slash_damage: 30,
        flame_damage: 3,
    }
}

fn world_inputs(w: &World) -> GargWorldInputs {
    GargWorldInputs {
        now: w.now,
        present: true,
        active: true,
        dead: false,
        pos: w.pos,
        player_pos: w.player,
        stomp_damage: 50,
    }
}

/// Ticks `from..=to`: the schedule, then the world tick, as the game runs
/// them.
fn run(g: &mut GargBrain, w: &mut World, from: u16, to: u16) {
    for now in from..=to {
        w.now = now;
        let i = inputs(w);
        g.think(&i, &MODEL, w);
        let wi = world_inputs(w);
        g.tick_world(&wi, w);
    }
}

fn spawned(w: &mut World) -> GargBrain {
    let mut g = GargBrain::new();
    w.now = 0;
    g.spawn(0);
    g
}

/// Azimuth of a beam in q12 turns (0 = +z, 1024 = +x).
fn azimuth(from: [i32; 3], to: [i32; 3]) -> i32 {
    crate::scientist_logic::precise_yaw_from_vec(to[0] - from[0], to[2] - from[2]) as i32
}

#[test]
fn it_flames_once_its_two_second_spawn_cooldown_runs_out() {
    let mut w = World::new([200, 0, 0]);
    let mut g = spawned(&mut w);
    run(&mut g, &mut w, 0, 60);
    assert_eq!(w.sounds(GargSound::FlameOn), [40]);
}

#[test]
fn it_stomps_only_after_five_seconds_of_unbroken_sight() {
    let mut w = World::new([500, 0, 0]);
    let mut g = spawned(&mut w);
    run(&mut g, &mut w, 0, 49);
    // A blink at tick 50 restarts the five seconds.
    w.now = 50;
    let mut i = inputs(&w);
    i.visible = false;
    g.think(&i, &MODEL, &mut w);
    w.pos = [0; 3];
    run(&mut g, &mut w, 51, 200);
    let stomps = w.sounds(GargSound::Stomp);
    assert_eq!(stomps, [150 + sl::GARG_STOMP_EVENT_TICKS as u16]);
}

#[test]
fn after_a_stomp_the_next_waits_twelve_seconds() {
    let mut w = World::new([500, 0, 0]);
    let mut g = spawned(&mut w);
    for now in 0..=700u16 {
        w.pos = [0; 3]; // planted
        run(&mut g, &mut w, now, now);
    }
    let stomps = w.sounds(GargSound::Stomp);
    let e = sl::GARG_STOMP_EVENT_TICKS as u16;
    let l = sl::GARG_STOMP_TICKS as u16;
    // Stomp starts at 100; its event lands 27 in; the next may start 240
    // after the event, once the sequence has finished.
    assert_eq!(stomps[0], 100 + e);
    assert_eq!(stomps[1], 100 + e + 240 + e);
    assert!(l < 240);
}

#[test]
fn the_swipe_lands_0_8_s_in_from_64_up_and_kicks_the_view() {
    let mut w = World::new([60, 0, 0]);
    let mut g = spawned(&mut w);
    run(&mut g, &mut w, 0, 40);
    let e = sl::GARG_SWIPE_EVENT_TICKS as u16;
    assert_eq!(e, 16);
    assert_eq!(
        w.events
            .iter()
            .filter(|x| matches!(x.1, Ev::Hurt(..) | Ev::Punch(..)))
            .copied()
            .collect::<Vec<_>>(),
        [(e, Ev::Hurt(30, [0, 0, 0])), (e, Ev::Punch(-341, -341))]
    );
    // The sequence ends 1.5 s in, then the next swipe starts.
    assert_eq!(sl::GARG_SWIPE_TICKS, 30);
    assert!(w.events.contains(&(30, Ev::State(GargState::Idle))));
}

#[test]
fn the_swipe_reaches_90_ahead_angled_down() {
    // Player box top at 36 + 16 pad: a player 70 units below the shoulder
    // line is missed, one at 60 ahead is hit, one at 125 ahead is missed.
    for (x, hit) in [(60, true), (125, false)] {
        let mut w = World::new([x, 0, 0]);
        let mut g = spawned(&mut w);
        let mut i = inputs(&w);
        // Force the swipe: the player counts as close.
        i.enemy_pos = [10, 0, 0];
        for now in 0..=20u16 {
            w.now = now;
            i.now = now;
            g.think(&i, &MODEL, &mut w);
        }
        assert_eq!(
            !w.ticks_of(|e| matches!(e, Ev::Hurt(..))).is_empty(),
            hit,
            "player at {x}"
        );
    }
}

#[test]
fn a_swipe_that_misses_the_player_can_hit_another_target() {
    let mut w = World::new([5000, 0, 0]);
    w.target_box = Some(([40, -40, -20], [80, 80, 20]));
    let mut g = spawned(&mut w);
    let mut i = inputs(&w);
    i.target_is_player = false;
    i.enemy_pos = [60, 0, 0];
    i.aim = Some([60, 20, 0]);
    i.slash_damage = 300;
    for now in 0..=20u16 {
        w.now = now;
        i.now = now;
        g.think(&i, &MODEL, &mut w);
    }
    assert_eq!(w.ticks_of(|e| *e == Ev::DamageTarget(255)), [16]);
}

#[test]
fn the_stomp_lands_1_35_s_in_shakes_and_sends_a_wave() {
    let mut w = World::new([400, 0, 0]);
    let mut g = spawned(&mut w);
    for now in 0..=200u16 {
        w.pos = [0; 3];
        run(&mut g, &mut w, now, now);
    }
    let e = sl::GARG_STOMP_EVENT_TICKS as u16;
    assert_eq!(e, 27);
    let at = 100 + e;
    // Shake 12 for 2 s within 1000, felt at 400: 12 * 600 / 1000.
    assert!(w.events.contains(&(at, Ev::Shake(7, 40))));
    // The wave sweeps the ground toward the player and hurts on the way.
    let hurts = w.ticks_of(|e| matches!(e, Ev::Hurt(50, _)));
    assert!(!hurts.is_empty());
    assert!(hurts[0] > at && hurts[0] < at + 40, "{hurts:?}");
    assert!(w.ticks_of(|e| matches!(e, Ev::Dust(..))).len() > 5);
}

#[test]
fn the_flame_sweep_lasts_4_5_s_and_the_next_waits_6_s() {
    let mut w = World::new([200, 0, 0]);
    let mut g = spawned(&mut w);
    for now in 0..=400u16 {
        w.pos = [0; 3];
        run(&mut g, &mut w, now, now);
    }
    let on = w.sounds(GargSound::FlameOn);
    let off = w.sounds(GargSound::FlameOff);
    assert_eq!(on[0], 40);
    assert_eq!(off[0], 130);
    assert!(on[1] >= 160, "{on:?}");
    assert!(w.events.contains(&(130, Ev::LoopMute)));
}

#[test]
fn the_flame_turns_8_degrees_a_tenth_up_to_45_aside() {
    // Player 30 degrees left of its facing (+x) at 200: the flame starts
    // straight and swings 91 q12 (8 degrees) per think.
    let p = [173, -50, 100];
    let mut w = World::new(p);
    let mut g = spawned(&mut w);
    run(&mut g, &mut w, 0, 48);
    let az: Vec<i32> = w
        .events
        .iter()
        .filter_map(|e| match e.1 {
            Ev::Beam(f, t, false) => Some(azimuth(f, t)),
            _ => None,
        })
        .step_by(2)
        .collect();
    let off: Vec<i32> = az.iter().map(|a| 1024 - a).collect();
    assert!((88..=94).contains(&off[0].abs()), "{off:?}");
    assert!((179..=185).contains(&off[1].abs()), "{off:?}");
    // Then it settles on the player's bearing (about 341).
    assert!(off.last().unwrap().abs() > 320 && off.last().unwrap().abs() < 360);
    // Moved to 55 degrees aside, the flame stops at 45 (512).
    let mut w = World::new([115, -50, 164]);
    let mut g = spawned(&mut w);
    let mut i = inputs(&w);
    i.enemy_pos = [200, 0, 0];
    for now in 0..=90u16 {
        w.now = now;
        i.now = now;
        g.think(&i, &MODEL, &mut w);
    }
    let last = w
        .events
        .iter()
        .rev()
        .find_map(|e| match e.1 {
            Ev::Beam(f, t, false) => Some(azimuth(f, t)),
            _ => None,
        })
        .unwrap();
    assert!(((1024 - last).abs() - 512).abs() <= 3, "{last}");
}

#[test]
fn a_player_far_or_aside_winds_the_flame_down_six_times_faster() {
    let mut w = World::new([200, 0, 0]);
    let mut g = spawned(&mut w);
    run(&mut g, &mut w, 0, 40);
    assert!(g.flaming());
    // Out to 450: each 0.1 s think takes another 0.5 s off the sweep.
    w.player = [450, 0, 0];
    run(&mut g, &mut w, 41, 100);
    let off = w.sounds(GargSound::FlameOff);
    assert_eq!(off.len(), 1);
    assert!(off[0] < 60, "{off:?}");
}

#[test]
fn both_flames_burn_a_player_in_front_with_the_falloff() {
    // The player's body centre level with the forearms, 150 ahead: each
    // flame passes 64-66 units to the side, inside the falloff band.
    let mut w = World::new([150, 72, 0]);
    let mut g = spawned(&mut w);
    run(&mut g, &mut w, 0, 46);
    let hurts: Vec<(u16, Ev)> = w
        .events
        .iter()
        .copied()
        .filter(|e| matches!(e.1, Ev::Hurt(..)))
        .collect();
    // Two hurts per think, one from each forearm (z -66 and +64), carrying
    // the tenths over between hurts (values recorded from ac83da7 behaviour).
    assert_eq!(
        hurts,
        [
            (42, Ev::Hurt(2, [111, 86, -66])),
            (42, Ev::Hurt(3, [110, 88, 64])),
            (44, Ev::Hurt(2, [111, 86, -66])),
            (44, Ev::Hurt(3, [110, 88, 64])),
            (46, Ev::Hurt(2, [111, 86, -66])),
            (46, Ev::Hurt(3, [110, 88, 64])),
        ]
    );
}

#[test]
fn chasing_it_shakes_the_ground_every_eleventh_step() {
    let mut w = World::new([2000, 0, 0]);
    w.alive = true;
    let mut g = spawned(&mut w);
    let mut i = inputs(&w);
    i.visible = false;
    for now in 0..=33u16 {
        w.now = now;
        i.now = now;
        i.pos = w.pos;
        g.think(&i, &MODEL, &mut w);
    }
    assert_eq!(w.sounds(GargSound::Step), [10, 21, 32]);
    // Shake 4 within 750, fading with distance (2000 away: none).
    assert!(w.ticks_of(|e| matches!(e, Ev::Shake(..))).is_empty());
    let mut w = World::new([300, 0, 0]);
    let mut g = spawned(&mut w);
    for now in 0..=10u16 {
        w.now = now;
        i.now = now;
        i.pos = [0; 3];
        i.player_pos = w.player;
        g.think(&i, &MODEL, &mut w);
    }
    assert_eq!(w.ticks_of(|e| *e == Ev::Shake(2, 20)), [10]);
}

#[test]
fn only_heavy_damage_hurts_it_and_any_heavy_hit_counts() {
    assert_eq!(scale_damage(200, false, 1000), 0);
    assert_eq!(scale_damage(1, true, 1000), 1);
    assert_eq!(scale_damage(100, true, 1000), 26);
    assert_eq!(scale_damage(200, true, 800), 64);
}

#[test]
fn pain_cries_come_at_most_every_2_5_to_4_seconds() {
    for (pick, gap) in [(Pick::Low, 50u16), (Pick::High, 80)] {
        let mut w = World::new([0; 3]);
        w.pick = pick;
        let mut g = spawned(&mut w);
        for now in 0..=200u16 {
            w.now = now;
            g.pain(now, [0; 3], &mut w);
        }
        let cries = w.sounds(GargSound::Pain);
        assert_eq!(cries[1] - cries[0], gap);
        assert_eq!(cries[2] - cries[1], gap);
    }
}

#[test]
fn death_brings_rising_fireballs_0_6_s_apart_then_gibs_at_1_6_s() {
    // The tick that notices the death only records it, so the first
    // (magnitude 60) fireball of the timeline never shows: three follow.
    // Recorded from ac83da7 behaviour.
    let mut w = World::new([5000, 0, 0]);
    w.pos = [100, 20, -40];
    let mut g = spawned(&mut w);
    for now in 0..=60u16 {
        w.now = now;
        let mut wi = world_inputs(&w);
        wi.dead = now >= 10;
        g.tick_world(&wi, &mut w);
    }
    assert_eq!(
        w.events,
        [
            (10, Ev::LoopStop),
            (22, Ev::Fx([30, 67, -110], 100)),
            (22, Ev::Sound(GargSound::Explosion, [100, 20, -40])),
            (34, Ev::Fx([30, 82, -110], 140)),
            (34, Ev::Sound(GargSound::Explosion, [100, 20, -40])),
            (42, Ev::Gib([100, 20, -40])),
            (46, Ev::Fx([30, 97, -110], 180)),
            (46, Ev::Sound(GargSound::Explosion, [100, 20, -40])),
        ]
    );
}

#[test]
fn without_a_target_it_idles_and_drops_the_target() {
    let mut w = World::new([200, 0, 0]);
    let mut g = spawned(&mut w);
    let mut i = inputs(&w);
    i.aim = None;
    g.think(&i, &MODEL, &mut w);
    assert_eq!(
        w.events,
        [(0, Ev::State(GargState::Idle)), (0, Ev::ClearTarget)]
    );
}

/// Recorded from ac83da7 behaviour: the player circles the gargantua at
/// shifting range for twenty seconds, ducking behind a wall now and then,
/// drawing from the game's generator. The state at the end of each tick as
/// runs, every other event except beams, which are counted.
#[test]
fn golden_duel() {
    let mut w = World::new([600, 0, 0]);
    w.planted = false;
    w.pick = Pick::Game(Lcg(Lcg::GAME_SEED));
    let mut g = spawned(&mut w);
    let mut states: Vec<(u16, GargState)> = Vec::new();
    for now in 0..400u16 {
        let t = now as i32;
        let r = 150 + (t * 7) % 400;
        let a = ((t * 9) & 0xfff) as u16;
        w.player = [
            psx_math::sincos::sin_q12((a + 1024) & 0xfff) * r >> 12,
            0,
            psx_math::sincos::sin_q12(a) * r >> 12,
        ];
        w.now = now;
        let mut i = inputs(&w);
        i.visible = (now / 50) % 4 != 3;
        g.think(&i, &MODEL, &mut w);
        let wi = world_inputs(&w);
        g.tick_world(&wi, &mut w);
        if let Some(s) = w.state {
            if states.last().map(|x| x.1) != Some(s) {
                states.push((now, s));
            }
        }
    }
    let mut beams = 0;
    let events: Vec<(u16, Ev)> = w
        .events
        .iter()
        .copied()
        .filter(|e| match e.1 {
            Ev::Beam(..) => {
                beams += 1;
                false
            }
            Ev::State(_) => false,
            _ => true,
        })
        .collect();
    assert_eq!(
        states,
        [
            (0, GargState::Move),
            (7, GargState::Attack),
            (37, GargState::Idle),
            (38, GargState::Move),
            (232, GargState::Attack),
            (277, GargState::Idle),
            (278, GargState::Move),
            (299, GargState::Attack),
            (328, GargState::Idle),
            (329, GargState::Move)
        ]
    );
    assert_eq!(
        events,
        [
            (41, Ev::Shake(2, 20)),
            (41, Ev::Sound(GargSound::Step, [165, 0, 40])),
            (52, Ev::Shake(3, 20)),
            (52, Ev::Sound(GargSound::Step, [289, 0, 184])),
            (63, Ev::Shake(3, 20)),
            (63, Ev::Sound(GargSound::Step, [264, 0, 228])),
            (74, Ev::Shake(3, 20)),
            (74, Ev::Sound(GargSound::Step, [138, 0, 204])),
            (85, Ev::Shake(3, 20)),
            (85, Ev::Sound(GargSound::Step, [137, 0, 294])),
            (96, Ev::Shake(3, 20)),
            (96, Ev::Sound(GargSound::Step, [107, 0, 399])),
            (107, Ev::Shake(3, 20)),
            (107, Ev::Sound(GargSound::Step, [61, 0, 476])),
            (118, Ev::Shake(2, 20)),
            (118, Ev::Sound(GargSound::Step, [4, 0, 488])),
            (129, Ev::Shake(3, 20)),
            (129, Ev::Sound(GargSound::Step, [-23, 0, 299])),
            (140, Ev::Shake(3, 20)),
            (140, Ev::Sound(GargSound::Step, [-100, 0, 295])),
            (151, Ev::Shake(3, 20)),
            (151, Ev::Sound(GargSound::Step, [-176, 0, 342])),
            (162, Ev::Shake(3, 20)),
            (162, Ev::Sound(GargSound::Step, [-279, 0, 376])),
            (173, Ev::Shake(2, 20)),
            (173, Ev::Sound(GargSound::Step, [-375, 0, 372])),
            (184, Ev::Shake(3, 20)),
            (184, Ev::Sound(GargSound::Step, [-251, 0, 226])),
            (195, Ev::Shake(3, 20)),
            (195, Ev::Sound(GargSound::Step, [-263, 0, 139])),
            (206, Ev::Shake(3, 20)),
            (206, Ev::Sound(GargSound::Step, [-352, 0, 122])),
            (217, Ev::Shake(3, 20)),
            (217, Ev::Sound(GargSound::Step, [-449, 0, 78])),
            (228, Ev::Shake(3, 20)),
            (228, Ev::Sound(GargSound::Step, [-531, 0, 12])),
            (232, Ev::Sound(GargSound::FlameOn, [-489, 0, -1])),
            (238, Ev::Hurt(1, [-378, 86, -67])),
            (240, Ev::Hurt(2, [-378, 86, -67])),
            (242, Ev::Hurt(3, [-378, 86, -67])),
            (244, Ev::Hurt(3, [-378, 86, -67])),
            (246, Ev::Hurt(3, [-378, 86, -67])),
            (248, Ev::Hurt(3, [-378, 86, -67])),
            (250, Ev::Hurt(3, [-378, 86, -67])),
            (252, Ev::Hurt(3, [-378, 86, -67])),
            (254, Ev::Hurt(3, [-378, 86, -67])),
            (256, Ev::Hurt(3, [-378, 86, -67])),
            (258, Ev::Hurt(3, [-378, 86, -67])),
            (260, Ev::Hurt(3, [-378, 86, -67])),
            (262, Ev::Hurt(1, [-378, 86, -67])),
            (277, Ev::LoopMute),
            (277, Ev::Sound(GargSound::FlameOff, [-489, 0, -1])),
            (285, Ev::Shake(2, 20)),
            (285, Ev::Sound(GargSound::Step, [-456, 0, -120])),
            (296, Ev::Shake(3, 20)),
            (296, Ev::Sound(GargSound::Step, [-276, 0, -142])),
            (326, Ev::Shake(8, 40)),
            (326, Ev::Sound(GargSound::Stomp, [-226, 0, -157])),
            (328, Ev::Dust([-191, 4, -157])),
            (330, Ev::Dust([-191, 3, -158])),
            (332, Ev::Dust([-191, 3, -159])),
            (334, Ev::Dust([-190, 3, -162])),
            (336, Ev::Dust([-188, 2, -166])),
            (337, Ev::Shake(2, 20)),
            (337, Ev::Sound(GargSound::Step, [-162, 0, -279])),
            (338, Ev::Dust([-186, 1, -172])),
            (340, Ev::Dust([-183, -1, -180])),
            (342, Ev::Dust([-179, -2, -191])),
            (344, Ev::Dust([-174, -5, -205])),
            (346, Ev::Dust([-168, -8, -222])),
            (348, Ev::Shake(3, 20)),
            (348, Ev::Sound(GargSound::Step, [-65, 0, -294])),
            (348, Ev::Dust([-160, -11, -242])),
            (350, Ev::Dust([-151, -16, -267])),
            (352, Ev::Dust([-140, -21, -295])),
            (354, Ev::Dust([-127, -26, -329])),
            (356, Ev::Dust([-113, -33, -367])),
            (358, Ev::Dust([-97, -41, -411])),
            (359, Ev::Shake(3, 20)),
            (359, Ev::Sound(GargSound::Step, [57, 0, -249])),
            (360, Ev::Dust([-78, -49, -461])),
            (362, Ev::Shake(3, 20)),
            (362, Ev::Sound(GargSound::Step, [69, 0, -262])),
            (362, Ev::Dust([-57, -59, -516])),
            (364, Ev::Dust([-34, -70, -578])),
            (366, Ev::Dust([-8, -82, -647])),
            (368, Ev::Dust([20, -95, -723])),
            (370, Ev::Dust([52, -109, -806])),
            (372, Ev::Dust([86, -125, -897])),
            (373, Ev::Shake(3, 20)),
            (373, Ev::Sound(GargSound::Step, [137, 0, -318])),
            (374, Ev::Dust([123, -142, -996])),
            (376, Ev::Dust([164, -161, -1104])),
            (384, Ev::Shake(3, 20)),
            (384, Ev::Sound(GargSound::Step, [217, 0, -355])),
            (395, Ev::Shake(3, 20)),
            (395, Ev::Sound(GargSound::Step, [323, 0, -378]))
        ]
    );
    assert_eq!(beams, 88);
    assert_eq!(w.pos, [377, 0, -380]);
}

/// Recorded from ac83da7 behaviour: one flame sweep at a player who stands
/// half behind cover 250 away, then steps out to 380; every beam.
#[test]
fn golden_flame_sweep() {
    let mut w = World::new([250, -40, 60]);
    w.walls.push(Wall { axis: 2, at: 50 });
    let mut g = spawned(&mut w);
    let mut log = Vec::new();
    for now in 0..=140u16 {
        if now == 80 {
            w.player = [380, -20, -30];
        }
        w.pos = [0; 3];
        run(&mut g, &mut w, now, now);
    }
    for e in &w.events {
        if matches!(e.1, Ev::Beam(..) | Ev::Hurt(..) | Ev::Sound(..)) {
            log.push(*e);
        }
    }
    assert_eq!(
        log,
        [
            (10, Ev::Sound(GargSound::Step, [0, 0, 0])),
            (21, Ev::Sound(GargSound::Step, [0, 0, 0])),
            (32, Ev::Sound(GargSound::Step, [0, 0, 0])),
            (40, Ev::Sound(GargSound::FlameOn, [0, 0, 0])),
            (42, Ev::Beam([111, 86, -66], [436, 62, -21], false)),
            (42, Ev::Beam([111, 86, -66], [241, 76, -48], true)),
            (42, Ev::Beam([110, 88, 64], [435, 64, 109], false)),
            (42, Ev::Beam([110, 88, 64], [240, 78, 82], true)),
            (44, Ev::Beam([111, 86, -66], [428, 39, 10], false)),
            (44, Ev::Beam([111, 86, -66], [237, 67, -36], true)),
            (44, Ev::Beam([110, 88, 64], [427, 41, 140], false)),
            (44, Ev::Beam([110, 88, 64], [236, 69, 94], true)),
            (46, Ev::Beam([111, 86, -66], [424, 16, 9], false)),
            (46, Ev::Beam([111, 86, -66], [236, 58, -36], true)),
            (46, Ev::Beam([110, 88, 64], [423, 18, 139], false)),
            (46, Ev::Beam([110, 88, 64], [235, 60, 94], true)),
            (48, Ev::Beam([111, 86, -66], [418, -6, 8], false)),
            (48, Ev::Beam([111, 86, -66], [234, 49, -37], true)),
            (48, Ev::Beam([110, 88, 64], [417, -4, 138], false)),
            (48, Ev::Beam([110, 88, 64], [233, 51, 93], true)),
            (50, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (50, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (50, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (50, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (52, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (52, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (52, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (52, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (54, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (54, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (54, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (54, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (56, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (56, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (56, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (56, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (58, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (58, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (58, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (58, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (60, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (60, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (60, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (60, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (62, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (62, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (62, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (62, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (64, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (64, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (64, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (64, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (66, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (66, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (66, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (66, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (68, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (68, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (68, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (68, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (70, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (70, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (70, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (70, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (72, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (72, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (72, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (72, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (74, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (74, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (74, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (74, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (76, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (76, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (76, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (76, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (78, Ev::Beam([111, 86, -66], [413, -24, 7], false)),
            (78, Ev::Beam([111, 86, -66], [232, 42, -37], true)),
            (78, Ev::Beam([110, 88, 64], [412, -22, 137], false)),
            (78, Ev::Beam([110, 88, 64], [231, 44, 93], true)),
            (80, Ev::Beam([111, 86, -66], [364, 16, -42], false)),
            (80, Ev::Beam([111, 86, -66], [211, 58, -57], true)),
            (80, Ev::Hurt(3, [111, 86, -66])),
            (80, Ev::Beam([110, 88, 64], [426, 0, 95], false)),
            (80, Ev::Beam([110, 88, 64], [236, 53, 76], true)),
            (82, Ev::Beam([111, 86, -66], [434, 21, -80], false)),
            (82, Ev::Beam([111, 86, -66], [240, 60, -72], true)),
            (82, Ev::Hurt(3, [111, 86, -66])),
            (82, Ev::Beam([110, 88, 64], [433, 23, 50], false)),
            (82, Ev::Beam([110, 88, 64], [239, 62, 58], true)),
            (84, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (84, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (84, Ev::Hurt(0, [111, 86, -66])),
            (84, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (84, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (86, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (86, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (86, Ev::Hurt(0, [111, 86, -66])),
            (86, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (86, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (88, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (88, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (88, Ev::Hurt(0, [111, 86, -66])),
            (88, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (88, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (90, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (90, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (90, Ev::Hurt(0, [111, 86, -66])),
            (90, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (90, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (92, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (92, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (92, Ev::Hurt(1, [111, 86, -66])),
            (92, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (92, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (94, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (94, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (94, Ev::Hurt(0, [111, 86, -66])),
            (94, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (94, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (96, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (96, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (96, Ev::Hurt(0, [111, 86, -66])),
            (96, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (96, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (98, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (98, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (98, Ev::Hurt(0, [111, 86, -66])),
            (98, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (98, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (100, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (100, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (100, Ev::Hurt(0, [111, 86, -66])),
            (100, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (100, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (102, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (102, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (102, Ev::Hurt(1, [111, 86, -66])),
            (102, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (102, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (104, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (104, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (104, Ev::Hurt(0, [111, 86, -66])),
            (104, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (104, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (106, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (106, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (106, Ev::Hurt(0, [111, 86, -66])),
            (106, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (106, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (108, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (108, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (108, Ev::Hurt(0, [111, 86, -66])),
            (108, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (108, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (110, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (110, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (110, Ev::Hurt(0, [111, 86, -66])),
            (110, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (110, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (112, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (112, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (112, Ev::Hurt(1, [111, 86, -66])),
            (112, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (112, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (114, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (114, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (114, Ev::Hurt(0, [111, 86, -66])),
            (114, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (114, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (116, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (116, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (116, Ev::Hurt(0, [111, 86, -66])),
            (116, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (116, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (118, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (118, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (118, Ev::Hurt(0, [111, 86, -66])),
            (118, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (118, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (120, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (120, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (120, Ev::Hurt(0, [111, 86, -66])),
            (120, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (120, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (122, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (122, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (122, Ev::Hurt(1, [111, 86, -66])),
            (122, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (122, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (124, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (124, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (124, Ev::Hurt(0, [111, 86, -66])),
            (124, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (124, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (126, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (126, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (126, Ev::Hurt(0, [111, 86, -66])),
            (126, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (126, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (128, Ev::Beam([111, 86, -66], [434, 25, -93], false)),
            (128, Ev::Beam([111, 86, -66], [240, 61, -77], true)),
            (128, Ev::Hurt(0, [111, 86, -66])),
            (128, Ev::Beam([110, 88, 64], [277, 56, 50], false)),
            (128, Ev::Beam([110, 88, 64], [176, 75, 58], true)),
            (130, Ev::Sound(GargSound::FlameOff, [0, 0, 0]))
        ]
    );
}
