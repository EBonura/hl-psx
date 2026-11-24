//! CD music: XA-ADPCM songs played through the SDK's drive-side player.
//!
//! The cooker packs the 27 CD tracks as four-song XA files (`hl_format::music`).
//! The drive decodes the channel it is told to follow and feeds the SPU's CD
//! input, so the CPU only starts, stops and watches the head. Like CD-DA, a
//! song is a read: any WORLD.PAK read preempts it, so main.rs suspends the
//! music before each stream and starts it again afterwards, at the position
//! it had reached.
//!
//! A song plays once (Half-Life cues do not loop). Every song of a file lasts
//! as long as the file's longest one, so a short song is followed by silence
//! until the file ends.

use hl_format::music as layout;
use psx_io::cd::xa::{DriveSpeed, Event, File, Player};
use psx_io::iso;
use psx_io::periph::Cd;
use psx_pack::cd::SECTOR_WORDS;

/// The drive's mixer level for the songs (`0x80` is unity).
const DRIVE_VOLUME: u8 = 0x80;
/// Sectors a second of single-speed audio takes.
const SECTORS_PER_SECOND: u32 = 75;

static mut PLAYER: Option<Player> = None;
/// First sector and sector count of each music file, `(0, 0)` where the disc
/// has none.
static mut DIRECTORY: [(u32, u32); layout::FILE_COUNT] = [(0, 0); layout::FILE_COUNT];
/// Sector of the song file at which the song now playing was started.
static mut STARTED_AT: u32 = 0;

/// What starting a song came to.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Start {
    /// The song plays.
    Playing,
    /// The disc has no such song; there is nothing to retry.
    Missing,
    /// The drive refused a command; try again later.
    Refused,
}

#[optimize(size)]
/// Hand over the CD token the player drives.
pub fn install(cd: Cd) {
    // SAFETY: single-threaded boot code, before anything plays.
    unsafe { PLAYER = Some(Player::new(cd)) };
}

/// Find the music files by name in the disc's root directory, so no sector
/// number is baked into the executable. `sector` is scratch for the reads.
///
/// # Safety
/// Drives the CD controller: call it once at boot, before any other CD use.
#[optimize(size)]
#[inline(never)]
pub unsafe fn locate_files(sector: &mut [u32; SECTOR_WORDS]) {
    // The root directory of this disc is one sector (the music files, the game file and a
    // handful of others), so the first sector is all there is to scan.
    let read = |lba, into: &mut [u32; SECTOR_WORDS]| crate::cdstream::core::read(lba, into);
    if !iso::read_root_sector(sector, read) {
        return;
    }
    let directory = iso::as_bytes(sector);
    for (file, name) in layout::FILE_NAMES.iter().enumerate() {
        if let Some((lba, bytes)) = iso::find_in_directory(directory, name) {
            DIRECTORY[file] = (lba, bytes / 2048);
        }
    }
}

#[optimize(size)]
/// Start `track`'s song `from_sector` sectors into its file (0 for the top),
/// with the drive's own mixer at unity. The SPU's CD input is the caller's.
pub unsafe fn start(track: u8, from_sector: u32) -> Start {
    let Some((file, channel)) = layout::locate(track) else {
        return Start::Missing;
    };
    let (lba, sectors) = DIRECTORY[file];
    let Some(player) = (*core::ptr::addr_of_mut!(PLAYER)).as_mut() else {
        return Start::Missing;
    };
    if sectors == 0 {
        return Start::Missing;
    }
    // A file that begins part-way into the song makes the resumed song end
    // where the whole one does.
    let from = from_sector.min(sectors.saturating_sub(1));
    let song = File::new(
        lba + from,
        sectors - from,
        layout::file_number(file),
        DriveSpeed::Single,
    )
    .song(channel);
    player.set_volume(DRIVE_VOLUME, DRIVE_VOLUME);
    if player.play(song, false).is_ok() {
        STARTED_AT = from;
        Start::Playing
    } else {
        Start::Refused
    }
}

#[optimize(size)]
/// Ask the drive where the head is. Notices the end of the song, which
/// pauses the drive. Call every few frames while a song plays.
pub unsafe fn poll() {
    if let Some(player) = (*core::ptr::addr_of_mut!(PLAYER)).as_mut() {
        player.poll();
    }
}

#[optimize(size)]
/// Where the song now playing has reached, as a sector of its file, or
/// `None` once it has finished. Polls the drive first.
pub unsafe fn position() -> Option<u32> {
    let player = (*core::ptr::addr_of_mut!(PLAYER)).as_mut()?;
    let event = player.poll();
    if event == Event::Finished || !player.is_playing() {
        return None;
    }
    // Rounded down to a whole interleave group, so the channel stays in step.
    let here = STARTED_AT + player.elapsed_millis() * SECTORS_PER_SECOND / 1000;
    Some(here & !(layout::SONGS_PER_FILE as u32 - 1))
}

#[optimize(size)]
/// Pause the drive and forget the song. Returns without waiting for the drive;
/// [`poll_stop`] finishes the Pause.
pub unsafe fn stop() {
    if let Some(player) = (*core::ptr::addr_of_mut!(PLAYER)).as_mut() {
        player.stop();
    }
}

#[optimize(size)]
/// Look at a stop's pending Pause once. Call every tick.
pub unsafe fn poll_stop() {
    if let Some(player) = (*core::ptr::addr_of_mut!(PLAYER)).as_mut() {
        player.poll_stop();
    }
}

#[optimize(size)]
/// Pause the drive and forget the song, waiting for the drive: before a data
/// read, and on the way to the menu.
pub unsafe fn stop_and_wait() {
    if let Some(player) = (*core::ptr::addr_of_mut!(PLAYER)).as_mut() {
        player.stop_and_wait();
    }
}
