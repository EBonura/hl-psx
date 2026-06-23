//! Parse a cooked `.hlm` v4 map (tools/hl-bsp --cook). HLM4 = HLM3 + BSP
//! visibility (PVS) appended after the texture blob.
//!
//!   magic "HLM4" | u32 n_verts,n_tris,n_texs,n_faces,bsp_off
//!   verts i16×3 | tri_idx u16×3 | tri_tex u16 | tri_uv u8×6 | tri_rgb u8×9 (pad)
//!   textures × n_texs: u16 w,h | u16 clut[16] | u8 pix[w*h/2]
//!   bsp @ bsp_off:
//!     u32 n_nodes,n_leaves,n_marks,vis_len
//!     face_first u32×n_faces | face_ntri u16×n_faces (pad 4)
//!     nodes  (i16 nx,ny,nz, i16 pad, i32 dist, i32 c0, i32 c1) × n_nodes
//!     leaves (i32 visofs, u16 mark_start, u16 mark_count) × n_leaves
//!     marks  u16 × n_marks (pad 4)
//!     vis    u8  × vis_len (pad 4)
//!
//! Fields are read via `from_le_bytes` (include_bytes! is only byte-aligned).

use psx_gte::math::Vec3I16;

fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn rd_i32(d: &[u8], o: usize) -> i32 {
    rd_u32(d, o) as i32
}
fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn rd_i16(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}
fn align4(x: usize) -> usize {
    (x + 3) & !3
}

const NODE_SZ: usize = 20;

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
    idx_off: usize,
    ttex_off: usize,
    tuv_off: usize,
    trgb_off: usize,
    texblk_off: usize,
    // BSP / PVS
    pub n_nodes: usize,
    pub n_leaves: usize,
    pub n_marks: usize,
    ff_off: usize,
    fn_off: usize,
    fp_off: usize,
    nodes_off: usize,
    leaves_off: usize,
    marks_off: usize,
    vis_off: usize,
    vis_len: usize,
    // Clip hull + spawn
    pub n_clip: usize,
    pub hull1_head: i32,
    pub spawn_pos: [i32; 3],
    pub spawn_yaw: i32,
    clipn_off: usize,
    // Entities (brush models)
    pub n_models: usize,
    pub n_ents: usize,
    models_off: usize,
    ents_off: usize,
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

const LEAF_SZ: usize = 16; // visofs i32 + mark range u16×2 + bbox centre i16×3 + radius u16
const CLIPNODE_SZ: usize = 16;
const ENT_SZ: usize = 48;

pub struct ClipNode {
    pub n: [i16; 3],
    pub c0: i16,
    pub c1: i16,
    pub dist: i32,
}

pub struct Ent {
    pub submodel: usize,
    pub kind: u16, // 0 static, 1 door
    pub origin: [i32; 3],
    pub mv: [i32; 3],
    pub center: [i32; 3],
    pub r2: i32,
    pub head: i32, // submodel hull-1 clipnode root
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
        let idx_off = v_off + n_verts * 6;
        let ttex_off = idx_off + n_tris * 6;
        let tuv_off = ttex_off + n_tris * 2;
        let trgb_off = tuv_off + n_tris * 6;
        let texblk_off = align4(trgb_off + n_tris * 9);

        let n_nodes = rd_u32(data, bsp_off) as usize;
        let n_leaves = rd_u32(data, bsp_off + 4) as usize;
        let n_marks = rd_u32(data, bsp_off + 8) as usize;
        let vis_len = rd_u32(data, bsp_off + 12) as usize;
        let ff_off = bsp_off + 16;
        let fn_off = ff_off + n_faces * 4;
        let fp_off = align4(fn_off + n_faces * 2);
        let nodes_off = align4(fp_off + n_faces * 12);
        let leaves_off = nodes_off + n_nodes * NODE_SZ;
        let marks_off = leaves_off + n_leaves * LEAF_SZ;
        let vis_off = align4(marks_off + n_marks * 2);

        let n_clip = rd_u32(data, clip_off) as usize;
        let hull1_head = rd_i32(data, clip_off + 4);
        let spawn_pos = [rd_i32(data, clip_off + 8), rd_i32(data, clip_off + 12), rd_i32(data, clip_off + 16)];
        let spawn_yaw = rd_i32(data, clip_off + 20);
        let clipn_off = clip_off + 24;

        let n_models = rd_u32(data, ent_off) as usize;
        let models_off = ent_off + 4;
        let n_ents_off = models_off + n_models * 8;
        let n_ents = rd_u32(data, n_ents_off) as usize;
        let ents_off = n_ents_off + 4;

        let tram_submodel = rd_u16(data, tram_off) as usize;
        let n_way = rd_u16(data, tram_off + 2) as usize;
        let tram_speed = rd_i32(data, tram_off + 4);
        let tram_head = rd_i32(data, tram_off + 8);
        let tram_base = [rd_i32(data, tram_off + 12), rd_i32(data, tram_off + 16), rd_i32(data, tram_off + 20)];
        let way_off = tram_off + 24;

        let n_props = rd_u32(data, prop_off) as usize;
        let props_off = prop_off + 4;

        Map {
            data, n_verts, n_tris, n_texs, n_faces,
            v_off, idx_off, ttex_off, tuv_off, trgb_off, texblk_off,
            n_nodes, n_leaves, n_marks,
            ff_off, fn_off, fp_off, nodes_off, leaves_off, marks_off, vis_off, vis_len,
            n_clip, hull1_head, spawn_pos, spawn_yaw, clipn_off,
            n_models, n_ents, models_off, ents_off,
            tram_submodel, tram_speed, tram_head, tram_base, n_way, way_off,
            n_props, props_off,
        }
    }

    /// `(model_type, origin, yaw)` for prop `i` (a placed studio model).
    #[inline]
    pub fn prop(&self, i: usize) -> (u16, [i32; 3], i32) {
        let o = self.props_off + i * 20;
        (
            rd_u16(self.data, o),
            [rd_i32(self.data, o + 4), rd_i32(self.data, o + 8), rd_i32(self.data, o + 12)],
            rd_i32(self.data, o + 16),
        )
    }

    #[inline]
    pub fn waypoint(&self, i: usize) -> [i32; 3] {
        let o = self.way_off + i * 12;
        [rd_i32(self.data, o), rd_i32(self.data, o + 4), rd_i32(self.data, o + 8)]
    }

    /// `(first_face, num_faces)` for BSP submodel `m` (0 = world).
    #[inline]
    pub fn submodel(&self, m: usize) -> (usize, usize) {
        let o = self.models_off + m * 8;
        (rd_u32(self.data, o) as usize, rd_u32(self.data, o + 4) as usize)
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
        }
    }

    #[inline]
    pub fn clipnode(&self, i: usize) -> ClipNode {
        let o = self.clipn_off + i * CLIPNODE_SZ;
        ClipNode {
            n: [rd_i16(self.data, o), rd_i16(self.data, o + 2), rd_i16(self.data, o + 4)],
            c0: rd_i16(self.data, o + 6),
            c1: rd_i16(self.data, o + 8),
            dist: rd_i32(self.data, o + 12),
        }
    }

    #[inline]
    pub fn vert(&self, i: usize) -> Vec3I16 {
        let o = self.v_off + i * 6;
        Vec3I16::new(rd_i16(self.data, o), rd_i16(self.data, o + 2), rd_i16(self.data, o + 4))
    }

    #[inline]
    pub fn tri_idx(&self, t: usize) -> (usize, usize, usize) {
        let o = self.idx_off + t * 6;
        (rd_u16(self.data, o) as usize, rd_u16(self.data, o + 2) as usize, rd_u16(self.data, o + 4) as usize)
    }

    #[inline]
    pub fn tri_tex(&self, t: usize) -> usize {
        rd_u16(self.data, self.ttex_off + t * 2) as usize
    }

    #[inline]
    pub fn tri_uv(&self, t: usize) -> [(u8, u8); 3] {
        let o = self.tuv_off + t * 6;
        let d = self.data;
        [(d[o], d[o + 1]), (d[o + 2], d[o + 3]), (d[o + 4], d[o + 5])]
    }

    /// Per-corner lightmap shade (PS1 modulation tint), one (r,g,b) per vertex.
    #[inline]
    pub fn tri_rgb(&self, t: usize) -> [(u8, u8, u8); 3] {
        let o = self.trgb_off + t * 9;
        let d = self.data;
        [
            (d[o], d[o + 1], d[o + 2]),
            (d[o + 3], d[o + 4], d[o + 5]),
            (d[o + 6], d[o + 7], d[o + 8]),
        ]
    }

    pub fn tex_blob(&self) -> &'static [u8] {
        &self.data[self.texblk_off..]
    }

    // ---- BSP / PVS ----

    #[inline]
    pub fn node(&self, i: usize) -> Node {
        let o = self.nodes_off + i * NODE_SZ;
        Node {
            n: [rd_i16(self.data, o), rd_i16(self.data, o + 2), rd_i16(self.data, o + 4)],
            dist: rd_i32(self.data, o + 8),
            c0: rd_i32(self.data, o + 12),
            c1: rd_i32(self.data, o + 16),
        }
    }

    /// `(visofs, mark_start, mark_count)` for leaf `i`.
    #[inline]
    pub fn leaf(&self, i: usize) -> (i32, usize, usize) {
        let o = self.leaves_off + i * LEAF_SZ;
        (rd_i32(self.data, o), rd_u16(self.data, o + 4) as usize, rd_u16(self.data, o + 6) as usize)
    }

    /// Leaf bounding sphere `(centre, radius)` in world space (frustum cull).
    #[inline]
    pub fn leaf_bounds(&self, i: usize) -> ([i32; 3], i32) {
        let o = self.leaves_off + i * LEAF_SZ;
        (
            [rd_i16(self.data, o + 8) as i32, rd_i16(self.data, o + 10) as i32, rd_i16(self.data, o + 12) as i32],
            rd_u16(self.data, o + 14) as i32,
        )
    }

    /// Face index referenced by marksurface `j`.
    #[inline]
    pub fn mark(&self, j: usize) -> usize {
        rd_u16(self.data, self.marks_off + j * 2) as usize
    }

    /// Face `f`'s world plane `(normal ×4096, dist)` for backface culling.
    #[inline]
    pub fn face_plane(&self, f: usize) -> ([i16; 3], i32) {
        let o = self.fp_off + f * 12;
        (
            [rd_i16(self.data, o), rd_i16(self.data, o + 2), rd_i16(self.data, o + 4)],
            rd_i32(self.data, o + 8),
        )
    }

    /// `(first_tri, tri_count)` for face `f`.
    #[inline]
    pub fn face_tris(&self, f: usize) -> (usize, usize) {
        (rd_u32(self.data, self.ff_off + f * 4) as usize, rd_u16(self.data, self.fn_off + f * 2) as usize)
    }

    pub fn vis(&self) -> &'static [u8] {
        &self.data[self.vis_off..self.vis_off + self.vis_len]
    }

    /// World-space AABB (min, max) -- fly-cam spawn fallback.
    pub fn bounds(&self) -> ([i32; 3], [i32; 3]) {
        let mut mn = [i32::MAX; 3];
        let mut mx = [i32::MIN; 3];
        for i in 0..self.n_verts {
            let v = self.vert(i);
            let p = [v.x as i32, v.y as i32, v.z as i32];
            for k in 0..3 {
                if p[k] < mn[k] {
                    mn[k] = p[k];
                }
                if p[k] > mx[k] {
                    mx[k] = p[k];
                }
            }
        }
        (mn, mx)
    }
}
