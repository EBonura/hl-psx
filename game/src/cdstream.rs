//! Stream a cooked map (`.hlm`) from the disc's WORLD.PAK (fixed LBA 1024) into
//! a RAM buffer. The CD-ROM DMA sector-read primitives are ported from PSoXide's
//! editor-playtest `cd_stream/hw.rs`; on top sits a thin parser for the pack
//! header + entry table.
//!
//! Pack layout (little-endian): 28-byte header
//!   [0..8] "PSOXWPAK" | u32 version=1 | u32 chunk_count | u32 total_sectors
//!   | u32 header_sectors | u32 table_bytes
//! then `chunk_count` × 24-byte entries
//!   u32 chunk_id | u32 sector_offset | u32 sector_count | u32 byte_size
//!   | u32 checksum (FNV-1a) | u32 reserved
//! then sector-aligned chunk payloads. hl-psx stores menu room N as two chunks:
//! resident world data in room_<2N>.psxc and temporary texture data in
//! room_<2N+1>.psxc.

#![allow(dead_code)]

pub const PACK_LBA: u32 = 1024;
const SECTOR_BYTES: usize = 2048;
const SECTOR_WORDS: usize = SECTOR_BYTES / 4;

static mut SECTOR_BUF: [u32; SECTOR_WORDS] = [0; SECTOR_WORDS];

/// Parsed WORLD.PAK table, cached on the first `load_chunk` so later loads skip
/// re-reading + re-scanning the (4-sector) header. Loading a map streams ~30
/// chunks (world, textures, 14 viewmodels, enemies); without the cache each one
/// seeks back to the pack LBA and rescans the whole table -- the dominant cost
/// of a map load. 512 covers the current 274-chunk pack with margin; a larger
/// pack falls back to the on-disk scan (PACK_CACHE_LEN = -2).
#[cfg(target_arch = "mips")]
const PACK_CACHE_MAX: usize = 512;

#[cfg(target_arch = "mips")]
#[derive(Clone, Copy)]
struct PackEntry {
    id: u32,
    sector_offset: u32,
    byte_size: u32,
}

#[cfg(target_arch = "mips")]
static mut PACK_CACHE: [PackEntry; PACK_CACHE_MAX] = [PackEntry {
    id: u32::MAX,
    sector_offset: 0,
    byte_size: 0,
}; PACK_CACHE_MAX];
// -1 = not built yet, -2 = pack too big for the cache (use the disk scan),
// >= 0 = number of cached entries.
#[cfg(target_arch = "mips")]
static mut PACK_CACHE_LEN: i32 = -1;

#[cfg(target_arch = "mips")]
mod hw {
    use super::{SECTOR_BUF, SECTOR_WORDS};

    const CD_BASE: u32 = 0x1F80_1800;
    const CD_STATUS: u32 = CD_BASE;
    const CD_RESPONSE: u32 = CD_BASE + 1;
    const CD_PARAM: u32 = CD_BASE + 2;
    const CD_IRQ: u32 = CD_BASE + 3;

    const STATUS_RESPONSE_FIFO_NOT_EMPTY: u8 = 1 << 5;
    const STATUS_PARAMETER_FIFO_NOT_FULL: u8 = 1 << 4;
    const STATUS_DATA_FIFO_NOT_EMPTY: u8 = 1 << 6;

    const IRQ_DATA_READY: u8 = 1;
    const IRQ_COMPLETE: u8 = 2;
    const IRQ_ACK: u8 = 3;
    const IRQ_DATA_END: u8 = 4;
    const IRQ_ERROR: u8 = 5;

    const CMD_SETLOC: u8 = 0x02;
    const CMD_READN: u8 = 0x06;
    const CMD_PAUSE: u8 = 0x09;
    const CMD_SETMODE: u8 = 0x0E;
    const CD_MODE_DOUBLE_SPEED_2048: u8 = 0x80;

    const ACK_POLL: u32 = 16_384;
    const PARAM_POLL: u32 = 16_384;
    const DATA_POLL: u32 = 4_000_000;
    const DMA_POLL: u32 = 65_536;
    const CLEANUP_POLL: u32 = 16_384;

    static mut PREPARED: bool = false;

    enum Wait {
        Matched,
        CdError,
        Timeout,
    }

    #[inline]
    unsafe fn wr_index(i: u8) {
        psx_io::write8(CD_STATUS, i & 0x03);
    }
    unsafe fn irq_flag() -> u8 {
        wr_index(1);
        let f = psx_io::read8(CD_IRQ) & 0x1F;
        wr_index(0);
        f
    }
    unsafe fn ack(irq: u8) {
        wr_index(1);
        psx_io::write8(CD_IRQ, irq & 0x1F);
        psx_io::irq::ack(1 << psx_io::irq::source::CDROM);
        wr_index(0);
    }
    unsafe fn ack_all() {
        wr_index(1);
        psx_io::write8(CD_IRQ, 0x5F);
        psx_io::irq::ack(1 << psx_io::irq::source::CDROM);
        wr_index(0);
    }
    unsafe fn enable_irqs() {
        wr_index(1);
        psx_io::write8(CD_PARAM, 0x1F);
        wr_index(0);
    }
    unsafe fn irq_enable() -> u8 {
        wr_index(0);
        let e = psx_io::read8(CD_IRQ) & 0x1F;
        wr_index(0);
        e
    }
    unsafe fn set_irq_enable(mask: u8) {
        wr_index(1);
        psx_io::write8(CD_PARAM, mask & 0x1F);
        wr_index(0);
    }
    unsafe fn drain_responses() {
        // The response FIFO is 16 bytes deep, so a real drain reads at most 16.
        // Bound the loop: on heavy streaming the CD/emulator can wedge the FIFO
        // "not-empty", and an unbounded drain spins forever (hung loader).
        wr_index(0);
        let mut guard = 0;
        while psx_io::read8(CD_STATUS) & STATUS_RESPONSE_FIFO_NOT_EMPTY != 0 && guard < 256 {
            let _ = psx_io::read8(CD_RESPONSE);
            guard += 1;
        }
    }
    unsafe fn data_fifo_ready() -> bool {
        wr_index(0);
        psx_io::read8(CD_STATUS) & STATUS_DATA_FIFO_NOT_EMPTY != 0
    }

    unsafe fn wait_param_room() -> bool {
        let mut i = 0;
        while i < PARAM_POLL {
            if psx_io::read8(CD_STATUS) & STATUS_PARAMETER_FIFO_NOT_FULL != 0 {
                return true;
            }
            i += 1;
        }
        false
    }

    unsafe fn dma_read_sector(buffer: *mut u32) {
        // Arm the data transfer (BFRD).
        wr_index(0);
        psx_io::write8(CD_IRQ, 0x80);
        wr_index(0);
        psx_io::dma::set_madr(psx_io::dma::Channel::Cdrom, buffer as u32);
        psx_io::dma::set_bcr_manual(psx_io::dma::Channel::Cdrom, SECTOR_WORDS as u16);
        psx_io::dma::set_chcr(psx_io::dma::Channel::Cdrom, 0x1140_0100);
        let mut i = 0;
        while psx_io::dma::is_busy(psx_io::dma::Channel::Cdrom) && i < DMA_POLL {
            i += 1;
        }
        psx_io::irq::ack(1 << psx_io::irq::source::DMA);
    }

    unsafe fn ack_unexpected(flag: u8) {
        match flag {
            IRQ_DATA_READY => {
                dma_read_sector(core::ptr::addr_of_mut!(SECTOR_BUF) as *mut u32);
                drain_responses();
                ack(IRQ_DATA_READY);
            }
            IRQ_COMPLETE | IRQ_ACK | IRQ_DATA_END => {
                drain_responses();
                ack(flag);
            }
            _ => {
                drain_responses();
                ack_all();
            }
        }
    }

    unsafe fn wait_irq(expected: u8, limit: u32) -> Wait {
        let mut i = 0;
        while i < limit {
            let flag = irq_flag();
            if flag == expected {
                return Wait::Matched;
            }
            if expected == IRQ_DATA_READY && data_fifo_ready() {
                return Wait::Matched;
            }
            if flag == IRQ_ERROR {
                return Wait::CdError;
            }
            if flag != 0 {
                ack_unexpected(flag);
            }
            i += 1;
        }
        Wait::Timeout
    }

    unsafe fn send_command(command: u8, params: &[u8], expected: u8, limit: u32) -> bool {
        let saved = irq_enable();
        set_irq_enable(0);
        ack_all();
        wr_index(0);
        drain_responses();
        wr_index(1);
        psx_io::write8(CD_IRQ, 0x40);
        wr_index(0);
        for &p in params {
            if !wait_param_room() {
                set_irq_enable(saved);
                wr_index(0);
                return false;
            }
            psx_io::write8(CD_PARAM, p);
        }
        psx_io::write8(CD_RESPONSE, command);
        let ok = match wait_irq(expected, limit) {
            Wait::Matched => {
                drain_responses();
                ack(expected);
                true
            }
            Wait::CdError => {
                drain_responses();
                ack_all();
                false
            }
            Wait::Timeout => false,
        };
        set_irq_enable(saved);
        wr_index(0);
        ok
    }

    const fn bin_to_bcd(v: u8) -> u8 {
        ((v / 10) << 4) | (v % 10)
    }
    fn lba_to_bcd_msf(lba: u32) -> (u8, u8, u8) {
        let abs = lba.saturating_add(150);
        (
            bin_to_bcd((abs / (60 * 75)) as u8),
            bin_to_bcd(((abs / 75) % 60) as u8),
            bin_to_bcd((abs % 75) as u8),
        )
    }

    pub unsafe fn prepare() -> bool {
        psx_io::irq::set_mask(1 << psx_io::irq::source::VBLANK);
        psx_io::irq::ack(1 << psx_io::irq::source::CDROM);
        enable_irqs();
        ack_all();
        psx_io::dma::enable_channel(psx_io::dma::Channel::Cdrom);
        if !PREPARED {
            let mut i = 0;
            while i < 16 {
                let f = irq_flag();
                if f == 0 {
                    break;
                }
                ack_unexpected(f);
                i += 1;
            }
            ack_all();
            PREPARED = true;
        }
        send_command(CMD_SETMODE, &[CD_MODE_DOUBLE_SPEED_2048], IRQ_ACK, ACK_POLL)
    }

    pub unsafe fn start_read(lba: u32) -> bool {
        let (m, s, f) = lba_to_bcd_msf(lba);
        if !send_command(CMD_SETLOC, &[m, s, f], IRQ_ACK, ACK_POLL) {
            return false;
        }
        if !send_command(CMD_READN, &[], IRQ_ACK, ACK_POLL) {
            return false;
        }
        enable_irqs();
        true
    }

    pub unsafe fn read_sector(buffer: *mut u32) -> bool {
        match wait_irq(IRQ_DATA_READY, DATA_POLL) {
            Wait::Matched => {}
            _ => {
                drain_responses();
                ack_all();
                return false;
            }
        }
        dma_read_sector(buffer);
        drain_responses();
        ack(IRQ_DATA_READY);
        true
    }

    pub unsafe fn stop() {
        if send_command(CMD_PAUSE, &[], IRQ_ACK, CLEANUP_POLL) {
            let _ = wait_irq(IRQ_COMPLETE, CLEANUP_POLL);
            drain_responses();
            ack(IRQ_COMPLETE);
        }
        ack_all();
    }
}

#[inline]
fn rd32(p: *const u8, o: usize) -> u32 {
    unsafe {
        (core::ptr::read_volatile(p.add(o)) as u32)
            | ((core::ptr::read_volatile(p.add(o + 1)) as u32) << 8)
            | ((core::ptr::read_volatile(p.add(o + 2)) as u32) << 16)
            | ((core::ptr::read_volatile(p.add(o + 3)) as u32) << 24)
    }
}

/// Read table entry `i` as (id, sector_offset, byte_size), handling an entry
/// that straddles two header sectors. `loaded` tracks the currently-buffered
/// header sector so sequential reads don't reload it.
#[cfg(target_arch = "mips")]
unsafe fn read_pack_entry(
    buf: *mut u32,
    loaded: &mut u32,
    header_sectors: u32,
    i: u32,
) -> Option<(u32, u32, u32)> {
    let table_offset = 28 + (i as usize) * 24;
    let sector = (table_offset / SECTOR_BYTES) as u32;
    if sector >= header_sectors {
        return None;
    }
    let within = table_offset % SECTOR_BYTES;
    if !load_pack_header_sector(buf, loaded, sector) {
        return None;
    }
    let p = buf as *const u8;
    if within + 24 <= SECTOR_BYTES {
        Some((rd32(p, within), rd32(p, within + 4), rd32(p, within + 12)))
    } else {
        // Entry spans this sector and the next; stitch the 24 bytes together.
        let first = SECTOR_BYTES - within;
        if sector + 1 >= header_sectors {
            return None;
        }
        let mut e = [0u8; 24];
        core::ptr::copy_nonoverlapping(p.add(within), e.as_mut_ptr(), first);
        if !load_pack_header_sector(buf, loaded, sector + 1) {
            return None;
        }
        let p = buf as *const u8;
        core::ptr::copy_nonoverlapping(p, e.as_mut_ptr().add(first), 24 - first);
        let e = e.as_ptr();
        Some((rd32(e, 0), rd32(e, 4), rd32(e, 12)))
    }
}

/// Read the pack header once and cache every entry, or mark the cache disabled
/// (`-2`) if the pack has more chunks than the cache holds.
#[cfg(target_arch = "mips")]
unsafe fn build_pack_cache() {
    let buf = core::ptr::addr_of_mut!(SECTOR_BUF) as *mut u32;
    let mut loaded = u32::MAX;
    if !load_pack_header_sector(buf, &mut loaded, 0) {
        return; // leave state -1 so a later call retries
    }
    let p = buf as *const u8;
    if rd32(p, 0) != u32::from_le_bytes(*b"PSOX") || rd32(p, 4) != u32::from_le_bytes(*b"WPAK") {
        return;
    }
    let chunk_count = rd32(p, 12);
    let header_sectors = rd32(p, 20).max(1);
    if chunk_count as usize > PACK_CACHE_MAX {
        PACK_CACHE_LEN = -2;
        return;
    }
    let mut n = 0usize;
    let mut i = 0u32;
    while i < chunk_count {
        let Some((id, so, bs)) = read_pack_entry(buf, &mut loaded, header_sectors, i) else {
            break;
        };
        PACK_CACHE[n] = PackEntry {
            id,
            sector_offset: so,
            byte_size: bs,
        };
        n += 1;
        i += 1;
    }
    PACK_CACHE_LEN = n as i32;
}

/// On-disk table scan (the pre-cache path), used only when the pack is too big
/// to cache. Returns (sector_offset, byte_size) for `chunk_id`.
#[cfg(target_arch = "mips")]
unsafe fn scan_pack_table(chunk_id: u32) -> Option<(u32, usize)> {
    let buf = core::ptr::addr_of_mut!(SECTOR_BUF) as *mut u32;
    let mut loaded = u32::MAX;
    if !load_pack_header_sector(buf, &mut loaded, 0) {
        return None;
    }
    let p = buf as *const u8;
    if rd32(p, 0) != u32::from_le_bytes(*b"PSOX") || rd32(p, 4) != u32::from_le_bytes(*b"WPAK") {
        return None;
    }
    let chunk_count = rd32(p, 12);
    let header_sectors = rd32(p, 20).max(1);
    let mut i = 0u32;
    while i < chunk_count {
        let Some((id, so, bs)) = read_pack_entry(buf, &mut loaded, header_sectors, i) else {
            break;
        };
        if id == chunk_id {
            return Some((so, bs as usize));
        }
        i += 1;
    }
    None
}

/// Resolve `chunk_id` to (sector_offset, byte_size), building the table cache on
/// first use so later lookups touch no disc.
#[cfg(target_arch = "mips")]
unsafe fn lookup_entry(chunk_id: u32) -> Option<(u32, usize)> {
    if PACK_CACHE_LEN == -1 {
        build_pack_cache();
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
    scan_pack_table(chunk_id)
}

/// Stream chunk `chunk_id` from WORLD.PAK into `dst`. Returns the chunk's byte
/// size on success, or `None` (no disc / not found / too big / read error).
#[cfg(target_arch = "mips")]
pub fn load_chunk(chunk_id: u32, dst: &mut [u32]) -> Option<usize> {
    unsafe {
        let buf = core::ptr::addr_of_mut!(SECTOR_BUF) as *mut u32;
        let (sector_offset, byte_size) = lookup_entry(chunk_id)?;
        if byte_size > dst.len() * 4 {
            return None;
        }
        // Payload: read the chunk's sectors into dst.
        if !hw::prepare() || !hw::start_read(PACK_LBA + sector_offset) {
            hw::stop();
            return None;
        }
        let dst_ptr = dst.as_mut_ptr() as *mut u8;
        // Read only as many sectors as `byte_size` needs; the table's padded
        // sector_count could be garbage and looping on it would hang the loader.
        let needed = ((byte_size as u32) + (SECTOR_BYTES as u32) - 1) / (SECTOR_BYTES as u32);
        let mut s = 0u32;
        while s < needed {
            if !hw::read_sector(buf) {
                hw::stop();
                return None;
            }
            let off = (s as usize) * SECTOR_BYTES;
            let copy = byte_size.saturating_sub(off).min(SECTOR_BYTES);
            if copy > 0 {
                core::ptr::copy_nonoverlapping(buf as *const u8, dst_ptr.add(off), copy);
            }
            s += 1;
        }
        hw::stop();
        Some(byte_size)
    }
}

#[cfg(target_arch = "mips")]
unsafe fn load_pack_header_sector(buf: *mut u32, loaded_sector: &mut u32, sector: u32) -> bool {
    if *loaded_sector == sector {
        return true;
    }
    if !hw::prepare() || !hw::start_read(PACK_LBA + sector) {
        hw::stop();
        return false;
    }
    let ok = hw::read_sector(buf);
    hw::stop();
    if ok {
        *loaded_sector = sector;
    }
    ok
}

#[cfg(not(target_arch = "mips"))]
pub fn load_chunk(_chunk_id: u32, _dst: &mut [u32]) -> Option<usize> {
    None
}

/// LZ4-wrapped chunk support ("HLZC" | u32 raw_len | LZ4 block).
///
/// `load_chunk` leaves the (compressed) payload at buf[0..loaded]. Move it to
/// the END of the buffer, then decode the LZ4 stream back to the head. Safe
/// in place: build.rs pads MAP_WORDS past the biggest raw map by more than
/// the worst-case LZ4 in-place margin (comp_len/255 + a few bytes), so the
/// write cursor can never overrun the unread source bytes.
///
/// Returns the decompressed length, or `loaded` unchanged for non-HLZC
/// chunks (raw passthrough -- models/SFX/old packs).
pub unsafe fn decompress_in_place(buf: &mut [u32], loaded: usize) -> usize {
    if loaded < 8 {
        return loaded;
    }
    let base = buf.as_mut_ptr() as *mut u8;
    let magic = u32::from_le_bytes([*base, *base.add(1), *base.add(2), *base.add(3)]);
    if magic != u32::from_le_bytes(*b"HLZC") {
        return loaded;
    }
    let raw_len = u32::from_le_bytes([
        *base.add(4),
        *base.add(5),
        *base.add(6),
        *base.add(7),
    ]) as usize;
    let cap = buf.len() * 4;
    let comp_len = loaded - 8;
    if raw_len > cap || comp_len > cap {
        return 0;
    }
    // Shift the payload to the buffer tail (overlapping regions: copy back
    // to front is safe because dst > src everywhere here).
    let src_tail = base.add(cap - comp_len);
    core::ptr::copy(base.add(8), src_tail, comp_len);
    lz4_block_decode(src_tail, src_tail.add(comp_len), base, raw_len)
}

/// Minimal LZ4 block decoder over raw pointers (src and dst may live in the
/// same buffer; the in-place margin guarantees dst never catches src).
unsafe fn lz4_block_decode(
    mut src: *const u8,
    src_end: *const u8,
    dst_base: *mut u8,
    dst_cap: usize,
) -> usize {
    let mut di = 0usize;
    loop {
        if src >= src_end {
            break;
        }
        let token = *src;
        src = src.add(1);
        // Literal run.
        let mut lit = (token >> 4) as usize;
        if lit == 15 {
            loop {
                let b = *src;
                src = src.add(1);
                lit += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        if lit > 0 {
            if di + lit > dst_cap {
                return 0;
            }
            core::ptr::copy(src, dst_base.add(di), lit);
            src = src.add(lit);
            di += lit;
        }
        if src >= src_end {
            break; // final literal run has no match part
        }
        // Match: little-endian offset + extendable length.
        let off = (*src as usize) | ((*src.add(1) as usize) << 8);
        src = src.add(2);
        if off == 0 || off > di {
            return 0;
        }
        let mut mlen = (token & 15) as usize;
        if mlen == 15 {
            loop {
                let b = *src;
                src = src.add(1);
                mlen += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        mlen += 4;
        if di + mlen > dst_cap {
            return 0;
        }
        // Byte-at-a-time forward copy: correct for overlapping matches
        // (off < mlen replicates the window, exactly LZ4 semantics).
        let mut mp = di - off;
        for _ in 0..mlen {
            *dst_base.add(di) = *dst_base.add(mp);
            di += 1;
            mp += 1;
        }
    }
    di
}
