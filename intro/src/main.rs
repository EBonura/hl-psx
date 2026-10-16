//! The boot program of the hl-psx disc: Half-Life's startup movie, then the
//! game.
//!
//! The game's image has almost no RAM to spare, so the movie cannot live in
//! it. The BIOS boots this small program instead (`PSX.EXE`, linked high in
//! RAM, see build.rs). It streams `INTRO.STR` through the SDK's MDEC player,
//! with its XA audio, until the last frame or until Start or Cross skips it;
//! then the SDK's chain loader reads the game's own executable (`HLGAME.EXE`,
//! an ordinary file in the disc's root directory) over the low RAM this
//! program has been using as the player's buffers, checks its checksum in
//! RAM, and jumps to it. The game boots from there as it does from the BIOS:
//! it initialises everything it uses itself.
//!
//! The movie is the one Half-Life plays at startup, cooked by
//! host/hl-intro-cook from the player's own install. A disc built without it
//! has no intro stage and boots the game directly.
#![no_std]
#![no_main]

extern crate psx_rt;

use psx_chainloader::paint;
use psx_chainloader::runtime::{self, Presentation};
use psx_fmv::player::{self, Config, Depth, Movie, Stop};
use psx_gpu::display::{DisplayConfig, Resolution, VideoMode};
use psx_gpu::Gpu;
use psx_io::periph::ControllerPort;
use psx_pad::{button, Port};
use psx_rt::{interrupts, tty, Peripherals};
use psx_spu::{self as spu, Spu, Volume};

mod cooked {
    include!("../../.hlpsx/intro/movie.rs");
}

const MOVIE_FILE: &str = "INTRO.STR";
const GAME_FILE: &str = "HLGAME.EXE";

/// Where the player's buffers sit: low RAM above the kernel, which the game's
/// image overwrites once the movie is over. They end below this program.
const PLAYER_MEMORY: usize = 0x8004_0000;
/// The checksum of the game's payload (hl-build computes it from the image
/// it packs, see build.rs).
const GAME_FNV: u32 = parse_u32(env!("HL_INTRO_GAME_FNV_VALUE"));
/// No sector for two seconds ends the movie.
const STALL_VBLANKS: u32 = 120;

const fn parse_u32(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let mut value = 0u32;
    let mut i = 0;
    while i < bytes.len() {
        value = value * 10 + (bytes[i] - b'0') as u32;
        i += 1;
    }
    value
}

extern "C" {
    /// First byte of this program's image (psoxide.ld).
    static __text_start: u8;
}

fn image_start() -> u32 {
    (&raw const __text_start) as u32
}

/// Start or Cross, newly pressed since the poll before.
struct Skip {
    port: ControllerPort,
    held: bool,
    last_vblank: u32,
}

impl Skip {
    fn poll(&mut self) -> bool {
        // The pad answers once per VBlank at most; the player calls this far
        // more often.
        let now = interrupts::vblank_count();
        if now == self.last_vblank {
            return false;
        }
        self.last_vblank = now;
        let held = psx_pad::poll_on(&mut self.port, Port::One)
            .buttons
            .is_held(button::START | button::CROSS);
        let pressed = held && !self.held;
        self.held = held;
        pressed
    }
}

fn play_movie(port: ControllerPort, limit: u32) {
    let Some((lba, _)) = player::find_root_file(MOVIE_FILE) else {
        tty::println("hl-intro: no INTRO.STR on this disc");
        return;
    };
    let movie = Movie {
        lba,
        frames: cooked::FRAMES,
        width: cooked::WIDTH,
        height: cooked::HEIGHT,
        max_chunks: cooked::MAX_CHUNKS,
        rle_words: cooked::RLE_WORDS,
        xa: cooked::XA,
    };
    let config = Config {
        // 15-bit: the picture can be drawn over, and the player leaves the
        // display as it found it.
        depth: Depth::Rgb15,
        buffers: player::DEFAULT_BUFFERS,
        screen_width: 320,
        screen_height: 240,
        // The cook sized the sound's delay for this headroom (hl-intro-cook).
        latency_vblanks: cooked::LATENCY_VBLANKS,
        stall_vblanks: STALL_VBLANKS,
    };
    let words = player::memory_words(&movie, config.depth);
    if PLAYER_MEMORY + words * 4 > limit as usize {
        tty::println("hl-intro: the movie's buffers would reach the intro's own image");
        return;
    }
    // SAFETY: low RAM between the kernel's area and this image: nothing lives
    // there until the game is loaded over it, after the player has returned.
    let memory = unsafe { core::slice::from_raw_parts_mut(PLAYER_MEMORY as *mut u32, words) };
    let mut skip = Skip {
        port,
        held: true,
        last_vblank: interrupts::vblank_count(),
    };
    let outcome = player::play(&movie, &config, memory, &mut || skip.poll());
    report(&outcome);
}

fn report(o: &player::Outcome) {
    let stop = match o.stop {
        Stop::End => "end",
        Stop::Skipped => "skipped",
        Stop::Stall => "stall",
        Stop::CdError => "cd-error",
        Stop::Wedged => "wedged",
        Stop::Setup => "setup",
    };
    tty::print("INTRO ");
    tty::print(stop);
    for (label, v) in [
        ("shown", o.shown),
        ("skipped", o.skipped),
        ("late", o.late),
        ("worst_late", o.worst_late_vblanks),
        ("dropped", o.dropped),
        ("errors", o.decode_errors),
        ("last_frame", o.last_frame),
        ("vblanks", o.vblanks),
        ("kcyc_vlc", o.kcyc_vlc),
        ("kcyc_mdec", o.kcyc_mdec),
        ("kcyc_wait", o.kcyc_wait),
    ] {
        tty::print(" ");
        tty::print(label);
        tty::print("=");
        print_decimal(v);
    }
    tty::println("");
}

fn print_decimal(mut v: u32) {
    let mut digits = [0u8; 10];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    tty::print(core::str::from_utf8(&digits[at..]).unwrap_or("?"));
}

/// A dim track and a bar over it, at the foot of a black screen, while the
/// game's image streams in.
struct Loading;

const BAR_X: i16 = 80;
const BAR_Y: i16 = 222;
const BAR_W: i16 = 160;
const BAR_H: i16 = 4;
const TRACK: u32 = 0x0030_3030;
const FILL: u32 = 0x00B0_B0B0;

impl Presentation for Loading {
    fn setup(&mut self) {
        paint::setup(319, 239);
    }
    fn begin(&mut self) {
        paint::rect(0, 0, 320, 240, 0);
        paint::rect(BAR_X, BAR_Y, BAR_W, BAR_H, TRACK);
        paint::draw_sync();
        paint::show();
    }
    fn update(&mut self, done: u32, total: u32) {
        let width = (done * BAR_W as u32 / total.max(1)).min(BAR_W as u32) as i16;
        paint::rect(BAR_X, BAR_Y, width, BAR_H, FILL);
        paint::draw_sync();
    }
    fn finish(&mut self) {}
    fn diagnostic_mode(&mut self) {
        paint::diagnostic_mode();
    }
    fn no_scratchpad(&mut self) {}
}

#[no_mangle]
fn main() {
    interrupts::install_vblank_counter();
    let peripherals = Peripherals::take().expect("the intro takes the peripherals once, at boot");
    // The player and the chain loader take the drive, the MDEC and the GPU's
    // channel over themselves, as their docs say; the display and the SPU are
    // set up here.
    let _gpu = Gpu::new(
        peripherals.gpu_dma,
        DisplayConfig::new(VideoMode::Ntsc, Resolution::R320X240),
    );
    let _spu = Spu::new(peripherals.spu_dma);
    spu::set_main_volume(Volume::MAX, Volume::MAX);

    play_movie(peripherals.controller_port, image_start());

    let Some((lba, _)) = player::find_root_file(GAME_FILE) else {
        tty::println("hl-intro: no HLGAME.EXE on this disc");
        loop {
            interrupts::wait_vblank();
        }
    };
    // SAFETY: the game's payload ends below this image (hl-build checks it
    // when it packs the disc), the player has returned, and nothing else
    // uses the drive. `run` never returns.
    unsafe { runtime::run(lba, 0, 0, GAME_FNV, image_start(), Loading) }
}
