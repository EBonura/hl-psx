//! monster_apache: the game side of [`apache_logic`]. One apache per map at
//! most (c2a5, c2a5a, c2a5w, c2a5x). This file decodes the cooked corner
//! chain, answers the brain's queries and applies what it decides.

use crate::apache_logic::{self, ApacheBrain, ApacheCorner, ApacheInputs, ApacheWorld};
use crate::setpiece_math::TraceHit;
use crate::*;
use hl_format::setpiece_audio as SP;

struct Apache {
    /// Its logic record, or u16::MAX when the map has none (or it is gone).
    li: u16,
    pi: u8,
    aux: u16,
    brain: ApacheBrain,
}

static mut AP: Apache = Apache {
    li: u16::MAX,
    pi: 0,
    aux: 0,
    brain: ApacheBrain::new(),
};

/// The tilt (reflected q8 pitch | roll << 8) the apache is drawn with.
pub(crate) static mut APACHE_TILT: (u8, u16) = (0xff, 0);

#[inline(always)]
unsafe fn ap() -> &'static mut Apache {
    &mut *core::ptr::addr_of_mut!(AP)
}

pub(crate) unsafe fn reset() {
    let a = ap();
    a.li = u16::MAX;
    APACHE_TILT.0 = 0xff;
    // See garg::reset: deadlines do not survive the room's clock.
    a.brain.reset();
}

/// Bind the map's apache.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn init(li: usize, rec: map::LogicEnt, pi: usize) {
    let a = ap();
    a.li = li as u16;
    a.pi = pi as u8;
    a.aux = rec.first_aux as u16;
    a.brain.bind(
        rec.aux_count,
        rec.flags,
        rec.speed,
        PROP_POS[pi],
        prop_yaw_value(PROP_YAW[pi]),
        SIM_NOW,
    );
}

/// Its trigger.
#[optimize(size)]
pub(crate) unsafe fn startup(li: usize) {
    let a = ap();
    if a.li as usize == li {
        a.brain.startup();
    }
}

/// A hit on the apache scaled onto its u8 actor health.
#[optimize(size)]
pub(crate) fn scale_damage(dmg: u8, blast: bool) -> u8 {
    let full = skill_table::SKILL_APACHE_HEALTH[settings::skill()] as u32;
    apache_logic::scale_damage(dmg, blast, full)
}

struct World<'a> {
    m: &'a Map,
    movers: &'a [phys::Mover],
    pi: usize,
    aux: usize,
    removed: bool,
}

impl ApacheWorld for World<'_> {
    fn corner(&mut self, k: usize) -> ApacheCorner {
        let fa = self.aux + k * 3;
        let m = self.m;
        let (a, b, c) = (m.logic_aux(fa), m.logic_aux(fa + 1), m.logic_aux(fa + 2));
        ApacheCorner {
            pos: [
                a.target as i16 as i32,
                a.delay_ticks as i16 as i32,
                b.target as i16 as i32,
            ],
            yaw_q12: c.target & 0xfff,
            pitch_q8: c.delay_ticks as u8 as i8,
        }
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        phys::trace_line(self.m, self.movers, from, to).map(trace_hit)
    }
    fn player_box_frac(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<i32> {
        unsafe {
            let p = LOGIC_PLAYER_POS;
            let h = LOGIC_PLAYER_HALF_HEIGHT;
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
    fn set_pos(&mut self, pos: [i32; 3]) {
        unsafe { prop_set_pos_exact(self.m, self.pi, pos) }
    }
    fn prop_pos(&mut self) -> [i32; 3] {
        unsafe { PROP_POS[self.pi] }
    }
    fn rotor(&mut self) {
        unsafe {
            setpiece_sfx::keep_loop(
                SP::APACHE_ROTOR,
                PROP_POS[self.pi],
                setpiece_sfx::OWNER_APACHE_ROTOR,
            )
        }
    }
    fn stop_rotor(&mut self) {
        unsafe { setpiece_sfx::stop_loop(setpiece_sfx::OWNER_APACHE_ROTOR) }
    }
    fn set_yaw(&mut self, yaw: u16) {
        unsafe { PROP_YAW[self.pi] = prop_with_yaw(PROP_YAW[self.pi], yaw) }
    }
    fn set_tilt(&mut self, tilt: u16) {
        unsafe { APACHE_TILT = (self.pi as u8, tilt) }
    }
    fn explosion_fx(&mut self, at: [i32; 3], mag: u8) {
        unsafe { queue_explosion_fx(at, mag) }
    }
    fn explode(&mut self, at: [i32; 3], damage: u8, radius: i32, by_player: bool) {
        unsafe { explode(self.m, at, damage, radius, by_player) }
    }
    fn gibs(&mut self, at: [i32; 3], count: u8) {
        unsafe { spawn_gibs(at, count) }
    }
    fn remove(&mut self) {
        unsafe { PROP_ACTIVE[self.pi] = 0 }
        self.removed = true;
    }
    fn rocket(&mut self, from: [i32; 3], dir: [i32; 3]) {
        unsafe {
            spawn_projectile_dir(PROJ_ROCKET, 150, from, dir, true);
        }
    }
    fn rocket_sound(&mut self, at: [i32; 3]) {
        unsafe { setpiece_sfx::play(SP::APACHE_ROCKET, at) }
    }
    fn gun_damage(&mut self) -> u8 {
        skill_damage(21).unwrap_or(8)
    }
    fn hurt_player(&mut self, damage: u8, from: [i32; 3]) {
        unsafe {
            PENDING_PLAYER_DAMAGE = PENDING_PLAYER_DAMAGE.saturating_add(damage as u16);
            note_damage_direction(from);
        }
    }
    fn tracer(&mut self, from: [i32; 3], to: [i32; 3]) {
        unsafe { push_tracer(from, to) }
    }
    fn gun_sound(&mut self, at: [i32; 3]) {
        unsafe { setpiece_sfx::play(SP::APACHE_GUN, at) }
    }
}

/// The apache's tick.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick(m: &Map, movers: &[phys::Mover]) {
    let a = ap();
    if a.li == u16::MAX {
        return;
    }
    let pi = a.pi as usize;
    if PROP_ACTIVE[pi] == 0 || PROP_KIND[pi] != 23 {
        a.li = u16::MAX;
        setpiece_sfx::stop_loop(setpiece_sfx::OWNER_APACHE_ROTOR);
        return;
    }
    let inputs = ApacheInputs {
        now: SIM_NOW,
        dead: PROP_HEALTH[pi] == 0,
        player_pos: LOGIC_PLAYER_POS,
        player_alive: LOGIC_PLAYER_HEALTH > 0,
        view_height: VIEW_HEIGHT,
        easy: settings::skill() == 0,
    };
    let mut w = World {
        m,
        movers,
        pi,
        aux: a.aux as usize,
        removed: false,
    };
    a.brain.tick(&inputs, &mut w);
    if w.removed {
        a.li = u16::MAX;
    }
}
