//! Parse a cooked `.hlm` map (tools/hl-bsp --cook). HLMA adds BSP visibility
//! plus brush-entity leaf membership for PVS culling.
//!
//!   magic "HLMA" | u32 n_verts,n_tris,n_texs,n_faces,bsp_off
//!   verts i16×3 | TriRec[22B] × n_tris
//!     TriRec = u16 idx[3], u8 tex, u8 uv[6], u8 rgb[9]
//!   optional legacy textures × n_texs: u16 w,h | u16 clut[16] | u8 pix[w*h/2]
//!   modern streamed builds keep texture pixels in a separate HLTX chunk:
//!     magic "HLTX" | u32 n_texs | textures...
//!   bsp @ bsp_off:
//!     u32 n_nodes,n_leaves,n_marks,vis_len
//!     FaceRec[28B] × n_faces
//!       FaceRec = u16 first_tri, u16 tri_count, i16 normal[3], i32 dist,
//!                 u16 plane_group, i16 center[3], u16 extent[3]
//!     nodes  (i16 nx,ny,nz, i32 dist, i16 c0, i16 c1) × n_nodes
//!     leaves (i32 visofs, u16 mark_start, u16 mark_count) × n_leaves
//!     marks  u16 × n_marks (pad 4)
//!     vis    u8  × vis_len (pad 4)
//!   clip:
//!     u32 n_clip | i32 hull0_head | i32 hull1_head | i32 spawn[3] |
//!     i32 spawn_yaw | ClipNode[16B] × n_clip
//!   entities:
//!     u32 n_models | (u32 firstface,u32 numface) × n_models
//!     u32 n_ents | EntRec[52B] × n_ents | u32 n_ent_leafs | u16 leaf_idx[]
//!   props:
//!     u32 n_props | (u16 type, i16 leaf, i32 origin[3], i32 yaw) × n_props
//!
//! Fields are read via `from_le_bytes` (include_bytes! is only byte-aligned).

use core::ptr;

use psx_gte::math::Vec3I16;

#[inline(always)]
fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
#[inline(always)]
fn rd_i32(d: &[u8], o: usize) -> i32 {
    rd_u32(d, o) as i32
}
#[inline(always)]
fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
#[inline(always)]
fn rd_i16(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}
#[inline(always)]
fn align4(x: usize) -> usize {
    (x + 3) & !3
}

const NODE_SZ: usize = 14;

pub struct Node {
    pub n: [i16; 3],
    pub dist: i32,
    pub c0: i32,
    pub c1: i32,
}

pub struct Map {
    data: &'static [u8],
    pub n_verts: usize,
    pub n_tris: usize,
    pub n_texs: usize,
    pub n_faces: usize,
    v_off: usize,
    tri_off: usize,
    // BSP / PVS
    pub n_nodes: usize,
    pub n_leaves: usize,
    pub n_marks: usize,
    faces_off: usize,
    nodes_off: usize,
    leaves_off: usize,
    marks_off: usize,
    vis_off: usize,
    vis_len: usize,
    // Clip hull + spawn
    pub n_clip: usize,
    pub hull0_head: i32,
    pub hull1_head: i32,
    pub spawn_pos: [i32; 3],
    pub spawn_yaw: i32,
    clipn_off: usize,
    // Entities (brush models)
    pub n_models: usize,
    pub n_ents: usize,
    models_off: usize,
    ents_off: usize,
    ent_leafs_off: usize,
    n_ent_leafs: usize,
    // Tram (func_tracktrain ride)
    pub tram_submodel: usize,
    pub tram_speed: i32,
    pub tram_head: i32,
    pub tram_base: [i32; 3], // wp0 - tram origin: places the brush onto the track
    pub n_way: usize,
    way_off: usize,
    // Props (point-entity model placements)
    pub n_props: usize,
    props_off: usize,
}

const LEAF_SZ: usize = 8; // visofs i32 + marks u16×2
const FACE_SZ: usize = 28;
const TRI_SZ: usize = 22;
const CLIPNODE_SZ: usize = 16;
const ENT_SZ: usize = 52;

pub struct ClipNode {
    pub n: [i16; 3],
    pub c0: i16,
    pub c1: i16,
    pub dist: i32,
}

#[derive(Clone, Copy)]
pub struct Ent {
    pub submodel: usize,
    pub kind: u16, // 0 solid/static, 1 door, 2 nonsolid visual brush
    pub origin: [i32; 3],
    pub mv: [i32; 3],
    pub center: [i32; 3],
    pub r2: i32,
    pub head: i32, // submodel hull-1 clipnode root
    pub leaf_start: usize,
    pub leaf_count: usize,
}

#[derive(Clone, Copy)]
pub struct RenderTri {
    pub idx: [u16; 3],
    pub tex: usize,
    pub uv_words: [u16; 3],
    pub rgb: [(u8, u8, u8); 3],
}

#[inline(always)]
const fn uv_word(u: u8, v: u8) -> u16 {
    (u as u16) | ((v as u16) << 8)
}

impl Map {
    pub fn load(data: &'static [u8]) -> Map {
        let n_verts = rd_u32(data, 4) as usize;
        let n_tris = rd_u32(data, 8) as usize;
        let n_texs = rd_u32(data, 12) as usize;
        let n_faces = rd_u32(data, 16) as usize;
        let bsp_off = rd_u32(data, 20) as usize;
        let clip_off = rd_u32(data, 24) as usize;
        let ent_off = rd_u32(data, 28) as usize;
        let tram_off = rd_u32(data, 32) as usize;
        let prop_off = rd_u32(data, 36) as usize;
        let v_off = 40;
        let tri_off = v_off + n_verts * 6;

        let n_nodes = rd_u32(data, bsp_off) as usize;
        let n_leaves = rd_u32(data, bsp_off + 4) as usize;
        let n_marks = rd_u32(data, bsp_off + 8) as usize;
        let vis_len = rd_u32(data, bsp_off + 12) as usize;
        let faces_off = bsp_off + 16;
        let nodes_off = faces_off + n_faces * FACE_SZ;
        let leaves_off = nodes_off + n_nodes * NODE_SZ;
        let marks_off = leaves_off + n_leaves * LEAF_SZ;
        let vis_off = align4(marks_off + n_marks * 2);

        let n_clip = rd_u32(data, clip_off) as usize;
        let hull0_head = rd_i32(data, clip_off + 4);
        let hull1_head = rd_i32(data, clip_off + 8);
        let spawn_pos = [
            rd_i32(data, clip_off + 12),
            rd_i32(data, clip_off + 16),
            rd_i32(data, clip_off + 20),
        ];
        let spawn_yaw = rd_i32(data, clip_off + 24);
        let clipn_off = clip_off + 28;

        let n_models = rd_u32(data, ent_off) as usize;
        let models_off = ent_off + 4;
        let n_ents_off = models_off + n_models * 8;
        let n_ents = rd_u32(data, n_ents_off) as usize;
        let ents_off = n_ents_off + 4;
        let n_ent_leafs_off = ents_off + n_ents * ENT_SZ;
        let n_ent_leafs = rd_u32(data, n_ent_leafs_off) as usize;
        let ent_leafs_off = n_ent_leafs_off + 4;

        let tram_submodel = rd_u16(data, tram_off) as usize;
        let n_way = rd_u16(data, tram_off + 2) as usize;
        let tram_speed = rd_i32(data, tram_off + 4);
        let tram_head = rd_i32(data, tram_off + 8);
        let tram_base = [
            rd_i32(data, tram_off + 12),
            rd_i32(data, tram_off + 16),
            rd_i32(data, tram_off + 20),
        ];
        let way_off = tram_off + 24;

        let n_props = rd_u32(data, prop_off) as usize;
        let props_off = prop_off + 4;

        Map {
            data,
            n_verts,
            n_tris,
            n_texs,
            n_faces,
            v_off,
            tri_off,
            n_nodes,
            n_leaves,
            n_marks,
            faces_off,
            nodes_off,
            leaves_off,
            marks_off,
            vis_off,
            vis_len,
            n_clip,
            hull0_head,
            hull1_head,
            spawn_pos,
            spawn_yaw,
            clipn_off,
            n_models,
            n_ents,
            models_off,
            ents_off,
            ent_leafs_off,
            n_ent_leafs,
            tram_submodel,
            tram_speed,
            tram_head,
            tram_base,
            n_way,
            way_off,
            n_props,
            props_off,
        }
    }

    /// `(model_type, origin, yaw, leaf)` for prop `i` (a placed studio model).
    #[inline]
    pub fn prop(&self, i: usize) -> (u16, [i32; 3], i32, i16) {
        let o = self.props_off + i * 20;
        (
            rd_u16(self.data, o),
            [
                rd_i32(self.data, o + 4),
                rd_i32(self.data, o + 8),
                rd_i32(self.data, o + 12),
            ],
            rd_i32(self.data, o + 16),
            rd_i16(self.data, o + 2),
        )
    }

    #[inline]
    pub fn waypoint(&self, i: usize) -> [i32; 3] {
        let o = self.way_off + i * 12;
        [
            rd_i32(self.data, o),
            rd_i32(self.data, o + 4),
            rd_i32(self.data, o + 8),
        ]
    }

    /// `(first_face, num_faces)` for BSP submodel `m` (0 = world).
    #[inline]
    pub fn submodel(&self, m: usize) -> (usize, usize) {
        let o = self.models_off + m * 8;
        (
            rd_u32(self.data, o) as usize,
            rd_u32(self.data, o + 4) as usize,
        )
    }

    #[inline]
    pub fn entity(&self, i: usize) -> Ent {
        let o = self.ents_off + i * ENT_SZ;
        let d = self.data;
        Ent {
            submodel: rd_u16(d, o) as usize,
            kind: rd_u16(d, o + 2),
            origin: [rd_i32(d, o + 4), rd_i32(d, o + 8), rd_i32(d, o + 12)],
            mv: [rd_i32(d, o + 16), rd_i32(d, o + 20), rd_i32(d, o + 24)],
            center: [rd_i32(d, o + 28), rd_i32(d, o + 32), rd_i32(d, o + 36)],
            r2: rd_i32(d, o + 40),
            head: rd_i32(d, o + 44),
            leaf_start: rd_u16(d, o + 48) as usize,
            leaf_count: rd_u16(d, o + 50) as usize,
        }
    }

    #[inline]
    pub fn ent_leaf(&self, i: usize) -> usize {
        if i >= self.n_ent_leafs {
            return 0;
        }
        rd_u16(self.data, self.ent_leafs_off + i * 2) as usize
    }

    #[inline]
    pub fn clipnode(&self, i: usize) -> ClipNode {
        let o = self.clipn_off + i * CLIPNODE_SZ;
        ClipNode {
            n: [
                rd_i16(self.data, o),
                rd_i16(self.data, o + 2),
                rd_i16(self.data, o + 4),
            ],
            c0: rd_i16(self.data, o + 6),
            c1: rd_i16(self.data, o + 8),
            dist: rd_i32(self.data, o + 12),
        }
    }

    #[inline]
    pub fn vert(&self, i: usize) -> Vec3I16 {
        let o = self.v_off + i * 6;
        Vec3I16::new(
            rd_i16(self.data, o),
            rd_i16(self.data, o + 2),
            rd_i16(self.data, o + 4),
        )
    }

    #[inline]
    pub fn tri_uv_words(&self, t: usize) -> [u16; 3] {
        let o = self.tri_off + t * TRI_SZ;
        let d = self.data;
        [
            uv_word(d[o + 7], d[o + 8]),
            uv_word(d[o + 9], d[o + 10]),
            uv_word(d[o + 11], d[o + 12]),
        ]
    }

    #[inline]
    pub fn render_tri(&self, t: usize, uv_words: [u16; 3]) -> RenderTri {
        let o = self.tri_off + t * TRI_SZ;
        let d = self.data;
        RenderTri {
            idx: [rd_u16(d, o), rd_u16(d, o + 2), rd_u16(d, o + 4)],
            tex: d[o + 6] as usize,
            uv_words,
            rgb: [
                (d[o + 13], d[o + 14], d[o + 15]),
                (d[o + 16], d[o + 17], d[o + 18]),
                (d[o + 19], d[o + 20], d[o + 21]),
            ],
        }
    }

    pub unsafe fn fill_uv_words_raw(&self, out: *mut [u16; 3], out_len: usize) -> usize {
        let n = self.n_tris.min(out_len);
        let mut t = 0;
        while t < n {
            ptr::write(out.add(t), self.tri_uv_words(t));
            t += 1;
        }
        n
    }

    // ---- BSP / PVS ----

    #[inline]
    pub fn node(&self, i: usize) -> Node {
        let o = self.nodes_off + i * NODE_SZ;
        Node {
            n: [
                rd_i16(self.data, o),
                rd_i16(self.data, o + 2),
                rd_i16(self.data, o + 4),
            ],
            dist: rd_i32(self.data, o + 6),
            c0: rd_i16(self.data, o + 10) as i32,
            c1: rd_i16(self.data, o + 12) as i32,
        }
    }

    /// `(visofs, mark_start, mark_count)` for leaf `i`.
    #[inline]
    pub fn leaf(&self, i: usize) -> (i32, usize, usize) {
        let o = self.leaves_off + i * LEAF_SZ;
        (
            rd_i32(self.data, o),
            rd_u16(self.data, o + 4) as usize,
            rd_u16(self.data, o + 6) as usize,
        )
    }

    /// Face index referenced by marksurface `j`.
    #[inline]
    pub fn mark(&self, j: usize) -> usize {
        rd_u16(self.data, self.marks_off + j * 2) as usize
    }

    #[inline]
    pub fn face_plane(&self, f: usize) -> ([i16; 3], i32) {
        let o = self.faces_off + f * FACE_SZ;
        (
            [
                rd_i16(self.data, o + 4),
                rd_i16(self.data, o + 6),
                rd_i16(self.data, o + 8),
            ],
            rd_i32(self.data, o + 10),
        )
    }

    #[inline]
    pub fn face_group(&self, f: usize) -> usize {
        rd_u16(self.data, self.faces_off + f * FACE_SZ + 14) as usize
    }

    #[inline]
    pub fn face_bounds(&self, f: usize) -> ([i32; 3], [i32; 3]) {
        let o = self.faces_off + f * FACE_SZ + 16;
        (
            [
                rd_i16(self.data, o) as i32,
                rd_i16(self.data, o + 2) as i32,
                rd_i16(self.data, o + 4) as i32,
            ],
            [
                rd_u16(self.data, o + 6) as i32,
                rd_u16(self.data, o + 8) as i32,
                rd_u16(self.data, o + 10) as i32,
            ],
        )
    }

    #[inline]
    pub fn face_tris(&self, f: usize) -> (usize, usize) {
        let o = self.faces_off + f * FACE_SZ;
        (
            rd_u16(self.data, o) as usize,
            rd_u16(self.data, o + 2) as usize,
        )
    }

    pub fn vis(&self) -> &'static [u8] {
        &self.data[self.vis_off..self.vis_off + self.vis_len]
    }
}
