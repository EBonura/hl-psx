//! Osprey brain: the troop carrier that flies its path_corner chain,
//! drops replacement grunts on ropes at its stop corners and crashes when
//! shot down. Free of PS1 state; the game feeds it through
//! [`OspreyWorld`].
//!
//! Positions are world units in runtime axes (y up), velocities units/s,
//! angles q12 turns [pitch, yaw, roll]; time is the 20 Hz tick.

use crate::setpiece_math::{dist2_3, time_reached, TraceHit};
use psx_math::{int32::isqrt_i32, sincos};

/// Prop slots the grunt roster can name (the game's prop table size).
pub const PROP_SLOTS: usize = 128;
/// Most grunts the osprey snapshots at the start.
const MAX_CARRY: u32 = 24;
const NO_REPEL: u8 = 0xff;

const WAIT: u8 = 0;
const FLY: u8 = 1;
const DEPLOY: u8 = 2;
const HOVER: u8 = 3;
const DYING: u8 = 4;

/// One path_corner of the chain: position, speed (0 = a deploy stop) and
/// angles [pitch, yaw, roll] in q12 turns.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Corner {
    pub pos: [i16; 3],
    pub speed: u16,
    pub ang: [i16; 3],
}

/// What the brain asks of, and tells, the game. Grunts are named by prop
/// slot, `0..PROP_SLOTS`.
pub trait OspreyWorld {
    /// Corner `k` of the osprey's chain.
    fn corner(&mut self, k: usize) -> Corner;
    /// The prop's authored attitude: the tilt word (reflected q8 pitch |
    /// roll << 8) and its yaw in q12 turns.
    fn authored_attitude(&mut self) -> (u16, i16);
    /// How many prop slots the start-up snapshot scans.
    fn grunt_scan_slots(&mut self) -> usize;
    /// Is slot `pi` a living, active grunt?
    fn is_live_grunt(&mut self, pi: usize) -> bool;
    /// Is slot `pi` inactive or at zero health?
    fn is_down(&mut self, pi: usize) -> bool;
    /// Is slot `pi` at zero health?
    fn is_dead(&mut self, pi: usize) -> bool;
    /// Bring slot `pi` back as a fresh grunt hanging from a rope at `at`,
    /// facing `yaw`; returns the floor height below it.
    fn respawn_grunt(&mut self, pi: usize, at: [i32; 3], yaw: u16) -> i32;
    /// Is slot `pi` still sliding down its rope?
    fn is_on_rope(&mut self, pi: usize) -> bool;
    fn grunt_pos(&mut self, pi: usize) -> [i32; 3];
    fn set_grunt_pos(&mut self, pi: usize, pos: [i32; 3]);
    /// Slot `pi` reached the floor and lets go of its rope.
    fn land_grunt(&mut self, pi: usize);
    fn rope(&mut self, from: [i32; 3], to: [i32; 3]);
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit>;
    /// A uniform random integer in `0..n` from the shared impact generator.
    fn random_below(&mut self, n: u32) -> u32;
    /// A harmless fireball of magnitude `mag`.
    fn explosion_fx(&mut self, at: [i32; 3], mag: u8);
    fn explode(&mut self, at: [i32; 3], damage: u8, radius: i32, by_player: bool);
    /// The osprey leaves the map.
    fn remove(&mut self);
    /// Keep the rotor loop playing at `at`.
    fn rotor(&mut self, at: [i32; 3]);
    fn stop_rotor(&mut self);
    fn set_pos(&mut self, pos: [i32; 3]);
    fn set_yaw(&mut self, yaw: u16);
    /// The drawn tilt word (reflected q8 pitch | roll << 8).
    fn set_tilt(&mut self, tilt: u16);
}

/// One tick's observations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OspreyInputs {
    pub now: u16,
    /// The osprey's prop is at zero health.
    pub dead: bool,
    /// The osprey's prop position.
    pub pos: [i32; 3],
}

/// The map's osprey.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OspreyBrain {
    corners: u8,
    loop_start: u8,
    phase: u8,
    goal: u8,
    start: u16,
    dt: u16,
    next: u16,
    spd: [u16; 2],
    p: [[i16; 3]; 2],
    v: [[i16; 3]; 2],
    a: [[i16; 3]; 2],
    vel: [i16; 3],
    repel: [u8; 4],
    repel_y: [i16; 4],
    grunts: [u32; 4],
}

impl Default for OspreyBrain {
    fn default() -> Self {
        Self::new()
    }
}

impl OspreyBrain {
    /// The power-on state, before any map binds an osprey.
    pub const fn new() -> Self {
        Self {
            corners: 1,
            loop_start: 0,
            phase: 0,
            goal: 0,
            start: 0,
            dt: 0,
            next: 0,
            spd: [0; 2],
            p: [[0; 3]; 2],
            v: [[0; 3]; 2],
            a: [[0; 3]; 2],
            vel: [0; 3],
            repel: [NO_REPEL; 4],
            repel_y: [0; 4],
            grunts: [0; 4],
        }
    }

    /// Bind a map's osprey: `corner_words` cooked aux words of chain,
    /// looping back to corner `loop_start`, standing at `pos`. It wakes 1 s
    /// after `now`, or on its first trigger when `wait_for_trigger`.
    /// Everything else carries over from the previous osprey.
    pub fn bind(
        &mut self,
        corner_words: usize,
        loop_start: u8,
        pos: [i32; 3],
        wait_for_trigger: bool,
        now: u16,
    ) {
        let o = self;
        o.corners = (corner_words / 3).max(1) as u8;
        o.loop_start = loop_start;
        o.phase = WAIT;
        o.repel = [NO_REPEL; 4];
        o.p[1] = [pos[0] as i16, pos[1] as i16, pos[2] as i16];
        o.v[1] = [0; 3];
        o.spd[1] = 0;
        o.next = if wait_for_trigger {
            0xffff
        } else {
            now.wrapping_add(20)
        };
    }

    /// A trigger: think on the next think tick.
    pub fn command_use(&mut self, now: u16) {
        self.next = now.wrapping_add(2);
    }

    /// The current velocity, units/s.
    #[allow(dead_code)] // read by the host tests
    pub fn velocity(&self) -> [i16; 3] {
        self.vel
    }

    /// Prop slots of the grunts riding the current deploy's ropes.
    #[allow(dead_code)] // read by the host tests
    pub fn ropes(&self) -> [Option<u8>; 4] {
        self.repel.map(|r| (r != NO_REPEL).then_some(r))
    }

    /// Head for corner `self.goal`.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn update_goal(&mut self, w: &mut impl OspreyWorld) {
        let o = self;
        let c = w.corner(o.goal as usize);
        let (pos, speed, ang) = (c.pos, c.speed, c.ang);
        o.p[0] = o.p[1];
        o.a[0] = o.a[1];
        o.v[0] = o.v[1];
        o.spd[0] = o.spd[1];
        let y = ang[1] as u16;
        o.v[1] = [
            ((sincos::sin_q12(y) * speed as i32) >> 12) as i16,
            0,
            ((sincos::sin_q12((y + 1024) & 0xfff) * speed as i32) >> 12) as i16,
        ];
        o.p[1] = pos;
        o.spd[1] = speed;
        o.start = o.start.wrapping_add(o.dt);
        let d = [
            pos[0] as i32 - o.p[0][0] as i32,
            pos[1] as i32 - o.p[0][1] as i32,
            pos[2] as i32 - o.p[0][2] as i32,
        ];
        o.dt = (isqrt_i32(dist2_3(d, [0; 3])) * 40 / (o.spd[0] as i32 + speed as i32).max(1))
            .clamp(1, 4000) as u16;
        let dy = o.a[0][1] as i32 - ang[1] as i32;
        if dy < -2048 {
            o.a[0][1] += 4096;
        } else if dy > 2048 {
            o.a[0][1] -= 4096;
        }
        o.a[1] = ang;
    }

    /// Has a grunt of the snapshot gone down?
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn has_dead(&self, w: &mut impl OspreyWorld) -> bool {
        let mut pi = 0;
        while pi < PROP_SLOTS {
            if self.grunts[pi >> 5] & (1 << (pi & 31)) != 0 && w.is_down(pi) {
                return true;
            }
            pi += 1;
        }
        false
    }

    /// Drop up to four replacements into fallen snapshot slots.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn deploy(&mut self, op: [i32; 3], w: &mut impl OspreyWorld) {
        let o = self;
        let yaw = (o.a[1][1] as u16) & 0xfff;
        let (s, c) = (sincos::sin_q12(yaw), sincos::sin_q12((yaw + 1024) & 0xfff));
        let mut r = 0;
        let mut pi = 0;
        while r < 4 {
            let f = if r & 1 == 0 { 32 } else { -64 };
            let side = if r < 2 { 100 } else { -100 };
            let src = [
                op[0] + ((s * f + c * side) >> 12),
                op[1] - 96,
                op[2] + ((c * f - s * side) >> 12),
            ];
            o.repel[r] = NO_REPEL;
            while pi < PROP_SLOTS {
                if o.grunts[pi >> 5] & (1 << (pi & 31)) != 0 && w.is_down(pi) {
                    let floor = w.respawn_grunt(pi, src, yaw);
                    o.repel[r] = pi as u8;
                    o.repel_y[r] = floor as i16;
                    pi += 1;
                    break;
                }
                pi += 1;
            }
            r += 1;
        }
    }

    /// One 20 Hz tick (a think every 0.1 s once awake).
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn tick(&mut self, i: &OspreyInputs, w: &mut impl OspreyWorld) {
        let o = self;
        let now = i.now;
        let think = o.next != 0xffff && time_reached(now, o.next);
        if think {
            o.next = now.wrapping_add(2);
        }
        if i.dead && o.phase != DYING {
            o.phase = DYING;
            o.start = now.wrapping_add(80);
        }
        match o.phase {
            WAIT => {
                if think {
                    let (tilt, yaw) = w.authored_attitude();
                    let q = |v: u16| ((v as u8 as i8) as i16) << 4;
                    o.a[1] = [q(tilt), yaw, q(tilt >> 8)];
                    w.set_tilt(tilt);
                    let mut n = 0;
                    o.grunts = [0; 4];
                    let slots = w.grunt_scan_slots();
                    let mut qi = 0;
                    while qi < slots {
                        if w.is_live_grunt(qi) && n < MAX_CARRY {
                            o.grunts[qi >> 5] |= 1 << (qi & 31);
                            n += 1;
                        }
                        qi += 1;
                    }
                    if n == 0 {
                        w.remove();
                        return;
                    }
                    o.phase = FLY;
                    o.start = now;
                    o.dt = 0;
                    o.goal = 0;
                    o.update_goal(w);
                }
                return;
            }
            DEPLOY => {
                if think {
                    o.deploy(i.pos, w);
                    o.phase = HOVER;
                }
            }
            HOVER => {
                if think
                    && (0..4).all(|r| {
                        let g = o.repel[r] as usize;
                        g >= PROP_SLOTS || w.is_dead(g) || !w.is_on_rope(g)
                    })
                {
                    o.start = now;
                    o.phase = FLY;
                }
            }
            DYING => {
                let p = i.pos;
                o.vel[1] -= 12;
                let mut np = p;
                for k in 0..3 {
                    np[k] += o.vel[k] as i32 / 20;
                }
                let hit = w.trace_line(p, np);
                if think && now & 3 == 0 {
                    let x = np[0] + (w.random_below(301) as i32 - 150);
                    let z = np[2] + (w.random_below(301) as i32 - 150);
                    w.explosion_fx([x, np[1] - 100, z], 60);
                }
                if hit.is_some() || time_reached(now, o.start) {
                    w.explode(np, 255, 750, false);
                    w.remove();
                    w.stop_rotor();
                    return;
                }
                w.set_pos(np);
                w.rotor(np);
                return;
            }
            _ => {
                if think && time_reached(now, o.start.wrapping_add(o.dt)) {
                    let (n, loop_start) = (o.corners as usize, o.loop_start);
                    if w.corner(o.goal as usize).speed == 0 {
                        o.phase = DEPLOY;
                    }
                    let dead = o.has_dead(w);
                    let mut guard = n;
                    loop {
                        let g = o.goal as usize + 1;
                        o.goal = if g >= n { loop_start } else { g as u8 };
                        guard -= 1;
                        if guard == 0 || w.corner(o.goal as usize).speed >= 400 || dead {
                            break;
                        }
                    }
                    o.update_goal(w);
                }
            }
        }
        // The grunts on ropes slide down to the floor under them.
        let mut r = 0;
        while r < 4 {
            let g = o.repel[r] as usize;
            if g < PROP_SLOTS && w.is_on_rope(g) {
                let p = w.grunt_pos(g);
                let y = (p[1] - 8).max(o.repel_y[r] as i32);
                w.set_grunt_pos(g, [p[0], y, p[2]]);
                w.rope([p[0], i.pos[1] + 16, p[2]], [p[0], y + 72, p[2]]);
                if y == o.repel_y[r] as i32 {
                    w.land_grunt(g);
                }
            }
            r += 1;
        }
        w.rotor(i.pos);
        if o.phase != FLY {
            return;
        }
        // Each leg blends both ends' straight-line extrapolations with a
        // smoothstep of the leg's elapsed fraction.
        let dt = o.dt as i32;
        let t = (now.wrapping_sub(o.start) as i16 as i32).clamp(0, dt);
        let x = t * 4096 / dt;
        let x2 = (x * x) >> 12;
        let f = 3 * x2 - 2 * ((x2 * x) >> 12);
        let mut pos = [0i32; 3];
        let mut ang = [0i32; 3];
        let mut k = 0;
        while k < 3 {
            ang[k] = (o.a[0][k] as i32 * (4096 - f) + o.a[1][k] as i32 * f) >> 12;
            let a = o.p[0][k] as i32 + o.v[0][k] as i32 * t / 20;
            let b = o.p[1][k] as i32 - o.v[1][k] as i32 * (dt - t) / 20;
            pos[k] = (a * (4096 - f) + b * f) >> 12;
            o.vel[k] = ((o.v[0][k] as i32 * (4096 - f) + o.v[1][k] as i32 * f) >> 12) as i16;
            k += 1;
        }
        w.set_pos(pos);
        w.set_yaw(ang[1] as u16 & 0xfff);
        w.set_tilt(((ang[0] >> 4) as u16 & 0xff) | (((ang[2] >> 4) as u16 & 0xff) << 8));
    }
}
