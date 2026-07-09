//! Stream a cooked map (`.hlm`) from the disc's WORLD.PAK (fixed LBA 1024)
//! into a RAM buffer. The CD-ROM DMA sector-read state machine, the pack
//! header/table parsing, and the HLZC/LZ4 decoder were extracted from this
//! file into the SDK's `psx_pack` crate (`psx_pack::cd` is the same
//! silicon-proven command sequence, repackaged from module statics into
//! `SectorReader`). What stays here is the hl-psx-specific pack-entry cache
//! plus thin wrappers preserving the game-facing API (`load_chunk`,
//! `decompress_in_place`).
//!
//! Pack layout: see `psx_pack`'s crate docs (28-byte "PSOXWPAK" header,
//! 24-byte table entries, sector-aligned payloads). hl-psx stores menu room N
//! as two chunks: resident world data in room_<2N>.psxc and temporary texture
//! data in room_<2N+1>.psxc.

#![allow(dead_code)]

#[cfg(target_arch = "mips")]
use psx_pack::cd::{SectorReader, SECTOR_WORDS};
#[cfg(target_arch = "mips")]
use psx_pack::SECTOR_BYTES;

/// WORLD.PAK's fixed start LBA (mkisopsx's default, pinned by the SDK).
pub const PACK_LBA: u32 = psx_pack::cd::WORLD_PACK_DEFAULT_LBA;

/// The one CD sector reader (it owns the "boot IRQs drained" flag and the
/// bounce buffer that unexpected DataReady sectors drain into) plus the
/// scratch sector every header scan and payload copy stages through.
/// Single-threaded polled MMIO, same contract as the statics they replace.
#[cfg(target_arch = "mips")]
static mut READER: SectorReader = SectorReader::new();
#[cfg(target_arch = "mips")]
static mut SECTOR_BUF: [u32; SECTOR_WORDS] = [0; SECTOR_WORDS];

/// Parsed WORLD.PAK table, cached on the first `load_chunk` so later loads skip
/// re-reading + re-scanning the (4-sector) header. Loading a map streams ~30
/// chunks (world, textures, 14 viewmodels, enemies); without the cache each one
/// seeks back to the pack LBA and rescans the whole table -- the dominant cost
/// of a map load. 512 covers the current 274-chunk pack with margin; a larger
/// pack falls back to the SDK's on-disc scan (PACK_CACHE_LEN = -2). The SDK
/// deliberately does no caching at its layer, so the cache stays game-side.
#[cfg(target_arch = "mips")]
const PACK_CACHE_MAX: usize = 512;

#[cfg(target_arch = "mips")]
#[derive(Clone, Copy)]
struct CachedEntry {
    id: u32,
    sector_offset: u32,
    byte_size: u32,
}

#[cfg(target_arch = "mips")]
static mut PACK_CACHE: [CachedEntry; PACK_CACHE_MAX] = [CachedEntry {
    id: u32::MAX,
    sector_offset: 0,
    byte_size: 0,
}; PACK_CACHE_MAX];
// -1 = not built yet, -2 = pack too big for the cache (use the disk scan),
// >= 0 = number of cached entries.
#[cfg(target_arch = "mips")]
static mut PACK_CACHE_LEN: i32 = -1;

/// The scratch sector as bytes (little-endian DMA words are exactly the
/// on-disc byte order).
#[cfg(target_arch = "mips")]
fn sector_bytes(scratch: &[u32; SECTOR_WORDS]) -> &[u8] {
    // SAFETY: a [u32; N] viewed as its own bytes; alignment only shrinks.
    unsafe { core::slice::from_raw_parts(scratch.as_ptr() as *const u8, SECTOR_BYTES) }
}

/// Read the pack header once and cache every entry, or mark the cache disabled
/// (`-2`) if the pack has more chunks than the cache holds. The header sectors
/// stream in a single ReadN pass (entries are laid out sequentially), parsed
/// with the SDK's host-tested helpers; the entry that straddles two sectors is
/// stitched through a 24-byte buffer, same as `psx_pack::cd`'s own scan.
#[cfg(target_arch = "mips")]
unsafe fn build_pack_cache(rd: &mut SectorReader, scratch: &mut [u32; SECTOR_WORDS]) {
    use psx_pack::{entry_location, parse_entry_at, parse_header, ENTRY_BYTES};
    if !rd.prepare() || !rd.start_read(PACK_LBA) || !rd.read_sector(scratch) {
        rd.stop();
        return; // leave state -1 so a later call retries
    }
    let Some(header) = parse_header(sector_bytes(scratch)) else {
        rd.stop();
        return;
    };
    if header.chunk_count as usize > PACK_CACHE_MAX {
        rd.stop();
        PACK_CACHE_LEN = -2;
        return;
    }
    let mut n = 0usize;
    let mut cur_sector = 0u32;
    'scan: while (n as u32) < header.chunk_count {
        let (sector, within) = entry_location(n as u32);
        if sector >= header.header_sectors {
            break;
        }
        while cur_sector < sector {
            if !rd.read_sector(scratch) {
                break 'scan;
            }
            cur_sector += 1;
        }
        let entry = if within + ENTRY_BYTES <= SECTOR_BYTES {
            parse_entry_at(sector_bytes(scratch), within)
        } else {
            // Entry spans this sector and the next; stitch the 24 bytes together.
            if sector + 1 >= header.header_sectors {
                break;
            }
            let first = SECTOR_BYTES - within;
            let mut stitched = [0u8; ENTRY_BYTES];
            stitched[..first].copy_from_slice(&sector_bytes(scratch)[within..]);
            if !rd.read_sector(scratch) {
                break;
            }
            cur_sector += 1;
            stitched[first..].copy_from_slice(&sector_bytes(scratch)[..ENTRY_BYTES - first]);
            parse_entry_at(&stitched, 0)
        };
        let Some(e) = entry else { break };
        PACK_CACHE[n] = CachedEntry {
            id: e.chunk_id,
            sector_offset: e.sector_offset,
            byte_size: e.byte_size,
        };
        n += 1;
    }
    rd.stop();
    PACK_CACHE_LEN = n as i32;
}

/// Resolve `chunk_id` to (sector_offset, byte_size), building the table cache
/// on first use so later lookups touch no disc. A pack too big for the cache
/// falls back to the SDK's sector-by-sector table scan.
#[cfg(target_arch = "mips")]
unsafe fn lookup_entry(
    rd: &mut SectorReader,
    scratch: &mut [u32; SECTOR_WORDS],
    chunk_id: u32,
) -> Option<(u32, usize)> {
    if PACK_CACHE_LEN == -1 {
        build_pack_cache(rd, scratch);
    }
    if PACK_CACHE_LEN >= 0 {
        let n = PACK_CACHE_LEN as usize;
        let mut k = 0;
        while k < n {
            let e = PACK_CACHE[k];
            if e.id == chunk_id {
                return Some((e.sector_offset, e.byte_size as usize));
            }
            k += 1;
        }
        return None;
    }
    psx_pack::cd::find_entry(rd, PACK_LBA, chunk_id, scratch)
        .map(|e| (e.sector_offset, e.byte_size as usize))
}

/// Stream chunk `chunk_id` from WORLD.PAK into `dst`. Returns the chunk's byte
/// size on success, or `None` (no disc / not found / too big / read error).
#[cfg(target_arch = "mips")]
pub fn load_chunk(chunk_id: u32, dst: &mut [u32]) -> Option<usize> {
    unsafe {
        let rd = &mut *core::ptr::addr_of_mut!(READER);
        let scratch = &mut *core::ptr::addr_of_mut!(SECTOR_BUF);
        let (sector_offset, byte_size) = lookup_entry(rd, scratch, chunk_id)?;
        if byte_size > dst.len() * 4 {
            return None;
        }
        // Payload: read the chunk's sectors into dst. This is the cached-entry
        // twin of psx_pack::cd::load_chunk's payload loop (the SDK version
        // rescans the table per call, which the cache exists to avoid).
        if !rd.prepare() || !rd.start_read(PACK_LBA + sector_offset) {
            rd.stop();
            return None;
        }
        let dst_ptr = dst.as_mut_ptr() as *mut u8;
        // Read only as many sectors as `byte_size` needs; the table's padded
        // sector_count could be garbage and looping on it would hang the loader.
        let needed = byte_size.div_ceil(SECTOR_BYTES);
        let mut s = 0usize;
        while s < needed {
            if !rd.read_sector(scratch) {
                rd.stop();
                return None;
            }
            let off = s * SECTOR_BYTES;
            let copy = byte_size.saturating_sub(off).min(SECTOR_BYTES);
            if copy > 0 {
                core::ptr::copy_nonoverlapping(
                    scratch.as_ptr() as *const u8,
                    dst_ptr.add(off),
                    copy,
                );
            }
            s += 1;
        }
        rd.stop();
        Some(byte_size)
    }
}

#[cfg(not(target_arch = "mips"))]
pub fn load_chunk(_chunk_id: u32, _dst: &mut [u32]) -> Option<usize> {
    None
}

/// LZ4-wrapped chunk support ("HLZC" | u32 raw_len | LZ4 block), decoded in
/// place by the SDK's `psx_pack::decompress_hlzc_in_place` (extracted from
/// this file's original decoder).
///
/// `load_chunk` leaves the (compressed) payload at buf[0..loaded]; the SDK
/// stages it at the END of the buffer and decodes the LZ4 stream back to the
/// head. Safe in place: build.rs pads MAP_WORDS past the biggest raw map by
/// more than the worst-case LZ4 in-place margin (comp_len/255 + a few bytes),
/// and the SDK decoder re-checks that margin per step, failing cleanly
/// instead of corrupting.
///
/// Returns the decompressed length, `loaded` unchanged for non-HLZC chunks
/// (raw passthrough -- models/SFX/old packs), or 0 on a corrupt stream.
pub unsafe fn decompress_in_place(buf: &mut [u32], loaded: usize) -> usize {
    let bytes = core::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 4);
    psx_pack::decompress_hlzc_in_place(bytes, loaded).unwrap_or(0)
}
