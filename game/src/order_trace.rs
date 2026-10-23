//! Ordering oracle stream (cargo feature `order-trace`, off by default).
//!
//! For one armed frame the guest writes every primitive the renderer put in the world ordering
//! table to PSoXide's guest debug-log port, as text lines the host tool `hl-order-oracle`
//! parses. The host rebuilds the frame twice from the same transformed vertices: painted in
//! the exact order the GPU would draw (the table walk), and with a per-pixel depth test. Pixels
//! where the two disagree are wrong-order pixels. Nothing here is compiled without the feature,
//! so shipping images are unchanged; there is no RAM ring, every record leaves through the port.
//!
//! Lines (all numbers are fixed-width lowercase hex, `|` separated):
//!   `OF|frame|eye x|eye y|eye z|r00..r22|h`  frame header (eye is the cooked world position, r the
//!         view rotation rows in Q12, h the projection distance; screen centre is 160,120)
//!   `OP|addr|kind|mode|n|key|face|policy|z0|z1|z2|z3|brush|ox|oy|oz`
//!         one per packet at its table insertion. `addr` is the packet address (24 bits), `kind`
//!         is the emission class (1 loop face, 2 raw-tri face, 3 patch face, 4 submodel face,
//!         5 crack backstop of a split quad, 6 patch underlay, 7 depth-split cell, 8 beam,
//!         9 studio model), `mode` how the depths were derived (1 refined pair, 2 one key call,
//!         3 one key call with a vertex count mismatch, 0 not derivable), `n` is 3 or 4,
//!         `key` the table slot, `face` the cooked face, `policy` the emit policy byte (blend 0x03,
//!         wave 0x04, cutout-backed 0x08, local depth 0x10, coplanar backdrop 0x20, tram grid 0x40,
//!         far key 0x80), `brush` the submodel number of the entity being drawn (0 = world) and
//!         `ox oy oz` its draw offset (i16), `zi` the per-vertex depth the key was
//!         taken from (the GTE SZ value), in the packet's own vertex order.
//!   `OW|addr|word0..wordK`                   one per packet in table walk (draw) order: the packet
//!         words after its tag, hex, exactly as the GPU reads them
//!   `OE|count`                                end of frame, count = packets walked
//!
//! The host joins `OP` onto `OW` by address; a walked packet with no `OP` (sprites, tram cache
//! replays, HUD-like extras) has no depth and is reported as such.

use psx_goldsrc::ordering::PrimitiveDepths;

const PORT: *mut u32 = 0xBF80_2F0C as *mut u32;
const LINE_CAP: usize = 200;

static mut LINE: [u8; LINE_CAP] = [0; LINE_CAP];
static mut LEN: usize = 0;
/// Armed by the probe (`snap`), consumed by the next frame that reaches `begin_frame`.
static mut ARMED: bool = false;
static mut ACTIVE: bool = false;

static mut CALL_KEY: [u16; 2] = [0; 2];
static mut CALL_DEPTH: [[u16; 4]; 2] = [[0; 4]; 2];
static mut CALL_COUNT: [u8; 2] = [0; 2];
static mut CALLS: u8 = 0;
static mut FACE: u32 = 0;
static mut KIND: u32 = 0;
static mut CUR_FACE: u32 = 0;
static mut BRUSH: u32 = 0;
static mut BRUSH_OFF: [i32; 3] = [0; 3];
// `PrimitiveDepths` keeps its fields private; read them back from the value (four i32 then the
// u8 count, checked here so a layout change breaks the build instead of the data).
const _: () = assert!(core::mem::size_of::<PrimitiveDepths>() == 20);

#[inline(never)]
unsafe fn put_hex(mut v: u32, digits: usize) {
    let mut i = digits;
    while i > 0 {
        i -= 1;
        if LEN + i < LINE_CAP {
            let d = (v & 15) as u8;
            LINE[LEN + i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        }
        v >>= 4;
    }
    LEN += digits;
}

#[inline(never)]
unsafe fn put_byte(b: u8) {
    if LEN < LINE_CAP {
        LINE[LEN] = b;
    }
    LEN += 1;
}

#[inline(never)]
unsafe fn flush_line() {
    let n = LEN.min(LINE_CAP);
    let mut i = 0;
    while i < n {
        core::ptr::write_volatile(PORT, LINE[i] as u32);
        i += 1;
    }
    core::ptr::write_volatile(PORT, b'\n' as u32);
    LEN = 0;
}

/// Ask for the next rendered frame to be streamed (probe op `snap`).
#[inline(never)]
pub unsafe fn arm() {
    ARMED = true;
}

/// Called once per frame as the world table is cleared. Starts a capture when armed.
#[inline(never)]
pub unsafe fn begin_frame(frame: u32, eye: [i32; 3], rot: [[i16; 3]; 3], h: i32) {
    ACTIVE = ARMED;
    ARMED = false;
    CALLS = 0;
    if !ACTIVE {
        return;
    }
    put_byte(b'O');
    put_byte(b'F');
    put_byte(b'|');
    put_hex(frame, 8);
    let mut i = 0;
    while i < 3 {
        put_byte(b'|');
        put_hex(eye[i] as u32, 8);
        i += 1;
    }
    // View rotation (rows, Q12) and the projection distance: the host recovers exact per-pixel
    // depth of a planar face from them.
    let mut r = 0;
    while r < 9 {
        put_byte(b'|');
        put_hex(rot[r / 3][r % 3] as u16 as u32, 4);
        r += 1;
    }
    put_byte(b'|');
    put_hex(h as u32, 4);
    flush_line();
}

#[inline(never)]
pub unsafe fn set_face(kind: u32, face: usize, radius: i32) {
    KIND = kind;
    if kind == 3 {
        // A patch run names its first loop vertex; the face index was set by the caller.
        FACE = CUR_FACE;
    } else {
        FACE = face as u32 | (radius.clamp(0, 65535) as u32) << 16;
    }
}

/// Mark the packets that follow as one emission class (5 = a split quad's crack backstop).
#[inline(never)]
pub unsafe fn set_kind(kind: u32) -> u32 {
    let old = KIND;
    KIND = kind;
    old
}

/// The brush entity (submodel number, 0 = none) the packets that follow belong to, and the offset
/// its vertices are drawn at.
#[inline(never)]
pub unsafe fn note_brush(model: u32, off: [i32; 3]) {
    BRUSH = model;
    BRUSH_OFF = off;
}

/// The face a following patch run belongs to (its radius rides in the upper half).
#[inline(never)]
pub unsafe fn set_cur_face(face: usize, radius: i32) {
    CUR_FACE = face as u32 | (radius.clamp(0, 65535) as u32) << 16;
}

#[inline(never)]
pub unsafe fn note_key(depths: PrimitiveDepths, key: usize) {
    if !ACTIVE {
        return;
    }
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

/// Record one packet about to enter the ordering table. `p` points at the packet including its
/// tag word, `words` is its `WORDS`.
#[inline(never)]
pub unsafe fn note_packet(site: u32, otz: usize, p: *const u32, words: u8) {
    if !ACTIVE {
        return;
    }
    let quad = words >= 12;
    let n = if quad { 4usize } else { 3 };
    let key = otz as u16;
    let (k0, k1) = (CALL_KEY[0], CALL_KEY[1]);
    let single = k1 == key;
    let pair = CALLS >= 2 && (k0 as u32 + k1 as u32) / 2 == key as u32;
    let mut mode = 0u32;
    let mut d = [0u16; 4];
    if quad && pair && !(single && k0 != k1) {
        // try_emit_quad_ctx pair: tri(pb,pa,pc) then tri(pa,pd,pc); packet = pb,pa,pc,pd.
        d = [
            CALL_DEPTH[0][0],
            CALL_DEPTH[0][1],
            CALL_DEPTH[0][2],
            CALL_DEPTH[1][1],
        ];
        mode = 1;
    } else if single {
        let mut i = 0;
        if CALL_COUNT[1] as usize == n {
            while i < n {
                d[i] = CALL_DEPTH[1][i];
                i += 1;
            }
            mode = 2;
        } else {
            while i < n {
                d[i] = CALL_DEPTH[1][if i < 3 { i } else { 3 }];
                i += 1;
            }
            mode = 3;
        }
    }
    let kind = if site >= 6 { site } else { KIND & 15 };
    put_byte(b'O');
    put_byte(b'P');
    put_byte(b'|');
    put_hex(p as usize as u32 & 0xff_ffff, 6);
    put_byte(b'|');
    put_hex(kind, 1);
    put_byte(b'|');
    put_hex(mode, 1);
    put_byte(b'|');
    put_hex(n as u32, 1);
    put_byte(b'|');
    put_hex(otz as u32, 4);
    put_byte(b'|');
    put_hex(FACE & 0xffff, 4);
    put_byte(b'|');
    put_hex(crate::EMIT_POLICY.0 as u32, 2);
    let mut i = 0;
    while i < 4 {
        put_byte(b'|');
        put_hex(d[i] as u32, 4);
        i += 1;
    }
    put_byte(b'|');
    put_hex(BRUSH, 4);
    i = 0;
    while i < 3 {
        put_byte(b'|');
        put_hex(BRUSH_OFF[i] as u32 & 0xffff, 4);
        i += 1;
    }
    flush_line();
    CALLS = 0;
}

/// Record one studio-model triangle at its table insertion. `key` is the fractional model key
/// (quarter units of depth, sixteen per table bucket) with the model bias applied; `bias` is that
/// bias in the same units so the host can compare true depths.
#[inline(never)]
pub unsafe fn note_model(key: u32, bias: u32, p: *const u32) {
    if !ACTIVE {
        return;
    }
    let z = ((key + bias) >> 2).min(0xffff);
    put_byte(b'O');
    put_byte(b'P');
    put_byte(b'|');
    put_hex(p as usize as u32 & 0xff_ffff, 6);
    put_byte(b'|');
    put_hex(9, 1);
    put_byte(b'|');
    put_hex(2, 1);
    put_byte(b'|');
    put_hex(3, 1);
    put_byte(b'|');
    put_hex(key >> 4, 4);
    put_byte(b'|');
    put_hex(0, 4);
    put_byte(b'|');
    put_hex(0, 2);
    let mut i = 0;
    while i < 3 {
        put_byte(b'|');
        put_hex(z, 4);
        i += 1;
    }
    put_byte(b'|');
    put_hex(0, 4);
    put_byte(b'|');
    put_hex(0, 4);
    put_byte(b'|');
    put_hex(0, 4);
    put_byte(b'|');
    put_hex(0, 4);
    put_byte(b'|');
    put_hex(0, 4);
    flush_line();
    CALLS = 0;
}

/// Stream the finished world table in draw order, then close the frame.
///
/// # Safety
/// As `OrderingTable::packets`: every chained packet must be live.
#[inline(never)]
pub unsafe fn end_frame<const N: usize>(ot: &psx_gpu::ot::OrderingTable<N>) {
    if !ACTIVE {
        return;
    }
    ACTIVE = false;
    let mut count = 0u32;
    for (p, words) in ot.packets() {
        put_byte(b'O');
        put_byte(b'W');
        put_byte(b'|');
        put_hex(p as usize as u32 & 0xff_ffff, 6);
        put_byte(b'|');
        let mut i = 1usize;
        while i <= words as usize {
            put_hex(*p.add(i), 8);
            i += 1;
        }
        flush_line();
        count += 1;
    }
    put_byte(b'O');
    put_byte(b'E');
    put_byte(b'|');
    put_hex(count, 8);
    flush_line();
}
