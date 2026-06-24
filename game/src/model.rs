//! Load a cooked `.hlmdl` (a Half-Life studio model, baked to per-frame posed
//! vertices by `hl-bsp --mdl`). The cook bakes a chosen sequence's frames host
//! side (no skinning on the PS1); the runtime just selects a frame.
//!
//!   magic "HMD2" | u32 n_verts, n_tris, n_texs, n_frames
//!   n_frames × (verts i16×3) | TriRec[16B] × n_tris | textures...
//!     TriRec = u16 idx[3], u16 tex, u8 uv[6], u16 pad

use psx_gte::math::Vec3I16;

#[inline(always)]
fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
#[inline(always)]
fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
#[inline(always)]
fn rd_i16(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}

pub struct Model {
    data: &'static [u8],
    pub n_verts: usize,
    pub n_tris: usize,
    pub n_texs: usize,
    pub n_frames: usize,
    v_off: usize,
    frame_stride: usize,
    tri_off: usize,
    texblk_off: usize,
}

const TRI_SZ: usize = 16;

#[derive(Clone, Copy)]
pub struct Tri {
    pub idx: [u16; 3],
    pub tex: usize,
    pub uv: [(u8, u8); 3],
}

impl Model {
    pub fn load(data: &'static [u8]) -> Model {
        let n_verts = rd_u32(data, 4) as usize;
        let n_tris = rd_u32(data, 8) as usize;
        let n_texs = rd_u32(data, 12) as usize;
        let n_frames = (rd_u32(data, 16) as usize).max(1);
        let v_off = 20;
        let frame_stride = n_verts * 6;
        let tri_off = v_off + n_frames * frame_stride;
        let texblk_off = tri_off + n_tris * TRI_SZ;
        Model {
            data,
            n_verts,
            n_tris,
            n_texs,
            n_frames,
            v_off,
            frame_stride,
            tri_off,
            texblk_off,
        }
    }

    #[inline]
    pub fn vert(&self, frame: usize, i: usize) -> Vec3I16 {
        let f = if frame < self.n_frames { frame } else { 0 };
        let o = self.v_off + f * self.frame_stride + i * 6;
        Vec3I16::new(
            rd_i16(self.data, o),
            rd_i16(self.data, o + 2),
            rd_i16(self.data, o + 4),
        )
    }

    #[inline]
    pub fn tri(&self, t: usize) -> Tri {
        let o = self.tri_off + t * TRI_SZ;
        let d = self.data;
        Tri {
            idx: [rd_u16(d, o), rd_u16(d, o + 2), rd_u16(d, o + 4)],
            tex: rd_u16(d, o + 6) as usize,
            uv: [
                (d[o + 8], d[o + 9]),
                (d[o + 10], d[o + 11]),
                (d[o + 12], d[o + 13]),
            ],
        }
    }

    pub fn tex_blob(&self) -> &'static [u8] {
        &self.data[self.texblk_off..]
    }
}
