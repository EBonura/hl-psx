//! monster_osprey: the game side of [`osprey_logic`]. This file decodes the
//! cooked corner chain, answers the brain's queries from the prop table and
//! applies what it decides.

use crate::osprey_logic::{Corner, OspreyBrain, OspreyInputs, OspreyWorld, PROP_SLOTS};
use crate::setpiece_math::TraceHit;
use crate::*;
use hl_format::setpiece_audio as SP;

const _: () = assert!(PROP_SLOTS == MAX_PROPS);

pub(crate) const PROP_TYPE_OSPREY: u8 = 61;

/// One osprey per map.
pub(crate) struct Osprey {
    /// Its logic record, or u16::MAX when the map has none (or it is gone).
    pub(crate) li: u16,
    pub(crate) aux: u16,
    pub(crate) pi: u8,
    brain: OspreyBrain,
}

pub(crate) static mut OSPREY: Osprey = Osprey {
    li: u16::MAX,
    aux: 0,
    pi: 0,
    brain: OspreyBrain::new(),
};
/// The drawn tilt; u16::MAX keeps the authored one until the first think.
pub(crate) static mut OSPREY_TILT: u16 = 0;

#[inline(always)]
unsafe fn osprey() -> &'static mut Osprey {
    &mut *core::ptr::addr_of_mut!(OSPREY)
}

/// Corner `k` of the osprey's cooked chain.
#[inline(never)]
#[optimize(size)]
unsafe fn osprey_corner(m: &Map, k: usize) -> Corner {
    let fa = OSPREY.aux as usize + k * 3;
    let (a, b, c) = (m.logic_aux(fa), m.logic_aux(fa + 1), m.logic_aux(fa + 2));
    let q = |v: u16| ((v as u8 as i8) as i16) << 4;
    Corner {
        pos: [a.target as i16, a.delay_ticks as i16, b.target as i16],
        speed: b.delay_ticks,
        ang: [
            q(c.delay_ticks),
            (c.target & 0xfff) as i16,
            q(c.delay_ticks >> 8),
        ],
    }
}

/// Bind the osprey record at map load.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn osprey_init(li: usize, rec: map::LogicEnt, pi: usize) {
    let o = osprey();
    o.li = li as u16;
    o.aux = rec.first_aux as u16;
    o.pi = pi as u8;
    o.brain.bind(
        rec.aux_count,
        rec.flags,
        PROP_POS[pi],
        rec.speed & 0x40 != 0,
        SIM_NOW,
    );
    OSPREY_TILT = u16::MAX;
}

/// A trigger aimed at the osprey.
pub(crate) unsafe fn command_use(now: u16) {
    osprey().brain.command_use(now);
}

struct World<'a> {
    m: &'a Map,
    movers: &'a [phys::Mover],
    pi: usize,
    removed: bool,
}

impl OspreyWorld for World<'_> {
    fn corner(&mut self, k: usize) -> Corner {
        unsafe { osprey_corner(self.m, k) }
    }
    fn authored_attitude(&mut self) -> (u16, i16) {
        let tilt = ((self.m.prop_orientation(self.pi) as u32) >> 16) as u16;
        (tilt, unsafe { (PROP_YAW[self.pi] & PROP_YAW_MASK) as i16 })
    }
    fn grunt_scan_slots(&mut self) -> usize {
        unsafe { PROP_COUNT.min(CARRY_MAILBOX_FIRST) }
    }
    fn is_live_grunt(&mut self, pi: usize) -> bool {
        unsafe { PROP_KIND[pi] == 8 && PROP_ACTIVE[pi] != 0 && PROP_HEALTH[pi] != 0 }
    }
    fn is_down(&mut self, pi: usize) -> bool {
        unsafe { PROP_ACTIVE[pi] == 0 || PROP_HEALTH[pi] == 0 }
    }
    fn is_dead(&mut self, pi: usize) -> bool {
        unsafe { PROP_HEALTH[pi] == 0 }
    }
    fn respawn_grunt(&mut self, pi: usize, at: [i32; 3], yaw: u16) -> i32 {
        unsafe {
            PROP_ACTIVE[pi] = 1;
            PROP_HEALTH[pi] = prop_start_health(8);
            PROP_STATE[pi] = PROP_STATE_IDLE;
            PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
            PROP_AI_TIMER[pi] = 0;
            PROP_DORMANT[pi] |= PROP_RUNTIME_PRISONER; // gliding until it lands
            PROP_YAW[pi] = prop_with_yaw(PROP_YAW[pi], yaw);
            prop_set_pos_exact(self.m, pi, at);
            seed_prop_render_transform(pi);
            prop_floor_y_down(self.m, pi, at, 4096).unwrap_or(at[1])
        }
    }
    fn is_on_rope(&mut self, pi: usize) -> bool {
        unsafe { PROP_DORMANT[pi] & PROP_RUNTIME_PRISONER != 0 }
    }
    fn grunt_pos(&mut self, pi: usize) -> [i32; 3] {
        unsafe { PROP_POS[pi] }
    }
    fn set_grunt_pos(&mut self, pi: usize, pos: [i32; 3]) {
        unsafe { prop_set_pos_exact(self.m, pi, pos) }
    }
    fn land_grunt(&mut self, pi: usize) {
        unsafe { PROP_DORMANT[pi] &= !PROP_RUNTIME_PRISONER }
    }
    fn rope(&mut self, from: [i32; 3], to: [i32; 3]) {
        unsafe { push_tracer_styled(from, to, TRACER_ROPE) }
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        phys::trace_line(self.m, self.movers, from, to).map(trace_hit)
    }
    fn random_below(&mut self, n: u32) -> u32 {
        unsafe { impact_rng().below(n) }
    }
    fn explosion_fx(&mut self, at: [i32; 3], mag: u8) {
        unsafe { queue_explosion_fx(at, mag) }
    }
    fn explode(&mut self, at: [i32; 3], damage: u8, radius: i32, by_player: bool) {
        unsafe { explode(self.m, at, damage, radius, by_player) }
    }
    fn remove(&mut self) {
        unsafe { PROP_ACTIVE[self.pi] = 0 }
        self.removed = true;
    }
    fn rotor(&mut self, at: [i32; 3]) {
        unsafe { setpiece_sfx::keep_loop(SP::OSPREY_ROTOR, at, setpiece_sfx::OWNER_OSPREY_ROTOR) }
    }
    fn stop_rotor(&mut self) {
        unsafe { setpiece_sfx::stop_loop(setpiece_sfx::OWNER_OSPREY_ROTOR) }
    }
    fn set_pos(&mut self, pos: [i32; 3]) {
        unsafe { prop_set_pos_exact(self.m, self.pi, pos) }
    }
    fn set_yaw(&mut self, yaw: u16) {
        unsafe { PROP_YAW[self.pi] = prop_with_yaw(PROP_YAW[self.pi], yaw) }
    }
    fn set_tilt(&mut self, tilt: u16) {
        unsafe { OSPREY_TILT = tilt }
    }
}

/// The osprey's tick.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_osprey(m: &Map, movers: &[phys::Mover]) {
    let o = osprey();
    let pi = o.pi as usize;
    let inputs = OspreyInputs {
        now: SIM_NOW,
        dead: PROP_HEALTH[pi] == 0,
        pos: PROP_POS[pi],
    };
    let mut w = World {
        m,
        movers,
        pi,
        removed: false,
    };
    o.brain.tick(&inputs, &mut w);
    if w.removed {
        o.li = u16::MAX;
    }
}
