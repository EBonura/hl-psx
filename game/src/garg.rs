//! monster_gargantua: the game side of [`garg_logic`]. The campaign places
//! at most one gargantua per map (c2a1, c2a5g, c4a1b, c4a3). This file runs
//! the shared actor targeting, answers the brain's queries and applies what
//! it decides.

use crate::garg_logic::{
    self, GargBrain, GargInputs, GargModel, GargSound, GargState, GargWorld, GargWorldInputs,
};
use crate::setpiece_logic as sl;
use crate::setpiece_math::TraceHit;
use crate::*;
use hl_format::setpiece_audio as SP;

/// garg.mdl facts: `run` covers 395.5 units over 20 frames at 18 fps (356
/// u/s, 18 a tick); the forearm flame attachments 2 and 3 in shootflames2
/// (forward, left, up); the attack and stomp event and sequence timings.
const MODEL: GargModel = GargModel {
    run_per_tick: 18,
    flame_attach: [[111, -66, 86], [110, 64, 88]],
    swipe_event: sl::GARG_SWIPE_EVENT_TICKS,
    swipe_len: sl::GARG_SWIPE_TICKS,
    stomp_event: sl::GARG_STOMP_EVENT_TICKS,
    stomp_len: sl::GARG_STOMP_TICKS,
};

struct Garg {
    pi: u8,
    brain: GargBrain,
}

static mut GARG: Garg = Garg {
    pi: 0xff,
    brain: GargBrain::new(),
};

#[inline(always)]
unsafe fn g() -> &'static mut Garg {
    &mut *core::ptr::addr_of_mut!(GARG)
}

/// Forget the previous map's gargantua.
pub(crate) unsafe fn reset() {
    let g = g();
    g.pi = 0xff;
    // A deadline on the previous attempt's clock would mute his pain cries
    // until the restarted clock caught up.
    g.brain.reset();
}

struct World<'a> {
    /// None for the pain and world ticks, which never trace or walk.
    m: Option<&'a Map>,
    movers: &'a [phys::Mover],
    pi: usize,
    target: u8,
}

impl GargWorld for World<'_> {
    fn random_below(&mut self, n: u32) -> u32 {
        unsafe { impact_rng().below(n) }
    }
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
        let m = self.m?;
        phys::trace_line(m, self.movers, from, to).map(trace_hit)
    }
    fn player_box_frac(&mut self, from: [i32; 3], to: [i32; 3], pad: i32) -> Option<i32> {
        unsafe {
            let p = LOGIC_PLAYER_POS;
            let (w, h) = (16 + pad, LOGIC_PLAYER_HALF_HEIGHT + pad);
            if LOGIC_PLAYER_HEALTH == 0 {
                return None;
            }
            segment_box_frac(
                from,
                to,
                [p[0] - w, p[1] - h, p[2] - w],
                [p[0] + w, p[1] + h, p[2] + w],
            )
        }
    }
    fn target_box_hit(&mut self, from: [i32; 3], to: [i32; 3]) -> bool {
        unsafe {
            let t = self.target as usize;
            let (mn, mx) = actor_collision_bounds(PROP_KIND[t], PROP_POS[t]);
            segment_box_frac(
                from,
                to,
                [mn[0] - 16, mn[1] - 18, mn[2] - 16],
                [mx[0] + 16, mx[1] + 18, mx[2] + 16],
            )
            .is_some()
        }
    }
    fn set_state(&mut self, state: GargState) {
        unsafe {
            PROP_STATE[self.pi] = match state {
                GargState::Idle => PROP_STATE_IDLE,
                GargState::Attack => PROP_STATE_ATTACK,
                GargState::Move => PROP_STATE_MOVE,
            }
        }
    }
    fn keep_target(&mut self) {
        unsafe { PROP_AI_TARGET[self.pi] = self.target }
    }
    fn clear_target(&mut self) {
        unsafe { PROP_AI_TARGET[self.pi] = PROP_TARGET_NONE }
    }
    fn face(&mut self, point: [i32; 3]) {
        unsafe { prop_face_point(self.pi, point) }
    }
    fn run_towards(&mut self, point: [i32; 3], step: i32) {
        if let Some(m) = self.m {
            unsafe {
                prop_move_towards_point(m, self.movers, self.pi, point, step);
            }
        }
    }
    fn hurt_player(&mut self, damage: u16, from: [i32; 3]) {
        unsafe {
            PENDING_PLAYER_DAMAGE = PENDING_PLAYER_DAMAGE.saturating_add(damage);
            note_damage_direction(from);
        }
    }
    fn view_punch(&mut self, pitch: i32, yaw: i32) {
        unsafe { add_view_punch(pitch, yaw) }
    }
    fn damage_target(&mut self, damage: u8) {
        unsafe { damage_prop(self.target as usize, damage, false) }
    }
    fn shake(&mut self, amplitude: i32, ticks: u16) {
        // Keep a stronger running shake.
        unsafe {
            if amplitude * SHAKE_DUR.max(1) as i32 >= SHAKE_AMP as i32 * SHAKE_TICKS as i32 {
                SHAKE_AMP = amplitude as u16;
                SHAKE_DUR = ticks;
                SHAKE_TICKS = ticks;
            }
        }
    }
    fn sound(&mut self, sound: GargSound, at: [i32; 3]) {
        unsafe {
            match sound {
                GargSound::Pain => setpiece_sfx::play(SP::GARG_PAIN, at),
                GargSound::FlameOn => setpiece_sfx::play(SP::GARG_FLAME_ON, at),
                GargSound::FlameOff => setpiece_sfx::play(SP::GARG_FLAME_OFF, at),
                GargSound::Step => setpiece_sfx::play(SP::GARG_STEP, at),
                GargSound::Stomp if setpiece_sfx::has(SP::GARG_STOMP) => {
                    setpiece_sfx::play(SP::GARG_STOMP, at)
                }
                GargSound::Stomp | GargSound::Explosion => sfx::play_world(sfx::EXPLODE, at),
            }
        }
    }
    fn flame_loop(&mut self, at: [i32; 3]) {
        unsafe { setpiece_sfx::keep_loop(SP::GARG_FLAME, at, setpiece_sfx::OWNER_GARG_FLAME) }
    }
    fn flame_loop_mute(&mut self) {
        unsafe { setpiece_sfx::mute_loop(setpiece_sfx::OWNER_GARG_FLAME) }
    }
    fn flame_loop_stop(&mut self) {
        unsafe { setpiece_sfx::stop_loop(setpiece_sfx::OWNER_GARG_FLAME) }
    }
    fn flame_beam(&mut self, from: [i32; 3], to: [i32; 3], core: bool) {
        let style = if core {
            TRACER_FLAME_CORE
        } else {
            TRACER_FLAME
        };
        unsafe { push_tracer_styled(from, to, style) }
    }
    fn stomp_dust(&mut self, at: [i32; 3]) {
        unsafe { spark_burst(at, 3, 4, 6, 6) }
    }
    fn explosion_fx(&mut self, at: [i32; 3], mag: u8) {
        unsafe { queue_explosion_fx(at, mag) }
    }
    fn gib(&mut self, at: [i32; 3]) {
        unsafe {
            spawn_gibs(at, 16);
            PROP_ACTIVE[self.pi] = 0;
        }
    }
}

/// A hit landed on the gargantua: maybe a pain cry.
#[optimize(size)]
pub(crate) unsafe fn pain(pi: usize) {
    let g = g();
    let mut w = World {
        m: None,
        movers: &[],
        pi,
        target: PROP_TARGET_NONE,
    };
    g.brain.pain(SIM_NOW, PROP_POS[pi], &mut w);
}

/// The gargantua's attack sequence: (roster slot, one-shot length, elapsed).
/// Slots 5 and 6 are the type-16 roster's `attack` and `stomp`.
#[optimize(size)]
pub(crate) unsafe fn gesture_clip(pi: usize) -> Option<(usize, usize, usize)> {
    let g = g();
    if g.pi as usize != pi {
        return None;
    }
    let (gesture, len, elapsed) = g.brain.gesture(SIM_NOW, &MODEL)?;
    Some((4 + gesture as usize, len as usize, elapsed as usize))
}

/// The gargantua's schedules, once per tick while the actor loop has it
/// awake.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick(
    m: &Map,
    movers: &[phys::Mover],
    sight_movers: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    nprops: usize,
) {
    let now = SIM_NOW;
    let g = g();
    if g.pi as usize != pi {
        g.pi = pi as u8;
        g.brain.spawn(now);
    }
    let (target, visible) = if ai_reacquire(pi) {
        let selected = retained_actor_target(m, sight_movers, pi, player_pos, nprops)
            .unwrap_or_else(|| {
                find_actor_target(m, sight_movers, pi, player_pos, nprops, 2048 * 2048)
            });
        prop_ai_set_target_visible(pi, selected.1);
        selected
    } else {
        (PROP_AI_TARGET[pi], prop_ai_target_visible(pi))
    };
    let aim = target_aim_point(target, player_pos, nprops);
    let target_is_player = target == PROP_TARGET_PLAYER;
    let inputs = GargInputs {
        now,
        visible,
        aim,
        target_is_player,
        enemy_pos: if target_is_player || aim.is_none() {
            player_pos
        } else {
            PROP_POS[target as usize]
        },
        pos: PROP_POS[pi],
        yaw: prop_yaw_value(PROP_YAW[pi]),
        player_pos: LOGIC_PLAYER_POS,
        player_alive: LOGIC_PLAYER_HEALTH > 0,
        view_height: VIEW_HEIGHT,
        slash_damage: skill_table::SKILL_GARG_SLASH[settings::skill()],
        flame_damage: skill_damage(PROP_TYPE_GARG).unwrap_or(3) as i32,
    };
    let mut w = World {
        m: Some(m),
        movers,
        pi,
        target,
    };
    g.brain.think(&inputs, &MODEL, &mut w);
}

/// Per-tick work that outlives the actor loop's view of the gargantua: the
/// travelling stomp wave and the death timeline.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_world() {
    let g = g();
    if g.pi == 0xff {
        return;
    }
    let pi = g.pi as usize;
    let inputs = GargWorldInputs {
        now: SIM_NOW,
        present: PROP_KIND[pi] == PROP_TYPE_GARG,
        active: PROP_ACTIVE[pi] != 0,
        dead: PROP_HEALTH[pi] == 0,
        pos: PROP_POS[pi],
        player_pos: LOGIC_PLAYER_POS,
        stomp_damage: skill_table::SKILL_GARG_STOMP[settings::skill()],
    };
    let mut w = World {
        m: None,
        movers: &[],
        pi,
        target: PROP_TARGET_NONE,
    };
    g.brain.tick_world(&inputs, &mut w);
}

/// A hit on the gargantua scaled onto its u8 actor health.
#[optimize(size)]
pub(crate) fn scale_damage(dmg: u8, heavy: bool) -> u8 {
    let full = skill_table::SKILL_GARG_HEALTH[settings::skill()] as u32;
    garg_logic::scale_damage(dmg, heavy, full)
}
