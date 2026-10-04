//! monster_nihilanth: the game side of [`nihilanth_logic`]. This file finds
//! and binds the map's nihilanth, resolves the authored marker names
//! (`n_recharger<n>`, `n_draw<n>`, `n_leaving<n>`, `n_teleport<n>`,
//! `n_min`, `n_max`), answers the brain's queries and applies what it
//! decides.

use crate::nihilanth_logic::{NihBrain, NihInputs, NihModel, NihSeq, NihSound, NihWorld};
use crate::setpiece_math::TraceHit;
use crate::*;
use hl_format::setpiece_audio as SP;

/// nihilanth.mdl facts at 8 fps, in ticks: `float` 30 frames; `attack1`,
/// `attack2`, `recharge` and `attack1_open` 50 frames; the zap on attack1
/// frame 35, the teleport ball on attack2 frame 39; recharge's sphere
/// events on frames 10..39; the open brain (hitboxes 3-5) centred 100 ahead
/// and 277 up in float_open, taken as a 100-unit sphere.
const MODEL: NihModel = NihModel {
    float_len: 75,
    attack_len: 125,
    zap_event: 88,
    tele_event: 98,
    recharge_frames: [10, 13, 16, 21, 24, 27, 30, 33, 36, 39, 39],
    brain_ahead: 100,
    brain_up: 277,
    brain_radius: 100,
};

struct Nih {
    pi: u8,
    brain: NihBrain,
}

static mut N: Nih = Nih {
    pi: 0xff,
    brain: NihBrain::new(),
};

#[inline(always)]
unsafe fn n() -> &'static mut Nih {
    &mut *core::ptr::addr_of_mut!(N)
}

pub(crate) unsafe fn reset() {
    let g = n();
    g.pi = 0xff;
    g.brain.reset();
}

/// The logic name id of `prefix` followed by `num` (0 = none), e.g. n_recharger3.
#[inline(never)]
#[optimize(size)]
fn name_id(m: &Map, prefix: &str, num: u8) -> u16 {
    let mut id = 1;
    while id <= m.n_logic_names {
        let s = m.logic_name(id as u16).as_bytes();
        let p = prefix.as_bytes();
        if s.len() == p.len() + (num != 0) as usize
            && s.starts_with(p)
            && (num == 0 || s[p.len()] == b'0' + num)
        {
            return id as u16;
        }
        id += 1;
    }
    0
}

/// A live (not killtargeted) marker record named `id`.
#[inline(never)]
#[optimize(size)]
unsafe fn marker(m: &Map, id: u16) -> Option<usize> {
    let nlogic = m.n_logic.min(MAX_LOGIC);
    (0..nlogic).find(|&li| {
        id != 0
            && LOGIC_KIND[li] != 0
            && LOGIC_STATE[li] != LOGIC_STATE_REMOVED
            && m.logic(li).targetname == id
    })
}

/// Bind the map's nihilanth prop.
#[inline(never)]
#[optimize(size)]
unsafe fn bind(m: &Map, pi: usize) {
    let g = n();
    g.pi = pi as u8;
    let z = |name| marker(m, name_id(m, name, 0)).map(|li| m.logic(li).origin[1]);
    g.brain.bind(
        skill_table::SKILL_NIHILANTH_HEALTH[settings::skill()] as i32,
        PROP_POS[pi][1],
        prop_yaw_value(PROP_YAW[pi]),
        z("n_min"),
        z("n_max"),
        &MODEL,
        SIM_NOW,
    );
}

/// The fight starts.
#[optimize(size)]
pub(crate) unsafe fn command_on() {
    let g = n();
    if g.pi != 0xff {
        g.brain.command_on();
    }
}

/// A shot traced at the nihilanth: does it reach his brain?
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn note_shot(pi: usize, start: [i32; 3], end: [i32; 3]) {
    let g = n();
    if g.pi as usize != pi {
        return;
    }
    g.brain.note_shot(PROP_POS[pi], start, end, &MODEL);
}

struct World<'a> {
    /// None for damage, which never needs the map.
    m: Option<&'a Map>,
    movers: &'a [phys::Mover],
    pi: usize,
}

impl NihWorld for World<'_> {
    fn random_below(&mut self, n: u32) -> u32 {
        unsafe { impact_rng().below(n) }
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        let m = self.m?;
        phys::trace_line(m, self.movers, from, to).map(trace_hit)
    }
    fn prop_pos(&mut self) -> [i32; 3] {
        unsafe { PROP_POS[self.pi] }
    }
    fn set_pose(&mut self, pos: [i32; 3], yaw: u16) {
        if let Some(m) = self.m {
            unsafe {
                prop_set_pos_exact(m, self.pi, pos);
                PROP_YAW[self.pi] = prop_with_yaw(PROP_YAW[self.pi], yaw);
            }
        }
    }
    fn set_shown_health(&mut self, health: u8) {
        unsafe { PROP_HEALTH[self.pi] = health }
    }
    fn die(&mut self) {
        // The u8 health's zero fires the cooked death trigger (n_dead).
        unsafe {
            PROP_HEALTH[self.pi] = 0;
            PROP_STATE[self.pi] = PROP_STATE_DEAD;
            PROP_DEATH_START[self.pi] = SIM_NOW;
            monster_damage_ai_triggers(self.pi);
        }
    }
    fn recharger(&mut self, level: u8) -> Option<(u16, [i32; 3])> {
        let m = self.m?;
        unsafe {
            marker(m, name_id(m, "n_recharger", level)).map(|li| (li as u16, m.logic(li).origin))
        }
    }
    fn recharger_origin(&mut self, id: u16) -> [i32; 3] {
        self.m.map_or([0; 3], |m| m.logic(id as usize).origin)
    }
    fn recharger_gone(&mut self, id: u16) -> bool {
        unsafe { LOGIC_STATE[id as usize] == LOGIC_STATE_REMOVED }
    }
    fn fire_draw(&mut self, level: u8) {
        if let Some(m) = self.m {
            unsafe {
                logic_fire_targets(
                    m,
                    m.n_logic.min(MAX_LOGIC),
                    m.n_ents,
                    name_id(m, "n_draw", level),
                    map::USE_ON,
                    SIM_NOW,
                    0,
                    logic_state::CALLER_NONE,
                );
            }
        }
    }
    fn has_teleport(&mut self, n: u8) -> bool {
        self.m
            .is_some_and(|m| name_id(m, "n_teleport", n) != 0 || name_id(m, "n_leaving", n) != 0)
    }
    fn teleport_player(&mut self, n: u8) {
        let Some(m) = self.m else { return };
        unsafe {
            let nlogic = m.n_logic.min(MAX_LOGIC);
            logic_fire_targets(
                m,
                nlogic,
                m.n_ents,
                name_id(m, "n_leaving", n),
                map::USE_ON,
                SIM_NOW,
                0,
                logic_state::CALLER_NONE,
            );
            if let Some(li) = marker(m, name_id(m, "n_teleport", n)) {
                if LOGIC_KIND[li] == map::LOGIC_TRIGGER_TELEPORT {
                    logic_teleport_touch(m, nlogic, m.logic(li));
                }
            }
        }
    }
    fn hurt_player(&mut self, damage: u16, from: Option<[i32; 3]>) {
        unsafe {
            PENDING_PLAYER_DAMAGE = PENDING_PLAYER_DAMAGE.saturating_add(damage);
            if let Some(from) = from {
                note_damage_direction(from);
            }
        }
    }
    fn ball_trail(&mut self, from: [i32; 3], to: [i32; 3], teleport: bool) {
        let style = if teleport { TRACER_TELE } else { TRACER_ZAP };
        unsafe { push_tracer_styled(from, to, style) }
    }
    fn sound(&mut self, sound: NihSound, at: [i32; 3]) {
        let id = match sound {
            NihSound::Attack => SP::NIH_ATTACK,
            NihSound::Ball => SP::NIH_BALL,
            NihSound::Tele => SP::NIH_TELE,
            NihSound::Recharge => SP::NIH_RECHARGE,
            NihSound::Die => SP::NIH_DIE,
            NihSound::Laugh => SP::NIH_LAUGH,
            NihSound::Pain => SP::NIH_PAIN,
        };
        unsafe { setpiece_sfx::play(id, at) }
    }
}

/// A hit on the nihilanth. Returns the u8 health to show.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn damage(pi: usize, dmg: u8) -> u8 {
    let g = n();
    if g.pi as usize != pi || g.brain.dying() {
        return PROP_HEALTH[pi];
    }
    let mut w = World {
        m: None,
        movers: &[],
        pi,
    };
    g.brain.damage(dmg, SIM_NOW, PROP_POS[pi], &mut w)
}

/// The nihilanth's current sequence as (roster slot, ticks, elapsed).
/// Roster: 0 float, 2 attack1, 5 attack2, 6 recharge, 7 float_open, 8
/// attack1_open.
#[optimize(size)]
pub(crate) unsafe fn clip(pi: usize) -> Option<(usize, usize, usize)> {
    let g = n();
    if g.pi as usize != pi {
        return None;
    }
    let (seq, open, len, elapsed) = g.brain.sequence(SIM_NOW)?;
    let slot = match seq {
        NihSeq::Attack1 => 2,
        NihSeq::Attack2 => 5,
        NihSeq::Recharge => 6,
        NihSeq::OpenAttack => 8,
        NihSeq::Float if open => 7,
        NihSeq::Float => 0,
    };
    Some((slot, len as usize, elapsed as usize))
}

/// The nihilanth's tick.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick(m: &Map, movers: &[phys::Mover]) {
    let g = n();
    if g.pi == 0xff {
        if !g.brain.dying() {
            // First tick of the map: find the nihilanth once.
            g.brain.mark_searched();
            let np = PROP_COUNT.min(CARRY_MAILBOX_FIRST);
            if let Some(pi) = (0..np).find(|&pi| PROP_KIND[pi] == 17 && PROP_ACTIVE[pi] != 0) {
                bind(m, pi);
            }
        }
        return;
    }
    let pi = g.pi as usize;
    if PROP_ACTIVE[pi] == 0 || PROP_KIND[pi] != 17 {
        g.pi = 0xff;
        return;
    }
    let inputs = NihInputs {
        now: SIM_NOW,
        player_pos: LOGIC_PLAYER_POS,
        player_alive: LOGIC_PLAYER_HEALTH > 0,
        shown_alive: PROP_HEALTH[pi] != 0,
        zap_damage: skill_table::SKILL_NIHILANTH_ZAP[settings::skill()],
    };
    let mut w = World {
        m: Some(m),
        movers,
        pi,
    };
    g.brain.tick(&inputs, &MODEL, &mut w);
}

/// The spheres circling him, for the renderer: count and centre.
pub(crate) unsafe fn spheres() -> Option<(u8, [i32; 3])> {
    let g = n();
    if g.pi == 0xff {
        return None;
    }
    g.brain.spheres(PROP_POS[g.pi as usize])
}
