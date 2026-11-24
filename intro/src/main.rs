//! The boot program of the hl-psx disc: the SDK's boot splash, Half-Life's
//! startup movie, then the game.
//!
//! The game's image has almost no RAM to spare, so the movie cannot live in
//! it. The BIOS boots this small program instead (`PSX.EXE`, linked high in
//! RAM, see build.rs). It streams `INTRO.STR` through the SDK's MDEC player,
//! with its XA audio, until the last frame or until Start or Cross skips it;
//! then it enters the game (`HLGAME.EXE`) with a program switch through the
//! SDK's resident scene core, which loads the game's image over the low RAM
//! this program has been using as the player's buffers, checks it and jumps to
//! it. The game initialises everything it uses itself.
//!
//! The movie is the one Half-Life plays at startup, cooked by
//! host/hl-intro-cook from the player's own install. A disc built without it
//! (a test disc) has no intro stage and boots the game directly.
//!
//! The splash (`psx_splash`) and the movie player (`psx_fmv`) are boot-only
//! memory (the SDK's psoxide-boot.ld, see build.rs and boot.rs): the EXE
//! stores them after `.data`, and they run above `.bss`. Nothing after the
//! movie calls them, and the game's image takes that RAM once it is loaded.
#![no_std]
#![no_main]
// The boot-only mover is MIPS assembly (boot.rs).
#![cfg_attr(target_arch = "mips", feature(asm_experimental_arch))]

extern crate psx_rt;
// The scene core's program side: its image, `__core_boot` and the slots `enter_game` calls.
extern crate psx_core;

use psx_fmv::player::{self, Config, Depth, Movie, Stop};
use psx_gpu::display::{DisplayConfig, DoubleBuffer, Resolution, VideoMode};
use psx_gpu::Gpu;
use psx_font::fonts::BASIC_8X16;
use psx_io::periph::ControllerPort;
use psx_loading::{Gp0, Indicator, Rgb, Style, Tail};
use psx_pad::{button, Port};
use psx_rt::{interrupts, tty, Peripherals};
use psx_spu::{self as spu, Spu, Volume};

mod boot;

mod cooked {
    include!("../../.hlpsx/intro/movie.rs");
}

const MOVIE_FILE: &str = "INTRO.STR";

/// Where the player's buffers sit: low RAM above the kernel, which the game's
/// image overwrites once the movie is over. They end below this program.
const PLAYER_MEMORY: usize = 0x8004_0000;
/// No sector for two seconds ends the movie.
const STALL_VBLANKS: u32 = 120;

extern "C" {
    /// First byte of this program's image (psoxide.ld).
    static __text_start: u8;
}

fn image_start() -> u32 {
    (&raw const __text_start) as u32
}

/// Start or Cross, newly pressed since the poll before.
pub(crate) struct Skip {
    pads: psx_pad::Controllers,
    port: ControllerPort,
    held: bool,
    last_vblank: u32,
}

impl Skip {
    /// The pad's buttons now, polled on the spot.
    pub(crate) fn buttons(&mut self) -> psx_pad::ButtonState {
        self.pads.poll(&mut self.port, Port::One).buttons
    }

    fn poll(&mut self) -> bool {
        // The pad answers once per VBlank at most; the player calls this far
        // more often.
        let now = interrupts::vblank_count();
        if now == self.last_vblank {
            return false;
        }
        self.last_vblank = now;
        let held = self
            .pads
            .poll(&mut self.port, Port::One)
            .buttons
            .is_held(button::START | button::CROSS);
        let pressed = held && !self.held;
        self.held = held;
        pressed
    }
}

/// Boot-only (psoxide-boot.ld's `.boot`): the film player it calls is boot-only memory that
/// resident code may not reach.
#[inline(never)]
#[link_section = ".text.boot_only.play_movie"]
fn play_movie(skip: &mut Skip, limit: u32) {
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
    // A button still held from the splash must be let go before it can skip.
    skip.held = true;
    skip.last_vblank = interrupts::vblank_count();
    let outcome = player::play(&movie, &config, memory, &mut || skip.poll());
    report(&outcome);
}

/// The movie's outcome, published for tests that wait on game state (the cross-emulator smoke):
/// `[stop, frames shown]`, stop 0 until the player returns, then 1 + the `Stop` (1 end,
/// 2 skipped, 3 stall, 4 cd-error, 5 wedged, 6 setup). The game loads below this image, so the
/// words survive the hand-off until the game reuses that memory.
#[no_mangle]
static mut INTRO_MOVIE: [u32; 2] = [0; 2];

fn report(o: &player::Outcome) {
    let code = match o.stop {
        Stop::End => 1,
        Stop::Skipped => 2,
        Stop::Stall => 3,
        Stop::CdError => 4,
        Stop::Wedged => 5,
        Stop::Setup => 6,
    };
    // SAFETY: single-threaded guest; written once, after the player returned.
    unsafe { INTRO_MOVIE = [code, o.shown] };
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

#[no_mangle]
fn main() {
    interrupts::install_vblank_counter();
    let peripherals = Peripherals::take().expect("the intro takes the peripherals once, at boot");
    // The player and the core take the drive, the MDEC and the GPU's
    // channel over themselves, as their docs say; the display and the SPU are
    // set up here.
    let mut gpu = Gpu::new(
        peripherals.gpu_dma,
        DisplayConfig::new(VideoMode::Ntsc, Resolution::R320X240),
    );
    let _spu = Spu::new(peripherals.spu_dma);
    spu::set_main_volume(Volume::MAX, Volume::MAX);

    let mut skip = Skip {
        pads: psx_pad::Controllers::new(),
        port: peripherals.controller_port,
        held: true,
        last_vblank: interrupts::vblank_count(),
    };
    // The intro counts its waits in frames, one flip per VBlank, as it did before the splash
    // moved to the SDK (which lifts the cap for itself and puts back what it found).
    psx_rt::present::set_cap(psx_rt::present::PresentCap::UNCAPPED);
    // The first poll of a pad asks it for analog mode, so a pad that cannot be seen is told
    // before the movie, not after it. The game polls with its own Controllers and asks again.
    skip.pads.poll(&mut skip.port, Port::One);
    let notice = match skip.pads.analog(Port::One) {
        psx_pad::AnalogRequirement::Analog => None,
        psx_pad::AnalogRequirement::DigitalOnly => {
            Some(["Digital pad detected", "Half-Life needs a DualShock"])
        }
        psx_pad::AnalogRequirement::Absent => {
            Some(["No controller found", "Plug a DualShock into port 1"])
        }
    };
    let mut fb = DoubleBuffer::new(Resolution::R320X240);
    // The disc's one splash. The intro reads no memory card, so the brightness is the default.
    psx_splash::show(
        &mut gpu,
        &mut fb,
        "Built with PSoXide",
        psx_display::Brightness::DEFAULT,
        || skip.buttons(),
    );
    if let Some(lines) = notice {
        boot::pad_notice(&mut gpu, &mut fb, &mut skip, lines);
    }
    boot::black(&mut gpu, &mut fb);
    play_movie(&mut skip, image_start());

    show_loading_frame(&mut gpu);
    enter_game();
}

/// Half-Life's loading indicator, the same one the game's `LOADING_STRIP` draws for every
/// load: a small black square in the bottom-right corner with a ring of orange lights.
const LOADING_STRIP: Style = Style::new(&BASIC_8X16, "", Rgb(255, 170, 0), Rgb(0, 0, 0))
    .with_tail(Tail::Spinner {
        lit: Rgb(255, 214, 96),
        unlit: Rgb(40, 34, 20),
    })
    .in_corner(24, 320, 240, 12)
    .timed(2, 64);

/// Leave the loading strip, still, in the loading frame (VRAM 0..320 x 0..240) and show it.
/// The core holds that frame through the switch's load and the game's first CD reads, until
/// the game draws its own strip in the same corner and animates it. Without it the frame
/// held is the movie's last one, black, for about five seconds.
fn show_loading_frame(gpu: &mut Gpu) {
    gpu.set_draw_area((0, 0), (319, 239));
    gpu.set_draw_offset((0, 0));
    psx_loading::prepare(&mut Gp0);
    psx_loading::rect(&mut Gp0, 0, 0, 320, 240, Rgb(0, 0, 0));
    Indicator::new(LOADING_STRIP).draw(&mut Gp0, LOADING_STRIP.look(0));
    gpu.wait_idle();
    interrupts::wait_vblank();
    gpu.set_display_start((0, 0));
}

/// The game's scene in SCENES.TBL (hl-build's manifest: BOOT 0, GAME 1).
const GAME_SCENE: u32 = 1;
/// This program's copy of SCENES.TBL: psx-core reads it on the switch (header and up to six rows).
static mut TABLE: [u32; 16 + 8 * 6] = [0; 16 + 8 * 6];

/// Enter the game with a core program switch: the core loads HLGAME.EXE (its row in the
/// table), checks it and jumps to it with the disc base and the CD-DA base in the entry
/// registers, and the loaded mask then holds the game's bit, so overlays it owns load. The
/// switch returns only when the core refused it before touching anything.
fn enter_game() -> ! {
    // SAFETY: single-threaded; the one borrow lives for the call.
    let table = unsafe { &mut *core::ptr::addr_of_mut!(TABLE) };
    let code = psx_core::link::switch(GAME_SCENE, &[0; 4], table);
    tty::print("hl-intro: the core refused the switch to the game, code ");
    print_decimal(code);
    tty::println("");
    loop {
        interrupts::wait_vblank();
    }
}
