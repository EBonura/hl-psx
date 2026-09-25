//! func_tank family (func_tank.cpp): CFuncTank::TrackTarget / Fire for the
//! AI guns, mortars, rockets and lasers, and the player-controlled tanks
//! that func_tankcontrols hand over.

use crate::*;

// ---- func_tank AI (func_tank.cpp CFuncTank::TrackTarget/Fire) ----
// Angles are pev->angles in q12 turns: [yaw, pitch] with GoldSrc's brush
// pitch sign (positive = barrel down). Rates are q12 per 20 Hz tick. The
// cooked keys live in seven aux words (hl_format::logic::TANK).
pub(crate) const MAX_TANKS: usize = 6;
pub(crate) const SF_TANK_ACTIVE: u16 = 1;
pub(crate) const SF_TANK_LINEOFSIGHT: u16 = 0x10;
pub(crate) const SF_TANK_CANCONTROL: u16 = 0x20;
pub(crate) static mut TANK_COUNT: usize = 0;
pub(crate) static mut TANK_LI: [u16; MAX_TANKS] = [0; MAX_TANKS];
pub(crate) static mut TANK_EI: [u16; MAX_TANKS] = [0; MAX_TANKS];
pub(crate) static mut TANK_AUX: [u16; MAX_TANKS] = [0; MAX_TANKS];
pub(crate) static mut TANK_TARGET: [u16; MAX_TANKS] = [0; MAX_TANKS];
pub(crate) static mut TANK_LOS: u8 = 0; // SF_TANK_LINEOFSIGHT per tank slot
pub(crate) static mut TANK_ANG: [[i16; 2]; MAX_TANKS] = [[0; 2]; MAX_TANKS];
pub(crate) static mut TANK_AVEL: [[i16; 2]; MAX_TANKS] = [[0; 2]; MAX_TANKS];
pub(crate) static mut TANK_SIGHT: [[i16; 3]; MAX_TANKS] = [[0; 3]; MAX_TANKS];
pub(crate) static mut TANK_FIRE_LAST: [u16; MAX_TANKS] = [0; MAX_TANKS]; // 0 = not firing
pub(crate) static mut TANK_SIGHT_TIME: [u16; MAX_TANKS] = [0; MAX_TANKS];
pub(crate) static mut TANK_NEXT: [u16; MAX_TANKS] = [0; MAX_TANKS];
pub(crate) static mut TANK_ON: u8 = 0; // SF_TANK_ACTIVE per tank slot
pub(crate) static mut TANK_SKIP: i32 = -1; // the thinking tank's brush: traces ignore it

/// One shared out-of-line decode of a logic record for cold paths.
#[inline(never)]
pub(crate) fn logic_cold(m: &Map, li: usize) -> map::LogicEnt {
    m.logic(li)
}

/// atan2 in q12 turns from +x toward +y, within about 0.25 degrees
/// (octant fold plus the t*(pi/4) + 0.273*t*(1-t) arctangent fit), signed.
#[inline(never)]
#[optimize(size)]
pub(crate) fn atan2_signed(y: i32, x: i32) -> i32 {
    let (mut ax, mut ay) = (x.unsigned_abs(), y.unsigned_abs());
    while ax >= 1 << 19 || ay >= 1 << 19 {
        ax >>= 4;
        ay >>= 4;
    }
    let (lo, hi) = if ax >= ay { (ay, ax) } else { (ax, ay) };
    let t = (lo << 12) / hi.max(1);
    let a = ((t * 512 + 178 * t * (4096 - t) / 4096) / 4096) as i32;
    let q = if ax >= ay { a } else { 1024 - a };
    let q = if x < 0 { 2048 - q } else { q };
    if y < 0 {
        -q
    } else {
        q
    }
}

/// UTIL_AngleDistance in q12 turns: `a - b` wrapped to -2048..2047.
#[inline(always)]
pub(crate) fn angle_dist_q12(a: i32, b: i32) -> i32 {
    ((a - b + 2048) & 0xfff) - 2048
}

/// UTIL_MakeVectors(pev->angles) as world rows [forward, right, up].
#[inline(never)]
#[optimize(size)]
pub(crate) fn tank_vectors(ang: [i16; 2]) -> [[i32; 3]; 3] {
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

/// The tank's seven cooked key words, as (target, delay_ticks) pairs.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_params(m: &Map, t: usize) -> [[u16; 2]; 7] {
    let mut out = [[0u16; 2]; 7];
    let mut k = 0;
    while k < 7 {
        let a = m.logic_aux(TANK_AUX[t] as usize + k);
        out[k] = [a.target, a.delay_ticks];
        k += 1;
    }
    out
}

/// CFuncTank::BarrelPosition.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_barrel(m: &Map, t: usize) -> [i32; 3] {
    let k = tank_params(m, t);
    let b = [k[3][0] as i16 as i32, k[3][1] as i16 as i32, k[4][0] as i16 as i32];
    let v = tank_vectors(TANK_ANG[t]);
    let mut p = ENT_CACHE[TANK_EI[t] as usize].origin;
    let mut r = 0;
    while r < 3 {
        let mut c = 0;
        while c < 3 {
            p[c] += (v[r][c] * b[r]) >> 12;
            c += 1;
        }
        r += 1;
    }
    p
}

/// A tank's bullet damage: bullet_damage, else FireBullets' skill.cfg value
/// for its BULLET_MONSTER_9MM/MP5/12MM (model types 1, 8 and 21 carry them).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_damage(m: &Map, t: usize) -> u8 {
    let k = tank_params(m, t);
    if k[4][1] != 0 {
        return k[4][1].min(255) as u8;
    }
    skill_damage([1, 1, 8, 21][(k[2][1] >> 8) as usize & 3]).unwrap_or(8)
}

/// Reset the map's tanks (CFuncTank::Spawn): centred, and active ones think
/// after a second.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tanks_init(m: &Map, now: u16) {
    TANK_COUNT = 0;
    TANK_ON = 0;
    TANK_LOS = 0;
    let nlogic = m.n_logic.min(MAX_LOGIC);
    let mut li = 0usize;
    while li < nlogic && TANK_COUNT < MAX_TANKS {
        if LOGIC_KIND[li] != map::LOGIC_TANK {
            li += 1;
            continue;
        }
        let rec = logic_cold(m, li);
        if rec.aux_count >= map::TANK_AUX_COUNT as usize {
            let t = TANK_COUNT;
            TANK_LI[t] = li as u16;
            TANK_EI[t] = rec.brush;
            TANK_AUX[t] = rec.first_aux as u16;
            TANK_TARGET[t] = rec.target;
            TANK_LOS |= ((rec.spawnflags & SF_TANK_LINEOFSIGHT != 0) as u8) << t;
            let c = tank_params(m, t)[6];
            TANK_ANG[t] = [c[0] as i16, ((c[1] << 4) as i16) >> 4];
            TANK_AVEL[t] = [0; 2];
            TANK_FIRE_LAST[t] = 0;
            // The map clock is 1.0 s here and m_lastSightTime is 0, so
            // CanFire holds for the first `persistence` seconds even blind
            // (c2a2d's siloguardgun fires four rounds on load). Spawn's
            // ltime + 1.0 think lands 0.75 s in: SV_ActivateServer's two
            // 0.1 s settle frames already advanced ltime.
            TANK_SIGHT_TIME[t] = now.wrapping_sub(20);
            TANK_NEXT[t] = now.wrapping_add(15);
            TANK_ON |= ((rec.spawnflags & SF_TANK_ACTIVE) as u8) << t;
            let b = tank_barrel(m, t);
            TANK_SIGHT[t] = [b[0] as i16, b[1] as i16, b[2] as i16];
            TANK_COUNT += 1;
        }
        li += 1;
    }
}

#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_slot(li: usize) -> Option<usize> {
    (0..TANK_COUNT).find(|&t| TANK_LI[t] as usize == li)
}

/// CFuncTank::Use for a tank the player cannot control: ShouldToggle, then
/// TankActivate / TankDeactivate (a dying gunner's TriggerTarget stops it).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_use(li: usize, use_type: u8, now: u16) {
    let Some(t) = tank_slot(li) else { return };
    let on = TANK_ON & (1 << t) != 0;
    if (use_type == map::USE_ON && on) || (use_type == map::USE_OFF && !on) {
        return;
    }
    TANK_ON ^= 1 << t;
    TANK_FIRE_LAST[t] = 0;
    TANK_NEXT[t] = now.wrapping_add(2);
}

/// Does `from -> end` reach the player's box before the world?
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn ray_hits_player(m: &Map, movers: &[phys::Mover], from: [i32; 3], end: [i32; 3]) -> bool {
    let p = LOGIC_PLAYER_POS;
    let h = PLAYER_HULL_HALF_HEIGHT;
    let wall = phys::trace_line_skip(m, movers, from, end, TANK_SKIP).map_or(4096, |hit| hit.frac);
    LOGIC_PLAYER_HEALTH > 0
        && segment_box_frac(from, end, [p[0] - 16, p[1] - h, p[2] - 16], [p[0] + 16, p[1] + h, p[2] + 16])
            .is_some_and(|f| f <= wall)
}

/// CFuncTank*::Fire. TANK_FIRE_LAST is m_fireLast (0 until the first think
/// in the cone, which only starts the clock); the round count is the elapsed
/// time times the fire rate, and the tank's target fires with the rounds.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_fire(m: &Map, nlogic: usize, nents: usize, movers: &[phys::Mover], t: usize, now: u16) {
    let last = TANK_FIRE_LAST[t];
    TANK_FIRE_LAST[t] = now.max(1);
    if last == 0 {
        return;
    }
    let k = tank_params(m, t);
    let count = (now.wrapping_sub(last) as u32 * k[2][0] as u32 / (20 * 256)) as i32;
    if count <= 0 {
        TANK_FIRE_LAST[t] = last;
        return;
    }
    let flags = k[2][1] >> 8;
    let spread = [0, 102, 205, 410, 1024][((flags >> 2) & 7).min(4) as usize];
    let v = tank_vectors(TANK_ANG[t]);
    let barrel = tank_barrel(m, t);
    let class = flags >> map::TANK_CLASS_SHIFT;
    let mut i = 0;
    while i < count {
        if class == map::TANK_CLASS_ROCKET {
            spawn_projectile_dir(PROJ_ROCKET, 100, barrel, v[0], true);
        } else if class == map::TANK_CLASS_MORTAR || class == map::TANK_CLASS_LASER || flags & 3 != 0 {
            // TankTrace / FireBullets: gTankSpread cone, 4096 units.
            let mut x = 0;
            let mut end = barrel;
            while x < 2 {
                let r = (IMPACT_RNG.below(4097) as i32 + IMPACT_RNG.below(4097) as i32 - 4096) * spread >> 12;
                let mut c = 0;
                while c < 3 {
                    end[c] += if x == 0 { v[0][c] } else { 0 } + ((r * v[x + 1][c]) >> 12);
                    c += 1;
                }
                x += 1;
            }
            let impact = phys::trace_line_skip(m, movers, barrel, end, TANK_SKIP).map_or(end, |h| h.pos);
            if class == map::TANK_CLASS_MORTAR {
                // Only one explosion, iMagnitude damage out to 2.5x.
                let mag = k[4][1].min(255) as i32;
                explode(m, impact, mag as u8, mag * 5 / 2, false);
                break;
            }
            if ray_hits_player(m, movers, barrel, end) {
                PENDING_PLAYER_DAMAGE = PENDING_PLAYER_DAMAGE.saturating_add(tank_damage(m, t) as u16);
                note_damage_direction(barrel);
            }
            push_tracer(barrel, impact);
        }
        i += 1;
    }
    logic_fire_targets(m, nlogic, nents, TANK_TARGET[t], map::USE_TOGGLE, now, 0, TANK_LI[t]);
}

/// Write a tank's angles into its brush pose (render + collision) the way
/// the cooker packs retained angles: reflected q8 turns per Euler axis.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_pose(t: usize) {
    let ei = TANK_EI[t] as usize;
    let q8 = |a: i16| ((-(a as i32) + 8) >> 4) as u32 & 0xff;
    let packed = q8(TANK_ANG[t][1]) | (q8(TANK_ANG[t][0]) << 8) | (ENT_CACHE[ei].mv[2] as u32 & 0x00ff_0000);
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
pub(crate) unsafe fn tick_tanks(m: &Map, nlogic: usize, nents: usize, movers: &[phys::Mover], view: [i32; 3], now: u16) {
    TANK_FIRE_CD = TANK_FIRE_CD.saturating_sub(256);
    let mut t = 0;
    while t < TANK_COUNT {
        tank_think(m, nlogic, nents, movers, view, now, t);
        t += 1;
    }
}

#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_think(m: &Map, nlogic: usize, nents: usize, movers: &[phys::Mover], view: [i32; 3], now: u16, t: usize) {
    let li = TANK_LI[t] as usize;
    let controlled = MOUNTED_TANK == li as i32;
    // MOVETYPE_PUSH integrates the avelocity set by the last think.
    if TANK_AVEL[t] != [0; 2] {
        TANK_ANG[t][0] = ((TANK_ANG[t][0] + TANK_AVEL[t][0]) & 0xfff) as i16;
        TANK_ANG[t][1] += TANK_AVEL[t][1];
        tank_pose(t);
    }
    if LOGIC_STATE[li] == LOGIC_STATE_REMOVED
        || !(controlled || (TANK_ON & (1 << t) != 0 && time_reached(now, TANK_NEXT[t])))
    {
        return;
    }
    TANK_AVEL[t] = [0; 2];
    TANK_NEXT[t] = now.wrapping_add(2);
    let k = tank_params(m, t);
    TANK_SKIP = TANK_EI[t] as i32;
    let o = ENT_CACHE[TANK_EI[t] as usize].origin;
    let barrel = tank_barrel(m, t);
    let mut update = false;
    let dir = if controlled {
        view
    } else {
        if !entity_touches_pvs(m, &ENT_CACHE[TANK_EI[t] as usize]) {
            TANK_NEXT[t] = now.wrapping_add(40); // FIND_CLIENT_IN_PVS failed: 2 s
            return;
        }
        let p = LOGIC_PLAYER_POS;
        let eye = [p[0], p[1] + VIEW_HEIGHT, p[2]];
        let range = isqrt_i32(dist2_3(eye, barrel));
        if range < k[5][0] as i16 as i32 || (k[5][1] as i16 > 0 && range > k[5][1] as i16 as i32) {
            return;
        }
        if LOGIC_PLAYER_HEALTH > 0 && phys::trace_line_skip(m, movers, barrel, eye, TANK_SKIP).is_none() {
            update = true;
            // BodyTarget: half way between the centre and the eyes.
            TANK_SIGHT[t] = [p[0] as i16, (p[1] + VIEW_HEIGHT / 2) as i16, p[2] as i16];
        }
        let s = TANK_SIGHT[t];
        [s[0] as i32 - o[0], s[1] as i32 - o[1], s[2] as i32 - o[2]]
    };
    let dist = isqrt_i32(dist2_3(dir, [0; 3]));
    let mut yaw = atan2_signed(dir[2], dir[0]);
    let mut pitch = -atan2_signed(dir[1], isqrt_i32(dist2_xz(dir, [0; 3])));
    let (by, bz) = (k[3][1] as i16 as i32, k[4][0] as i16 as i32);
    if !controlled && (by | bz) != 0 {
        // AdjustAnglesForBarrel: aim the offset barrel, not the pivot.
        let d2 = (dist - bz).saturating_mul(dist - bz);
        yaw += atan2_signed(by, isqrt_i32((d2 - by * by).max(0)));
        pitch -= atan2_signed(-bz, isqrt_i32((d2 - bz * bz).max(0)));
    }
    let (yc, pc) = (k[6][0] as i32, ((k[6][1] << 4) as i16 >> 4) as i32);
    yaw = yc + angle_dist_q12(yaw, yc);
    let yr = k[0][1] as i32;
    if yaw > yc + yr || yaw < yc - yr {
        yaw = yaw.clamp(yc - yr, yc + yr);
        update = false; // saw the player, but out of the yaw range
    }
    if update {
        TANK_SIGHT_TIME[t] = now;
    }
    pitch = (pc + angle_dist_q12(pitch, pc)).clamp(pc - k[1][0] as i32, pc + k[1][0] as i32);
    // Move toward the target at the rate or less: avelocity = dist * 10 /s.
    let dy = angle_dist_q12(yaw, TANK_ANG[t][0] as i32);
    let dx = angle_dist_q12(pitch, TANK_ANG[t][1] as i32);
    let (yrate, prate) = ((k[0][0] & 0xff) as i32, (k[0][0] >> 8) as i32);
    TANK_AVEL[t] = [(dy / 2).clamp(-yrate, yrate) as i16, (dx / 2).clamp(-prate, prate) as i16];
    if controlled {
        return;
    }
    let los = TANK_LOS & (1 << t) != 0;
    let aimed = dx.abs() < (k[1][1] >> 8) as i32 && dy.abs() < (k[1][1] & 0xff) as i32;
    if now.wrapping_sub(TANK_SIGHT_TIME[t]) < k[2][1] & 0xff && (aimed || los) {
        // SF_TANK_LINEOFSIGHT: only when the barrel's own line reaches the player.
        let f = tank_vectors(TANK_ANG[t])[0];
        let end = [barrel[0] + ((f[0] * dist) >> 12), barrel[1] + ((f[1] * dist) >> 12), barrel[2] + ((f[2] * dist) >> 12)];
        if !los || ray_hits_player(m, movers, barrel, end) {
            tank_fire(m, nlogic, nents, movers, t, now);
            return;
        }
    }
    TANK_FIRE_LAST[t] = 0;
}

/// Where the player took the controls (m_vecControllerUsePos).
pub(crate) static mut TANK_USE_POS: [i32; 3] = [0; 3];

/// CFuncTankControls::Use: the tank whose func_tankcontrols volume the
/// player stands in (padded by 32 units for the use reach), if any.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_controls_at(m: &Map, p: [i32; 3]) -> Option<usize> {
    for t in 0..TANK_COUNT {
        let li = TANK_LI[t] as usize;
        let rec = logic_cold(m, li);
        let mut k = map::TANK_AUX_COUNT as usize;
        while k + 3 <= rec.aux_count {
            let w = |i: usize| {
                let a = m.logic_aux(rec.first_aux + k + i / 2);
                (if i % 2 == 0 { a.target } else { a.delay_ticks }) as i16 as i32
            };
            if (0..3).all(|c| p[c] >= w(c) - 32 && p[c] <= w(c + 3) + 32) && rec.spawnflags & SF_TANK_CANCONTROL != 0 {
                return Some(li);
            }
            k += 3;
        }
    }
    None
}

/// CFuncTank::StartControl (master-gated) from a func_tankcontrols use.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_try_control(m: &Map, nlogic: usize, p: [i32; 3]) -> bool {
    let Some(li) = tank_controls_at(m, p) else {
        return false;
    };
    if master_ok(m, nlogic, m.logic(li).arg1) {
        MOUNTED_TANK = li as i32;
        TANK_USE_POS = p;
        TANK_FIRE_CD = 0;
        sfx::play(sfx::BUTTON);
    } else {
        sfx::play(sfx::DRY);
    }
    true
}

/// CFuncTank::OnControls: the player keeps the tank while within 30 units
/// of where they took it.
pub(crate) unsafe fn tank_still_controlled(p: [i32; 3]) -> bool {
    dist2_3(p, TANK_USE_POS) < 30 * 30
}

/// CFuncTank::ControllerPostFrame: one round of the tank's class along its
/// own angles, then m_flNextAttack = now + 1 / firerate. TANK_FIRE_CD keeps
/// the wait in 1/256 ticks.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tank_player_fire(m: &Map, movers: &[phys::Mover]) -> bool {
    let Some(t) = tank_slot(MOUNTED_TANK as usize) else {
        return false;
    };
    if TANK_FIRE_CD >= 256 {
        return false;
    }
    let k = tank_params(m, t);
    TANK_FIRE_CD += (20 * 256 * 256 / k[2][0].max(1) as u32).min(u16::MAX as u32 - 256) as u16;
    let barrel = tank_barrel(m, t);
    let v = tank_vectors(TANK_ANG[t]);
    let class = (k[2][1] >> 8) >> map::TANK_CLASS_SHIFT;
    TANK_SKIP = TANK_EI[t] as i32;
    if class == map::TANK_CLASS_ROCKET {
        // CFuncTankRocket::Fire: an rpg_rocket owned by the tank.
        spawn_projectile_dir(PROJ_ROCKET, 100, barrel, v[0], false);
    } else if class == map::TANK_CLASS_MORTAR {
        // CFuncTankMortar::Fire: one explosion of iMagnitude at the trace end.
        let far = [barrel[0] + v[0][0], barrel[1] + v[0][1], barrel[2] + v[0][2]]; // 4096 units
        let end = phys::trace_line_skip(m, movers, barrel, far, TANK_SKIP).map_or(far, |h| h.pos);
        let mag = k[4][1].min(255) as i32;
        explode(m, end, mag as u8, mag * 5 / 2, true);
        push_tracer_styled(barrel, end, TRACER_BULLET);
    } else {
        // CFuncTankGun::Fire: FireBullets along the barrel.
        let fire_rot = view_rotation((1024 - TANK_ANG[t][0] as i32) as u16 & 0xfff, -TANK_ANG[t][1]);
        let base_t = [-dot12(fire_rot.m[0], barrel), -dot12(fire_rot.m[1], barrel), -dot12(fire_rot.m[2], barrel)];
        fire_hitscan(m, movers, barrel, &fire_rot, base_t, tank_damage(m, t), false, 8192, 2, 2, 0, 0);
        sfx::play(sfx::MP5);
    }
    TANK_SKIP = -1;
    logic_fire_targets(m, m.n_logic.min(MAX_LOGIC), m.n_ents, TANK_TARGET[t], map::USE_TOGGLE, SIM_NOW, 0, TANK_LI[t]);
    true
}
