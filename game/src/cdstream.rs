//! GoldSrc chunk streaming uses the shared owner; this facade supplies the
//! game's render-arena lease and persistent viewmodel-entry capacity.
pub use psx_goldsrc::chunk_stream::{decompress_in_place, ChunkLoad, StreamPump, PACK_LBA};

#[cfg(target_arch = "mips")]
mod target {
    use psx_goldsrc::chunk_stream::{CachedStreamer, PacketArena, CACHE_ENTRY_BYTES};
    struct Arena;
    // The game serializes loading and rendering on the main thread. Rendering
    // invalidates this overlay before reclaiming it; cache construction retires
    // retained packet metadata before its first write. The progress hook draws
    // the loading strip and never reenters the chunk streamer.
    unsafe impl PacketArena for Arena {
        const CAPACITY: usize = crate::room_budget::PACK_CACHE_ENTRIES;
        #[inline(always)]
        unsafe fn storage() -> *mut u8 {
            core::ptr::addr_of_mut!(crate::PRIMITIVE_PACKETS).cast()
        }
        #[inline(always)]
        unsafe fn invalidate_render_cache() {
            crate::invalidate_world_packet_cache();
        }
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
        unsafe { owner().prime_persistent_entries(first_id, count) }
    }
    #[inline]
    pub fn set_sector_hook(hook: Option<fn(usize, usize)>) {
        unsafe { owner().set_sector_hook(hook) }
    }
    #[inline]
    pub fn load_chunk(id: u32, dst: &mut [u32]) -> Option<usize> {
        unsafe { owner().load_chunk(id, dst) }
    }
    #[inline]
    pub fn load_chunk_decompressed(id: u32, dst: &mut [u32]) -> Option<super::ChunkLoad> {
        unsafe { owner().load_chunk_decompressed(id, dst) }
    }
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
    pub fn set_sector_hook(_: Option<fn(usize, usize)>) {}
    pub fn load_chunk(_: u32, _: &mut [u32]) -> Option<usize> {
        None
    }
    pub fn load_chunk_decompressed(_: u32, _: &mut [u32]) -> Option<super::ChunkLoad> {
        None
    }
    pub unsafe fn stream_begin(_: u32, _: *mut u32, _: usize) -> bool {
        false
    }
    pub unsafe fn stream_pump() -> super::StreamPump {
        super::StreamPump::Idle
    }
    pub fn stream_active() -> bool {
        false
    }
    pub fn stream_abort() {}
}
#[cfg(not(target_arch = "mips"))]
pub use host::*;
