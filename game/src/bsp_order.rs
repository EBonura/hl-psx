//! BSP back-to-front ordering for the world (stage 1).
//!
//! The cooked world faces sit in render-node order (HLMI), so a node's faces are one run. Each
//! frame the visible nodes are walked far side first from the eye and every visible node, and
//! every brush entity anchored in the tree, takes the next ordering-table key. Faces then use
//! the key of their node instead of a depth derived from their vertices, which is exact for any
//! two world surfaces. Primitives that live outside the tree (models, sprites, beams) keep their
//! depth keys, mapped through a monotone depth-to-key table built from the same walk so they
//! interleave with the world where their depth says they belong.
//!
//! Static cost (bytes): `VIS_*` 5 per visible node, `KEY_K` 1, `KIDX` one per cached face
//! record, `ENT_KEY` one per entity, `DEPTH_KEY16` two per table slot; `bsp_order_ram_bytes`
//! reports the sum.

use crate::map::Map;

/// Visible render nodes tracked per camera leaf (nodes that own at least one listed face).
pub const MAX_VIS: usize = 1024;
/// Brush entities placed in the tree per frame.
pub const MAX_ITEMS: usize = 48;
/// Cached face records with a node slot (`MAX_PVS_FACE_RECS`).
pub const KIDX_CAP: usize = 1024;
const NO_SLOT: u16 = 0xffff;
const WALK_DEPTH: usize = 64;

/// The cached visible-node list is valid for the PVS in use.
static mut VALID: bool = false;
/// This frame's walk ran and the world, entities and depth-keyed primitives take rank keys.
static mut ACTIVE: bool = false;
static mut VIS_N: usize = 0;
/// Most visible nodes any rebuilt view has listed (counts past the table too).
static mut VIS_PEAK: u16 = 0;
static mut VIS_SEEN: u16 = 0;
static mut VIS_NODE: [u16; MAX_VIS] = [0; MAX_VIS];
static mut VIS_FIRST: [u16; MAX_VIS] = [0; MAX_VIS];
static mut VIS_COUNT: [u8; MAX_VIS] = [0; MAX_VIS];
static mut KIDX: [u16; KIDX_CAP] = [NO_SLOT; KIDX_CAP];
/// Ordering-table key of each visible node (valid after `frame`).
static mut KEY_K: [u16; MAX_VIS] = [0; MAX_VIS];
/// Items anchored in the tree this frame, sorted by anchor node.
static mut ITEM_N: usize = 0;
static mut ITEM_ANCHOR: [u16; MAX_ITEMS] = [0; MAX_ITEMS];
static mut ITEM_SIDE: [u8; MAX_ITEMS] = [0; MAX_ITEMS];
static mut ITEM_ID: [u8; MAX_ITEMS] = [0; MAX_ITEMS];
static mut ITEM_KEY: [u16; MAX_ITEMS] = [0; MAX_ITEMS];
static mut ITEM_DEPTH: [u16; MAX_ITEMS] = [0; MAX_ITEMS];
/// Key levels each item holds (1 for a small entity) and its radius in depth buckets.
static mut ITEM_LEVELS: [u8; MAX_ITEMS] = [1; MAX_ITEMS];
static mut ITEM_RB: [u8; MAX_ITEMS] = [0; MAX_ITEMS];
static mut ITEM_HI: [u16; MAX_ITEMS] = [0; MAX_ITEMS];
/// The face being emitted belongs to a banded item: its key is `CUR_LO` plus its relative depth.
static mut CUR_LEVELS: u8 = 1;
static mut CUR_LO: u16 = 0;
static mut CUR_HI: u16 = 0;
static mut CUR_DLO: i32 = 0;
static mut CUR_SPAN: i32 = 1;
/// Most key levels one entity takes (its radius in depth buckets, doubled).
pub const MAX_BAND: u8 = 24;
/// Monotone depth bucket -> sixteenths of an ordering-table key (see `build_depth_map`).
static mut DEPTH_KEY16: [u16; crate::DEPTH_LEN] = [0; crate::DEPTH_LEN];
/// Key of the next walk step in 8.8 fixed point, counting down.
static mut NEXT_KEY: u32 = 0;
/// Fixed-point decrement per walk step (256 = one key; less when the walk has more steps than keys).
static mut KEY_STEP: u32 = 256;

/// Bytes of static RAM the ordering tables take.
#[allow(dead_code)] // reported by the host walk check
pub const fn ram_bytes() -> usize {
    MAX_VIS * 2 * 2
        + MAX_VIS
        + MAX_VIS * 2
        + KIDX_CAP * 2
        + MAX_ITEMS * (2 + 1 + 1 + 2 + 2)
        + crate::DEPTH_LEN * 2
        + 16
}

#[inline(always)]
pub unsafe fn active() -> bool {
    ACTIVE
}

/// The cached node list is usable (the map has a node table and the view fits the tables).
#[inline(always)]
pub unsafe fn valid() -> bool {
    VALID
}

/// Clear the per-frame state before the world is built.
#[inline(always)]
pub unsafe fn begin_frame() {
    ACTIVE = false;
    ITEM_N = 0;
}

fn any_marked(marks: &[u32], lo: usize, hi: usize) -> bool {
    let mut f = lo;
    while f < hi {
        let w = f >> 5;
        if w >= marks.len() {
            return false;
        }
        let bit = f & 31;
        let span = (32 - bit).min(hi - f);
        let mask = if span == 32 {
            u32::MAX
        } else {
            ((1u32 << span) - 1) << bit
        };
        if marks[w] & mask != 0 {
            return true;
        }
        f += span;
    }
    false
}

/// Rebuild the visible-node list and the per-record node slots after the PVS face list changed.
/// `index` is the cached face list (face id per record), `marks` the bitset of listed faces.
#[inline(never)]
pub unsafe fn rebuild(m: &Map, face_count: usize, index: &[u16], marks: &[u32]) {
    ACTIVE = false;
    VALID = false;
    VIS_N = 0;
    if !m.has_node_faces() {
        return;
    }
    let run_faces = m.node_face_run_faces();
    let n_nodes = m.n_nodes;
    let blocks = n_nodes.div_ceil(32);
    let mut seen = 0usize;
    let mut b = 0usize;
    while b < blocks {
        let mut first = m.node_face_checkpoint(b);
        let end_first = m.node_face_checkpoint(b + 1);
        if end_first > first && any_marked(marks, first, end_first.min(run_faces)) {
            let mut n = b << 5;
            let stop = (n + 32).min(n_nodes);
            while n < stop {
                let c = m.node_face_count(n);
                if c != 0 && any_marked(marks, first, first + c) {
                    seen += 1;
                    if VIS_N < MAX_VIS {
                        VIS_NODE[VIS_N] = n as u16;
                        VIS_FIRST[VIS_N] = first as u16;
                        VIS_COUNT[VIS_N] = c.min(255) as u8;
                        VIS_N += 1;
                    }
                }
                first += c;
                n += 1;
            }
        }
        b += 1;
    }
    core::ptr::write_volatile(core::ptr::addr_of_mut!(VIS_SEEN), seen as u16);
    if seen as u16 > core::ptr::read_volatile(core::ptr::addr_of!(VIS_PEAK)) {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(VIS_PEAK), seen as u16);
    }
    if seen > MAX_VIS {
        VIS_N = 0;
        return;
    }
    // Node slot of every cached record, by the node run its face falls in.
    let cached = face_count.min(KIDX_CAP).min(index.len());
    let mut e = 0usize;
    while e < cached {
        KIDX[e] = slot_of_face(index[e] as usize);
        e += 1;
    }
    while e < KIDX_CAP {
        KIDX[e] = NO_SLOT;
        e += 1;
    }
    VALID = true;
}

/// Visible-node slot whose run holds `face`, or `NO_SLOT`.
#[inline]
unsafe fn slot_of_face(face: usize) -> u16 {
    let (mut lo, mut hi) = (0usize, VIS_N);
    while lo < hi {
        let mid = (lo + hi) >> 1;
        if (VIS_FIRST[mid] as usize) <= face {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return NO_SLOT;
    }
    let k = lo - 1;
    if face < VIS_FIRST[k] as usize + VIS_COUNT[k] as usize {
        k as u16
    } else {
        NO_SLOT
    }
}

/// Ordering key of a cached face record or listed face, `0` when the face has no node slot.
#[inline(always)]
pub unsafe fn face_key(entry: usize, face: usize) -> u16 {
    if !ACTIVE {
        return 0;
    }
    let slot = if entry < KIDX_CAP {
        KIDX[entry]
    } else {
        slot_of_face(face)
    };
    if slot == NO_SLOT {
        0
    } else {
        KEY_K[slot as usize]
    }
}

/// The node and child side (0 = front child, 1 = back child) whose child is the leaf holding `p`.
#[inline(never)]
pub unsafe fn anchor_of(m: &Map, p: [i32; 3]) -> (u16, u8) {
    let mut idx = 0usize;
    let mut guard = 0;
    loop {
        let nd = m.node(idx);
        let side = dot_q5(nd.n, p) - nd.dist_q5;
        let (next, s) = if side >= 0 {
            (nd.c0, 0u8)
        } else {
            (nd.c1, 1u8)
        };
        guard += 1;
        if next < 0 || guard > 128 {
            return (idx as u16, s);
        }
        idx = next as usize;
    }
}

#[inline(always)]
fn dot_q5(row: [i16; 3], e: [i32; 3]) -> i32 {
    ((row[0] as i32 * e[0]) + (row[1] as i32 * e[1]) + (row[2] as i32 * e[2]))
        >> (crate::map::PLANE_NORMAL_FRAC_BITS - 5)
}

/// Units a brush entity's placement point moves toward the eye: a button, panel or platform sits
/// flush with a surface of the world, and its visible faces are on the eye side of its centre.
const NUDGE: i64 = 16;

/// `center` moved `NUDGE` units toward `eye` (unchanged when the eye is closer than that).
#[inline]
pub fn toward_eye(center: [i32; 3], eye: [i32; 3]) -> [i32; 3] {
    let d = [
        (eye[0] - center[0]) as i64,
        (eye[1] - center[1]) as i64,
        (eye[2] - center[2]) as i64,
    ];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) as u64;
    let len = len.isqrt() as i64;
    if len <= NUDGE {
        return center;
    }
    [
        center[0] + (d[0] * NUDGE / len) as i32,
        center[1] + (d[1] * NUDGE / len) as i32,
        center[2] + (d[2] * NUDGE / len) as i32,
    ]
}

/// Queue one brush entity for this frame's walk.
#[inline]
pub unsafe fn add_item(anchor: (u16, u8), id: u8, depth_bucket: u16, radius_bucket: u8) {
    if !VALID || ITEM_N >= MAX_ITEMS {
        return;
    }
    // insertion sort by anchor
    let mut i = ITEM_N;
    while i > 0 && ITEM_ANCHOR[i - 1] > anchor.0 {
        ITEM_ANCHOR[i] = ITEM_ANCHOR[i - 1];
        ITEM_SIDE[i] = ITEM_SIDE[i - 1];
        ITEM_ID[i] = ITEM_ID[i - 1];
        ITEM_DEPTH[i] = ITEM_DEPTH[i - 1];
        ITEM_LEVELS[i] = ITEM_LEVELS[i - 1];
        ITEM_RB[i] = ITEM_RB[i - 1];
        i -= 1;
    }
    ITEM_ANCHOR[i] = anchor.0;
    ITEM_SIDE[i] = anchor.1;
    ITEM_ID[i] = id;
    ITEM_DEPTH[i] = depth_bucket;
    ITEM_RB[i] = radius_bucket;
    ITEM_LEVELS[i] = (radius_bucket as u16 * 2 + 1).min(MAX_BAND as u16) as u8;
    ITEM_N += 1;
}

/// Make item `id` the owner of the faces about to be emitted; returns its nearest key, `0` if unplaced.
#[inline]
pub unsafe fn select_item(id: u8) -> u16 {
    if !ACTIVE {
        return 0;
    }
    let mut i = 0usize;
    while i < ITEM_N {
        if ITEM_ID[i] == id {
            CUR_LEVELS = ITEM_LEVELS[i];
            CUR_LO = ITEM_KEY[i];
            CUR_HI = ITEM_HI[i];
            let rb = ITEM_RB[i] as i32;
            CUR_DLO = ITEM_DEPTH[i] as i32 - rb;
            CUR_SPAN = (rb * 2).max(1);
            return ITEM_KEY[i];
        }
        i += 1;
    }
    0
}

/// The faces about to be emitted are a node's (one key each).
#[inline(always)]
pub unsafe fn select_node() {
    CUR_LEVELS = 1;
}

#[inline(always)]
pub unsafe fn banded() -> bool {
    CUR_LEVELS > 1
}

/// Key of a face of the selected banded item from its depth key `dk` (depth buckets).
#[inline]
pub unsafe fn band_key(dk: usize) -> u16 {
    let levels = CUR_LEVELS as i32;
    let off = (((dk as i32 - CUR_DLO) * (levels - 1)) / CUR_SPAN).clamp(0, levels - 1);
    (CUR_LO as i32 + off).min(CUR_HI as i32) as u16
}

#[derive(Clone, Copy)]
struct Frame {
    node: u16,
    end: u16,
    vlo: u16,
    vhi: u16,
    elo: u16,
    ehi: u16,
    stage: u8,
    near_first: bool,
}

/// First index in `[lo, hi)` of a node-sorted `u16` list whose value is at least `bound`.
#[inline]
fn lower_bound16(list: &[u16], lo: usize, hi: usize, bound: u16) -> usize {
    let (mut lo, mut hi) = (lo, hi);
    while lo < hi {
        let mid = (lo + hi) >> 1;
        if list[mid] < bound {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Walk the visible nodes far side first and hand out keys (counting down from `key_top`).
#[inline(never)]
pub unsafe fn walk(m: &Map, eye: [i32; 3], key_top: u16) {
    if !VALID {
        return;
    }
    NEXT_KEY = (key_top as u32) << 8;
    let mut steps = VIS_N as u32;
    let mut q = 0usize;
    while q < ITEM_N {
        steps += ITEM_LEVELS[q] as u32;
        q += 1;
    }
    KEY_STEP = if steps > key_top as u32 - 1 {
        (((key_top as u32 - 1) << 8) / steps).max(1)
    } else {
        256
    };
    let mut stack = [Frame {
        node: 0,
        end: 0,
        vlo: 0,
        vhi: 0,
        elo: 0,
        ehi: 0,
        stage: 0,
        near_first: false,
    }; WALK_DEPTH];
    let mut sp = 1usize;
    stack[0] = Frame {
        node: 0,
        end: u16::MAX,
        vlo: 0,
        vhi: VIS_N as u16,
        elo: 0,
        ehi: ITEM_N as u16,
        stage: 0,
        near_first: false,
    };
    while sp > 0 {
        let f = &mut stack[sp - 1];
        let nd = m.node(f.node as usize);
        let split = if nd.c1 >= 0 { nd.c1 as u16 } else { f.end };
        match f.stage {
            0 | 2 => {
                // stage 0 runs the far side, stage 2 the near side
                let front = {
                    let side = dot_q5(nd.n, eye) - nd.dist_q5;
                    side >= 0
                };
                f.near_first = front;
                // which child is visited now: far in stage 0, near in stage 2
                let want_front = if f.stage == 0 { !front } else { front };
                let (child, side_flag) = if want_front {
                    (nd.c0, 0u8)
                } else {
                    (nd.c1, 1u8)
                };
                f.stage += 1;
                if child >= 0 {
                    // sub-ranges of this child's subtree
                    let own = (f.vlo < f.vhi && VIS_NODE[f.vlo as usize] == f.node) as u16;
                    let v0 = f.vlo + own;
                    let vmid = lower_bound16(
                        &*core::ptr::addr_of!(VIS_NODE),
                        v0 as usize,
                        f.vhi as usize,
                        split,
                    ) as u16;
                    let e0 = f.elo + count_anchor(f.elo, f.ehi, f.node);
                    let emid = lower_bound16(
                        &*core::ptr::addr_of!(ITEM_ANCHOR),
                        e0 as usize,
                        f.ehi as usize,
                        split,
                    ) as u16;
                    let (vlo, vhi, elo, ehi, end) = if want_front {
                        // front child = c0: nodes in (node, split)
                        (v0, vmid, e0, emid, split)
                    } else {
                        (vmid, f.vhi, emid, f.ehi, f.end)
                    };
                    if vlo < vhi || elo < ehi {
                        if sp < WALK_DEPTH {
                            stack[sp] = Frame {
                                node: child as u16,
                                end,
                                vlo,
                                vhi,
                                elo,
                                ehi,
                                stage: 0,
                                near_first: false,
                            };
                            sp += 1;
                        }
                    }
                } else {
                    // leaf: entities anchored here on this side
                    let mut i = f.elo as usize;
                    while i < f.ehi as usize && ITEM_ANCHOR[i] == f.node {
                        if ITEM_SIDE[i] == side_flag {
                            let first = take_key();
                            let mut last = first;
                            let mut l = 1u8;
                            while l < ITEM_LEVELS[i] {
                                last = take_key();
                                l += 1;
                            }
                            ITEM_KEY[i] = last;
                            ITEM_HI[i] = first;
                        }
                        i += 1;
                    }
                }
            }
            1 => {
                // the node's own faces sit between the two sides
                if f.vlo < f.vhi && VIS_NODE[f.vlo as usize] == f.node {
                    KEY_K[f.vlo as usize] = take_key();
                }
                f.stage = 2;
            }
            _ => {
                sp -= 1;
            }
        }
    }
}

#[inline(always)]
unsafe fn take_key() -> u16 {
    let k = (NEXT_KEY >> 8) as u16;
    if NEXT_KEY >= (2 << 8) + KEY_STEP {
        NEXT_KEY -= KEY_STEP;
    }
    k.max(1)
}

/// Items anchored at exactly `node` starting at `lo` (items are sorted by anchor).
#[inline]
unsafe fn count_anchor(lo: u16, hi: u16, node: u16) -> u16 {
    let mut i = lo as usize;
    while i < hi as usize && ITEM_ANCHOR[i] == node {
        i += 1;
    }
    (i - lo as usize) as u16
}

/// Build the monotone depth-bucket -> key16 table from `(depth bucket, key)` samples of the walk.
/// `depth_of_face(f)` gives the depth bucket of a face; item depths were given to `add_item`.
#[inline(never)]
pub unsafe fn build_depth_map(depth_of_face: &mut dyn FnMut(usize) -> usize) {
    if !VALID {
        return;
    }
    // keys 0 means "no sample"; per bucket keep the largest (farthest) key
    let table = &mut *core::ptr::addr_of_mut!(DEPTH_KEY16);
    let mut b = 0;
    while b < table.len() {
        table[b] = 0;
        b += 1;
    }
    let mut k = 0usize;
    while k < VIS_N {
        let bucket = depth_of_face(VIS_FIRST[k] as usize).clamp(1, table.len() - 1);
        let key = KEY_K[k];
        if key > table[bucket] {
            table[bucket] = key;
        }
        k += 1;
    }
    let mut i = 0usize;
    while i < ITEM_N {
        let bucket = (ITEM_DEPTH[i] as usize).clamp(1, table.len() - 1);
        let mid = ITEM_KEY[i] + (ITEM_HI[i] - ITEM_KEY[i]) / 2;
        if mid > table[bucket] {
            table[bucket] = mid;
        }
        i += 1;
    }
    // near-to-far: keys never decrease with depth; interpolate between samples
    let mut prev_b = 0usize;
    let mut prev_k = 0u16;
    let mut b = 1usize;
    let mut first = true;
    while b < table.len() {
        let key = table[b];
        if key != 0 {
            let key = key.max(prev_k);
            if first {
                let mut x = 1usize;
                while x < b {
                    table[x] = key << 4;
                    x += 1;
                }
                first = false;
            } else {
                let span = (b - prev_b) as i32;
                let step = (((key - prev_k) as i32) << 4) / span;
                let mut acc = (prev_k as i32) << 4;
                let mut x = prev_b + 1;
                while x < b {
                    acc += step;
                    table[x] = acc as u16;
                    x += 1;
                }
            }
            table[b] = key << 4;
            prev_b = b;
            prev_k = key;
        }
        b += 1;
    }
    if first {
        return;
    }
    let mut x = prev_b + 1;
    while x < table.len() {
        table[x] = prev_k << 4;
        x += 1;
    }
    ACTIVE = true;
}

/// Depth bucket -> key (the bucket itself while the ranking is off).
#[inline(never)]
pub unsafe fn map_key(bucket: usize) -> usize {
    if ACTIVE {
        (DEPTH_KEY16[bucket.min(crate::DEPTH_LEN - 1)] >> 4) as usize
    } else {
        bucket
    }
}

/// Studio-model quarter-depth key mapped to the walk's key space, keeping its fine position.
#[inline(never)]
pub unsafe fn map_model_key(key: u16) -> u16 {
    if !ACTIVE {
        return key;
    }
    let b = ((key >> 4) as usize).min(crate::DEPTH_LEN - 2);
    let lo = DEPTH_KEY16[b] as u32;
    let hi = DEPTH_KEY16[b + 1] as u32;
    let fine = (key & 15) as u32;
    let mapped = lo + (((hi.saturating_sub(lo)) * fine) >> 4);
    mapped.clamp(16, (crate::OT_LEN as u32) * 16 - 1) as u16
}

/// Host check hooks: visible-node count and `(node, key)` of slot `k`.
#[allow(dead_code)]
pub unsafe fn debug_vis_len() -> usize {
    VIS_N
}
#[allow(dead_code)]
pub unsafe fn debug_slot(k: usize) -> (u16, u16) {
    (VIS_NODE[k], KEY_K[k])
}
#[allow(dead_code)]
pub unsafe fn debug_item(i: usize) -> (u16, u8, u8, u16) {
    (ITEM_ANCHOR[i], ITEM_SIDE[i], ITEM_ID[i], ITEM_KEY[i])
}
#[allow(dead_code)]
#[allow(dead_code)]
pub unsafe fn debug_item_len() -> usize {
    ITEM_N
}
