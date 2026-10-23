//! GoldSrc chunk streaming uses the shared owner; this facade supplies the
//! game's render-arena lease and persistent viewmodel-entry capacity.
pub use psx_goldsrc::chunk_stream::{ChunkLoad, StreamPump, PACK_LBA};

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
    type Owner = CachedStreamer<psx_pack::cd::SectorReader, Arena, 16>;
    static mut OWNER: Owner = unsafe { Owner::new(psx_pack::cd::SectorReader::new()) };
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
        stream_abort();
        unsafe { owner().prime_persistent_entries(first_id, count) }
    }
    #[inline]
    pub fn load_chunk_decompressed(id: u32, dst: &mut [u32]) -> Option<super::ChunkLoad> {
        // A blocking read takes the drive: close any interrupt-driven session first.
        stream_abort();
        unsafe { owner().load_chunk_decompressed(id, dst) }
    }

    // ---- Interrupt-serviced chunk sessions (the weapon-switch viewmodel stream) ----
    //
    // The controller delivers a sector every 6.7 ms at double speed and keeps only a
    // few, handing over the newest when software is late. A pump that looks once per
    // tick (50 ms or more) therefore lost most of a chunk, and the game threw the
    // holes away and read the chunk again with everything stopped. A session starts the
    // read exactly as before (the owner's `stream_begin`: prepare, Setloc, ReadN) and
    // then lets the CD interrupt pop each sector into the destination
    // (`psx_cdstream::drain`); the tick only asks how far it got, and ends the read with
    // the owner's `stream_abort` (the polled reader's Pause). While a session is open
    // nothing else touches the drive: every polled entry point below closes it first.
    use core::ptr::addr_of_mut;
    use psx_cdstream::drain::{self, DrainState};

    /// The open session, if any.
    struct Session {
        active: bool,
        dst: *mut u32,
        dst_words: usize,
        byte_size: usize,
    }
    static mut SESSION: Session = Session {
        active: false,
        dst: core::ptr::null_mut(),
        dst_words: 0,
        byte_size: 0,
    };
    /// VBlanks without a sector before a session is given up (the drive's worst
    /// measured start after audio is about a second).
    const SESSION_TIMEOUT_VBLANKS: u32 = 300;

    #[optimize(size)]
    #[inline(never)]
    pub unsafe fn stream_begin(id: u32, dst: *mut u32, words: usize) -> bool {
        stream_abort();
        let Some((_, byte_size)) = owner().resolve_entry(id) else {
            return false;
        };
        if byte_size == 0 || byte_size > words * 4 || !owner().stream_begin(id, dst, words) {
            return false;
        }
        let sectors = byte_size.div_ceil(psx_pack::SECTOR_BYTES) as u32;
        if !drain::start(dst, sectors, byte_size as u32, SESSION_TIMEOUT_VBLANKS) {
            owner().stream_abort();
            return false;
        }
        *addr_of_mut!(SESSION) = Session {
            active: true,
            dst,
            dst_words: words,
            byte_size,
        };
        true
    }

    #[optimize(size)]
    #[inline(never)]
    pub unsafe fn stream_pump() -> super::StreamPump {
        let session = &mut *addr_of_mut!(SESSION);
        if !session.active {
            return super::StreamPump::Idle;
        }
        match drain::poll() {
            DrainState::Running { .. } => super::StreamPump::InFlight,
            DrainState::Done => {
                stream_abort();
                let buf = core::slice::from_raw_parts_mut(session.dst, session.dst_words);
                let raw_len =
                    psx_goldsrc::chunk_stream::decompress_in_place(buf, session.byte_size);
                if raw_len == 0 {
                    super::StreamPump::Failed
                } else {
                    super::StreamPump::Done(super::ChunkLoad {
                        stored_len: session.byte_size,
                        raw_len,
                    })
                }
            }
            // Failed, or forgotten: the caller falls back to a blocking read.
            _ => {
                stream_abort();
                super::StreamPump::Failed
            }
        }
    }

    /// Whether a session owns the drive.
    #[inline]
    pub fn stream_active() -> bool {
        unsafe { (*addr_of_mut!(SESSION)).active }
    }

    /// Close the open session and stop the drive, so a polled command can follow.
    #[optimize(size)]
    #[inline(never)]
    pub fn stream_abort() {
        unsafe {
            if (*addr_of_mut!(SESSION)).active {
                (*addr_of_mut!(SESSION)).active = false;
                drain::cancel();
            }
            owner().stream_abort();
        }
    }
}
#[cfg(target_arch = "mips")]
pub use target::*;

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
