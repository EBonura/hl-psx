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
//! Entry layout (`WORDS` u32 each, newest at `(ORD_N - 1) % CAP`). A vertex is its
//! screen (x, y) kept to the 11 bits per axis the GPU keeps (x | y << 11), which is
//! what the host matches against a GP0 draw dump.
//!   0..4  vertex v0..v3 (v3 = 0 for a GT3) | that vertex's depth (clamped to 1023) << 22
//!   4     key (9 bits) | (n - 3) << 9 | site << 10 | mode << 14 | kind << 16 | radius << 19
//!           mode: 1 = refined pair, 2 = one key call, 3 = one key call with a vertex
//!           count mismatch, 0 = not derivable; depths are in the packet's own vertex order
//!           kind: 1 loop face, 2 raw-tri face, 3 patch face, 4 submodel face, 5 crack
//!           backstop of a split patch quad, 0 unknown
//!           radius: the face's cooked bounding radius (clamped to 4095)
//!   5     face index (the cooked face the packet came from)
//!
//! Studio-model triangles go to a second, smaller ring (`MOD_BUF`, `MOD_N`,
//! `MOD_WORDS` u32 each): the three vertices packed as above, and the fractional
//! model key (quarter units of depth, sixteen per table bucket) in the spare
//! ten bits of word 0 (low) and word 1 (high).
//!
//! `note_key` runs inside the world key function and keeps the last two
//! calls; a refined pair emits two keys then one packet with their mean, every
//! other emitter calls it once per packet.

use psx_goldsrc::ordering::PrimitiveDepths;

pub const CAP: usize = 520;
pub const WORDS: usize = 6;
pub const MOD_CAP: usize = 700;
pub const MOD_WORDS: usize = 3;

#[no_mangle]
pub static mut ORD_BUF: [[u32; WORDS]; CAP] = [[0; WORDS]; CAP];
#[no_mangle]
pub static mut ORD_N: u32 = 0;
#[no_mangle]
pub static mut MOD_BUF: [[u32; MOD_WORDS]; MOD_CAP] = [[0; MOD_WORDS]; MOD_CAP];
#[no_mangle]
pub static mut MOD_N: u32 = 0;

static mut CALL_KEY: [u16; 2] = [0; 2];
static mut CALL_DEPTH: [[u16; 4]; 2] = [[0; 4]; 2];
static mut CALL_COUNT: [u8; 2] = [0; 2];
static mut CALLS: u8 = 0;
static mut FACE: u32 = 0;
static mut KIND: u32 = 0;
static mut CUR_FACE: u32 = 0;
// `PrimitiveDepths` keeps its fields private; read them back from the value.
// The layout (four i32 then the u8 count) is checked by the join tool, which
// recomputes every key from the depths it reads here.
const _: () = assert!(core::mem::size_of::<PrimitiveDepths>() == 20);

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

/// The face a following patch run belongs to (its radius rides in the upper half).
#[inline(never)]
pub unsafe fn set_cur_face(face: usize, radius: i32) {
    CUR_FACE = face as u32 | (radius.clamp(0, 65535) as u32) << 16;
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

/// An (x, y) vertex word reduced to the 11 bits per axis the GPU keeps.
#[inline(always)]
fn pack_xy(w: u32) -> u32 {
    (w & 0x7ff) | ((w >> 16) & 0x7ff) << 11
}

/// Record one packet about to enter the ordering table. `p` points at the
/// packet including its tag word, `words` is its `WORDS`.
#[inline(never)]
pub unsafe fn note_packet(site: u32, otz: usize, p: *const u32, words: u8) {
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
        d = [CALL_DEPTH[0][0], CALL_DEPTH[0][1], CALL_DEPTH[0][2], CALL_DEPTH[1][1]];
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
    let v = [*p.add(3), *p.add(6), *p.add(9), if quad { *p.add(12) } else { 0 }];
    let e = &mut ORD_BUF[(ORD_N as usize) % CAP];
    let mut i = 0;
    while i < 4 {
        e[i] = pack_xy(v[i]) | ((d[i] as u32).min(1023)) << 22;
        i += 1;
    }
    let radius = (FACE >> 16).min(4095);
    e[4] = (otz as u32 & 0x1ff)
        | (((n - 3) as u32) << 9)
        | ((site & 15) << 10)
        | (mode << 14)
        | ((KIND & 7) << 16)
        | (radius << 19);
    e[5] = FACE & 0xffff;
    ORD_N = ORD_N.wrapping_add(1);
    CALLS = 0;
}

/// Record one studio-model triangle at its table insertion. `key` is the fractional model key
/// (quarter units of depth, sixteen per table bucket); `p` points at the packet including its tag.
#[inline(never)]
pub unsafe fn note_model(key: u32, p: *const u32) {
    let e = &mut MOD_BUF[(MOD_N as usize) % MOD_CAP];
    let key = key.min(0x1fff);
    e[0] = pack_xy(*p.add(3)) | (key & 0x3ff) << 22;
    e[1] = pack_xy(*p.add(5)) | (key >> 10) << 22;
    e[2] = pack_xy(*p.add(7));
    MOD_N = MOD_N.wrapping_add(1);
    CALLS = 0;
}
