//! CApache (apache.cpp): HuntThink / Flight's thrust-and-tilt flight model,
//! the chin gun, HVR rocket pairs, damage rules and the crash. One apache
//! per map at most (c2a5, c2a5a, c2a5w, c2a5x).
//!
//! State is kept in GoldSrc axes (x, y, z up) so Flight's arithmetic reads
//! like the SDK: positions and velocities x16, angles and angular
//! velocities in 1/16 degree (pitch, yaw, roll), thinking every 0.1 s and
//! integrating MOVETYPE_FLY every 20 Hz tick.

use crate::*;
use hl_format::setpiece_audio as SP;

/// The next tick FireGun may start another tu_fire1 burst.
static mut GUN_SOUND_NEXT: u16 = 0;

const SF_WAITFORTRIGGER: u16 = 0x04 | 0x40;
const SF_NOWRECKAGE: u16 = 0x08;
const D: i32 = 16; // fixed-point scale of degrees and units

struct Apache {
    li: u16,
    pi: u8,
    /// 0 waiting for its trigger, 1 hunting, 2 dying.
    phase: u8,
    corner: u8,
    corners: u8,
    loop_start: u8,
    rockets: u8,
    side: i8,
    aux: u16,
    flags: u16,
    /// Timers in ticks: m_flLastSeen, m_flPrevSeen, m_flNextRocket.
    last_seen: u16,
    prev_seen: u16,
    next_rocket: u16,
    pos: [i32; 3],
    vel: [i32; 3],
    ang: [i32; 3],
    avel: [i32; 3],
    force: i32,
    goal_speed: i32,
    gun: [i32; 2],
    desired: [i32; 3],
    goal_dir: [i32; 3],
    target: [i32; 3],
}

static mut AP: Apache = Apache {
    li: u16::MAX,
    pi: 0,
    phase: 0,
    corner: 0,
    corners: 0,
    loop_start: 0,
    rockets: 10,
    side: 1,
    aux: 0,
    flags: 0,
    last_seen: 0,
    prev_seen: 0,
    next_rocket: 0,
    pos: [0; 3],
    vel: [0; 3],
    ang: [0; 3],
    avel: [0; 3],
    force: 0,
    goal_speed: 0,
    gun: [0; 2],
    desired: [0; 3],
    goal_dir: [0; 3],
    target: [0; 3],
};

/// The tilt (reflected q8 pitch | roll << 8) the apache is drawn with.
pub(crate) static mut APACHE_TILT: (u8, u16) = (0xff, 0);

#[inline(always)]
unsafe fn ap() -> &'static mut Apache {
    &mut *core::ptr::addr_of_mut!(AP)
}

pub(crate) unsafe fn reset() {
    AP.li = u16::MAX;
    APACHE_TILT.0 = 0xff;
    // See garg::reset: deadlines do not survive the room's clock.
    GUN_SOUND_NEXT = 0;
}

/// Sine and cosine (q12) of an angle in 1/16 degree.
#[inline(never)]
#[optimize(size)]
fn sc(a: i32) -> (i32, i32) {
    let t = ((a * 32 / 45) & 0xfff) as u16; // 1/16 degree -> 4096ths
    (sincos::sin_q12(t), sincos::sin_q12(t.wrapping_add(1024) & 0xfff))
}

/// UTIL_MakeAimVectors: AngleVectors with the pitch negated. q12
/// [forward, right, up] in GoldSrc axes.
#[inline(never)]
#[optimize(size)]
fn aim_vectors(a: [i32; 3]) -> [[i32; 3]; 3] {
    let (sp, cp) = sc(-a[0]);
    let (sy, cy) = sc(a[1]);
    let (sr, cr) = sc(a[2]);
    let m = |x: i32, y: i32| (x * y) >> 12;
    [
        [m(cp, cy), m(cp, sy), -sp],
        [m(-m(sr, sp), cy) + m(cr, sy), m(-m(sr, sp), sy) - m(cr, cy), -m(sr, cp)],
        [m(m(cr, sp), cy) + m(sr, sy), m(m(cr, sp), sy) - m(sr, cy), m(cr, cp)],
    ]
}

/// `a . b >> 12`; one side is a q12 unit vector, the other under 2^17.
#[inline(never)]
#[optimize(size)]
fn dot(a: [i32; 3], b: [i32; 3]) -> i32 {
    (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) >> 12
}

/// `a + b * s >> 12`.
#[inline(never)]
#[optimize(size)]
fn mad(a: [i32; 3], b: [i32; 3], s: i32) -> [i32; 3] {
    let mut r = a;
    for k in 0..3 {
        r[k] += (b[k] * s) >> 12;
    }
    r
}

/// `a - b`.
#[inline(never)]
#[optimize(size)]
fn sub(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Unit (q12) direction of a vector (components under 2^14).
#[inline(never)]
#[optimize(size)]
fn norm(v: [i32; 3]) -> [i32; 3] {
    let l = isqrt_i32(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).max(1);
    [v[0] * 4096 / l, v[1] * 4096 / l, v[2] * 4096 / l]
}

/// A x16 vector in whole units.
#[inline(never)]
#[optimize(size)]
fn whole(v: [i32; 3]) -> [i32; 3] {
    [v[0] / D, v[1] / D, v[2] / D]
}

/// The apache's runtime-axes position in whole units.
#[inline(never)]
#[optimize(size)]
fn at(a: &Apache) -> [i32; 3] {
    hl(whole(a.pos))
}

/// Runtime axes <-> GoldSrc axes.
#[inline(always)]
#[optimize(size)]
fn hl(p: [i32; 3]) -> [i32; 3] {
    [p[0], p[2], p[1]]
}

/// The path_corner `k` of the cooked chain: (position, forward of its angles).
#[inline(never)]
#[optimize(size)]
unsafe fn corner(m: &Map, k: usize) -> ([i32; 3], [i32; 3]) {
    let fa = AP.aux as usize + k * 3;
    let (a, b, c) = (m.logic_aux(fa), m.logic_aux(fa + 1), m.logic_aux(fa + 2));
    let pos = hl([a.target as i16 as i32, a.delay_ticks as i16 as i32, b.target as i16 as i32]);
    // The corner's yaw is cooked in the runtime convention (0 = +Z); its
    // pitch as a reflected q8.
    let yaw_deg = (1024 - (c.target & 0xfff) as i32) * 360 * D / 4096;
    let pitch_deg = -((c.delay_ticks as u8 as i8) as i32) * 360 * D / 256;
    (pos, aim_vectors([pitch_deg, yaw_deg, 0])[0])
}

/// Bind the map's apache (CApache::Spawn).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn init(li: usize, rec: map::LogicEnt, pi: usize) {
    let a = ap();
    a.li = li as u16;
    a.pi = pi as u8;
    a.aux = rec.first_aux as u16;
    a.corners = (rec.aux_count / 3) as u8;
    a.loop_start = rec.flags;
    a.corner = 0;
    a.flags = rec.speed;
    a.phase = if rec.speed & SF_WAITFORTRIGGER != 0 { 0 } else { 1 };
    a.rockets = 10;
    a.side = 1;
    let p = hl(PROP_POS[pi]);
    a.pos = [p[0] * D, p[1] * D, p[2] * D];
    a.vel = [0; 3];
    a.avel = [0; 3];
    a.ang = [0, (1024 - prop_yaw_value(PROP_YAW[pi]) as i32) * 360 * D / 4096, 0];
    a.force = 0;
    a.goal_speed = 0;
    a.gun = [0; 2];
    a.next_rocket = SIM_NOW;
    a.last_seen = SIM_NOW.wrapping_sub(2000);
    a.prev_seen = a.last_seen;
    a.desired = p;
    a.goal_dir = aim_vectors(a.ang)[0];
    a.corner = 0xff; // the first corner loads on the first tick
}

/// CApache::StartupUse.
#[optimize(size)]
pub(crate) unsafe fn startup(li: usize) {
    if AP.li as usize == li && AP.phase == 0 {
        AP.phase = 1;
    }
}

/// CApache::TakeDamage / TraceAttack: blast doubles; a hit of 50 or less
/// ricochets; the skill.cfg health scales onto the u8 actor health.
#[optimize(size)]
pub(crate) fn scale_damage(dmg: u8, blast: bool) -> u8 {
    let d = dmg as u32 * if blast { 2 } else { 1 };
    if d <= 50 {
        return 0;
    }
    let full = skill_table::SKILL_APACHE_HEALTH[settings::skill()].max(255) as u32;
    (d * 255 / full).min(255) as u8
}

/// Per-tick MOVETYPE_FLY / MOVETYPE_TOSS integration, and a think every
/// other tick.
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
    let now = SIM_NOW;
    if a.corner == 0xff {
        a.corner = 0;
        if a.corners != 0 {
            (a.desired, a.goal_dir) = corner(m, 0);
        }
    }
    if a.phase == 0 {
        return;
    }
    if PROP_HEALTH[pi] == 0 && a.phase == 1 {
        // Killed: MOVETYPE_TOSS at 0.3 gravity, crash after 15 s (4 s with
        // SF_NOWRECKAGE) or on the first solid touch.
        a.phase = 2;
        a.next_rocket = now.wrapping_add(if a.flags & SF_NOWRECKAGE != 0 { 80 } else { 300 });
    }
    let think = now & 1 == 0;
    if a.phase == 2 {
        a.vel[2] -= 800 * 3 / 10 * D / 20;
        if think {
            for k in 0..3 {
                a.avel[k] = a.avel[k] * 102 / 100;
            }
        }
    }
    // Move, and FlyTouch / CrashTouch against the world.
    let from = at(a);
    for k in 0..3 {
        a.pos[k] += a.vel[k] / 20;
        a.ang[k] += a.avel[k] / 20;
    }
    let to = at(a);
    if let Some(h) = phys::trace_line(m, movers, from, to) {
        if a.phase == 2 {
            a.next_rocket = now;
        } else {
            let n = hl(h.normal);
            let w = whole(a.vel);
            let s = isqrt_i32(dot(w, w) << 12) + 200;
            a.vel = mad(a.vel, n, s * D);
        }
        let p = hl(h.pos);
        a.pos = p.map(|x| x * D);
    }
    prop_set_pos_exact(m, pi, at(a));
    // CApache::ShowDamage/Flight: ap_rotor2 on CHAN_STATIC while it flies.
    setpiece_sfx::keep_loop(SP::APACHE_ROTOR, PROP_POS[pi], setpiece_sfx::OWNER_APACHE_ROTOR);
    PROP_YAW[pi] = prop_with_yaw(PROP_YAW[pi], ((1024 - a.ang[1] * 4096 / (360 * D)) & 0xfff) as u16);
    let q8 = |d: i32| ((-d * 256 / (360 * D)) & 0xff) as u16;
    APACHE_TILT = (a.pi, q8(a.ang[0]) | (q8(a.ang[2]) << 8));
    if !think {
        return;
    }
    if a.phase == 2 {
        dying(m, pi, now);
    } else {
        hunt(m, movers, now);
    }
}

/// CApache::DyingThink.
#[inline(never)]
#[optimize(size)]
unsafe fn dying(m: &Map, pi: usize, now: u16) {
    let o = PROP_POS[pi];
    if !time_reached(now, AP.next_rocket) {
        if now & 3 == 0 {
            let r = |v: i32| v + IMPACT_RNG.below(301) as i32 - 150;
            queue_explosion_fx([r(o[0]), o[1] - 50 - IMPACT_RNG.below(101) as i32, r(o[2])], 50);
        }
        return;
    }
    // RadiusDamage(300, DMG_BLAST) (u8-capped), the fireball and gibs.
    explode(m, o, 255, 750, false);
    spawn_gibs(o, 12);
    setpiece_sfx::stop_loop(setpiece_sfx::OWNER_APACHE_ROTOR);
    PROP_ACTIVE[pi] = 0;
    AP.li = u16::MAX;
}

/// CApache::HuntThink.
#[inline(never)]
#[optimize(size)]
unsafe fn hunt(m: &Map, movers: &[phys::Mover], now: u16) {
    let a = ap();
    let origin = whole(a.pos);
    let here = at(a);
    let p = LOGIC_PLAYER_POS;
    let enemy = hl(p);
    // Look(4092) + BestVisibleEnemy + FVisible: the player in sight.
    let seen = LOGIC_PLAYER_HEALTH > 0
        && dist2_3(here, p) < 4092 * 4092
        && phys::trace_line(m, movers, here, [p[0], p[1] + VIEW_HEIGHT, p[2]]).is_none();
    if a.goal_speed < 800 {
        a.goal_speed += 5;
    }
    if seen {
        // m_flPrevSeen restarts when the enemy was last seen over 5 s ago.
        if time_reached(now, a.last_seen.wrapping_add(100)) {
            a.prev_seen = now;
        }
        a.last_seen = now;
        a.target = enemy;
    }
    let to_target = norm(sub(a.target, origin));
    if a.corners == 0 {
        a.desired = origin;
    }
    let mut length = isqrt_i32(dist2_3(origin, a.desired));
    if a.corners != 0 && length < 128 {
        // The next path_corner; the cooked chain ends where it loops back.
        let next = a.corner + 1;
        a.corner = if next >= a.corners { a.loop_start } else { next };
        (a.desired, a.goal_dir) = corner(m, a.corner as usize);
        length = isqrt_i32(dist2_3(origin, a.desired));
    }
    let to_desired = norm(sub(a.desired, origin));
    let want = if length > 250 {
        if !time_reached(now, a.last_seen.wrapping_add(1800)) && dot(to_target, to_desired) > 1024 {
            to_target
        } else {
            to_desired
        }
    } else {
        a.goal_dir
    };
    flight(a, want);
    // FireGun while seen within the last second, after two seconds in view.
    if !time_reached(now, a.last_seen.wrapping_add(20)) && time_reached(now, a.prev_seen.wrapping_add(40)) {
        if fire_gun(m, movers, a) && a.goal_speed > 400 {
            a.goal_speed = 400;
        }
        if settings::skill() == 0 {
            a.next_rocket = now.wrapping_add(200); // no rockets while firing on easy
        }
    }
    // HVR rockets: pairs 0.1 s apart, a pair per 0.5 s, ten then 10 s off.
    let v = aim_vectors(a.ang);
    let vel = whole(a.vel);
    let est = norm(mad(vel, v[0], 800));
    let fire = if a.rockets % 2 == 1 {
        true
    } else if a.ang[0] < 0
        && dot(vel, v[0]) > -100
        && time_reached(now, a.next_rocket)
        && !time_reached(now, a.last_seen.wrapping_add(1200))
        && dot(to_target, est) > 3953
    {
        let far = mad(origin, est, 4096);
        let end = phys::trace_line(m, movers, here, hl(far)).map_or(hl(far), |h| h.pos);
        dist2_3(end, hl(a.target)) < 512 * 512
    } else {
        false
    };
    if fire && a.rockets > 0 {
        // Launch point: 1.5 * (forward 21, right +-70, up -79).
        let s = a.side as i32;
        let src = mad(mad(mad(origin, v[0], 32), v[1], 105 * s), v[2], -119);
        spawn_projectile_dir(PROJ_ROCKET, 150, hl(src), hl(v[0]), true);
        setpiece_sfx::play(SP::APACHE_ROCKET, hl(src));
        a.rockets -= 1;
        a.side = -a.side;
        a.next_rocket = now.wrapping_add(10);
        if a.rockets == 0 {
            a.next_rocket = now.wrapping_add(200);
            a.rockets = 10;
        }
    }
}

/// CApache::Flight, per 0.1 s think.
#[inline(never)]
#[optimize(size)]
unsafe fn flight(a: &mut Apache, want: [i32; 3]) {
    // Aim ahead by `s` seconds of angular velocity, tilted 5 degrees down.
    fn add(a: &Apache, s: i32) -> [i32; 3] {
        [a.ang[0] + a.avel[0] * s + 5 * D, a.ang[1] + a.avel[1] * s, a.ang[2] + a.avel[2] * s]
    }
    // Yaw toward where we want to go, as we will face in two seconds.
    let v = aim_vectors(add(a, 2));
    if dot(want, v[1]) < 0 {
        if a.avel[1] < 60 * D {
            a.avel[1] += 8 * D;
        }
    } else if a.avel[1] > -60 * D {
        a.avel[1] -= 8 * D;
    }
    a.avel[1] = a.avel[1] * 98 / 100;
    // Where we will be in two seconds.
    let v = aim_vectors(add(a, 1));
    let mut est = mad(whole(a.pos), whole(a.vel), 8192);
    est = mad(est, v[2], a.force * 20 / D);
    est[2] -= 768;
    let v = aim_vectors(add(a, 0));
    a.vel = mad(a.vel, v[2], a.force);
    a.vel[2] -= 384 * D / 10; // gravity, 38.4 per think
    let vel = whole(a.vel);
    let mut speed = isqrt_i32(dot(vel, vel) << 12);
    if v[0][0] * vel[0] + v[0][1] * vel[1] < 0 {
        speed = -speed;
    }
    let off = sub(a.desired, est);
    let dist = dot(off, v[0]);
    let slip = -dot(off, v[1]);
    // Bank into sideways travel.
    let (roll, rv) = (a.ang[2], &mut a.avel[2]);
    if slip > 0 {
        if roll > -30 * D && *rv > -15 * D {
            *rv -= 4 * D;
        } else {
            *rv += 2 * D;
        }
    } else if roll < 30 * D && *rv < 15 * D {
        *rv += 4 * D;
    } else {
        *rv -= 2 * D;
    }
    // Sideways drag, then general drag.
    for k in 0..3 {
        a.vel[k] = a.vel[k] * (4096 - (v[1][k].abs() * 5 / 100)) / 4096 * 995 / 1000;
    }
    // Collective: hold the desired height two seconds out.
    let high = est[2] > a.desired[2];
    if a.force < 80 * D && !high && est[2] != a.desired[2] {
        a.force += 12 * D;
    } else if a.force > 30 * D && high {
        a.force -= 8 * D;
    }
    // Pitch forward or back to reach the target.
    let pitch = a.ang[0] + a.avel[0];
    if dist > 0 && speed < a.goal_speed && pitch > -40 * D {
        a.avel[0] -= 12 * D;
    } else if dist < 0 && speed > -50 && pitch < 20 * D {
        a.avel[0] += 12 * D;
    } else if pitch > 0 {
        a.avel[0] -= 4 * D;
    } else if pitch < 0 {
        a.avel[0] += 4 * D;
    }
}

/// CApache::FireGun: the chin turret (bone controllers yaw -90..90, pitch
/// -10..45) slews 12 degrees per think toward the target; a 12mm round in
/// the 4 degree cone when it is within 0.98 of it.
#[inline(never)]
#[optimize(size)]
unsafe fn fire_gun(m: &Map, movers: &[phys::Mover], a: &mut Apache) -> bool {
    let v = aim_vectors(a.ang);
    // Gun pivot: attachment 2 of apache.mdl, 97 ahead and 145 below.
    let gun = mad(mad(whole(a.pos), v[0], 97), v[2], -145);
    let t = norm(sub(a.target, gun));
    let local = [dot(v[0], t), -dot(v[1], t), dot(v[2], t)];
    let ang = |y: i32, x: i32| crate::tank::atan_s(y, x) * 360 * D / 4096;
    let want_yaw = ang(local[1], local[0]);
    let want_pitch = -ang(local[2], isqrt_i32(local[0] * local[0] + local[1] * local[1]));
    let step = |cur: i32, want: i32| cur + (want - cur).clamp(-12 * D, 12 * D);
    a.gun[0] = step(a.gun[0], want_yaw).clamp(-90 * D, 90 * D);
    a.gun[1] = step(a.gun[1], want_pitch).clamp(-10 * D, 45 * D);
    // 0.98 is about 11.5 degrees between barrel and target.
    let off = (want_yaw - a.gun[0]).abs() + (want_pitch - a.gun[1]).abs();
    if off > 11 * D {
        return false;
    }
    let spread = |_: ()| IMPACT_RNG.below(287) as i32 - 143; // VECTOR_CONE_4DEGREES, q12
    let (r, u) = (spread(()), spread(()));
    let dir = mad(mad(t, v[1], r), v[2], u);
    let far = hl(mad(gun, dir, 8192));
    let from = hl(gun);
    let wall = phys::trace_line(m, movers, from, far);
    let end = wall.as_ref().map_or(far, |h| h.pos);
    let p = LOGIC_PLAYER_POS;
    let h = LOGIC_PLAYER_HALF_HEIGHT;
    if LOGIC_PLAYER_HEALTH > 0
        && segment_box_frac(from, end, [p[0] - 16, p[1] - h, p[2] - 16], [p[0] + 16, p[1] + h, p[2] + 16]).is_some()
    {
        PENDING_PLAYER_DAMAGE = PENDING_PLAYER_DAMAGE.saturating_add(skill_damage(21).unwrap_or(8) as u16);
        note_damage_direction(from);
    }
    push_tracer(from, end);
    // FireGun emits tu_fire1 on CHAN_WEAPON every shot, each cutting off the
    // last; one 1.35 s burst per second keeps the pool from flooding.
    if time_reached(SIM_NOW, GUN_SOUND_NEXT) {
        GUN_SOUND_NEXT = SIM_NOW.wrapping_add(20);
        setpiece_sfx::play(SP::APACHE_GUN, from);
    }
    true
}
