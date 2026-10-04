//! Apache brain: the attack helicopter's hunt, thrust-and-tilt flight
//! model, chin gun, rocket pairs, damage rule and crash. Free of PS1 state;
//! the game feeds it through [`ApacheWorld`].
//!
//! Internally the state is kept in GoldSrc axes (x, y, z up): positions and
//! velocities x16, angles and angular velocities in 1/16 degree (pitch,
//! yaw, roll). It thinks every 0.1 s and integrates every 20 Hz tick. The
//! world speaks runtime axes (y up) in whole units.

use crate::setpiece_math::{atan_s, dist2_3, time_reached, TraceHit};
use psx_math::{int32::isqrt_i32, sincos};

const SF_WAITFORTRIGGER: u16 = 0x04 | 0x40;
const SF_NOWRECKAGE: u16 = 0x08;
/// Fixed-point scale of degrees and units.
const D: i32 = 16;

/// One path_corner as cooked: runtime-axes position, runtime yaw (q12, 0 =
/// +z) and reflected q8 pitch.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ApacheCorner {
    pub pos: [i32; 3],
    pub yaw_q12: u16,
    pub pitch_q8: i8,
}

/// What the brain asks of, and tells, the game. Positions in runtime axes.
pub trait ApacheWorld {
    fn corner(&mut self, k: usize) -> ApacheCorner;
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit>;
    /// Where `from -> to` enters the player's box (q12 of the segment);
    /// None when it misses or the player is dead.
    fn player_box_frac(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<i32>;
    /// A uniform random integer in `0..n` from the shared impact generator.
    fn random_below(&mut self, n: u32) -> u32;
    fn set_pos(&mut self, pos: [i32; 3]);
    /// The prop's position as the game now holds it.
    fn prop_pos(&mut self) -> [i32; 3];
    /// Keep the rotor loop playing at the prop.
    fn rotor(&mut self);
    fn stop_rotor(&mut self);
    /// Facing in runtime q12 yaw.
    fn set_yaw(&mut self, yaw: u16);
    /// The drawn tilt word (reflected q8 pitch | roll << 8).
    fn set_tilt(&mut self, tilt: u16);
    fn explosion_fx(&mut self, at: [i32; 3], mag: u8);
    fn explode(&mut self, at: [i32; 3], damage: u8, radius: i32, by_player: bool);
    fn gibs(&mut self, at: [i32; 3], count: u8);
    /// The wreck leaves the map.
    fn remove(&mut self);
    fn rocket(&mut self, from: [i32; 3], dir: [i32; 3]);
    fn rocket_sound(&mut self, at: [i32; 3]);
    /// The chin gun's skill damage per round.
    fn gun_damage(&mut self) -> u8;
    fn hurt_player(&mut self, damage: u8, from: [i32; 3]);
    fn tracer(&mut self, from: [i32; 3], to: [i32; 3]);
    fn gun_sound(&mut self, at: [i32; 3]);
}

/// One tick's observations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ApacheInputs {
    pub now: u16,
    /// The apache's prop is at zero health.
    pub dead: bool,
    pub player_pos: [i32; 3],
    pub player_alive: bool,
    /// Eye height above the player's origin.
    pub view_height: i32,
    /// Easy skill: firing the gun holds back rockets.
    pub easy: bool,
}

/// The flight model's state, for driving one step on its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Kinematics {
    /// GoldSrc axes, units x16.
    pub pos: [i32; 3],
    /// GoldSrc axes, units/s x16.
    pub vel: [i32; 3],
    /// (pitch, yaw, roll) in 1/16 degree.
    pub ang: [i32; 3],
    /// 1/16 degree per second.
    pub avel: [i32; 3],
    /// Vertical thrust, units/s x16 per think.
    pub force: i32,
    /// Speed it pitches forward to reach, units/s.
    pub goal_speed: i32,
    /// Where it wants to be, GoldSrc axes, whole units.
    pub desired: [i32; 3],
}

/// The map's apache.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ApacheBrain {
    /// 0 waiting for its trigger, 1 hunting, 2 dying.
    phase: u8,
    corner: u8,
    corners: u8,
    loop_start: u8,
    rockets: u8,
    side: i8,
    flags: u16,
    last_seen: u16,
    prev_seen: u16,
    /// Rocket clock; while dying, the crash deadline.
    next_rocket: u16,
    gun_sound_next: u16,
    pos: [i32; 3],
    vel: [i32; 3],
    ang: [i32; 3],
    avel: [i32; 3],
    force: i32,
    goal_speed: i32,
    gun: [i32; 2],
    desired: [i32; 3],
    goal_dir: [i32; 3],
    target: [i32; 3],
}

/// Sine and cosine (q12) of an angle in 1/16 degree.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
fn sc(a: i32) -> (i32, i32) {
    let t = ((a * 32 / 45) & 0xfff) as u16;
    (
        sincos::sin_q12(t),
        sincos::sin_q12(t.wrapping_add(1024) & 0xfff),
    )
}

/// q12 [forward, right, up] of (pitch, yaw, roll), pitch negated as an aim.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
fn aim_vectors(a: [i32; 3]) -> [[i32; 3]; 3] {
    let (sp, cp) = sc(-a[0]);
    let (sy, cy) = sc(a[1]);
    let (sr, cr) = sc(a[2]);
    let m = |x: i32, y: i32| (x * y) >> 12;
    [
        [m(cp, cy), m(cp, sy), -sp],
        [
            m(-m(sr, sp), cy) + m(cr, sy),
            m(-m(sr, sp), sy) - m(cr, cy),
            -m(sr, cp),
        ],
        [
            m(m(cr, sp), cy) + m(sr, sy),
            m(m(cr, sp), sy) - m(sr, cy),
            m(cr, cp),
        ],
    ]
}

/// `a . b >> 12`.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
fn dot(a: [i32; 3], b: [i32; 3]) -> i32 {
    (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) >> 12
}

/// `a + b * s >> 12`.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
fn mad(a: [i32; 3], b: [i32; 3], s: i32) -> [i32; 3] {
    let mut r = a;
    for k in 0..3 {
        r[k] += (b[k] * s) >> 12;
    }
    r
}

#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
fn sub(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Unit (q12) direction of a vector.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
fn norm(v: [i32; 3]) -> [i32; 3] {
    let l = isqrt_i32(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).max(1);
    [v[0] * 4096 / l, v[1] * 4096 / l, v[2] * 4096 / l]
}

/// A x16 vector in whole units.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
fn whole(v: [i32; 3]) -> [i32; 3] {
    [v[0] / D, v[1] / D, v[2] / D]
}

/// Runtime axes <-> GoldSrc axes.
#[inline(always)]
fn hl(p: [i32; 3]) -> [i32; 3] {
    [p[0], p[2], p[1]]
}

/// A hit of `dmg` (blast when `blast`) scaled onto the u8 actor health
/// standing for `full_health`: blast counts double, and 50 or less after
/// that does nothing.
pub fn scale_damage(dmg: u8, blast: bool, full_health: u32) -> u8 {
    let d = dmg as u32 * if blast { 2 } else { 1 };
    if d <= 50 {
        return 0;
    }
    let full = full_health.max(255);
    (d * 255 / full).min(255) as u8
}

impl Default for ApacheBrain {
    fn default() -> Self {
        Self::new()
    }
}

impl ApacheBrain {
    /// The power-on state, before any map binds an apache.
    pub const fn new() -> Self {
        Self {
            phase: 0,
            corner: 0,
            corners: 0,
            loop_start: 0,
            rockets: 10,
            side: 1,
            flags: 0,
            last_seen: 0,
            prev_seen: 0,
            next_rocket: 0,
            gun_sound_next: 0,
            pos: [0; 3],
            vel: [0; 3],
            ang: [0; 3],
            avel: [0; 3],
            force: 0,
            goal_speed: 0,
            gun: [0; 2],
            desired: [0; 3],
            goal_dir: [0; 3],
            target: [0; 3],
        }
    }

    /// A new map: forget the gun-sound throttle.
    pub fn reset(&mut self) {
        self.gun_sound_next = 0;
    }

    /// Bind a map's apache: `corner_words` cooked aux words of chain looping
    /// back to `loop_start`, spawnflags `flags`, standing at runtime `pos`
    /// facing runtime yaw `yaw_q12`, at tick `now`. Its last target carries
    /// over from the previous apache.
    pub fn bind(
        &mut self,
        corner_words: usize,
        loop_start: u8,
        flags: u16,
        pos: [i32; 3],
        yaw_q12: u16,
        now: u16,
    ) {
        let a = self;
        a.corners = (corner_words / 3) as u8;
        a.loop_start = loop_start;
        a.flags = flags;
        a.phase = if flags & SF_WAITFORTRIGGER != 0 { 0 } else { 1 };
        a.rockets = 10;
        a.side = 1;
        let p = hl(pos);
        a.pos = [p[0] * D, p[1] * D, p[2] * D];
        a.vel = [0; 3];
        a.avel = [0; 3];
        a.ang = [0, (1024 - yaw_q12 as i32) * 360 * D / 4096, 0];
        a.force = 0;
        a.goal_speed = 0;
        a.gun = [0; 2];
        a.next_rocket = now;
        a.last_seen = now.wrapping_sub(2000);
        a.prev_seen = a.last_seen;
        a.desired = p;
        a.goal_dir = aim_vectors(a.ang)[0];
        a.corner = 0xff; // the first corner loads on the first tick
    }

    /// Its trigger: a waiting apache starts hunting.
    pub fn startup(&mut self) {
        if self.phase == 0 {
            self.phase = 1;
        }
    }

    #[allow(dead_code)] // read by the host tests
    pub fn kinematics(&self) -> Kinematics {
        Kinematics {
            pos: self.pos,
            vel: self.vel,
            ang: self.ang,
            avel: self.avel,
            force: self.force,
            goal_speed: self.goal_speed,
            desired: self.desired,
        }
    }

    #[allow(dead_code)] // set by the host tests
    pub fn set_kinematics(&mut self, k: Kinematics) {
        self.pos = k.pos;
        self.vel = k.vel;
        self.ang = k.ang;
        self.avel = k.avel;
        self.force = k.force;
        self.goal_speed = k.goal_speed;
        self.desired = k.desired;
    }

    /// Rockets left before the reload pause.
    #[allow(dead_code)] // read by the host tests
    pub fn rockets(&self) -> u8 {
        self.rockets
    }

    /// Path corner `k`: GoldSrc position and the forward of its angles.
    fn corner_goal(w: &mut impl ApacheWorld, k: usize) -> ([i32; 3], [i32; 3]) {
        let c = w.corner(k);
        let yaw_deg = (1024 - (c.yaw_q12 & 0xfff) as i32) * 360 * D / 4096;
        let pitch_deg = -(c.pitch_q8 as i32) * 360 * D / 256;
        (hl(c.pos), aim_vectors([pitch_deg, yaw_deg, 0])[0])
    }

    /// One 20 Hz tick: integrate, then think on even ticks.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn tick(&mut self, i: &ApacheInputs, w: &mut impl ApacheWorld) {
        let a = self;
        let now = i.now;
        if a.corner == 0xff {
            a.corner = 0;
            if a.corners != 0 {
                (a.desired, a.goal_dir) = Self::corner_goal(w, 0);
            }
        }
        if a.phase == 0 {
            return;
        }
        if i.dead && a.phase == 1 {
            a.phase = 2;
            a.next_rocket = now.wrapping_add(if a.flags & SF_NOWRECKAGE != 0 {
                80
            } else {
                300
            });
        }
        let think = now & 1 == 0;
        if a.phase == 2 {
            a.vel[2] -= 800 * 3 / 10 * D / 20;
            if think {
                for k in 0..3 {
                    a.avel[k] = a.avel[k] * 102 / 100;
                }
            }
        }
        let from = hl(whole(a.pos));
        for k in 0..3 {
            a.pos[k] += a.vel[k] / 20;
            a.ang[k] += a.avel[k] / 20;
        }
        let to = hl(whole(a.pos));
        if let Some(h) = w.trace_line(from, to) {
            if a.phase == 2 {
                a.next_rocket = now;
            } else {
                let n = hl(h.normal);
                let v = whole(a.vel);
                let s = isqrt_i32(dot(v, v) << 12) + 200;
                a.vel = mad(a.vel, n, s * D);
            }
            let p = hl(h.pos);
            a.pos = p.map(|x| x * D);
        }
        w.set_pos(hl(whole(a.pos)));
        w.rotor();
        w.set_yaw(((1024 - a.ang[1] * 4096 / (360 * D)) & 0xfff) as u16);
        let q8 = |d: i32| ((-d * 256 / (360 * D)) & 0xff) as u16;
        w.set_tilt(q8(a.ang[0]) | (q8(a.ang[2]) << 8));
        if !think {
            return;
        }
        if a.phase == 2 {
            a.dying(now, w);
        } else {
            a.hunt(i, w);
        }
    }

    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn dying(&mut self, now: u16, w: &mut impl ApacheWorld) {
        let o = w.prop_pos();
        if !time_reached(now, self.next_rocket) {
            if now & 3 == 0 {
                let x = o[0] + w.random_below(301) as i32 - 150;
                let y = o[1] - 50 - w.random_below(101) as i32;
                let z = o[2] + w.random_below(301) as i32 - 150;
                w.explosion_fx([x, y, z], 50);
            }
            return;
        }
        w.explode(o, 255, 750, false);
        w.gibs(o, 12);
        w.stop_rotor();
        w.remove();
    }

    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn hunt(&mut self, i: &ApacheInputs, w: &mut impl ApacheWorld) {
        let a = self;
        let now = i.now;
        let origin = whole(a.pos);
        let here = hl(origin);
        let p = i.player_pos;
        let enemy = hl(p);
        let seen = i.player_alive
            && dist2_3(here, p) < 4092 * 4092
            && w.trace_line(here, [p[0], p[1] + i.view_height, p[2]])
                .is_none();
        if a.goal_speed < 800 {
            a.goal_speed += 5;
        }
        if seen {
            if time_reached(now, a.last_seen.wrapping_add(100)) {
                a.prev_seen = now;
            }
            a.last_seen = now;
            a.target = enemy;
        }
        let to_target = norm(sub(a.target, origin));
        if a.corners == 0 {
            a.desired = origin;
        }
        let mut length = isqrt_i32(dist2_3(origin, a.desired));
        if a.corners != 0 && length < 128 {
            let next = a.corner + 1;
            a.corner = if next >= a.corners {
                a.loop_start
            } else {
                next
            };
            (a.desired, a.goal_dir) = Self::corner_goal(w, a.corner as usize);
            length = isqrt_i32(dist2_3(origin, a.desired));
        }
        let to_desired = norm(sub(a.desired, origin));
        let want = if length > 250 {
            if !time_reached(now, a.last_seen.wrapping_add(1800))
                && dot(to_target, to_desired) > 1024
            {
                to_target
            } else {
                to_desired
            }
        } else {
            a.goal_dir
        };
        a.flight(want);
        if !time_reached(now, a.last_seen.wrapping_add(20))
            && time_reached(now, a.prev_seen.wrapping_add(40))
        {
            if a.fire_gun(now, w) && a.goal_speed > 400 {
                a.goal_speed = 400;
            }
            if i.easy {
                a.next_rocket = now.wrapping_add(200);
            }
        }
        let v = aim_vectors(a.ang);
        let vel = whole(a.vel);
        let est = norm(mad(vel, v[0], 800));
        let fire = if a.rockets % 2 == 1 {
            true
        } else if a.ang[0] < 0
            && dot(vel, v[0]) > -100
            && time_reached(now, a.next_rocket)
            && !time_reached(now, a.last_seen.wrapping_add(1200))
            && dot(to_target, est) > 3953
        {
            let far = mad(origin, est, 4096);
            let end = w.trace_line(here, hl(far)).map_or(hl(far), |h| h.pos);
            dist2_3(end, hl(a.target)) < 512 * 512
        } else {
            false
        };
        if fire && a.rockets > 0 {
            let s = a.side as i32;
            let src = mad(mad(mad(origin, v[0], 32), v[1], 105 * s), v[2], -119);
            w.rocket(hl(src), hl(v[0]));
            w.rocket_sound(hl(src));
            a.rockets -= 1;
            a.side = -a.side;
            a.next_rocket = now.wrapping_add(10);
            if a.rockets == 0 {
                a.next_rocket = now.wrapping_add(200);
                a.rockets = 10;
            }
        }
    }

    /// One 0.1 s step of the flight model toward heading `want` (q12, GoldSrc
    /// axes), holding `desired`.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn flight(&mut self, want: [i32; 3]) {
        let a = self;
        fn add(a: &ApacheBrain, s: i32) -> [i32; 3] {
            [
                a.ang[0] + a.avel[0] * s + 5 * D,
                a.ang[1] + a.avel[1] * s,
                a.ang[2] + a.avel[2] * s,
            ]
        }
        let v = aim_vectors(add(a, 2));
        if dot(want, v[1]) < 0 {
            if a.avel[1] < 60 * D {
                a.avel[1] += 8 * D;
            }
        } else if a.avel[1] > -60 * D {
            a.avel[1] -= 8 * D;
        }
        a.avel[1] = a.avel[1] * 98 / 100;
        let v = aim_vectors(add(a, 1));
        let mut est = mad(whole(a.pos), whole(a.vel), 8192);
        est = mad(est, v[2], a.force * 20 / D);
        est[2] -= 768;
        let v = aim_vectors(add(a, 0));
        a.vel = mad(a.vel, v[2], a.force);
        a.vel[2] -= 384 * D / 10;
        let vel = whole(a.vel);
        let mut speed = isqrt_i32(dot(vel, vel) << 12);
        if v[0][0] * vel[0] + v[0][1] * vel[1] < 0 {
            speed = -speed;
        }
        let off = sub(a.desired, est);
        let dist = dot(off, v[0]);
        let slip = -dot(off, v[1]);
        let (roll, rv) = (a.ang[2], &mut a.avel[2]);
        if slip > 0 {
            if roll > -30 * D && *rv > -15 * D {
                *rv -= 4 * D;
            } else {
                *rv += 2 * D;
            }
        } else if roll < 30 * D && *rv < 15 * D {
            *rv += 4 * D;
        } else {
            *rv -= 2 * D;
        }
        for k in 0..3 {
            a.vel[k] = a.vel[k] * (4096 - (v[1][k].abs() * 5 / 100)) / 4096 * 995 / 1000;
        }
        let high = est[2] > a.desired[2];
        if a.force < 80 * D && !high && est[2] != a.desired[2] {
            a.force += 12 * D;
        } else if a.force > 30 * D && high {
            a.force -= 8 * D;
        }
        let pitch = a.ang[0] + a.avel[0];
        if dist > 0 && speed < a.goal_speed && pitch > -40 * D {
            a.avel[0] -= 12 * D;
        } else if dist < 0 && speed > -50 && pitch < 20 * D {
            a.avel[0] += 12 * D;
        } else if pitch > 0 {
            a.avel[0] -= 4 * D;
        } else if pitch < 0 {
            a.avel[0] += 4 * D;
        }
    }

    /// The chin turret slews toward the target and fires a round when on it.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn fire_gun(&mut self, now: u16, w: &mut impl ApacheWorld) -> bool {
        let a = self;
        let v = aim_vectors(a.ang);
        // Gun pivot: 97 ahead and 145 below (an apache.mdl attachment).
        let gun = mad(mad(whole(a.pos), v[0], 97), v[2], -145);
        let t = norm(sub(a.target, gun));
        let local = [dot(v[0], t), -dot(v[1], t), dot(v[2], t)];
        let ang = |y: i32, x: i32| atan_s(y, x) * 360 * D / 4096;
        let want_yaw = ang(local[1], local[0]);
        let want_pitch = -ang(
            local[2],
            isqrt_i32(local[0] * local[0] + local[1] * local[1]),
        );
        let step = |cur: i32, want: i32| cur + (want - cur).clamp(-12 * D, 12 * D);
        // Bone controller limits of apache.mdl: yaw -90..90, pitch -10..45.
        a.gun[0] = step(a.gun[0], want_yaw).clamp(-90 * D, 90 * D);
        a.gun[1] = step(a.gun[1], want_pitch).clamp(-10 * D, 45 * D);
        let off = (want_yaw - a.gun[0]).abs() + (want_pitch - a.gun[1]).abs();
        if off > 11 * D {
            return false;
        }
        let r = w.random_below(287) as i32 - 143;
        let u = w.random_below(287) as i32 - 143;
        let dir = mad(mad(t, v[1], r), v[2], u);
        let far = hl(mad(gun, dir, 8192));
        let from = hl(gun);
        let end = w.trace_line(from, far).map_or(far, |h| h.pos);
        if w.player_box_frac(from, end).is_some() {
            let d = w.gun_damage();
            w.hurt_player(d, from);
        }
        w.tracer(from, end);
        if time_reached(now, a.gun_sound_next) {
            a.gun_sound_next = now.wrapping_add(20);
            w.gun_sound(from);
        }
        true
    }
}
