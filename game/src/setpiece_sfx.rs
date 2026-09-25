//! Set-piece monster sounds (gargantua, Apache, Osprey, Nihilanth). Each map's
//! audio bank carries the samples of the set pieces it places
//! (hl_format::setpiece_audio); the cooker stores slot -> bank id on the map's
//! MAP_FLAGS record, bound here at map start. Silent when a bank lacks a slot.

use crate::sfx;
use hl_format::setpiece_audio;

/// This map's bank id for each set-piece sound slot (u8::MAX: not carried).
static mut SETPIECE: [u8; setpiece_audio::COUNT] = [u8::MAX; setpiece_audio::COUNT];
/// Loop owners for set pieces sit above every logic-record index.
pub const OWNER_GARG_FLAME: u16 = 0xFF00;
pub const OWNER_APACHE_ROTOR: u16 = 0xFF01;
pub const OWNER_OSPREY_ROTOR: u16 = 0xFF02;

/// Forget the previous map's set-piece ids (a map without them keeps none).
#[optimize(size)]
pub unsafe fn clear() {
    SETPIECE = [u8::MAX; setpiece_audio::COUNT];
}

/// Read the (slot, bank id) aux pairs the cooker stored on MAP_FLAGS.
#[inline(never)]
#[optimize(size)]
pub unsafe fn bind(m: &crate::map::Map, rec: crate::map::LogicEnt) {
    let mut k = 0usize;
    while k < rec.aux_count as usize {
        let a = m.logic_aux(rec.first_aux as usize + k);
        if (a.target as usize) < setpiece_audio::COUNT {
            SETPIECE[a.target as usize] = a.delay_ticks as u8;
        }
        k += 1;
    }
}

#[inline]
pub unsafe fn has(slot: u8) -> bool {
    SETPIECE[slot as usize] != u8::MAX
}

/// One-shot of a set-piece slot at a world position (silent when the map's
/// bank could not hold it).
#[inline(never)]
#[optimize(size)]
pub unsafe fn play(slot: u8, pos: [i32; 3]) {
    let id = SETPIECE[slot as usize];
    if id != u8::MAX {
        sfx::play_map_world(id, pos);
    }
}

/// Keep a set-piece loop running at `pos`: keyed once, then its level
/// follows the source's distance on the voice it holds (the map loop's own
/// 300-unit steps, set every call), re-keyed only if another loop took the
/// voice.
#[inline(never)]
#[optimize(size)]
pub unsafe fn keep_loop(slot: u8, pos: [i32; 3], owner: u16) {
    let id = SETPIECE[slot as usize];
    if id == u8::MAX {
        return;
    }
    if sfx::loop_voice(owner).is_none() {
        // Keyed at full level (authored attenuation 3: none), then set below.
        sfx::play_map_loop_authored(id, pos, owner, 100, 3);
    }
    let Some(v) = sfx::loop_voice(owner) else {
        return;
    };
    let d = [
        (pos[0] - sfx::EAR[0]).clamp(-16384, 16384),
        (pos[1] - sfx::EAR[1]).clamp(-16384, 16384),
        (pos[2] - sfx::EAR[2]).clamp(-16384, 16384),
    ];
    let dist = psx_math::int32::isqrt_i32(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
    let den = if dist >= 2400 { 16 } else { 1 + dist / 300 } as u16;
    let vol = psx_spu::Volume::linear(1, den);
    psx_spu::Voice::new(v).set_volume(vol, vol);
}

#[inline]
pub unsafe fn stop_loop(owner: u16) {
    sfx::stop_map_loop(owner);
}

/// Silence a loop between bursts but keep its voice, so a source that starts
/// and stops all fight long (the gargantua's flame) is keyed once per map
/// rather than rotating through the map-loop voices and evicting others.
#[inline(never)]
#[optimize(size)]
pub unsafe fn mute_loop(owner: u16) {
    if let Some(v) = sfx::loop_voice(owner) {
        psx_spu::Voice::new(v).set_volume(psx_spu::Volume::SILENCE, psx_spu::Volume::SILENCE);
    }
}
