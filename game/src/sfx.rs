//! Authored sound identifiers and the game-owned shared playback state.
pub const CHUNK_ID: u32 = 3000;
pub const CHUNK_ID_LIGHT: u32 = 3050;
pub const CHUNK_ID_TRAINING_WEAPONS: u32 = 3051;

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
pub const DOOR_MOVE: u8 = 14;
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
pub const HEV_BELL: u8 = 42;
pub const GEIGER: u8 = 43; // radiation/toxic-zone click
pub const HEV_ACTIVATE: u8 = 44; // suit power-on voice (pickup)
pub const HEV_HEALTH_CRIT: u8 = 45; // "health critical"
pub const HEV_NEAR_DEATH: u8 = 46; // "near death"
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
pub const MENU_ACCEPT: u8 = BUTTON;
// Dialogue is not a global SFX id anymore -- it streams per-map (chunk
// 3100+idx) and plays via play_voice / play_voice_world (local ids).

const MAX_SFX: usize = 70;

static mut STATE: psx_goldsrc::hsfx::Hsfx<MAX_SFX, CHARGER_HEALTH_LOOP, CHARGER_HEV_LOOP> =
    psx_goldsrc::hsfx::Hsfx::new();
pub const VOICE_CHUNK_BASE: u32 = 3100;

#[inline]
pub unsafe fn stop_dialogue() {
    MAP_SAMPLES = 0; // Hsfx::stop_dialogue empties the map bank's count too
    STATE.stop_dialogue()
}

#[inline]
pub unsafe fn stop_map_loops() {
    loops_reset();
    STATE.stop_map_loops()
}

#[inline]
pub unsafe fn stop_all() {
    loops_reset();
    STATE.stop_all()
}

#[inline]
pub unsafe fn init_from_pack(pack: &[u8]) -> usize {
    STATE.init_from_pack(pack)
}

#[inline]
pub unsafe fn load_dialogue_pack(pack: &[u8]) -> usize {
    loops_reset();
    let n = STATE.load_dialogue_pack(pack);
    MAP_SAMPLES = n.min(u8::MAX as usize) as u8;
    n
}

#[inline]
pub unsafe fn play_voice(local_id: u8, den: u16) -> u16 {
    STATE.play_voice(local_id, den)
}

#[inline]
pub unsafe fn play_voice_authored(
    local_id: u8,
    pos: [i32; 3],
    volume_percent: u8,
    packed_attenuation: u8,
) -> u16 {
    STATE.play_voice_authored(local_id, pos, volume_percent, packed_attenuation)
}

#[inline]
pub unsafe fn voice_ticks(local_id: u8) -> u16 {
    STATE.voice_ticks(local_id)
}

#[inline]
pub unsafe fn play_voice_world(local_id: u8, pos: [i32; 3]) -> u16 {
    STATE.play_voice_world(local_id, pos)
}

#[inline]
pub unsafe fn play_map_vol(local_id: u8, den: u16) {
    STATE.play_map_vol(local_id, den)
}

#[inline]
pub unsafe fn play_map_authored(
    local_id: u8,
    pos: [i32; 3],
    volume_percent: u8,
    packed_attenuation: u8,
) {
    STATE.play_map_authored(local_id, pos, volume_percent, packed_attenuation)
}

#[inline]
pub unsafe fn play_map(local_id: u8) {
    STATE.play_map(local_id)
}

#[inline]
pub unsafe fn play_map_world(local_id: u8, pos: [i32; 3]) {
    STATE.play_map_world(local_id, pos)
}

/// Out of line: Hsfx's distance falloff would otherwise be inlined at every
/// map-loop call site.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub unsafe fn play_map_loop_world(local_id: u8, pos: [i32; 3], owner: u16) {
    loop_keyed(local_id, owner);
    STATE.play_map_loop_world(local_id, pos, owner)
}

#[inline]
pub unsafe fn play_map_loop_authored(
    local_id: u8,
    pos: [i32; 3],
    owner: u16,
    volume_percent: u8,
    packed_attenuation: u8,
) {
    loop_keyed(local_id, owner);
    STATE.play_map_loop_authored(local_id, pos, owner, volume_percent, packed_attenuation)
}

#[inline]
pub unsafe fn stop_map_loop(owner: u16) {
    loop_stopped(owner);
    STATE.stop_map_loop(owner)
}

/// The listener position of the last `set_ear`, for set-piece loop levels.
pub static mut EAR: [i32; 3] = [0; 3];

// Mirror of Hsfx's map-loop voice allocation (psx-goldsrc hsfx.rs: voices
// MAP_LOOP_VOICE_FIRST 17 .. +MAP_LOOP_VOICE_COUNT 7, taken round-robin, reset
// by stop_map_loops / stop_all / load_dialogue_pack, and a loop is keyed only
// when its local id is in the map bank and its owner holds no voice yet). It
// lets a moving set-piece loop change its level in place, as the SDK's
// SND_CHANGE_VOL does, instead of re-keying onto the next voice and evicting
// whatever map loop held it.
const LOOP_VOICE_FIRST: u8 = 17;
const LOOP_VOICES: usize = 7;
static mut LOOP_OWNER: [u16; LOOP_VOICES] = [u16::MAX; LOOP_VOICES];
static mut LOOP_NEXT: u8 = 0;
static mut MAP_SAMPLES: u8 = 0;

#[cfg_attr(target_arch = "mips", optimize(size))]
unsafe fn loops_reset() {
    LOOP_OWNER = [u16::MAX; LOOP_VOICES];
    LOOP_NEXT = 0;
}

#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
unsafe fn loop_keyed(local_id: u8, owner: u16) {
    if local_id >= MAP_SAMPLES || loop_voice(owner).is_some() {
        return;
    }
    LOOP_OWNER[LOOP_NEXT as usize] = owner;
    LOOP_NEXT = ((LOOP_NEXT as usize + 1) % LOOP_VOICES) as u8;
}

#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
unsafe fn loop_stopped(owner: u16) {
    for o in LOOP_OWNER.iter_mut() {
        if *o == owner {
            *o = u16::MAX;
        }
    }
}

/// The SPU voice a map loop owner holds, if any.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub unsafe fn loop_voice(owner: u16) -> Option<u8> {
    LOOP_OWNER.iter().position(|&o| o == owner).map(|i| LOOP_VOICE_FIRST + i as u8)
}

#[inline]
pub unsafe fn play_vol(id: u8, den: u16) {
    STATE.play_vol(id, den)
}

#[inline]
pub unsafe fn play(id: u8) {
    STATE.play(id)
}

#[inline]
pub unsafe fn charger_stop() {
    STATE.charger_stop()
}

#[inline]
pub unsafe fn set_ear(pos: [i32; 3]) {
    EAR = pos;
    STATE.set_ear(pos)
}

#[inline]
pub unsafe fn play_world(id: u8, pos: [i32; 3]) {
    STATE.play_world(id, pos)
}

#[inline]
pub unsafe fn play_at(id: u8, dist2: i32) {
    STATE.play_at(id, dist2)
}
