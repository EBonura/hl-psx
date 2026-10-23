//! Census of how many world faces and leaves one camera leaf can see (cook-time PVS bound).

use std::fs;
use std::path::Path;

fn le32(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

fn le16(b: &[u8], o: usize) -> usize {
    usize::from(u16::from_le_bytes([b[o], b[o + 1]]))
}

fn lump(d: &[u8], i: usize) -> &[u8] {
    &d[le32(d, 4 + 8 * i)..le32(d, 4 + 8 * i) + le32(d, 8 + 8 * i)]
}

/// (visible leaves, visible faces, visible nodes with a visible leaf below) maxima over camera leaves.
pub fn bound(bsp: &Path) -> Result<(usize, usize, usize, usize, usize), String> {
    let d = fs::read(bsp).map_err(|e| format!("{}: {e}", bsp.display()))?;
    let vis = lump(&d, 4);
    let leaves = lump(&d, 10);
    let marks = lump(&d, 11);
    let nodes = lump(&d, 5);
    let models = lump(&d, 14);
    let world_leaves = le32(models, 48) + 1; // dmodel_t.visleafs + the solid leaf 0
    let (world_first, world_num) = (le32(models, 56), le32(models, 60));
    let n_leaves = leaves.len() / 28;
    let n_nodes = nodes.len() / 24;
    let mut max_leaves = 0;
    let mut max_faces = 0;
    let mut max_nodes = 0;
    let mut seen = vec![0u32; le32(&d, 8 + 8 * 7) / 20 + 1];
    let mut stamp = 0u32;
    for cam in 1..world_leaves.min(n_leaves) {
        let visofs = i32::from_le_bytes([
            leaves[cam * 28 + 4],
            leaves[cam * 28 + 5],
            leaves[cam * 28 + 6],
            leaves[cam * 28 + 7],
        ]);
        let mut row = vec![0u8; world_leaves.div_ceil(8)];
        if visofs < 0 {
            row.fill(0xff);
        } else {
            let mut p = visofs as usize;
            let mut o = 0;
            while o < row.len() && p < vis.len() {
                if vis[p] != 0 {
                    row[o] = vis[p];
                    o += 1;
                    p += 1;
                } else {
                    o += vis[p + 1] as usize;
                    p += 2;
                }
            }
        }
        stamp += 1;
        let mut faces = 0;
        let mut vleaves = 0;
        let mut leaf_vis = vec![false; n_leaves];
        for l in 1..world_leaves.min(n_leaves) {
            // vis bit l-1
            if row[(l - 1) >> 3] & (1 << ((l - 1) & 7)) == 0 && l != cam {
                continue;
            }
            vleaves += 1;
            leaf_vis[l] = true;
            let first = le16(leaves, l * 28 + 20);
            let num = le16(leaves, l * 28 + 22);
            for m in first..first + num {
                let f = le16(marks, m * 2);
                if f >= world_first && f < world_first + world_num && seen[f] != stamp {
                    seen[f] = stamp;
                    faces += 1;
                }
            }
        }
        // nodes with a visible leaf below: bottom-up in reverse index order
        let mut nv = vec![false; n_nodes];
        for n in (0..n_nodes).rev() {
            let mut any = false;
            for c in 0..2 {
                let ch = i16::from_le_bytes([nodes[n * 24 + 4 + c * 2], nodes[n * 24 + 5 + c * 2]]);
                any |= if ch < 0 {
                    leaf_vis
                        .get((-1 - i32::from(ch)) as usize)
                        .copied()
                        .unwrap_or(false)
                } else {
                    nv[ch as usize]
                };
            }
            nv[n] = any;
        }
        let vnodes = nv.iter().filter(|b| **b).count();
        max_leaves = max_leaves.max(vleaves);
        max_faces = max_faces.max(faces);
        max_nodes = max_nodes.max(vnodes);
    }
    Ok((max_leaves, max_faces, max_nodes, n_nodes, n_leaves))
}
