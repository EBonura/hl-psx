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
    use psx_goldsrc::chunk_stream::{ChunkReader, DrainPoll};
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

    /// Words of the scene table copy: the header and four rows (two images, two scenes).
    #[cfg(not(scene_overlay_test))]
    const TABLE_WORDS: usize = 16 + 8 * 4;
    /// The overlay test's table: two scenes and four images.
    #[cfg(scene_overlay_test)]
    const TABLE_WORDS: usize = 16 + 8 * 6;
    static mut TABLE: [u32; TABLE_WORDS] = [0; TABLE_WORDS];

    /// The overlay test (cfg `scene_overlay_test`, set by HLPSX_SCENE_OVERLAY_TEST at build time;
    /// never shipped): at boot, ask the core for image 2 (an Overlay owned by
    /// HLGAME.EXE) and image 3 (one owned by PSX.EXE, the intro), and keep what it answered, with
    /// the handoff block's loaded mask before and after:
    /// `[status 2, status 3, mask before, mask after]`.
    #[cfg(scene_overlay_test)]
    #[no_mangle]
    pub static mut OVERLAY_TEST: [u32; 4] = [0xffff_ffff; 4];

    #[cfg(scene_overlay_test)]
    pub fn overlay_test() {
        let table = scene_table();
        let before = psx_core::link::block_word(0x74);
        let owned_by_game = psx_core::link::load_image(2, table);
        let owned_by_intro = psx_core::link::load_image(3, table);
        let after = psx_core::link::block_word(0x74);
        // SAFETY: once, at boot, single-threaded.
        unsafe { OVERLAY_TEST = [owned_by_game, owned_by_intro, before, after] };
    }

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
            psx_core_stream_open(lba, IMPLICIT_SEEK) == 0
        }
        unsafe fn read_sector(&mut self, dst: &mut [u32; SECTOR_WORDS]) -> bool {
            psx_core_stream_sector(dst.as_mut_ptr()) == 0
        }
        unsafe fn stop(&mut self) {
            let _ = psx_core_pause();
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
