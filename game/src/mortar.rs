//! func_mortar_field and monster_mortar (mortar.cpp): a Use drops
//! `m_iCount` shells around a point of the field, each exploding 2.5 s
//! later plus 0.2-0.5 s per shell.

use crate::*;

const MAX_SHELLS: usize = 12;
/// A falling shell: world position and the tick it explodes on; tick 0
/// frees the slot.
static mut SHELL_POS: [[i32; 3]; MAX_SHELLS] = [[0; 3]; MAX_SHELLS];
static mut SHELL_AT: [u16; MAX_SHELLS] = [0; MAX_SHELLS];
/// Whether the player's Use sent each shell (the player as owner).
static mut SHELL_PLAYER: u16 = 0;

pub(crate) unsafe fn reset() {
    SHELL_AT = [0; MAX_SHELLS];
}

/// Position (0..4096) of the momentary_rot_button named `name`
/// (CMomentaryRotButton's ideal_yaw), if the map has one.
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
                return logic_valid_brush(rec.brush, m.n_ents).map(|ei| ENT_PHASE[ei].clamp(0, 4096));
            }
        }
    }
    None
}

/// CFuncMortarField::FieldUse.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn field_use(m: &Map, nlogic: usize, rec: map::LogicEnt, now: u16) {
    let (mn, mx) = (rec.mins, rec.maxs);
    let span = |a: i32, b: i32| a + (IMPACT_RNG.below((b - a).max(0) as u32 + 1) as i32);
    // Random spot in the field, at its top (HL x/y are runtime x/z).
    let mut start = [span(mn[0], mx[0]), mx[1], span(mn[2], mx[2])];
    let player = LOGIC_ACTIVATOR == 1;
    match rec.speed >> 8 {
        // Trigger activator: over whoever set it off.
        1 if player => {
            start[0] = LOGIC_PLAYER_POS[0];
            start[2] = LOGIC_PLAYER_POS[2];
        }
        // Table: the x/y controllers place it across the field.
        2 => {
            if let Some(f) = controller(m, nlogic, rec.arg0) {
                start[0] = mn[0] + (((mx[0] - mn[0]) * f) >> 12);
            }
            if let Some(f) = controller(m, nlogic, rec.arg1) {
                start[2] = mn[2] + (((mx[2] - mn[2]) * f) >> 12);
            }
        }
        _ => {}
    }
    let spread = if rec.aux_count != 0 { m.logic_aux(rec.first_aux).target as i32 } else { 0 };
    let movers = &*core::ptr::slice_from_raw_parts(core::ptr::addr_of!(MOVERS).cast::<phys::Mover>(), MOVER_COUNT.min(MAX_ENTS + 1));
    let mut t = 50u16; // 2.5 s
    for _ in 0..(rec.speed & 0xff) {
        let spot = [start[0] + span(-spread, spread), start[1], start[2] + span(-spread, spread)];
        let down = [spot[0], spot[1] - 4096, spot[2]];
        let ground = phys::trace_line(m, movers, spot, down).map_or(down, |h| h.pos);
        if let Some(s) = (0..MAX_SHELLS).find(|&s| SHELL_AT[s] == 0) {
            SHELL_POS[s] = ground;
            SHELL_AT[s] = now.wrapping_add(t).max(1);
            SHELL_PLAYER = (SHELL_PLAYER & !(1 << s)) | ((player as u16) << s);
        }
        t += 4 + IMPACT_RNG.below(7) as u16; // RANDOM_FLOAT(0.2, 0.5)
    }
}

/// CMortar::MortarExplode for every shell whose time has come: the 1024-unit
/// lgtning column, a 200-damage DMG_BLAST | DMG_MORTAR explosion (radius
/// 2.5x) and UTIL_ScreenShake(25, 1 s, 750).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick(m: &Map, now: u16) {
    for s in 0..MAX_SHELLS {
        if SHELL_AT[s] != 0 && time_reached(now, SHELL_AT[s]) {
            SHELL_AT[s] = 0;
            let p = SHELL_POS[s];
            push_tracer_styled(p, [p[0], p[1] + 1024, p[2]], TRACER_MORTAR);
            explode(m, p, 200, 500, SHELL_PLAYER & (1 << s) != 0);
            let d = isqrt_i32(dist2_3(p, LOGIC_PLAYER_POS));
            if d < 750 {
                SHAKE_AMP = (25 * (750 - d) / 750) as u16;
                SHAKE_DUR = 20;
                SHAKE_TICKS = 20;
            }
        }
    }
}
