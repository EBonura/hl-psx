//! One-shot authored funnel effects own their particles independently of debris.
use crate::*;
static mut BURSTS: [env_funnel::Burst; env_funnel::BURSTS] =
    [env_funnel::Burst::ZERO; env_funnel::BURSTS];

#[inline(never)]
#[optimize(size)]
pub unsafe fn emit(m: &Map, rec: map::LogicEnt) -> bool {
    if rec.aux_count < 2 || sprite::def(rec.arg1 as usize).n_frames == 0 {return false;}
    let scale = m.logic_aux(rec.first_aux).target;
    let mut burst = env_funnel::Burst::ZERO;
    burst.origin = rec.origin;
    burst.scale_q12 = scale;
    burst.sprite = rec.arg1 as u8;
    burst.reverse = rec.spawnflags & 1 != 0;
    // The retail speed depends on absolute source height, before map scaling.
    let source_y = m.logic_aux(rec.first_aux + 1).target as i16 as i32;
    for index in 0..env_funnel::PARTICLES {
        let height = 100 + impact_rng().below(701) as u16;
        let speed = 64 + impact_rng().below(65) as u16;
        let Some(flight) = env_funnel::Flight::new(index,height,speed,source_y,burst.reverse,scale) else {return false;};
        burst.flights[index] = flight;
    }
    env_funnel::admit(&mut *core::ptr::addr_of_mut!(BURSTS),burst)
}
pub unsafe fn clear() {BURSTS = [env_funnel::Burst::ZERO;env_funnel::BURSTS];}
pub unsafe fn tick() {
    for burst in &mut *core::ptr::addr_of_mut!(BURSTS) {burst.advance();}
}
pub unsafe fn render(packets: &mut PrimitivePacketArena<'_>, np: &mut usize,
                     rot: &Mat3I16, base_t: [i32;3]) {
    for burst in &*core::ptr::addr_of!(BURSTS) {
        if burst.sprite == 255 {continue;}
        let d = sprite::def(burst.sprite as usize);
        let half_w = d.base_w as i32 * burst.scale_q12 as i32 / 8192;
        let half_h = d.base_h as i32 * burst.scale_q12 as i32 / 8192;
        for (index,flight) in burst.flights.iter().enumerate() {
            let opacity = flight.opacity(burst.age);
            if opacity == 0 {continue;}
            let tint = (opacity as u16 * 128 / 255) as u8;
            emit_billboard_rgb_mode(packets,np,
                flight.position_q4(burst.origin,index,burst.reverse,burst.scale_q12,burst.age).map(|p|p>>4),
                half_w,half_h,sprite::slot_for(burst.sprite as usize,0),
                d.crush_w,d.crush_h,rot,base_t,(tint,tint,tint),BlendMode::Add);
        }
    }
}
