//! Load a cooked `.hlmdl` (a Half-Life studio model, baked to per-frame posed
//! vertices by `hl-bsp --mdl`). The cook bakes a chosen sequence's frames host
//! side (no skinning on the PS1); the runtime just selects a frame.
//!
//!   magic "HMD2" | u32 n_verts, n_tris, n_texs, n_frames
//!   n_frames × (verts i16×3) | TriRec[16B] × n_tris | textures...
//!
//! Clip-aware files use HMD3:
//!   magic "HMD3" | u32 n_verts, n_tris, n_texs, n_frames, n_clips
//!   ClipRec[4B] × n_clips | n_frames × verts | TriRec[16B] × n_tris | textures...
//!     ClipRec = u16 first_frame, u16 frame_count
//!     TriRec = u16 idx[3], u16 tex, u8 uv[6], u16 pad
//!
//! Compact-frame files use HMD4:
//!   magic "HMD4" | u32 n_verts,n_tris,n_texs,n_frames,n_clips,frame_data_len
//!   ClipRec[4B] × n_clips | FrameRec[8B] × n_frames | frame_data | tris | textures
//!     FrameRec = u32 frame_data_offset, u8 mode, u8 pad[3]
//!     mode 0 = full i16 xyz frame, mode 1 = i8 xyz delta from frame 0

use core::ptr;

use psx_engine::TexturedModelRenderFace;
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
    pub n_clips: usize,
    clips_off: usize,
    frame_desc_off: usize,
    v_off: usize,
    frame_stride: usize,
    tri_off: usize,
    texblk_off: usize,
    compact_frames: bool,
    base_frame_off: usize,
}

const TRI_SZ: usize = 16;
const FRAME_REC_SZ: usize = 8;
const FRAME_MODE_BASE_I8: u8 = 1;

#[derive(Clone, Copy)]
pub struct Tri {
    pub idx: [u16; 3],
    pub tex: usize,
    pub uv: [(u8, u8); 3],
}

#[derive(Clone, Copy)]
pub struct RenderFace {
    pub face: TexturedModelRenderFace,
    pub tex: u16,
}

impl RenderFace {
    pub const ZERO: Self = Self {
        face: TexturedModelRenderFace::ZERO,
        tex: 0,
    };
}

impl Model {
    pub fn load(data: &'static [u8]) -> Model {
        let hmd3 = data.get(0..4) == Some(b"HMD3");
        let hmd4 = data.get(0..4) == Some(b"HMD4");
        let n_verts = rd_u32(data, 4) as usize;
        let n_tris = rd_u32(data, 8) as usize;
        let n_texs = rd_u32(data, 12) as usize;
        let n_frames = (rd_u32(data, 16) as usize).max(1);
        let n_clips = if hmd3 || hmd4 {
            (rd_u32(data, 20) as usize).max(1)
        } else {
            1
        };
        let frame_data_len = if hmd4 { rd_u32(data, 24) as usize } else { 0 };
        let clips_off = if hmd4 {
            28
        } else if hmd3 {
            24
        } else {
            0
        };
        let frame_desc_off = if hmd4 { clips_off + n_clips * 4 } else { 0 };
        let v_off = if hmd4 {
            frame_desc_off + n_frames * FRAME_REC_SZ
        } else if hmd3 {
            clips_off + n_clips * 4
        } else {
            20
        };
        let frame_stride = n_verts * 6;
        let tri_off = if hmd4 {
            v_off + frame_data_len
        } else {
            v_off + n_frames * frame_stride
        };
        let texblk_off = tri_off + n_tris * TRI_SZ;
        let base_frame_off = if hmd4 {
            v_off + rd_u32(data, frame_desc_off) as usize
        } else {
            v_off
        };
        Model {
            data,
            n_verts,
            n_tris,
            n_texs,
            n_frames,
            n_clips,
            clips_off,
            frame_desc_off,
            v_off,
            frame_stride,
            tri_off,
            texblk_off,
            compact_frames: hmd4,
            base_frame_off,
        }
    }

    #[inline]
    pub fn clip_len(&self, clip: usize) -> usize {
        if self.clips_off == 0 {
            return self.n_frames.max(1);
        }
        let c = clip.min(self.n_clips.saturating_sub(1));
        let o = self.clips_off + c * 4;
        (rd_u16(self.data, o + 2) as usize).max(1)
    }

    #[inline]
    pub fn clip_frame(&self, clip: usize, local_frame: usize) -> usize {
        if self.clips_off == 0 {
            return local_frame % self.n_frames.max(1);
        }
        let c = clip.min(self.n_clips.saturating_sub(1));
        let o = self.clips_off + c * 4;
        let first = rd_u16(self.data, o) as usize;
        let count = (rd_u16(self.data, o + 2) as usize).max(1);
        (first + (local_frame % count)).min(self.n_frames.saturating_sub(1))
    }

    #[inline]
    pub fn vert(&self, frame: usize, i: usize) -> Vec3I16 {
        let f = if frame < self.n_frames { frame } else { 0 };
        if self.compact_frames {
            return self.compact_vert(f, i);
        }
        let o = self.v_off + f * self.frame_stride + i * 6;
        Vec3I16::new(
            rd_i16(self.data, o),
            rd_i16(self.data, o + 2),
            rd_i16(self.data, o + 4),
        )
    }

    #[inline]
    fn compact_vert(&self, frame: usize, i: usize) -> Vec3I16 {
        let desc = self.frame_desc_off + frame * FRAME_REC_SZ;
        let frame_off = self.v_off + rd_u32(self.data, desc) as usize;
        match self.data[desc + 4] {
            FRAME_MODE_BASE_I8 => {
                let base = self.base_frame_off + i * 6;
                let delta = frame_off + i * 3;
                Vec3I16::new(
                    rd_i16(self.data, base) + self.data[delta] as i8 as i16,
                    rd_i16(self.data, base + 2) + self.data[delta + 1] as i8 as i16,
                    rd_i16(self.data, base + 4) + self.data[delta + 2] as i8 as i16,
                )
            }
            _ => {
                let o = frame_off + i * 6;
                Vec3I16::new(
                    rd_i16(self.data, o),
                    rd_i16(self.data, o + 2),
                    rd_i16(self.data, o + 4),
                )
            }
        }
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

    pub unsafe fn fill_render_faces_raw(&self, out: *mut RenderFace, out_len: usize) -> usize {
        let n = self.n_tris.min(out_len);
        let mut t = 0;
        while t < n {
            let tri = self.tri(t);
            ptr::write(
                out.add(t),
                RenderFace {
                    face: TexturedModelRenderFace::new(tri.idx, tri.uv),
                    tex: tri.tex.min(u16::MAX as usize) as u16,
                },
            );
            t += 1;
        }
        n
    }

    pub fn tex_blob(&self) -> &'static [u8] {
        &self.data[self.texblk_off..]
    }
}
