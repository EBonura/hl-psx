//! Bounded, allocation-free decoder for HLMJ world-leaf visibility rows.
//!
//! An odd leaf owns a complete GoldSrc zero-run row. An even leaf may store
//! its XOR against the preceding odd leaf, tagged in bit 31 of its offset.
//! The two cursors stream directly into the caller's existing visibility bits.

use crate::map;

const DELTA: u32 = 0x8000_0000;
const ALL: u32 = u32::MAX;

fn word(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(offset..offset.checked_add(4)?)?.try_into().ok()?))
}

fn leaf_offset(leaves: &[u8], leaf: usize) -> Option<u32> {
    word(leaves, leaf.checked_mul(map::LEAF_RECORD_SIZE)?)
}

struct Row<'a> {
    data: &'a [u8],
    pos: usize,
    zeros: usize,
}

impl<'a> Row<'a> {
    fn new(data: &'a [u8], offset: usize) -> Option<Self> {
        (offset < data.len()).then_some(Self { data, pos: offset, zeros: 0 })
    }

    fn next(&mut self) -> Option<u8> {
        if self.zeros != 0 {
            self.zeros -= 1;
            return Some(0);
        }
        let value = *self.data.get(self.pos)?;
        self.pos += 1;
        if value != 0 {
            return Some(value);
        }
        let run = *self.data.get(self.pos)? as usize;
        self.pos += 1;
        if run == 0 {
            return None;
        }
        self.zeros = run - 1;
        Some(0)
    }
}

/// Decode or OR one world-leaf row. The room must have passed `validate` at
/// load time, so a malformed input cannot partially change live VIS_BITS.
pub fn decode(vis: &[u8], leaves: &[u8], leaf: usize, width: usize, out: &mut [u8], merge: bool) -> bool {
    if leaf == 0 || out.len() < width {
        return false;
    }
    let Some(tagged) = leaf_offset(leaves, leaf) else { return false };
    if tagged == ALL { return false; }
    let delta = tagged & DELTA != 0;
    if delta && leaf & 1 != 0 { return false; }
    let Some(mut current) = Row::new(vis, (tagged & !DELTA) as usize) else { return false };
    let mut anchor = if delta {
        let Some(prior) = leaf_offset(leaves, leaf - 1) else { return false };
        if prior & DELTA != 0 { return false; }
        let Some(row) = Row::new(vis, prior as usize) else { return false };
        Some(row)
    } else { None };
    for value in out.iter_mut().take(width) {
        let Some(mut decoded) = current.next() else { return false };
        if let Some(ref mut row) = anchor {
            let Some(previous) = row.next() else { return false };
            decoded ^= previous;
        }
        if merge { *value |= decoded; } else { *value = decoded; }
    }
    true
}

/// Validate all world rows once, before `Map::load` exposes unchecked fields.
/// Submodel-only leaves retain `-1` and never enter this decoder.
pub fn validate(vis: &[u8], leaves: &[u8], world_leaves: usize, width: usize) -> bool {
    if world_leaves == 0 || width == 0 || leaf_offset(leaves, 0) != leaf_offset(leaves, 1) {
        return false;
    }
    let mut previous_end = 0usize;
    for leaf in 1..=world_leaves {
        let Some(tagged) = leaf_offset(leaves, leaf) else { return false };
        if tagged == ALL || (tagged & DELTA != 0 && leaf & 1 != 0) { return false; }
        let offset = (tagged & !DELTA) as usize;
        if offset != previous_end { return false; }
        let end = if leaf == world_leaves {
            vis.len()
        } else {
            let Some(next) = leaf_offset(leaves, leaf + 1) else { return false };
            (next & !DELTA) as usize
        };
        if end <= offset || end > vis.len() { return false; }
        let Some(mut row) = Row::new(vis, offset) else { return false };
        for _ in 0..width {
            if row.next().is_none() { return false; }
        }
        if row.zeros != 0 || row.pos != end { return false; }
        previous_end = end;
    }
    previous_end == vis.len()
}

fn align4(value: usize) -> Option<usize> {
    Some(value.checked_add(3)? & !3)
}

/// Check BSP/PVS section bounds and every encoded HLMJ row before the normal
/// map reader uses its intentionally unchecked hot-path visibility accessors.
pub fn validate_room(data: &[u8]) -> bool {
    let Some(magic) = word(data, map::HEADER_MAGIC_OFFSET) else { return false };
    if magic != map::MAGIC_HLMJ || data.len() < map::HEADER_SIZE { return false; }
    let (Some(bsp), Some(clip), Some(faces)) = (
        word(data, map::HEADER_BSP_OFFSET),
        word(data, map::HEADER_CLIP_OFFSET),
        word(data, map::HEADER_FACE_COUNT_OFFSET),
    ) else { return false };
    let (bsp, clip, faces) = (bsp as usize, clip as usize, faces as usize);
    if bsp < map::HEADER_SIZE || clip > data.len() || bsp >= clip { return false; }
    let field = |delta| bsp.checked_add(delta).and_then(|at| word(data, at));
    let (Some(planes), Some(groups), Some(nodes), Some(counts), Some(marks), Some(vis_len)) = (
        field(0), field(4), field(8), field(12), field(16), field(20),
    ) else { return false };
    let total = (counts & 0xffff) as usize;
    let world = (counts >> 16) as usize;
    if total < 2 || world == 0 || world >= total { return false; }
    let Some(leaves_off) = (|| {
        let mut at = bsp.checked_add(24)?;
        at = at.checked_add((planes as usize).checked_mul(map::PLANE_RECORD_SIZE)?)?;
        at = at.checked_add((groups as usize).checked_mul(map::FACE_GROUP_RECORD_SIZE)?)?;
        at = at.checked_add(faces.checked_mul(map::FACE_RECORD_SIZE)?)?;
        at.checked_add((nodes as usize).checked_mul(map::NODE_RECORD_SIZE)?)
    })() else { return false };
    let Some(leaves_end) = leaves_off.checked_add(total * map::LEAF_RECORD_SIZE) else { return false };
    let Some(vis_off) = align4(leaves_end).and_then(|at| at.checked_add((marks as usize).checked_mul(2)?)).and_then(align4) else { return false };
    let Some(vis_end) = vis_off.checked_add(vis_len as usize) else { return false };
    if vis_end > clip || align4(vis_end) != Some(clip) { return false; }
    let (Some(leaves), Some(vis)) = (data.get(leaves_off..leaves_end), data.get(vis_off..vis_end)) else { return false };
    for leaf in world + 1..total {
        if leaf_offset(leaves, leaf) != Some(ALL) { return false; }
    }
    validate(vis, leaves, world, world.div_ceil(8))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec::Vec;
    use super::*;

    fn leaves(offsets: &[u32]) -> Vec<u8> {
        let mut out = Vec::new();
        for &offset in offsets { out.extend_from_slice(&offset.to_le_bytes()); out.extend_from_slice(&0u32.to_le_bytes()); }
        out
    }

    #[test]
    fn pair_replace_merge_and_alias() {
        let vis = [0x80, 0, 2, 0x20, 0, 1, 0x40, 0, 2];
        let marks = leaves(&[0, 0, DELTA | 4]);
        assert!(validate(&vis, &marks, 2, 4));
        let mut row = [0; 4];
        assert!(decode(&vis, &marks, 2, 4, &mut row, false));
        assert_eq!(row, [0x80, 0x40, 0, 0x20]);
        let mut merged = [0x11; 4];
        assert!(decode(&vis, &marks, 2, 4, &mut merged, true));
        assert_eq!(merged, [0x91, 0x51, 0x11, 0x31]);
        assert!(decode(&vis, &marks, 1, 4, &mut row, false));
        assert_eq!(row, [0x80, 0, 0, 0x20]);
    }

    #[test]
    fn malformed_rows_fail_validation() {
        let vis = [0x80, 0, 2, 0x20, 0, 1, 0x40, 0, 2];
        let good = [0, 0, DELTA | 4];
        for offsets in [[0, 0, ALL], [0, 0, DELTA | 3], [0, DELTA, DELTA | 4], [4, 0, DELTA | 4]] {
            assert!(!validate(&vis, &leaves(&offsets), 2, 4));
        }
        let mut bad = vis;
        bad[5] = 5;
        assert!(!validate(&bad, &leaves(&good), 2, 4));
        bad = vis;
        bad[5] = 0;
        assert!(!validate(&bad, &leaves(&good), 2, 4));
        assert!(!validate(&vis[..7], &leaves(&good), 2, 4));
    }
}
