//! Load a cooked `.hlmdl` (a Half-Life studio model baked to its reference pose
//! by `hl-bsp --mdl`). Static posed textured mesh -- same geometry+texture
//! layout as a map, minus BSP/PVS/clip data.
//!
//!   magic "HMDL" | u32 n_verts, n_tris, n_texs
//!   verts i16×3 | tri_idx u16×3 | tri_tex u16 | tri_uv u8×6 (pad4) | textures...

use psx_gte::math::Vec3I16;

fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn rd_i16(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}

pub struct Model {
    data: &'static [u8],
    pub n_verts: usize,
    pub n_tris: usize,
    pub n_texs: usize,
    v_off: usize,
    idx_off: usize,
    ttex_off: usize,
    tuv_off: usize,
    texblk_off: usize,
}

impl Model {
    pub fn load(data: &'static [u8]) -> Model {
        let n_verts = rd_u32(data, 4) as usize;
        let n_tris = rd_u32(data, 8) as usize;
        let n_texs = rd_u32(data, 12) as usize;
        let v_off = 16;
        let idx_off = v_off + n_verts * 6;
        let ttex_off = idx_off + n_tris * 6;
        let tuv_off = ttex_off + n_tris * 2;
        let texblk_off = (tuv_off + n_tris * 6 + 3) & !3;
        Model { data, n_verts, n_tris, n_texs, v_off, idx_off, ttex_off, tuv_off, texblk_off }
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

    pub fn tex_blob(&self) -> &'static [u8] {
        &self.data[self.texblk_off..]
    }
}
