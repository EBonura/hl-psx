//! Authored sound identifiers and the game-owned shared playback state.
use hl_format::weapon_audio as wa;

pub const CHUNK_ID: u32 = 3000;
pub const CHUNK_ID_LIGHT: u32 = 3050;
pub const CHUNK_ID_TRAINING_WEAPONS: u32 = 3051;
/// Per-map core profile table: "HCPT", u16 count, u16 pad, then one u16
/// chunk id per map index (the cooker's census of which core sounds a map's
/// entities can emit; 0 = no entry). Profiles live at 3053..=3099.
pub const CORE_TABLE_CHUNK: u32 = 3052;
/// Maps the core profile table can describe.
pub const CORE_TABLE_MAPS: usize = 128;

// ---- sample ids (host/hl-content `SOUNDS` order) ----
pub const GLOCK: u8 = 0;
pub const MP5: u8 = 1;
pub const SHOTGUN: u8 = 2;
pub const PYTHON: u8 = 3;
pub const XBOW: u8 = 4;
pub const GAUSS: u8 = 5;
pub const RPG: u8 = 6;
pub const CBAR_MISS: u8 = 7;
pub const CBAR_HIT: u8 = 8;
pub const EXPLODE: u8 = 9;
pub const RIC: u8 = 10;
pub const ELECTRO: u8 = 11;
pub const PAIN: u8 = 12;
pub const BODYDROP: u8 = 13;
#[allow(dead_code)] // completes the sound bank id table; no code names it
pub const DOOR_MOVE: u8 = 14;
#[allow(dead_code)] // completes the sound bank id table; no code names it
pub const DOOR_STOP: u8 = 15;
pub const BUTTON: u8 = 16;
pub const PICKUP: u8 = 17;
pub const SUIT: u8 = 18;
pub const HC_ATTACK: u8 = 19;
pub const ZO_ATTACK: u8 = 20;
pub const HE_BLAST: u8 = 21;
pub const GLASS_BREAK: u8 = 22;
pub const WOOD_BREAK: u8 = 23;
pub const MEDSHOT: u8 = 24;
pub const STEP1: u8 = 25;
pub const STEP2: u8 = 26;
pub const RELOAD: u8 = 27;
pub const DRY: u8 = 28;
pub const ZO_PAIN: u8 = 29;
pub const HC_PAIN: u8 = 30;
pub const HC_DIE: u8 = 31;
pub const GR_PAIN: u8 = 32;
pub const GR_DIE: u8 = 33;
pub const BA_PAIN: u8 = 34;
pub const BA_DIE: u8 = 35;
pub const HE_PAIN: u8 = 36;
pub const HE_DIE: u8 = 37;
pub const SLV_PAIN: u8 = 38;
pub const SLV_DIE: u8 = 39;
pub const BC_PAIN: u8 = 40;
pub const BC_DIE: u8 = 41;
#[allow(dead_code)] // completes the sound bank id table; no code names it
pub const HEV_BELL: u8 = 42;
pub const GEIGER: u8 = 43; // radiation/toxic-zone click
pub const HEV_ACTIVATE: u8 = 44; // suit power-on voice (pickup)
                                 // ids 45 and 46 held "health critical" and "near death"; hev.rs speaks them now
pub const CHARGER_HEALTH_LOOP: u8 = 47;
pub const CHARGER_HEV_LOOP: u8 = 48;
pub const FLASHLIGHT: u8 = 49;
pub const M203: u8 = 50;
pub const SHOTGUN_DOUBLE: u8 = 51;
pub const GAUSS_CHARGE: u8 = 52;
pub const AMMO_PICKUP: u8 = 53;
pub const HEALTHKIT: u8 = 54;
pub const HEALTH_DENY: u8 = 55;
pub const SUIT_DENY: u8 = 56;
pub const RELOAD_357: u8 = 57;
pub const RELOAD_XBOW: u8 = 58;
pub const MP5_CLIP_RELEASE: u8 = 59;
pub const MP5_CLIP_INSERT: u8 = 60;
pub const RELOAD_GLOCK: u8 = 61;
pub const RELOAD_SHOTGUN_ALT: u8 = 62;
pub const SHOTGUN_PUMP: u8 = 63;
pub const BARNEY_ATTACK: u8 = 64;
pub const MENU_MOVE: u8 = 65;
pub const BULLET_HIT1: u8 = 66; // flesh bullet impact (TEXTURETYPE CHAR_TEX_FLESH)
pub const BULLET_HIT2: u8 = 67;
pub const WOOD_IMPACT: u8 = 68; // debris/wood1: crowbar against CHAR_TEX_WOOD
pub const CBAR_HITBOD: u8 = 69; // crowbar landing on flesh (never the world dong)
// ---- the weapons' own sounds (ids 70.. of the cooked core) ----
pub const MINE_DEPLOY: u8 = 70; // tripmine stuck on a wall (mine_deploy)
pub const MINE_ACTIVATE: u8 = 71; // its beam comes up (mine_activate)
pub const SQK_HUNT1: u8 = 72; // a snark squeaks as it springs
pub const SQK_HUNT2: u8 = 73;
pub const SQK_HUNT3: u8 = 74;
pub const SQK_BITE: u8 = 75; // sqk_deploy1, the sound a bite makes
pub const SQK_BLAST: u8 = 76; // a snark pops
pub const EGON_WINDUP: u8 = 77; // the Gluon Gun spinning up
pub const EGON_RUN: u8 = 78; // its running hum, restarted while the beam holds
pub const EGON_OFF: u8 = 79; // and spinning down
pub const HORNET_FIRE: u8 = 80; // the hive hand's shot
pub const HORNET_BUZZ: u8 = 81; // a hornet in flight
pub const HORNET_HIT: u8 = 82; // and where it lands
pub const ROCKET_FLY: u8 = 83; // a rocket's motor (rocket1)
pub const XBOW_HIT: u8 = 84; // a bolt into the world
pub const XBOW_HITBOD: u8 = 85; // and into flesh
pub const GRENADE_HIT: u8 = 86; // a grenade's bounce (grenade_hit1)
pub const SATCHEL_BOUNCE: u8 = 87; // a satchel's (g_bounce2)
pub const GRENADE_PIN: u8 = 88; // the pin of a hand grenade
pub const WPN_MOVESELECT: u8 = 89; // the weapon list steps (R1/L1)
/// "No sound": past the bank, so `Hsfx` ignores it.
pub const NO_SOUND: u8 = u8::MAX;
pub const MENU_ACCEPT: u8 = BUTTON;
// Dialogue is not a global SFX id anymore -- it streams per-map (chunk
// 3100+idx) and plays via play_voice / play_voice_world (local ids).

const MAX_SFX: usize = 91;

static mut STATE: psx_goldsrc::hsfx::Hsfx<MAX_SFX, CHARGER_HEALTH_LOOP, CHARGER_HEV_LOOP> =
    psx_goldsrc::hsfx::Hsfx::new();

#[inline(always)]
unsafe fn state(
) -> &'static mut psx_goldsrc::hsfx::Hsfx<MAX_SFX, CHARGER_HEALTH_LOOP, CHARGER_HEV_LOOP> {
    &mut *core::ptr::addr_of_mut!(STATE)
}
pub const VOICE_CHUNK_BASE: u32 = 3100;

#[inline]
pub unsafe fn stop_all() {
    state().stop_all();
    bank_stop();
}

/// The SPU driver the pack loaders upload through. `Spu::new` resets the SPU
/// and writes the silence block every one-shot's repeat address points at,
/// which `init_from_pack` used to get from `psx_spu::init`, so each bank load
/// takes a fresh driver. The dialogue loader reuses it: a map change must not
/// reset the voices that are playing.
static mut SPU: Option<psx_spu::Spu> = None;
static mut ROOM_CORE_END: u32 = 0x1010;
// Last processed request can be neutral when its program lacks safe space.
static mut ROOM_APPLIED: u8 = u8::MAX;
static mut ROOM_SAMPLE_END: u32 = 0x1010;
static mut ROOM_EFFECT_BYTES: u32 = 0;

fn checked_room_bank(pack: &[u8], base: u32, limit: usize) -> u32 {
    let end = crate::env_sound_audio::bank_end(pack, base, limit)
        .expect("sound bank exceeds SPU RAM or has invalid layout");
    let n = u32::from_le_bytes(pack[4..8].try_into().unwrap()) as usize;
    for index in 0..n {
        let at = 8 + index * 8;
        let off = u32::from_le_bytes(pack[at..at + 4].try_into().unwrap()) as usize;
        let len = u32::from_le_bytes(pack[at + 4..at + 8].try_into().unwrap()) as usize;
        psx_asset::Audio::from_bytes(&pack[off..off + len])
            .expect("invalid sound payload before bank DMA");
    }
    end
}

/// Stop old effect writes before a map bank replaces any of their storage.
unsafe fn mute_room_effect() {
    use psx_hw::spu::{REVERB_VOL_LEFT, REVERB_VOL_RIGHT, SPUCNT};
    let control = psx_io::read_u16(SPUCNT);
    psx_io::write_u16(REVERB_VOL_LEFT, 0);
    psx_io::write_u16(REVERB_VOL_RIGHT, 0);
    psx_io::write_u16(0x1f801d98, 0);
    psx_io::write_u16(0x1f801d9a, 0);
    psx_io::write_u16(SPUCNT, control & !(1 << 7));
    ROOM_EFFECT_BYTES = 0;
}

/// Optional effects use only their exact, safe top-of-SPU workspace. Tight
/// banks retain every authored sample and receive neutral output instead.
#[cfg_attr(target_arch = "mips", optimize(size))]
pub unsafe fn apply_room(room: u8) {
    use psx_hw::spu::{REVERB_VOL_LEFT, REVERB_VOL_RIGHT, REVERB_WORK_BASE, SPUCNT};
    if ROOM_APPLIED == room {
        return;
    }
    let program = crate::env_sound_audio::program(room).expect("unsupported room program");
    mute_room_effect();
    let control = psx_io::read_u16(SPUCNT);
    if let Some(work) = crate::env_sound_audio::workspace(&program, ROOM_SAMPLE_END) {
        #[repr(C, align(4))]
        struct Zeros([u8; 256]);
        static ZERO: Zeros = Zeros([0; 256]);
        let spu = (*core::ptr::addr_of_mut!(SPU))
            .as_mut()
            .expect("room effect before SPU init");
        let mut offset = 0;
        while offset < work.bytes {
            let bytes = (work.bytes - offset).min(ZERO.0.len() as u32) as usize;
            spu.upload_adpcm(psx_spu::SpuAddr::new(work.base + offset), &ZERO.0[..bytes]);
            offset += bytes as u32;
        }
        psx_io::write_u16(REVERB_WORK_BASE, (work.base / 8) as u16);
        for (index, value) in program.registers.iter().enumerate() {
            psx_io::write_u16(0x1f801dc0 + index as u32 * 2, *value);
        }
        psx_io::write_u16(0x1f801d98, 0xffff);
        psx_io::write_u16(0x1f801d9a, 0x00ff);
        psx_io::write_u16(REVERB_VOL_LEFT, program.wet);
        psx_io::write_u16(REVERB_VOL_RIGHT, program.wet);
        psx_io::write_u16(SPUCNT, control | (1 << 7));
        ROOM_EFFECT_BYTES = work.bytes;
    }
    ROOM_APPLIED = room;
}

// ---- the held weapon's bank ----
//
// The sounds of `hl_format::weapon_audio::BANK_IDS` are not in the core bank: the
// weapon in hand brings them (a bank chunk streamed when it is drawn) into the
// region the core's last entry reserves. They play on the pool's last voice.

/// The viewmodel whose bank sits in the region, `BANK_NONE` before any does.
static mut BANK_LIVE: u8 = BANK_NONE;
const BANK_NONE: u8 = u8::MAX;
/// SPU address / 8 and rate of each sound of the live bank.
static mut BANK_ADDR: [(u16, u16); wa::BANK_MAX_IDS] = [(0, 0); wa::BANK_MAX_IDS];
/// Where the core reserves the region (SPU byte address) and its length in bytes; 0 when it has none.
static mut BANK_REGION: (u32, u32) = (0, 0);
/// The bank's own voice: the tail of what was Hsfx's 15-voice one-shot pool (the
/// suit's words are 15 and the chargers 16). `init_from_pack` takes it out of the
/// pool's rotation (`reserve_pool_tail`), so no pool sound starts on it and cuts a
/// shotgun's second barrel, and a banked sound cuts nothing of the pool's.
const BANK_VOICE: u8 = 14;
/// The one banked sound that holds until it is stopped (the Tau Cannon's charge hum
/// loops, `bank_stop` ends it); every other banked sound is a one-shot.
const BANK_HELD: u8 = GAUSS_CHARGE;

/// Find the bank region of a freshly uploaded core `pack`: the ADPCM of its last entry.
#[cfg_attr(target_arch = "mips", optimize(size))]
unsafe fn locate_bank_region(pack: &[u8]) {
    BANK_LIVE = BANK_NONE;
    BANK_REGION = (0, 0);
    let n = u32::from_le_bytes(pack[4..8].try_into().unwrap()) as usize;
    // A pack of the original ids and no more holds no weapon at all (the
    // lightweight profile); otherwise the region is its last entry.
    if n <= wa::CORE_IDS {
        return;
    }
    let mut at = 0x1010u32;
    let mut index = 0;
    while index < n {
        let len = u32::from_le_bytes(pack[12 + index * 8..16 + index * 8].try_into().unwrap()) - 32;
        if index == n - 1 {
            BANK_REGION = (at, len);
        }
        at = ((at + len + 15) & !15) + 16;
        index += 1;
    }
}

/// The viewmodel whose bank is live.
#[inline]
pub unsafe fn bank_live() -> u8 {
    BANK_LIVE
}

/// Lay the bank chunk of viewmodel `wm` (an HSFX pack) into the region, in the
/// order `BANK_IDS[wm]` names. False when it does not fit or does not match.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub unsafe fn load_bank(wm: usize, pack: &[u8]) -> bool {
    use psx_sfx::PARKING_TAIL;
    // Whatever the old bank was playing is about to be overwritten.
    let voice = psx_spu::Voice::new(BANK_VOICE);
    voice.set_volume(psx_spu::Volume::SILENCE, psx_spu::Volume::SILENCE);
    psx_spu::Voice::key_off(voice.mask());
    BANK_LIVE = BANK_NONE;
    let ids = wa::BANK_IDS[wm];
    let (base, room) = BANK_REGION;
    if ids.is_empty() || pack.len() < 8 || &pack[0..4] != b"HSFX" {
        return false;
    }
    if u32::from_le_bytes(pack[4..8].try_into().unwrap()) as usize != ids.len() {
        return false;
    }
    let Some(spu) = (*core::ptr::addr_of_mut!(SPU)).as_mut() else {
        return false;
    };
    let mut at = base;
    let mut k = 0;
    while k < ids.len() {
        let off = u32::from_le_bytes(pack[8 + k * 8..12 + k * 8].try_into().unwrap()) as usize;
        let len = u32::from_le_bytes(pack[12 + k * 8..16 + k * 8].try_into().unwrap()) as usize;
        let sample = &pack[off..off + len];
        let adpcm = &sample[32..];
        if at + wa::spu_bytes(adpcm.len()) as u32 > base + room {
            return false;
        }
        BANK_ADDR[k] = (
            (at / 8) as u16,
            u32::from_le_bytes(sample[16..20].try_into().unwrap()) as u16,
        );
        spu.upload_adpcm(psx_spu::SpuAddr::new(at), adpcm);
        at = (at + adpcm.len() as u32 + 15) & !15;
        spu.upload_adpcm(psx_spu::SpuAddr::new(at), &PARKING_TAIL);
        at += PARKING_TAIL.len() as u32;
        k += 1;
    }
    BANK_LIVE = wm as u8;
    true
}

/// End whatever the bank's voice is playing (the Tau Cannon's charge hum, which
/// loops until told to stop).
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub unsafe fn bank_stop() {
    let voice = psx_spu::Voice::new(BANK_VOICE);
    voice.set_volume(psx_spu::Volume::SILENCE, psx_spu::Volume::SILENCE);
    psx_spu::Voice::release(voice.mask());
}

/// Play a banked sound when its weapon's bank is the live one.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
unsafe fn bank_play(id: u8, den: u16) {
    if BANK_LIVE == BANK_NONE {
        return;
    }
    let ids = wa::BANK_IDS[BANK_LIVE as usize];
    let mut k = 0;
    while k < ids.len() {
        if ids[k] == id {
            let (addr, rate) = BANK_ADDR[k];
            let at = psx_spu::SpuAddr::new(addr as u32 * 8);
            let volume = psx_spu::Volume::linear(1, den.max(1));
            let voice = psx_spu::Voice::new(BANK_VOICE);
            if id == BANK_HELD {
                // A loop keeps its sustain until `bank_stop`, like the chargers' beds.
                voice.configure_sample(at, rate as u32, volume, psx_spu::Adsr::sample());
                psx_spu::Voice::start(voice.mask());
            } else {
                psx_sfx::OneShot::new(psx_sfx::Sample::resident(at, rate as u32, 0), volume)
                    .play(voice);
            }
            return;
        }
        k += 1;
    }
}

#[inline(always)]
fn banked(id: u8) -> bool {
    (id as usize) < wa::REGION_ENTRY && wa::BANKED_MASK[(id >> 5) as usize] >> (id & 31) & 1 != 0
}

#[inline]
pub unsafe fn init_from_pack(pack: &[u8]) -> usize {
    let end = checked_room_bank(pack, 0x1010, MAX_SFX);
    mute_room_effect();
    ROOM_CORE_END = end;
    ROOM_SAMPLE_END = end;
    ROOM_APPLIED = u8::MAX;
    // SAFETY: the game drives the SPU from this module alone, one caller at a time.
    let spu =
        (*core::ptr::addr_of_mut!(SPU)).insert(psx_spu::Spu::new(psx_io::periph::SpuDma::steal()));
    let ready = state().init_from_pack(spu, pack);
    // The bank plays on the pool's last voice: take it out of the rotation.
    state().reserve_pool_tail(BANK_VOICE);
    locate_bank_region(pack);
    assert_eq!(
        ready,
        u32::from_le_bytes(pack[4..8].try_into().unwrap()) as usize
    );
    ready
}

#[inline]
pub unsafe fn load_dialogue_pack(pack: &[u8]) -> usize {
    let dialogue_end = if pack.is_empty() {
        ROOM_CORE_END
    } else {
        checked_room_bank(pack, ROOM_CORE_END, 96)
    };
    // The suit's words (hev.rs) sit above the dialogue and below the room
    // effect's workspace.
    let end = dialogue_end + crate::hev::spu_bytes(pack);
    // Even a same-number preset must be re-admitted against this new bank.
    mute_room_effect();
    ROOM_APPLIED = u8::MAX;
    ROOM_SAMPLE_END = end;
    let spu = (*core::ptr::addr_of_mut!(SPU))
        .get_or_insert_with(|| psx_spu::Spu::new(psx_io::periph::SpuDma::steal()));
    let ready = state().load_dialogue_pack(spu, pack);
    if pack.is_empty() {
        crate::hev::unload();
    } else {
        crate::hev::load(spu, pack, dialogue_end);
    }
    if !pack.is_empty() {
        assert_eq!(
            ready,
            u32::from_le_bytes(pack[4..8].try_into().unwrap()) as usize
        );
    }
    ready
}

#[inline]
pub unsafe fn play_voice(local_id: u8, den: u16) -> u16 {
    trace(1, local_id, [0; 3]);
    state().play_voice(local_id, den)
}

#[inline]
pub unsafe fn play_voice_authored(
    local_id: u8,
    pos: [i32; 3],
    volume_percent: u8,
    packed_attenuation: u8,
) -> u16 {
    trace(1, local_id, pos);
    state().play_voice_authored(local_id, pos, volume_percent, packed_attenuation)
}

#[inline]
pub unsafe fn voice_ticks(local_id: u8) -> u16 {
    state().voice_ticks(local_id)
}

/// Silence the dialogue channel: a speaker died mid-sentence (CTalkMonster::Killed
/// calls StopTalking).
#[inline(never)]
pub unsafe fn stop_dialogue() {
    state().stop_dialogue()
}

#[inline]
pub unsafe fn play_voice_world(local_id: u8, pos: [i32; 3]) -> u16 {
    trace(1, local_id, pos);
    state().play_voice_world(local_id, pos)
}

#[inline]
pub unsafe fn play_map_authored(
    local_id: u8,
    pos: [i32; 3],
    volume_percent: u8,
    packed_attenuation: u8,
) {
    trace(2, local_id, pos);
    state().play_map_authored(local_id, pos, volume_percent, packed_attenuation)
}

#[inline]
pub unsafe fn play_map_world(local_id: u8, pos: [i32; 3]) {
    trace(2, local_id, pos);
    state().play_map_world(local_id, pos)
}

/// Out of line: Hsfx's distance falloff would otherwise be inlined at every
/// map-loop call site.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub unsafe fn play_map_loop_world(local_id: u8, pos: [i32; 3], owner: u16) {
    trace(3, local_id, pos);
    state().play_map_loop_world(local_id, pos, owner)
}

#[inline]
pub unsafe fn play_map_loop_authored(
    local_id: u8,
    pos: [i32; 3],
    owner: u16,
    volume_percent: u8,
    packed_attenuation: u8,
) {
    trace(3, local_id, pos);
    state().play_map_loop_authored(local_id, pos, owner, volume_percent, packed_attenuation)
}

#[inline]
pub unsafe fn stop_map_loop(owner: u16) {
    state().stop_map_loop(owner)
}

/// Re-level the map's authored loops (ambient_generic) against the listener,
/// keying those that came into range: once per simulation tick, after set_ear.
#[inline]
pub unsafe fn update_map_loops() {
    state().update_map_loops()
}

/// Set the level of the voice `owner`'s map loop holds, in place (GoldSrc's
/// SND_CHANGE_VOL). False when it holds none.
#[inline]
pub unsafe fn set_map_loop_volume(owner: u16, gain: psx_spu::Volume) -> bool {
    state().set_map_loop_volume(owner, gain)
}

/// Move `owner`'s map loop to `play_map_loop_world`'s level for `pos`, in
/// place. False when it holds no voice.
#[inline]
pub unsafe fn set_map_loop_world(owner: u16, pos: [i32; 3]) -> bool {
    state().set_map_loop_world(owner, pos)
}

#[inline]
pub unsafe fn play_vol(id: u8, den: u16) {
    if id == NO_SOUND {
        return;
    }
    trace(0, id, [0; 3]);
    if banked(id) {
        bank_play(id, den)
    } else {
        state().play_vol(id, den)
    }
}

#[inline]
pub unsafe fn play(id: u8) {
    play_vol(id, 1)
}

#[inline]
pub unsafe fn charger_stop() {
    state().charger_stop()
}

#[inline]
pub unsafe fn set_ear(pos: [i32; 3]) {
    state().set_ear(pos)
}

#[inline]
pub unsafe fn play_world(id: u8, pos: [i32; 3]) {
    trace(0, id, pos);
    state().play_world(id, pos)
}

/// Interaction-probe trace: one `probe event=sfx` line per playback request.
/// a = path (0 resident core id, 1 dialogue voice, 2 map-bank one-shot, 3 map
/// loop), b = the id in that path's table, c..e = world position (0 when the
/// sound is not positional). Never part of a shipping build.
#[cfg(feature = "interaction-probe")]
#[inline(never)]
unsafe fn trace(path: i32, id: u8, pos: [i32; 3]) {
    crate::reference_trace::probe_vals(
        crate::SIM_NOW as u32,
        "sfx",
        &[path, id as i32, pos[0], pos[1], pos[2]],
    );
}

/// The resident sound last asked for (u8::MAX before any). The weapon journeys
/// read it to prove which sample a shot, a blast or a placed mine plays, so
/// the link map must keep the symbol: a volatile store does.
pub static mut LAST_CORE: u8 = u8::MAX;

#[cfg(not(feature = "interaction-probe"))]
#[inline(always)]
unsafe fn trace(path: i32, id: u8, _pos: [i32; 3]) {
    if path == 0 {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(LAST_CORE), id);
    }
}
