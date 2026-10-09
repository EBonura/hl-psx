//! Ordering trace ring (cargo feature `order-trace`, off by default).
//!
//! Records, for every world GT3/GT4 packet the renderer inserts into the
//! ordering table, the table key it received, the screen-space vertex words
//! (so the entry joins exactly with a GP0 draw dump), the depths the key was
//! derived from, and which cooked face produced it. The ring lives in plain
//! RAM under a fixed symbol (`ORD_BUF` / `ORD_N`), so a poll-bound
//! `--dump-ram` capture plus the linker map is all the host needs; the
//! `hlt wokv` command joins the two. Nothing here is compiled without the
//! feature, so shipping images are unchanged.
//!
//! Entry layout (`WORDS` u32 each, newest at `(ORD_N - 1) % CAP`):
//!   0  key | n << 16 | site << 20 | kind << 28
//!   1  face id (kind 1 loop face, 2 raw-tri face: face index; 3 patch: first
//!      triangle index; 4 submodel face: face index; 0 unknown); the face's bounding
//!      radius (cooked face sphere, 0 for patches) sits in the upper 16 bits
//!   2..6  the packet's v0..v3 vertex words (v3 = 0 for a GT3)
//!   6  key of the older recorded key call | key of the newer << 16
//!   7  older call depths d0 | d1 << 16, 8  d2 | d3 << 16
//!   9  newer call depths d0 | d1 << 16, 10 d2 | d3 << 16
//!   11 older call count | newer call count << 8 | calls recorded << 16
//!
//! `note_key` runs inside the world key function and keeps the last two
//! calls; a refined pair emits two keys then one packet with their mean, every
//! other emitter calls it once per packet.

use psx_goldsrc::ordering::PrimitiveDepths;

pub const CAP: usize = 560;
pub const WORDS: usize = 12;

#[no_mangle]
pub static mut ORD_BUF: [[u32; WORDS]; CAP] = [[0; WORDS]; CAP];
#[no_mangle]
pub static mut ORD_N: u32 = 0;

static mut CALL_KEY: [u16; 2] = [0; 2];
static mut CALL_DEPTH: [[u16; 4]; 2] = [[0; 4]; 2];
static mut CALL_COUNT: [u8; 2] = [0; 2];
static mut CALLS: u8 = 0;
static mut FACE: u32 = 0;
static mut KIND: u32 = 0;

// `PrimitiveDepths` keeps its fields private; read them back from the value.
// The layout (four i32 then the u8 count) is checked by the join tool, which
// recomputes every key from the depths it reads here.
const _: () = assert!(core::mem::size_of::<PrimitiveDepths>() == 20);

#[inline(never)]
pub unsafe fn set_face(kind: u32, face: usize, radius: i32) {
    KIND = kind;
    FACE = face as u32 | (radius.clamp(0, 65535) as u32) << 16;
}

#[inline(never)]
pub unsafe fn note_key(depths: PrimitiveDepths, key: usize) {
    let raw: [u32; 5] = core::mem::transmute_copy(&depths);
    CALL_KEY[0] = CALL_KEY[1];
    CALL_DEPTH[0] = CALL_DEPTH[1];
    CALL_COUNT[0] = CALL_COUNT[1];
    CALL_KEY[1] = key as u16;
    let mut i = 0;
    while i < 4 {
        CALL_DEPTH[1][i] = (raw[i] as i32).clamp(0, 65535) as u16;
        i += 1;
    }
    CALL_COUNT[1] = (raw[4] & 0xff) as u8;
    if CALLS < 2 {
        CALLS += 1;
    }
}

/// Record one packet about to enter the ordering table. `p` points at the
/// packet including its tag word, `words` is its `WORDS`.
#[inline(never)]
pub unsafe fn note_packet(site: u32, otz: usize, p: *const u32, words: u8) {
    let quad = words >= 12;
    let n = if quad { 4u32 } else { 3 };
    let e = &mut ORD_BUF[(ORD_N as usize) % CAP];
    e[0] = (otz as u32 & 0xffff) | (n << 16) | (site << 20) | (KIND << 28);
    e[1] = FACE;
    e[2] = *p.add(3);
    e[3] = *p.add(6);
    e[4] = *p.add(9);
    e[5] = if quad { *p.add(12) } else { 0 };
    e[6] = CALL_KEY[0] as u32 | (CALL_KEY[1] as u32) << 16;
    e[7] = CALL_DEPTH[0][0] as u32 | (CALL_DEPTH[0][1] as u32) << 16;
    e[8] = CALL_DEPTH[0][2] as u32 | (CALL_DEPTH[0][3] as u32) << 16;
    e[9] = CALL_DEPTH[1][0] as u32 | (CALL_DEPTH[1][1] as u32) << 16;
    e[10] = CALL_DEPTH[1][2] as u32 | (CALL_DEPTH[1][3] as u32) << 16;
    e[11] = CALL_COUNT[0] as u32 | (CALL_COUNT[1] as u32) << 8 | (CALLS as u32) << 16;
    ORD_N = ORD_N.wrapping_add(1);
    CALLS = 0;
}

/// Record one studio-model triangle at its table insertion. `key` is the fractional model key
/// (quarter units of depth, sixteen per table bucket); `p` points at the packet including its tag.
#[inline(never)]
pub unsafe fn note_model(key: u32, p: *const u32) {
    let otz = key >> 4;
    let depth = (key >> 2) as u16 as u32;
    let e = &mut ORD_BUF[(ORD_N as usize) % CAP];
    e[0] = (otz & 0xffff) | (3 << 16) | (9 << 20);
    e[1] = 0;
    e[2] = *p.add(3);
    e[3] = *p.add(5);
    e[4] = *p.add(7);
    e[5] = 0;
    e[6] = otz << 16;
    e[7] = 0;
    e[8] = 0;
    e[9] = depth | depth << 16;
    e[10] = depth | depth << 16;
    e[11] = 3 << 8 | 1 << 16;
    ORD_N = ORD_N.wrapping_add(1);
    CALLS = 0;
}
