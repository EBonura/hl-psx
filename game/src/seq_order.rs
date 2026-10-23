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
/// Buckets in the world table.
const OT_BUCKETS: u32 = FAR_KEY + 1;
/// Widest and narrowest slot pitch in buckets. A slot's world faces key at its bucket `K`; `K + 1`
/// is the far-biased bucket (a parent behind its children), `K - 1` the cutout bucket, and the
/// buckets `K - pitch + 2 ..= K - 2` form the window objects standing in that leaf draw in.
const MAX_PITCH: u32 = 12;
const MIN_PITCH: u32 = 4;
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
/// Buckets between two slots in the current sequence.
static mut PITCH: u32 = MIN_PITCH;

/// Mark the nodes that have a visible leaf below them. Cold: run once per visibility rebuild.
///
/// # Safety
/// Single-threaded renderer state; `visibility` must be the row the face lists were built from.
pub unsafe fn prepare(m: &Map, visibility: &[u8]) {
    let words = core::ptr::addr_of_mut!(NODE_VIS).cast::<u32>();
    let n_nodes = m.n_nodes.min(MAX_NODES);
    let n_vis = m.n_visleaves.min(MAX_LEAVES);
    // Bits past the last node stay clear; clear only the words in use.
    let used = n_nodes.div_ceil(32);
    let mut w = 0;
    while w < used {
        *words.add(w) = 0;
        w += 1;
    }
    let row = visibility.as_ptr();
    let mut visible = 0u32;
    let mut i = 0;
    while i < n_vis {
        visible += u32::from(*row.add(i >> 3) >> (i & 7) & 1);
        i += 1;
    }
    VISIBLE_LEAVES = visible;
    // Children always have larger indices than their parent, so one backward pass suffices.
    let mut n = n_nodes;
    while n > 0 {
        n -= 1;
        let (c0, c1) = m.node_children(n);
        let mut any = false;
        let mut k = 0;
        while k < 2 {
            let child = if k == 0 { c0 } else { c1 };
            any |= if child < 0 {
                let leaf = (-child - 1) as usize;
                leaf >= 1 && leaf <= n_vis && (*row.add((leaf - 1) >> 3) >> ((leaf - 1) & 7)) & 1 != 0
            } else {
                *words.add((child as usize) >> 5) & (1 << (child as usize & 31)) != 0
            };
            k += 1;
        }
        if any {
            *words.add(n >> 5) |= 1 << (n & 31);
        }
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
    let n_vis = m.n_visleaves.min(MAX_LEAVES);
    // Spread the slots as wide as the frame's leaf count allows: the room between two slots is
    // what objects standing in a leaf order themselves in. A leaf that adds faces needs a slot,
    // and no more of them do than there are visible leaves.
    let pitch = (OT_BUCKETS / (visible + 8)).clamp(MIN_PITCH, MAX_PITCH);
    PITCH = pitch;
    // The first pass spaces the slots by the leaf count of the previous frame. When this frame
    // needs more slots than there are, the nearest leaves would pile into the last one, so the
    // pass is run again with the exact count.
    let mut estimate = LAST_USED.max(1);
    let mut pass = 0;
    loop {
        STAMP = (STAMP + 1) & 0x3f;
        if STAMP == 0 {
            STAMP = 1;
        }
        let stamp = STAMP << STAMP_SHIFT;
        let mut slots = Slots {
            estimate,
            acc: 0,
            pair: 0,
            count: FAR_KEY / pitch - 1,
            pitch,
            last: FAR_KEY as u16,
            used: 0,
            over: false,
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
        if slots.over && pass == 0 {
            pass = 1;
            estimate = slots.used.max(1);
            continue;
        }
        break;
    }
    READY = true;
}

/// Slot allocation state of one pass.
struct Slots {
    /// Leaves that took a slot in the previous pass: sizes this pass's spacing.
    estimate: u32,
    acc: u32,
    pair: u32,
    /// Slots available at this pitch, and the pitch.
    count: u32,
    pitch: u32,
    /// Key of the most recently allocated slot, shared by leaves that add no face.
    last: u16,
    used: u32,
    /// Whether the pass wanted a slot past the last one.
    over: bool,
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
                let pair = if slots.pair < slots.count {
                    slots.pair
                } else {
                    slots.over = true;
                    slots.count - 1
                };
                key = (FAR_KEY - slots.pitch * pair) as u16;
                slots.last = key;
                slots.used += 1;
                // Once there are more leaves than slots, advance every `estimate / count`.
                slots.acc += if slots.estimate < slots.count { slots.estimate } else { slots.count };
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

/// The buckets objects standing in the leaf whose slot is `key` draw in: after the slot's world
/// and cutout buckets, before the next nearer slot's far-biased bucket.
#[inline(always)]
unsafe fn window(key: i32) -> (i32, i32) {
    let hi = key - 2;
    let lo = (key - PITCH as i32 + 2).min(hi);
    (lo.max(1), hi.max(1))
}

/// Gap of the nearest visited leaf among `leaves`: an object spanning several leaves draws after
/// the world faces of every leaf it touches. `None` when none was visited.
#[inline(always)]
pub unsafe fn gap_nearest(leaves: &[i32]) -> Option<(i32, i32)> {
    if !READY {
        return None;
    }
    let mut best = 0i32;
    for &leaf in leaves {
        if leaf >= 1 && leaf as usize <= MAX_LEAVES {
            let key = LEAF_SLOT[leaf as usize] as i32;
            if key != 0 && (best == 0 || key < best) {
                best = key;
            }
        }
    }
    (best != 0).then(|| window(best))
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
    Some(window(key))
}

/// Key range of an object at `point`: the slot of the visited leaf the point is in, or when that
/// leaf was not visited (the point is inside rock, or in a leaf the PVS hides) the visited leaf
/// nearest to it in the tree: at each node the descent follows the point's side while that side
/// still has a visible leaf below it, else the other side. `None` when nothing is visible.
#[inline(always)]
pub unsafe fn anchor_gap(
    m: &Map,
    visibility: &[u8],
    point: [i32; 3],
    dot: fn([i16; 3], [i32; 3]) -> i32,
) -> Option<(i32, i32)> {
    if !READY {
        return None;
    }
    let n_vis = m.n_visleaves.min(MAX_LEAVES);
    let mut item = 0i32;
    let mut guard = 0;
    loop {
        if item < 0 {
            let leaf = (-item - 1) as usize;
            if leaf >= 1 && leaf <= n_vis {
                let key = LEAF_SLOT[leaf] as i32;
                if key != 0 {
                    return Some(window(key));
                }
            }
            return None;
        }
        guard += 1;
        if guard > 256 {
            return None;
        }
        let n = item as usize;
        if n >= m.n_nodes.min(MAX_NODES) {
            return None;
        }
        let nd = m.node(n);
        let (own, other) = if dot(nd.n, point) - nd.dist_q5 >= 0 {
            (nd.c0, nd.c1)
        } else {
            (nd.c1, nd.c0)
        };
        item = if child_visible(visibility, n_vis, own) { own } else { other };
    }
}

#[inline(always)]
unsafe fn child_visible(visibility: &[u8], n_vis: usize, child: i32) -> bool {
    if child < 0 {
        let leaf = (-child - 1) as usize;
        leaf >= 1 && leaf <= n_vis && visibility[(leaf - 1) >> 3] & (1 << ((leaf - 1) & 7)) != 0
    } else {
        node_visible(child as usize)
    }
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
