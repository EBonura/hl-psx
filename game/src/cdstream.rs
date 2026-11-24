//! GoldSrc chunk streaming uses the SDK's owner (`psx_goldsrc::chunk_stream`): blocking loads and
//! the weapon-switch session. This facade supplies the game's render-arena lease and
//! persistent viewmodel-entry capacity.
pub use psx_goldsrc::chunk_stream::{ChunkLoad, StreamPump};

#[cfg(target_arch = "mips")]
mod target {
    use psx_goldsrc::chunk_stream::{CachedStreamer, PacketArena, CACHE_ENTRY_BYTES};
    struct Arena;
    // The game serializes loading and rendering on the main thread. Rendering
    // invalidates this overlay before reclaiming it; the arena keeps no packets
    // across frames. The progress hook draws the loading strip and never
    // reenters the chunk streamer.
    unsafe impl PacketArena for Arena {
        const CAPACITY: usize = crate::room_budget::PACK_CACHE_ENTRIES;
        #[inline(always)]
        unsafe fn storage() -> *mut u8 {
            core::ptr::addr_of_mut!(crate::PRIMITIVE_PACKETS).cast()
        }
        #[inline(always)]
        unsafe fn invalidate_render_cache() {}
    }
    const _: () = assert!(
        core::mem::size_of::<crate::RenderPacketScratch>() >= Arena::CAPACITY * CACHE_ENTRY_BYTES
    );
    #[cfg(not(feature = "scene-core"))]
    type Owner = CachedStreamer<psx_pack::cd::SectorReader, Arena, 32>;
    #[cfg(not(feature = "scene-core"))]
    static mut OWNER: Owner = unsafe { Owner::new(psx_pack::cd::SectorReader::new()) };
    #[cfg(feature = "scene-core")]
    type Owner = CachedStreamer<super::core::CoreReader, Arena, 32>;
    #[cfg(feature = "scene-core")]
    static mut OWNER: Owner = unsafe { Owner::new(super::core::CoreReader) };
    #[inline(always)]
    unsafe fn owner() -> &'static mut Owner {
        &mut *core::ptr::addr_of_mut!(OWNER)
    }
    #[inline(always)]
    pub unsafe fn invalidate_cache() {
        owner().invalidate_cache();
    }
    #[inline]
    pub fn prime_persistent_entries(first_id: u32, count: usize) {
        unsafe { owner().prime_persistent_entries(first_id, count) }
    }
    #[inline]
    pub fn load_chunk_decompressed(id: u32, dst: &mut [u32]) -> Option<super::ChunkLoad> {
        unsafe { owner().load_chunk_decompressed(id, dst) }
    }

    // ---- Interrupt-serviced chunk sessions (the weapon-switch viewmodel stream) ----
    //
    // The owner starts the read with the polled reader (prepare, Setloc, ReadN), hands each
    // sector to the CD interrupt (`psx_cdstream::drain`), and ends it with the reader's Pause.
    // A blocking load, a prime or a new session closes an open one first, inside the owner.
    #[inline]
    pub unsafe fn stream_begin(id: u32, dst: *mut u32, words: usize) -> bool {
        owner().stream_begin(id, dst, words)
    }
    #[inline]
    pub unsafe fn stream_pump() -> super::StreamPump {
        owner().stream_pump()
    }
    #[inline]
    pub fn stream_active() -> bool {
        unsafe { owner().stream_active() }
    }
    #[inline]
    pub fn stream_abort() {
        unsafe { owner().stream_abort() }
    }
}
#[cfg(target_arch = "mips")]
pub use target::*;

/// The resident scene core's CD calls (CORE-ABI.md section 3, P2-STREAM-CLIENT.md), for the
/// `scene-core` build. The slot symbols come from psx-core (feature `link`), which puts the core
/// in the window at boot.
#[cfg(all(target_arch = "mips", feature = "scene-core"))]
pub mod core {
    use psx_goldsrc::chunk_stream::{ChunkReader, DrainPoll, PACK_LBA};
    use psx_pack::cd::SECTOR_WORDS;
    use psx_pack::{entry_location, parse_entry_at, parse_header, PackEntry, ENTRY_BYTES, SECTOR_BYTES};

    /// `psx_core_stream_open` flag: today's implicit seek (Setloc, ReadN), as HL reads now.
    const IMPLICIT_SEEK: u32 = 1;

    extern "C" {
        fn psx_core_read(lba: u32, sectors: u32, dst: *mut u32, hook: Option<extern "C" fn(u32)>) -> u32;
        fn psx_core_pause() -> u32;
        fn psx_core_client_attach() -> u32;
        fn psx_core_client_detach();
        fn psx_core_stream_open(lba: u32, flags: u32) -> u32;
        fn psx_core_stream_sector(dst: *mut u32) -> u32;
        fn psx_core_set_idle_hook(hook: Option<extern "C" fn()>);
    }

    static mut ATTACHED: bool = false;

    /// Read whole sectors into `dst` through the core (blocking, ends paused).
    pub fn read(lba: u32, dst: &mut [u32; SECTOR_WORDS]) -> bool {
        // SAFETY: the core owns the drive; `dst` is one whole sector, borrowed for the call.
        unsafe { psx_core_read(lba, 1, dst.as_mut_ptr(), None) == 0 }
    }

    /// Words of the scene table copy: the header and five rows (three images, two scenes).
    const TABLE_WORDS: usize = 16 + 8 * 5;
    static mut TABLE: [u32; TABLE_WORDS] = [0; TABLE_WORDS];

    /// The title menu overlay's image in the scene table: hl-build's manifest lists PSX.EXE,
    /// HLGAME.EXE, then MENU.OVL.
    const MENU_IMAGE: u32 = 2;

    /// Read the pre-play title menu's code (MENU.OVL, menu.rs's `.text.overlay.1.*`) into the
    /// world arena's start before a call into it. A volatile overlay: the core reads it on every
    /// call, since a map overwrites that memory. The menu cannot run without it: a refusal or a
    /// failed read is the core's fatal screen with its code.
    pub fn load_menu() {
        let status = psx_core::link::load_image(MENU_IMAGE, scene_table());
        if status != 0 {
            psx_core::link::fatal(status);
        }
        // Link-time symbol: only its address is taken (it is MENU.OVL's size).
        let own = core::ptr::addr_of!(__overlay_1_bytes) as u32;
        // SAFETY: single-threaded; the read just ended, so the tail's words are this load's.
        unsafe { *core::ptr::addr_of_mut!(TAIL) = psx_core::link::image_tail(MENU_IMAGE, own, scene_table()) };
    }

    extern "C" {
        /// MENU.OVL's size in bytes (psoxide-overlay.ld), for `image_tail`.
        static __overlay_1_bytes: u32;
    }

    /// WORLD.PAK's first sectors as MENU.OVL's tail brought them in (hl-build's `tail=` on its
    /// line: the overlay sits right before the pack, so the same read carries on into the pack's
    /// header). The words lie in the world arena right after the menu's code, where the menu art
    /// and every map load later write. So the tail is good for one read only: `load_menu` sets it,
    /// and the first `start_read` after it takes it (when that read starts at the pack's first
    /// sector) or drops it (any other read), before anything can have written over those words.
    /// `None` on a disc that gives MENU.OVL no tail, and after that first read: then every read
    /// comes from the disc, as it always does.
    static mut TAIL: Option<&'static [u32]> = None;

    /// The read `start_read` served from [`TAIL`]: the tail's words not yet handed out and the
    /// sector they stand for. When they run out mid-read the read goes on from the disc at that
    /// sector. `None` while a disc read (or none) is open.
    static mut FROM_TAIL: Option<(&'static [u32], u32)> = None;

    /// The program's copy of the scene table, for switches and image loads: psx-core reads
    /// SCENES.TBL into it on the first one that needs it, so a boot that never switches or loads
    /// an image reads nothing.
    #[allow(dead_code)]
    pub fn scene_table() -> &'static mut [u32] {
        // SAFETY: single-threaded; the one borrow lives for one core call.
        unsafe { &mut *core::ptr::addr_of_mut!(TABLE) }
    }

    pub fn set_idle_hook(hook: Option<extern "C" fn()>) {
        // SAFETY: a plain store in the core.
        unsafe { psx_core_set_idle_hook(hook) }
    }

    /// goldsrc's transport on the core's stream-client calls.
    pub struct CoreReader;

    impl CoreReader {
        unsafe fn header_sector(
            &mut self,
            lba: u32,
            scratch: &mut [u32; SECTOR_WORDS],
            loaded: &mut u32,
            sector: u32,
        ) -> bool {
            if *loaded == sector {
                return true;
            }
            let ok = self.start_read(lba + sector) && self.read_sector(scratch);
            self.stop();
            if ok {
                *loaded = sector;
            }
            ok
        }
    }

    fn bytes(scratch: &[u32; SECTOR_WORDS]) -> &[u8] {
        // SAFETY: the words viewed as their own bytes.
        unsafe { core::slice::from_raw_parts(scratch.as_ptr().cast::<u8>(), SECTOR_BYTES) }
    }

    impl ChunkReader for CoreReader {
        unsafe fn prepare(&mut self) -> bool {
            // `psx_core_stream_open` prepares the drive itself.
            true
        }
        unsafe fn start_read(&mut self, lba: u32) -> bool {
            let tail = (*core::ptr::addr_of_mut!(TAIL)).take();
            *core::ptr::addr_of_mut!(FROM_TAIL) = None;
            match tail {
                Some(words) if lba == PACK_LBA && words.len() >= SECTOR_WORDS => {
                    *core::ptr::addr_of_mut!(FROM_TAIL) = Some((words, lba));
                    true
                }
                _ => psx_core_stream_open(lba, IMPLICIT_SEEK) == 0,
            }
        }
        unsafe fn read_sector(&mut self, dst: &mut [u32; SECTOR_WORDS]) -> bool {
            if let Some((words, lba)) = *core::ptr::addr_of!(FROM_TAIL) {
                if words.len() >= SECTOR_WORDS {
                    dst.copy_from_slice(&words[..SECTOR_WORDS]);
                    *core::ptr::addr_of_mut!(FROM_TAIL) = Some((&words[SECTOR_WORDS..], lba + 1));
                    return true;
                }
                // The tail is spent: the read goes on from the disc where the tail stopped.
                *core::ptr::addr_of_mut!(FROM_TAIL) = None;
                if psx_core_stream_open(lba, IMPLICIT_SEEK) != 0 {
                    return false;
                }
            }
            psx_core_stream_sector(dst.as_mut_ptr()) == 0
        }
        unsafe fn stop(&mut self) {
            // A read served whole from the tail started nothing on the drive: nothing to pause.
            if (*core::ptr::addr_of_mut!(FROM_TAIL)).take().is_none() {
                let _ = psx_core_pause();
            }
            if ATTACHED {
                ATTACHED = false;
                psx_core_client_detach();
            }
        }
        unsafe fn find_entry(
            &mut self,
            pack_lba: u32,
            id: u32,
            scratch: &mut [u32; SECTOR_WORDS],
        ) -> Option<PackEntry> {
            let mut loaded = u32::MAX;
            if !self.header_sector(pack_lba, scratch, &mut loaded, 0) {
                return None;
            }
            let header = parse_header(bytes(scratch))?;
            let mut index = 0;
            while index < header.chunk_count {
                let (sector, within) = entry_location(index);
                if sector >= header.header_sectors
                    || !self.header_sector(pack_lba, scratch, &mut loaded, sector)
                {
                    return None;
                }
                let entry = if within + ENTRY_BYTES <= SECTOR_BYTES {
                    parse_entry_at(bytes(scratch), within)?
                } else {
                    let first = SECTOR_BYTES - within;
                    let mut stitched = [0u8; ENTRY_BYTES];
                    stitched[..first].copy_from_slice(&bytes(scratch)[within..]);
                    if !self.header_sector(pack_lba, scratch, &mut loaded, sector + 1) {
                        return None;
                    }
                    stitched[first..].copy_from_slice(&bytes(scratch)[..ENTRY_BYTES - first]);
                    parse_entry_at(&stitched, 0)?
                };
                if entry.chunk_id == id {
                    return Some(entry);
                }
                index += 1;
            }
            None
        }
        unsafe fn drain_start(&mut self, dst: *mut u32, sectors: u32, byte_len: u32, timeout_vblanks: u32) -> bool {
            if psx_core_client_attach() != 0 {
                return false;
            }
            ATTACHED = true;
            // SAFETY: the trait's contract is the drain's.
            psx_cdstream::drain::start(dst, sectors, byte_len, timeout_vblanks)
        }
        fn drain_poll(&mut self) -> DrainPoll {
            use psx_cdstream::drain::{poll, DrainState};
            match poll() {
                DrainState::Running { .. } => DrainPoll::Running,
                DrainState::Done => DrainPoll::Done,
                _ => DrainPoll::Failed,
            }
        }
        fn drain_cancel(&mut self) {
            psx_cdstream::drain::cancel();
        }
    }
}
#[cfg(all(target_arch = "mips", feature = "scene-core"))]
pub use self::core::set_idle_hook;

// Host logic tests have no disc transport; retain the original inert boundary.
#[cfg(not(target_arch = "mips"))]
mod host {
    pub unsafe fn invalidate_cache() {}
    pub fn prime_persistent_entries(_: u32, _: usize) {}
    pub fn load_chunk(_: u32, _: &mut [u32]) -> Option<usize> {
        None
    }
    pub fn load_chunk_decompressed(_: u32, _: &mut [u32]) -> Option<super::ChunkLoad> {
        None
    }
    pub unsafe fn stream_begin(_: u32, _: *mut u32, _: usize) -> bool {
        false
    }
    #[optimize(size)]
    #[inline(never)]
    pub unsafe fn stream_pump() -> super::StreamPump {
        super::StreamPump::Idle
    }
    pub fn stream_active() -> bool {
        false
    }
    #[optimize(size)]
    #[inline(never)]
    pub fn stream_abort() {}
}
#[cfg(not(target_arch = "mips"))]
pub use host::*;
