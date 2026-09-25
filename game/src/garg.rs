//! CGargantua (gargantua.cpp): chase, swipe, flame sweep and stomp, and its
//! death. The campaign places at most one gargantua per map (c2a1, c2a5g,
//! c4a1b, c4a3), so its schedule timers live here, not in per-actor arrays.

use crate::setpiece_logic::{self as sl, GargChoice, Stomp};
use crate::*;
use hl_format::setpiece_audio as SP;

/// garg.mdl `run`: 395.5 units over 20 frames at 18 fps (356 u/s), per tick.
const RUN_PER_TICK: i32 = 18;

/// m_painSoundTime: the next tick a hurt gargantua may cry out.
static mut PAIN_NEXT: u16 = 0;

/// CGargantua::TraceAttack: a pain cry at most every 2.5 to 4 s.
#[optimize(size)]
pub(crate) unsafe fn pain(pi: usize) {
    if time_reached(SIM_NOW, PAIN_NEXT) {
        PAIN_NEXT = SIM_NOW.wrapping_add(50 + IMPACT_RNG.below(31) as u16);
        setpiece_sfx::play(SP::GARG_PAIN, PROP_POS[pi]);
    }
}

const FLAME_LENGTH: i32 = 330;
/// Forearm flame attachments 2 and 3 in shootflames2 (forward, left, up).
const FLAME_ATTACH: [[i32; 3]; 2] = [[111, -66, 86], [110, 64, 88]];

struct Garg {
    pi: u8,
    /// 0 none, 1 swipe (`Attack`), 2 stomp: the attack sequence playing.
    gesture: u8,
    burn_tenths: u8,
    feet: u8,
    stomp_on: bool,
    stomp_first: bool,
    removed: bool,
    /// m_seeTime: a stomp needs the enemy in sight continuously until then.
    see: u16,
    /// m_flameTime: no new flame sweep before this tick.
    flame_next: u16,
    /// m_flWaitFinished of a running TASK_FLAME_SWEEP, 0 when not flaming.
    flame_end: u16,
    gesture_start: u16,
    stomp_tick: u16,
    /// Tick of death, for DeathEffect's timeline; 0xffff while alive.
    died: u16,
    /// m_flameX / m_flameY, q12 turns off the body angles.
    flame_ang: [i32; 2],
    stomp_pos: [i32; 3], // world x16
    stomp_dir: [i32; 3], // q12
    stomp: Stomp,
}

static mut GARG: Garg = Garg {
    pi: 0xff,
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
};

#[inline(always)]
unsafe fn g() -> &'static mut Garg {
    &mut *core::ptr::addr_of_mut!(GARG)
}

/// Forget the previous map's gargantua.
pub(crate) unsafe fn reset() {
    let g = g();
    g.pi = 0xff;
    g.stomp_on = false;
    g.died = 0xffff;
    g.removed = false;
    g.gesture = 0;
    g.flame_end = 0;
}

/// Unit vector of a prop yaw (0 = +Z, 1024 = +X), q12.
#[inline(never)]
#[optimize(size)]
fn forward(yaw: u16) -> [i32; 3] {
    [sincos::sin_q12(yaw & 0xfff), 0, sincos::sin_q12(yaw.wrapping_add(1024) & 0xfff)]
}

/// `o + dir * len` for a q12 direction.
#[inline(never)]
#[optimize(size)]
fn along(o: [i32; 3], dir: [i32; 3], len: i32) -> [i32; 3] {
    [o[0] + ((dir[0] * len) >> 12), o[1] + ((dir[1] * len) >> 12), o[2] + ((dir[2] * len) >> 12)]
}

/// Where `from -> to` first meets the player's box padded by `pad`
/// (x/z, and y), as a q12 fraction; None when it misses or they are dead.
#[inline(never)]
#[optimize(size)]
unsafe fn player_frac(from: [i32; 3], to: [i32; 3], pad: i32) -> Option<i32> {
    let p = LOGIC_PLAYER_POS;
    let (w, h) = (16 + pad, LOGIC_PLAYER_HALF_HEIGHT + pad);
    if LOGIC_PLAYER_HEALTH == 0 {
        return None;
    }
    segment_box_frac(from, to, [p[0] - w, p[1] - h, p[2] - w], [p[0] + w, p[1] + h, p[2] + w])
}

#[inline(never)]
#[optimize(size)]
unsafe fn hurt_player(dmg: u16, from: [i32; 3]) {
    PENDING_PLAYER_DAMAGE = PENDING_PLAYER_DAMAGE.saturating_add(dmg);
    note_damage_direction(from);
}

/// The garg's attack sequence: (roster slot, one-shot length, elapsed).
/// Slots 5 and 6 are the type-16 roster's `attack` and `stomp`.
#[optimize(size)]
pub(crate) unsafe fn gesture_clip(pi: usize) -> Option<(usize, usize, usize)> {
    let g = g();
    if g.pi as usize != pi || g.gesture == 0 {
        return None;
    }
    let len = if g.gesture == 1 { sl::GARG_SWIPE_TICKS } else { sl::GARG_STOMP_TICKS };
    Some((4 + g.gesture as usize, len as usize, SIM_NOW.wrapping_sub(g.gesture_start) as usize))
}

/// UTIL_ScreenShake as the player feels it, keeping a stronger shake.
#[inline(never)]
#[optimize(size)]
unsafe fn shake(from: [i32; 3], amplitude: i32, ticks: u16, radius: i32) {
    let amp = sl::shake_amplitude(amplitude, isqrt_i32(dist2_3(from, LOGIC_PLAYER_POS)), radius);
    if amp > 0 && amp * SHAKE_DUR.max(1) as i32 >= SHAKE_AMP as i32 * SHAKE_TICKS as i32 {
        SHAKE_AMP = amp as u16;
        SHAKE_DUR = ticks;
        SHAKE_TICKS = ticks;
    }
}

/// CGargantua's schedules, once per tick while the actor loop has it awake.
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
        // CGargantua::Spawn: m_seeTime = now + 5 s, m_flameTime = now + 2 s.
        reset();
        g.pi = pi as u8;
        g.see = now.wrapping_add(100);
        g.flame_next = now.wrapping_add(40);
    }
    let (target, visible) = if ai_reacquire(pi) {
        let selected = retained_actor_target(m, sight_movers, pi, player_pos, nprops)
            .unwrap_or_else(|| find_actor_target(m, sight_movers, pi, player_pos, nprops, 2048 * 2048));
        prop_ai_set_target_visible(pi, selected.1);
        selected
    } else {
        (PROP_AI_TARGET[pi], prop_ai_target_visible(pi))
    };
    // PrescheduleThink: m_seeTime restarts whenever the enemy is unseen.
    if !visible {
        g.see = now.wrapping_add(100);
    }
    let Some(aim) = target_aim_point(target, player_pos, nprops) else {
        if g.gesture == 0 {
            PROP_STATE[pi] = PROP_STATE_IDLE;
            PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
        }
        g.flame_end = 0;
        return;
    };
    PROP_AI_TARGET[pi] = target;
    let pos = PROP_POS[pi];
    let yaw = prop_yaw_value(PROP_YAW[pi]);
    let f = forward(yaw);
    if g.flame_end != 0 {
        flame_task(m, movers, pi, aim, now);
        return;
    }
    if g.gesture != 0 {
        let t = now.wrapping_sub(g.gesture_start);
        let (event, len) = if g.gesture == 1 {
            (sl::GARG_SWIPE_EVENT_TICKS, sl::GARG_SWIPE_TICKS)
        } else {
            (sl::GARG_STOMP_EVENT_TICKS, sl::GARG_STOMP_TICKS)
        };
        if t < event as u16 {
            prop_face_point(pi, aim); // TASK_FACE_ENEMY
        } else if t == event as u16 {
            if g.gesture == 1 {
                swipe(pi, target, f);
            } else {
                stomp_attack(m, movers, pi, aim, f);
                g.see = now.wrapping_add(240); // m_seeTime = now + 12 s
            }
        } else if t >= len as u16 {
            g.gesture = 0;
            PROP_STATE[pi] = PROP_STATE_IDLE;
        }
        return;
    }
    // CheckAttacks: 2D facing cosine, origin-to-origin distance.
    let enemy = if target == PROP_TARGET_PLAYER { player_pos } else { PROP_POS[target as usize] };
    let (dx, dz) = (enemy[0] - pos[0], enemy[2] - pos[2]);
    let dot = (f[0] * dx + f[2] * dz) / isqrt_i32(dx * dx + dz * dz).max(1);
    let dist = isqrt_i32(dist2_3(enemy, pos));
    let choice = if visible {
        sl::garg_choice(dot, dist, time_reached(now, g.see), time_reached(now, g.flame_next))
    } else {
        GargChoice::Chase
    };
    PROP_STATE[pi] = PROP_STATE_ATTACK;
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
            // TASK_FLAME_SWEEP 4.5 s; m_flameTime = now + 6 s.
            g.flame_end = now.wrapping_add(90).max(1);
            g.flame_next = now.wrapping_add(120);
            g.flame_ang = [0; 2];
            // FlameCreate: pBeamAttackSounds[1] on the body, [2] on the weapon.
            setpiece_sfx::play(SP::GARG_FLAME_ON, pos);
        }
        GargChoice::Chase => {
            PROP_STATE[pi] = PROP_STATE_MOVE;
            prop_face_point(pi, aim);
            prop_move_towards_point(m, movers, pi, aim, RUN_PER_TICK);
            // GARG_AE_LEFT/RIGHT_FOOT, twice per run cycle (frames 9 and 19
            // at 18 fps): UTIL_ScreenShake(4, 1 s, radius 750).
            g.feet = g.feet.wrapping_add(1);
            if g.feet % 11 == 0 {
                shake(pos, 4, 20, 750);
                setpiece_sfx::play(SP::GARG_STEP, pos);
            }
        }
    }
}

/// GARG_AE_SLASH_LEFT: GargantuaCheckTraceHullAttack(90) sweeps a head hull
/// from 64 up to 90 ahead and 27 down; what it meets takes the slash.
#[inline(never)]
#[optimize(size)]
unsafe fn swipe(pi: usize, target: u8, f: [i32; 3]) {
    let pos = PROP_POS[pi];
    let from = [pos[0], pos[1] + 64, pos[2]];
    let mut to = along(from, f, 90);
    to[1] -= 27;
    let dmg = skill_table::SKILL_GARG_SLASH[settings::skill()];
    if player_frac(from, to, 16).is_some() {
        hurt_player(dmg, pos);
        add_view_punch(-341, -341); // punchangle (-30, -30, 30)
    } else if target != PROP_TARGET_PLAYER {
        let (mn, mx) = actor_collision_bounds(PROP_KIND[target as usize], PROP_POS[target as usize]);
        if segment_box_frac(from, to, [mn[0] - 16, mn[1] - 18, mn[2] - 16], [mx[0] + 16, mx[1] + 18, mx[2] + 16]).is_some() {
            damage_prop(target as usize, dmg.min(255) as u8, false);
        }
    }
}

/// CGargantua::StompAttack: aim from 60 up and 35 ahead at the enemy, trace
/// 1024 (ignoring monsters) and send a CStomp that far.
#[inline(never)]
#[optimize(size)]
unsafe fn stomp_attack(m: &Map, movers: &[phys::Mover], pi: usize, aim: [i32; 3], f: [i32; 3]) {
    let g = g();
    let pos = PROP_POS[pi];
    let mut start = along(pos, f, 35);
    start[1] += 60;
    // ShootAtEnemy aims at CBaseMonster::BodyTarget: a random point from
    // the player's centre up to their eyes.
    let aim = if PROP_AI_TARGET[pi] == PROP_TARGET_PLAYER {
        let p = LOGIC_PLAYER_POS;
        [p[0], p[1] + IMPACT_RNG.below(VIEW_HEIGHT as u32 + 1) as i32, p[2]]
    } else {
        aim
    };
    let dir = dir_q12(start, aim);
    let far = along(start, dir, 1024);
    let end = phys::trace_line(m, movers, start, far).map_or(far, |h| h.pos);
    g.stomp = Stomp::new(isqrt_i32(dist2_3(start, end)));
    g.stomp_on = true;
    g.stomp_first = true;
    g.stomp_tick = SIM_NOW;
    g.stomp_pos = [start[0] << 4, start[1] << 4, start[2] << 4];
    g.stomp_dir = dir;
    shake(pos, 12, 40, 1000); // UTIL_ScreenShake(12, 2 s, 1000)
    if setpiece_sfx::has(SP::GARG_STOMP) {
        setpiece_sfx::play(SP::GARG_STOMP, pos);
    } else {
        sfx::play_world(sfx::EXPLODE, pos);
    }
}

/// TASK_FLAME_SWEEP: FlameControls and FlameUpdate on every 10 Hz think.
#[inline(never)]
#[optimize(size)]
unsafe fn flame_task(m: &Map, movers: &[phys::Mover], pi: usize, aim: [i32; 3], now: u16) {
    let g = g();
    if time_reached(now, g.flame_end) {
        g.flame_end = 0; // FlameDestroy; ACT_IDLE
        PROP_STATE[pi] = PROP_STATE_IDLE;
        setpiece_sfx::stop_loop(setpiece_sfx::OWNER_GARG_FLAME);
        setpiece_sfx::play(SP::GARG_FLAME_OFF, PROP_POS[pi]);
        return;
    }
    setpiece_sfx::keep_loop(SP::GARG_FLAME, PROP_POS[pi], setpiece_sfx::OWNER_GARG_FLAME);
    if now.wrapping_sub(g.flame_end) & 1 != 0 {
        return;
    }
    let pos = PROP_POS[pi];
    let yaw = prop_yaw_value(PROP_YAW[pi]) as i32;
    // Angles from origin + 64 to the enemy, relative to the body.
    let d = [aim[0] - pos[0], aim[1] - pos[1] - 64, aim[2] - pos[2]];
    let horiz = isqrt_i32(d[0] * d[0] + d[2] * d[2]);
    let want = [
        crate::tank::atan_s(d[1], horiz),
        sl::angle_dist_q12(crate::tank::atan_s(d[0], d[2]), yaw),
    ];
    if horiz.max(d[1].abs()) > 400 || want[1].abs() > 683 {
        // Beyond 400 units or 60 degrees aside, the sweep winds down 6x.
        g.flame_end = g.flame_end.wrapping_sub(10);
        g.flame_next = g.flame_next.wrapping_sub(10);
    }
    // FlameControls: yaw within 45 degrees, approached 8 (yaw) and 4
    // (pitch) degrees per think.
    g.flame_ang[0] = sl::approach_angle_q12(want[0], g.flame_ang[0], 46);
    g.flame_ang[1] = sl::approach_angle_q12(want[1].clamp(-512, 512), g.flame_ang[1], 91);
    let p = (g.flame_ang[0] & 0xfff) as u16;
    let (sp, cp) = (sincos::sin_q12(p), sincos::sin_q12(p.wrapping_add(1024) & 0xfff));
    let fa = forward(((yaw + g.flame_ang[1]) & 0xfff) as u16);
    let dir = [(fa[0] * cp) >> 12, sp, (fa[2] * cp) >> 12];
    let f = forward(yaw as u16);
    let fire = skill_damage(PROP_TYPE_GARG).unwrap_or(3) as i32;
    let pp = LOGIC_PLAYER_POS;
    let spot = [pp[0], pp[1] + VIEW_HEIGHT / 2, pp[2]];
    for a in FLAME_ATTACH {
        // Attachment in the world: forward, left (= -right), up.
        let start = [
            pos[0] + ((f[0] * a[0] - f[2] * a[1]) >> 12),
            pos[1] + a[2],
            pos[2] + ((f[2] * a[0] + f[0] * a[1]) >> 12),
        ];
        // dont_ignore_monsters: the flame stops at the world or the player.
        let far = along(start, dir, FLAME_LENGTH);
        let mut frac = phys::trace_line(m, movers, start, far).map_or(4096, |h| h.frac);
        if let Some(fr) = player_frac(start, far, 0) {
            frac = frac.min(fr);
        }
        let len = (FLAME_LENGTH * frac) >> 12;
        push_tracer_styled(start, along(start, dir, len), TRACER_FLAME);
        push_tracer_styled(start, along(start, dir, len * 2 / 5), TRACER_FLAME_CORE);
        // FlameDamage: full damage within 64 units of the flame's nearest
        // point to the player's body, 0.4 less per unit beyond.
        let t = (((spot[0] - start[0]) * dir[0] + (spot[1] - start[1]) * dir[1] + (spot[2] - start[2]) * dir[2]) >> 12).clamp(0, len);
        let src = along(start, dir, t);
        if LOGIC_PLAYER_HEALTH > 0 && phys::trace_line(m, movers, src, spot).is_none() {
            if let Some(tenths) = sl::flame_damage_tenths(fire, isqrt_i32(dist2_3(src, spot))) {
                let total = g.burn_tenths as i32 + tenths;
                hurt_player((total / 10) as u16, start);
                g.burn_tenths = (total % 10) as u8;
            }
        }
    }
}

/// Per-tick work that outlives the actor loop's view of the gargantua: the
/// travelling stomp wave and the timed DeathEffect.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_world(m: &Map) {
    let g = g();
    if g.pi == 0xff {
        return;
    }
    let now = SIM_NOW;
    if g.stomp_on && now.wrapping_sub(g.stomp_tick) & 1 == 0 {
        // CStomp::Think: this think's head-hull sweep, then advance.
        let from = [g.stomp_pos[0] >> 4, (g.stomp_pos[1] >> 4) + 30, g.stomp_pos[2] >> 4];
        if player_frac(from, along(from, g.stomp_dir, g.stomp.sweep_q4() >> 4), 16).is_some() {
            hurt_player(skill_table::SKILL_GARG_STOMP[settings::skill()], from);
        }
        let (moved, done) = g.stomp.think(g.stomp_first);
        g.stomp_first = false;
        g.stomp_pos = along(g.stomp_pos, g.stomp_dir, moved);
        if moved > 0 {
            // The sprite trail lands on the floor under the wave.
            spark_burst([from[0], from[1] - 86, from[2]], 3, 4, 6, 6);
        }
        g.stomp_on = !done;
    }
    let pi = g.pi as usize;
    if PROP_KIND[pi] != PROP_TYPE_GARG {
        return;
    }
    if g.died == 0xffff {
        if PROP_ACTIVE[pi] != 0 && PROP_HEALTH[pi] == 0 {
            g.died = now;
            g.flame_end = 0;
            g.gesture = 0;
            setpiece_sfx::stop_loop(setpiece_sfx::OWNER_GARG_FLAME);
        }
        return;
    }
    // DeathEffect: four no-damage explosions 0.6 s apart rising from 32 up,
    // magnitudes 60..180 within 70 units; TASK_DIE gibs the gargantua 1.6 s
    // in (kRenderFxExplode, ten gibs and a flesh BREAKMODEL).
    let t = now.wrapping_sub(g.died);
    if t % 12 == 0 && t <= 36 {
        let i = (t / 12) as i32;
        let o = PROP_POS[pi];
        let r = |v: i32| v + IMPACT_RNG.below(141) as i32 - 70;
        queue_explosion_fx([r(o[0]), o[1] + 32 + 15 * i, r(o[2])], (60 + 40 * i) as u8);
        sfx::play_world(sfx::EXPLODE, o);
    }
    if t == 32 && !g.removed {
        g.removed = true;
        spawn_gibs(PROP_POS[pi], 16);
        PROP_ACTIVE[pi] = 0;
    }
}

/// CGargantua::TraceAttack / TakeDamage: only GARG_DAMAGE (blast, energy
/// beam, crush, mortar) hurts; bullets and clubs ricochet. The u8 actor
/// health stands for the skill.cfg health, so damage scales onto it.
#[optimize(size)]
pub(crate) fn scale_damage(dmg: u8, heavy: bool) -> u8 {
    if !heavy {
        return 0;
    }
    let full = skill_table::SKILL_GARG_HEALTH[settings::skill()].max(1) as u32;
    ((dmg as u32 * 255 + full / 2) / full).max(1) as u8
}
