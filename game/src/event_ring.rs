//! Diagnostic ring of the most recent fired targets and changelevels.
//!
//! Compiled only when `HLPSX_EVENT_RING` is set for the build (a `hl_event_ring` cfg from build.rs). Read it from a RAM dump: the
//! symbol `HL_EVENT_RING` starts with the bytes `EVRING01`, a little-endian
//! u32 event count at offset 8, then `SLOTS` records of 16 bytes: map tick
//! (u16), use type (u8, 0xff for a changelevel), depth (u8) and the first 12
//! bytes of the target name (a changelevel stores the next map name).

pub const SLOTS: usize = 384;

#[no_mangle]
pub static mut HL_EVENT_RING: [u8; 16 + SLOTS * 16] = [0; 16 + SLOTS * 16];

/// Record one event. Single-threaded game loop, so no locking.
pub unsafe fn push(tick: u16, name: &str, use_type: u8, depth: u8) {
    let ring = &mut *core::ptr::addr_of_mut!(HL_EVENT_RING);
    if ring[0] == 0 {
        ring[..8].copy_from_slice(b"EVRING01");
    }
    let count = u32::from_le_bytes([ring[8], ring[9], ring[10], ring[11]]);
    let at = 16 + (count as usize % SLOTS) * 16;
    ring[at..at + 2].copy_from_slice(&tick.to_le_bytes());
    ring[at + 2] = use_type;
    ring[at + 3] = depth;
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < 12 {
        ring[at + 4 + i] = if i < bytes.len() { bytes[i] } else { 0 };
        i += 1;
    }
    ring[8..12].copy_from_slice(&count.wrapping_add(1).to_le_bytes());
}
