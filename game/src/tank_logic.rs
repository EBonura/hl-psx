//! func_tank brain: one mounted gun, rocket or mortar battery. It tracks
//! the player, turns within its arcs and fires volleys; a player can take
//! it over through a control volume. Free of PS1 state; the game feeds it
//! through [`TankWorld`].
//!
//! Angles are q12 turns, [yaw, pitch] with positive pitch meaning barrel
//! down; turn rates are q12 per 20 Hz tick. Positions are world units in
//! runtime axes (y up).

use crate::setpiece_math::{atan_s, dist2_3, dist2_xz, time_reached, TraceHit};
use psx_math::{int32::isqrt_i32, sincos};

const SF_ACTIVE: u16 = 1;
const SF_LINEOFSIGHT: u16 = 0x10;
const SF_CANCONTROL: u16 = 0x20;
const ON: u8 = 1;
const LOS: u8 = 2;
const CONTROL: u8 = 4;
const CLASS_SHIFT: u8 = 5;
const CLASS_ROCKET: u8 = 2;
const CLASS_MORTAR: u8 = 3;

/// A tank's map keys, already unpacked from the cooked record.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TankKeys {
    /// 1 active at load, 0x10 fire only along the barrel line, 0x20 the
    /// player can take it over.
    pub spawnflags: u16,
    /// Bullet type (bits 0-1), spread index (2-4), class (5-6: 0 gun,
    /// 1 laser, 2 rockets, 3 mortar).
    pub kind: u8,
    /// How long after last sighting it keeps firing, in ticks.
    pub persistence: u8,
    /// Turn rate [yaw, pitch], q12 per tick.
    pub turn_rate: [u8; 2],
    /// Aim tolerance [yaw, pitch], q12.
    pub tolerance: [u8; 2],
    /// Half arcs about the spawn angles [yaw, pitch], q12.
    pub range: [i16; 2],
    /// Rounds per second, q8.
    pub fire_rate: u16,
    /// Barrel offset from the pivot [forward, right, up].
    pub barrel: [i16; 3],
    /// Bullet damage (0 = the bullet type's skill damage) or mortar
    /// magnitude.
    pub damage: u16,
    pub min_range: i16,
    /// 0 = unlimited.
    pub max_range: i16,
    /// Spawn angles [yaw, pitch], q12.
    pub centre: [i16; 2],
    /// Control volumes (mins, maxs) that hand the tank to the player.
    pub controls: [([i32; 3], [i32; 3]); 2],
    pub n_controls: u8,
}

/// What a use asks of the tank.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TankUse {
    On,
    Off,
    Toggle,
}

/// One think's observations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TankInputs {
    pub now: u16,
    /// The player is mounted on this tank.
    pub controlled: bool,
    /// The tank was killtargeted.
    pub removed: bool,
    /// The tank brush's pivot.
    pub origin: [i32; 3],
    /// The player's aim direction (q12), which a mounted tank mirrors.
    pub view: [i32; 3],
    pub player_pos: [i32; 3],
    pub player_alive: bool,
    /// Eye height above the player's origin.
    pub view_height: i32,
}

/// What the brain asks of, and tells, the game.
pub trait TankWorld {
    /// Is the tank inside the player's potentially visible set?
    fn in_pvs(&mut self) -> bool;
    /// A line trace that ignores the tank's own brush.
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit>;
    /// Where `from -> to` enters the player's box (q12 of the segment);
    /// None when it misses or the player is dead.
    fn player_box_frac(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<i32>;
    /// A uniform random integer in `0..n` from the shared impact generator.
    fn random_below(&mut self, n: u32) -> u32;
    /// Skill damage of bullet type 0..3 (0 and 1 the 9mm, 2 the MP5, 3 the
    /// 12mm).
    fn skill_round_damage(&mut self, bullet: u8) -> u8;
    /// The brush turned to `angles` [yaw, pitch].
    fn set_pose(&mut self, angles: [i16; 2]);
    fn launch_rocket(&mut self, from: [i32; 3], dir: [i32; 3], by_player: bool);
    fn tracer(&mut self, from: [i32; 3], to: [i32; 3]);
    fn explode(&mut self, at: [i32; 3], magnitude: u8, radius: i32, by_player: bool);
    /// A round from the mounted gun, resolved by the player's hitscan.
    fn player_round(&mut self, from: [i32; 3], end: [i32; 3], damage: u8);
    fn hurt_player(&mut self, damage: u8, from: [i32; 3]);
    /// The volley fires the tank's target.
    fn fire_target(&mut self);
    /// The mounted gun's firing sound (guns only).
    fn gun_sound(&mut self);
}

/// The answer to a player using a control volume.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlAttempt {
    /// The player is in none of this tank's volumes.
    NotHere,
    /// The player took the tank.
    Taken,
    /// The tank's master refused.
    Refused,
}

/// One tank's persistent state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TankBrain {
    flags: u8,
    kind: u8,
    persist: u8,
    ang: [i16; 2],
    avel: [i16; 2],
    sight: [i16; 3],
    fire_last: u16,
    sight_time: u16,
    next: u16,
    rate: [u8; 2],
    tol: [u8; 2],
    range: [i16; 2],
    fire_rate: u16,
    barrel: [i16; 3],
    damage: u16,
    min_range: i16,
    max_range: i16,
    centre: [i16; 2],
    controls: [([i32; 3], [i32; 3]); 2],
    n_controls: u8,
}

/// [forward, right, up] rows of the tank's angles.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
fn vectors(ang: [i16; 2]) -> [[i32; 3]; 3] {
    let y = ang[0] as u16;
    let p = (-(ang[1] as i32)) as u16; // pitch up
    let (sy, cy) = (
        sincos::sin_q12(y & 0xfff),
        sincos::sin_q12(y.wrapping_add(1024) & 0xfff),
    );
    let (sp, cp) = (
        sincos::sin_q12(p & 0xfff),
        sincos::sin_q12(p.wrapping_add(1024) & 0xfff),
    );
    [
        [(cp * cy) >> 12, sp, (cp * sy) >> 12],
        [sy, 0, -cy],
        [(-sp * cy) >> 12, cp, (-sp * sy) >> 12],
    ]
}

#[inline(always)]
fn angle_dist(a: i32, b: i32) -> i32 {
    ((a - b + 2048) & 0xfff) - 2048
}

impl TankBrain {
    /// A tank as the map loads at tick `now`, its pivot at `origin`.
    pub fn spawn(k: &TankKeys, origin: [i32; 3], now: u16) -> Self {
        let mut t = Self {
            flags: (k.spawnflags & SF_ACTIVE != 0) as u8 * ON
                | (k.spawnflags & SF_LINEOFSIGHT != 0) as u8 * LOS
                | (k.spawnflags & SF_CANCONTROL != 0) as u8 * CONTROL,
            kind: k.kind,
            persist: k.persistence,
            ang: k.centre,
            avel: [0; 2],
            sight: [0; 3],
            fire_last: 0,
            // The map clock is 1.0 s here and the last sighting 0, so the
            // tank may fire for its first `persistence` seconds even blind
            // (c2a2d's silo guard gun fires four rounds on load). The first
            // think lands 0.75 s in: the server's two settle frames already
            // advanced the clock.
            sight_time: now.wrapping_sub(20),
            next: now.wrapping_add(15),
            rate: k.turn_rate,
            tol: k.tolerance,
            range: k.range,
            fire_rate: k.fire_rate.max(1),
            barrel: k.barrel,
            damage: k.damage,
            min_range: k.min_range,
            max_range: k.max_range,
            centre: k.centre,
            controls: k.controls,
            n_controls: k.n_controls.min(2),
        };
        let b = t.barrel_pos(origin);
        t.sight = [b[0] as i16, b[1] as i16, b[2] as i16];
        t
    }

    /// Current angles [yaw, pitch], q12.
    #[allow(dead_code)] // read by the host tests
    pub fn angles(&self) -> [i16; 2] {
        self.ang
    }

    /// Angular velocity [yaw, pitch] the next tick integrates, q12 per tick.
    #[allow(dead_code)] // read by the host tests
    pub fn angular_velocity(&self) -> [i16; 2] {
        self.avel
    }

    /// Is the tank switched on?
    #[allow(dead_code)] // read by the host tests
    pub fn active(&self) -> bool {
        self.flags & ON != 0
    }

    /// The muzzle position for the pivot `origin`.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn barrel_pos(&self, origin: [i32; 3]) -> [i32; 3] {
        let v = vectors(self.ang);
        let mut p = origin;
        for r in 0..3 {
            for c in 0..3 {
                p[c] += (v[r][c] * self.barrel[r] as i32) >> 12;
            }
        }
        p
    }

    /// A use at tick `now`: switch on or off unless already so.
    pub fn use_tank(&mut self, how: TankUse, now: u16) {
        let on = self.flags & ON != 0;
        if (how == TankUse::On && on) || (how == TankUse::Off && !on) {
            return;
        }
        self.flags ^= ON;
        self.fire_last = 0;
        self.next = now.wrapping_add(2);
    }

    fn round_damage(&self, w: &mut impl TankWorld) -> u8 {
        if self.damage != 0 {
            return self.damage.min(255) as u8;
        }
        w.skill_round_damage(self.kind & 3)
    }

    /// Does `from -> end` reach the player's box before the world?
    fn ray_hits_player(w: &mut impl TankWorld, from: [i32; 3], end: [i32; 3]) -> bool {
        let wall = w.trace_line(from, end).map_or(4096, |hit| hit.frac);
        w.player_box_frac(from, end).is_some_and(|f| f <= wall)
    }

    /// `count` rounds of the tank's class along its barrel, then its target
    /// fires.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn fire(&self, origin: [i32; 3], count: i32, player: bool, w: &mut impl TankWorld) {
        let spread = [0, 102, 205, 410, 1024][((self.kind >> 2) & 7).min(4) as usize];
        let v = vectors(self.ang);
        let barrel = self.barrel_pos(origin);
        let class = self.kind >> CLASS_SHIFT;
        for _ in 0..count {
            if class == CLASS_ROCKET {
                w.launch_rocket(barrel, v[0], player);
                continue;
            }
            let mut end = barrel;
            for x in 0..2 {
                let r = (w.random_below(4097) as i32 + w.random_below(4097) as i32 - 4096) * spread
                    >> 12;
                for c in 0..3 {
                    end[c] += if x == 0 { v[0][c] } else { 0 } + ((r * v[x + 1][c]) >> 12);
                }
            }
            let impact = w.trace_line(barrel, end).map_or(end, |h| h.pos);
            w.tracer(barrel, impact);
            if class == CLASS_MORTAR {
                let mag = self.damage.min(255) as i32;
                w.explode(impact, mag as u8, mag * 5 / 2, player);
                break;
            }
            if player {
                let d = self.round_damage(w);
                w.player_round(barrel, end, d);
            } else if Self::ray_hits_player(w, barrel, end) {
                let d = self.round_damage(w);
                w.hurt_player(d, barrel);
            }
        }
        w.fire_target();
    }

    /// One 20 Hz tick: integrate the last think's turn, then think when due
    /// (every 0.1 s while active, every tick while mounted).
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn tick(&mut self, i: &TankInputs, w: &mut impl TankWorld) {
        let t = self;
        let now = i.now;
        let controlled = i.controlled;
        if t.avel != [0; 2] {
            t.ang[0] = ((t.ang[0] + t.avel[0]) & 0xfff) as i16;
            t.ang[1] += t.avel[1];
            w.set_pose(t.ang);
        }
        if i.removed || !(controlled || (t.flags & ON != 0 && time_reached(now, t.next))) {
            return;
        }
        t.avel = [0; 2];
        t.next = now.wrapping_add(2);
        let o = i.origin;
        let barrel = t.barrel_pos(o);
        let mut update = false;
        let dir = if controlled {
            i.view
        } else {
            if !w.in_pvs() {
                t.next = now.wrapping_add(40);
                return;
            }
            let p = i.player_pos;
            let eye = [p[0], p[1] + i.view_height, p[2]];
            let range = isqrt_i32(dist2_3(eye, barrel));
            if range < t.min_range as i32 || (t.max_range > 0 && range > t.max_range as i32) {
                return;
            }
            if i.player_alive && w.trace_line(barrel, eye).is_none() {
                update = true;
                t.sight = [p[0] as i16, (p[1] + i.view_height / 2) as i16, p[2] as i16];
            }
            [
                t.sight[0] as i32 - o[0],
                t.sight[1] as i32 - o[1],
                t.sight[2] as i32 - o[2],
            ]
        };
        let dist = isqrt_i32(dist2_3(dir, [0; 3]));
        let mut yaw = atan_s(dir[2], dir[0]);
        let mut pitch = -atan_s(dir[1], isqrt_i32(dist2_xz(dir, [0; 3])));
        let (by, bz) = (t.barrel[1] as i32, t.barrel[2] as i32);
        if !controlled && (by | bz) != 0 {
            let d2 = (dist - bz).saturating_mul(dist - bz);
            yaw += atan_s(by, isqrt_i32((d2 - by * by).max(0)));
            pitch -= atan_s(-bz, isqrt_i32((d2 - bz * bz).max(0)));
        }
        let (yc, pc) = (t.centre[0] as i32, t.centre[1] as i32);
        yaw = yc + angle_dist(yaw, yc);
        let yr = t.range[0] as i32;
        if yaw > yc + yr || yaw < yc - yr {
            yaw = yaw.clamp(yc - yr, yc + yr);
            update = false;
        }
        if update {
            t.sight_time = now;
        }
        let pr = t.range[1] as i32;
        pitch = (pc + angle_dist(pitch, pc)).clamp(pc - pr, pc + pr);
        let dy = angle_dist(yaw, t.ang[0] as i32);
        let dx = angle_dist(pitch, t.ang[1] as i32);
        t.avel = [
            (dy / 2).clamp(-(t.rate[0] as i32), t.rate[0] as i32) as i16,
            (dx / 2).clamp(-(t.rate[1] as i32), t.rate[1] as i32) as i16,
        ];
        if controlled {
            return;
        }
        let los = t.flags & LOS != 0;
        let aimed = dx.abs() < t.tol[1] as i32 && dy.abs() < t.tol[0] as i32;
        if now.wrapping_sub(t.sight_time) < t.persist as u16 && (aimed || los) {
            let f = vectors(t.ang)[0];
            let end = [
                barrel[0] + ((f[0] * dist) >> 12),
                barrel[1] + ((f[1] * dist) >> 12),
                barrel[2] + ((f[2] * dist) >> 12),
            ];
            if !los || Self::ray_hits_player(w, barrel, end) {
                let last = t.fire_last;
                t.fire_last = now.max(1);
                if last != 0 {
                    let count =
                        (now.wrapping_sub(last) as u32 * t.fire_rate as u32 / (20 * 256)) as i32;
                    if count > 0 {
                        t.fire(o, count, false, w);
                    } else {
                        t.fire_last = last;
                    }
                }
                return;
            }
        }
        t.fire_last = 0;
    }

    /// The player uses a control volume while standing at `p`: does this
    /// tank take them? `master_ok` is asked only for a volume they are in.
    pub fn try_control(&self, p: [i32; 3], master_ok: impl FnOnce() -> bool) -> ControlAttempt {
        if self.flags & CONTROL == 0 {
            return ControlAttempt::NotHere;
        }
        for &(mn, mx) in &self.controls[..self.n_controls as usize] {
            if (0..3).all(|c| p[c] >= mn[c] - 32 && p[c] <= mx[c] + 32) {
                return if master_ok() {
                    ControlAttempt::Taken
                } else {
                    ControlAttempt::Refused
                };
            }
        }
        ControlAttempt::NotHere
    }

    /// The mounted player pulls the trigger. `cooldown` is the player's
    /// wait in 1/256 ticks; a round goes out only when it is under one
    /// tick, and then one round's wait (1 / fire rate) is added.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn player_fire(
        &self,
        origin: [i32; 3],
        cooldown: &mut u16,
        w: &mut impl TankWorld,
    ) -> bool {
        if *cooldown >= 256 {
            return false;
        }
        *cooldown += (20 * 256 * 256 / self.fire_rate as u32).min(u16::MAX as u32 - 256) as u16;
        if self.kind >> CLASS_SHIFT == 0 {
            w.gun_sound();
        }
        self.fire(origin, 1, true, w);
        true
    }
}

/// The mounted player's trigger wait after one more tick.
pub const fn cooldown_after_tick(cooldown: u16) -> u16 {
    cooldown.saturating_sub(256)
}

/// Does a player at `p` keep a tank they took at `use_pos`?
pub const fn still_controlled(p: [i32; 3], use_pos: [i32; 3]) -> bool {
    dist2_3(p, use_pos) < 30 * 30
}
