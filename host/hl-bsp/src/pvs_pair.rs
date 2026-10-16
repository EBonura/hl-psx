//! Lossless HLMJ world-leaf PVS encoding. Source BSP visibility remains in its
//! original GoldSrc form for cook-time spawn and world-pipeline decisions.

use hl_format::pvs_pair;

const SOURCE_LEAF_SIZE: usize = 28;
const SOURCE_VISOFS: usize = 4;
const DELTA: u32 = 0x8000_0000;

fn source_row(vis: &[u8], leaves: &[u8], leaf: usize, width: usize) -> Result<Vec<u8>, String> {
    let pos = leaf
        .checked_mul(SOURCE_LEAF_SIZE)
        .and_then(|n| n.checked_add(SOURCE_VISOFS))
        .ok_or_else(|| format!("leaf {leaf} source offset overflow"))?;
    let bytes: [u8; 4] = leaves
        .get(pos..pos + 4)
        .ok_or_else(|| format!("leaf {leaf} source record truncated"))?
        .try_into()
        .unwrap();
    let offset = i32::from_le_bytes(bytes);
    let mut row = vec![0u8; width];
    if offset < 0 {
        row.fill(0xff);
        return Ok(row);
    }
    let mut input = offset as usize;
    let mut output = 0usize;
    while output < width {
        let value = *vis
            .get(input)
            .ok_or_else(|| format!("leaf {leaf} PVS truncated"))?;
        input += 1;
        if value != 0 {
            row[output] = value;
            output += 1;
        } else {
            let count = *vis
                .get(input)
                .ok_or_else(|| format!("leaf {leaf} zero run truncated"))?
                as usize;
            input += 1;
            if count == 0 || count > width - output {
                return Err(format!("leaf {leaf} PVS zero run exceeds row"));
            }
            output += count;
        }
    }
    Ok(row)
}

fn rle(row: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    let mut i = 0usize;
    while i < row.len() {
        if row[i] != 0 {
            encoded.push(row[i]);
            i += 1;
        } else {
            let start = i;
            while i < row.len() && row[i] == 0 && i - start < 255 {
                i += 1;
            }
            encoded.extend_from_slice(&[0, (i - start) as u8]);
        }
    }
    encoded
}

pub(crate) fn encode(
    vis: &[u8],
    leaves: &[u8],
    world_leaves: usize,
) -> Result<(Vec<u8>, Vec<u32>), String> {
    if world_leaves == 0 {
        return Err("world has no PVS leaves".into());
    }
    let width = world_leaves.div_ceil(8);
    let mut rows = Vec::with_capacity(world_leaves);
    for leaf in 1..=world_leaves {
        rows.push(source_row(vis, leaves, leaf, width)?);
    }
    let mut blob = Vec::new();
    let mut offsets = vec![0u32; world_leaves + 1];
    for (i, row) in rows.iter().enumerate() {
        let leaf = i + 1;
        let raw = rle(row);
        let delta = if leaf & 1 == 0 {
            let xor: Vec<_> = row.iter().zip(&rows[i - 1]).map(|(&a, &b)| a ^ b).collect();
            Some(rle(&xor))
        } else {
            None
        };
        let chosen = delta.as_ref().filter(|encoded| encoded.len() < raw.len());
        let offset = u32::try_from(blob.len()).map_err(|_| "PVS offset exceeds u32")?;
        if offset & DELTA != 0 {
            return Err("PVS offset exceeds 31-bit tagged range".into());
        }
        offsets[leaf] = offset | if chosen.is_some() { DELTA } else { 0 };
        blob.extend_from_slice(chosen.unwrap_or(&raw));
    }
    offsets[0] = offsets[1];
    let mut packed_leaves = Vec::with_capacity(offsets.len() * 8);
    for &offset in &offsets {
        packed_leaves.extend_from_slice(&offset.to_le_bytes());
        packed_leaves.extend_from_slice(&0u32.to_le_bytes());
    }
    if !pvs_pair::validate(&blob, &packed_leaves, world_leaves, width) {
        return Err("new PVS failed shared decoder validation".into());
    }
    for (i, expected) in rows.iter().enumerate() {
        let mut actual = vec![0u8; width];
        if !pvs_pair::decode(&blob, &packed_leaves, i + 1, width, &mut actual, false)
            || actual != *expected
        {
            return Err(format!("leaf {} replacement PVS mismatch", i + 1));
        }
        actual.fill(0x55);
        if !pvs_pair::decode(&blob, &packed_leaves, i + 1, width, &mut actual, true)
            || actual
                .iter()
                .zip(expected)
                .any(|(&got, &want)| got != (want | 0x55))
        {
            return Err(format!("leaf {} merged PVS mismatch", i + 1));
        }
    }
    Ok((blob, offsets))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(offsets: &[i32]) -> Vec<u8> {
        let mut out = vec![0u8; offsets.len() * SOURCE_LEAF_SIZE];
        for (leaf, &offset) in offsets.iter().enumerate() {
            out[leaf * SOURCE_LEAF_SIZE + SOURCE_VISOFS..][..4]
                .copy_from_slice(&offset.to_le_bytes());
        }
        out
    }

    #[test]
    fn roundtrip_pair_and_legacy_all_visible_source() {
        let source = [
            0x80, 0x40, 0x22, 0x20, 0x55, 0x77, 0x99, 0x11, 0x80, 0x40, 0x23, 0x20, 0x55, 0x77,
            0x99, 0x11,
        ];
        let mut source_offsets = vec![-1; 58];
        source_offsets[1] = 0;
        source_offsets[2] = 8;
        let (blob, offsets) = encode(&source, &leaves(&source_offsets), 57).unwrap();
        assert_eq!(offsets[0], offsets[1]);
        assert_eq!(offsets[2] & DELTA, DELTA);
        assert!(!blob.is_empty());
    }

    #[test]
    fn malformed_source_run_rejected() {
        assert!(encode(&[0, 0], &leaves(&[-1, 0]), 1).is_err());
        assert!(encode(&[0, 2], &leaves(&[-1, 0]), 1).is_err());
    }
}
