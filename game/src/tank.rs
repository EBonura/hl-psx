//! func_tank family: the game side of [`tank_logic`]. This file unpacks
//! the cooked keys (hl_format::logic::TANK), answers the brain's queries
//! from the map and applies what it decides.

use crate::setpiece_math::TraceHit;
use crate::tank_logic::{
    cooldown_after_tick, still_controlled, ControlAttempt, TankBrain, TankInputs, TankKeys,
    TankUse, TankWorld,
};
use crate::*;

pub(crate) const MAX_TANKS: usize = 6;

#[derive(Clone, Copy)]
struct Tank {
    li: u16,
    ei: u16,
    target: u16,
    brain: TankBrain,
}

static mut TANKS: [Option<Tank>; MAX_TANKS] = [None; MAX_TANKS];
pub(crate) static mut TANK_COUNT: usize = 0;
/// Where the player took the controls.
static mut TANK_USE_POS: [i32; 3] = [0; 3];

#[inline(always)]
unsafe fn tanks() -> &'static mut [Option<Tank>; MAX_TANKS] {
    &mut *core::ptr::addr_of_mut!(TANKS)
}

/// Reset the map's tanks.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tanks_init(m: &Map, now: u16) {
    TANK_COUNT = 0;
    for li in 0..m.n_logic.min(MAX_LOGIC) {
        if LOGIC_KIND[li] != map::LOGIC_TANK || TANK_COUNT >= MAX_TANKS {
            continue;
        }
        let rec = m.logic(li);
        if rec.aux_count < map::TANK_AUX_COUNT as usize {
            continue;
        }
        let w = |k: usize| {
            let a = m.logic_aux(rec.first_aux + k);
            (a.target, a.delay_ticks)
        };
        let (rates, yaw_range) = w(0);
        let (pitch_range, tol) = w(1);
        let (fire_rate, persist_flags) = w(2);
        let (bx, by) = w(3);
        let (bz, damage) = w(4);
        let (min_range, max_range) = w(5);
        let (yc, pc) = w(6);
        let mut controls = [([0; 3], [0; 3]); 2];
        let mut n_controls = 0;
        let mut k = map::TANK_AUX_COUNT as usize;
        while n_controls < 2 && k + 3 <= rec.aux_count {
            let mut b = [0i32; 6];
            for i in 0..3 {
                let (t, d) = w(k + i);
                b[i * 2] = t as i16 as i32;
                b[i * 2 + 1] = d as i16 as i32;
            }
            controls[n_controls] = ([b[0], b[1], b[2]], [b[3], b[4], b[5]]);
            n_controls += 1;
            k += 3;
        }
        let keys = TankKeys {
            spawnflags: rec.spawnflags,
            kind: (persist_flags >> 8) as u8,
            persistence: persist_flags as u8,
            turn_rate: [rates as u8, (rates >> 8) as u8],
            tolerance: [tol as u8, (tol >> 8) as u8],
            range: [yaw_range as i16, pitch_range as i16],
            fire_rate,
            barrel: [bx as i16, by as i16, bz as i16],
            damage,
            min_range: min_range as i16,
            max_range: max_range as i16,
            centre: [yc as i16, ((pc << 4) as i16) >> 4],
            controls,
            n_controls: n_controls as u8,
        };
        tanks()[TANK_COUNT] = Some(Tank {
            li: li as u16,
            ei: rec.brush,
            target: rec.target,
            brain: TankBrain::spawn(&keys, ENT_CACHE[rec.brush as usize].origin, now),
        });
        TANK_COUNT += 1;
    }
}

#[inline(never)]
#[optimize(size)]
unsafe fn slot(li: usize) -> Option<&'static mut Tank> {
    tanks()[..TANK_COUNT]
        .iter_mut()
        .flatten()
        .find(|t| t.li as usize == li)
}

/// A use of a tank the player cannot control (a dying gunner's trigger
/// stops it).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_use(li: usize, use_type: u8, now: u16) {
    let Some(t) = slot(li) else { return };
    let how = if use_type == map::USE_ON {
        TankUse::On
    } else if use_type == map::USE_OFF {
        TankUse::Off
    } else {
        TankUse::Toggle
    };
    t.brain.use_tank(how, now);
}

struct World<'a> {
    m: &'a Map,
    movers: &'a [phys::Mover],
    li: u16,
    ei: u16,
    target: u16,
}

impl TankWorld for World<'_> {
    fn in_pvs(&mut self) -> bool {
        unsafe { entity_touches_pvs(self.m, &ENT_CACHE[self.ei as usize]) }
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        phys::trace_line_skip(self.m, self.movers, from, to, self.ei as i32).map(trace_hit)
    }
    fn player_box_frac(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<i32> {
        unsafe {
            let p = LOGIC_PLAYER_POS;
            let h = PLAYER_HULL_HALF_HEIGHT;
            if LOGIC_PLAYER_HEALTH == 0 {
                return None;
            }
            segment_box_frac(
                from,
                to,
                [p[0] - 16, p[1] - h, p[2] - 16],
                [p[0] + 16, p[1] + h, p[2] + 16],
            )
        }
    }
    fn random_below(&mut self, n: u32) -> u32 {
        unsafe { impact_rng().below(n) }
    }
    fn skill_round_damage(&mut self, bullet: u8) -> u8 {
        skill_damage([1, 1, 8, 21][bullet as usize & 3]).unwrap_or(8)
    }
    fn set_pose(&mut self, angles: [i16; 2]) {
        // The cooker's retained-angle packing: reflected q8 turns per axis.
        unsafe {
            let ei = self.ei as usize;
            let q8 = |a: i16| ((-(a as i32) + 8) >> 4) as u32 & 0xff;
            let packed =
                q8(angles[1]) | (q8(angles[0]) << 8) | (ENT_CACHE[ei].mv[2] as u32 & 0x00ff_0000);
            if ENT_CACHE[ei].mv[2] as u32 != packed {
                ENT_CACHE[ei].mv[2] = packed as i32;
                ENT_PHASE[ei] ^= 1; // a new render pose: leave the static brush pass
                live_entity_pvs_mark_dirty(ei);
            }
        }
    }
    fn launch_rocket(&mut self, from: [i32; 3], dir: [i32; 3], by_player: bool) {
        unsafe {
            spawn_projectile_dir(PROJ_ROCKET, 100, from, dir, !by_player);
        }
    }
    fn tracer(&mut self, from: [i32; 3], to: [i32; 3]) {
        unsafe { push_tracer(from, to) }
    }
    fn explode(&mut self, at: [i32; 3], magnitude: u8, radius: i32, by_player: bool) {
        unsafe { explode(self.m, at, magnitude, radius, by_player) }
    }
    fn player_round(&mut self, barrel: [i32; 3], end: [i32; 3], damage: u8) {
        // The mounted gun's round goes through the player's hitscan, aimed
        // along `barrel -> end`.
        unsafe {
            let a = [
                crate::setpiece_math::atan_s(end[2] - barrel[2], end[0] - barrel[0]),
                0,
            ];
            let d = [end[0] - barrel[0], end[1] - barrel[1], end[2] - barrel[2]];
            let pitch = crate::setpiece_math::atan_s(d[1], isqrt_i32(d[0] * d[0] + d[2] * d[2]));
            let rot = view_rotation(((1024 - a[0]) & 0xfff) as u16, pitch as i16);
            let base_t = [
                -dot12(rot.m[0], barrel),
                -dot12(rot.m[1], barrel),
                -dot12(rot.m[2], barrel),
            ];
            fire_hitscan(
                self.m,
                self.movers,
                barrel,
                &rot,
                base_t,
                damage,
                false,
                8192,
                2,
                2,
                0,
                0,
            );
        }
    }
    fn hurt_player(&mut self, damage: u8, from: [i32; 3]) {
        unsafe {
            PENDING_PLAYER_DAMAGE = PENDING_PLAYER_DAMAGE.saturating_add(damage as u16);
            note_damage_direction(from);
        }
    }
    fn fire_target(&mut self) {
        unsafe {
            logic_fire_targets(
                self.m,
                self.m.n_logic.min(MAX_LOGIC),
                self.m.n_ents,
                self.target,
                map::USE_TOGGLE,
                SIM_NOW,
                0,
                self.li,
            );
        }
    }
    fn gun_sound(&mut self) {
        unsafe { sfx::play(sfx::MP5) }
    }
}

/// Every tank of the map, once per tick. `view` is the player's aim
/// direction, which the mounted tank mirrors.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_tanks(m: &Map, movers: &[phys::Mover], view: [i32; 3], now: u16) {
    TANK_FIRE_CD = cooldown_after_tick(TANK_FIRE_CD);
    for i in 0..TANK_COUNT {
        let Some(t) = &mut tanks()[i] else { continue };
        let inputs = TankInputs {
            now,
            controlled: MOUNTED_TANK == t.li as i32,
            removed: LOGIC_STATE[t.li as usize] == LOGIC_STATE_REMOVED,
            origin: ENT_CACHE[t.ei as usize].origin,
            view,
            player_pos: LOGIC_PLAYER_POS,
            player_alive: LOGIC_PLAYER_HEALTH > 0,
            view_height: VIEW_HEIGHT,
        };
        let mut w = World {
            m,
            movers,
            li: t.li,
            ei: t.ei,
            target: t.target,
        };
        t.brain.tick(&inputs, &mut w);
    }
}

/// The player uses at `p`: a tank whose control volume they stand in takes
/// them, master permitting.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_try_control(m: &Map, nlogic: usize, p: [i32; 3]) -> bool {
    for t in tanks()[..TANK_COUNT].iter().flatten() {
        let li = t.li;
        match t
            .brain
            .try_control(p, || master_ok(m, nlogic, m.logic(li as usize).arg1))
        {
            ControlAttempt::NotHere => continue,
            ControlAttempt::Taken => {
                MOUNTED_TANK = li as i32;
                TANK_USE_POS = p;
                TANK_FIRE_CD = 0;
                sfx::play(sfx::BUTTON);
            }
            ControlAttempt::Refused => sfx::play(sfx::DRY),
        }
        return true;
    }
    false
}

/// Does the player at `p` keep the tank they took?
#[optimize(size)]
pub(crate) unsafe fn tank_still_controlled(p: [i32; 3]) -> bool {
    still_controlled(p, TANK_USE_POS)
}

/// The mounted player's trigger.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_player_fire(m: &Map, movers: &[phys::Mover]) -> bool {
    let Some(t) = slot(MOUNTED_TANK as usize) else {
        return false;
    };
    let mut w = World {
        m,
        movers,
        li: t.li,
        ei: t.ei,
        target: t.target,
    };
    let origin = ENT_CACHE[t.ei as usize].origin;
    let mut cd = TANK_FIRE_CD;
    let fired = t.brain.player_fire(origin, &mut cd, &mut w);
    TANK_FIRE_CD = cd;
    fired
}
