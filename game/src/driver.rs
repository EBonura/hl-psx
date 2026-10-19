//! The game's one [`Gpu`] driver.
//!
//! Immediate drawing, the display flip, and the kicks of the frame's ordering
//! tables all reach GP0, GP1 or channel 2 from places that share no frame-loop
//! state: the deferred overlay tail runs after the next tick's simulation, the
//! HUD text helpers are called from a dozen render paths, and the loading strip
//! is drawn from inside map streaming. Threading `&mut Gpu` through every one
//! of those signatures would add a pointer argument to each render call for a
//! zero-sized handle, so the token lives here instead and callers borrow it
//! for the length of one call.
//!
//! The rule that keeps that sound is the one the borrow checker enforces
//! elsewhere: never hold the reference across a call that borrows it again, and
//! never draw immediately while a linked-list walk is running (callers fence
//! the walk with [`psx_gpu::chain::wait`] first, as they did before).

use core::mem::MaybeUninit;
use core::ptr::addr_of_mut;
use psx_gpu::material::{BlendMode, TextureMaterial};
use psx_gpu::prim::{QuadFlat, QuadTexturedMaterial, Sprite, TriFlat, TriTextured};
use psx_gpu::Gpu;

static mut DRIVER: MaybeUninit<Gpu> = MaybeUninit::uninit();

/// Park the driver. Called once, at boot, with the token `Gpu::new` consumed.
pub fn install(gpu: Gpu) {
    // SAFETY: single-threaded boot, before any caller of `gpu()` can run.
    unsafe {
        (*addr_of_mut!(DRIVER)).write(gpu);
    }
}

/// Run `f` with the driver.
///
/// The borrow cannot outlive the closure, so the one thing left to get wrong is
/// calling `with` from inside `f`; nothing here does. The game is
/// single-threaded and the VBlank handler never touches GP0, GP1 or channel 2
/// through this token.
#[inline(always)]
pub fn with<R>(f: impl FnOnce(&mut Gpu) -> R) -> R {
    // SAFETY: `install` ran at boot, and `f` is the only borrow while it runs.
    f(unsafe { (*addr_of_mut!(DRIVER)).assume_init_mut() })
}

// Out-of-line immediate draws. Each packet encode is a dozen stores, and a
// menu, pause or HUD site that inlines its own copy costs far more code than
// the call it saves: the SDK's old free functions were shared out-of-line
// code for the same reason, and the RAM budget counts every byte of `.text`.

/// A flat quad, now.
#[inline(never)]
pub fn quad_flat(verts: [(i16, i16); 4], r: u8, g: u8, b: u8) {
    with(|gpu| gpu.draw(&QuadFlat::new(verts, r, g, b)));
}

/// GP0(E1h) (the semi-transparency equation) and then a flat semi-transparent quad
/// (GP0(2Ah)) as one packet: written out, because the SDK's material route costs
/// several hundred bytes of image for the two rectangles the pause menu draws.
#[repr(C, align(4))]
struct QuadBlended {
    tag: u32,
    mode: u32,
    command: u32,
    verts: [u32; 4],
}

// SAFETY: `repr(C)`, 4-byte aligned, a tag word and then six initialised `u32`
// payload words that nothing mutates while the packet is borrowed.
unsafe impl psx_gpu::prim::GpuPacket for QuadBlended {
    const WORDS: u8 = 6;
}

/// A flat quad drawn through a semi-transparency equation, now: `abr` is GP0(E1h)
/// bits 5..6 (0 average, 1 add, 2 subtract, 3 add a quarter).
#[inline(never)]
pub fn quad_flat_blended(verts: [(i16, i16); 4], rgb: (u8, u8, u8), abr: u32) {
    let pack = |(x, y): (i16, i16)| (y as u16 as u32) << 16 | x as u16 as u32;
    with(|gpu| {
        gpu.draw(&QuadBlended {
            tag: 0,
            // Drawing to the display area allowed (bit 10); no texture page, no dither.
            mode: 0xE100_0400 | abr << 5,
            command: 0x2A00_0000 | (rgb.2 as u32) << 16 | (rgb.1 as u32) << 8 | rgb.0 as u32,
            verts: [
                pack(verts[0]),
                pack(verts[1]),
                pack(verts[2]),
                pack(verts[3]),
            ],
        })
    });
}

/// GP0(80h), a VRAM-to-VRAM rectangle copy. The SDK has no packet for it; the
/// pause menu is the only user.
#[repr(C, align(4))]
struct VramCopy {
    tag: u32,
    command: u32,
    from: u32,
    to: u32,
    size: u32,
}

// SAFETY: `repr(C)`, 4-byte aligned, a tag word and then four initialised `u32`
// payload words that nothing mutates while the packet is borrowed.
unsafe impl psx_gpu::prim::GpuPacket for VramCopy {
    const WORDS: u8 = 4;
}

/// Copy a rectangle of VRAM, now: `size` pixels from `from` to `to`, each a
/// top-left (x, y). The copy follows the mask-bit mode, so a caller that
/// needs every pixel copied clears it first (`MaskMode::empty()`).
#[inline(never)]
pub fn copy_vram(from: (u16, u16), to: (u16, u16), size: (u16, u16)) {
    let pack = |(x, y): (u16, u16)| (y as u32) << 16 | x as u32;
    with(|gpu| {
        gpu.draw(&VramCopy {
            tag: 0,
            command: 0x8000_0000,
            from: pack(from),
            to: pack(to),
            size: pack((size.0, size.1)),
        })
    });
}

/// A flat triangle, now. A translucent mode first writes its draw mode.
#[inline(never)]
pub fn tri_flat_blended(verts: [(i16, i16); 3], r: u8, g: u8, b: u8, blend_mode: BlendMode) {
    with(|gpu| {
        if !blend_mode.is_translucent() {
            gpu.draw(&TriFlat::new(verts, r, g, b));
            return;
        }
        gpu.set_draw_mode(TextureMaterial::blended(0, 0, (r, g, b), blend_mode));
        gpu.draw(&TriFlat::new(verts, r, g, b).translucent());
    });
}

/// A textured quad, now.
#[inline(never)]
pub fn quad_textured_material(
    verts: [(i16, i16); 4],
    uvs: [(u8, u8); 4],
    material: TextureMaterial,
) {
    with(|gpu| gpu.draw(&QuadTexturedMaterial::with_material(verts, uvs, material)));
}

/// A textured triangle, now.
#[inline(never)]
pub fn tri_textured_material(
    verts: [(i16, i16); 3],
    uvs: [(u8, u8); 3],
    material: TextureMaterial,
) {
    with(|gpu| gpu.draw(&TriTextured::with_material(verts, uvs, material)));
}

/// A textured rectangle, now, after its material's draw mode.
#[inline(never)]
pub fn sprite_material(x: i16, y: i16, w: u16, h: u16, uv: (u8, u8), material: TextureMaterial) {
    with(|gpu| {
        gpu.set_draw_mode(material);
        gpu.draw(&Sprite::with_material(x, y, w, h, uv, material));
    });
}
