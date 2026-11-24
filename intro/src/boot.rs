//! Boot-only memory: the entry that moves it into place, and the intro's own
//! code that runs from it.
//!
//! The SDK's psoxide-boot.ld puts `psx_splash`, `psx_fmv` and any `.boot_only`
//! section in `.boot`, run from `__bss_end` up. `__boot_entry` is the EXE's
//! initial PC: it moves the stored bytes from `__boot_lma` up to their run
//! address (from the end backwards, the two ranges may overlap; nothing to do
//! when they are the same, which is how build.rs links it), zeroes the
//! boot-only `.bss` and goes on to `_start`, which clears `.bss`.
//! (An SDK whose psx-rt defines `__boot_entry` supplies the mover instead; this copy is built
//! only for older pins.)
//! It uses no stack and leaves the loader's argument registers alone. Nothing
//! in the boot range has run since the BIOS flushed the I-cache for this EXE,
//! and `main` installs the VBlank counter (which flushes it again) before the
//! splash, so no stale line can be executed.

use psx_font::fonts::BASIC_8X16;
use psx_font::FontAtlas;
use psx_gpu::display::DoubleBuffer;
use psx_gpu::Gpu;
use psx_pad::button;
use psx_rt::interrupts;
use psx_vram::{Clut, TextureDepth, TexturePage};

use crate::Skip;

#[cfg(all(target_arch = "mips", not(sdk_boot_entry)))]
core::arch::global_asm!(
    r#"
    .section .text.__boot_entry,"ax",@progbits
    .globl __boot_entry
    .set push
    .set noreorder
    .set noat
__boot_entry:
    lui   $t0, %hi(__boot_lma)
    addiu $t0, $t0, %lo(__boot_lma)
    lui   $t1, %hi(__boot_start)
    addiu $t1, $t1, %lo(__boot_start)
    lui   $t2, %hi(__boot_load_end)
    addiu $t2, $t2, %lo(__boot_load_end)
    beq   $t0, $t1, 2f
    subu  $t3, $t2, $t1
    addu  $t4, $t0, $t3
1:  beq   $t2, $t1, 2f
    nop
    addiu $t4, $t4, -4
    lw    $t5, 0($t4)
    addiu $t2, $t2, -4
    b     1b
    sw    $t5, 0($t2)
2:  lui   $t1, %hi(__boot_load_end)
    addiu $t1, $t1, %lo(__boot_load_end)
    lui   $t2, %hi(__boot_end)
    addiu $t2, $t2, %lo(__boot_end)
3:  beq   $t1, $t2, 4f
    nop
    sw    $zero, 0($t1)
    b     3b
    addiu $t1, $t1, 4
4:  j     _start
    nop
    .set pop
"#
);

const FONT_TPAGE: TexturePage = TexturePage::new(320, 0, TextureDepth::Bit4);
const FONT_CLUT: Clut = Clut::new(320, 256);
/// Frames the notice holds unless a press ends it.
const NOTICE_FRAMES: i32 = 120;
/// Frames before a press can end it, so the press that ended the splash does not.
const NOTICE_GRACE: i32 = 8;

/// A pad that is not a DualShock in analog mode: two lines after the splash, before the movie.
/// The SDK splash takes no text but its one line, so the notice HL's own splash carried under
/// the tag is a screen of its own, shown only to such a pad.
#[inline(never)]
#[link_section = ".text.boot_only.pad_notice"]
pub fn pad_notice(gpu: &mut Gpu, fb: &mut DoubleBuffer, skip: &mut Skip, lines: [&str; 2]) {
    let font = FontAtlas::upload(&BASIC_8X16, FONT_TPAGE, FONT_CLUT);
    let ends = button::CROSS | button::CIRCLE | button::START;
    let mut held = true;
    for frame in 0..NOTICE_FRAMES {
        let now = skip.buttons().is_held(ends);
        if frame > NOTICE_GRACE && now && !held {
            break;
        }
        held = now;
        fb.clear(gpu, (0, 0, 0));
        let mut y = 104;
        for line in lines {
            let x = 160 - font.text_width(line) as i16 / 2;
            font.draw_text(x, y, line, (96, 64, 32));
            y += 18;
        }
        gpu.wait_idle();
        interrupts::wait_vblank();
        fb.swap(gpu);
    }
}

/// Leave both buffers black, so the movie starts from black.
#[inline(never)]
#[link_section = ".text.boot_only.black"]
pub fn black(gpu: &mut Gpu, fb: &mut DoubleBuffer) {
    for _ in 0..2 {
        fb.clear(gpu, (0, 0, 0));
        gpu.wait_idle();
        interrupts::wait_vblank();
        fb.swap(gpu);
    }
}
