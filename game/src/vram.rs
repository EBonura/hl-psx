//! Upload the cooked map textures to VRAM once at load, using the SDK's
//! `psx-vram` atlas + CLUT allocators, and hand back a per-texture material
//! the renderer feeds straight into `TriTextured`.
//!
//! Layout: 4-bit texture pages in the texture region X=320.. (band Y=0); CLUTs
//! in the free band Y=480.. (framebuffers own X=0..319 Y=0..479). All 164 c1a0
//! textures are <=64x64, so one band of 11 pages (16 textures each) holds them.

use psx_gpu::material::{TextureMaterial, TextureWindow, TexturedGouraudPacketMaterial};
use psx_vram::{upload_bytes, ClutRowAllocator, TexDepth, TextureWindowAtlas, Tpage, VramRect};

use core::ptr;

const TEX_X0: u16 = 320; // first texture-page column (after the framebuffers)
const COLS: u16 = 11; // X=320,384,..,960
const PAGES: usize = 22; // two bands (Y=0 and Y=256): map textures + model textures
const CLUT_ROWS: usize = 24; // Y=480..503
const CLUT_BASE_Y: u16 = 480;
const VRAM_W: u16 = 1024;
const VRAM_H: u16 = 512;

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

static mut ATLAS: TextureWindowAtlas<PAGES> = TextureWindowAtlas::new();
static mut CLUTS: ClutRowAllocator<CLUT_ROWS> = ClutRowAllocator::new(CLUT_BASE_Y);

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

        let clut = CLUTS.alloc(16)?;
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
