//! CNihilanth (nihilanth.cpp): the sphere-shielded, crystal-recharged final
//! boss. Health lives here in skill.cfg units; the u8 actor health only
//! mirrors it for the HUD and the death trigger.
//!
//! * StartupThink: 20 energy spheres circle him. HuntThink absorbs one
//!   sphere per think while he is hurt (+1/20 of his health each).
//! * NextActivity: below half health or half his spheres he flies to the
//!   level's n_recharger (the crystal) and runs `recharge`, which emits a
//!   sphere per event 4 and fires n_draw<level>; a destroyed crystal kills
//!   its recharger, and once levels 1-9 are gone he opens his head
//!   (m_irritation 2).
//! * Attacks while angry and close: `attack1` (event 2: a second of zap
//!   ball pairs), `attack2` (event 3: a teleport ball that sends the player
//!   through n_leaving<n> / n_teleport<n>), `attack1_open` (event 6).
//! * TakeDamage floors him at 1 unless the last trace hit his open head
//!   (hitboxes 3-5, hitgroup 2) while m_irritation is 2.
//! * Flight holds m_posDesired.z = the player's height + m_flAdj inside
//!   n_min / n_max and yaws toward the player.

use crate::*;
use hl_format::setpiece_audio as SP;

const N_SPHERES: u8 = 20;
/// Sequence timing in ticks at framerate 1: frames / 8 fps.
const SEQ_FLOAT: u16 = 75; // float / float_open: 30 frames
const SEQ_ATTACK: u16 = 125; // attack1, attack2, recharge, attack1_open: 50 frames
const EV_ZAP: u16 = 88; // attack1 frame 35
const EV_TELE: u16 = 98; // attack2 frame 39
/// `recharge` event 4 frames: 10, 13, ..., 39.
const RECHARGE_EVENTS: [u8; 11] = [10, 13, 16, 21, 24, 27, 30, 33, 36, 39, 39];

const SEQ_FLOAT_ID: u8 = 0;
const SEQ_ATTACK1: u8 = 1;
const SEQ_ATTACK2: u8 = 2;
const SEQ_RECHARGE: u8 = 3;
const SEQ_OPEN_ATTACK: u8 = 4;

const MAX_BALLS: usize = 6;

struct Nih {
    pi: u8,
    irritation: u8,
    level: u8,
    teleport: u8,
    spheres: u8,
    seq: u8,
    dead: bool,
    head_hit: bool,
    health: i32,
    full: i32,
    z: i32,  // x16
    vz: i32, // x16 per second
    force: i32,
    yaw: i32,  // runtime yaw, q12 << 4
    avel: i32, // q12 << 4 per second
    adj: i32,
    min_z: i32,
    max_z: i32,
    desired_z: i32,
    desired: [i32; 2], // q12 xz direction to face
    recharger: u16,
    seq_start: u16,
    seq_len: u16,
    last_seen: u16,
    shoot_end: u16,
    /// Energy balls: position, velocity (units per tick), 1 zap / 2 teleport.
    ball_pos: [[i32; 3]; MAX_BALLS],
    ball_vel: [[i32; 3]; MAX_BALLS],
    ball_kind: [u8; MAX_BALLS],
}

static mut N: Nih = Nih {
    pi: 0xff,
    irritation: 0,
    level: 1,
    teleport: 1,
    spheres: 0,
    seq: 0,
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
};

/// m_flNextPainSound.
static mut PAIN_NEXT: u16 = 0;

/// Event 2 (zen): the ball launch, with an attack cry one time in five.
#[inline(never)]
#[optimize(size)]
unsafe fn zen_sound(head: [i32; 3]) {
    if IMPACT_RNG.below(5) == 0 {
        setpiece_sfx::play(SP::NIH_ATTACK, head);
    }
    setpiece_sfx::play(SP::NIH_BALL, head);
}

#[inline(always)]
unsafe fn n() -> &'static mut Nih {
    &mut *core::ptr::addr_of_mut!(N)
}

pub(crate) unsafe fn reset() {
    N.pi = 0xff;
    N.dead = false; // doubles as "not searched yet" while unbound
    N.ball_kind = [0; MAX_BALLS];
}

/// The logic name id of `prefix` followed by `num` (0 = none), e.g. n_recharger3.
#[inline(never)]
#[optimize(size)]
fn name_id(m: &Map, prefix: &str, num: u8) -> u16 {
    let mut id = 1;
    while id <= m.n_logic_names {
        let s = m.logic_name(id as u16).as_bytes();
        let p = prefix.as_bytes();
        if s.len() == p.len() + (num != 0) as usize && s.starts_with(p) && (num == 0 || s[p.len()] == b'0' + num) {
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
        id != 0 && LOGIC_KIND[li] != 0 && LOGIC_STATE[li] != LOGIC_STATE_REMOVED && m.logic(li).targetname == id
    })
}

/// CNihilanth::Spawn + StartupThink for the map's nihilanth prop.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn bind(m: &Map, pi: usize) {
    let g = n();
    g.pi = pi as u8;
    g.irritation = 0;
    g.level = 1;
    g.teleport = 1;
    g.spheres = N_SPHERES;
    g.dead = false;
    g.full = skill_table::SKILL_NIHILANTH_HEALTH[settings::skill()] as i32;
    g.health = g.full;
    g.z = PROP_POS[pi][1] * 16;
    g.vz = 0;
    g.force = 0;
    g.yaw = (prop_yaw_value(PROP_YAW[pi]) as i32) << 4;
    g.avel = 0;
    g.adj = 512;
    g.recharger = u16::MAX;
    g.seq = SEQ_FLOAT_ID;
    g.seq_start = SIM_NOW;
    g.seq_len = SEQ_FLOAT;
    g.last_seen = SIM_NOW.wrapping_sub(2000);
    // The simulation clock restarts with the room, so a volley or pain
    // deadline left from the previous attempt would lie ahead of it: after a
    // death in the fight he fired from the first tick of the retry and
    // killed the player on arrival, every time.
    g.shoot_end = SIM_NOW;
    PAIN_NEXT = SIM_NOW;
    g.desired_z = 512;
    let z = |name| marker(m, name_id(m, name, 0)).map(|li| m.logic(li).origin[1]);
    g.min_z = z("n_min").unwrap_or(-4096);
    g.max_z = z("n_max").unwrap_or(4096);
}

/// CNihilanth::CommandUse(USE_ON): the fight starts.
#[optimize(size)]
pub(crate) unsafe fn command_on() {
    if N.pi != 0xff && N.irritation == 0 {
        N.irritation = 1;
    }
}

/// A shot traced at the nihilanth: does it reach his brain, hitgroup 2
/// (hitboxes 3-5, centred 100 ahead and 277 up in float_open)? Tested as
/// the segment passing within 100 units of that centre, a sphere standing
/// in for the posed boxes.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn note_shot(pi: usize, start: [i32; 3], end: [i32; 3]) {
    if N.pi as usize != pi {
        return;
    }
    let o = PROP_POS[pi];
    let yaw = ((N.yaw >> 4) & 0xfff) as u16;
    let c = [o[0] + (sincos::sin_q12(yaw) * 100 >> 12), o[1] + 277, o[2] + (sincos::sin_q12(yaw.wrapping_add(1024) & 0xfff) * 100 >> 12)];
    let d = dir_q12(start, end);
    let t = ((c[0] - start[0]) * d[0] + (c[1] - start[1]) * d[1] + (c[2] - start[2]) * d[2]) >> 12;
    let q = [start[0] + (d[0] * t >> 12), start[1] + (d[1] * t >> 12), start[2] + (d[2] * t >> 12)];
    N.head_hit = t > 0 && dist2_3(q, c) < 100 * 100;
}

/// CNihilanth::TraceAttack + TakeDamage. Returns the u8 health to show.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn damage(pi: usize, dmg: u8) -> u8 {
    let g = n();
    if g.pi as usize != pi || g.dead {
        return PROP_HEALTH[pi];
    }
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
        setpiece_sfx::play(SP::NIH_DIE, PROP_POS[pi]); // DeathSound
        return 1; // DyingThink rises to n_max before n_dead fires
    }
    // PainSound: a laugh while above half health, a cry once the head is
    // open; at most one every 2 to 5 s.
    if time_reached(SIM_NOW, PAIN_NEXT) {
        PAIN_NEXT = SIM_NOW.wrapping_add(40 + IMPACT_RNG.below(61) as u16);
        if g.health > g.full / 2 {
            setpiece_sfx::play(SP::NIH_LAUGH, PROP_POS[pi]);
        } else if g.irritation >= 2 {
            setpiece_sfx::play(SP::NIH_PAIN, PROP_POS[pi]);
        }
    }
    (g.health * 255 / g.full.max(1)).clamp(1, 255) as u8
}

/// The nihilanth's current sequence as (roster slot, ticks, elapsed, loops).
/// Roster: 0 float, 2 attack1, 5 attack2, 6 recharge, 7 float_open, 8
/// attack1_open.
#[optimize(size)]
pub(crate) unsafe fn clip(pi: usize) -> Option<(usize, usize, usize)> {
    let g = n();
    if g.pi as usize != pi || g.dead {
        return None;
    }
    let slot = match g.seq {
        SEQ_ATTACK1 => 2,
        SEQ_ATTACK2 => 5,
        SEQ_RECHARGE => 6,
        SEQ_OPEN_ATTACK => 8,
        _ if g.irritation >= 2 => 7,
        _ => 0,
    };
    Some((slot, g.seq_len as usize, SIM_NOW.wrapping_sub(g.seq_start) as usize))
}

/// Pick the next sequence (CNihilanth::NextActivity).
#[inline(never)]
#[optimize(size)]
unsafe fn next_activity(m: &Map, nlogic: usize, now: u16, player_seen: bool, dist: i32, facing: bool) {
    let g = n();
    if (g.health < g.full / 2 || g.spheres < N_SPHERES / 2) && g.recharger == u16::MAX && g.level <= 9 {
        match marker(m, name_id(m, "n_recharger", g.level)) {
            Some(li) => {
                g.recharger = li as u16;
                let r = m.logic(li).origin;
                let p = PROP_POS[g.pi as usize];
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
    // Sequences speed up as he weakens: framerate = 2 - health / max.
    let rate = 8192 - g.health.clamp(0, g.full) * 4096 / g.full.max(1);
    let len = |ticks: u16| ((ticks as i32 * 4096) / rate) as u16;
    if g.recharger != u16::MAX {
        let r = m.logic(g.recharger as usize).origin;
        if ((g.z >> 4) - r[1]).abs() < 128 {
            if g.seq != SEQ_RECHARGE {
                // Event 5 (start up sphere machine): a recharge cry.
                setpiece_sfx::play(SP::NIH_RECHARGE, PROP_POS[g.pi as usize]);
                logic_fire_targets(m, nlogic, m.n_ents, name_id(m, "n_draw", g.level), map::USE_ON, now, 0, logic_state::CALLER_NONE);
            }
            g.seq = SEQ_RECHARGE;
            g.seq_len = len(SEQ_ATTACK);
            return;
        }
    } else if player_seen && g.irritation != 0 && dist < 256 && facing {
        g.seq = if g.irritation >= 2 && g.health < g.full / 2 {
            SEQ_OPEN_ATTACK
        } else if IMPACT_RNG.below(2) == 0 {
            SEQ_ATTACK1
        } else {
            SEQ_ATTACK2
        };
        g.seq_len = len(SEQ_ATTACK);
        return;
    }
    g.seq = SEQ_FLOAT_ID;
    g.seq_len = len(SEQ_FLOAT);
}

/// HuntThink / DyingThink, every tick (the monster thinks every other).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick(m: &Map, movers: &[phys::Mover]) {
    let g = n();
    if g.pi == 0xff {
        if !g.dead {
            // First tick of the map: find the nihilanth once.
            g.dead = true;
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
    let now = SIM_NOW;
    let nlogic = m.n_logic.min(MAX_LOGIC);
    tick_balls(m, movers, nlogic, now);
    let pos = PROP_POS[pi];
    let p = LOGIC_PLAYER_POS;
    let head = [pos[0], (g.z >> 4) + 300, pos[2]];
    let seen = LOGIC_PLAYER_HEALTH > 0 && phys::trace_line(m, movers, head, p).is_none();
    // Flight (every tick): yaw toward the desired heading, vertical force
    // toward m_posDesired.z two seconds out, 0.995 drag.
    g.z += g.vz / 20;
    g.yaw += g.avel / 20;
    if now & 1 == 0 {
        let yaw = ((g.yaw >> 4) & 0xfff) as u16;
        let right = [sincos::sin_q12(yaw.wrapping_add(1024) & 0xfff), sincos::sin_q12(yaw.wrapping_add(2048) & 0xfff)];
        let side = (g.desired[0] * right[0] + g.desired[1] * right[1]) >> 12;
        // A degree is 4096 / 360 q12 turns; x16 fixed point.
        const DEG: i32 = 182;
        // Runtime yaw turns clockwise, GoldSrc's counter-clockwise: a
        // heading to the left (flSide < 0) lowers it.
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
    prop_set_pos_exact(m, pi, [pos[0], (g.z >> 4), pos[2]]);
    PROP_YAW[pi] = prop_with_yaw(PROP_YAW[pi], ((g.yaw >> 4) & 0xfff) as u16);
    if now & 1 != 0 {
        return;
    }
    if g.dead {
        // DyingThink: rise to n_max, then n_dead (the u8 health's zero
        // fires the cooked AITRIGGER_DEATH_USE_ON record).
        if ((g.z >> 4) - g.max_z).abs() < 16 && PROP_HEALTH[pi] != 0 {
            PROP_HEALTH[pi] = 0;
            PROP_STATE[pi] = PROP_STATE_DEAD;
            PROP_DEATH_START[pi] = now;
            monster_damage_ai_triggers(pi);
        }
        return;
    }
    // Absorb a sphere per think while hurt.
    if g.health < g.full && g.spheres > 0 {
        g.spheres -= 1;
        g.health = (g.health + g.full / N_SPHERES as i32).min(g.full);
    }
    PROP_HEALTH[pi] = (g.health * 255 / g.full.max(1)).clamp(1, 255) as u8;
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
    // Sequence events.
    let t = now.wrapping_sub(g.seq_start) as i32;
    let at = |frame_ticks: u16| (frame_ticks as i32 * g.seq_len as i32 / SEQ_ATTACK as i32) as i32;
    match g.seq {
        SEQ_ATTACK1 if t == at(EV_ZAP) => {
            g.shoot_end = now.wrapping_add(20);
            zen_sound(head);
        }
        SEQ_OPEN_ATTACK if t == at(EV_ZAP) => {
            launch(head, p, 1);
            zen_sound(head);
        }
        SEQ_ATTACK2 if t == at(EV_TELE) => {
            if name_id(m, "n_teleport", g.teleport) != 0 || name_id(m, "n_leaving", g.teleport) != 0 {
                // Event 6: an attack cry, then TeleportInit's x_teleattack1.
                setpiece_sfx::play(SP::NIH_ATTACK, head);
                setpiece_sfx::play(SP::NIH_TELE, head);
                launch(head, p, 2);
            } else {
                g.teleport += 1;
                g.shoot_end = now.wrapping_add(20);
                setpiece_sfx::play(SP::NIH_BALL, head);
            }
        }
        SEQ_RECHARGE => {
            if RECHARGE_EVENTS.iter().any(|&f| t == at((f as u16 * 5) / 2)) {
                if g.spheres < N_SPHERES {
                    g.spheres += 1;
                } else {
                    g.recharger = u16::MAX;
                }
            }
            if g.recharger != u16::MAX && LOGIC_STATE[g.recharger as usize] == LOGIC_STATE_REMOVED {
                g.recharger = u16::MAX;
            }
        }
        _ => {}
    }
    // ShootBalls: a pair from the hands every 0.2 s for a second.
    if !time_reached(now, g.shoot_end) && t % 4 == 0 {
        launch([head[0] + 100, head[1] - 150, head[2]], p, 1);
        launch([head[0] - 100, head[1] - 150, head[2]], p, 1);
    }
    if t >= g.seq_len as i32 {
        if g.seq == SEQ_RECHARGE && g.spheres >= N_SPHERES {
            g.recharger = u16::MAX;
        }
        // flDist to m_posDesired (straight below or above him) and flDot of
        // m_vecDesired with his facing.
        let yaw = ((g.yaw >> 4) & 0xfff) as u16;
        let facing = g.desired[0] * sincos::sin_q12(yaw) + g.desired[1] * sincos::sin_q12(yaw.wrapping_add(1024) & 0xfff) > 0;
        next_activity(m, nlogic, now, !time_reached(now, g.last_seen.wrapping_add(100)), ((g.z >> 4) - g.desired_z).abs(), facing);
    }
}

/// A zap (1) or teleport (2) energy ball from `from` toward `to`.
#[inline(never)]
#[optimize(size)]
unsafe fn launch(from: [i32; 3], to: [i32; 3], kind: u8) {
    let g = n();
    if let Some(b) = (0..MAX_BALLS).find(|&b| g.ball_kind[b] == 0) {
        g.ball_kind[b] = kind;
        g.ball_pos[b] = from;
        let d = dir_q12(from, to);
        // ZapInit: 200 u/s; the teleport ball keeps a fifth of its climb.
        g.ball_vel[b] = [d[0] * 10 >> 12, d[1] * if kind == 2 { 2 } else { 10 } >> 12, d[2] * 10 >> 12];
    }
}

/// CNihilanthHVR ZapThink / TeleportThink.
#[inline(never)]
#[optimize(size)]
unsafe fn tick_balls(m: &Map, movers: &[phys::Mover], nlogic: usize, now: u16) {
    let g = n();
    let p = LOGIC_PLAYER_POS;
    for b in 0..MAX_BALLS {
        let kind = g.ball_kind[b];
        if kind == 0 {
            continue;
        }
        let pos = g.ball_pos[b];
        let d = isqrt_i32(dist2_3(pos, p));
        if kind == 1 {
            // Accelerate 1.2x per 0.05 s to 2000 u/s; within 256 of the
            // player's centre, a sk_nihilanth_zap shock.
            let v = g.ball_vel[b];
            if isqrt_i32(dist2_3(v, [0; 3])) < 100 {
                g.ball_vel[b] = v.map(|x| x * 6 / 5);
            }
            if d < 256 {
                g.ball_kind[b] = 0;
                if phys::trace_line(m, movers, pos, p).is_none() && LOGIC_PLAYER_HEALTH > 0 {
                    PENDING_PLAYER_DAMAGE = PENDING_PLAYER_DAMAGE.saturating_add(skill_table::SKILL_NIHILANTH_ZAP[settings::skill()]);
                    note_damage_direction(pos);
                }
                continue;
            }
        } else {
            // MovetoTarget the player's centre; within 128, n_leaving<n> is
            // used and n_teleport<n> touched by the player.
            // m_vecIdeal capped at 300 u/s plus 300 u/s toward the target.
            let dir = dir_q12(pos, p);
            let v = g.ball_vel[b];
            let s = isqrt_i32(dist2_3(v, [0; 3])).max(15);
            g.ball_vel[b] = [0, 1, 2].map(|k| v[k] * 15 / s + (dir[k] * 15 >> 12));
            if d < 128 {
                g.ball_kind[b] = 0;
                let n = g.teleport;
                logic_fire_targets(m, nlogic, m.n_ents, name_id(m, "n_leaving", n), map::USE_ON, now, 0, logic_state::CALLER_NONE);
                if let Some(li) = marker(m, name_id(m, "n_teleport", n)) {
                    if LOGIC_KIND[li] == map::LOGIC_TRIGGER_TELEPORT {
                        logic_teleport_touch(m, nlogic, m.logic(li));
                    }
                }
                continue;
            }
        }
        let v = g.ball_vel[b];
        let next = [pos[0] + v[0], pos[1] + v[1], pos[2] + v[2]];
        if phys::trace_line(m, movers, pos, next).is_some() {
            // ZapTouch: RadiusDamage(50, DMG_SHOCK).
            if kind == 1 && d < 125 {
                PENDING_PLAYER_DAMAGE = PENDING_PLAYER_DAMAGE.saturating_add((50 * (125 - d) / 125) as u16);
            }
            g.ball_kind[b] = 0;
            continue;
        }
        g.ball_pos[b] = next;
        push_tracer_styled(pos, next, if kind == 1 { TRACER_ZAP } else { TRACER_TELE });
    }
}

/// The spheres circling him, for the renderer: count and centre.
pub(crate) unsafe fn spheres() -> Option<(u8, [i32; 3])> {
    let g = n();
    if g.pi == 0xff || g.dead {
        return None;
    }
    let p = PROP_POS[g.pi as usize];
    Some((g.spheres, [p[0], (g.z >> 4) + 240, p[2]]))
}
