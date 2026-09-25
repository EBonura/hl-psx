//! func_tank family (func_tank.cpp): CFuncTank::TrackTarget / Fire for the
//! AI guns, mortars, rockets and lasers, and the player-controlled tanks
//! that func_tankcontrols hand over.
//!
//! Angles are pev->angles in q12 turns, [yaw, pitch] with GoldSrc's brush
//! pitch sign (positive = barrel down); rates are q12 per 20 Hz tick. The
//! cooked keys (hl_format::logic::TANK) are decoded once per map.

use crate::*;

pub(crate) const MAX_TANKS: usize = 6;
const SF_TANK_ACTIVE: u16 = 1;
const SF_TANK_LINEOFSIGHT: u16 = 0x10;
const SF_TANK_CANCONTROL: u16 = 0x20;
const ON: u8 = 1;
const LOS: u8 = 2;
const CONTROL: u8 = 4;

#[derive(Clone, Copy)]
struct Tank {
    li: u16,
    ei: u16,
    target: u16,
    aux: u16,
    aux_count: u8,
    /// ON | LOS | CONTROL.
    flags: u8,
    /// bullet (bits 0-1), spread (2-4), class (5-6).
    kind: u8,
    persist: u8,
    ang: [i16; 2],
    avel: [i16; 2],
    sight: [i16; 3],
    /// m_fireLast (0 = the next think in the cone only starts the clock).
    fire_last: u16,
    sight_time: u16,
    next: u16,
    rate: [u8; 2], // yaw, pitch
    tol: [u8; 2],
    range: [i16; 2], // yaw, pitch
    fire_rate: u16,  // q8 per second
    barrel: [i16; 3],
    damage: u16,
    min_range: i16,
    max_range: i16,
    centre: [i16; 2],
}

const NO_TANK: Tank = Tank {
    li: 0,
    ei: 0,
    target: 0,
    aux: 0,
    aux_count: 0,
    flags: 0,
    kind: 0,
    persist: 0,
    ang: [0; 2],
    avel: [0; 2],
    sight: [0; 3],
    fire_last: 0,
    sight_time: 0,
    next: 0,
    rate: [0; 2],
    tol: [0; 2],
    range: [0; 2],
    fire_rate: 0,
    barrel: [0; 3],
    damage: 0,
    min_range: 0,
    max_range: 0,
    centre: [0; 2],
};

static mut TANKS: [Tank; MAX_TANKS] = [NO_TANK; MAX_TANKS];
pub(crate) static mut TANK_COUNT: usize = 0;
/// The thinking tank's brush: its own traces ignore it.
static mut TANK_SKIP: i32 = -1;
/// Where the player took the controls (m_vecControllerUsePos).
static mut TANK_USE_POS: [i32; 3] = [0; 3];

/// Signed atan2 in q12 turns from +x toward +y, within about 0.25 degrees
/// (scientist_logic's 0.273-bend fit); the SDK's octant-linear atan2_q12 is
/// up to 4 degrees off, which walks rounds off a distant target.
#[inline(never)]
#[optimize(size)]
pub(crate) fn atan_s(y: i32, x: i32) -> i32 {
    ((scientist_logic::precise_yaw_from_vec(y, x) as i32 + 2048) & 0xfff) - 2048
}

#[inline(always)]
#[optimize(size)]
fn angle_dist(a: i32, b: i32) -> i32 {
    ((a - b + 2048) & 0xfff) - 2048
}

/// UTIL_MakeVectors(pev->angles) as world rows [forward, right, up].
#[inline(never)]
#[optimize(size)]
fn vectors(ang: [i16; 2]) -> [[i32; 3]; 3] {
    let y = ang[0] as u16;
    let p = (-(ang[1] as i32)) as u16; // pitch up
    let (sy, cy) = (sincos::sin_q12(y & 0xfff), sincos::sin_q12(y.wrapping_add(1024) & 0xfff));
    let (sp, cp) = (sincos::sin_q12(p & 0xfff), sincos::sin_q12(p.wrapping_add(1024) & 0xfff));
    [
        [(cp * cy) >> 12, sp, (cp * sy) >> 12],
        [sy, 0, -cy],
        [(-sp * cy) >> 12, cp, (-sp * sy) >> 12],
    ]
}

/// CFuncTank::BarrelPosition.
#[inline(never)]
#[optimize(size)]
unsafe fn barrel(t: &Tank) -> [i32; 3] {
    let v = vectors(t.ang);
    let mut p = ENT_CACHE[t.ei as usize].origin;
    for r in 0..3 {
        for c in 0..3 {
            p[c] += (v[r][c] * t.barrel[r] as i32) >> 12;
        }
    }
    p
}

/// A gun's round: bullet_damage, else FireBullets' skill.cfg value for its
/// BULLET_MONSTER_9MM/MP5/12MM (model types 1, 8 and 21 carry them).
#[optimize(size)]
fn round_damage(t: &Tank) -> u8 {
    if t.damage != 0 {
        return t.damage.min(255) as u8;
    }
    skill_damage([1, 1, 8, 21][t.kind as usize & 3]).unwrap_or(8)
}

/// Reset the map's tanks (CFuncTank::Spawn): centred, and active ones think
/// after a second.
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
        let t = &mut TANKS[TANK_COUNT];
        let (rates, yaw_range) = w(0);
        let (pitch_range, tol) = w(1);
        let (fire_rate, persist_flags) = w(2);
        let (bx, by) = w(3);
        let (bz, damage) = w(4);
        let (min_range, max_range) = w(5);
        let (yc, pc) = w(6);
        *t = Tank {
            li: li as u16,
            ei: rec.brush,
            target: rec.target,
            aux: rec.first_aux as u16,
            aux_count: rec.aux_count as u8,
            flags: (rec.spawnflags & SF_TANK_ACTIVE != 0) as u8 * ON
                | (rec.spawnflags & SF_TANK_LINEOFSIGHT != 0) as u8 * LOS
                | (rec.spawnflags & SF_TANK_CANCONTROL != 0) as u8 * CONTROL,
            kind: (persist_flags >> 8) as u8,
            persist: persist_flags as u8,
            ang: [yc as i16, ((pc << 4) as i16) >> 4],
            avel: [0; 2],
            sight: [0; 3],
            fire_last: 0,
            // The map clock is 1.0 s here and m_lastSightTime is 0, so
            // CanFire holds for the first `persistence` seconds even blind
            // (c2a2d's siloguardgun fires four rounds on load). Spawn's
            // ltime + 1.0 think lands 0.75 s in: SV_ActivateServer's two
            // 0.1 s settle frames already advanced ltime.
            sight_time: now.wrapping_sub(20),
            next: now.wrapping_add(15),
            rate: [rates as u8, (rates >> 8) as u8],
            tol: [tol as u8, (tol >> 8) as u8],
            range: [yaw_range as i16, pitch_range as i16],
            fire_rate: fire_rate.max(1),
            barrel: [bx as i16, by as i16, bz as i16],
            damage,
            min_range: min_range as i16,
            max_range: max_range as i16,
            centre: [yc as i16, ((pc << 4) as i16) >> 4],
        };
        let b = barrel(t);
        t.sight = [b[0] as i16, b[1] as i16, b[2] as i16];
        TANK_COUNT += 1;
    }
}

#[inline(never)]
#[optimize(size)]
unsafe fn slot(li: usize) -> Option<&'static mut Tank> {
    TANKS[..TANK_COUNT].iter_mut().find(|t| t.li as usize == li)
}

/// CFuncTank::Use for a tank the player cannot control: ShouldToggle, then
/// TankActivate / TankDeactivate (a dying gunner's TriggerTarget stops it).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_use(li: usize, use_type: u8, now: u16) {
    let Some(t) = slot(li) else { return };
    let on = t.flags & ON != 0;
    if (use_type == map::USE_ON && on) || (use_type == map::USE_OFF && !on) {
        return;
    }
    t.flags ^= ON;
    t.fire_last = 0;
    t.next = now.wrapping_add(2);
}

/// Does `from -> end` reach the player's box before the world?
#[inline(never)]
#[optimize(size)]
unsafe fn ray_hits_player(m: &Map, movers: &[phys::Mover], from: [i32; 3], end: [i32; 3]) -> bool {
    let p = LOGIC_PLAYER_POS;
    let h = PLAYER_HULL_HALF_HEIGHT;
    let wall = phys::trace_line_skip(m, movers, from, end, TANK_SKIP).map_or(4096, |hit| hit.frac);
    LOGIC_PLAYER_HEALTH > 0
        && segment_box_frac(from, end, [p[0] - 16, p[1] - h, p[2] - 16], [p[0] + 16, p[1] + h, p[2] + 16])
            .is_some_and(|f| f <= wall)
}

/// CFuncTank*::Fire: `count` rounds of the tank's class along its barrel,
/// then its target fires. The player's rounds hit actors through the
/// hitscan; the AI's rounds are tested against the player.
#[inline(never)]
#[optimize(size)]
unsafe fn fire(m: &Map, movers: &[phys::Mover], t: &Tank, count: i32, player: bool) {
    let spread = [0, 102, 205, 410, 1024][((t.kind >> 2) & 7).min(4) as usize];
    let v = vectors(t.ang);
    let barrel = barrel(t);
    let class = (t.kind >> map::TANK_CLASS_SHIFT) as u16;
    TANK_SKIP = t.ei as i32;
    for _ in 0..count {
        if class == map::TANK_CLASS_ROCKET {
            spawn_projectile_dir(PROJ_ROCKET, 100, barrel, v[0], !player);
            continue;
        }
        // TankTrace: gTankSpread cone, 4096 units.
        let mut end = barrel;
        for x in 0..2 {
            let r = (IMPACT_RNG.below(4097) as i32 + IMPACT_RNG.below(4097) as i32 - 4096) * spread >> 12;
            for c in 0..3 {
                end[c] += if x == 0 { v[0][c] } else { 0 } + ((r * v[x + 1][c]) >> 12);
            }
        }
        let impact = phys::trace_line_skip(m, movers, barrel, end, TANK_SKIP).map_or(end, |h| h.pos);
        push_tracer(barrel, impact);
        if class == map::TANK_CLASS_MORTAR {
            // One explosion of iMagnitude, out to 2.5x.
            let mag = t.damage.min(255) as i32;
            explode(m, impact, mag as u8, mag * 5 / 2, player);
            break;
        }
        if player {
            // FireBullets from the controlled gun: the player's hitscan.
            let a = [atan_s(end[2] - barrel[2], end[0] - barrel[0]), 0];
            let d = [end[0] - barrel[0], end[1] - barrel[1], end[2] - barrel[2]];
            let pitch = atan_s(d[1], isqrt_i32(d[0] * d[0] + d[2] * d[2]));
            let rot = view_rotation(((1024 - a[0]) & 0xfff) as u16, pitch as i16);
            let base_t = [-dot12(rot.m[0], barrel), -dot12(rot.m[1], barrel), -dot12(rot.m[2], barrel)];
            fire_hitscan(m, movers, barrel, &rot, base_t, round_damage(t), false, 8192, 2, 2, 0, 0);
        } else if ray_hits_player(m, movers, barrel, end) {
            PENDING_PLAYER_DAMAGE = PENDING_PLAYER_DAMAGE.saturating_add(round_damage(t) as u16);
            note_damage_direction(barrel);
        }
    }
    TANK_SKIP = -1;
    logic_fire_targets(m, m.n_logic.min(MAX_LOGIC), m.n_ents, t.target, map::USE_TOGGLE, SIM_NOW, 0, t.li);
}

/// Write a tank's angles into its brush pose (render + collision) the way
/// the cooker packs retained angles: reflected q8 turns per Euler axis.
#[inline(never)]
#[optimize(size)]
unsafe fn pose(t: &Tank) {
    let ei = t.ei as usize;
    let q8 = |a: i16| ((-(a as i32) + 8) >> 4) as u32 & 0xff;
    let packed = q8(t.ang[1]) | (q8(t.ang[0]) << 8) | (ENT_CACHE[ei].mv[2] as u32 & 0x00ff_0000);
    if ENT_CACHE[ei].mv[2] as u32 != packed {
        ENT_CACHE[ei].mv[2] = packed as i32;
        ENT_PHASE[ei] ^= 1; // a new render pose: leave the static brush pass
        live_entity_pvs_mark_dirty(ei);
    }
}

/// CFuncTank::Think / TrackTarget for every tank of the map, at the SDK's
/// 10 Hz (20 Hz while the player controls it). `view` is the player's aim
/// direction, which the mounted tank mirrors.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_tanks(m: &Map, movers: &[phys::Mover], view: [i32; 3], now: u16) {
    TANK_FIRE_CD = TANK_FIRE_CD.saturating_sub(256);
    for i in 0..TANK_COUNT {
        think(m, movers, view, now, &mut *core::ptr::addr_of_mut!(TANKS[i]));
    }
}

#[inline(never)]
#[optimize(size)]
unsafe fn think(m: &Map, movers: &[phys::Mover], view: [i32; 3], now: u16, t: &mut Tank) {
    let controlled = MOUNTED_TANK == t.li as i32;
    // MOVETYPE_PUSH integrates the avelocity set by the last think.
    if t.avel != [0; 2] {
        t.ang[0] = ((t.ang[0] + t.avel[0]) & 0xfff) as i16;
        t.ang[1] += t.avel[1];
        pose(t);
    }
    if LOGIC_STATE[t.li as usize] == LOGIC_STATE_REMOVED
        || !(controlled || (t.flags & ON != 0 && time_reached(now, t.next)))
    {
        return;
    }
    t.avel = [0; 2];
    t.next = now.wrapping_add(2);
    TANK_SKIP = t.ei as i32;
    let o = ENT_CACHE[t.ei as usize].origin;
    let barrel = barrel(t);
    let mut update = false;
    let dir = if controlled {
        view
    } else {
        if !entity_touches_pvs(m, &ENT_CACHE[t.ei as usize]) {
            t.next = now.wrapping_add(40); // FIND_CLIENT_IN_PVS failed: 2 s
            return;
        }
        let p = LOGIC_PLAYER_POS;
        let eye = [p[0], p[1] + VIEW_HEIGHT, p[2]];
        let range = isqrt_i32(dist2_3(eye, barrel));
        if range < t.min_range as i32 || (t.max_range > 0 && range > t.max_range as i32) {
            return;
        }
        if LOGIC_PLAYER_HEALTH > 0 && phys::trace_line_skip(m, movers, barrel, eye, TANK_SKIP).is_none() {
            update = true;
            // BodyTarget: half way between the centre and the eyes.
            t.sight = [p[0] as i16, (p[1] + VIEW_HEIGHT / 2) as i16, p[2] as i16];
        }
        [t.sight[0] as i32 - o[0], t.sight[1] as i32 - o[1], t.sight[2] as i32 - o[2]]
    };
    let dist = isqrt_i32(dist2_3(dir, [0; 3]));
    let mut yaw = atan_s(dir[2], dir[0]);
    let mut pitch = -atan_s(dir[1], isqrt_i32(dist2_xz(dir, [0; 3])));
    let (by, bz) = (t.barrel[1] as i32, t.barrel[2] as i32);
    if !controlled && (by | bz) != 0 {
        // AdjustAnglesForBarrel: aim the offset barrel, not the pivot.
        let d2 = (dist - bz).saturating_mul(dist - bz);
        yaw += atan_s(by, isqrt_i32((d2 - by * by).max(0)));
        pitch -= atan_s(-bz, isqrt_i32((d2 - bz * bz).max(0)));
    }
    let (yc, pc) = (t.centre[0] as i32, t.centre[1] as i32);
    yaw = yc + angle_dist(yaw, yc);
    let yr = t.range[0] as i32;
    if yaw > yc + yr || yaw < yc - yr {
        yaw = yaw.clamp(yc - yr, yc + yr);
        update = false; // saw the player, but out of the yaw range
    }
    if update {
        t.sight_time = now;
    }
    let pr = t.range[1] as i32;
    pitch = (pc + angle_dist(pitch, pc)).clamp(pc - pr, pc + pr);
    // Move toward the target at the rate or less: avelocity = dist * 10 /s.
    let dy = angle_dist(yaw, t.ang[0] as i32);
    let dx = angle_dist(pitch, t.ang[1] as i32);
    t.avel = [
        (dy / 2).clamp(-(t.rate[0] as i32), t.rate[0] as i32) as i16,
        (dx / 2).clamp(-(t.rate[1] as i32), t.rate[1] as i32) as i16,
    ];
    if controlled {
        return;
    }
    let los = t.flags & LOS != 0;
    let aimed = dx.abs() < t.tol[1] as i32 && dy.abs() < t.tol[0] as i32;
    if now.wrapping_sub(t.sight_time) < t.persist as u16 && (aimed || los) {
        // SF_TANK_LINEOFSIGHT: only when the barrel's own line reaches the player.
        let f = vectors(t.ang)[0];
        let end = [barrel[0] + ((f[0] * dist) >> 12), barrel[1] + ((f[1] * dist) >> 12), barrel[2] + ((f[2] * dist) >> 12)];
        if !los || ray_hits_player(m, movers, barrel, end) {
            // The round count is the elapsed time times the fire rate; the
            // first think in the cone only starts the clock.
            let last = t.fire_last;
            t.fire_last = now.max(1);
            if last != 0 {
                let count = (now.wrapping_sub(last) as u32 * t.fire_rate as u32 / (20 * 256)) as i32;
                if count > 0 {
                    fire(m, movers, t, count, false);
                } else {
                    t.fire_last = last;
                }
            }
            return;
        }
    }
    t.fire_last = 0;
}

/// CFuncTankControls::Use: a tank whose func_tankcontrols volume the player
/// stands in (padded by 32 units for the use reach) takes them, master
/// permitting (CFuncTank::StartControl).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_try_control(m: &Map, nlogic: usize, p: [i32; 3]) -> bool {
    for t in TANKS[..TANK_COUNT].iter() {
        let mut k = map::TANK_AUX_COUNT as usize;
        while t.flags & CONTROL != 0 && k + 3 <= t.aux_count as usize {
            let mut b = [0i32; 6];
            for i in 0..3 {
                let a = m.logic_aux(t.aux as usize + k + i);
                b[i * 2] = a.target as i16 as i32;
                b[i * 2 + 1] = a.delay_ticks as i16 as i32;
            }
            if (0..3).all(|c| p[c] >= b[c] - 32 && p[c] <= b[c + 3] + 32) {
                if master_ok(m, nlogic, m.logic(t.li as usize).arg1) {
                    MOUNTED_TANK = t.li as i32;
                    TANK_USE_POS = p;
                    TANK_FIRE_CD = 0;
                    sfx::play(sfx::BUTTON);
                } else {
                    sfx::play(sfx::DRY);
                }
                return true;
            }
            k += 3;
        }
    }
    false
}

/// CFuncTank::OnControls: the player keeps the tank while within 30 units
/// of where they took it.
#[optimize(size)]
pub(crate) unsafe fn tank_still_controlled(p: [i32; 3]) -> bool {
    dist2_3(p, TANK_USE_POS) < 30 * 30
}

/// CFuncTank::ControllerPostFrame: one round of the tank's class along its
/// own angles, then m_flNextAttack = now + 1 / firerate. TANK_FIRE_CD keeps
/// the wait in 1/256 ticks.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_player_fire(m: &Map, movers: &[phys::Mover]) -> bool {
    let Some(t) = slot(MOUNTED_TANK as usize) else {
        return false;
    };
    if TANK_FIRE_CD >= 256 {
        return false;
    }
    TANK_FIRE_CD += (20 * 256 * 256 / t.fire_rate as u32).min(u16::MAX as u32 - 256) as u16;
    if t.kind >> map::TANK_CLASS_SHIFT == 0 {
        sfx::play(sfx::MP5);
    }
    fire(m, movers, t, 1, true);
    true
}
