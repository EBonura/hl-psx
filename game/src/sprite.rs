//! Per-map sprite billboards (env_sprite / env_glow / env_spark / env_explosion)
//! and beam textures (env_beam / env_laser). The cooked pack streams from
//! WORLD.PAK chunk `SPRITE_CHUNK_BASE + map_index`; its frame textures append to
//! the shared texture atlas right after the map's models (so they reset per map
//! and share the residency budget). The draw itself lives in `main.rs`.
//!
//! Pack format ("HSPR", see `host/hl-content`):
//!   magic "HSPR" | u16 n_sprites, u16 n_frames_total
//!   per sprite (12B): u8 n_frames, u8 blend(0 normal/1 add), u16 first_frame,
//!                     u16 base_w, u16 base_h, u16 crush_w, u16 crush_h
//!   then n_frames_total texture blobs (u16 w,h | u16 clut[16] | u8 pix4).

use crate::vram::{upload_tex_blob_raw, TexSlot, EMPTY_SLOT};

pub const SPRITE_CHUNK_BASE: u32 = 3200; // WORLD.PAK chunk = base + map_index
pub const MAX_SPRITES: usize = 12; // unique sprites per map (matches the cook cap)
                                   // The full 96-map campaign peaks at 49 sampled frame textures (c2a5e).
                                   // Keeping the old 14-frame table silently dropped tail sprites on 38 maps;
                                   // clearing only the previous map's live prefix keeps the linked image within
                                   // 8 bytes of the 14-frame build, and the audited VRAM peak still fits.
pub const MAX_SPRITE_FRAMES: usize = 49; // total frame textures per map

#[derive(Copy, Clone)]
pub struct SpriteDef {
    pub n_frames: u8,
    pub blend: u8, // 0 = normal (index 0 transparent), 1 = additive
    pub first_frame: u16,
    pub base_w: u16, // native sprite pixel size, for world-scale
    pub base_h: u16,
    pub crush_w: u16, // the 4bpp texture size, for UVs
    pub crush_h: u16,
}

pub const EMPTY_DEF: SpriteDef = SpriteDef {
    n_frames: 0,
    blend: 0,
    first_frame: 0,
    base_w: 0,
    base_h: 0,
    crush_w: 0,
    crush_h: 0,
};

static mut SPRITE_DEFS: [SpriteDef; MAX_SPRITES] = [EMPTY_DEF; MAX_SPRITES];
static mut SPRITE_SLOTS: [TexSlot; MAX_SPRITE_FRAMES] = [EMPTY_SLOT; MAX_SPRITE_FRAMES];
static mut N_SPRITES: usize = 0;
static mut N_SPRITE_FRAMES: usize = 0;

// Resident explosion sprite (s_explod.spr): its own single-sprite pack, loaded
// every map into these dedicated slots (weapon blasts happen anywhere, not just
// where env_explosion entities are placed). Separate from the per-map tables so
// reset() never clears it; it re-appends to the atlas each map load.
pub const EXPL_CHUNK_ID: u32 = 3002;
pub const MAX_EXPL_FRAMES: usize = 5;
static mut EXPL_DEF: SpriteDef = EMPTY_DEF;
static mut EXPL_SLOTS: [TexSlot; MAX_EXPL_FRAMES] = [EMPTY_SLOT; MAX_EXPL_FRAMES];

/// Reset the sprite tables (called each map load before the pack streams).
pub unsafe fn reset() {
    // Clear only the live prefix from the previous map. Runtime bounds keep the
    // MIPS build as two compact loops; iterating whole fixed arrays caused LLVM
    // to unroll the 49-slot clear and spent more code RAM than the slots did.
    let old_sprites = N_SPRITES;
    let old_frames = N_SPRITE_FRAMES;
    N_SPRITES = 0;
    N_SPRITE_FRAMES = 0;
    let defs = core::ptr::addr_of_mut!(SPRITE_DEFS).cast::<SpriteDef>();
    let mut i = 0;
    while i < old_sprites {
        defs.add(i).write(EMPTY_DEF);
        i += 1;
    }
    let slots = core::ptr::addr_of_mut!(SPRITE_SLOTS).cast::<TexSlot>();
    i = 0;
    while i < old_frames {
        slots.add(i).write(EMPTY_SLOT);
        i += 1;
    }
}

/// Parse an HSPR pack and append its frame textures to the shared atlas. Must
/// run after the map + model textures are uploaded (it appends, no atlas reset).
/// This is a once-per-map loader, not frame work. Keep it out of `play()` so
/// LTO cannot fold its parser and bounds exits into that already huge function;
/// doing so can exceed the MIPS-I PC16 branch span in diagnostic builds.
#[inline(never)]
pub unsafe fn load_pack(data: &[u8]) {
    reset();
    if data.len() < 8 || &data[0..4] != b"HSPR" {
        return;
    }
    let n_sprites = u16::from_le_bytes([data[4], data[5]]) as usize;
    let n_frames = u16::from_le_bytes([data[6], data[7]]) as usize;
    if n_sprites > MAX_SPRITES || n_frames > MAX_SPRITE_FRAMES {
        return;
    }
    let mut off = 8;
    for i in 0..n_sprites {
        if off + 12 > data.len() {
            return;
        }
        SPRITE_DEFS[i] = SpriteDef {
            n_frames: data[off],
            blend: data[off + 1],
            first_frame: u16::from_le_bytes([data[off + 2], data[off + 3]]),
            base_w: u16::from_le_bytes([data[off + 4], data[off + 5]]),
            base_h: u16::from_le_bytes([data[off + 6], data[off + 7]]),
            crush_w: u16::from_le_bytes([data[off + 8], data[off + 9]]),
            crush_h: u16::from_le_bytes([data[off + 10], data[off + 11]]),
        };
        off += 12;
    }
    N_SPRITES = n_sprites;
    N_SPRITE_FRAMES = n_frames;
    // The frame section is exactly the upload_tex_blob layout.
    let slots = core::ptr::addr_of_mut!(SPRITE_SLOTS) as *mut TexSlot;
    upload_tex_blob_raw(&data[off..], n_frames, slots, MAX_SPRITE_FRAMES);
}

/// Parse the resident single-sprite explosion pack (same HSPR layout, n=1) into
/// EXPL_DEF/EXPL_SLOTS, appending its frames to the atlas. Call each map load.
#[inline(never)]
pub unsafe fn load_explosion(data: &[u8]) {
    EXPL_DEF = EMPTY_DEF;
    for s in EXPL_SLOTS.iter_mut() {
        *s = EMPTY_SLOT;
    }
    if data.len() < 20 || &data[0..4] != b"HSPR" {
        return;
    }
    let n_frames = u16::from_le_bytes([data[6], data[7]]) as usize;
    if n_frames == 0 || n_frames > MAX_EXPL_FRAMES {
        return;
    }
    EXPL_DEF = SpriteDef {
        n_frames: data[8],
        blend: data[9],
        first_frame: 0,
        base_w: u16::from_le_bytes([data[12], data[13]]),
        base_h: u16::from_le_bytes([data[14], data[15]]),
        crush_w: u16::from_le_bytes([data[16], data[17]]),
        crush_h: u16::from_le_bytes([data[18], data[19]]),
    };
    let slots = core::ptr::addr_of_mut!(EXPL_SLOTS) as *mut TexSlot;
    upload_tex_blob_raw(&data[20..], n_frames, slots, MAX_EXPL_FRAMES);
}

#[inline]
pub fn expl_def() -> SpriteDef {
    unsafe { EXPL_DEF }
}

/// The texture slot for the resident explosion's frame (clamped to its range).
#[inline]
pub fn expl_slot(frame: usize) -> TexSlot {
    unsafe {
        if EXPL_DEF.n_frames == 0 {
            return EMPTY_SLOT;
        }
        let fi = frame.min(EXPL_DEF.n_frames as usize - 1);
        *EXPL_SLOTS.get(fi).unwrap_or(&EMPTY_SLOT)
    }
}

#[inline]
pub fn n_sprites() -> usize {
    unsafe { N_SPRITES }
}

#[inline]
pub fn def(id: usize) -> SpriteDef {
    unsafe { *SPRITE_DEFS.get(id).unwrap_or(&EMPTY_DEF) }
}

/// The texture slot for sprite `id`'s frame `frame` (clamped to its range).
#[inline]
pub fn slot_for(id: usize, frame: usize) -> TexSlot {
    unsafe {
        let d = def(id);
        if d.n_frames == 0 {
            return EMPTY_SLOT;
        }
        let fi = d.first_frame as usize + frame.min(d.n_frames as usize - 1);
        *SPRITE_SLOTS.get(fi).unwrap_or(&EMPTY_SLOT)
    }
}
