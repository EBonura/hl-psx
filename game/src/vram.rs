//! Upload the cooked map textures to VRAM once at load, using the SDK's
//! `psx-vram` atlas + CLUT allocators, and hand back a per-texture material
//! the renderer feeds straight into `TriTextured`.
//!
//! Layout: 4-bit texture pages in the texture region X=320.. (band Y=0); CLUTs
//! in the free band Y=480.. (framebuffers own X=0..319 Y=0..479). All 164 c1a0
//! textures are <=64x64, so one band of 11 pages (16 textures each) holds them.

use psx_gpu::material::{TextureMaterial, TextureWindow, TexturedGouraudPacketMaterial};
use psx_vram::{upload_bytes, ClutRowAllocator, TexDepth, TextureWindowAtlas, Tpage, VramRect};

use crate::map::Map;
use core::ptr;

const TEX_X0: u16 = 320; // first texture-page column (after the framebuffers)
const COLS: u16 = 11; // X=320,384,..,960
const PAGES: usize = 22; // two bands (Y=0 and Y=256): map textures + model textures
const CLUT_ROWS: usize = 24; // Y=480..503
const CLUT_BASE_Y: u16 = 480;

#[derive(Copy, Clone)]
pub struct TexSlot {
    pub packet: TexturedGouraudPacketMaterial,
    pub valid: bool,
}

const EMPTY_MATERIAL: TextureMaterial = TextureMaterial::opaque(0, 0, (128, 128, 128));

pub const EMPTY_SLOT: TexSlot = TexSlot {
    packet: TexturedGouraudPacketMaterial::from_texture(EMPTY_MATERIAL),
    valid: false,
};

static mut ATLAS: TextureWindowAtlas<PAGES> = TextureWindowAtlas::new();
static mut CLUTS: ClutRowAllocator<CLUT_ROWS> = ClutRowAllocator::new(CLUT_BASE_Y);

/// Upload every cooked map texture, filling `slots`. Resets the VRAM allocators
/// first so reloading a map (return-to-menu) starts from a clean atlas instead
/// of running the cursors off the end.
pub fn upload_textures(map: &Map, slots: &mut [TexSlot]) -> usize {
    unsafe { upload_textures_raw(map, slots.as_mut_ptr(), slots.len()) }
}

/// Raw-pointer variant for filling `static mut` slot tables without creating
/// references to those statics.
pub unsafe fn upload_textures_raw(map: &Map, slots: *mut TexSlot, slot_len: usize) -> usize {
    unsafe {
        ATLAS = TextureWindowAtlas::new();
        CLUTS = ClutRowAllocator::new(CLUT_BASE_Y);
    }
    unsafe { upload_tex_blob_raw(map.tex_blob(), map.n_texs, slots, slot_len) }
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
        upload_bytes(VramRect::new(vram_x, vram_y, w / 4, h), pix);

        let clut = CLUTS.alloc(16)?;
        upload_bytes(VramRect::new(clut.x(), clut.y(), 16, 1), clut_bytes);

        let win = TextureWindow::power_of_two_tile(pl.origin_u(), pl.origin_v(), w as u8, h as u8);
        let material =
            TextureMaterial::opaque(clut.uv_clut_word(), tpage.uv_tpage_word(0), (128, 128, 128))
                .with_texture_window(win);
        let packet = TexturedGouraudPacketMaterial::from_texture(material);
        Some(TexSlot {
            packet,
            valid: true,
        })
    }
}
