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
}

pub struct Bsp {
    pub planes: Vec<Plane>,
    pub nodes: Vec<Node>,
    pub models: Vec<Model>,
    /// Node owning each face (`u32::MAX` when no node lists it).
    pub node_of_face: Vec<u32>,
    /// Polygon of every face.
    pub faces: Vec<Vec<[f32; 3]>>,
    /// (plane number, back side) of every face.
    pub face_plane: Vec<(usize, bool)>,
    /// Marked faces of every leaf.
    pub leaf_faces: Vec<Vec<u32>>,
    /// Leaves that mark each face.
    pub face_leaves: Vec<Vec<u32>>,
}

fn lump(data: &[u8], index: usize) -> Result<&[u8], String> {
    let word =
        |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
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
        let verts: Vec<[f32; 3]> = lump(&data, 3)?
            .chunks_exact(12)
            .map(|v| [f32le(v, 0), f32le(v, 4), f32le(v, 8)])
            .collect();
        let edges: Vec<[usize; 2]> = lump(&data, 12)?
            .chunks_exact(4)
            .map(|e| [usize::from(le16(e, 0)), usize::from(le16(e, 2))])
            .collect();
        let surf: Vec<i32> = lump(&data, 13)?
            .chunks_exact(4)
            .map(|e| le32(e, 0) as i32)
            .collect();
        let faces = lump(&data, 7)?
            .chunks_exact(20)
            .map(|f| {
                let first = le32(f, 4) as usize;
                let num = usize::from(le16(f, 8));
                (first..first + num)
                    .map(|i| {
                        let se = surf[i];
                        let e = edges[se.unsigned_abs() as usize];
                        verts[if se >= 0 { e[0] } else { e[1] }]
                    })
                    .collect()
            })
            .collect();
        let face_plane = lump(&data, 7)?
            .chunks_exact(20)
            .map(|f| (usize::from(le16(f, 0)), le16(f, 2) != 0))
            .collect();
        let marks: Vec<u32> = lump(&data, 11)?
            .chunks_exact(2)
            .map(|m| u32::from(le16(m, 0)))
            .collect();
        let leaf_faces = lump(&data, 10)?
            .chunks_exact(28)
            .map(|l| {
                let first = usize::from(le16(l, 20));
                let num = usize::from(le16(l, 22));
                marks[first..(first + num).min(marks.len())].to_vec()
            })
            .collect();
        let mut face_leaves: Vec<Vec<u32>> = vec![Vec::new(); face_count];
        let leaf_faces_ref: &Vec<Vec<u32>> = &leaf_faces;
        for (leaf, fs) in leaf_faces_ref.iter().enumerate() {
            for f in fs {
                if let Some(v) = face_leaves.get_mut(*f as usize) {
                    v.push(leaf as u32);
                }
            }
        }
        Ok(Self {
            face_leaves,
            leaf_faces,
            face_plane,
            faces,
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

impl Bsp {
    /// The plane a face lies on in the game's (cooked) axes, as (normal, distance) with the
    /// normal facing out of the face, translated by a brush draw offset (cooked units).
    pub fn face_plane_cooked(&self, face: usize, offset: [i32; 3]) -> Option<([f64; 3], f64)> {
        let &(plane, back) = self.face_plane.get(face)?;
        let plane = self.planes.get(plane)?;
        let sign = if back { -1.0 } else { 1.0 };
        // Cooked axes are (x, z, y) of the BSP's.
        let n = [
            sign * f64::from(plane.normal[0]),
            sign * f64::from(plane.normal[2]),
            sign * f64::from(plane.normal[1]),
        ];
        let o = [
            f64::from(offset[0]),
            f64::from(offset[1]),
            f64::from(offset[2]),
        ];
        let d = sign * f64::from(plane.dist) + n[0] * o[0] + n[1] * o[1] + n[2] * o[2];
        Some((n, d))
    }
}

impl Bsp {
    /// Rank of every leaf in the same back-to-front walk of `model` from `eye`.
    pub fn leaf_ranks(&self, model: usize, eye: [f32; 3]) -> Vec<u32> {
        let mut rank = vec![u32::MAX; self.leaf_faces.len()];
        let mut next = 0u32;
        let mut stack: Vec<(i32, u8)> = vec![(self.models[model].head_node, 0)];
        while let Some((node, state)) = stack.pop() {
            if node < 0 {
                let leaf = (-1 - node) as usize;
                if leaf < rank.len() {
                    rank[leaf] = next;
                    next += 1;
                }
                continue;
            }
            let n = node as usize;
            let front = self.side(n, eye) >= 0.0;
            let (near, far) = if front { (0, 1) } else { (1, 0) };
            if state == 0 {
                stack.push((node, 1));
                stack.push((self.nodes[n].children[far], 0));
            } else {
                stack.push((self.nodes[n].children[near], 0));
            }
        }
        rank
    }
}

impl Bsp {
    /// Leaf containing `p` in the world tree.
    pub fn point_leaf(&self, p: [f32; 3]) -> usize {
        let mut node = self.models[0].head_node;
        while node >= 0 {
            let n = node as usize;
            node = self.nodes[n].children[usize::from(self.side(n, p) < 0.0)];
        }
        (-1 - node) as usize
    }
}

impl Bsp {
    /// Outward normal of a face in the BSP's own axes.
    pub fn face_normal(&self, face: usize) -> [f32; 3] {
        let Some(&(plane, back)) = self.face_plane.get(face) else {
            return [0.0; 3];
        };
        let n = self.planes[plane].normal;
        if back {
            [-n[0], -n[1], -n[2]]
        } else {
            n
        }
    }
}
