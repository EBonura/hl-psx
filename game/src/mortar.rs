//! func_mortar_field and monster_mortar: the game side of
//! [`mortar_logic`]. This file only gathers the field's keys, answers the
//! brain's queries from the map and applies what it decides.

use crate::mortar_logic::{FieldUse, MortarState, MortarWorld};
use crate::setpiece_math::TraceHit;
use crate::*;

static mut FIELD: MortarState = MortarState::new();

#[inline(always)]
unsafe fn field() -> &'static mut MortarState {
    &mut *core::ptr::addr_of_mut!(FIELD)
}

pub(crate) unsafe fn reset() {
    field().reset();
}

/// Position (0..4096) of the momentary_rot_button named `name`, if the map
/// has one.
#[inline(never)]
#[optimize(size)]
unsafe fn controller(m: &Map, nlogic: usize, name: u16) -> Option<i32> {
    if name == 0 {
        return None;
    }
    for li in 0..nlogic {
        if LOGIC_KIND[li] == map::LOGIC_MOMENTARY {
            let rec = m.logic(li);
            if rec.targetname == name {
                return logic_valid_brush(rec.brush, m.n_ents)
                    .map(|ei| ENT_PHASE[ei].clamp(0, 4096));
            }
        }
    }
    None
}

struct World<'a> {
    m: &'a Map,
    nlogic: usize,
    controllers: [u16; 2],
}

impl MortarWorld for World<'_> {
    fn random_below(&mut self, n: u32) -> u32 {
        unsafe { impact_rng().below(n) }
    }
    fn controller(&mut self, axis: usize) -> Option<i32> {
        unsafe { controller(self.m, self.nlogic, self.controllers[axis]) }
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        let movers = unsafe {
            &*core::ptr::slice_from_raw_parts(
                core::ptr::addr_of!(MOVERS).cast::<phys::Mover>(),
                MOVER_COUNT.min(MAX_ENTS + 1),
            )
        };
        phys::trace_line(self.m, movers, from, to).map(trace_hit)
    }
    fn beam(&mut self, from: [i32; 3], to: [i32; 3]) {
        unsafe { push_tracer_styled(from, to, TRACER_MORTAR) }
    }
    fn explode(&mut self, at: [i32; 3], damage: u8, radius: i32, by_player: bool) {
        unsafe { explode(self.m, at, damage, radius, by_player) }
    }
    fn set_shake(&mut self, amplitude: u16, ticks: u16) {
        unsafe {
            SHAKE_AMP = amplitude;
            SHAKE_DUR = ticks;
            SHAKE_TICKS = ticks;
        }
    }
}

/// A use of a mortar field.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn field_use(m: &Map, nlogic: usize, rec: map::LogicEnt, now: u16) {
    let spread = if rec.aux_count != 0 {
        m.logic_aux(rec.first_aux).target as i32
    } else {
        0
    };
    let u = FieldUse {
        mins: rec.mins,
        maxs: rec.maxs,
        mode: (rec.speed >> 8) as u8,
        count: rec.speed as u8,
        spread,
        by_player: LOGIC_ACTIVATOR == 1,
        player_pos: LOGIC_PLAYER_POS,
    };
    let mut w = World {
        m,
        nlogic,
        controllers: [rec.arg0, rec.arg1],
    };
    field().field_use(&u, now, &mut w);
}

/// Land the shells whose time has come.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick(m: &Map, now: u16) {
    let mut w = World {
        m,
        nlogic: 0,
        controllers: [0; 2],
    };
    field().tick(now, LOGIC_PLAYER_POS, &mut w);
}
