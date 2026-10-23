//! The pieces of a GoldSrc BSP the ordering lab needs: planes, nodes with their faces, models.

use std::fs;
use std::path::Path;

pub struct Plane {
    pub normal: [f32; 3],
    pub dist: f32,
}

pub struct Node {
    pub plane: usize,
    /// Child index: `>= 0` a node, negative `-1 - leaf` a leaf.
    pub children: [i32; 2],
    pub first_face: u32,
    pub num_faces: u32,
}

pub struct Model {
    pub head_node: i32,
    pub first_face: u32,
    pub num_faces: u32,
}

pub struct Bsp {
    pub planes: Vec<Plane>,
    pub nodes: Vec<Node>,
    pub models: Vec<Model>,
    /// Node owning each face (`u32::MAX` when no node lists it).
    pub node_of_face: Vec<u32>,
}

fn lump(data: &[u8], index: usize) -> Result<&[u8], String> {
    let word = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
    let (offset, length) = (word(4 + 8 * index), word(8 + 8 * index));
    data.get(offset..offset + length)
        .ok_or_else(|| format!("lump {index} out of range"))
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn f32le(b: &[u8], o: usize) -> f32 {
    f32::from_bits(le32(b, o))
}

impl Bsp {
    pub fn load(path: &Path) -> Result<Self, String> {
        let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let planes = lump(&data, 1)?
            .chunks_exact(20)
            .map(|p| Plane {
                normal: [f32le(p, 0), f32le(p, 4), f32le(p, 8)],
                dist: f32le(p, 12),
            })
            .collect();
        let nodes: Vec<Node> = lump(&data, 5)?
            .chunks_exact(24)
            .map(|n| Node {
                plane: le32(n, 0) as usize,
                children: [i32::from(le16(n, 4) as i16), i32::from(le16(n, 6) as i16)],
                first_face: u32::from(le16(n, 20)),
                num_faces: u32::from(le16(n, 22)),
            })
            .collect();
        let models: Vec<Model> = lump(&data, 14)?
            .chunks_exact(64)
            .map(|m| Model {
                head_node: le32(m, 36) as i32,
                first_face: le32(m, 56),
                num_faces: le32(m, 60),
            })
            .collect();
        let face_count = lump(&data, 7)?.len() / 20;
        let mut node_of_face = vec![u32::MAX; face_count];
        for (i, node) in nodes.iter().enumerate() {
            for f in node.first_face..node.first_face + node.num_faces {
                if let Some(slot) = node_of_face.get_mut(f as usize) {
                    *slot = i as u32;
                }
            }
        }
        Ok(Self {
            planes,
            nodes,
            models,
            node_of_face,
        })
    }

    /// Signed distance of `p` from a node's plane (positive on the front).
    pub fn side(&self, node: usize, p: [f32; 3]) -> f32 {
        let plane = &self.planes[self.nodes[node].plane];
        plane.normal[0] * p[0] + plane.normal[1] * p[1] + plane.normal[2] * p[2] - plane.dist
    }

    /// Rank of every node in a back-to-front walk of `model` from `eye`: far subtree, the node's
    /// own faces, near subtree. Nodes never reached keep `u32::MAX`.
    pub fn back_to_front_ranks(&self, model: usize, eye: [f32; 3]) -> Vec<u32> {
        let mut rank = vec![u32::MAX; self.nodes.len()];
        let mut next = 0u32;
        // Explicit stack of (node, state): state 0 = enter, 1 = after far child.
        let mut stack: Vec<(i32, u8)> = vec![(self.models[model].head_node, 0)];
        while let Some((node, state)) = stack.pop() {
            if node < 0 {
                continue;
            }
            let n = node as usize;
            let front = self.side(n, eye) >= 0.0;
            let (near, far) = if front { (0, 1) } else { (1, 0) };
            if state == 0 {
                stack.push((node, 1));
                stack.push((self.nodes[n].children[far], 0));
            } else {
                rank[n] = next;
                next += 1;
                stack.push((self.nodes[n].children[near], 0));
            }
        }
        rank
    }
}
