//! Upload the cooked map textures to VRAM once at load, using the SDK's
//! `psx-vram` atlas + CLUT allocators, and hand back a per-texture material
//! the renderer feeds straight into `TriTextured`.
//!
//! Layout: 4-bit texture pages in the texture region X=320.. (bands Y=0 and
//! Y=256); CLUTs in the free block X=0..319 Y=480..511, which is what is left
//! under the two 320x240 framebuffers at X=0..319 Y=0..479. All 164 c1a0
//! textures are <=64x64, so one band of 11 pages (16 textures each) holds them.
//!
//! The X bound on the CLUT block is load-bearing, not cosmetic. Texture band 1
//! spans Y=256..511, so the CLUT rows sit *inside* it and only their X keeps
//! the two apart. This was wrong until 2026-08-19: the block was bounded at
//! X<960 (just clear of the fixed HUD page), so slots 20..59 of every row
//! landed in band-1 pages 11..20 and a world palette could be written over
//! texture pixels, or a texture over a palette. `clut_block_hits_a_texture_page`
//! below now proves the separation at compile time.

use psx_gpu::material::{TextureMaterial, TextureWindow, TexturedGouraudPacketMaterial};
use psx_vram::{upload_bytes, ClutRowAllocator, TexDepth, TextureWindowAtlas, Tpage, VramRect};

use core::ptr;

const TEX_X0: u16 = 320; // first texture-page column (after the framebuffers)
const COLS: u16 = 11; // X=320,384,..,960
                      // Allocator pages 0..20 cover X=320..960 in band 0 and X=320..896 in
                      // band 1. The final band-1 page at X=960 is deliberately excluded: HUD pixels,
                      // the pre-scaled gameplay font, and their CLUTs live there at fixed addresses.
                      // The all-96 asset simulation peaks at page 13 for map+models and page 19 even
                      // under the impossible upper bound of appending every viewmodel at once.
const PAGES: usize = 21;
/// Y=480..511: every row under the framebuffers, all of it clear of the
/// texture region because the block stops at [`CLUT_MAX_X`].
const CLUT_ROWS: usize = 32;
const CLUT_BASE_Y: u16 = 480;
/// The CLUT block ends where the texture region begins. `ClutRowAllocator`
/// has no reservation primitive, so `upload_one` discards and thereby retires
/// any slot at or beyond this X; the allocation naturally continues on the
/// next row. This also subsumes the old X<960 bound that kept world palettes
/// off the fixed HUD/font page at X=960..1023.
const CLUT_MAX_X: u16 = TEX_X0;
/// Usable 16-entry CLUTs: 20 slots a row over 32 rows. `main.rs` asserts this
/// covers every texture slot table that can be resident at once.
pub const CLUT_CAPACITY: usize = (CLUT_MAX_X / 16) as usize * CLUT_ROWS;
const VRAM_W: u16 = 1024;
const VRAM_H: u16 = 512;

/// Does any CLUT slot the allocator can hand out land inside a texture page?
///
/// `ClutRowAllocator` walks rows in order and slots left to right within a
/// row, so slot `s` is always at X = `s * 16`. Every slot below
/// [`CLUT_MAX_X`] is reachable, and the block's rows are `CLUT_BASE_Y ..
/// CLUT_BASE_Y + CLUT_ROWS`. A texture page is 64 halfwords wide and 256 rows
/// tall at its `tpage` origin. Const-evaluated, so it costs no image bytes;
/// it exists so moving the block, adding a page band or growing `CLUT_ROWS`
/// fails the build instead of quietly corrupting VRAM at run time.
const fn clut_block_hits_a_texture_page() -> bool {
    let mut slot = 0u16;
    while slot * 16 < CLUT_MAX_X {
        let x = slot * 16;
        let mut page = 0usize;
        while page < PAGES {
            let px = TEX_X0 + (page as u16 % COLS) * 64;
            let py: u16 = if page / (COLS as usize) == 0 { 0 } else { 256 };
            let rows_overlap = py < CLUT_BASE_Y + CLUT_ROWS as u16 && CLUT_BASE_Y < py + 256;
            let cols_overlap = x < px + 64 && px < x + 16;
            if rows_overlap && cols_overlap {
                return true;
            }
            page += 1;
        }
        slot += 1;
    }
    false
}
const _: () = assert!(
    !clut_block_hits_a_texture_page(),
    "a reachable CLUT slot overlaps a texture page"
);
const _: () = assert!(
    CLUT_BASE_Y as usize + CLUT_ROWS <= VRAM_H as usize,
    "the CLUT block runs off the bottom of VRAM"
);

#[derive(Copy, Clone)]
pub struct TexSlot {
    pub material: TextureMaterial,
    pub packet: TexturedGouraudPacketMaterial,
    pub valid: bool,
    /// A solid single-colour texture (every texel index 0) -- e.g. GoldSrc's
    /// `black` backdrop wall. These sit nearly coplanar with the real geometry
    /// in front of them; without a Z-buffer the per-triangle OT can tie-break the
    /// wrong way and the backdrop occludes the detail. The renderer biases these
    /// to the back of the OT so the foreground always wins (a backdrop is, by
    /// definition, behind everything near it).
    pub backdrop: bool,
}

pub const EMPTY_MATERIAL_PUB: TextureMaterial = TextureMaterial::opaque(0, 0, (128, 128, 128));
const EMPTY_MATERIAL: TextureMaterial = EMPTY_MATERIAL_PUB;

pub const EMPTY_SLOT: TexSlot = TexSlot {
    material: EMPTY_MATERIAL,
    packet: TexturedGouraudPacketMaterial::from_texture(EMPTY_MATERIAL),
    valid: false,
    backdrop: false,
};

/// All-zero twin of [`EMPTY_SLOT`], used only so the slot tables land in
/// `.bss` rather than `.data`; the real default is installed at boot. A
/// non-zero initializer is stored in the executable and copied to RAM at
/// load, where `.bss` costs neither.
pub const ZERO_SLOT: TexSlot = TexSlot {
    material: TextureMaterial::opaque(0, 0, (0, 0, 0)),
    packet: TexturedGouraudPacketMaterial {
        tex_window_word: 0,
        color0_command_word: 0,
        clut_high_word: 0,
        tpage_high_word: 0,
    },
    valid: false,
    backdrop: false,
};

static mut ATLAS: TextureWindowAtlas<PAGES> = TextureWindowAtlas::new();
static mut CLUTS: ClutRowAllocator<CLUT_ROWS> = ClutRowAllocator::new(CLUT_BASE_Y);

// Viewmodel-pool checkpoint: both allocators are Copy, so we snapshot them once
// the map's map/HUD/sprite/enemy/glock textures are all uploaded. Restoring frees
// exactly the switched-viewmodel textures loaded afterwards (nothing else comes
// after the checkpoint) so the VM pool can evict back to glock-only and reload
// the held weapon -- fixing the "switched weapon shows the glock model" fallback.
static mut VM_CP: Option<(TextureWindowAtlas<PAGES>, ClutRowAllocator<CLUT_ROWS>)> = None;

/// Snapshot the allocators (call after all non-viewmodel textures are resident).
pub fn vm_checkpoint() {
    unsafe { VM_CP = Some((ATLAS, CLUTS)) };
}

/// Restore the allocators to the checkpoint, freeing every switched-viewmodel
/// texture. No-op if no checkpoint was taken this map.
pub fn vm_restore() {
    unsafe {
        if let Some((a, c)) = VM_CP {
            ATLAS = a;
            CLUTS = c;
        }
    }
}

#[inline(always)]
fn rd_u32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *d.get(o)?,
        *d.get(o + 1)?,
        *d.get(o + 2)?,
        *d.get(o + 3)?,
    ]))
}

unsafe fn reset_allocators() {
    unsafe {
        ATLAS = TextureWindowAtlas::new();
        CLUTS = ClutRowAllocator::new(CLUT_BASE_Y);
        VM_CP = None; // invalidate the previous map's viewmodel checkpoint
    }
}

#[inline(always)]
fn canonical_ram_const<T>(p: *const T) -> *const T {
    #[cfg(target_arch = "mips")]
    {
        (((p as usize) & 0x001f_ffff) | 0x8000_0000) as *const T
    }
    #[cfg(not(target_arch = "mips"))]
    {
        p
    }
}

#[inline(always)]
fn canonical_ram_mut<T>(p: *mut T) -> *mut T {
    canonical_ram_const(p as *const T) as *mut T
}

#[inline(always)]
unsafe fn canonical_ram_bytes(data: &[u8]) -> &[u8] {
    unsafe { core::slice::from_raw_parts(canonical_ram_const(data.as_ptr()), data.len()) }
}

/// Upload a streamed map texture chunk:
/// `magic "HLTX" | u32 n_texs | texture blob`.
///
/// Returns `(n_texs, failed_uploads)` so the caller can report the same
/// counters as the legacy inline map-texture path.
pub unsafe fn upload_tex_chunk_raw(
    data: &[u8],
    slots: *mut TexSlot,
    slot_len: usize,
) -> Option<(usize, usize)> {
    let data = unsafe { canonical_ram_bytes(data) };
    if data.len() < 8 || data.get(0..4)? != b"HLTX" {
        return None;
    }
    let n_texs = rd_u32(data, 4)? as usize;
    unsafe { reset_allocators() };
    let failed = unsafe { upload_tex_blob_raw(&data[8..], n_texs, slots, slot_len) };
    Some((n_texs, failed))
}

/// Upload a streamed texture chunk without resetting the atlas. Use this for
/// model textures after the room's material chunk has established the frame's
/// VRAM allocation state.
pub unsafe fn upload_tex_chunk_append_raw(
    data: &[u8],
    slots: *mut TexSlot,
    slot_len: usize,
) -> Option<(usize, usize)> {
    let data = unsafe { canonical_ram_bytes(data) };
    if data.len() < 8 || data.get(0..4)? != b"HLTX" {
        return None;
    }
    let n_texs = rd_u32(data, 4)? as usize;
    let failed = unsafe { upload_tex_blob_raw(&data[8..], n_texs, slots, slot_len) };
    Some((n_texs, failed))
}

/// Upload only the textures selected by `used_mask`, compacting their slots
/// and writing the old-to-new slot mapping into `remap`. This keeps studio
/// bodygroups that are not present in the current map from consuming scarce
/// texture slots or VRAM. Studio model chunks are limited to 32 textures when
/// using this path so the selection and remap stay stack-cheap on PS1.
pub unsafe fn upload_tex_chunk_append_masked_raw(
    data: &[u8],
    slots: *mut TexSlot,
    slot_len: usize,
    used_mask: u32,
    remap: &mut [u8; 32],
) -> Option<(usize, usize)> {
    let data = unsafe { canonical_ram_bytes(data) };
    if data.len() < 8 || data.get(0..4)? != b"HLTX" {
        return None;
    }
    let n_texs = rd_u32(data, 4)? as usize;
    if n_texs > remap.len() {
        return None;
    }
    remap.fill(u8::MAX);
    let valid_mask = if n_texs == 32 {
        u32::MAX
    } else {
        (1u32 << n_texs) - 1
    };
    let selected = (used_mask & valid_mask).count_ones() as usize;
    if selected > slot_len {
        return None;
    }

    let slots = canonical_ram_mut(slots);
    let blob = &data[8..];
    let mut off = 0usize;
    let mut out = 0usize;
    let mut failed = 0usize;
    for i in 0..n_texs {
        if off + 36 > blob.len() {
            return None;
        }
        let w = u16::from_le_bytes([blob[off], blob[off + 1]]);
        let h = u16::from_le_bytes([blob[off + 2], blob[off + 3]]);
        let clut = &blob[off + 4..off + 36];
        let pix_len = (w as usize * h as usize) / 2;
        let pix_off = off + 36;
        if pix_off + pix_len > blob.len() {
            return None;
        }
        let pix = &blob[pix_off..pix_off + pix_len];
        off = pix_off + pix_len;
        if used_mask & (1u32 << i) == 0 {
            continue;
        }
        remap[i] = out as u8;
        let slot = match upload_one(w, h, clut, pix) {
            Some(s) => s,
            None => {
                failed += 1;
                EMPTY_SLOT
            }
        };
        unsafe {
            ptr::write(slots.add(out), slot);
        }
        out += 1;
    }
    Some((out, failed))
}

/// Upload a `.hlm`/`.hlmdl` texture blob (u16 w,h | u16 clut[16] | u8 pix4 each)
/// into `slots`. Returns the count that did not fit VRAM.
pub fn upload_tex_blob(data: &[u8], n_texs: usize, slots: &mut [TexSlot]) -> usize {
    unsafe { upload_tex_blob_raw(data, n_texs, slots.as_mut_ptr(), slots.len()) }
}

/// Raw-pointer variant for filling `static mut` slot tables without creating
/// references to those statics.
pub unsafe fn upload_tex_blob_raw(
    data: &[u8],
    n_texs: usize,
    slots: *mut TexSlot,
    slot_len: usize,
) -> usize {
    let data = unsafe { canonical_ram_bytes(data) };
    let slots = canonical_ram_mut(slots);
    let mut off = 0usize;
    let mut failed = 0usize;
    for i in 0..n_texs {
        if off + 36 > data.len() {
            break;
        }
        let w = u16::from_le_bytes([data[off], data[off + 1]]);
        let h = u16::from_le_bytes([data[off + 2], data[off + 3]]);
        let clut = &data[off + 4..off + 36];
        let pix_len = (w as usize * h as usize) / 2;
        let pix_off = off + 36;
        if pix_off + pix_len > data.len() {
            break;
        }
        let pix = &data[pix_off..pix_off + pix_len];
        off = pix_off + pix_len;
        if i >= slot_len {
            continue;
        }
        let slot = match upload_one(w, h, clut, pix) {
            Some(s) => s,
            None => {
                failed += 1;
                EMPTY_SLOT
            }
        };
        unsafe {
            ptr::write(slots.add(i), slot);
        }
    }
    failed
}

fn upload_one(w: u16, h: u16, clut_bytes: &[u8], pix: &[u8]) -> Option<TexSlot> {
    unsafe {
        let pl = ATLAS.allocate(w, h)?;
        let page = pl.page_index();
        let tpage_x = TEX_X0 + (page % COLS) * 64;
        let tpage_y = if page / COLS == 0 { 0 } else { 256 };
        let tpage = Tpage::new(tpage_x, tpage_y, TexDepth::Bit4);
        // 4-bit pixels pack 4 texels per VRAM halfword -> w/4 halfwords wide.
        let vram_x = tpage_x + (pl.origin_u() as u16) / 4;
        let vram_y = tpage_y + pl.origin_v() as u16;
        if !rect_fits_vram(vram_x, vram_y, w / 4, h) {
            return None;
        }
        upload_bytes(VramRect::new(vram_x, vram_y, w / 4, h), pix);

        let clut = loop {
            let candidate = CLUTS.alloc(16)?;
            if candidate.x() < CLUT_MAX_X {
                break candidate;
            }
            // Leaving the rest of the row marked occupied retires it, so the
            // next allocation continues on the following row. Everything at or
            // past `CLUT_MAX_X` belongs to a texture page or the fixed HUD page.
        };
        if !rect_fits_vram(clut.x(), clut.y(), 16, 1) {
            return None;
        }
        upload_bytes(VramRect::new(clut.x(), clut.y(), 16, 1), clut_bytes);

        let win = TextureWindow::power_of_two_tile(pl.origin_u(), pl.origin_v(), w as u8, h as u8);
        let material =
            TextureMaterial::opaque(clut.uv_clut_word(), tpage.uv_tpage_word(0), (128, 128, 128))
                .with_texture_window(win);
        let packet = TexturedGouraudPacketMaterial::from_texture(material);
        // Every texel index 0 -> a solid single-colour fill (the `black`
        // backdrop, now grey-lifted by the cook). Flag it for OT back-biasing.
        let backdrop = !pix.is_empty() && pix.iter().all(|&b| b == 0);
        Some(TexSlot {
            material,
            packet,
            valid: true,
            backdrop,
        })
    }
}

#[inline(always)]
fn rect_fits_vram(x: u16, y: u16, w: u16, h: u16) -> bool {
    w > 0
        && h > 0
        && (x as u32 + w as u32) <= VRAM_W as u32
        && (y as u32 + h as u32) <= VRAM_H as u32
}
