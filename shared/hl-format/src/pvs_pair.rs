//! Bounded, allocation-free decoder for HLMJ/HLMK world-leaf visibility rows.
//!
//! Every world leaf owns one GoldSrc zero-run stream. An anchor row stores the
//! complete visibility row. A delta row stores only the XOR against an earlier
//! leaf's decoded row, named by a backwards distance in bits 24..30 of the leaf's
//! tagged offset (bit 31 marks a delta row). Chains of deltas end at an anchor
//! and are at most `MAX_DELTAS` long, so a decode is one bounded fill, one
//! stream walk for the anchor and a sparse XOR walk for each delta.

use crate::map;

const DELTA: u32 = 0x8000_0000;
const DISTANCE_SHIFT: u32 = 24;
const DISTANCE_MASK: u32 = 0x7f;
const OFFSET_MASK: u32 = 0x00ff_ffff;
const ALL: u32 = u32::MAX;

/// Longest chain of delta rows above an anchor.
pub const MAX_DELTAS: usize = 8;
/// Farthest earlier leaf a delta row may name.
pub const MAX_DISTANCE: usize = DISTANCE_MASK as usize;
/// Widest row, in bytes, the decoder handles: 4,096 world leaves.
pub const MAX_WIDTH: usize = 512;

/// Tagged leaf offset of an anchor row starting at `offset` in the stream blob.
pub const fn anchor_tag(offset: u32) -> u32 {
    offset
}

/// Tagged leaf offset of a delta row at `offset` against the leaf `distance`
/// positions earlier.
pub const fn delta_tag(offset: u32, distance: u32) -> u32 {
    DELTA | (distance << DISTANCE_SHIFT) | offset
}

/// Largest stream offset a tag can carry.
pub const MAX_OFFSET: usize = OFFSET_MASK as usize;

fn word(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn leaf_offset(leaves: &[u8], leaf: usize) -> Option<u32> {
    word(leaves, leaf.checked_mul(map::LEAF_RECORD_SIZE)?)
}

/// Walk one zero-run stream covering exactly `width` bytes, calling `put` for each
/// nonzero byte. Returns the stream position after the row, or `None` on a
/// malformed or overlong stream.
#[inline(always)]
fn walk(
    data: &[u8],
    mut pos: usize,
    width: usize,
    mut put: impl FnMut(usize, u8),
) -> Option<usize> {
    let mut out = 0usize;
    while out < width {
        let value = *data.get(pos)?;
        pos += 1;
        if value != 0 {
            put(out, value);
            out += 1;
        } else {
            let run = *data.get(pos)? as usize;
            pos += 1;
            if run == 0 || run > width - out {
                return None;
            }
            out += run;
        }
    }
    Some(pos)
}

/// Decode or OR one world-leaf row. The room must have passed `validate` at
/// load time, so a malformed input cannot partially change live VIS_BITS.
pub fn decode(
    vis: &[u8],
    leaves: &[u8],
    leaf: usize,
    width: usize,
    out: &mut [u8],
    merge: bool,
) -> bool {
    if leaf == 0 || width > MAX_WIDTH || out.len() < width {
        return false;
    }
    // Chain from the requested row down to its anchor.
    let mut starts = [0usize; MAX_DELTAS + 1];
    let mut count = 0usize;
    let mut at = leaf;
    loop {
        let Some(tagged) = leaf_offset(leaves, at) else {
            return false;
        };
        if tagged == ALL || count > MAX_DELTAS {
            return false;
        }
        starts[count] = (tagged & OFFSET_MASK) as usize;
        count += 1;
        if tagged & DELTA == 0 {
            break;
        }
        let distance = ((tagged >> DISTANCE_SHIFT) & DISTANCE_MASK) as usize;
        if distance == 0 || distance >= at {
            return false;
        }
        at -= distance;
    }
    let mut scratch = [0u8; MAX_WIDTH];
    let row: &mut [u8] = if merge {
        &mut scratch[..width]
    } else {
        &mut out[..width]
    };
    row.fill(0);
    let anchor = starts[count - 1];
    if walk(vis, anchor, width, |i, v| row[i] = v).is_none() {
        return false;
    }
    for &start in starts[..count - 1].iter().rev() {
        if walk(vis, start, width, |i, v| row[i] ^= v).is_none() {
            return false;
        }
    }
    if merge {
        for (o, &v) in out[..width].iter_mut().zip(&scratch[..width]) {
            *o |= v;
        }
    }
    true
}

/// Validate all world rows once, before `Map::load` exposes unchecked fields.
/// Submodel-only leaves retain `-1` and never enter this decoder.
pub fn validate(vis: &[u8], leaves: &[u8], world_leaves: usize, width: usize) -> bool {
    if world_leaves == 0
        || width == 0
        || width > MAX_WIDTH
        || leaf_offset(leaves, 0) != leaf_offset(leaves, 1)
    {
        return false;
    }
    let mut previous_end = 0usize;
    for leaf in 1..=world_leaves {
        let Some(tagged) = leaf_offset(leaves, leaf) else {
            return false;
        };
        if tagged == ALL {
            return false;
        }
        let offset = (tagged & OFFSET_MASK) as usize;
        if offset != previous_end {
            return false;
        }
        if tagged & DELTA != 0 {
            // The chain must reach an anchor within MAX_DELTAS steps, backwards.
            let mut at = leaf;
            let mut tag = tagged;
            let mut steps = 0usize;
            while tag & DELTA != 0 {
                let distance = ((tag >> DISTANCE_SHIFT) & DISTANCE_MASK) as usize;
                steps += 1;
                if distance == 0 || distance >= at || steps > MAX_DELTAS {
                    return false;
                }
                at -= distance;
                let Some(next) = leaf_offset(leaves, at) else {
                    return false;
                };
                if next == ALL {
                    return false;
                }
                tag = next;
            }
        } else if tagged & !OFFSET_MASK != 0 {
            return false;
        }
        let end = if leaf == world_leaves {
            vis.len()
        } else {
            let Some(next) = leaf_offset(leaves, leaf + 1) else {
                return false;
            };
            (next & OFFSET_MASK) as usize
        };
        if end <= offset || end > vis.len() {
            return false;
        }
        match walk(vis, offset, width, |_, _| {}) {
            Some(pos) if pos == end => {}
            _ => return false,
        }
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
    let Some(magic) = word(data, map::HEADER_MAGIC_OFFSET) else {
        return false;
    };
    if !matches!(magic, map::MAGIC_HLMJ | map::MAGIC_HLMK) || data.len() < map::HEADER_SIZE {
        return false;
    }
    let (Some(bsp), Some(clip), Some(faces)) = (
        word(data, map::HEADER_BSP_OFFSET),
        word(data, map::HEADER_CLIP_OFFSET),
        word(data, map::HEADER_FACE_COUNT_OFFSET),
    ) else {
        return false;
    };
    let (bsp, clip, faces) = (bsp as usize, clip as usize, faces as usize);
    if bsp < map::HEADER_SIZE || clip > data.len() || bsp >= clip {
        return false;
    }
    let field = |delta| bsp.checked_add(delta).and_then(|at| word(data, at));
    let (Some(planes), Some(groups), Some(nodes), Some(counts), Some(marks), Some(vis_len)) = (
        field(0),
        field(4),
        field(8),
        field(12),
        field(16),
        field(20),
    ) else {
        return false;
    };
    let total = (counts & 0xffff) as usize;
    let world = (counts >> 16) as usize;
    if total < 2 || world == 0 || world >= total {
        return false;
    }
    let Some(leaves_off) = (|| {
        let mut at = bsp.checked_add(24)?;
        at = at.checked_add((planes as usize).checked_mul(map::PLANE_RECORD_SIZE)?)?;
        at = at.checked_add((groups as usize).checked_mul(map::FACE_GROUP_RECORD_SIZE)?)?;
        at = at.checked_add(faces.checked_mul(map::FACE_RECORD_SIZE)?)?;
        at.checked_add((nodes as usize).checked_mul(map::NODE_RECORD_SIZE)?)
    })() else {
        return false;
    };
    let Some(leaves_end) = leaves_off.checked_add(total * map::LEAF_RECORD_SIZE) else {
        return false;
    };
    let Some(vis_off) = align4(leaves_end)
        .and_then(|at| at.checked_add((marks as usize).checked_mul(2)?))
        .and_then(align4)
    else {
        return false;
    };
    let Some(vis_end) = vis_off.checked_add(vis_len as usize) else {
        return false;
    };
    if vis_end > clip || align4(vis_end) != Some(clip) {
        return false;
    }
    let (Some(leaves), Some(vis)) = (data.get(leaves_off..leaves_end), data.get(vis_off..vis_end))
    else {
        return false;
    };
    for leaf in world + 1..total {
        if leaf_offset(leaves, leaf) != Some(ALL) {
            return false;
        }
    }
    validate(vis, leaves, world, world.div_ceil(8))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn leaves(offsets: &[u32]) -> Vec<u8> {
        let mut out = Vec::new();
        for &offset in offsets {
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out
    }

    // Rows (width 4): leaf 1 = 80 00 00 20, leaf 2 = 80 40 00 20 (delta 1: byte 1 ^= 40),
    // leaf 3 = 80 40 00 60 (delta against leaf 2: byte 3 ^= 40).
    fn sample() -> (Vec<u8>, Vec<u8>) {
        let vis = std::vec![0x80, 0, 2, 0x20, 0, 1, 0x40, 0, 2, 0, 3, 0x40];
        let table = leaves(&[0, 0, delta_tag(4, 1), delta_tag(9, 1)]);
        (vis, table)
    }

    #[test]
    fn chain_replace_merge_and_alias() {
        let (vis, table) = sample();
        assert!(validate(&vis, &table, 3, 4));
        let mut row = [0; 4];
        assert!(decode(&vis, &table, 1, 4, &mut row, false));
        assert_eq!(row, [0x80, 0, 0, 0x20]);
        assert!(decode(&vis, &table, 2, 4, &mut row, false));
        assert_eq!(row, [0x80, 0x40, 0, 0x20]);
        assert!(decode(&vis, &table, 3, 4, &mut row, false));
        assert_eq!(row, [0x80, 0x40, 0, 0x60]);
        let mut merged = [0x11; 4];
        assert!(decode(&vis, &table, 3, 4, &mut merged, true));
        assert_eq!(merged, [0x91, 0x51, 0x11, 0x71]);
    }

    #[test]
    fn malformed_rows_fail_validation() {
        let (vis, good) = sample();
        let bad_tables = [
            leaves(&[0, 0, delta_tag(4, 1), ALL]),
            leaves(&[0, 0, delta_tag(4, 0), delta_tag(9, 1)]),
            leaves(&[0, 0, delta_tag(4, 2), delta_tag(9, 1)]),
            leaves(&[0, DELTA, delta_tag(4, 1), delta_tag(9, 1)]),
            leaves(&[4, 4, delta_tag(4, 1), delta_tag(9, 1)]),
        ];
        assert!(validate(&vis, &good, 3, 4));
        for table in &bad_tables {
            assert!(!validate(&vis, table, 3, 4));
        }
        let mut bad = vis.clone();
        bad[5] = 5;
        assert!(!validate(&bad, &good, 3, 4));
        bad = vis.clone();
        bad[5] = 0;
        assert!(!validate(&bad, &good, 3, 4));
        assert!(!validate(&vis[..11], &good, 3, 4));
    }

    #[test]
    fn chains_longer_than_the_bound_are_refused() {
        // 10 leaves of width 1: anchor then nine one-byte deltas, each a run-free literal.
        let mut vis = std::vec![0x01u8];
        let mut table = std::vec![0u32, 0];
        for k in 0..9u32 {
            table.push(delta_tag(vis.len() as u32, 1));
            let _ = k;
            vis.push(0x01);
        }
        assert!(!validate(&vis, &leaves(&table), 10, 1));
        let mut row = [0u8; 1];
        assert!(!decode(&vis, &leaves(&table), 10, 1, &mut row, false));
        assert!(decode(&vis, &leaves(&table), 9, 1, &mut row, false));
        assert_eq!(
            row,
            [0x01 ^ 0x01 ^ 0x01 ^ 0x01 ^ 0x01 ^ 0x01 ^ 0x01 ^ 0x01 ^ 0x01]
        );
    }
}
