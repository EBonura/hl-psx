//! Apache (HV-05): hunting goal speed, corner advance and player pursuit,
//! the per-step flight model, the chin gun gate, rocket pairs and reload,
//! damage rules, the wall bounce and the crash.

use crate::apache_logic::{
    scale_damage, ApacheBrain, ApacheCorner, ApacheInputs, ApacheWorld, Kinematics,
};
use crate::behaviour::setpiece_kit::{segment_box_frac, trace_walls, Lcg, Pick, Wall};
use crate::setpiece_math::TraceHit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    Fx([i32; 3], u8),
    Explode([i32; 3], u8, i32, bool),
    Gibs([i32; 3], u8),
    StopRotor,
    Remove,
    Rocket([i32; 3], [i32; 3]),
    RocketSound([i32; 3]),
    Hurt(u8, [i32; 3]),
    Tracer([i32; 3], [i32; 3]),
    GunSound([i32; 3]),
}

struct World {
    pick: Pick,
    corners: Vec<ApacheCorner>,
    walls: Vec<Wall>,
    player: [i32; 3],
    alive: bool,
    prop: [i32; 3],
    /// The game stops ticking a removed apache.
    gone: bool,
    now: u16,
    pos: Vec<(u16, [i32; 3])>,
    yaw: Vec<(u16, u16)>,
    tilt: Vec<(u16, u16)>,
    events: Vec<(u16, Ev)>,
}

impl World {
    fn new(player: [i32; 3]) -> Self {
        Self {
            pick: Pick::Low,
            corners: Vec::new(),
            walls: Vec::new(),
            player,
            alive: true,
            prop: [0; 3],
            gone: false,
            now: 0,
            pos: Vec::new(),
            yaw: Vec::new(),
            tilt: Vec::new(),
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

impl ApacheWorld for World {
    fn corner(&mut self, k: usize) -> ApacheCorner {
        self.corners[k]
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        trace_walls(&self.walls, from, to)
    }
    fn player_box_frac(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<i32> {
        if !self.alive {
            return None;
        }
        let p = self.player;
        segment_box_frac(
            from,
            to,
            [p[0] - 16, p[1] - 36, p[2] - 16],
            [p[0] + 16, p[1] + 36, p[2] + 16],
        )
    }
    fn random_below(&mut self, n: u32) -> u32 {
        self.pick.below(n)
    }
    fn set_pos(&mut self, pos: [i32; 3]) {
        self.prop = pos;
        self.pos.push((self.now, pos));
    }
    fn prop_pos(&mut self) -> [i32; 3] {
        self.prop
    }
    fn rotor(&mut self) {}
    fn stop_rotor(&mut self) {
        self.ev(Ev::StopRotor);
    }
    fn set_yaw(&mut self, yaw: u16) {
        self.yaw.push((self.now, yaw));
    }
    fn set_tilt(&mut self, tilt: u16) {
        self.tilt.push((self.now, tilt));
    }
    fn explosion_fx(&mut self, at: [i32; 3], mag: u8) {
        self.ev(Ev::Fx(at, mag));
    }
    fn explode(&mut self, at: [i32; 3], damage: u8, radius: i32, by_player: bool) {
        self.ev(Ev::Explode(at, damage, radius, by_player));
    }
    fn gibs(&mut self, at: [i32; 3], count: u8) {
        self.ev(Ev::Gibs(at, count));
    }
    fn remove(&mut self) {
        self.gone = true;
        self.ev(Ev::Remove);
    }
    fn rocket(&mut self, from: [i32; 3], dir: [i32; 3]) {
        self.ev(Ev::Rocket(from, dir));
    }
    fn rocket_sound(&mut self, at: [i32; 3]) {
        self.ev(Ev::RocketSound(at));
    }
    fn gun_damage(&mut self) -> u8 {
        8
    }
    fn hurt_player(&mut self, damage: u8, from: [i32; 3]) {
        self.ev(Ev::Hurt(damage, from));
    }
    fn tracer(&mut self, from: [i32; 3], to: [i32; 3]) {
        self.ev(Ev::Tracer(from, to));
    }
    fn gun_sound(&mut self, at: [i32; 3]) {
        self.ev(Ev::GunSound(at));
    }
}

/// An apache bound at runtime `pos` facing +x (runtime yaw a quarter turn)
/// at tick `now`, hunting at once.
fn apache(pos: [i32; 3], now: u16, corners: usize) -> ApacheBrain {
    let mut a = ApacheBrain::new();
    a.bind(corners * 3, 0, 0, pos, 1024, now);
    a
}

fn inputs(w: &World) -> ApacheInputs {
    ApacheInputs {
        now: w.now,
        dead: false,
        player_pos: w.player,
        player_alive: w.alive,
        view_height: 28,
        easy: false,
    }
}

fn run(a: &mut ApacheBrain, w: &mut World, from: u16, to: u16) {
    for now in from..=to {
        if w.gone {
            return;
        }
        w.now = now;
        let i = inputs(w);
        a.tick(&i, w);
    }
}

fn corner(pos: [i32; 3], yaw_q12: u16) -> ApacheCorner {
    ApacheCorner {
        pos,
        yaw_q12,
        pitch_q8: 0,
    }
}

#[test]
fn a_flagged_apache_waits_for_its_trigger() {
    for flag in [0x04, 0x40] {
        let mut a = ApacheBrain::new();
        a.bind(0, 0, flag, [0, 500, 0], 1024, 0);
        let mut w = World::new([0, -5000, 0]);
        w.alive = false;
        run(&mut a, &mut w, 0, 40);
        assert!(w.pos.is_empty());
        a.startup();
        run(&mut a, &mut w, 41, 42);
        assert_eq!(w.pos.len(), 2);
    }
}

#[test]
fn goal_speed_rises_five_per_think_up_to_800() {
    let mut a = apache([0, 500, 0], 0, 0);
    let mut w = World::new([0, 0, 0]);
    w.alive = false;
    run(&mut a, &mut w, 0, 9);
    assert_eq!(a.kinematics().goal_speed, 25);
    run(&mut a, &mut w, 10, 400);
    assert_eq!(a.kinematics().goal_speed, 800);
}

#[test]
fn it_moves_to_the_next_corner_within_128_units() {
    let mut a = apache([0, 500, 0], 0, 2);
    let mut w = World::new([0, 0, 0]);
    w.alive = false;
    w.corners = vec![corner([100, 500, 0], 1024), corner([3000, 600, 900], 1024)];
    run(&mut a, &mut w, 0, 0);
    // GoldSrc axes: x, then runtime z as y, runtime y as z.
    assert_eq!(a.kinematics().desired, [3000, 900, 600]);
    // At 200 units it keeps the corner.
    let mut a = apache([0, 500, 0], 0, 2);
    w.corners[0] = corner([200, 500, 0], 1024);
    run(&mut a, &mut w, 1, 2);
    assert_eq!(a.kinematics().desired, [200, 0, 500]);
}

/// Yaw rate after one think: positive turns left, negative right.
fn first_yaw_rate(corner0: ApacheCorner, player: [i32; 3], alive: bool) -> i32 {
    let mut a = apache([0, 500, 0], 0, 1);
    let mut w = World::new(player);
    w.alive = alive;
    w.corners = vec![corner0];
    run(&mut a, &mut w, 0, 0);
    a.kinematics().avel[1]
}

#[test]
fn far_from_its_corner_it_heads_for_a_seen_player_within_75_degrees() {
    let ahead = corner([3000, 500, 0], 1024);
    // 8 degrees a second a step, less 2 percent: +-125 in 1/16 degree.
    assert_eq!(first_yaw_rate(ahead, [1000, 450, 1000], true), 125);
    // Not seen: straight on to the corner (which counts as "not left").
    assert_eq!(first_yaw_rate(ahead, [1000, 450, 1000], false), -125);
    // Seen but about 79 degrees off the corner line: the corner wins.
    assert_eq!(first_yaw_rate(ahead, [200, 450, 1000], true), -125);
}

#[test]
fn close_to_its_corner_it_adopts_the_corners_facing() {
    // 200 units out: between the 128 advance and the 250 pursuit radius.
    assert_eq!(
        first_yaw_rate(corner([200, 500, 0], 0), [0, 0, 0], false),
        125
    );
    assert_eq!(
        first_yaw_rate(corner([200, 500, 0], 2048), [0, 0, 0], false),
        -125
    );
}

#[test]
fn a_player_seen_long_ago_no_longer_draws_it_off_its_path() {
    let mut a = apache([0, 500, 0], 0, 1);
    let mut w = World::new([1000, 450, 1000]);
    w.corners = vec![corner([3000, 500, 0], 1024)];
    run(&mut a, &mut w, 0, 0);
    w.alive = false;
    // 90 s later (1800 ticks) the old sighting stops counting.
    let mut k = a.kinematics();
    for now in 1..=1802u16 {
        w.now = now;
        let i = inputs(&w);
        k.avel = [0; 3];
        k.ang = [0; 3];
        k.vel = [0; 3];
        k.pos = [0, 0, 500 * 16];
        a.set_kinematics(k);
        a.tick(&i, &mut w);
        if now == 1798 {
            assert_eq!(a.kinematics().avel[1], 125, "still pursuing at 1798");
        }
    }
    assert_eq!(a.kinematics().avel[1], -125);
}

#[test]
fn one_flight_step_from_a_level_hover() {
    let mut a = ApacheBrain::new();
    let k0 = Kinematics {
        pos: [0, 0, 500 * 16],
        vel: [0; 3],
        ang: [0; 3],
        avel: [0; 3],
        force: 0,
        goal_speed: 0,
        desired: [0, 0, 500],
    };
    a.set_kinematics(k0);
    a.flight([4096, 0, 0]);
    let k = a.kinematics();
    // Gravity takes 38.4 u/s per step, then 0.5% drag.
    assert_eq!(k.vel, [0, 0, -(384 * 16 / 10) * 995 / 1000]);
    // Two seconds out it is predicted 768 low: thrust rises 12 u/s.
    assert_eq!(k.force, 12 * 16);
    // Turning: the heading is dead ahead, which counts as "to the right".
    assert_eq!(k.avel[1], -(8 * 16) * 98 / 100);
}

#[test]
fn flight_rates_saturate_at_their_limits() {
    let mut a = ApacheBrain::new();
    a.set_kinematics(Kinematics {
        pos: [0, 0, 500 * 16],
        vel: [0; 3],
        // Facing so that two seconds of turning bring it to +x.
        ang: [0, -2 * 60 * 16, 0],
        avel: [0, 60 * 16, 0],
        force: 80 * 16,
        goal_speed: 0,
        desired: [0, 0, 400],
    });
    a.flight([0, 4096, 0]);
    let k = a.kinematics();
    // Yaw rate already at 60: only the 2% decay.
    assert_eq!(k.avel[1], 60 * 16 * 98 / 100);
    // Predicted above the wanted height: thrust falls 8.
    assert_eq!(k.force, 72 * 16);
    // Thrust never falls below 30.
    let mut a = ApacheBrain::new();
    a.set_kinematics(Kinematics {
        pos: [0, 0, 500 * 16],
        force: 30 * 16,
        desired: [0, 0, -2000],
        ..Kinematics::default()
    });
    a.flight([4096, 0, 0]);
    assert_eq!(a.kinematics().force, 30 * 16);
}

#[test]
fn it_pitches_nose_down_toward_a_goal_ahead_and_relaxes_otherwise() {
    let mut a = ApacheBrain::new();
    a.set_kinematics(Kinematics {
        pos: [0, 0, 500 * 16],
        goal_speed: 400,
        desired: [2000, 0, 500],
        ..Kinematics::default()
    });
    a.flight([4096, 0, 0]);
    assert_eq!(a.kinematics().avel[0], -12 * 16);
    // Already pitched past -40: it eases back 4.
    let mut a = ApacheBrain::new();
    a.set_kinematics(Kinematics {
        pos: [0, 0, 500 * 16],
        ang: [-41 * 16, 0, 0],
        goal_speed: 400,
        desired: [2000, 0, 500],
        ..Kinematics::default()
    });
    a.flight([4096, 0, 0]);
    assert_eq!(a.kinematics().avel[0], 4 * 16);
}

#[test]
fn it_banks_into_sideways_error_up_to_30_degrees() {
    // The goal is off to the left: roll rate steps 4 one way.
    let mut a = ApacheBrain::new();
    a.set_kinematics(Kinematics {
        pos: [0, 0, 500 * 16],
        desired: [0, 1000, 500],
        ..Kinematics::default()
    });
    a.flight([4096, 0, 0]);
    let left = a.kinematics().avel[2];
    assert_eq!(left.abs(), 4 * 16);
    // Rolled past 30 that way, it backs off 2.
    let mut a = ApacheBrain::new();
    a.set_kinematics(Kinematics {
        pos: [0, 0, 500 * 16],
        ang: [0, 0, 31 * 16 * left.signum()],
        desired: [0, 1000, 500],
        ..Kinematics::default()
    });
    a.flight([4096, 0, 0]);
    assert_eq!(a.kinematics().avel[2], -2 * 16 * left.signum());
}

#[test]
fn the_chin_gun_waits_for_two_seconds_of_sight_and_slows_it_to_400() {
    let mut a = apache([0, 500, 0], 0, 0);
    let mut k = a.kinematics();
    k.goal_speed = 700;
    a.set_kinematics(k);
    let mut w = World::new([600, 0, 0]);
    run(&mut a, &mut w, 0, 39);
    assert!(w.ticks_of(|e| matches!(e, Ev::Tracer(..))).is_empty());
    let mut first = None;
    for now in 40..=60u16 {
        run(&mut a, &mut w, now, now);
        if first.is_none() && !w.ticks_of(|e| matches!(e, Ev::Tracer(..))).is_empty() {
            first = Some(now);
            assert_eq!(a.kinematics().goal_speed, 400);
        }
    }
    let first = first.expect("the gun fired");
    assert!((40..=46).contains(&first), "{first}");
    // One firing sound per second at most.
    let sounds = w.ticks_of(|e| matches!(e, Ev::GunSound(..)));
    assert!(sounds.windows(2).all(|p| p[1] - p[0] >= 20), "{sounds:?}");
}

#[test]
fn the_chin_gun_needs_the_player_seen_within_the_last_second() {
    let mut a = apache([0, 500, 0], 0, 0);
    let mut w = World::new([600, 0, 0]);
    run(&mut a, &mut w, 0, 30);
    // Hidden from tick 31: last seen at 30, so the gun may run to tick 48.
    w.walls.push(Wall { axis: 0, at: 300 });
    run(&mut a, &mut w, 31, 120);
    let shots = w.ticks_of(|e| matches!(e, Ev::Tracer(..)));
    assert!(shots.iter().all(|&t| t < 50), "{shots:?}");
}

/// Hold the apache still, nose 10 degrees down, 500 over a floor with the
/// player where its predicted line meets the floor.
fn rocket_range(a: &mut ApacheBrain, w: &mut World, from: u16, to: u16, easy: bool) {
    let k = Kinematics {
        pos: [0, 0, 500 * 16],
        vel: [0, 0, 610],
        ang: [-10 * 16, 0, 0],
        avel: [0; 3],
        force: 0,
        goal_speed: 0,
        desired: [0, 0, 500],
    };
    for now in from..=to {
        a.set_kinematics(k);
        w.now = now;
        let mut i = inputs(w);
        i.easy = easy;
        a.tick(&i, w);
    }
}

#[test]
fn rockets_go_in_pairs_a_tenth_apart_ten_then_a_ten_second_pause() {
    let mut a = apache([0, 500, 0], 0, 0);
    let mut w = World::new([2836, 0, 0]);
    w.walls.push(Wall { axis: 1, at: 0 });
    // Keep the chin gun out of it: hide the player's eyes after the first
    // sighting so only the 60 s rocket memory remains.
    rocket_range(&mut a, &mut w, 0, 0, false);
    w.alive = false;
    rocket_range(&mut a, &mut w, 1, 260, false);
    assert_eq!(
        w.ticks_of(|e| matches!(e, Ev::Rocket(..))),
        [0, 2, 12, 14, 24, 26, 36, 38, 48, 50, 250, 252]
    );
    // Launch points alternate sides, 210 apart across the hull.
    let src: Vec<[i32; 3]> = w
        .events
        .iter()
        .filter_map(|e| match e.1 {
            Ev::Rocket(s, _) => Some(s),
            _ => None,
        })
        .take(2)
        .collect();
    assert_eq!(src[0][0], src[1][0]);
    assert!((src[0][2] - src[1][2]).abs() >= 200, "{src:?}");
    assert_eq!(a.rockets(), 8);
}

#[test]
fn rockets_hold_fire_when_the_predicted_line_misses_the_player() {
    let mut a = apache([0, 500, 0], 0, 0);
    let mut w = World::new([1600, 0, 0]);
    w.walls.push(Wall { axis: 1, at: 0 });
    rocket_range(&mut a, &mut w, 0, 0, false);
    w.alive = false;
    rocket_range(&mut a, &mut w, 1, 40, false);
    assert!(w.ticks_of(|e| matches!(e, Ev::Rocket(..))).is_empty());
}

#[test]
fn on_easy_the_gun_window_blocks_rockets_for_ten_seconds() {
    let mut a = apache([0, 500, 0], 0, 0);
    let mut w = World::new([2836, 0, 0]);
    w.walls.push(Wall { axis: 1, at: 0 });
    rocket_range(&mut a, &mut w, 0, 0, true);
    w.alive = false;
    rocket_range(&mut a, &mut w, 1, 60, true);
    // The player stays visible to the gun gate's memory for a second; past
    // the 2 s mark nothing lets it open again, so rockets proceed. With the
    // player unseen from tick 1 the gate never opens: same as normal.
    assert_eq!(
        w.ticks_of(|e| matches!(e, Ev::Rocket(..))),
        [0, 2, 12, 14, 24, 26, 36, 38, 48, 50]
    );
    // With the player kept in view, the gun window opens at 2 s and every
    // think inside it pushes rockets back 10 s.
    let mut a = apache([0, 500, 0], 0, 0);
    let mut w = World::new([2836, 0, 0]);
    w.walls.push(Wall { axis: 1, at: 0 });
    rocket_range(&mut a, &mut w, 0, 300, true);
    let rockets = w.ticks_of(|e| matches!(e, Ev::Rocket(..)));
    assert!(rockets.iter().all(|&t| t < 40), "{rockets:?}");
}

#[test]
fn blast_counts_double_and_hits_of_50_or_less_do_nothing() {
    assert_eq!(scale_damage(50, false, 250), 0);
    assert_eq!(scale_damage(51, false, 250), 51);
    assert_eq!(scale_damage(26, true, 250), 52);
    assert_eq!(scale_damage(25, true, 250), 0);
    // Health over 255 scales the hit down onto the u8 health.
    assert_eq!(scale_damage(100, false, 1000), 25);
    assert_eq!(scale_damage(255, true, 255), 255);
}

#[test]
fn it_bounces_off_walls_at_its_speed_plus_200() {
    let mut a = apache([0, 500, 0], 1, 0);
    let mut w = World::new([0, 0, 0]);
    w.alive = false;
    w.walls.push(Wall { axis: 0, at: 10 });
    let mut k = a.kinematics();
    k.vel = [300 * 16, 0, 0];
    a.set_kinematics(k);
    // An odd tick moves without thinking.
    run(&mut a, &mut w, 1, 1);
    let v = a.kinematics().vel[0] / 16;
    assert!((-210..=-180).contains(&v), "{v}");
    // It stops at the hit point the world reported.
    assert_eq!(w.pos[0].1, [9, 500, 0]);
}

#[test]
fn shot_down_it_falls_spins_up_and_blows_after_fifteen_seconds() {
    for (flags, fuse) in [(0u16, 300u16), (0x08, 80)] {
        let mut a = ApacheBrain::new();
        a.bind(0, 0, flags, [0, 500, 0], 1024, 0);
        let mut w = World::new([0, 0, 0]);
        w.alive = false;
        run(&mut a, &mut w, 0, 9);
        let mut k = a.kinematics();
        k.avel = [0, 100 * 16, 0];
        k.vel = [0; 3];
        a.set_kinematics(k);
        for now in 10..=400u16 {
            if w.gone {
                break;
            }
            w.now = now;
            let mut i = inputs(&w);
            i.dead = true;
            a.tick(&i, &mut w);
            if now == 11 {
                // 0.3 gravity: 12 u/s lost per tick.
                assert_eq!(a.kinematics().vel[2], -2 * 192);
                assert_eq!(a.kinematics().avel[1], 100 * 16 * 102 / 100);
            }
        }
        let boom = w.ticks_of(|e| matches!(e, Ev::Explode(_, 255, 750, false)));
        assert_eq!(boom, [10 + fuse]);
        let tail: Vec<Ev> = w.events.iter().rev().take(3).map(|e| e.1).collect();
        assert_eq!(tail, [Ev::Remove, Ev::StopRotor, tail[2]]);
        assert!(matches!(tail[2], Ev::Gibs(_, 12)));
    }
}

#[test]
fn a_falling_apache_explodes_on_the_think_after_impact() {
    let mut a = apache([0, 60, 0], 0, 0);
    let mut w = World::new([0, 0, 0]);
    w.alive = false;
    w.walls.push(Wall { axis: 1, at: 0 });
    for now in 10..=100u16 {
        if w.gone {
            break;
        }
        w.now = now;
        let mut i = inputs(&w);
        i.dead = true;
        a.tick(&i, &mut w);
    }
    let boom = w.ticks_of(|e| matches!(e, Ev::Explode(..)));
    assert_eq!(boom.len(), 1);
    assert!(boom[0] < 60, "{boom:?}");
}

/// Recorded from ac83da7 behaviour: twenty flight steps from a slow level
/// cruise toward a heading 60 degrees left and a goal 800 ahead and 200 up.
#[test]
fn golden_flight_steps() {
    let mut a = ApacheBrain::new();
    a.set_kinematics(Kinematics {
        pos: [0, 0, 400 * 16],
        vel: [150 * 16, 0, 0],
        ang: [0, 0, 0],
        avel: [0; 3],
        force: 40 * 16,
        goal_speed: 300,
        desired: [800, 0, 600],
    });
    let want = [2048, 3547, 0];
    let mut steps = Vec::new();
    for _ in 0..20 {
        a.flight(want);
        let k = a.kinematics();
        steps.push((k.vel, k.avel, k.force));
        // Integrate the 0.1 s the way the ticks would.
        let mut k = k;
        for _ in 0..2 {
            for c in 0..3 {
                k.pos[c] += k.vel[c] / 20;
                k.ang[c] += k.avel[c] / 20;
            }
        }
        a.set_kinematics(k);
    }
    assert_eq!(
        steps,
        [
            ([2333, 0, 22], [-192, 125, -64], 832),
            ([2262, 2, 234], [-384, 247, 0], 704),
            ([2227, 3, 320], [-576, 367, 64], 896),
            ([2235, 3, 597], [-384, 485, 128], 1088),
            ([2290, 0, 1061], [-192, 600, 192], 960),
            ([2349, -11, 1392], [-384, 462, 256], 832),
            ([2428, -28, 1587], [-192, 578, 224], 704),
            ([2499, -47, 1650], [0, 441, 288], 576),
            ([2548, -72, 1584], [-192, 306, 224], 768),
            ([2648, -112, 1698], [0, 425, 160], 640),
            ([2720, -146, 1684], [-192, 291, 96], 512),
            ([2772, -167, 1544], [0, 410, 160], 704),
            ([2870, -201, 1586], [-192, 276, 96], 576),
            ([2942, -220, 1502], [0, 395, 160], 768),
            ([3067, -251, 1596], [192, 261, 96], 640),
            ([3144, -279, 1570], [0, 130, 32], 512),
            ([3184, -298, 1422], [192, 252, -32], 704),
            ([3259, -330, 1461], [0, 121, -96], 576),
            ([3295, -347, 1379], [192, 244, -160], 448),
            ([3282, -352, 1182], [0, 113, -224], 640)
        ]
    );
}

/// Recorded from ac83da7 behaviour: an apache hunts a three-corner loop
/// while the player stands in the open, for twelve seconds, drawing from
/// the game's generator. Positions every 10 ticks and every event.
#[test]
fn golden_hunt() {
    let mut a = apache([0, 600, 0], 0, 3);
    let mut w = World::new([1200, 0, 400]);
    w.pick = Pick::Game(Lcg(Lcg::GAME_SEED));
    w.walls.push(Wall { axis: 1, at: 0 });
    w.corners = vec![
        corner([1500, 650, 0], 1024),
        corner([1500, 700, 1500], 0),
        corner([0, 600, 1500], 3072),
    ];
    let mut track = Vec::new();
    for now in 0..240u16 {
        run(&mut a, &mut w, now, now);
        if now % 10 == 0 {
            track.push((now, w.prop));
        }
    }
    assert_eq!(
        track,
        [
            (0, [0, 600, 0]),
            (10, [0, 567, 0]),
            (20, [20, 578, -4]),
            (30, [92, 628, -15]),
            (40, [204, 666, -29]),
            (50, [346, 687, -47]),
            (60, [496, 696, -68]),
            (70, [641, 698, -89]),
            (80, [772, 697, -109]),
            (90, [882, 697, -126]),
            (100, [974, 699, -140]),
            (110, [1058, 703, -144]),
            (120, [1138, 709, -137]),
            (130, [1211, 709, -118]),
            (140, [1273, 703, -91]),
            (150, [1324, 698, -58]),
            (160, [1365, 697, -25]),
            (170, [1400, 700, 0]),
            (180, [1433, 710, 18]),
            (190, [1469, 739, 56]),
            (200, [1495, 775, 136]),
            (210, [1503, 799, 261]),
            (220, [1499, 809, 431]),
            (230, [1490, 813, 650])
        ]
    );
    assert_eq!(
        w.events,
        [
            (22, Ev::Rocket([15, 453, -98], [3459, -1991, 916])),
            (22, Ev::RocketSound([15, 453, -98])),
            (24, Ev::Rocket([-14, 496, 104], [3389, -2084, 971])),
            (24, Ev::RocketSound([-14, 496, 104])),
            (40, Ev::Tracer([209, 494, 5], [1145, 0, 410])),
            (40, Ev::GunSound([209, 494, 5])),
            (42, Ev::Tracer([242, 501, 4], [1208, 0, 366])),
            (44, Ev::Hurt(8, [276, 506, 1])),
            (44, Ev::Tracer([276, 506, 1], [1227, 0, 420])),
            (46, Ev::Hurt(8, [312, 511, -2])),
            (46, Ev::Tracer([312, 511, -2], [1226, 0, 401])),
            (48, Ev::Tracer([348, 516, -5], [1226, 0, 374])),
            (50, Ev::Hurt(8, [384, 520, -9])),
            (50, Ev::Tracer([384, 520, -9], [1256, 0, 443])),
            (52, Ev::Tracer([417, 524, -12], [1157, 0, 411])),
            (54, Ev::Hurt(8, [454, 527, -17])),
            (54, Ev::Tracer([454, 527, -17], [1239, 0, 397])),
            (56, Ev::Hurt(8, [487, 531, -19])),
            (56, Ev::Tracer([487, 531, -19], [1216, 0, 409])),
            (58, Ev::Tracer([521, 533, -24], [1258, 0, 390])),
            (60, Ev::Hurt(8, [553, 535, -29])),
            (60, Ev::Tracer([553, 535, -29], [1196, 0, 413])),
            (60, Ev::GunSound([553, 535, -29])),
            (62, Ev::Tracer([586, 537, -31], [1173, 0, 389])),
            (64, Ev::Hurt(8, [617, 539, -35])),
            (64, Ev::Tracer([617, 539, -35], [1191, 0, 390])),
            (66, Ev::Tracer([648, 540, -39], [1238, 1, 386])),
            (68, Ev::Hurt(8, [679, 543, -41])),
            (68, Ev::Tracer([679, 543, -41], [1209, 0, 415])),
            (70, Ev::Tracer([707, 542, -47], [1235, 1, 392])),
            (72, Ev::Tracer([738, 543, -49], [1181, 1, 372])),
            (74, Ev::Hurt(8, [766, 542, -54])),
            (74, Ev::Tracer([766, 542, -54], [1218, 0, 392])),
            (76, Ev::Tracer([792, 543, -54], [1205, 1, 383])),
            (78, Ev::Hurt(8, [817, 543, -58])),
            (78, Ev::Tracer([817, 543, -58], [1206, 0, 404])),
            (80, Ev::Tracer([842, 545, -60], [1172, 0, 388])),
            (80, Ev::GunSound([842, 545, -60])),
            (82, Ev::Hurt(8, [867, 546, -61])),
            (82, Ev::Tracer([867, 546, -61], [1215, 0, 415])),
            (84, Ev::Tracer([889, 546, -64], [1198, 0, 360])),
            (86, Ev::Hurt(8, [911, 547, -66])),
            (86, Ev::Tracer([911, 547, -66], [1203, 0, 431])),
            (88, Ev::Tracer([931, 547, -68], [1215, 0, 377])),
            (90, Ev::Hurt(8, [948, 545, -71])),
            (90, Ev::Tracer([948, 545, -71], [1202, 1, 412])),
            (92, Ev::Hurt(8, [963, 541, -79])),
            (92, Ev::Tracer([963, 541, -79], [1200, 0, 390])),
            (94, Ev::Tracer([977, 541, -81], [1209, 0, 380])),
            (96, Ev::Hurt(8, [989, 538, -87])),
            (96, Ev::Tracer([989, 538, -87], [1187, 0, 414])),
            (98, Ev::Hurt(8, [1004, 536, -91])),
            (98, Ev::Tracer([1004, 536, -91], [1190, 0, 387])),
            (100, Ev::Hurt(8, [1015, 534, -97])),
            (100, Ev::Tracer([1015, 534, -97], [1189, 0, 387])),
            (100, Ev::GunSound([1015, 534, -97])),
            (102, Ev::Tracer([1028, 533, -99], [1192, 0, 379])),
            (104, Ev::Tracer([1044, 534, -99], [1222, 0, 399])),
            (106, Ev::Tracer([1059, 535, -103], [1228, 1, 408])),
            (108, Ev::Tracer([1074, 535, -101], [1227, 1, 403])),
            (110, Ev::Hurt(8, [1088, 536, -104])),
            (110, Ev::Tracer([1088, 536, -104], [1210, 1, 426])),
            (112, Ev::Hurt(8, [1101, 537, -102])),
            (112, Ev::Tracer([1101, 537, -102], [1207, 1, 403])),
            (114, Ev::Hurt(8, [1116, 537, -103])),
            (114, Ev::Tracer([1116, 537, -103], [1190, 0, 426])),
            (116, Ev::Hurt(8, [1130, 539, -101])),
            (116, Ev::Tracer([1130, 539, -101], [1208, 1, 422])),
            (118, Ev::Hurt(8, [1148, 541, -95])),
            (118, Ev::Tracer([1148, 541, -95], [1202, 0, 418])),
            (120, Ev::Hurt(8, [1161, 542, -93])),
            (120, Ev::Tracer([1161, 542, -93], [1185, 0, 410])),
            (120, Ev::GunSound([1161, 542, -93])),
            (122, Ev::Hurt(8, [1179, 543, -86])),
            (122, Ev::Tracer([1179, 543, -86], [1191, 0, 400])),
            (124, Ev::Hurt(8, [1194, 543, -83])),
            (124, Ev::Tracer([1194, 543, -83], [1210, 0, 397])),
            (126, Ev::Tracer([1207, 544, -75], [1218, 0, 386])),
            (128, Ev::Tracer([1221, 543, -70], [1179, 0, 411])),
            (130, Ev::Hurt(8, [1234, 544, -63])),
            (130, Ev::Tracer([1234, 544, -63], [1185, 1, 405])),
            (132, Ev::Hurt(8, [1247, 544, -57])),
            (132, Ev::Tracer([1247, 544, -57], [1203, 0, 406])),
            (134, Ev::Tracer([1260, 544, -50], [1176, 0, 421])),
            (136, Ev::Hurt(8, [1271, 543, -43])),
            (136, Ev::Tracer([1271, 543, -43], [1191, 1, 388])),
            (138, Ev::Hurt(8, [1283, 542, -34])),
            (138, Ev::Tracer([1283, 542, -34], [1205, 1, 388])),
            (140, Ev::Hurt(8, [1293, 541, -28])),
            (140, Ev::Tracer([1293, 541, -28], [1216, 0, 395])),
            (140, Ev::GunSound([1293, 541, -28])),
            (142, Ev::Hurt(8, [1304, 541, -18])),
            (142, Ev::Tracer([1304, 541, -18], [1207, 1, 414])),
            (144, Ev::Hurt(8, [1315, 540, -12])),
            (144, Ev::Tracer([1315, 540, -12], [1216, 1, 412])),
            (146, Ev::Hurt(8, [1327, 541, -2])),
            (146, Ev::Tracer([1327, 541, -2], [1196, 1, 399])),
            (148, Ev::Tracer([1339, 540, 3], [1222, 0, 380])),
            (150, Ev::Tracer([1352, 540, 12], [1203, 0, 381])),
            (152, Ev::Hurt(8, [1365, 540, 17])),
            (152, Ev::Tracer([1365, 540, 17], [1216, 1, 406])),
            (154, Ev::Hurt(8, [1379, 541, 24])),
            (154, Ev::Tracer([1379, 541, 24], [1201, 1, 386])),
            (156, Ev::Hurt(8, [1391, 541, 28])),
            (156, Ev::Tracer([1391, 541, 28], [1202, 0, 408])),
            (158, Ev::Tracer([1402, 542, 37], [1220, 0, 376])),
            (160, Ev::Hurt(8, [1411, 542, 43])),
            (160, Ev::Tracer([1411, 542, 43], [1181, 1, 414])),
            (160, Ev::GunSound([1411, 542, 43])),
            (162, Ev::Hurt(8, [1421, 544, 49])),
            (162, Ev::Tracer([1421, 544, 49], [1182, 1, 414])),
            (164, Ev::Hurt(8, [1430, 545, 54])),
            (164, Ev::Tracer([1430, 545, 54], [1209, 1, 395])),
            (170, Ev::Tracer([1454, 547, 66], [1220, 1, 372])),
            (172, Ev::Hurt(8, [1455, 545, 64])),
            (172, Ev::Tracer([1455, 545, 64], [1204, 0, 398])),
            (174, Ev::Tracer([1456, 541, 60], [1213, 1, 383])),
            (176, Ev::Tracer([1457, 539, 56], [1211, 0, 379])),
            (178, Ev::Hurt(8, [1460, 540, 53])),
            (178, Ev::Tracer([1460, 540, 53], [1193, 0, 403])),
            (180, Ev::Hurt(8, [1464, 541, 51])),
            (180, Ev::Tracer([1464, 541, 51], [1199, 0, 400])),
            (180, Ev::GunSound([1464, 541, 51])),
            (182, Ev::Hurt(8, [1469, 545, 51])),
            (182, Ev::Tracer([1469, 545, 51], [1178, 0, 425])),
            (184, Ev::Hurt(8, [1475, 550, 52])),
            (184, Ev::Tracer([1475, 550, 52], [1206, 0, 400])),
            (186, Ev::Hurt(8, [1482, 554, 53])),
            (186, Ev::Tracer([1482, 554, 53], [1195, 0, 387])),
            (188, Ev::Hurt(8, [1488, 560, 57])),
            (188, Ev::Tracer([1488, 560, 57], [1199, 0, 411])),
            (190, Ev::Hurt(8, [1495, 566, 60])),
            (190, Ev::Tracer([1495, 566, 60], [1206, 0, 393])),
            (192, Ev::Hurt(8, [1503, 574, 69])),
            (192, Ev::Tracer([1503, 574, 69], [1174, 0, 410])),
            (194, Ev::Hurt(8, [1509, 583, 82])),
            (194, Ev::Tracer([1509, 583, 82], [1210, 1, 392])),
            (196, Ev::Hurt(8, [1515, 590, 98])),
            (196, Ev::Tracer([1515, 590, 98], [1179, 0, 410])),
            (198, Ev::Hurt(8, [1520, 596, 116])),
            (198, Ev::Tracer([1520, 596, 116], [1183, 1, 415])),
            (200, Ev::Tracer([1522, 602, 132], [1234, 1, 380])),
            (200, Ev::GunSound([1522, 602, 132])),
            (202, Ev::Hurt(8, [1523, 607, 150])),
            (202, Ev::Tracer([1523, 607, 150], [1197, 1, 396])),
            (204, Ev::Hurt(8, [1523, 612, 174])),
            (204, Ev::Tracer([1523, 612, 174], [1190, 1, 394])),
            (206, Ev::Tracer([1521, 617, 197], [1219, 1, 384])),
            (208, Ev::Hurt(8, [1519, 621, 223])),
            (208, Ev::Tracer([1519, 621, 223], [1209, 1, 393])),
            (210, Ev::Hurt(8, [1516, 624, 249])),
            (210, Ev::Tracer([1516, 624, 249], [1203, 0, 410])),
            (212, Ev::Tracer([1513, 627, 277], [1200, 1, 383])),
            (214, Ev::Hurt(8, [1510, 631, 309])),
            (214, Ev::Tracer([1510, 631, 309], [1187, 1, 389])),
            (216, Ev::Tracer([1508, 633, 343], [1223, 1, 387])),
            (218, Ev::Hurt(8, [1504, 633, 373])),
            (218, Ev::Tracer([1504, 633, 373], [1212, 1, 390])),
            (220, Ev::Hurt(8, [1501, 634, 411])),
            (220, Ev::Tracer([1501, 634, 411], [1185, 0, 394])),
            (220, Ev::GunSound([1501, 634, 411])),
            (222, Ev::Hurt(8, [1497, 637, 455])),
            (222, Ev::Tracer([1497, 637, 455], [1204, 0, 386])),
            (224, Ev::Hurt(8, [1495, 638, 502])),
            (224, Ev::Tracer([1495, 638, 502], [1190, 1, 397]))
        ]
    );
}

/// Recorded from ac83da7 behaviour: killed in a banking turn, it falls and
/// strikes the floor, drawing fireball scatter from the game's generator.
#[test]
fn golden_crash() {
    let mut a = apache([0, 400, 0], 0, 1);
    let mut w = World::new([0, 0, 0]);
    w.alive = false;
    w.pick = Pick::Game(Lcg(5));
    w.walls.push(Wall { axis: 1, at: 0 });
    w.corners = vec![corner([2000, 400, 800], 1024)];
    run(&mut a, &mut w, 0, 59);
    for now in 60..=200u16 {
        if w.gone {
            break;
        }
        w.now = now;
        let mut i = inputs(&w);
        i.dead = true;
        a.tick(&i, &mut w);
    }
    let fall: Vec<(u16, [i32; 3])> = w.pos.iter().copied().filter(|p| p.0 % 8 == 0).collect();
    assert_eq!(
        fall,
        [
            (0, [0, 400, 0]),
            (8, [0, 374, 0]),
            (16, [5, 363, 5]),
            (24, [41, 397, 26]),
            (32, [110, 437, 60]),
            (40, [206, 467, 106]),
            (48, [324, 485, 167]),
            (56, [448, 489, 239]),
            (64, [572, 477, 317]),
            (72, [695, 427, 394]),
            (80, [819, 338, 472]),
            (88, [942, 211, 549]),
            (96, [1066, 46, 627])
        ]
    );
    assert_eq!(
        w.events,
        [
            (60, Ev::Fx([501, 430, 132], 50)),
            (64, Ev::Fx([657, 417, 357], 50)),
            (68, Ev::Fx([609, 341, 253], 50)),
            (72, Ev::Fx([715, 338, 523], 50)),
            (76, Ev::Fx([857, 288, 385], 50)),
            (80, Ev::Fx([908, 218, 608], 50)),
            (84, Ev::Fx([737, 214, 573], 50)),
            (88, Ev::Fx([963, 139, 496], 50)),
            (92, Ev::Fx([888, 78, 469], 50)),
            (96, Ev::Fx([1114, -30, 497], 50)),
            (100, Ev::Explode([1096, 0, 646], 255, 750, false)),
            (100, Ev::Gibs([1096, 0, 646], 12)),
            (100, Ev::StopRotor),
            (100, Ev::Remove)
        ]
    );
}
