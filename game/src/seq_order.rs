//! BSP-sequenced draw order for the world pass.
//!
//! The PS1 has no depth buffer, and a scalar ordering-table key cannot order two overlapping
//! polygons whose depth ranges overlap (a long wall seen obliquely against anything near it).
//! The map's BSP does order them exactly. A back-to-front walk of the tree from the eye visits
//! the visible leaves in an order in which, along any ray, nearer geometry always comes later.
//! A face lies in front of everything behind its plane, so drawing each face at the position of
//! the FARTHEST visible leaf that marks it reproduces the painter's order exactly for world
//! geometry (measured by the ordering oracle against a depth-tested render of the same packets).
//!
//! This module computes, per frame, that sequence and turns it into table keys: one slot of two
//! buckets per leaf that adds faces (fewer slots when more leaves than SLOT_PAIRS, shared by runs
//! of adjacent leaves), farthest at the top of the table. World faces key at the slot of the
//! farthest leaf that marks them; the odd bucket between two slots carries the small biases the
//! key function applies (a parent behind its children, a liquid over its terrain).
//!
//! No per-face storage is added beyond one stamped halfword per face; the stamp makes the table
//! valid for exactly one pass without clearing it.

use crate::map::Map;
use crate::{MAX_FACES, MAX_LEAVES, MAX_NODES};

/// Key of the farthest slot.
const FAR_KEY: u32 = 639;
/// Distinct slots (each two buckets wide) a frame's leaf sequence is spread over.
const SLOT_PAIRS: u32 = 319;
/// Stamp field of a face key (the key itself needs ten bits).
const STAMP_SHIFT: u32 = 10;
const KEY_MASK: u16 = (1 << STAMP_SHIFT) - 1;

const NODE_VIS_WORDS: usize = MAX_NODES / 32;

/// Bit n: some leaf below node n is in the current visibility row.
static mut NODE_VIS: [u32; NODE_VIS_WORDS] = [0; NODE_VIS_WORDS];
/// Slot key of each leaf the current sequence visited; 0 for leaves it did not.
static mut LEAF_SLOT: [u16; MAX_LEAVES + 1] = [0; MAX_LEAVES + 1];
/// `stamp << 10 | key` per face, written when the pass first reaches the face.
static mut FACE_KEY: [u16; MAX_FACES] = [0; MAX_FACES];
static mut STAMP: u16 = 0;
/// Visible leaves of the row `prepare` saw, which sizes the slot spacing.
static mut VISIBLE_LEAVES: u32 = 0;
/// Leaves that took a slot in the last pass.
static mut LAST_USED: u32 = 0;
/// Whether the last `sequence` call produced a usable sequence.
static mut READY: bool = false;

/// Mark the nodes that have a visible leaf below them. Cold: run once per visibility rebuild.
///
/// # Safety
/// Single-threaded renderer state; `visibility` must be the row the face lists were built from.
pub unsafe fn prepare(m: &Map, visibility: &[u8]) {
    let words = core::ptr::addr_of_mut!(NODE_VIS).cast::<u32>();
    let n_nodes = m.n_nodes.min(MAX_NODES);
    let mut w = 0;
    while w < NODE_VIS_WORDS {
        *words.add(w) = 0;
        w += 1;
    }
    let n_vis = m.n_visleaves.min(MAX_LEAVES);
    let mut visible = 0u32;
    let mut i = 0;
    while i < n_vis {
        if visibility[i >> 3] & (1 << (i & 7)) != 0 {
            visible += 1;
        }
        i += 1;
    }
    VISIBLE_LEAVES = visible;
    // Children always have larger indices than their parent, so one backward pass suffices.
    let mut n = n_nodes;
    while n > 0 {
        n -= 1;
        let (c0, c1) = m.node_children(n);
        if child_visible(words, visibility, n_vis, c0) || child_visible(words, visibility, n_vis, c1) {
            *words.add(n >> 5) |= 1 << (n & 31);
        }
    }
}

#[inline(always)]
unsafe fn child_visible(words: *const u32, visibility: &[u8], n_vis: usize, child: i32) -> bool {
    if child < 0 {
        let leaf = (-child - 1) as usize;
        leaf >= 1 && leaf <= n_vis && visibility[(leaf - 1) >> 3] & (1 << ((leaf - 1) & 7)) != 0
    } else {
        *words.add((child as usize) >> 5) & (1 << (child as usize & 31)) != 0
    }
}

#[inline(always)]
unsafe fn node_visible(n: usize) -> bool {
    *core::ptr::addr_of!(NODE_VIS).cast::<u32>().add(n >> 5) & (1 << (n & 31)) != 0
}

/// Walk the tree back to front from `eye`, giving every visible leaf a slot and every face its
/// key, the slot of the farthest visible leaf that marks it. Call once per frame, after the
/// visibility row is current and before any key is read. `depth` is the view-space depth of a
/// world point.
///
/// # Safety
/// Single-threaded renderer state; `visibility` must be the row `prepare` saw.
pub unsafe fn sequence(
    m: &Map,
    visibility: &[u8],
    eye: [i32; 3],
    dot: fn([i16; 3], [i32; 3]) -> i32,
    _depth: &dyn Fn([i32; 3]) -> i32,
) {
    READY = false;
    let visible = VISIBLE_LEAVES;
    if visible == 0 || m.n_nodes == 0 || !node_visible(0) {
        return;
    }
    STAMP = (STAMP + 1) & 0x3f;
    if STAMP == 0 {
        STAMP = 1;
    }
    let stamp = STAMP << STAMP_SHIFT;
    let n_vis = m.n_visleaves.min(MAX_LEAVES);
    let mut slots = Slots {
        estimate: LAST_USED.max(1),
        acc: 0,
        pair: 0,
        last: FAR_KEY as u16,
        used: 0,
    };
    let mut stack = [0i16; 160];
    let mut sp = 1usize;
    stack[0] = 0;
    while sp > 0 {
        sp -= 1;
        let item = stack[sp] as i32;
        if item < 0 {
            let leaf = (-item - 1) as usize;
            if leaf >= 1 && leaf <= n_vis {
                emit_leaf(m, visibility, leaf, stamp, &mut slots);
            }
            continue;
        }
        let n = item as usize;
        if !node_visible(n) {
            continue;
        }
        let nd = m.node(n);
        let front = dot(nd.n, eye) - nd.dist_q5 >= 0;
        let (near, far) = if front { (nd.c0, nd.c1) } else { (nd.c1, nd.c0) };
        if sp + 2 > stack.len() {
            return;
        }
        // Far is popped first, so it is pushed last.
        stack[sp] = near as i16;
        stack[sp + 1] = far as i16;
        sp += 2;
    }
    LAST_USED = slots.used;
    READY = true;
}

/// Slot allocation state of one pass.
struct Slots {
    /// Leaves that took a slot in the previous pass: sizes this pass's spacing.
    estimate: u32,
    acc: u32,
    pair: u32,
    /// Key of the most recently allocated slot, shared by leaves that add no face.
    last: u16,
    used: u32,
}

/// Give `leaf` a slot and key the faces it marks that no farther leaf has. A leaf that adds no
/// face does not consume a slot.
#[inline(always)]
unsafe fn emit_leaf(m: &Map, visibility: &[u8], leaf: usize, stamp: u16, slots: &mut Slots) {
    if visibility[(leaf - 1) >> 3] & (1 << ((leaf - 1) & 7)) == 0 {
        return;
    }
    let (_, first, count) = m.leaf(leaf);
    let mut have = false;
    let mut key = slots.last;
    let mut j = first;
    let end = first + count;
    while j < end {
        let face = m.mark(j);
        if face < MAX_FACES && FACE_KEY[face] & !KEY_MASK != stamp {
            if !have {
                have = true;
                let pair = if slots.pair < SLOT_PAIRS { slots.pair } else { SLOT_PAIRS - 1 };
                key = (FAR_KEY - 2 * pair) as u16;
                slots.last = key;
                slots.used += 1;
                // Once there are more leaves than slots, advance every `estimate / SLOT_PAIRS`.
                slots.acc += if slots.estimate < SLOT_PAIRS { slots.estimate } else { SLOT_PAIRS };
                if slots.acc >= slots.estimate {
                    slots.acc -= slots.estimate;
                    slots.pair += 1;
                }
            }
            FACE_KEY[face] = stamp | key;
        }
        j += 1;
    }
    LEAF_SLOT[leaf] = key;
}

/// Key range an entity in the leaf holding it may use: the odd bucket just nearer than the leaf's
/// slot. `None` when the sequence did not visit that leaf.
#[inline(always)]
pub unsafe fn gap_of_leaf(leaf: i32) -> Option<(i32, i32)> {
    if !READY || leaf < 1 || leaf as usize > MAX_LEAVES {
        return None;
    }
    let key = LEAF_SLOT[leaf as usize] as i32;
    if key == 0 {
        return None;
    }
    Some((key - 1, key - 1))
}

/// Table key of `face` in the current sequence, or 0 when it has none (no sequence, or the face
/// is not marked by a visible leaf).
#[inline(always)]
pub unsafe fn face_key(face: usize) -> i32 {
    if !READY || face >= MAX_FACES {
        return 0;
    }
    let k = FACE_KEY[face];
    if k & !KEY_MASK == STAMP << STAMP_SHIFT {
        (k & KEY_MASK) as i32
    } else {
        0
    }
}

/// Whether a sequence is in force this frame.
#[inline(always)]
pub unsafe fn ready() -> bool {
    READY
}
