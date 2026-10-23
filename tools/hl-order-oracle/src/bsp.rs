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
    pub center: [f32; 3],
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
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
    /// Polygon of every face.
    pub faces: Vec<Vec<[f32; 3]>>,
    /// (plane number, back side) of every face.
    pub face_plane: Vec<(usize, bool)>,
    /// Marked faces of every leaf.
    pub leaf_faces: Vec<Vec<u32>>,
    /// Leaves that mark each face.
    pub face_leaves: Vec<Vec<u32>>,
    /// Bounding box centre of every leaf (BSP axes).
    pub leaf_center: Vec<[f32; 3]>,
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
                mins: [f32le(m, 0), f32le(m, 4), f32le(m, 8)],
                maxs: [f32le(m, 12), f32le(m, 16), f32le(m, 20)],
                center: [
                    (f32le(m, 0) + f32le(m, 12)) / 2.0,
                    (f32le(m, 4) + f32le(m, 16)) / 2.0,
                    (f32le(m, 8) + f32le(m, 20)) / 2.0,
                ],
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
        let leaf_center = lump(&data, 10)?
            .chunks_exact(28)
            .map(|l| {
                let g = |o: usize| f32::from(le16(l, o) as i16);
                [
                    (g(8) + g(14)) / 2.0,
                    (g(10) + g(16)) / 2.0,
                    (g(12) + g(18)) / 2.0,
                ]
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
            leaf_center,
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
    /// Signed distance of a point from a face's own node plane.
    pub fn face_side(&self, face: usize, p: [f32; 3]) -> f32 {
        let node = self.node_of_face[face];
        if node == u32::MAX {
            return 0.0;
        }
        self.side(node as usize, p)
    }

    /// (min, max) signed distance of a face polygon from another face's node plane.
    pub fn span_against(&self, plane_of: usize, face: usize) -> (f32, f32) {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for v in &self.faces[face] {
            let d = self.face_side(plane_of, *v);
            lo = lo.min(d);
            hi = hi.max(d);
        }
        (lo, hi)
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
    /// Every leaf of the world tree the box touches.
    pub fn box_leaves(&self, mins: [f32; 3], maxs: [f32; 3]) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![self.models[0].head_node];
        while let Some(node) = stack.pop() {
            if node < 0 {
                out.push((-1 - node) as usize);
                continue;
            }
            let n = &self.nodes[node as usize];
            let plane = &self.planes[n.plane];
            // Box corner extremes along the plane normal.
            let mut near = -plane.dist;
            let mut far = -plane.dist;
            for k in 0..3 {
                let (lo, hi) = (plane.normal[k] * mins[k], plane.normal[k] * maxs[k]);
                near += lo.min(hi);
                far += lo.max(hi);
            }
            if far >= 0.0 {
                stack.push(n.children[0]);
            }
            if near < 0.0 {
                stack.push(n.children[1]);
            }
        }
        out
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

impl Bsp {
    /// Whether a node has an open leaf with a rank below it, for each node (memoised).
    pub fn open_nodes(&self, rank: &[u32]) -> Vec<bool> {
        let mut open = vec![false; self.nodes.len()];
        let mut done = vec![false; self.nodes.len()];
        fn walk(b: &Bsp, n: usize, rank: &[u32], open: &mut [bool], done: &mut [bool]) -> bool {
            if done[n] {
                return open[n];
            }
            let mut any = false;
            for c in b.nodes[n].children {
                any |= if c < 0 {
                    let leaf = (-1 - c) as usize;
                    leaf != 0 && rank.get(leaf).is_some_and(|r| *r != u32::MAX)
                } else {
                    walk(b, c as usize, rank, open, done)
                };
            }
            done[n] = true;
            open[n] = any;
            any
        }
        for n in 0..self.nodes.len() {
            walk(self, n, rank, &mut open, &mut done);
        }
        open
    }

    /// The open leaf at `p`, or when `p` is in rock the open leaf nearest to it in the tree
    /// (the guest's `anchor_gap`): follow the point's side while it still has an open leaf below.
    pub fn open_leaf(&self, p: [f32; 3], rank: &[u32], open: &[bool]) -> Option<usize> {
        let mut node = self.models[0].head_node;
        let ok = |c: i32| {
            if c < 0 {
                let leaf = (-1 - c) as usize;
                leaf != 0 && rank.get(leaf).is_some_and(|r| *r != u32::MAX)
            } else {
                open[c as usize]
            }
        };
        while node >= 0 {
            let n = node as usize;
            let side = usize::from(self.side(n, p) < 0.0);
            let own = self.nodes[n].children[side];
            node = if ok(own) {
                own
            } else {
                self.nodes[n].children[1 - side]
            };
        }
        let leaf = (-1 - node) as usize;
        (leaf != 0 && rank.get(leaf).is_some_and(|r| *r != u32::MAX)).then_some(leaf)
    }
}

impl Bsp {
    /// Back-to-front rank of every child slot of the world tree from `eye`: index `2 * node +
    /// side` (side 0 = the plane's front child). Solid children (leaf 0) get a rank of their own,
    /// so a point in rock has a position in the sequence. Also returns the slot of every leaf.
    pub fn slot_ranks(&self, eye: [f32; 3]) -> (Vec<u32>, Vec<u32>) {
        let mut slot_rank = vec![u32::MAX; self.nodes.len() * 2];
        let mut leaf_slot = vec![u32::MAX; self.leaf_faces.len()];
        let mut next = 0u32;
        // (node, state, slot of this node's own entry): state 0 = enter, 1 = after far child.
        let mut stack: Vec<(i32, u8)> = vec![(self.models[0].head_node, 0)];
        let mut near_of: Vec<usize> = vec![0; self.nodes.len()];
        while let Some((node, state)) = stack.pop() {
            if node < 0 {
                continue;
            }
            let n = node as usize;
            let front = self.side(n, eye) >= 0.0;
            let (near, far) = if front { (0usize, 1usize) } else { (1, 0) };
            near_of[n] = near;
            let mut visit = |side: usize, next: &mut u32, stack: &mut Vec<(i32, u8)>| {
                let c = self.nodes[n].children[side];
                if c < 0 {
                    slot_rank[2 * n + side] = *next;
                    *next += 1;
                    let leaf = (-1 - c) as usize;
                    if leaf != 0 && leaf < leaf_slot.len() {
                        leaf_slot[leaf] = (2 * n + side) as u32;
                    }
                } else {
                    stack.push((c, 0));
                }
            };
            if state == 0 {
                stack.push((node, 1));
                visit(far, &mut next, &mut stack);
            } else {
                visit(near, &mut next, &mut stack);
            }
        }
        (slot_rank, leaf_slot)
    }

    /// Child slot (`2 * node + side`) of the world tree that holds `p`.
    pub fn point_slot(&self, p: [f32; 3]) -> usize {
        let mut node = self.models[0].head_node;
        loop {
            let n = node as usize;
            let side = usize::from(self.side(n, p) < 0.0);
            node = self.nodes[n].children[side];
            if node < 0 {
                return 2 * n + side;
            }
        }
    }
}

impl Bsp {
    /// One back-to-front walk numbering every event: child slots (`2 * node + side`) and node
    /// emissions (a node's own faces draw between its far and near subtrees) share a counter.
    /// Returns (slot ranks, node ranks).
    pub fn event_ranks(&self, eye: [f32; 3]) -> (Vec<u32>, Vec<u32>) {
        let mut slot_rank = vec![u32::MAX; self.nodes.len() * 2];
        let mut node_rank = vec![u32::MAX; self.nodes.len()];
        let mut next = 0u32;
        let mut stack: Vec<(i32, u8)> = vec![(self.models[0].head_node, 0)];
        while let Some((node, state)) = stack.pop() {
            if node < 0 {
                continue;
            }
            let n = node as usize;
            let front = self.side(n, eye) >= 0.0;
            let (near, far) = if front { (0usize, 1usize) } else { (1, 0) };
            let side = if state == 0 { far } else { near };
            if state == 1 {
                node_rank[n] = next;
                next += 1;
            }
            let c = self.nodes[n].children[side];
            if state == 0 {
                stack.push((node, 1));
            }
            if c < 0 {
                slot_rank[2 * n + side] = next;
                next += 1;
            } else {
                stack.push((c, 0));
            }
        }
        (slot_rank, node_rank)
    }
}

impl Bsp {
    /// Cut a convex polygon (BSP axes) through the world tree. Returns one fragment per child
    /// slot it touches: (slot `2 * node + side`, polygon, whether the slot is solid).
    pub fn split_polygon(&self, poly: &[[f32; 3]]) -> Vec<(usize, Vec<[f32; 3]>, bool)> {
        let mut out = Vec::new();
        self.split_into(self.models[0].head_node, poly.to_vec(), &mut out);
        out
    }

    fn split_into(
        &self,
        node: i32,
        poly: Vec<[f32; 3]>,
        out: &mut Vec<(usize, Vec<[f32; 3]>, bool)>,
    ) {
        if poly.len() < 3 {
            return;
        }
        let n = node as usize;
        let d: Vec<f32> = poly.iter().map(|v| self.side(n, *v)).collect();
        const EPS: f32 = 0.05;
        let all_front = d.iter().all(|x| *x >= -EPS);
        let all_back = d.iter().all(|x| *x <= EPS);
        let mut parts: [Vec<[f32; 3]>; 2] = [Vec::new(), Vec::new()];
        if all_front || all_back {
            parts[usize::from(!all_front)] = poly;
        } else {
            for i in 0..poly.len() {
                let j = (i + 1) % poly.len();
                let (a, b) = (poly[i], poly[j]);
                let (da, db) = (d[i], d[j]);
                if da >= -EPS {
                    parts[0].push(a);
                }
                if da <= EPS {
                    parts[1].push(a);
                }
                if (da > EPS && db < -EPS) || (da < -EPS && db > EPS) {
                    let t = da / (da - db);
                    let m = [
                        a[0] + (b[0] - a[0]) * t,
                        a[1] + (b[1] - a[1]) * t,
                        a[2] + (b[2] - a[2]) * t,
                    ];
                    parts[0].push(m);
                    parts[1].push(m);
                }
            }
        }
        for side in 0..2 {
            let part = std::mem::take(&mut parts[side]);
            if part.len() < 3 {
                continue;
            }
            let c = self.nodes[n].children[side];
            if c < 0 {
                out.push((2 * n + side, part, c == -1));
            } else {
                self.split_into(c, part, out);
            }
        }
    }
}
