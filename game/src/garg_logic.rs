//! Gargantua brain: its attack schedule (swipe, stomp, flame sweep, chase),
//! the travelling stomp wave, footstep shakes, pain cadence, the damage
//! rule and the death timeline. Free of PS1 state; the game feeds it
//! through [`GargWorld`]. Target acquisition stays with the game's shared
//! actor AI and arrives as inputs.
//!
//! The stomp wave's motion and the attack choice are `setpiece_logic`'s
//! (`Stomp`, `garg_choice`); this module schedules and applies them.
//! Positions are world units in runtime axes (y up); yaw is q12 turns with
//! 0 = +z and 1024 = +x; time is the 20 Hz tick.

use crate::setpiece_logic::{self as sl, GargChoice, Stomp};
use crate::setpiece_math::{atan_s, dir_q12, dist2_3, time_reached, TraceHit};
use psx_math::{int32::isqrt_i32, sincos};

const FLAME_LENGTH: i32 = 330;

/// Facts the brain takes from garg.mdl.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GargModel {
    /// Chase speed in units per tick (the run sequence's travel).
    pub run_per_tick: i32,
    /// The two forearm flame attachments (forward, left, up) from the origin.
    pub flame_attach: [[i32; 3]; 2],
    /// Swipe: ticks to the slash event, and the whole sequence.
    pub swipe_event: u8,
    pub swipe_len: u8,
    /// Stomp: ticks to the stomp event, and the whole sequence.
    pub stomp_event: u8,
    pub stomp_len: u8,
}

/// The actor state the brain puts the gargantua in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GargState {
    Idle,
    Attack,
    Move,
}

/// Sounds the brain plays.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GargSound {
    Pain,
    FlameOn,
    FlameOff,
    Step,
    /// The stomp landing.
    Stomp,
    /// A death-sequence explosion.
    Explosion,
}

/// What the brain asks of, and tells, the game.
pub trait GargWorld {
    /// A uniform random integer in `0..n` from the shared impact generator.
    fn random_below(&mut self, n: u32) -> u32;
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit>;
    /// Where `from -> to` enters the player's box grown by `pad` on every
    /// side (q12 of the segment); None when it misses or the player is dead.
    fn player_box_frac(&mut self, from: [i32; 3], to: [i32; 3], pad: i32) -> Option<i32>;
    /// Does `from -> to` cross the current (non-player) target's box?
    fn target_box_hit(&mut self, from: [i32; 3], to: [i32; 3]) -> bool;
    fn set_state(&mut self, state: GargState);
    /// Hold on to this tick's target.
    fn keep_target(&mut self);
    /// Drop the target.
    fn clear_target(&mut self);
    fn face(&mut self, point: [i32; 3]);
    /// Walk toward `point`, up to `step` units this tick.
    fn run_towards(&mut self, point: [i32; 3], step: i32);
    fn hurt_player(&mut self, damage: u16, from: [i32; 3]);
    /// Kick the player's view by (pitch, yaw) in q12 turns.
    fn view_punch(&mut self, pitch: i32, yaw: i32);
    fn damage_target(&mut self, damage: u8);
    /// A screen shake the player feels at `amplitude` for `ticks` (a
    /// stronger running shake may win).
    fn shake(&mut self, amplitude: i32, ticks: u16);
    fn sound(&mut self, sound: GargSound, at: [i32; 3]);
    /// Keep the flame loop playing at `at`.
    fn flame_loop(&mut self, at: [i32; 3]);
    fn flame_loop_mute(&mut self);
    fn flame_loop_stop(&mut self);
    /// A flame beam drawn from `from` to `to` (`core` for the hot inner one).
    fn flame_beam(&mut self, from: [i32; 3], to: [i32; 3], core: bool);
    /// Dust kicked up under the stomp wave.
    fn stomp_dust(&mut self, at: [i32; 3]);
    fn explosion_fx(&mut self, at: [i32; 3], mag: u8);
    /// Burst into gibs at `at` and leave the map.
    fn gib(&mut self, at: [i32; 3]);
}

/// One schedule tick's observations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GargInputs {
    pub now: u16,
    /// The target is in sight.
    pub visible: bool,
    /// Where to aim at the target, None when it has none.
    pub aim: Option<[i32; 3]>,
    pub target_is_player: bool,
    /// The target's origin.
    pub enemy_pos: [i32; 3],
    pub pos: [i32; 3],
    pub yaw: u16,
    /// The player's origin and liveness, for flame, stomp aim and shakes.
    pub player_pos: [i32; 3],
    pub player_alive: bool,
    pub view_height: i32,
    /// Skill damage of a swipe, and of the flame per 0.1 s.
    pub slash_damage: u16,
    pub flame_damage: i32,
}

/// One world tick's observations (the stomp wave and the death timeline
/// run while the actor loop is not looking).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GargWorldInputs {
    pub now: u16,
    /// The slot still holds the gargantua.
    pub present: bool,
    pub active: bool,
    pub dead: bool,
    pub pos: [i32; 3],
    pub player_pos: [i32; 3],
    pub stomp_damage: u16,
}

/// The map's gargantua.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GargBrain {
    /// 0 none, 1 swipe, 2 stomp: the attack sequence playing.
    gesture: u8,
    burn_tenths: u8,
    feet: u8,
    stomp_on: bool,
    stomp_first: bool,
    removed: bool,
    /// A stomp needs the enemy in sight continuously until this tick.
    see: u16,
    /// No new flame sweep before this tick.
    flame_next: u16,
    /// End of a running flame sweep, 0 when not flaming.
    flame_end: u16,
    gesture_start: u16,
    stomp_tick: u16,
    /// Tick of death; 0xffff while alive.
    died: u16,
    /// Flame aim (pitch, yaw) in q12 turns off the body.
    flame_ang: [i32; 2],
    stomp_pos: [i32; 3],
    stomp_dir: [i32; 3],
    stomp: Stomp,
    pain_next: u16,
}

/// Unit vector of a yaw (0 = +z, 1024 = +x), q12.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
fn forward(yaw: u16) -> [i32; 3] {
    [
        sincos::sin_q12(yaw & 0xfff),
        0,
        sincos::sin_q12(yaw.wrapping_add(1024) & 0xfff),
    ]
}

/// `o + dir * len` for a q12 direction.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
fn along(o: [i32; 3], dir: [i32; 3], len: i32) -> [i32; 3] {
    [
        o[0] + ((dir[0] * len) >> 12),
        o[1] + ((dir[1] * len) >> 12),
        o[2] + ((dir[2] * len) >> 12),
    ]
}

/// A hit on the gargantua scaled onto its u8 actor health standing for
/// `full_health`: only heavy damage (blast, energy, crush, mortar) counts,
/// and any heavy hit takes at least 1.
pub fn scale_damage(dmg: u8, heavy: bool, full_health: u32) -> u8 {
    if !heavy {
        return 0;
    }
    let full = full_health.max(1);
    ((dmg as u32 * 255 + full / 2) / full).max(1) as u8
}

impl Default for GargBrain {
    fn default() -> Self {
        Self::new()
    }
}

impl GargBrain {
    pub const fn new() -> Self {
        Self {
            gesture: 0,
            burn_tenths: 0,
            feet: 0,
            stomp_on: false,
            stomp_first: false,
            removed: false,
            see: 0,
            flame_next: 0,
            flame_end: 0,
            gesture_start: 0,
            stomp_tick: 0,
            died: 0xffff,
            flame_ang: [0; 2],
            stomp_pos: [0; 3],
            stomp_dir: [0; 3],
            stomp: Stomp::new(0),
            pain_next: 0,
        }
    }

    /// Forget the previous gargantua (a new map or a new one).
    pub fn reset(&mut self) {
        self.stomp_on = false;
        self.died = 0xffff;
        self.removed = false;
        self.gesture = 0;
        self.flame_end = 0;
        self.pain_next = 0;
    }

    /// A gargantua first seen at tick `now`: no stomp for 5 s, no flame for
    /// 2 s.
    pub fn spawn(&mut self, now: u16) {
        self.reset();
        self.see = now.wrapping_add(100);
        self.flame_next = now.wrapping_add(40);
    }

    /// Is a flame sweep running?
    #[allow(dead_code)] // read by the host tests
    pub fn flaming(&self) -> bool {
        self.flame_end != 0
    }

    /// The attack sequence playing at `now`: (1 swipe or 2 stomp, its
    /// length, elapsed ticks).
    pub fn gesture(&self, now: u16, model: &GargModel) -> Option<(u8, u8, u16)> {
        if self.gesture == 0 {
            return None;
        }
        let len = if self.gesture == 1 {
            model.swipe_len
        } else {
            model.stomp_len
        };
        Some((self.gesture, len, now.wrapping_sub(self.gesture_start)))
    }

    /// A hit landed: a pain cry at most every 2.5 to 4 s.
    pub fn pain(&mut self, now: u16, pos: [i32; 3], w: &mut impl GargWorld) {
        if time_reached(now, self.pain_next) {
            self.pain_next = now.wrapping_add(50 + w.random_below(31) as u16);
            w.sound(GargSound::Pain, pos);
        }
    }

    /// Shake as the player at `player` feels it from `from`.
    fn shake(
        from: [i32; 3],
        player: [i32; 3],
        amplitude: i32,
        ticks: u16,
        radius: i32,
        w: &mut impl GargWorld,
    ) {
        let amp = sl::shake_amplitude(amplitude, isqrt_i32(dist2_3(from, player)), radius);
        if amp > 0 {
            w.shake(amp, ticks);
        }
    }

    /// One schedule tick while the actor loop has it awake.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn think(&mut self, i: &GargInputs, model: &GargModel, w: &mut impl GargWorld) {
        let g = self;
        let now = i.now;
        if !i.visible {
            g.see = now.wrapping_add(100);
        }
        let Some(aim) = i.aim else {
            if g.gesture == 0 {
                w.set_state(GargState::Idle);
                w.clear_target();
            }
            g.flame_end = 0;
            return;
        };
        w.keep_target();
        let pos = i.pos;
        let f = forward(i.yaw);
        if g.flame_end != 0 {
            g.flame_task(i, aim, model, w);
            return;
        }
        if g.gesture != 0 {
            let t = now.wrapping_sub(g.gesture_start);
            let (event, len) = if g.gesture == 1 {
                (model.swipe_event, model.swipe_len)
            } else {
                (model.stomp_event, model.stomp_len)
            };
            if t < event as u16 {
                w.face(aim);
            } else if t == event as u16 {
                if g.gesture == 1 {
                    Self::swipe(i, f, w);
                } else {
                    g.stomp_attack(i, aim, f, w);
                    g.see = now.wrapping_add(240);
                }
            } else if t >= len as u16 {
                g.gesture = 0;
                w.set_state(GargState::Idle);
            }
            return;
        }
        let enemy = i.enemy_pos;
        let (dx, dz) = (enemy[0] - pos[0], enemy[2] - pos[2]);
        let dot = (f[0] * dx + f[2] * dz) / isqrt_i32(dx * dx + dz * dz).max(1);
        let dist = isqrt_i32(dist2_3(enemy, pos));
        let choice = if i.visible {
            sl::garg_choice(
                dot,
                dist,
                time_reached(now, g.see),
                time_reached(now, g.flame_next),
            )
        } else {
            GargChoice::Chase
        };
        w.set_state(GargState::Attack);
        match choice {
            GargChoice::Swipe => {
                g.gesture = 1;
                g.gesture_start = now;
            }
            GargChoice::Stomp => {
                g.gesture = 2;
                g.gesture_start = now;
            }
            GargChoice::Flame => {
                g.flame_end = now.wrapping_add(90).max(1);
                g.flame_next = now.wrapping_add(120);
                g.flame_ang = [0; 2];
                w.sound(GargSound::FlameOn, pos);
            }
            GargChoice::Chase => {
                w.set_state(GargState::Move);
                w.face(aim);
                w.run_towards(aim, model.run_per_tick);
                // Two footfalls per run cycle.
                g.feet = g.feet.wrapping_add(1);
                if g.feet % 11 == 0 {
                    Self::shake(pos, i.player_pos, 4, 20, 750, w);
                    w.sound(GargSound::Step, pos);
                }
            }
        }
    }

    /// The slash: a hull sweep from 64 up to 90 ahead and 27 down.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn swipe(i: &GargInputs, f: [i32; 3], w: &mut impl GargWorld) {
        let pos = i.pos;
        let from = [pos[0], pos[1] + 64, pos[2]];
        let mut to = along(from, f, 90);
        to[1] -= 27;
        let dmg = i.slash_damage;
        if w.player_box_frac(from, to, 16).is_some() {
            w.hurt_player(dmg, pos);
            w.view_punch(-341, -341);
        } else if !i.target_is_player && w.target_box_hit(from, to) {
            w.damage_target(dmg.min(255) as u8);
        }
    }

    /// The stomp: aim from 60 up and 35 ahead at the enemy and send a wave
    /// as far as a 1024 trace reaches.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn stomp_attack(&mut self, i: &GargInputs, aim: [i32; 3], f: [i32; 3], w: &mut impl GargWorld) {
        let g = self;
        let pos = i.pos;
        let mut start = along(pos, f, 35);
        start[1] += 60;
        let aim = if i.target_is_player {
            let p = i.player_pos;
            [
                p[0],
                p[1] + w.random_below(i.view_height as u32 + 1) as i32,
                p[2],
            ]
        } else {
            aim
        };
        let dir = dir_q12(start, aim);
        let far = along(start, dir, 1024);
        let end = w.trace_line(start, far).map_or(far, |h| h.pos);
        g.stomp = Stomp::new(isqrt_i32(dist2_3(start, end)));
        g.stomp_on = true;
        g.stomp_first = true;
        g.stomp_tick = i.now;
        g.stomp_pos = [start[0] << 4, start[1] << 4, start[2] << 4];
        g.stomp_dir = dir;
        Self::shake(pos, i.player_pos, 12, 40, 1000, w);
        w.sound(GargSound::Stomp, pos);
    }

    /// The flame sweep, aimed and applied every 0.1 s.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    fn flame_task(
        &mut self,
        i: &GargInputs,
        aim: [i32; 3],
        model: &GargModel,
        w: &mut impl GargWorld,
    ) {
        let g = self;
        let now = i.now;
        if time_reached(now, g.flame_end) {
            g.flame_end = 0;
            w.set_state(GargState::Idle);
            w.flame_loop_mute();
            w.sound(GargSound::FlameOff, i.pos);
            return;
        }
        w.flame_loop(i.pos);
        if now.wrapping_sub(g.flame_end) & 1 != 0 {
            return;
        }
        let pos = i.pos;
        let yaw = i.yaw as i32;
        let d = [aim[0] - pos[0], aim[1] - pos[1] - 64, aim[2] - pos[2]];
        let horiz = isqrt_i32(d[0] * d[0] + d[2] * d[2]);
        let want = [
            atan_s(d[1], horiz),
            sl::angle_dist_q12(atan_s(d[0], d[2]), yaw),
        ];
        if horiz.max(d[1].abs()) > 400 || want[1].abs() > 683 {
            // Too far or too far aside: the sweep and the cooldown both run
            // down at six times the rate.
            g.flame_end = g.flame_end.wrapping_sub(10);
            g.flame_next = g.flame_next.wrapping_sub(10);
        }
        g.flame_ang[0] = sl::approach_angle_q12(want[0], g.flame_ang[0], 46);
        g.flame_ang[1] = sl::approach_angle_q12(want[1].clamp(-512, 512), g.flame_ang[1], 91);
        let p = (g.flame_ang[0] & 0xfff) as u16;
        let (sp, cp) = (
            sincos::sin_q12(p),
            sincos::sin_q12(p.wrapping_add(1024) & 0xfff),
        );
        let fa = forward(((yaw + g.flame_ang[1]) & 0xfff) as u16);
        let dir = [(fa[0] * cp) >> 12, sp, (fa[2] * cp) >> 12];
        let f = forward(yaw as u16);
        let fire = i.flame_damage;
        let pp = i.player_pos;
        let spot = [pp[0], pp[1] + i.view_height / 2, pp[2]];
        for a in model.flame_attach {
            let start = [
                pos[0] + ((f[0] * a[0] - f[2] * a[1]) >> 12),
                pos[1] + a[2],
                pos[2] + ((f[2] * a[0] + f[0] * a[1]) >> 12),
            ];
            let far = along(start, dir, FLAME_LENGTH);
            let mut frac = w.trace_line(start, far).map_or(4096, |h| h.frac);
            if let Some(fr) = w.player_box_frac(start, far, 0) {
                frac = frac.min(fr);
            }
            let len = (FLAME_LENGTH * frac) >> 12;
            w.flame_beam(start, along(start, dir, len), false);
            w.flame_beam(start, along(start, dir, len * 2 / 5), true);
            let t = (((spot[0] - start[0]) * dir[0]
                + (spot[1] - start[1]) * dir[1]
                + (spot[2] - start[2]) * dir[2])
                >> 12)
                .clamp(0, len);
            let src = along(start, dir, t);
            if i.player_alive && w.trace_line(src, spot).is_none() {
                if let Some(tenths) = sl::flame_damage_tenths(fire, isqrt_i32(dist2_3(src, spot))) {
                    let total = g.burn_tenths as i32 + tenths;
                    w.hurt_player((total / 10) as u16, start);
                    g.burn_tenths = (total % 10) as u8;
                }
            }
        }
    }

    /// The stomp wave and the death timeline, every tick.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn tick_world(&mut self, i: &GargWorldInputs, w: &mut impl GargWorld) {
        let g = self;
        let now = i.now;
        if g.stomp_on && now.wrapping_sub(g.stomp_tick) & 1 == 0 {
            let from = [
                g.stomp_pos[0] >> 4,
                (g.stomp_pos[1] >> 4) + 30,
                g.stomp_pos[2] >> 4,
            ];
            if w.player_box_frac(from, along(from, g.stomp_dir, g.stomp.sweep_q4() >> 4), 16)
                .is_some()
            {
                w.hurt_player(i.stomp_damage, from);
            }
            let (moved, done) = g.stomp.think(g.stomp_first);
            g.stomp_first = false;
            g.stomp_pos = along(g.stomp_pos, g.stomp_dir, moved);
            if moved > 0 {
                w.stomp_dust([from[0], from[1] - 86, from[2]]);
            }
            g.stomp_on = !done;
        }
        if !i.present {
            return;
        }
        if g.died == 0xffff {
            if i.active && i.dead {
                g.died = now;
                g.flame_end = 0;
                g.gesture = 0;
                w.flame_loop_stop();
            }
            return;
        }
        // Four fireballs 0.6 s apart rising from 32 up, magnitudes 60..180
        // within 70 units; gibs 1.6 s in.
        let t = now.wrapping_sub(g.died);
        if t % 12 == 0 && t <= 36 {
            let k = (t / 12) as i32;
            let o = i.pos;
            let x = o[0] + w.random_below(141) as i32 - 70;
            let z = o[2] + w.random_below(141) as i32 - 70;
            w.explosion_fx([x, o[1] + 32 + 15 * k, z], (60 + 40 * k) as u8);
            w.sound(GargSound::Explosion, o);
        }
        if t == 32 && !g.removed {
            g.removed = true;
            w.gib(i.pos);
        }
    }
}
