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
    type Owner = CachedStreamer<psx_pack::cd::SectorReader, Arena, 32>;
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
