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
        wr_index(0);
        while psx_io::read8(CD_STATUS) & STATUS_RESPONSE_FIFO_NOT_EMPTY != 0 {
            let _ = psx_io::read8(CD_RESPONSE);
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

/// Stream chunk `chunk_id` from WORLD.PAK into `dst`. Returns the chunk's byte
/// size on success, or `None` (no disc / not found / too big / read error).
#[cfg(target_arch = "mips")]
pub fn load_chunk(chunk_id: u32, dst: &mut [u32]) -> Option<usize> {
    unsafe {
        let buf = core::ptr::addr_of_mut!(SECTOR_BUF) as *mut u32;
        // Header/table: find the entry, including packs whose table spans more
        // than one sector. Future full-game asset packs can easily exceed the
        // old one-sector table limit.
        let mut loaded_header_sector = u32::MAX;
        if !load_pack_header_sector(buf, &mut loaded_header_sector, 0) {
            return None;
        }
        let p = buf as *const u8;
        if rd32(p, 0) != u32::from_le_bytes(*b"PSOX") || rd32(p, 4) != u32::from_le_bytes(*b"WPAK")
        {
            return None;
        }
        let chunk_count = rd32(p, 12);
        let header_sectors = rd32(p, 20).max(1);
        let mut entry: Option<(u32, u32, usize)> = None; // (sector_offset, sector_count, byte_size)
        let mut i = 0u32;
        while i < chunk_count {
            let table_offset = 28 + (i as usize) * 24;
            let sector = (table_offset / SECTOR_BYTES) as u32;
            if sector >= header_sectors {
                break;
            }
            let within = table_offset % SECTOR_BYTES;
            if !load_pack_header_sector(buf, &mut loaded_header_sector, sector) {
                return None;
            }
            let p = buf as *const u8;
            let (entry_id, sector_offset, sector_count, byte_size) = if within + 24 <= SECTOR_BYTES
            {
                (
                    rd32(p, within),
                    rd32(p, within + 4),
                    rd32(p, within + 8),
                    rd32(p, within + 12) as usize,
                )
            } else {
                let first = SECTOR_BYTES - within;
                if sector + 1 >= header_sectors {
                    break;
                }
                let mut entry_bytes = [0u8; 24];
                core::ptr::copy_nonoverlapping(p.add(within), entry_bytes.as_mut_ptr(), first);
                if !load_pack_header_sector(buf, &mut loaded_header_sector, sector + 1) {
                    return None;
                }
                let p = buf as *const u8;
                core::ptr::copy_nonoverlapping(p, entry_bytes.as_mut_ptr().add(first), 24 - first);
                let e = entry_bytes.as_ptr();
                (rd32(e, 0), rd32(e, 4), rd32(e, 8), rd32(e, 12) as usize)
            };
            if entry_id == chunk_id {
                entry = Some((sector_offset, sector_count, byte_size));
                break;
            }
            i += 1;
        }
        let (sector_offset, sector_count, byte_size) = entry?;
        if byte_size > dst.len() * 4 {
            return None;
        }
        // Payload: read sector_count sectors into dst.
        if !hw::prepare() || !hw::start_read(PACK_LBA + sector_offset) {
            hw::stop();
            return None;
        }
        let dst_ptr = dst.as_mut_ptr() as *mut u8;
        let mut s = 0u32;
        while s < sector_count {
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
