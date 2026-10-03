//! Nihilanth brain: the sphere-shielded, crystal-recharged final boss. His
//! health lives here in skill units; the game's u8 actor health only mirrors
//! it. Free of PS1 state; the game feeds it through [`NihWorld`].
//!
//! Positions are world units in runtime axes (y up); his height is kept
//! x16 and his yaw in q12 turns x16. Time is the 20 Hz tick; he thinks on
//! even ticks.

use crate::setpiece_math::{dir_q12, dist2_3, time_reached, TraceHit};
use psx_math::{int32::isqrt_i32, sincos};

const N_SPHERES: u8 = 20;
const MAX_BALLS: usize = 6;
/// Height of his head (sight, zap launches) above his origin.
const HEAD_UP: i32 = 300;
/// The sphere ring's centre above his origin.
const SPHERES_UP: i32 = 240;

/// Facts the brain takes from nihilanth.mdl, in ticks at framerate 1.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NihModel {
    /// `float` / `float_open` length.
    pub float_len: u16,
    /// `attack1`, `attack2`, `recharge` and `attack1_open` length.
    pub attack_len: u16,
    /// The zap event in `attack1` / `attack1_open`.
    pub zap_event: u16,
    /// The teleport-ball event in `attack2`.
    pub tele_event: u16,
    /// `recharge`'s sphere events, in frames (x 2.5 for ticks).
    pub recharge_frames: [u8; 11],
    /// The open brain's hit sphere: centre ahead and up, and radius.
    pub brain_ahead: i32,
    pub brain_up: i32,
    pub brain_radius: i32,
}

/// The sequence he is playing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NihSeq {
    Float,
    Attack1,
    Attack2,
    Recharge,
    OpenAttack,
}

/// Sounds the brain plays.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NihSound {
    Attack,
    Ball,
    Tele,
    Recharge,
    Die,
    Laugh,
    Pain,
}

/// What the brain asks of, and tells, the game. Recharger ids are the
/// game's own handles for `n_recharger<level>` markers.
pub trait NihWorld {
    /// A uniform random integer in `0..n` from the shared impact generator.
    fn random_below(&mut self, n: u32) -> u32;
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit>;
    /// His prop position as the game now holds it.
    fn prop_pos(&mut self) -> [i32; 3];
    /// Move his prop to `pos` facing runtime yaw `yaw`.
    fn set_pose(&mut self, pos: [i32; 3], yaw: u16);
    /// The u8 health the HUD shows.
    fn set_shown_health(&mut self, health: u8);
    /// His death: health 0, the dead state and his death triggers.
    fn die(&mut self);
    /// The live `n_recharger<level>` marker: its id and origin.
    fn recharger(&mut self, level: u8) -> Option<(u16, [i32; 3])>;
    fn recharger_origin(&mut self, id: u16) -> [i32; 3];
    /// Was the recharger killtargeted (its crystal destroyed)?
    fn recharger_gone(&mut self, id: u16) -> bool;
    /// Fire `n_draw<level>`.
    fn fire_draw(&mut self, level: u8);
    /// Does the map name `n_teleport<n>` or `n_leaving<n>`?
    fn has_teleport(&mut self, n: u8) -> bool;
    /// The teleport ball reached the player: fire `n_leaving<n>` and touch
    /// `n_teleport<n>`.
    fn teleport_player(&mut self, n: u8);
    /// Damage the player; `from` notes the direction when given.
    fn hurt_player(&mut self, damage: u16, from: Option<[i32; 3]>);
    /// A ball's trail for this tick.
    fn ball_trail(&mut self, from: [i32; 3], to: [i32; 3], teleport: bool);
    fn sound(&mut self, sound: NihSound, at: [i32; 3]);
}

/// One tick's observations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NihInputs {
    pub now: u16,
    pub player_pos: [i32; 3],
    pub player_alive: bool,
    /// His u8 actor health is not yet zero.
    pub shown_alive: bool,
    /// Skill damage of a zap.
    pub zap_damage: u16,
}

/// The map's nihilanth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NihBrain {
    irritation: u8,
    level: u8,
    teleport: u8,
    spheres: u8,
    seq: NihSeq,
    dead: bool,
    head_hit: bool,
    health: i32,
    full: i32,
    z: i32,
    vz: i32,
    force: i32,
    yaw: i32,
    avel: i32,
    adj: i32,
    min_z: i32,
    max_z: i32,
    desired_z: i32,
    desired: [i32; 2],
    recharger: u16,
    seq_start: u16,
    seq_len: u16,
    last_seen: u16,
    shoot_end: u16,
    ball_pos: [[i32; 3]; MAX_BALLS],
    ball_vel: [[i32; 3]; MAX_BALLS],
    /// 0 free, 1 zap, 2 teleport.
    ball_kind: [u8; MAX_BALLS],
    pain_next: u16,
}

impl Default for NihBrain {
    fn default() -> Self {
        Self::new()
    }
}

impl NihBrain {
    pub const fn new() -> Self {
        Self {
            irritation: 0,
            level: 1,
            teleport: 1,
            spheres: 0,
            seq: NihSeq::Float,
            dead: false,
            head_hit: false,
            health: 0,
            full: 1,
            z: 0,
            vz: 0,
            force: 0,
            yaw: 0,
            avel: 0,
            adj: 512,
            min_z: -4096,
            max_z: 4096,
            desired_z: 0,
            desired: [4096, 0],
            recharger: u16::MAX,
            seq_start: 0,
            seq_len: 1,
            last_seen: 0,
            shoot_end: 0,
            ball_pos: [[0; 3]; MAX_BALLS],
            ball_vel: [[0; 3]; MAX_BALLS],
            ball_kind: [0; MAX_BALLS],
            pain_next: 0,
        }
    }

    /// A new map: no balls in flight, and the next unbound tick searches
    /// for him again.
    pub fn reset(&mut self) {
        self.dead = false;
        self.ball_kind = [0; MAX_BALLS];
    }

    /// While unbound, `dying` doubles as "already searched": a search marks
    /// it, and only [`Self::reset`] or a bind clears it.
    pub fn mark_searched(&mut self) {
        self.dead = true;
    }

    /// Bind the map's nihilanth at tick `now`: `full_health` skill health,
    /// standing at height `y` facing runtime yaw `yaw`, held between the
    /// `n_min` and `n_max` marker heights when the map has them.
    pub fn bind(
        &mut self,
        full_health: i32,
        y: i32,
        yaw: u16,
        min_z: Option<i32>,
        max_z: Option<i32>,
        model: &NihModel,
        now: u16,
    ) {
        let g = self;
        g.irritation = 0;
        g.level = 1;
        g.teleport = 1;
        g.spheres = N_SPHERES;
        g.dead = false;
        g.full = full_health;
        g.health = g.full;
        g.z = y * 16;
        g.vz = 0;
        g.force = 0;
        g.yaw = (yaw as i32) << 4;
        g.avel = 0;
        g.adj = 512;
        g.recharger = u16::MAX;
        g.seq = NihSeq::Float;
        g.seq_start = now;
        g.seq_len = model.float_len;
        g.last_seen = now.wrapping_sub(2000);
        // The clock restarts with the room: deadlines from the last attempt
        // must not lie ahead of it.
        g.shoot_end = now;
        g.pain_next = now;
        g.desired_z = 512;
        g.min_z = min_z.unwrap_or(-4096);
        g.max_z = max_z.unwrap_or(4096);
    }

    /// The fight starts.
    pub fn command_on(&mut self) {
        if self.irritation == 0 {
            self.irritation = 1;
        }
    }

    /// Is he dying (rising to his death height)?
    pub fn dying(&self) -> bool {
        self.dead
    }

    /// Remaining skill health.
    #[allow(dead_code)] // read by the host tests
    pub fn health(&self) -> i32 {
        self.health
    }

    /// 0 calm, 1 fighting, 2 head open, 3 head open and just hit there.
    #[allow(dead_code)] // read by the host tests
    pub fn irritation(&self) -> u8 {
        self.irritation
    }

    /// Flight state: (height x16, vertical speed x16 per second, thrust,
    /// yaw q12 x16, yaw rate, wanted height, height offset over the player).
    #[allow(dead_code)] // read by the host tests
    pub fn flight(&self) -> (i32, i32, i32, i32, i32, i32, i32) {
        (
            self.z,
            self.vz,
            self.force,
            self.yaw,
            self.avel,
            self.desired_z,
            self.adj,
        )
    }

    /// His height, whole units.
    #[allow(dead_code)] // read by the host tests
    pub fn height(&self) -> i32 {
        self.z >> 4
    }

    /// The sequence playing at `now`: (sequence, head open, length, elapsed).
    pub fn sequence(&self, now: u16) -> Option<(NihSeq, bool, u16, u16)> {
        if self.dead {
            return None;
        }
        Some((
            self.seq,
            self.irritation >= 2,
            self.seq_len,
            now.wrapping_sub(self.seq_start),
        ))
    }

    /// Spheres left and the ring's centre over his origin `pos`.
    pub fn spheres(&self, pos: [i32; 3]) -> Option<(u8, [i32; 3])> {
        if self.dead {
            return None;
        }
        Some((self.spheres, [pos[0], (self.z >> 4) + SPHERES_UP, pos[2]]))
    }

    /// A shot from `start` toward `end` at him standing at `pos`: does it
    /// pass through his brain?
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn note_shot(&mut self, pos: [i32; 3], start: [i32; 3], end: [i32; 3], model: &NihModel) {
        let o = pos;
        let yaw = ((self.yaw >> 4) & 0xfff) as u16;
        let c = [
            o[0] + (sincos::sin_q12(yaw) * model.brain_ahead >> 12),
            o[1] + model.brain_up,
            o[2] + (sincos::sin_q12(yaw.wrapping_add(1024) & 0xfff) * model.brain_ahead >> 12),
        ];
        let d = dir_q12(start, end);
        let t =
            ((c[0] - start[0]) * d[0] + (c[1] - start[1]) * d[1] + (c[2] - start[2]) * d[2]) >> 12;
        let q = [
            start[0] + (d[0] * t >> 12),
            start[1] + (d[1] * t >> 12),
            start[2] + (d[2] * t >> 12),
        ];
        self.head_hit = t > 0 && dist2_3(q, c) < model.brain_radius * model.brain_radius;
    }

    /// A hit of `dmg` at tick `now` while he stands at `pos`. Returns the u8
    /// health to show.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn damage(&mut self, dmg: u8, now: u16, pos: [i32; 3], w: &mut impl NihWorld) -> u8 {
        let g = self;
        if g.irritation == 3 {
            g.irritation = 2;
        }
        if g.irritation == 2 && g.head_hit && dmg > 2 {
            g.irritation = 3;
        }
        g.head_hit = false;
        let d = dmg as i32;
        if d >= g.health {
            g.health = 1;
            if g.irritation != 3 {
                return 1;
            }
        }
        g.health -= d;
        if g.health <= 0 {
            g.dead = true;
            g.desired_z = g.max_z;
            w.sound(NihSound::Die, pos);
            return 1;
        }
        if time_reached(now, g.pain_next) {
            g.pain_next = now.wrapping_add(40 + w.random_below(61) as u16);
            if g.health > g.full / 2 {
                w.sound(NihSound::Laugh, pos);
            } else if g.irritation >= 2 {
                w.sound(NihSound::Pain, pos);
            }
        }
        (g.health * 255 / g.full.max(1)).clamp(1, 255) as u8
    }

    /// Pick the next sequence.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn next_activity(
        &mut self,
        now: u16,
        player_seen: bool,
        dist: i32,
        facing: bool,
        model: &NihModel,
        w: &mut impl NihWorld,
    ) {
        let g = self;
        if (g.health < g.full / 2 || g.spheres < N_SPHERES / 2)
            && g.recharger == u16::MAX
            && g.level <= 9
        {
            match w.recharger(g.level) {
                Some((id, r)) => {
                    g.recharger = id;
                    let p = w.prop_pos();
                    g.desired_z = r[1];
                    let d = [r[0] - p[0], r[2] - p[2]];
                    let l = isqrt_i32(d[0] * d[0] + d[1] * d[1]).max(1);
                    g.desired = [d[0] * 4096 / l, d[1] * 4096 / l];
                }
                None => {
                    g.level += 1;
                    if g.level > 9 {
                        g.irritation = 2;
                    }
                }
            }
        }
        g.seq_start = now;
        // Sequences speed up as he weakens: framerate 2 - health / max.
        let rate = 8192 - g.health.clamp(0, g.full) * 4096 / g.full.max(1);
        let len = |ticks: u16| ((ticks as i32 * 4096) / rate) as u16;
        if g.recharger != u16::MAX {
            let r = w.recharger_origin(g.recharger);
            if ((g.z >> 4) - r[1]).abs() < 128 {
                if g.seq != NihSeq::Recharge {
                    let at = w.prop_pos();
                    w.sound(NihSound::Recharge, at);
                    w.fire_draw(g.level);
                }
                g.seq = NihSeq::Recharge;
                g.seq_len = len(model.attack_len);
                return;
            }
        } else if player_seen && g.irritation != 0 && dist < 256 && facing {
            g.seq = if g.irritation >= 2 && g.health < g.full / 2 {
                NihSeq::OpenAttack
            } else if w.random_below(2) == 0 {
                NihSeq::Attack1
            } else {
                NihSeq::Attack2
            };
            g.seq_len = len(model.attack_len);
            return;
        }
        g.seq = NihSeq::Float;
        g.seq_len = len(model.float_len);
    }

    /// One 20 Hz tick.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn tick(&mut self, i: &NihInputs, model: &NihModel, w: &mut impl NihWorld) {
        let now = i.now;
        self.tick_balls(i, w);
        let g = self;
        let pos = w.prop_pos();
        let p = i.player_pos;
        let head = [pos[0], (g.z >> 4) + HEAD_UP, pos[2]];
        let seen = i.player_alive && w.trace_line(head, p).is_none();
        g.z += g.vz / 20;
        g.yaw += g.avel / 20;
        if now & 1 == 0 {
            let yaw = ((g.yaw >> 4) & 0xfff) as u16;
            let right = [
                sincos::sin_q12(yaw.wrapping_add(1024) & 0xfff),
                sincos::sin_q12(yaw.wrapping_add(2048) & 0xfff),
            ];
            let side = (g.desired[0] * right[0] + g.desired[1] * right[1]) >> 12;
            // A degree is 4096 / 360 q12 turns; x16 fixed point.
            const DEG: i32 = 182;
            // Runtime yaw turns clockwise: a heading to the left lowers it.
            if side < 0 {
                if g.avel > -180 * DEG {
                    g.avel -= 6 * DEG;
                }
            } else if g.avel < 180 * DEG {
                g.avel += 6 * DEG;
            }
            g.avel = g.avel * 98 / 100;
            let est = (g.z >> 4) + (g.vz >> 3) + (g.force * 5 >> 2);
            g.vz += g.force;
            g.vz = g.vz * 995 / 1000;
            if g.force < 100 * 16 && est < g.desired_z {
                g.force += 10 * 16;
            } else if g.force > -100 * 16 && est > g.desired_z {
                g.force -= 10 * 16;
            }
        }
        w.set_pose([pos[0], (g.z >> 4), pos[2]], ((g.yaw >> 4) & 0xfff) as u16);
        if now & 1 != 0 {
            return;
        }
        if g.dead {
            if ((g.z >> 4) - g.max_z).abs() < 16 && i.shown_alive {
                w.die();
            }
            return;
        }
        if g.health < g.full && g.spheres > 0 {
            g.spheres -= 1;
            g.health = (g.health + g.full / N_SPHERES as i32).min(g.full);
        }
        w.set_shown_health((g.health * 255 / g.full.max(1)).clamp(1, 255) as u8);
        if seen && g.recharger == u16::MAX {
            g.last_seen = now;
            let d = [p[0] - pos[0], p[2] - pos[2]];
            let l = isqrt_i32(d[0] * d[0] + d[1] * d[1]).max(1);
            g.desired = [d[0] * 4096 / l, d[1] * 4096 / l];
            g.desired_z = p[1] + g.adj;
        } else if g.recharger == u16::MAX {
            g.adj = (g.adj + 10).min(1000);
        }
        g.desired_z = g.desired_z.clamp(g.min_z, g.max_z);
        let t = now.wrapping_sub(g.seq_start) as i32;
        let seq_len = g.seq_len as i32;
        let at = |frame_ticks: u16| frame_ticks as i32 * seq_len / model.attack_len as i32;
        match g.seq {
            NihSeq::Attack1 if t == at(model.zap_event) => {
                g.shoot_end = now.wrapping_add(20);
                Self::zen_sound(head, w);
            }
            NihSeq::OpenAttack if t == at(model.zap_event) => {
                g.launch(head, p, 1);
                Self::zen_sound(head, w);
            }
            NihSeq::Attack2 if t == at(model.tele_event) => {
                if w.has_teleport(g.teleport) {
                    w.sound(NihSound::Attack, head);
                    w.sound(NihSound::Tele, head);
                    g.launch(head, p, 2);
                } else {
                    g.teleport += 1;
                    g.shoot_end = now.wrapping_add(20);
                    w.sound(NihSound::Ball, head);
                }
            }
            NihSeq::Recharge => {
                if model
                    .recharge_frames
                    .iter()
                    .any(|&f| t == at((f as u16 * 5) / 2))
                {
                    if g.spheres < N_SPHERES {
                        g.spheres += 1;
                    } else {
                        g.recharger = u16::MAX;
                    }
                }
                if g.recharger != u16::MAX && w.recharger_gone(g.recharger) {
                    g.recharger = u16::MAX;
                }
            }
            _ => {}
        }
        // A pair of zaps from the hands every 0.2 s while the volley lasts.
        if !time_reached(now, g.shoot_end) && t % 4 == 0 {
            g.launch([head[0] + 100, head[1] - 150, head[2]], p, 1);
            g.launch([head[0] - 100, head[1] - 150, head[2]], p, 1);
        }
        if t >= g.seq_len as i32 {
            if g.seq == NihSeq::Recharge && g.spheres >= N_SPHERES {
                g.recharger = u16::MAX;
            }
            let yaw = ((g.yaw >> 4) & 0xfff) as u16;
            let facing = g.desired[0] * sincos::sin_q12(yaw)
                + g.desired[1] * sincos::sin_q12(yaw.wrapping_add(1024) & 0xfff)
                > 0;
            g.next_activity(
                now,
                !time_reached(now, g.last_seen.wrapping_add(100)),
                ((g.z >> 4) - g.desired_z).abs(),
                facing,
                model,
                w,
            );
        }
    }

    /// The ball launch: an attack cry one time in five.
    fn zen_sound(head: [i32; 3], w: &mut impl NihWorld) {
        if w.random_below(5) == 0 {
            w.sound(NihSound::Attack, head);
        }
        w.sound(NihSound::Ball, head);
    }

    /// A zap (1) or teleport (2) ball from `from` toward `to`.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn launch(&mut self, from: [i32; 3], to: [i32; 3], kind: u8) {
        let g = self;
        if let Some(b) = (0..MAX_BALLS).find(|&b| g.ball_kind[b] == 0) {
            g.ball_kind[b] = kind;
            g.ball_pos[b] = from;
            let d = dir_q12(from, to);
            // 200 u/s; the teleport ball keeps a fifth of its climb.
            g.ball_vel[b] = [
                d[0] * 10 >> 12,
                d[1] * if kind == 2 { 2 } else { 10 } >> 12,
                d[2] * 10 >> 12,
            ];
        }
    }

    /// Move the balls in flight.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn tick_balls(&mut self, i: &NihInputs, w: &mut impl NihWorld) {
        let g = self;
        let p = i.player_pos;
        for b in 0..MAX_BALLS {
            let kind = g.ball_kind[b];
            if kind == 0 {
                continue;
            }
            let pos = g.ball_pos[b];
            let d = isqrt_i32(dist2_3(pos, p));
            if kind == 1 {
                // Accelerate 1.2x per tick up to 2000 u/s; within 256 of the
                // player, a zap.
                let v = g.ball_vel[b];
                if isqrt_i32(dist2_3(v, [0; 3])) < 100 {
                    g.ball_vel[b] = v.map(|x| x * 6 / 5);
                }
                if d < 256 {
                    g.ball_kind[b] = 0;
                    if w.trace_line(pos, p).is_none() && i.player_alive {
                        w.hurt_player(i.zap_damage, Some(pos));
                    }
                    continue;
                }
            } else {
                // Home on the player's centre: at most 300 u/s kept plus
                // 300 u/s toward the target; within 128, teleport them.
                let dir = dir_q12(pos, p);
                let v = g.ball_vel[b];
                let s = isqrt_i32(dist2_3(v, [0; 3])).max(15);
                g.ball_vel[b] = [0, 1, 2].map(|k| v[k] * 15 / s + (dir[k] * 15 >> 12));
                if d < 128 {
                    g.ball_kind[b] = 0;
                    w.teleport_player(g.teleport);
                    continue;
                }
            }
            let v = g.ball_vel[b];
            let next = [pos[0] + v[0], pos[1] + v[1], pos[2] + v[2]];
            if w.trace_line(pos, next).is_some() {
                // A zap ball bursting on a wall shocks within 125.
                if kind == 1 && d < 125 {
                    w.hurt_player((50 * (125 - d) / 125) as u16, None);
                }
                g.ball_kind[b] = 0;
                continue;
            }
            g.ball_pos[b] = next;
            w.ball_trail(pos, next, kind == 2);
        }
    }
}
