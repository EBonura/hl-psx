//! Per-map sprite billboards (env_sprite / env_glow / env_spark / env_explosion)
//! and beam textures (env_beam / env_laser). The cooked pack streams from
//! WORLD.PAK chunk `SPRITE_CHUNK_BASE + map_index`; its frame textures append to
//! the shared texture atlas right after the map's models (so they reset per map
//! and share the residency budget). The draw itself lives in `main.rs`.
//!
//! Pack format ("HSPR", see tools/extract_sprites.py):
//!   magic "HSPR" | u16 n_sprites, u16 n_frames_total
//!   per sprite (12B): u8 n_frames, u8 blend(0 normal/1 add), u16 first_frame,
//!                     u16 base_w, u16 base_h, u16 crush_w, u16 crush_h
//!   then n_frames_total texture blobs (u16 w,h | u16 clut[16] | u8 pix4).

use crate::vram::{upload_tex_blob_raw, TexSlot, EMPTY_SLOT};

pub const SPRITE_CHUNK_BASE: u32 = 3200; // WORLD.PAK chunk = base + map_index
pub const MAX_SPRITES: usize = 12; // unique sprites per map (matches the cook cap)
pub const MAX_SPRITE_FRAMES: usize = 14; // total frame textures per map

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

/// Reset the sprite tables (called each map load before the pack streams).
pub unsafe fn reset() {
    N_SPRITES = 0;
    for d in SPRITE_DEFS.iter_mut() {
        *d = EMPTY_DEF;
    }
    for s in SPRITE_SLOTS.iter_mut() {
        *s = EMPTY_SLOT;
    }
}

/// Parse an HSPR pack and append its frame textures to the shared atlas. Must
/// run after the map + model textures are uploaded (it appends, no atlas reset).
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
    // The frame section is exactly the upload_tex_blob layout.
    let slots = core::ptr::addr_of_mut!(SPRITE_SLOTS) as *mut TexSlot;
    upload_tex_blob_raw(&data[off..], n_frames, slots, MAX_SPRITE_FRAMES);
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
