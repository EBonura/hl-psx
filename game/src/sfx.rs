//! HL sound effects: one WORLD.PAK chunk of cooked SPU-ADPCM samples,
//! uploaded to SPU RAM once at boot (zero main-RAM cost afterwards).
//!
//! Pack layout (tools/extract_sfx.py -- ID ORDER MUST MATCH the consts here):
//!   "HSFX" | u32 count | count x (u32 offset, u32 len) | .psau blobs
//!
//! Playback rotates a pool of one-shot voices; volume is a linear fraction
//! (den 1 = full). `play_at` derives the fraction from world distance.

use psx_asset::Audio;
use psx_spu::{self as spu, Adsr, SpuAddr, Voice, Volume};

pub const CHUNK_ID: u32 = 3000;

// ---- sample ids (extract_sfx.py order) ----
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

const MAX_SFX: usize = 24;
const SPU_SAMPLE_BASE: u32 = 0x1010; // BIOS convention: 0x0000..0x1000 reserved
const VOICE_POOL: u8 = 16; // voices 0..15 one-shots; 16..23 reserved (loops/music)

static mut ADDRS: [u32; MAX_SFX] = [0; MAX_SFX];
static mut RATES: [u32; MAX_SFX] = [0; MAX_SFX];
static mut COUNT: usize = 0;
static mut NEXT_VOICE: u8 = 0;

fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Parse a staged HSFX pack and upload every sample to SPU RAM.
/// Returns the number of samples ready.
pub unsafe fn init_from_pack(pack: &[u8]) -> usize {
    spu::init();
    if pack.len() < 8 || &pack[0..4] != b"HSFX" {
        return 0;
    }
    let n = (rd_u32(pack, 4) as usize).min(MAX_SFX);
    let mut next_addr = SPU_SAMPLE_BASE;
    let mut ready = 0usize;
    for i in 0..n {
        let off = rd_u32(pack, 8 + i * 8) as usize;
        let len = rd_u32(pack, 12 + i * 8) as usize;
        if off + len > pack.len() {
            break;
        }
        let Ok(audio) = Audio::from_bytes(&pack[off..off + len]) else {
            break;
        };
        let bytes = audio.adpcm_bytes();
        if next_addr + bytes.len() as u32 > 512 * 1024 {
            break;
        }
        let addr = SpuAddr::new(next_addr);
        spu::upload_adpcm(addr, bytes);
        ADDRS[i] = next_addr;
        RATES[i] = audio.sample_rate_hz();
        next_addr = (next_addr + bytes.len() as u32 + 7) & !7;
        ready = i + 1;
    }
    COUNT = ready;
    ready
}

/// Fire-and-forget one-shot. `den` is the inverse volume (1 = full, bigger =
/// quieter); ids come from the consts above.
pub unsafe fn play_vol(id: u8, den: u16) {
    let i = id as usize;
    if i >= COUNT {
        return;
    }
    let v = Voice::new(NEXT_VOICE);
    NEXT_VOICE = (NEXT_VOICE + 1) % VOICE_POOL;
    v.configure_sample(
        SpuAddr::new(ADDRS[i]),
        RATES[i],
        Volume::linear(1, den.max(1)),
        Adsr::sample(),
    );
    Voice::key_on(v.mask());
}

/// Full-volume one-shot (player-local sounds: own weapon, pain, pickups).
pub unsafe fn play(id: u8) {
    play_vol(id, 1);
}

static mut EAR: [i32; 3] = [0; 3];

/// Update the listener position (player) once per frame.
pub unsafe fn set_ear(pos: [i32; 3]) {
    EAR = pos;
}

/// World-positioned one-shot, attenuated by distance to the last `set_ear`.
pub unsafe fn play_world(id: u8, pos: [i32; 3]) {
    let dx = pos[0] - EAR[0];
    let dy = pos[1] - EAR[1];
    let dz = pos[2] - EAR[2];
    // Saturating: coords are ±32k so squares fit i64; clamp into i32.
    let d2 = (dx as i64 * dx as i64 + dy as i64 * dy as i64 + dz as i64 * dz as i64)
        .min(i32::MAX as i64) as i32;
    play_at(id, d2);
}

/// World-positioned one-shot: volume falls off with distance, silent past
/// ~1600 units. `dist2` is squared world distance (dist2_xz-style i32).
pub unsafe fn play_at(id: u8, dist2: i32) {
    if dist2 < 0 {
        return; // overflowed square: treat as out of range
    }
    // den = 1 + dist/200 (integer): full <200u, 1/2 at 400u, 1/8 past 1400u.
    let mut lo = 0i32;
    let mut hi = 1600i32;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if mid * mid < dist2 {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo >= 1600 {
        return;
    }
    play_vol(id, (1 + lo / 200) as u16);
}
