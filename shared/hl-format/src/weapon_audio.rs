//! The held weapon's sounds: which core ids ride in a weapon's own bank.
//!
//! A gun's fire, reload and charge sounds are only needed while it is in the
//! player's hands, so they are not resident in every map's core bank. Each
//! viewmodel with such sounds has a small bank chunk (an HSFX pack holding the
//! sounds of `BANK_IDS[wm]` in that order) that the game streams when the
//! weapon is drawn, into the fixed region the core bank reserves for it (the
//! last entry of the core pack, `region_entry`). The ids keep their meaning in
//! the shared sound table; `sfx::play(id)` routes the banked ones to the bank.

/// Viewmodels, in the game's `WEAPON_MODELS` order.
pub const VIEWMODELS: usize = 15;

/// First world-pack chunk of the banks (`BANK_CHUNK_BASE + wm`). The ids are
/// chosen so that, taken modulo 32 (the chunk streamer's persistent entry
/// cache), they never meet a viewmodel chunk's (1000..=1014).
pub const BANK_CHUNK_BASE: u32 = 3031;

/// Most ids one bank holds.
pub const BANK_MAX_IDS: usize = 4;

/// Core ids each viewmodel's bank holds, in pack order. The crowbar's sounds,
/// the thrown and placed weapons' and everything a monster also plays stay in
/// the resident core.
pub const BANK_IDS: [&[u8]; VIEWMODELS] = [
    &[],               // 0  v_9mmhandgun (its reload stays resident: the pistol is what a map loads first, and loading a bank then costs a seek)
    &[3, 57],          // 1  v_357: shot, reload
    &[59, 60],         // 2  v_9mmar: clip release, clip insert
    &[4, 58],          // 3  v_crossbow: shot, reload
    &[],               // 4  v_crowbar
    &[77, 78, 79],     // 5  v_egon: windup, hum, off
    &[5, 52],          // 6  v_gauss: shot, charge
    &[88],             // 7  v_grenade: the pin
    &[80],             // 8  v_hgun: shot
    &[6],              // 9  v_rpg: launch
    &[],               // 10 v_satchel
    &[],               // 11 v_satchel_radio
    &[27, 51, 62, 63], // 12 v_shotgun: reload, double barrel, shell, pump
    &[],               // 13 v_squeak
    &[],               // 14 v_tripmine
];

/// The viewmodel whose bank holds core id `id`, if any.
pub const fn bank_of(id: u8) -> Option<usize> {
    let mut wm = 0;
    while wm < VIEWMODELS {
        let ids = BANK_IDS[wm];
        let mut k = 0;
        while k < ids.len() {
            if ids[k] == id {
                return Some(wm);
            }
            k += 1;
        }
        wm += 1;
    }
    None
}

/// Bit `id` of this is set for every id a bank holds.
pub const BANKED_MASK: [u32; 3] = {
    let mut mask = [0u32; 3];
    let mut wm = 0;
    while wm < VIEWMODELS {
        let mut k = 0;
        while k < BANK_IDS[wm].len() {
            let id = BANK_IDS[wm][k] as usize;
            mask[id >> 5] |= 1 << (id & 31);
            k += 1;
        }
        wm += 1;
    }
    mask
};

/// Ids of the shared sound table's first 70 sounds, the original core. A core
/// pack of this many entries (the lightweight profile) has no bank region.
pub const CORE_IDS: usize = 70;

/// The index, in the full core pack, of the entry that reserves the bank
/// region. Every profile that has a region has it as its last entry.
pub const REGION_ENTRY: usize = 90;

/// Bytes SPU RAM one sample of `adpcm_bytes` takes in a bank or the core: the
/// ADPCM rounded up to a block, then the 16-byte parking tail.
pub const fn spu_bytes(adpcm_bytes: usize) -> usize {
    ((adpcm_bytes + 15) & !15) + 16
}

const _: () = {
    let mut wm = 0;
    while wm < VIEWMODELS {
        assert!(BANK_IDS[wm].len() <= BANK_MAX_IDS);
        let mut k = 0;
        while k < BANK_IDS[wm].len() {
            assert!((BANK_IDS[wm][k] as usize) < REGION_ENTRY);
            k += 1;
        }
        wm += 1;
    }
    // Chunk ids modulo 32 clear of the viewmodels' 1000..=1014 (24..=31 apart from 8..=22).
    let mut wm = 0;
    while wm < VIEWMODELS {
        let slot = (BANK_CHUNK_BASE + wm as u32) % 32;
        assert!(slot < 8 || slot > 22);
        wm += 1;
    }
};
