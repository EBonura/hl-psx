//! Osprey (HV-04): start and snapshot, leg timing and blending, corner
//! skipping, deploy and replacement slots, and the crash.

use crate::behaviour::setpiece_kit::{trace_walls, Lcg, Pick, Wall};
use crate::osprey_logic::{Corner, OspreyBrain, OspreyInputs, OspreyWorld};
use crate::setpiece_math::TraceHit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    Remove,
    Explode([i32; 3], u8, i32, bool),
    Fx([i32; 3], u8),
    StopRotor,
    Respawn(usize, [i32; 3], u16),
    Landed(usize),
}

#[derive(Clone, Copy, Debug)]
struct Prop {
    grunt: bool,
    active: bool,
    health: bool,
    on_rope: bool,
    pos: [i32; 3],
}

const GRUNT: Prop = Prop {
    grunt: true,
    active: true,
    health: true,
    on_rope: false,
    pos: [0; 3],
};

struct World {
    pick: Pick,
    corners: Vec<Corner>,
    attitude: (u16, i16),
    props: Vec<Prop>,
    floor: i32,
    walls: Vec<Wall>,
    now: u16,
    attitude_asks: Vec<u16>,
    pos: Vec<(u16, [i32; 3])>,
    yaw: Vec<(u16, u16)>,
    tilt: Vec<(u16, u16)>,
    rotor: Vec<(u16, [i32; 3])>,
    ropes: Vec<(u16, [i32; 3], [i32; 3])>,
    events: Vec<(u16, Ev)>,
}

impl World {
    fn new(corners: Vec<Corner>, grunts: usize) -> Self {
        Self {
            pick: Pick::Low,
            corners,
            attitude: (0x0302, 1024),
            props: vec![GRUNT; grunts],
            floor: -400,
            walls: Vec::new(),
            now: 0,
            attitude_asks: Vec::new(),
            pos: Vec::new(),
            yaw: Vec::new(),
            tilt: Vec::new(),
            rotor: Vec::new(),
            ropes: Vec::new(),
            events: Vec::new(),
        }
    }
    fn ev(&mut self, e: Ev) {
        self.events.push((self.now, e));
    }
    fn kill(&mut self, pi: usize) {
        self.props[pi].health = false;
    }
}

impl OspreyWorld for World {
    fn corner(&mut self, k: usize) -> Corner {
        self.corners[k]
    }
    fn authored_attitude(&mut self) -> (u16, i16) {
        self.attitude_asks.push(self.now);
        self.attitude
    }
    fn grunt_scan_slots(&mut self) -> usize {
        self.props.len()
    }
    fn is_live_grunt(&mut self, pi: usize) -> bool {
        let p = self.props[pi];
        p.grunt && p.active && p.health
    }
    fn is_down(&mut self, pi: usize) -> bool {
        self.props.get(pi).is_some_and(|p| !p.active || !p.health)
    }
    fn is_dead(&mut self, pi: usize) -> bool {
        !self.props[pi].health
    }
    fn respawn_grunt(&mut self, pi: usize, at: [i32; 3], yaw: u16) -> i32 {
        self.props[pi] = Prop {
            on_rope: true,
            pos: at,
            ..GRUNT
        };
        self.ev(Ev::Respawn(pi, at, yaw));
        self.floor
    }
    fn is_on_rope(&mut self, pi: usize) -> bool {
        self.props[pi].on_rope
    }
    fn grunt_pos(&mut self, pi: usize) -> [i32; 3] {
        self.props[pi].pos
    }
    fn set_grunt_pos(&mut self, pi: usize, pos: [i32; 3]) {
        self.props[pi].pos = pos;
    }
    fn land_grunt(&mut self, pi: usize) {
        self.props[pi].on_rope = false;
        self.ev(Ev::Landed(pi));
    }
    fn rope(&mut self, from: [i32; 3], to: [i32; 3]) {
        self.ropes.push((self.now, from, to));
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        trace_walls(&self.walls, from, to)
    }
    fn random_below(&mut self, n: u32) -> u32 {
        self.pick.below(n)
    }
    fn explosion_fx(&mut self, at: [i32; 3], mag: u8) {
        self.ev(Ev::Fx(at, mag));
    }
    fn explode(&mut self, at: [i32; 3], damage: u8, radius: i32, by_player: bool) {
        self.ev(Ev::Explode(at, damage, radius, by_player));
    }
    fn remove(&mut self) {
        self.ev(Ev::Remove);
    }
    fn rotor(&mut self, at: [i32; 3]) {
        self.rotor.push((self.now, at));
    }
    fn stop_rotor(&mut self) {
        self.ev(Ev::StopRotor);
    }
    fn set_pos(&mut self, pos: [i32; 3]) {
        self.pos.push((self.now, pos));
    }
    fn set_yaw(&mut self, yaw: u16) {
        self.yaw.push((self.now, yaw));
    }
    fn set_tilt(&mut self, tilt: u16) {
        self.tilt.push((self.now, tilt));
    }
}

/// Tracks the osprey prop the way the game does: its position is whatever
/// the brain last set, and it may be killed on a given tick.
struct Run {
    o: OspreyBrain,
    w: World,
    at: [i32; 3],
    dead: bool,
    gone: bool,
}

impl Run {
    fn new(corners: Vec<Corner>, grunts: usize, wait: bool, now: u16) -> Self {
        let mut o = OspreyBrain::new();
        let at = [0, 0, 0];
        o.bind(corners.len() * 3, 0, at, wait, now);
        Self {
            o,
            w: World::new(corners, grunts),
            at,
            dead: false,
            gone: false,
        }
    }
    fn tick(&mut self, now: u16) {
        if self.gone {
            return;
        }
        self.w.now = now;
        let i = OspreyInputs {
            now,
            dead: self.dead,
            pos: self.at,
        };
        let before = self.w.pos.len();
        self.o.tick(&i, &mut self.w);
        if self.w.pos.len() > before {
            self.at = self.w.pos.last().unwrap().1;
        }
        if self.w.events.iter().any(|e| e.1 == Ev::Remove) {
            self.gone = true;
        }
    }
    fn run(&mut self, from: u16, to: u16) {
        for now in from..=to {
            self.tick(now);
        }
    }
    fn pos_at(&self, t: u16) -> [i32; 3] {
        self.w.pos.iter().find(|p| p.0 == t).unwrap().1
    }
}

/// A corner at `x` along +x, flying +x (yaw a quarter turn) at `speed`.
fn east(x: i16, speed: u16) -> Corner {
    Corner {
        pos: [x, 0, 0],
        speed,
        ang: [0, 1024, 0],
    }
}

#[test]
fn it_wakes_one_second_after_load_and_takes_the_authored_attitude() {
    let mut r = Run::new(vec![east(1000, 500)], 1, false, 100);
    r.run(100, 125);
    assert_eq!(r.w.attitude_asks, [120]);
    assert_eq!(r.w.tilt[0], (120, 0x0302));
    // Flying starts the tick after the wake think.
    assert_eq!(r.w.pos[0].0, 121);
}

#[test]
fn a_waiting_osprey_sleeps_until_triggered() {
    let mut r = Run::new(vec![east(1000, 500)], 1, true, 100);
    r.run(100, 300);
    assert!(r.w.attitude_asks.is_empty());
    r.o.command_use(300);
    r.run(301, 303);
    assert_eq!(r.w.attitude_asks, [302]);
}

#[test]
fn with_no_living_grunts_it_disappears() {
    let mut r = Run::new(vec![east(1000, 500)], 3, false, 0);
    for pi in 0..3 {
        r.w.kill(pi);
    }
    r.run(0, 40);
    assert_eq!(r.w.events, [(20, Ev::Remove)]);
    assert!(r.w.pos.is_empty());
}

#[test]
fn each_leg_lasts_twice_its_length_over_the_two_corner_speeds() {
    // From rest at the origin to 1000 units at 500 u/s: 2 * 1000 / 500 s.
    let mut r = Run::new(vec![east(1000, 500), east(2000, 500)], 1, false, 0);
    r.run(0, 200);
    assert_eq!(r.pos_at(100), [1000, 0, 0]);
    assert!(r.pos_at(99)[0] < 1000);
    // The next leg, 1000 units at 500 + 500, takes 2 s at constant speed.
    for t in [100u16, 110, 120, 130, 140] {
        assert_eq!(
            r.pos_at(t),
            [1000 + 25 * (t as i32 - 100), 0, 0],
            "tick {t}"
        );
    }
}

#[test]
fn a_leg_eases_in_and_out_from_rest() {
    // Both ends head across the leg (+z), so along it the position is a
    // pure smoothstep: 0.15625, 0.5 and 0.84375 of the way at a quarter,
    // half and three quarters of the 2 s leg.
    let north = |x: i16| Corner {
        pos: [x, 0, 0],
        speed: 500,
        ang: [0, 0, 0],
    };
    let mut r = Run::new(vec![north(1000), north(2000)], 1, false, 0);
    r.run(0, 140);
    let x: Vec<i32> = [100u16, 110, 120, 130, 140]
        .iter()
        .map(|&t| r.pos_at(t)[0])
        .collect();
    assert_eq!(x, [1000, 1156, 1500, 1843, 2000]);
}

#[test]
fn heading_turns_the_short_way_across_north() {
    let c0 = Corner {
        pos: [1000, 0, 0],
        speed: 400,
        ang: [0, 4000, 0],
    };
    let c1 = Corner {
        pos: [2000, 0, 0],
        speed: 400,
        ang: [0, 100, 0],
    };
    let mut r = Run::new(vec![c0, c1], 1, false, 0);
    r.w.attitude = (0, 4000);
    r.run(0, 400);
    // The leg toward c1 starts at the first arrival (2 s at 1000 / 400).
    let leg: Vec<u16> =
        r.w.yaw
            .iter()
            .filter(|y| (120..=170).contains(&y.0))
            .map(|y| y.1)
            .collect();
    assert!(leg.iter().all(|&y| y >= 4000 || y <= 100), "{leg:?}");
    assert!(leg.contains(&0) || leg.iter().any(|&y| y < 50));
}

#[test]
fn it_skips_slow_corners_while_every_grunt_lives() {
    let chain = vec![east(1000, 500), east(1500, 0), east(2500, 500)];
    let mut r = Run::new(chain.clone(), 2, false, 0);
    r.run(0, 400);
    assert!(!r.w.events.iter().any(|e| matches!(e.1, Ev::Respawn(..))));
    // It went from the first corner straight to the third.
    assert_eq!(r.pos_at(100), [1000, 0, 0]);
    assert_eq!(r.pos_at(140), [2000, 0, 0]);
    // Once a grunt dies it takes the slow corner and deploys there.
    let mut r = Run::new(chain, 2, false, 0);
    r.run(0, 50);
    r.w.kill(1);
    r.run(51, 400);
    assert!(r.w.events.iter().any(|e| matches!(e.1, Ev::Respawn(1, ..))));
}

#[test]
fn it_snapshots_at_most_24_grunts() {
    let chain = vec![east(1000, 500), east(1500, 0), east(2500, 500)];
    let mut r = Run::new(chain, 30, false, 0);
    r.run(0, 30);
    for pi in 24..30 {
        r.w.kill(pi);
    }
    r.run(31, 400);
    // Grunts it never counted do not make it stop.
    assert!(!r.w.events.iter().any(|e| matches!(e.1, Ev::Respawn(..))));
}

#[test]
fn it_drops_four_replacements_from_the_rope_points() {
    let chain = vec![east(1000, 0), east(3000, 500)];
    let mut r = Run::new(chain, 6, false, 0);
    r.run(0, 30);
    for pi in [0, 2, 3, 4, 5] {
        r.w.kill(pi);
    }
    r.run(31, 4100);
    let drops: Vec<Ev> =
        r.w.events
            .iter()
            .filter(|e| matches!(e.1, Ev::Respawn(..)))
            .map(|e| e.1)
            .collect();
    // Facing +x: right is -z. 32 ahead / 64 behind, 100 either side, 96 down,
    // filling the fallen snapshot slots in order; a fifth waits.
    assert_eq!(
        drops,
        [
            Ev::Respawn(0, [1032, -96, -100], 1024),
            Ev::Respawn(2, [936, -96, -100], 1024),
            Ev::Respawn(3, [1032, -96, 100], 1024),
            Ev::Respawn(4, [936, -96, 100], 1024),
        ]
    );
    assert_eq!(r.o.ropes(), [Some(0), Some(2), Some(3), Some(4)]);
}

#[test]
fn rope_grunts_slide_down_8_a_tick_and_it_leaves_once_all_are_down() {
    let chain = vec![east(1000, 0), east(3000, 500)];
    let mut r = Run::new(chain, 2, false, 0);
    r.w.floor = -200;
    r.run(0, 30);
    r.w.kill(1);
    // Arrival at the stop corner is capped at 4000 ticks (from rest to a
    // 0-speed corner), so run long.
    let mut landed = None;
    let mut left = None;
    let mut t = 31u16;
    while t < 4300 {
        r.tick(t);
        if landed.is_none() && r.w.events.iter().any(|e| e.1 == Ev::Landed(1)) {
            landed = Some(t);
        }
        if landed.is_some() && left.is_none() && r.w.pos.last().is_some_and(|p| p.0 == t) {
            left = Some(t);
        }
        t += 1;
    }
    let landed = landed.expect("the grunt landed");
    let drop =
        r.w.events
            .iter()
            .find(|e| matches!(e.1, Ev::Respawn(..)))
            .unwrap()
            .0;
    // 96 below a hull at y 0 down to -200: 104 units at 8 a tick, the
    // first slide on the drop tick itself.
    assert_eq!(landed - drop, 12);
    assert_eq!(r.w.props[1].pos[1], -200);
    // Flight resumes on the next think after the landing.
    let left = left.expect("it flew on");
    assert!(left > landed && left - landed <= 2, "{landed} {left}");
}

#[test]
fn shot_down_it_falls_at_three_tenths_gravity_and_explodes_after_four_seconds() {
    let mut r = Run::new(vec![east(1000, 500), east(2000, 500)], 1, false, 0);
    r.run(0, 110);
    let vx = r.o.velocity()[0];
    r.dead = true;
    r.run(111, 300);
    let fall: Vec<i32> =
        r.w.pos
            .iter()
            .filter(|p| p.0 > 110)
            .map(|p| p.1[1])
            .collect();
    // 12 u/s lost per tick, moving velocity / 20 a tick: drops of 0, 1,
    // 1 and 2 units.
    assert_eq!(&fall[..4], [0, -1, -2, -4]);
    let first = r.w.pos.iter().find(|p| p.0 == 111).unwrap().1;
    assert_eq!(first[0], r.pos_at(110)[0] + vx as i32 / 20);
    let boom: Vec<(u16, Ev)> =
        r.w.events
            .iter()
            .copied()
            .filter(|e| !matches!(e.1, Ev::Fx(..)))
            .collect();
    assert_eq!(boom.len(), 3);
    assert_eq!(boom[0].0, 111 + 80);
    assert!(matches!(boom[0].1, Ev::Explode(_, 255, 750, false)));
    assert_eq!(&boom[1..], [(191, Ev::Remove), (191, Ev::StopRotor)]);
}

#[test]
fn a_falling_osprey_explodes_on_first_impact() {
    let mut r = Run::new(vec![east(1000, 500), east(2000, 500)], 1, false, 0);
    r.run(0, 110);
    r.w.walls.push(Wall { axis: 1, at: -20 });
    r.dead = true;
    r.run(111, 300);
    let ex =
        r.w.events
            .iter()
            .find(|e| matches!(e.1, Ev::Explode(..)))
            .unwrap();
    assert!(ex.0 < 191);
    assert!(r.gone);
}

/// Recorded from ac83da7 behaviour: an osprey with six grunts flies a
/// four-corner loop that includes one stop corner. Two grunts die in
/// flight; it diverts to the stop, ropes two replacements down and flies
/// on. Positions every 20 ticks, every yaw change at those ticks, events.
#[test]
fn golden_resupply_loop() {
    let chain = vec![
        Corner {
            pos: [800, 100, 0],
            speed: 600,
            ang: [0, 1024, 0],
        },
        Corner {
            pos: [1600, 50, 800],
            speed: 300,
            ang: [-32, 0, 48],
        },
        Corner {
            pos: [1600, 0, 1600],
            speed: 0,
            ang: [0, 3800, 0],
        },
        Corner {
            pos: [400, 200, 1200],
            speed: 700,
            ang: [16, 2600, -16],
        },
    ];
    let mut r = Run::new(chain, 6, false, 10);
    r.w.floor = -300;
    let mut track = Vec::new();
    for now in 10..=900u16 {
        if now == 150 {
            r.w.kill(1);
            r.w.kill(4);
        }
        r.tick(now);
        if now % 20 == 0 {
            track.push((now, r.at, r.w.yaw.last().map(|y| y.1)));
        }
    }
    assert_eq!(
        track,
        [
            (20, [0, 0, 0], None),
            (40, [-46, 9, 0], Some(1024)),
            (60, [65, 59, 0], Some(1024)),
            (80, [703, 99, 0], Some(1024)),
            (100, [1175, 140, 690], Some(1660)),
            (120, [463, 199, 1236], Some(2587)),
            (140, [35, 155, 436], Some(1902)),
            (160, [767, 100, 0], Some(1027)),
            (180, [1444, 83, 108], Some(692)),
            (200, [1645, 56, 556], Some(127)),
            (220, [1600, 48, 948], Some(4090)),
            (240, [1600, 40, 1302], Some(4041)),
            (260, [1600, 27, 1563], Some(3964)),
            (280, [1600, 14, 1665], Some(3883)),
            (300, [1600, 3, 1636], Some(3820)),
            (320, [1600, 0, 1600], Some(3800)),
            (340, [1600, 0, 1600], Some(3800)),
            (360, [1633, 19, 1693], Some(3684)),
            (380, [1520, 89, 1826], Some(3261)),
            (400, [1016, 166, 1633], Some(2801)),
            (420, [372, 199, 1175], Some(2597)),
            (440, [78, 144, 315], Some(1720)),
            (460, [863, 100, 15], Some(1035)),
            (480, [1094, 159, 948], Some(1962)),
            (500, [311, 198, 1111], Some(2573)),
            (520, [127, 136, 245], Some(1603)),
            (540, [931, 102, 59], Some(1070)),
            (560, [1032, 166, 1037], Some(2079)),
            (580, [246, 195, 1035], Some(2528)),
            (600, [193, 129, 182], Some(1487)),
            (620, [998, 106, 124], Some(1124)),
            (640, [958, 174, 1111], Some(2190)),
            (660, [185, 191, 950], Some(2466)),
            (680, [271, 122, 129], Some(1379)),
            (700, [1058, 110, 208], Some(1195)),
            (720, [872, 180, 1172], Some(2294)),
            (740, [128, 186, 857], Some(2386)),
            (760, [358, 116, 86], Some(1282)),
            (780, [1109, 116, 306], Some(1282)),
            (800, [779, 186, 1217], Some(2386)),
            (820, [82, 180, 762], Some(2294)),
            (840, [453, 110, 52], Some(1196)),
            (860, [1148, 122, 411], Some(1379)),
            (880, [682, 191, 1246], Some(2466)),
            (900, [48, 174, 666], Some(2192))
        ]
    );
    assert_eq!(
        r.w.events,
        [
            (320, Ev::Respawn(1, [1509, -96, 1653], 2600)),
            (320, Ev::Respawn(4, [1581, -96, 1717], 2600)),
            (345, Ev::Landed(1)),
            (345, Ev::Landed(4))
        ]
    );
    assert_eq!(r.w.ropes.len(), 52);
}

/// Recorded from ac83da7 behaviour: shot down mid-leg over a floor, drawing
/// fireball scatter from the game's generator until it hits the ground.
#[test]
fn golden_crash() {
    let mut r = Run::new(vec![east(1000, 600), east(2600, 600)], 2, false, 0);
    r.w.pick = Pick::Game(Lcg(Lcg::GAME_SEED));
    r.w.walls.push(Wall { axis: 1, at: -150 });
    r.run(0, 130);
    r.dead = true;
    r.run(131, 260);
    let fall: Vec<(u16, [i32; 3])> = r.w.pos.iter().copied().filter(|p| p.0 > 130).collect();
    assert_eq!(
        fall,
        [
            (131, [2359, 0, 0]),
            (132, [2389, -1, 0]),
            (133, [2419, -2, 0]),
            (134, [2449, -4, 0]),
            (135, [2479, -7, 0]),
            (136, [2509, -10, 0]),
            (137, [2539, -14, 0]),
            (138, [2569, -18, 0]),
            (139, [2599, -23, 0]),
            (140, [2629, -29, 0]),
            (141, [2659, -35, 0]),
            (142, [2689, -42, 0]),
            (143, [2719, -49, 0]),
            (144, [2749, -57, 0]),
            (145, [2779, -66, 0]),
            (146, [2809, -75, 0]),
            (147, [2839, -85, 0]),
            (148, [2869, -95, 0]),
            (149, [2899, -106, 0]),
            (150, [2929, -118, 0]),
            (151, [2959, -130, 0]),
            (152, [2989, -143, 0])
        ]
    );
    assert_eq!(
        r.w.events,
        [
            (132, Ev::Fx([2333, -101, 66], 60)),
            (136, Ev::Fx([2473, -110, 124], 60)),
            (140, Ev::Fx([2728, -129, 111], 60)),
            (144, Ev::Fx([2793, -157, 79], 60)),
            (148, Ev::Fx([3016, -195, -20], 60)),
            (152, Ev::Fx([2997, -243, -2], 60)),
            (153, Ev::Explode([3019, -156, 0], 255, 750, false)),
            (153, Ev::Remove),
            (153, Ev::StopRotor)
        ]
    );
}
