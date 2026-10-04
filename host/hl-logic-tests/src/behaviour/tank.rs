//! func_tank family (HV-03): think cadence, PVS retry, range and sight,
//! aim clamp and turn rate, the fire condition and round count, spread,
//! rocket and mortar volleys, and the player's controls.

use crate::behaviour::setpiece_kit::{segment_box_frac, trace_walls, Lcg, Pick, Wall};
use crate::setpiece_math::TraceHit;
use crate::tank_logic::{
    cooldown_after_tick, still_controlled, ControlAttempt, TankBrain, TankInputs, TankKeys,
    TankUse, TankWorld,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    Rocket([i32; 3], [i32; 3], bool),
    Tracer([i32; 3], [i32; 3]),
    Explode([i32; 3], u8, i32, bool),
    PlayerRound([i32; 3], [i32; 3], u8),
    Hurt(u8, [i32; 3]),
    Target,
    GunSound,
}

const ORIGIN: [i32; 3] = [0, 64, 0];
const VIEW_HEIGHT: i32 = 28;

struct World {
    pick: Pick,
    pvs: bool,
    walls: Vec<Wall>,
    player: [i32; 3],
    alive: bool,
    now: u16,
    pvs_asks: Vec<u16>,
    poses: Vec<(u16, [i16; 2])>,
    events: Vec<(u16, Ev)>,
}

impl World {
    fn new(pick: Pick, player: [i32; 3]) -> Self {
        Self {
            pick,
            pvs: true,
            walls: Vec::new(),
            player,
            alive: true,
            now: 0,
            pvs_asks: Vec::new(),
            poses: Vec::new(),
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

impl TankWorld for World {
    fn in_pvs(&mut self) -> bool {
        self.pvs_asks.push(self.now);
        self.pvs
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
    fn skill_round_damage(&mut self, bullet: u8) -> u8 {
        10 + bullet
    }
    fn set_pose(&mut self, angles: [i16; 2]) {
        self.poses.push((self.now, angles));
    }
    fn launch_rocket(&mut self, from: [i32; 3], dir: [i32; 3], by_player: bool) {
        self.ev(Ev::Rocket(from, dir, by_player));
    }
    fn tracer(&mut self, from: [i32; 3], to: [i32; 3]) {
        self.ev(Ev::Tracer(from, to));
    }
    fn explode(&mut self, at: [i32; 3], magnitude: u8, radius: i32, by_player: bool) {
        self.ev(Ev::Explode(at, magnitude, radius, by_player));
    }
    fn player_round(&mut self, from: [i32; 3], end: [i32; 3], damage: u8) {
        self.ev(Ev::PlayerRound(from, end, damage));
    }
    fn hurt_player(&mut self, damage: u8, from: [i32; 3]) {
        self.ev(Ev::Hurt(damage, from));
    }
    fn fire_target(&mut self) {
        self.ev(Ev::Target);
    }
    fn gun_sound(&mut self) {
        self.ev(Ev::GunSound);
    }
}

/// An active MP5-type gun with a 0.025 spread, 2 rounds/s, 2 s persistence,
/// turning 30/tick within a quarter turn either side of +x.
fn gun() -> TankKeys {
    TankKeys {
        spawnflags: 1,
        kind: 2 | (1 << 2),
        persistence: 40,
        turn_rate: [30, 30],
        tolerance: [46, 46],
        range: [1024, 256],
        fire_rate: 2 * 256,
        barrel: [32, 0, 0],
        damage: 0,
        min_range: 0,
        max_range: 2048,
        centre: [0, 0],
        controls: [([0; 3], [0; 3]); 2],
        n_controls: 0,
    }
}

fn inputs(w: &World) -> TankInputs {
    TankInputs {
        now: w.now,
        controlled: false,
        removed: false,
        origin: ORIGIN,
        view: [4096, 0, 0],
        player_pos: w.player,
        player_alive: w.alive,
        view_height: VIEW_HEIGHT,
    }
}

/// Run ticks `from..=to` of an unmounted tank.
fn run(t: &mut TankBrain, w: &mut World, from: u16, to: u16) {
    for now in from..=to {
        w.now = now;
        let i = inputs(w);
        t.tick(&i, w);
    }
}

#[test]
fn an_active_tank_first_thinks_three_quarters_of_a_second_after_load_then_every_tenth() {
    let mut t = TankBrain::spawn(&gun(), ORIGIN, 100);
    let mut w = World::new(Pick::Low, [5000, 0, 0]);
    run(&mut t, &mut w, 100, 125);
    assert_eq!(w.pvs_asks, [115, 117, 119, 121, 123, 125]);
}

#[test]
fn an_inactive_tank_never_thinks_until_used() {
    let mut k = gun();
    k.spawnflags = 0;
    let mut t = TankBrain::spawn(&k, ORIGIN, 100);
    assert!(!t.active());
    let mut w = World::new(Pick::Low, [600, 50, 0]);
    run(&mut t, &mut w, 100, 140);
    assert!(w.pvs_asks.is_empty());
    t.use_tank(TankUse::On, 140);
    run(&mut t, &mut w, 141, 144);
    assert_eq!(w.pvs_asks, [142, 144]);
}

#[test]
fn outside_the_players_pvs_it_retries_after_two_seconds() {
    let mut t = TankBrain::spawn(&gun(), ORIGIN, 100);
    let mut w = World::new(Pick::Low, [600, 50, 0]);
    w.pvs = false;
    run(&mut t, &mut w, 100, 200);
    assert_eq!(w.pvs_asks, [115, 155, 195]);
    assert!(w.events.is_empty());
}

#[test]
fn it_ignores_a_player_outside_its_min_and_max_range() {
    for (min, player) in [(0, [2500, 36, 0]), (300, [200, 36, 0])] {
        let mut k = gun();
        k.min_range = min;
        let mut t = TankBrain::spawn(&k, ORIGIN, 0);
        let mut w = World::new(Pick::Low, player);
        run(&mut t, &mut w, 0, 60);
        assert_eq!(t.angular_velocity(), [0, 0]);
        assert_eq!(t.angles(), [0, 0]);
        assert!(w.events.is_empty());
    }
}

#[test]
fn it_turns_at_ten_times_the_error_per_second_up_to_its_rate() {
    // The player sits 45 degrees to the left at barrel height.
    let mut t = TankBrain::spawn(&gun(), ORIGIN, 0);
    let mut w = World::new(Pick::Low, [700, 50, 700]);
    run(&mut t, &mut w, 0, 15);
    // Error 512 q12: half of it per tick is far over the 30 rate.
    assert_eq!(t.angular_velocity(), [30, 0]);
    // Well inside the rate, the step is half the remaining error per tick.
    let mut k = gun();
    k.turn_rate = [255, 255];
    k.centre = [500, 0];
    let mut t = TankBrain::spawn(&k, ORIGIN, 0);
    run(&mut t, &mut w, 16, 16);
    assert_eq!(t.angular_velocity(), [6, 0]);
}

#[test]
fn it_settles_on_the_sight_point_half_way_up_to_the_eyes() {
    // Player origin 14 below the pivot: the sight point (origin + 14) is
    // level with it, so the barrel settles at zero pitch.
    let mut t = TankBrain::spawn(&gun(), ORIGIN, 0);
    let mut w = World::new(Pick::Low, [700, 50, 700]);
    run(&mut t, &mut w, 0, 200);
    assert_eq!(t.angles(), [512, 0]);
}

#[test]
fn aim_is_clamped_to_its_arcs_and_a_target_outside_them_goes_stale() {
    let mut k = gun();
    k.range = [512, 256];
    let mut t = TankBrain::spawn(&k, ORIGIN, 0);
    // Straight behind it, a little to the right.
    let mut w = World::new(Pick::Low, [-1000, 50, -10]);
    run(&mut t, &mut w, 0, 200);
    // Yaw is kept in 0..4096: a quarter turn right of +x reads 3584.
    assert!(matches!(t.angles()[0], 512 | 3584), "{:?}", t.angles());
    // The sighting outside the yaw arc never counted, so it never fired.
    assert!(w.ticks_of(|e| matches!(e, Ev::Tracer(..))).is_empty());
}

#[test]
fn the_first_think_in_the_cone_only_starts_the_clock() {
    // Player dead ahead: aimed from the start, 2 rounds a second.
    let mut t = TankBrain::spawn(&gun(), ORIGIN, 100);
    let mut w = World::new(Pick::Low, [600, 50, 0]);
    run(&mut t, &mut w, 100, 150);
    assert_eq!(w.ticks_of(|e| matches!(e, Ev::Tracer(..))), [125, 135, 145]);
    // Each volley fires the target once.
    assert_eq!(w.ticks_of(|e| *e == Ev::Target), [125, 135, 145]);
}

#[test]
fn rounds_per_volley_are_the_elapsed_time_times_the_fire_rate() {
    let mut k = gun();
    k.fire_rate = 20 * 256; // 20 a second: two per 0.1 s think
    let mut t = TankBrain::spawn(&k, ORIGIN, 100);
    let mut w = World::new(Pick::Low, [600, 50, 0]);
    run(&mut t, &mut w, 100, 121);
    assert_eq!(
        w.ticks_of(|e| matches!(e, Ev::Tracer(..))),
        [117, 117, 119, 119, 121, 121]
    );
    assert_eq!(w.ticks_of(|e| *e == Ev::Target), [117, 119, 121]);
}

#[test]
fn it_keeps_firing_blind_for_its_persistence_after_load() {
    // A wall hides the player; the load-time sighting still counts.
    let mut k = gun();
    k.persistence = 60;
    let mut t = TankBrain::spawn(&k, ORIGIN, 100);
    let mut w = World::new(Pick::Low, [600, 50, 0]);
    w.walls.push(Wall { axis: 0, at: 300 });
    run(&mut t, &mut w, 100, 200);
    assert_eq!(w.ticks_of(|e| matches!(e, Ev::Tracer(..))), [125, 135]);
    assert!(w.ticks_of(|e| matches!(e, Ev::Hurt(..))).is_empty());
}

#[test]
fn line_of_sight_tanks_fire_off_target_only_when_the_barrel_line_reaches_the_player() {
    let mut k = gun();
    k.spawnflags |= 0x10;
    k.tolerance = [0, 0]; // never "aimed"
    let mut t = TankBrain::spawn(&k, ORIGIN, 100);
    let mut w = World::new(Pick::Low, [600, 50, 0]);
    run(&mut t, &mut w, 100, 150);
    assert_eq!(w.ticks_of(|e| matches!(e, Ev::Tracer(..))), [125, 135, 145]);
    // Same tank, player off the barrel line: no rounds while it turns.
    let mut t = TankBrain::spawn(&k, ORIGIN, 100);
    let mut w = World::new(Pick::Low, [600, 50, 400]);
    run(&mut t, &mut w, 100, 120);
    assert!(w.events.is_empty());
}

#[test]
fn gun_spread_follows_the_five_step_table_over_4096_units() {
    for (index, lateral) in [
        (0, 0),
        (1, 102),
        (2, 205),
        (3, 410),
        (4, 1024),
        (5, 1024),
        (7, 1024),
    ] {
        let mut k = gun();
        k.kind = (index << 2) | 2;
        let t = TankBrain::spawn(&k, ORIGIN, 0);
        let mut w = World::new(Pick::High, [0, -5000, 0]);
        let mut cd = 0;
        assert!(t.player_fire(ORIGIN, &mut cd, &mut w));
        let b = [32, 64, 0];
        assert_eq!(
            w.events[1].1,
            Ev::Tracer(b, [b[0] + 4096, b[1] + lateral, b[2] - lateral]),
            "spread index {index}"
        );
    }
}

#[test]
fn ai_rounds_hurt_the_player_with_bullet_or_skill_damage() {
    for (damage, want) in [(0, 12), (40, 40), (300, 255)] {
        let mut k = gun();
        k.kind = 2; // MP5 bullets, no spread
        k.damage = damage;
        let mut t = TankBrain::spawn(&k, ORIGIN, 100);
        let mut w = World::new(Pick::Low, [600, 50, 0]);
        run(&mut t, &mut w, 100, 125);
        assert_eq!(w.events[1], (125, Ev::Hurt(want, [32, 64, 0])));
    }
}

#[test]
fn rockets_launch_one_per_round_and_mortars_explode_once_per_volley() {
    let mut k = gun();
    k.fire_rate = 20 * 256;
    k.kind = 2 << 5;
    let mut t = TankBrain::spawn(&k, ORIGIN, 100);
    let mut w = World::new(Pick::Low, [600, 50, 0]);
    run(&mut t, &mut w, 100, 117);
    assert_eq!(
        w.events,
        [
            (117, Ev::Rocket([32, 64, 0], [4096, 0, 0], false)),
            (117, Ev::Rocket([32, 64, 0], [4096, 0, 0], false)),
            (117, Ev::Target),
        ]
    );
    k.kind = 3 << 5;
    k.damage = 120;
    let mut t = TankBrain::spawn(&k, ORIGIN, 100);
    let mut w = World::new(Pick::Low, [600, 50, 0]);
    w.walls.push(Wall { axis: 0, at: 1000 });
    run(&mut t, &mut w, 100, 117);
    assert_eq!(
        w.events,
        [
            (117, Ev::Tracer([32, 64, 0], [1000, 64, 0])),
            (117, Ev::Explode([1000, 64, 0], 120, 300, false)),
            (117, Ev::Target),
        ]
    );
}

#[test]
fn uses_switch_it_on_and_off_and_toggle() {
    let mut t = TankBrain::spawn(&gun(), ORIGIN, 0);
    t.use_tank(TankUse::On, 5);
    assert!(t.active());
    t.use_tank(TankUse::Off, 6);
    assert!(!t.active());
    t.use_tank(TankUse::Off, 7);
    assert!(!t.active());
    t.use_tank(TankUse::Toggle, 8);
    assert!(t.active());
    t.use_tank(TankUse::Toggle, 9);
    assert!(!t.active());
}

#[test]
fn a_control_volume_hands_over_the_tank_within_32_units_if_the_master_allows() {
    let mut k = gun();
    k.spawnflags |= 0x20;
    k.controls[1] = ([100, 0, 100], [200, 72, 200]);
    k.n_controls = 2;
    let t = TankBrain::spawn(&k, ORIGIN, 0);
    assert_eq!(t.try_control([68, 36, 150], || true), ControlAttempt::Taken);
    assert_eq!(
        t.try_control([67, 36, 150], || true),
        ControlAttempt::NotHere
    );
    assert_eq!(
        t.try_control([232, 104, 232], || true),
        ControlAttempt::Taken
    );
    assert_eq!(
        t.try_control([150, 36, 150], || false),
        ControlAttempt::Refused
    );
    // Without the controllable flag no volume works.
    k.spawnflags = 1;
    let t = TankBrain::spawn(&k, ORIGIN, 0);
    assert_eq!(
        t.try_control([150, 36, 150], || true),
        ControlAttempt::NotHere
    );
}

#[test]
fn the_player_keeps_the_tank_within_30_units_of_where_they_took_it() {
    assert!(still_controlled([29, 0, 0], [0; 3]));
    assert!(!still_controlled([30, 0, 0], [0; 3]));
    assert!(still_controlled([17, 17, 17], [0; 3]));
}

#[test]
fn the_mounted_gun_fires_once_per_one_over_fire_rate() {
    let t = TankBrain::spawn(&gun(), ORIGIN, 0);
    let mut w = World::new(Pick::Low, [0, -5000, 0]);
    let mut cd = 0u16;
    let mut shots = Vec::new();
    for tick in 0..30u16 {
        cd = cooldown_after_tick(cd);
        w.now = tick;
        if t.player_fire(ORIGIN, &mut cd, &mut w) {
            shots.push(tick);
        }
    }
    assert_eq!(shots, [0, 10, 20]);
    assert_eq!(w.ticks_of(|e| *e == Ev::GunSound), [0, 10, 20]);
    assert!(w
        .events
        .iter()
        .all(|e| !matches!(e.1, Ev::Hurt(..) | Ev::Rocket(..))));
}

#[test]
fn a_fire_rate_of_zero_counts_as_one_round_a_second_or_slower() {
    let mut k = gun();
    k.fire_rate = 0;
    let t = TankBrain::spawn(&k, ORIGIN, 0);
    let mut w = World::new(Pick::Low, [0, -5000, 0]);
    let mut cd = 0u16;
    assert!(t.player_fire(ORIGIN, &mut cd, &mut w));
    assert_eq!(cd, u16::MAX - 256);
}

#[test]
fn only_guns_make_the_mounted_gun_sound_and_mounted_rockets_belong_to_the_player() {
    let mut k = gun();
    k.kind = 2 << 5;
    let t = TankBrain::spawn(&k, ORIGIN, 0);
    let mut w = World::new(Pick::Low, [0, -5000, 0]);
    let mut cd = 0;
    t.player_fire(ORIGIN, &mut cd, &mut w);
    assert_eq!(
        w.events,
        [
            (0, Ev::Rocket([32, 64, 0], [4096, 0, 0], true)),
            (0, Ev::Target)
        ]
    );
}

#[test]
fn a_mounted_tank_follows_the_view_every_tick_without_firing_itself() {
    let mut t = TankBrain::spawn(&gun(), ORIGIN, 0);
    let mut w = World::new(Pick::Low, [0, 0, 0]);
    for now in 0..6u16 {
        w.now = now;
        let mut i = inputs(&w);
        i.controlled = true;
        i.view = [2896, 0, 2896]; // 45 degrees left
        t.tick(&i, &mut w);
    }
    assert!(w.pvs_asks.is_empty());
    assert!(w.events.is_empty());
    assert_eq!(t.angles(), [150, 0]);
    assert_eq!(t.angular_velocity(), [30, 0]);
}

#[test]
fn a_removed_tank_stops_thinking_but_finishes_its_last_turn() {
    let mut t = TankBrain::spawn(&gun(), ORIGIN, 0);
    let mut w = World::new(Pick::Low, [700, 50, 700]);
    run(&mut t, &mut w, 0, 15);
    for now in 16..20u16 {
        w.now = now;
        let mut i = inputs(&w);
        i.removed = true;
        t.tick(&i, &mut w);
    }
    assert_eq!(w.pvs_asks, [15]);
    // The think at 15 set 30 a tick; ticks 16-19 still integrate it.
    assert_eq!(t.angles(), [120, 0]);
}

/// Recorded from ac83da7 behaviour: an MP5 gun tank tracking a player who
/// walks across its arc at 15 units a tick for four seconds, drawing from
/// the game's generator. Angles every fourth tick, then every event.
#[test]
fn golden_gun_tank_tracks_a_walking_player() {
    let mut t = TankBrain::spawn(&gun(), ORIGIN, 0);
    let mut w = World::new(Pick::Game(Lcg(Lcg::GAME_SEED)), [800, 40, -600]);
    let mut angles = Vec::new();
    for now in 0..80u16 {
        w.now = now;
        w.player[2] = -600 + 15 * now as i32;
        let i = inputs(&w);
        t.tick(&i, &mut w);
        if now % 4 == 0 {
            angles.push(t.angles());
        }
    }
    assert_eq!(
        angles,
        [
            [0, 0],
            [0, 0],
            [0, 0],
            [0, 0],
            [4066, 4],
            [3946, 8],
            [3884, 8],
            [3927, 8],
            [3972, 8],
            [4020, 8],
            [4070, 8],
            [25, 8],
            [74, 8],
            [122, 8],
            [169, 8],
            [212, 8],
            [254, 8],
            [294, 8],
            [331, 8],
            [367, 8]
        ]
    );
    assert_eq!(
        w.events,
        [
            (31, Ev::Tracer([31, 63, -7], [4035, 32, -858])),
            (31, Ev::Target),
            (41, Ev::Tracer([31, 63, -1], [4123, -50, -109])),
            (41, Ev::Target),
            (51, Ev::Tracer([31, 63, 5], [4063, 3, 708])),
            (51, Ev::Target),
            (61, Ev::Tracer([30, 63, 10], [3893, 56, 1368])),
            (61, Ev::Target),
            (71, Ev::Tracer([28, 63, 15], [3599, 3, 2014])),
            (71, Ev::Hurt(12, [28, 63, 15])),
            (71, Ev::Target)
        ]
    );
}

/// Recorded from ac83da7 behaviour: a line-of-sight mortar with an offset
/// barrel and a fast fire rate shelling a player who stands still beyond a
/// wall it can see over, for three seconds.
#[test]
fn golden_offset_mortar_with_line_of_sight() {
    let mut k = gun();
    k.spawnflags = 1 | 0x10;
    k.kind = (3 << 5) | (2 << 2);
    k.damage = 150;
    k.fire_rate = 3 * 256;
    k.barrel = [48, 8, 12];
    k.persistence = 30;
    let mut t = TankBrain::spawn(&k, ORIGIN, 0);
    let mut w = World::new(Pick::Game(Lcg(99)), [900, 0, 300]);
    w.walls.push(Wall { axis: 0, at: 1400 });
    let mut angles = Vec::new();
    for now in 0..60u16 {
        w.now = now;
        let i = inputs(&w);
        t.tick(&i, &mut w);
        if now % 5 == 0 {
            angles.push(t.angles());
        }
    }
    assert_eq!(
        angles,
        [
            [0, 0],
            [0, 0],
            [0, 0],
            [0, 0],
            [150, 44],
            [216, 44],
            [216, 44],
            [216, 44],
            [216, 44],
            [216, 44],
            [216, 44],
            [216, 44]
        ]
    );
    assert_eq!(w.poses.len(), 8);
    assert_eq!(
        w.events,
        [
            (31, Ev::Tracer([47, 71, 7], [1399, 28, 495])),
            (31, Ev::Explode([1399, 28, 495], 150, 375, false)),
            (31, Ev::Target),
            (39, Ev::Tracer([47, 71, 7], [1399, -35, 434])),
            (39, Ev::Explode([1399, -35, 434], 150, 375, false)),
            (39, Ev::Target),
            (47, Ev::Tracer([47, 71, 7], [1399, -1, 528])),
            (47, Ev::Explode([1399, -1, 528], 150, 375, false)),
            (47, Ev::Target),
            (55, Ev::Tracer([47, 71, 7], [1399, -23, 533])),
            (55, Ev::Explode([1399, -23, 533], 150, 375, false)),
            (55, Ev::Target)
        ]
    );
}

/// Recorded from ac83da7 behaviour: a mounted 12mm gun with a 0.1 spread
/// swept by the player's view while the trigger is held for two seconds.
#[test]
fn golden_mounted_gun_sweep() {
    let mut k = gun();
    k.kind = 3 | (3 << 2);
    k.fire_rate = 5 * 256;
    let mut t = TankBrain::spawn(&k, ORIGIN, 0);
    let mut w = World::new(Pick::Game(Lcg(1234)), [0, 0, 0]);
    w.walls.push(Wall { axis: 0, at: 2000 });
    let mut cd = 0u16;
    let mut log = Vec::new();
    for now in 0..40u16 {
        w.now = now;
        cd = cooldown_after_tick(cd);
        let mut i = inputs(&w);
        i.controlled = true;
        let yaw = (now as i32 - 20) * 40;
        i.view = [
            psx_math::sincos::sin_q12(((yaw + 1024) & 0xfff) as u16),
            -300,
            psx_math::sincos::sin_q12((yaw & 0xfff) as u16),
        ];
        t.tick(&i, &mut w);
        let fired = t.player_fire(ORIGIN, &mut cd, &mut w);
        log.push((t.angles(), cd, fired));
    }
    assert_eq!(
        log,
        [
            ([0, 0], 1024, true),
            ([4066, 25], 768, false),
            ([4036, 37], 512, false),
            ([4006, 43], 256, false),
            ([3976, 46], 1024, true),
            ([3946, 48], 768, false),
            ([3916, 49], 512, false),
            ([3886, 49], 256, false),
            ([3856, 49], 1024, true),
            ([3826, 49], 768, false),
            ([3796, 49], 512, false),
            ([3766, 49], 256, false),
            ([3753, 49], 1024, true),
            ([3765, 49], 768, false),
            ([3791, 49], 512, false),
            ([3821, 49], 256, false),
            ([3851, 49], 1024, true),
            ([3881, 49], 768, false),
            ([3911, 49], 512, false),
            ([3941, 49], 256, false),
            ([3971, 49], 1024, true),
            ([4001, 49], 768, false),
            ([4031, 49], 512, false),
            ([4061, 49], 256, false),
            ([4091, 49], 1024, true),
            ([25, 49], 768, false),
            ([55, 49], 512, false),
            ([85, 49], 256, false),
            ([115, 49], 1024, true),
            ([145, 49], 768, false),
            ([175, 49], 512, false),
            ([205, 49], 256, false),
            ([235, 49], 1024, true),
            ([265, 49], 768, false),
            ([295, 49], 512, false),
            ([325, 49], 256, false),
            ([355, 49], 1024, true),
            ([385, 49], 768, false),
            ([415, 49], 512, false),
            ([445, 49], 256, false)
        ]
    );
    assert_eq!(
        w.events,
        [
            (0, Ev::GunSound),
            (0, Ev::Tracer([32, 64, 0], [2000, 39, 19])),
            (0, Ev::PlayerRound([32, 64, 0], [4128, 14, 40], 13)),
            (0, Ev::Target),
            (4, Ev::GunSound),
            (4, Ev::Tracer([31, 61, -6], [1999, -14, -213])),
            (4, Ev::PlayerRound([31, 61, -6], [4115, -94, -434], 13)),
            (4, Ev::Target),
            (8, Ev::GunSound),
            (8, Ev::Tracer([29, 61, -12], [1999, -12, -846])),
            (8, Ev::PlayerRound([29, 61, -12], [3803, -77, -1609], 13)),
            (8, Ev::Target),
            (12, Ev::GunSound),
            (12, Ev::Tracer([27, 61, -17], [1999, -216, -1383])),
            (12, Ev::PlayerRound([27, 61, -17], [3384, -410, -2341], 13)),
            (12, Ev::Target),
            (16, Ev::GunSound),
            (16, Ev::Tracer([29, 61, -12], [1999, -198, -689])),
            (16, Ev::PlayerRound([29, 61, -12], [3881, -444, -1334], 13)),
            (16, Ev::Target),
            (20, Ev::GunSound),
            (20, Ev::Tracer([31, 61, -7], [1999, -83, -450])),
            (20, Ev::PlayerRound([31, 61, -7], [4017, -230, -902], 13)),
            (20, Ev::Target),
            (24, Ev::GunSound),
            (24, Ev::Tracer([31, 61, -1], [1999, -139, 111])),
            (24, Ev::PlayerRound([31, 61, -1], [4108, -353, 231], 13)),
            (24, Ev::Target),
            (28, Ev::GunSound),
            (28, Ev::Tracer([31, 61, 5], [1999, -165, 175])),
            (28, Ev::PlayerRound([31, 61, 5], [4103, -406, 357], 13)),
            (28, Ev::Target),
            (32, Ev::GunSound),
            (32, Ev::Tracer([29, 61, 11], [1999, -149, 768])),
            (32, Ev::PlayerRound([29, 61, 11], [3833, -344, 1473], 13)),
            (32, Ev::Target),
            (36, Ev::GunSound),
            (36, Ev::Tracer([27, 61, 16], [1999, -18, 1174])),
            (36, Ev::PlayerRound([27, 61, 16], [3557, -79, 2089], 13)),
            (36, Ev::Target)
        ]
    );
}
