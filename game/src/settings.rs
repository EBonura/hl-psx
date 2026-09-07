//! User options: screen position (TV centering), brightness, music/SFX volume,
//! analog-stick deadzone, and transition autosaves. Held in RAM -- they persist
//! across menu<->play within a session and reset on power cycle (no memory-card
//! save). Modelled on the Celeste Classic Collection's options.

use psx_pad::Deadzone;

use psx_gpu::{set_display_offset, Resolution, VideoMode};
use psx_spu::{CdVolume, Volume};

pub const VOL_MAX: u8 = 8; // slider steps 0..8
pub const SCREEN_RANGE: i32 = 24; // +-24 px/lines of screen shift
pub const ANALOG_DEADZONE_DEFAULT: u8 = 28;
pub const ANALOG_DEADZONE_MAX: u8 = 64;

pub static mut SCREEN_X: i32 = 0; // horizontal TV shift, px
pub static mut SCREEN_Y: i32 = 0; // vertical TV shift, lines
pub static mut MUSIC_VOL: u8 = VOL_MAX; // CD-DA (music) 0..8
pub static mut SFX_VOL: u8 = VOL_MAX; // SPU voices (SFX + speech) 0..8
static mut ANALOG_DEADZONE: u8 = ANALOG_DEADZONE_DEFAULT; // radial, axis units 0..128
                                                          // psx_pad::Deadzone owns the radial test and squares the radius itself, so
                                                          // there is one implementation of that shape rather than one per game.
static mut ANALOG_DEADZONE_ZONE: Deadzone = Deadzone::new(ANALOG_DEADZONE_DEFAULT as i16);
static mut AUTOSAVE: bool = false; // opt in: level changes do not write by default

// ---- Brightness ------------------------------------------------------------
// Quake-PSX cooks six gamma'd copies of its one shared 256-colour palette into
// consecutive VRAM CLUT rows, so its slider is a row index in the texture-page
// word. Half-Life has no shared palette to copy: every cooked 4bpp texture owns
// a private 16-entry CLUT (see `vram::upload_one`), so the same trick would
// need six times the CLUT band. The band is 32 VRAM rows below the
// framebuffers, most of them already shared with texture pages, against a peak
// of roughly 400 resident textures: six variants do not fit and no amount of
// repacking makes them. What Half-Life does have is the equivalent choke point
// one stage later -- the per-map 256-entry light palette every lit world corner is
// looked up in, plus the single `shade` byte each studio model draw modulates
// with. Applying the curve there costs nothing per pixel and nothing per frame:
// the world palette is expanded once per map load (and once more when the level
// changes), and the model paths pay one call that returns immediately at the
// default level.

pub const BRIGHTNESS_LEVELS: u8 = 8;
/// Default to displayed level 2 (zero-based index 1).
/// Index 4 preserves cooked lighting; lower levels darken and higher levels
/// lift mid-tones. The full eight-level range remains available in Options.
pub const DEFAULT_BRIGHTNESS: u8 = 1;

/// Q8 blend weight from `v` toward the curve target for each level: negative
/// pulls toward `v^2` (darker), positive toward `sqrt(v)` (brighter), and zero
/// is the untouched palette. Both targets are exact at 0 and 255, so no level
/// crushes black or blows out white.
/// Levels 7 and 8 continue the bright side at the same 112 spacing as level 6.
/// The weight may exceed 256: the blend stays monotonic and reaches exactly 255
/// at v=255 for any weight up to about 512, so nothing wraps through `as u8`.
/// Past that the curve overshoots and clips, so do not extend this table
/// further without redoing that check.
const BRIGHTNESS_MIX: [i16; BRIGHTNESS_LEVELS as usize] = [-224, -168, -112, -56, 0, 112, 224, 336];

static mut BRIGHTNESS: u8 = DEFAULT_BRIGHTNESS;
// Cached `BRIGHTNESS_MIX[BRIGHTNESS]`, so the per-draw entry point is one static
// load and a branch rather than a bounds-checked table index. Keep the cached
// initializer tied to the default so boot and the Options row cannot disagree.
static mut BRIGHT_MIX: i16 = BRIGHTNESS_MIX[DEFAULT_BRIGHTNESS as usize];
// Set when the level changes with a map already resident; the pause menu is the
// only way that can happen, so it is consumed there rather than per frame.
static mut BRIGHT_DIRTY: bool = false;

/// Integer square root of `n` (`n < 2^16`), used only when the palette is
/// rebuilt or a model shade is lifted.
#[inline(always)]
fn isqrt16(mut n: u32) -> i32 {
    let mut root = 0u32;
    let mut bit = 1u32 << 14;
    while bit > n {
        bit >>= 2;
    }
    while bit != 0 {
        if n >= root + bit {
            n -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root as i32
}

/// Apply the current brightness curve to one 8-bit light or shade channel. At
/// the default level this is a load, a compare and a return; the curve itself
/// is out of line and out of the hot text bucket, so selecting a non-default
/// level costs image size but never disturbs the world loop's cache layout.
#[inline]
pub fn bright(v: u8) -> u8 {
    let mix = unsafe { BRIGHT_MIX };
    if mix == 0 {
        return v;
    }
    bright_curve(v, mix)
}

/// Out-of-line [`bright`], for the palette expansion. Inlining the gate at all
/// six call sites there costs several hundred bytes of image -- and image bytes
/// come straight out of the RAM left above `.bss` -- to save a jump on a path
/// that runs twice per map.
#[inline(never)]
#[link_section = ".hlpsx_cold.brightness"]
pub fn bright_cold(v: u8) -> u8 {
    bright(v)
}

#[inline(never)]
#[link_section = ".hlpsx_cold.brightness"]
fn bright_curve(v: u8, mix: i16) -> u8 {
    let x = v as i32;
    // sqrt(255 * x) is 255 * sqrt(x / 255) without leaving integers; the dark
    // target is the matching x^2 / 255. Both bracket x, so the blend stays in
    // range without a clamp.
    let target = if mix > 0 {
        isqrt16((x as u32) * 255)
    } else {
        (x * x + 127) / 255
    };
    let weight = mix.unsigned_abs() as i32;
    (x + (((target - x) * weight) >> 8)) as u8
}

/// Select a brightness level. Marks the resident map's light palette stale; the
/// menu that changed it re-expands on the way out.
pub fn set_brightness_level(level: u8) {
    let level = level.min(BRIGHTNESS_LEVELS - 1);
    unsafe {
        if BRIGHTNESS == level {
            return;
        }
        BRIGHTNESS = level;
        BRIGHT_MIX = BRIGHTNESS_MIX[level as usize];
        BRIGHT_DIRTY = true;
    }
}

/// Consume the "light palette needs re-expanding" flag.
pub fn take_brightness_dirty() -> bool {
    unsafe {
        let dirty = BRIGHT_DIRTY;
        BRIGHT_DIRTY = false;
        dirty
    }
}

/// Re-issue the GP1 06h/07h display windows shifted by the screen offset. Moves
/// the picture on the TV without touching the VRAM layout (so it also shifts the
/// menu -- a live preview of the setting). Mode and resolution must match what
/// main passes to gpu::init.
pub fn apply_display() {
    let (sx, sy) = unsafe { (SCREEN_X, SCREEN_Y) };
    set_display_offset(VideoMode::Ntsc, Resolution::R320X240, sx as i16, sy as i16);
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

/// Adjust a setting by index (0 screen X, 1 screen Y, 2 music, 3 SFX,
/// 4 analog deadzone, 6 brightness) by `delta` and apply it live. Returns the
/// new value.
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
            4 => {
                ANALOG_DEADZONE =
                    (ANALOG_DEADZONE as i32 + delta).clamp(0, ANALOG_DEADZONE_MAX as i32) as u8;
                ANALOG_DEADZONE_ZONE = Deadzone::new(ANALOG_DEADZONE as i16);
                ANALOG_DEADZONE as i32
            }
            6 => {
                let level = (BRIGHTNESS as i32 + delta).clamp(0, BRIGHTNESS_LEVELS as i32 - 1);
                set_brightness_level(level as u8);
                BRIGHTNESS as i32 + 1
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
            3 => SFX_VOL as i32,
            4 => ANALOG_DEADZONE as i32,
            5 => AUTOSAVE as i32,
            // Shown 1..6 so the row reads like Quake's, not 0-based.
            6 => BRIGHTNESS as i32 + 1,
            _ => 0,
        }
    }
}

/// The configured stick dead region.
#[inline(always)]
pub fn analog_deadzone() -> Deadzone {
    unsafe { ANALOG_DEADZONE_ZONE }
}

#[inline(always)]
pub fn autosave_enabled() -> bool {
    unsafe { AUTOSAVE }
}

pub fn toggle_autosave() {
    unsafe { AUTOSAVE = !AUTOSAVE };
}

// ---- Debug toggles ---------------------------------------------------------
// Session-only cheats, reachable from the main-menu Options screen and the
// in-game pause menu. Held here beside the other RAM-only options so both
// menus and the gameplay loop read one source of truth. They persist across
// menu<->play like the sliders do, because arming one from the main menu
// before starting a level is the point.

pub const DEBUG_FLY: u8 = 1 << 0; // noclip: free flight, no collision or gravity
pub const DEBUG_WEAPONS: u8 = 1 << 1; // every weapon, fully loaded
pub const DEBUG_GOD: u8 = 1 << 2; // no damage, topped up on enable
pub const DEBUG_STATS: u8 = 1 << 3; // position / leaf / room / fps overlay

/// Menu order; the index is the bit position, so row `i` toggles `1 << i`.
pub const DEBUG_LABELS: [&str; 4] = ["Fly Mode", "All Weapons", "God Mode", "Show Stats"];
// The pause menu draws one centred string per row, so its labels carry the
// state inline instead of a separate value column.
pub const DEBUG_LABELS_ON: [&str; 4] = [
    "Fly Mode: ON",
    "All Weapons: ON",
    "God Mode: ON",
    "Show Stats: ON",
];
pub const DEBUG_LABELS_OFF: [&str; 4] = [
    "Fly Mode: OFF",
    "All Weapons: OFF",
    "God Mode: OFF",
    "Show Stats: OFF",
];

static mut DEBUG_FLAGS: u8 = 0;

#[inline(always)]
pub fn debug_on(bit: u8) -> bool {
    unsafe { DEBUG_FLAGS & bit != 0 }
}

pub fn debug_toggle(index: usize) {
    if index < DEBUG_LABELS.len() {
        unsafe { DEBUG_FLAGS ^= 1 << index };
    }
}

/// Arm a debug bit unconditionally. Camera-pin diagnostic builds use this so
/// every capture self-documents its pose in the stats overlay; shipping
/// builds fold the pin away and never call it.
#[allow(dead_code)]
pub fn debug_force(bit: u8) {
    unsafe { DEBUG_FLAGS |= bit };
}
