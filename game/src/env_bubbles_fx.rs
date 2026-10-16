//! Authored brush bubble volumes use the existing bounded world debris pool.
//! Retail fizz keeps its allocator-zeroed sine multiplier; current/rise is linear.
use crate::*;

#[inline(never)]
#[optimize(size)]
pub unsafe fn emit(m: &Map, rec: map::LogicEnt) {
    let id = rec.arg1 as usize;
    let def = sprite::def(id);
    if def.n_frames == 0 || rec.aux_count < 2 {
        return;
    }
    let current = m.logic_aux(rec.first_aux);
    let world_scale = m.logic_aux(rec.first_aux + 1).target as i32;
    if world_scale == 0 {
        return;
    }
    for _ in 0..env_bubbles::burst_count(rec.arg0 as u8) {
        let x = rec.mins[0] + impact_rng().below((rec.maxs[0] - rec.mins[0]).max(1) as u32) as i32;
        let z = rec.mins[2] + impact_rng().below((rec.maxs[2] - rec.mins[2]).max(1) as u32) as i32;
        let rise = 80 + impact_rng().below(61) as i32;
        let life =
            env_bubbles::lifetime_ticks((rec.maxs[1] - rec.mins[1]) * 4096 / world_scale, rise);
        let slot = DEBRIS_CURSOR % MAX_DEBRIS;
        spawn_debris_fixed(
            [x * 16, rec.mins[1] * 16, z * 16],
            env_bubbles::velocity_q4(
                [current.target as i16, current.delay_ticks as i16],
                rise,
                world_scale,
            ),
            DEBRIS_BUBBLE,
            1,
        );
        BUBBLE_TTL[slot] = life;
        DEBRIS[slot].blood_sprite = id as u8;
        DEBRIS[slot].blood_frame = impact_rng().below(def.n_frames as u32) as u8;
        let divisor_q8 = 512 + impact_rng().below(769) as i32;
        DEBRIS[slot].blood_half_q4 =
            env_bubbles::half_width_q4(def.base_w as i32, world_scale, divisor_q8);
    }
}

pub unsafe fn render(
    packets: &mut PrimitivePacketArena<'_>,
    np: &mut usize,
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    for bubble in (*core::ptr::addr_of!(DEBRIS)).iter() {
        if bubble.ttl == 0 || bubble.kind != DEBRIS_BUBBLE {
            continue;
        }
        let d = sprite::def(bubble.blood_sprite as usize);
        let half = bubble.blood_half_q4 as i32 / 16;
        emit_billboard_rgb_mode(
            packets,
            np,
            bubble.pos.map(|p| p >> 4),
            half,
            half,
            sprite::slot_for(bubble.blood_sprite as usize, bubble.blood_frame as usize),
            d.crush_w,
            d.crush_h,
            rot,
            base_t,
            (128, 128, 128),
            BlendMode::Average,
        );
    }
}
