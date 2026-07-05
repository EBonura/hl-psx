//! User options: screen position (TV centering) + music/SFX volume. Held in RAM
//! -- they persist across menu<->play within a session and reset on power cycle
//! (no memory-card save). Modelled on the Celeste Classic Collection's options.

use psx_io::gpu::write_gp1;
use psx_spu::{CdVolume, Volume};

pub const VOL_MAX: u8 = 8; // slider steps 0..8
pub const SCREEN_RANGE: i32 = 24; // +-24 px/lines of screen shift

pub static mut SCREEN_X: i32 = 0; // horizontal TV shift, px
pub static mut SCREEN_Y: i32 = 0; // vertical TV shift, lines
pub static mut MUSIC_VOL: u8 = VOL_MAX; // CD-DA (music) 0..8
pub static mut SFX_VOL: u8 = VOL_MAX; // SPU voices (SFX + speech) 0..8

// NTSC 320x240 display-window defaults (must match psx_gpu::init).
const H_START: i32 = 0x260;
const H_CLK: i32 = 8; // GPU clocks per pixel
const V_START: i32 = 0x10;
const W: i32 = 320;
const H: i32 = 240;

/// Re-issue the GP1 06h/07h display windows shifted by the screen offset. Moves
/// the picture on the TV without touching the VRAM layout (so it also shifts the
/// menu -- a live preview of the setting).
pub fn apply_display() {
    let (sx, sy) = unsafe { (SCREEN_X, SCREEN_Y) };
    let h0 = (H_START + sx * H_CLK).max(0) as u32;
    let h1 = h0 + (W * H_CLK) as u32;
    let v0 = (V_START + sy).max(0) as u32;
    let v1 = v0 + H as u32;
    write_gp1(0x0600_0000 | (h1 << 12) | h0); // horizontal display range
    write_gp1(0x0700_0000 | (v1 << 10) | v0); // vertical display range
}

/// Apply the audio volumes: music = CD input gain, SFX = SPU main (voice) volume.
pub fn apply_audio() {
    let (m, s) = unsafe { (MUSIC_VOL as i32, SFX_VOL as i32) };
    let cd = CdVolume((0x7FFF * m / VOL_MAX as i32) as i16);
    let vo = Volume((0x3FFF * s / VOL_MAX as i32) as i16);
    psx_spu::set_cd_volume(cd, cd);
    psx_spu::set_main_volume(vo, vo);
}

pub fn apply_all() {
    apply_display();
    apply_audio();
}

/// The CD (music) volume for the current setting -- music_apply() re-issues this
/// on each track change (so it must honour the slider, not force MAX).
pub fn music_cd_volume() -> CdVolume {
    let m = unsafe { MUSIC_VOL as i32 };
    CdVolume((0x7FFF * m / VOL_MAX as i32) as i16)
}

/// Adjust a setting by index (0 screen X, 1 screen Y, 2 music, 3 SFX) by `delta`
/// and apply it live. Returns the new value (for the slider readout).
pub fn adjust(index: usize, delta: i32) -> i32 {
    unsafe {
        match index {
            0 => {
                SCREEN_X = (SCREEN_X + delta).clamp(-SCREEN_RANGE, SCREEN_RANGE);
                apply_display();
                SCREEN_X
            }
            1 => {
                SCREEN_Y = (SCREEN_Y + delta).clamp(-SCREEN_RANGE, SCREEN_RANGE);
                apply_display();
                SCREEN_Y
            }
            2 => {
                MUSIC_VOL = (MUSIC_VOL as i32 + delta).clamp(0, VOL_MAX as i32) as u8;
                apply_audio();
                MUSIC_VOL as i32
            }
            3 => {
                SFX_VOL = (SFX_VOL as i32 + delta).clamp(0, VOL_MAX as i32) as u8;
                apply_audio();
                SFX_VOL as i32
            }
            _ => 0,
        }
    }
}

pub fn value(index: usize) -> i32 {
    unsafe {
        match index {
            0 => SCREEN_X,
            1 => SCREEN_Y,
            2 => MUSIC_VOL as i32,
            _ => SFX_VOL as i32,
        }
    }
}
