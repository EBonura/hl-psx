//! Nihilanth (HV-06): spheres and healing, the height offset, flight, the
//! recharge flow, attack choice, zap and teleport balls, the damage floor,
//! pain cadence and death.

use crate::behaviour::setpiece_kit::{trace_walls, Lcg, Pick, Wall};
use crate::nihilanth_logic::{NihBrain, NihInputs, NihModel, NihSeq, NihSound, NihWorld};
use crate::setpiece_math::TraceHit;

const MODEL: NihModel = NihModel {
    float_len: 75,
    attack_len: 125,
    zap_event: 88,
    tele_event: 98,
    recharge_frames: [10, 13, 16, 21, 24, 27, 30, 33, 36, 39, 39],
    brain_ahead: 100,
    brain_up: 277,
    brain_radius: 100,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    Sound(NihSound, [i32; 3]),
    Hurt(u16, Option<[i32; 3]>),
    Draw(u8),
    Teleport(u8),
    Die,
}

struct World {
    pick: Pick,
    walls: Vec<Wall>,
    pos: [i32; 3],
    yaw: u16,
    shown: u8,
    rechargers: Vec<(u8, u16, [i32; 3])>,
    gone: Vec<u16>,
    teleports: Vec<u8>,
    now: u16,
    trails: Vec<(u16, [i32; 3], [i32; 3], bool)>,
    events: Vec<(u16, Ev)>,
}

impl World {
    fn new() -> Self {
        Self {
            pick: Pick::Low,
            walls: Vec::new(),
            pos: [0, 0, 0],
            yaw: 0,
            shown: 255,
            rechargers: Vec::new(),
            gone: Vec::new(),
            teleports: Vec::new(),
            now: 0,
            trails: Vec::new(),
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
}

impl NihWorld for World {
    fn random_below(&mut self, n: u32) -> u32 {
        self.pick.below(n)
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        trace_walls(&self.walls, from, to)
    }
    fn prop_pos(&mut self) -> [i32; 3] {
        self.pos
    }
    fn set_pose(&mut self, pos: [i32; 3], yaw: u16) {
        self.pos = pos;
        self.yaw = yaw;
    }
    fn set_shown_health(&mut self, health: u8) {
        self.shown = health;
    }
    fn die(&mut self) {
        self.shown = 0;
        self.ev(Ev::Die);
    }
    fn recharger(&mut self, level: u8) -> Option<(u16, [i32; 3])> {
        self.rechargers
            .iter()
            .find(|r| r.0 == level && !self.gone.contains(&r.1))
            .map(|r| (r.1, r.2))
    }
    fn recharger_origin(&mut self, id: u16) -> [i32; 3] {
        self.rechargers.iter().find(|r| r.1 == id).unwrap().2
    }
    fn recharger_gone(&mut self, id: u16) -> bool {
        self.gone.contains(&id)
    }
    fn fire_draw(&mut self, level: u8) {
        self.ev(Ev::Draw(level));
    }
    fn has_teleport(&mut self, n: u8) -> bool {
        self.teleports.contains(&n)
    }
    fn teleport_player(&mut self, n: u8) {
        self.ev(Ev::Teleport(n));
    }
    fn hurt_player(&mut self, damage: u16, from: Option<[i32; 3]>) {
        self.ev(Ev::Hurt(damage, from));
    }
    fn ball_trail(&mut self, from: [i32; 3], to: [i32; 3], teleport: bool) {
        self.trails.push((self.now, from, to, teleport));
    }
    fn sound(&mut self, sound: NihSound, at: [i32; 3]) {
        self.ev(Ev::Sound(sound, at));
    }
}

/// Him at the origin facing +z, 1000 skill health, the room 0..2000 high.
fn bound(w: &mut World, now: u16) -> NihBrain {
    let mut n = NihBrain::new();
    n.bind(1000, w.pos[1], 0, Some(0), Some(2000), &MODEL, now);
    n
}

fn inputs(w: &World, player: [i32; 3]) -> NihInputs {
    NihInputs {
        now: w.now,
        player_pos: player,
        player_alive: true,
        shown_alive: w.shown != 0,
        zap_damage: 10,
    }
}

fn run(n: &mut NihBrain, w: &mut World, player: [i32; 3], from: u16, to: u16) {
    for now in from..=to {
        w.now = now;
        let i = inputs(w, player);
        n.tick(&i, &MODEL, w);
    }
}

#[test]
fn he_starts_with_twenty_spheres_and_full_health() {
    let mut w = World::new();
    w.pos = [10, 300, 20];
    let n = bound(&mut w, 0);
    assert_eq!(n.spheres(w.pos), Some((20, [10, 540, 20])));
    assert_eq!(n.health(), 1000);
    assert_eq!(n.sequence(0), Some((NihSeq::Float, false, 75, 0)));
}

#[test]
fn while_hurt_he_absorbs_a_sphere_per_think_for_a_twentieth_of_his_health() {
    let mut w = World::new();
    let mut n = bound(&mut w, 0);
    assert_eq!(n.damage(120, 0, w.pos, &mut w), 224);
    assert_eq!(n.health(), 880);
    run(&mut n, &mut w, [0, -5000, 0], 0, 0);
    assert_eq!(n.health(), 930);
    assert_eq!(n.spheres(w.pos).unwrap().0, 19);
    run(&mut n, &mut w, [0, -5000, 0], 1, 4);
    assert_eq!(n.health(), 1000);
    assert_eq!(n.spheres(w.pos).unwrap().0, 17);
    run(&mut n, &mut w, [0, -5000, 0], 5, 8);
    assert_eq!(n.spheres(w.pos).unwrap().0, 17);
    assert_eq!(w.shown, 255);
}

#[test]
fn he_holds_the_players_height_plus_an_offset_that_grows_while_unseen() {
    let mut w = World::new();
    let mut n = bound(&mut w, 0);
    let player = [0, 100, 600];
    run(&mut n, &mut w, player, 0, 0);
    assert_eq!(n.flight().5, 612);
    // Twenty unseen thinks: the offset grows 10 a think.
    w.walls.push(Wall { axis: 2, at: 300 });
    run(&mut n, &mut w, player, 1, 40);
    assert_eq!(n.flight().6, 712);
    w.walls.clear();
    run(&mut n, &mut w, player, 41, 42);
    assert_eq!(n.flight().5, 812);
    // It stops at 1000.
    w.walls.push(Wall { axis: 2, at: 300 });
    run(&mut n, &mut w, player, 43, 200);
    assert_eq!(n.flight().6, 1000);
}

#[test]
fn the_wanted_height_stays_between_the_min_and_max_markers() {
    let mut w = World::new();
    let mut n = NihBrain::new();
    n.bind(1000, 0, 0, Some(200), Some(700), &MODEL, 0);
    run(&mut n, &mut w, [0, 900, 600], 0, 0);
    assert_eq!(n.flight().5, 700);
    run(&mut n, &mut w, [0, -900, 600], 1, 2);
    assert_eq!(n.flight().5, 200);
}

#[test]
fn he_turns_toward_the_player_six_degrees_a_step_with_two_percent_decay() {
    let mut w = World::new();
    let mut n = bound(&mut w, 0);
    // Facing +z; the player off to +x.
    run(&mut n, &mut w, [600, 512, 0], 0, 0);
    // The first think still aims at the bound heading (+z): no side.
    let first = n.flight().4;
    run(&mut n, &mut w, [600, 512, 0], 1, 2);
    let second = n.flight().4;
    assert_eq!(first.abs(), 6 * 182 * 98 / 100);
    assert_eq!(second, (first + 6 * 182 * first.signum()) * 98 / 100);
    // The rate stops growing at 180 degrees a second.
    for k in 3..400u16 {
        run(&mut n, &mut w, [600 * (k as i32 % 2 * 2 - 1), 512, 0], k, k);
    }
    assert!(n.flight().4.abs() <= 180 * 182);
}

#[test]
fn thrust_steps_ten_toward_the_height_two_seconds_out() {
    let mut w = World::new();
    let mut n = bound(&mut w, 0);
    // Player high above: wanted 1512; thrust climbs 10 a think up to 100.
    let p = [0, 1000, 600];
    run(&mut n, &mut w, p, 0, 0);
    assert_eq!(n.flight().2, 160);
    let mut last = 160;
    for now in 1..=200u16 {
        run(&mut n, &mut w, p, now, now);
        let f = n.flight().2;
        assert!(matches!(f - last, -160 | 0 | 160), "{last} -> {f}");
        assert!(f.abs() <= 1600 + 160);
        last = f;
    }
    // Vertical speed keeps 99.5% each think.
    let (_, vz, f, ..) = n.flight();
    run(&mut n, &mut w, p, 41, 42);
    assert_eq!(n.flight().1, (vz + f) * 995 / 1000);
}

#[test]
fn below_half_health_he_flies_to_his_recharger_and_recharges_within_128() {
    let mut w = World::new();
    w.rechargers = vec![(1, 7, [500, 400, 0])];
    let mut n = bound(&mut w, 0);
    n.damage(200, 0, w.pos, &mut w);
    n.damage(200, 1, w.pos, &mut w);
    n.damage(200, 2, w.pos, &mut w);
    // 400 left, and he absorbs spheres meanwhile; float ends at 75.
    run(&mut n, &mut w, [0, -5000, 0], 0, 80);
    let f = n.flight();
    assert_eq!(f.5, 400, "heads for the recharger's height");
    // Once within 128 of it, the next sequence is the recharge.
    let mut started = None;
    for now in 81..=600u16 {
        run(&mut n, &mut w, [0, -5000, 0], now, now);
        if started.is_none() && !w.ticks_of(|e| *e == Ev::Draw(1)).is_empty() {
            started = Some(now);
            assert_eq!(n.sequence(now).unwrap().0, NihSeq::Recharge);
            assert!((n.height() - 400).abs() < 128);
        }
    }
    let started = started.expect("recharged");
    assert_eq!(
        w.ticks_of(|e| matches!(e, Ev::Sound(NihSound::Recharge, _))),
        [started]
    );
}

#[test]
fn a_destroyed_crystal_ends_the_recharge_and_a_missing_one_moves_up_a_level() {
    let mut w = World::new();
    // No level 1-9 rechargers at all: each search moves a level up, and
    // past level 9 his head opens.
    let mut n = bound(&mut w, 0);
    n.damage(200, 0, w.pos, &mut w);
    n.damage(200, 1, w.pos, &mut w);
    n.damage(200, 2, w.pos, &mut w);
    assert_eq!(n.irritation(), 0);
    run(&mut n, &mut w, [0, -5000, 0], 0, 2000);
    assert_eq!(n.irritation(), 2);
    // Killing the crystal mid-recharge drops it.
    let mut w = World::new();
    w.rechargers = vec![(1, 7, [0, 0, 0])];
    let mut n = bound(&mut w, 0);
    for _ in 0..3 {
        n.damage(200, 0, w.pos, &mut w);
    }
    run(&mut n, &mut w, [0, -5000, 0], 0, 152);
    assert_eq!(n.sequence(152).unwrap().0, NihSeq::Recharge);
    w.gone.push(7);
    run(&mut n, &mut w, [0, -5000, 0], 153, 154);
    // With the recharger gone he stops absorbing toward it; the sequence
    // plays out and the next pick is no longer a recharge at that marker.
    run(&mut n, &mut w, [0, -5000, 0], 155, 400);
    assert_eq!(w.ticks_of(|e| *e == Ev::Draw(1)).len(), 1);
}

/// Bring him level with the player (the player's height plus the offset,
/// so he can attack) and start the fight.
fn angry(w: &mut World, player: [i32; 3]) -> NihBrain {
    w.pos = [0, player[1] + 512, 0];
    let mut n = bound(w, 0);
    n.command_on();
    n
}

#[test]
fn he_attacks_only_once_triggered_with_the_player_close_seen_and_in_front() {
    let player = [0, 0, 500];
    let mut w = World::new();
    w.pos = [0, 512, 0];
    let mut n = bound(&mut w, 0);
    run(&mut n, &mut w, player, 0, 300);
    assert!(w.trails.is_empty(), "not triggered");
    let mut w = World::new();
    let mut n = angry(&mut w, player);
    run(&mut n, &mut w, player, 0, 300);
    assert!(!w.trails.is_empty());
}

#[test]
fn attack_one_fires_a_second_of_zap_pairs_every_fifth_of_a_second() {
    let player = [0, 0, 3000];
    let mut w = World::new();
    w.pick = Pick::Low; // the coin picks attack1
    let mut n = angry(&mut w, player);
    run(&mut n, &mut w, player, 0, 76);
    assert_eq!(n.sequence(76).unwrap().0, NihSeq::Attack1);
    run(&mut n, &mut w, player, 77, 200);
    // Launches show as each new ball's first trail.
    let launches: Vec<u16> = w
        .trails
        .iter()
        .filter(|t| t.1[1] == 512 + 300 - 150)
        .map(|t| t.0)
        .collect();
    // Launched on the event think (76 + 88); a trail shows from the next tick.
    let first = launches[0];
    assert_eq!(first, 76 + 88 + 1);
    assert_eq!(
        launches.iter().filter(|&&t| t == first).count(),
        2,
        "a pair"
    );
    let distinct: std::collections::BTreeSet<u16> = launches.iter().copied().collect();
    assert_eq!(
        distinct.into_iter().collect::<Vec<_>>(),
        [first, first + 4, first + 8]
    );
}

#[test]
fn attack_two_sends_a_teleport_ball_only_when_the_map_has_a_target() {
    let player = [0, 0, 500];
    let mut w = World::new();
    w.pick = Pick::High; // the coin picks attack2
    w.teleports = vec![1];
    let mut n = angry(&mut w, player);
    run(&mut n, &mut w, player, 0, 300);
    assert!(w
        .events
        .contains(&(76 + 98, Ev::Sound(NihSound::Tele, [0, 812, 0]))));
    assert_eq!(w.ticks_of(|e| *e == Ev::Teleport(1)).len(), 1);
    // Without one the counter moves on and he volleys instead.
    let mut w = World::new();
    w.pick = Pick::High;
    let mut n = angry(&mut w, player);
    run(&mut n, &mut w, player, 0, 200);
    assert!(w.ticks_of(|e| matches!(e, Ev::Teleport(_))).is_empty());
    assert!(w
        .events
        .contains(&(76 + 98, Ev::Sound(NihSound::Ball, [0, 812, 0]))));
    assert!(w.trails.iter().all(|t| !t.3));
}

#[test]
fn zap_balls_speed_up_a_fifth_a_tick_and_shock_within_256() {
    // Held down by n_max so his hands (150 below his 300-up head) are 250
    // above the player: small velocity components never grow under the
    // integer fifth, so a ball barely descends and only shocks a player
    // near its own height.
    let player = [0, -100, 1500];
    let mut w = World::new();
    let mut n = NihBrain::new();
    n.bind(1000, 0, 0, Some(-100), Some(0), &MODEL, 0);
    n.command_on();
    run(&mut n, &mut w, player, 0, 400);
    let ball: Vec<i32> = w
        .trails
        .iter()
        .filter(|t| t.1[0] == 100 && t.0 >= 164)
        .map(|t| t.2[2] - t.1[2])
        .take(5)
        .collect();
    // Starts near 10 a tick (200 u/s) and grows by a fifth a tick.
    assert_eq!(ball[0], 10);
    assert!(
        ball.windows(2)
            .all(|p| p[1] == p[0] * 6 / 5 || p[1] == p[0]),
        "{ball:?}"
    );
    let shocks = w.ticks_of(|e| matches!(e, Ev::Hurt(10, Some(_))));
    assert!(!shocks.is_empty());
}

#[test]
fn sequences_speed_up_as_he_weakens() {
    let mut w = World::new();
    let mut n2 = bound(&mut w, 0);
    n2.damage(250, 0, w.pos, &mut w);
    n2.damage(250, 0, w.pos, &mut w);
    run(&mut n2, &mut w, [0, -5000, 0], 0, 76);
    let (_, _, len, _) = n2.sequence(76).unwrap();
    // The health at the switch decides the rate: 2 - h / max.
    let h = n2.health();
    assert_eq!(len as i32, 75 * 4096 / (8192 - h * 4096 / 1000));
}

#[test]
fn a_killing_blow_leaves_one_unless_it_hit_his_open_brain_hard() {
    let mut w = World::new();
    let mut n = bound(&mut w, 0);
    assert_eq!(n.damage(255, 0, w.pos, &mut w), 189);
    for _ in 0..3 {
        n.damage(255, 0, w.pos, &mut w);
    }
    assert_eq!(n.health(), 1);
    assert_eq!(n.damage(255, 0, w.pos, &mut w), 1);
    assert_eq!(n.health(), 1);
    assert!(!n.dying());
    // Open head (no rechargers: run until it opens), a hard shot into it.
    let mut w = World::new();
    let mut n = bound(&mut w, 0);
    for _ in 0..4 {
        n.damage(255, 0, w.pos, &mut w);
    }
    run(&mut n, &mut w, [0, -5000, 0], 0, 2000);
    assert_eq!(n.irritation(), 2);
    while n.health() > 1 {
        n.damage(255, 0, w.pos, &mut w);
    }
    // A shot through the brain (100 ahead, 277 up) for 3.
    let pos = w.pos;
    n.note_shot(
        pos,
        [pos[0] - 1000, pos[1] + 277, pos[2] + 100],
        [pos[0] + 1000, pos[1] + 277, pos[2] + 100],
        &MODEL,
    );
    assert_eq!(n.damage(3, 0, pos, &mut w), 1);
    assert_eq!(n.irritation(), 3);
    assert!(n.dying());
}

#[test]
fn pain_sounds_come_every_two_to_five_seconds_laughing_while_strong() {
    for (pick, gap) in [(Pick::Low, 40u16), (Pick::High, 100)] {
        let mut w = World::new();
        w.pick = pick;
        let mut n = bound(&mut w, 0);
        for now in 0..=250u16 {
            w.now = now;
            n.damage(1, now, w.pos, &mut w);
        }
        let laughs = w.ticks_of(|e| matches!(e, Ev::Sound(NihSound::Laugh, _)));
        assert_eq!(laughs[1] - laughs[0], gap);
    }
    // Below half with his head shut: silent.
    let mut w = World::new();
    let mut n = bound(&mut w, 0);
    n.damage(200, 0, w.pos, &mut w);
    n.damage(200, 0, w.pos, &mut w);
    n.damage(200, 0, w.pos, &mut w);
    w.events.clear();
    n.damage(1, 200, w.pos, &mut w);
    assert!(w.events.is_empty());
}

#[test]
fn dying_he_rises_to_the_max_marker_and_then_dies() {
    let mut w = World::new();
    w.pos = [0, 600, 0];
    let mut n = NihBrain::new();
    n.bind(10, 600, 0, Some(0), Some(900), &MODEL, 0);
    n.command_on();
    // Open his head the quick way: three killing hits are floored...
    n.damage(50, 0, w.pos, &mut w);
    assert!(!n.dying());
    // ...so fake an open head by running out the levels.
    run(&mut n, &mut w, [0, -5000, 0], 0, 2000);
    let pos = w.pos;
    n.note_shot(
        pos,
        [pos[0], pos[1] + 277, pos[2] - 500],
        [pos[0], pos[1] + 277, pos[2] + 500],
        &MODEL,
    );
    n.damage(9, 2001, pos, &mut w);
    n.note_shot(
        pos,
        [pos[0], pos[1] + 277, pos[2] - 500],
        [pos[0], pos[1] + 277, pos[2] + 500],
        &MODEL,
    );
    n.damage(9, 2001, pos, &mut w);
    assert!(n.dying());
    assert!(w
        .events
        .iter()
        .any(|e| matches!(e.1, Ev::Sound(NihSound::Die, _))));
    run(&mut n, &mut w, [0, -5000, 0], 2002, 3000);
    let died = w.ticks_of(|e| *e == Ev::Die);
    assert_eq!(died.len(), 1);
    assert!((n.height() - 900).abs() < 16 + 16);
}

/// Recorded from ac83da7 behaviour: a triggered fight with the player in
/// the open below him, two rechargers, a teleport target, periodic hits
/// and one shot into his brain, drawing from the game's generator for
/// thirty seconds. Height every 20 ticks, every event, the trail count.
#[test]
fn golden_fight() {
    let player = [200, 0, 400];
    let mut w = World::new();
    w.pick = Pick::Game(Lcg(Lcg::GAME_SEED));
    w.rechargers = vec![(1, 3, [-600, 700, 0]), (2, 4, [600, 300, 600])];
    w.teleports = vec![2];
    let mut n = angry(&mut w, player);
    let mut heights = Vec::new();
    for now in 0..600u16 {
        w.now = now;
        if now % 37 == 0 {
            let pos = w.pos;
            n.damage(90, now, pos, &mut w);
        }
        if now == 450 {
            w.gone.push(3);
        }
        let i = inputs(&w, player);
        n.tick(&i, &MODEL, &mut w);
        if now % 20 == 0 {
            heights.push((now, n.height(), n.spheres(w.pos).map(|s| s.0)));
        }
    }
    assert_eq!(
        heights,
        [
            (0, 512, Some(19)),
            (20, 512, Some(18)),
            (40, 512, Some(16)),
            (60, 512, Some(16)),
            (80, 512, Some(14)),
            (100, 512, Some(14)),
            (120, 512, Some(12)),
            (140, 512, Some(12)),
            (160, 512, Some(10)),
            (180, 512, Some(10)),
            (200, 512, Some(8)),
            (220, 527, Some(8)),
            (240, 586, Some(6)),
            (260, 635, Some(5)),
            (280, 662, Some(4)),
            (300, 678, Some(2)),
            (320, 687, Some(4)),
            (340, 691, Some(4)),
            (360, 696, Some(5)),
            (380, 700, Some(4)),
            (400, 700, Some(4)),
            (420, 700, Some(2)),
            (440, 700, Some(3)),
            (460, 698, Some(3)),
            (480, 660, Some(4)),
            (500, 599, Some(4)),
            (520, 562, Some(2)),
            (540, 542, Some(2)),
            (560, 528, Some(0)),
            (580, 525, Some(0))
        ]
    );
    assert_eq!(
        w.events,
        [
            (0, Ev::Sound(NihSound::Laugh, [0, 512, 0])),
            (74, Ev::Sound(NihSound::Laugh, [0, 512, 0])),
            (164, Ev::Sound(NihSound::Ball, [0, 812, 0])),
            (179, Ev::Hurt(10, Some([114, 155, 261]))),
            (183, Ev::Hurt(10, Some([114, 155, 261]))),
            (185, Ev::Sound(NihSound::Laugh, [0, 512, 0])),
            (187, Ev::Hurt(10, Some([114, 155, 261]))),
            (195, Ev::Hurt(10, Some([114, 155, 261]))),
            (278, Ev::Sound(NihSound::Recharge, [0, 659, 0])),
            (278, Ev::Draw(1)),
            (296, Ev::Sound(NihSound::Laugh, [0, 674, 0])),
            (407, Ev::Sound(NihSound::Laugh, [0, 700, 0])),
            (481, Ev::Sound(NihSound::Laugh, [0, 660, 0])),
            (592, Ev::Sound(NihSound::Laugh, [0, 521, 0]))
        ]
    );
    assert_eq!(w.trails.len(), 1349);
}
