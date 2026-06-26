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
//! Compact-frame files use HMD4/HMD5:
//!   magic "HMD4" | u32 n_verts,n_tris,n_texs,n_frames,n_clips,frame_data_len
//!   ClipRec[4B] × n_clips | FrameRec[8B] × n_frames | frame_data | tris | textures
//!     FrameRec = u32 frame_data_offset, u8 mode, u8 pad[3]
//!     mode 0 = full i16 xyz frame, mode 1 = i8 xyz delta from frame 0
//! HMD5 inserts `u16 local_to_world_q12, u16 flags` after frame_data_len.
//! HMD6 uses the HMD5 header and expands TriRec to 20B with i8 face normals.

use core::ptr;

use psx_engine::TexturedModelRenderFace;
use psx_gte::math::Vec3I16;

// ponytail: unchecked reads, same rationale as map.rs -- no D-cache on the
// R3000, so dropping the bounds branch lets LLVM coalesce these into MIPS
// unaligned word loads (lwl/lwr). Offsets derive from the cooked model header
// (magic-checked at load), so they are in range by construction.
#[inline(always)]
fn rd_u32(d: &[u8], o: usize) -> u32 {
    unsafe {
        u32::from_le_bytes([
            *d.get_unchecked(o),
            *d.get_unchecked(o + 1),
            *d.get_unchecked(o + 2),
            *d.get_unchecked(o + 3),
        ])
    }
}
#[inline(always)]
fn rd_u16(d: &[u8], o: usize) -> u16 {
    unsafe { u16::from_le_bytes([*d.get_unchecked(o), *d.get_unchecked(o + 1)]) }
}
#[inline(always)]
fn rd_i16(d: &[u8], o: usize) -> i16 {
    rd_u16(d, o) as i16
}

pub struct Model {
    data: &'static [u8],
    pub n_verts: usize,
    pub n_tris: usize,
    pub n_frames: usize,
    pub n_clips: usize,
    clips_off: usize,
    frame_desc_off: usize,
    v_off: usize,
    frame_stride: usize,
    tri_off: usize,
    tri_sz: usize,
    tri_has_normals: bool,
    compact_frames: bool,
    base_frame_off: usize,
    local_to_world_q12: u16,
}

#[derive(Clone, Copy)]
pub struct ModelFrame<'a> {
    data: &'a [u8],
    compact_frames: bool,
    mode: u8,
    frame_off: usize,
    base_frame_off: usize,
}

const TRI_SZ: usize = 16;
const TRI_SZ_HMD6: usize = 20;
const FRAME_REC_SZ: usize = 8;
const FRAME_MODE_BASE_I8: u8 = 1;
const LOCAL_TO_WORLD_IDENTITY_Q12: u16 = 4096;

#[derive(Clone, Copy)]
pub struct Tri {
    pub idx: [u16; 3],
    pub tex: usize,
    pub uv: [(u8, u8); 3],
    pub normal: [i8; 3],
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
        let hmd5 = data.get(0..4) == Some(b"HMD5");
        let hmd6 = data.get(0..4) == Some(b"HMD6");
        let compact = hmd4 || hmd5 || hmd6;
        let n_verts = rd_u32(data, 4) as usize;
        let n_tris = rd_u32(data, 8) as usize;
        let n_frames = (rd_u32(data, 16) as usize).max(1);
        let n_clips = if hmd3 || compact {
            (rd_u32(data, 20) as usize).max(1)
        } else {
            1
        };
        let frame_data_len = if compact {
            rd_u32(data, 24) as usize
        } else {
            0
        };
        let local_to_world_q12 = if hmd5 || hmd6 {
            let q = rd_u16(data, 28);
            if q == 0 {
                LOCAL_TO_WORLD_IDENTITY_Q12
            } else {
                q
            }
        } else {
            LOCAL_TO_WORLD_IDENTITY_Q12
        };
        let clips_off = if hmd5 || hmd6 {
            32
        } else if hmd4 {
            28
        } else if hmd3 {
            24
        } else {
            0
        };
        let frame_desc_off = if compact { clips_off + n_clips * 4 } else { 0 };
        let v_off = if compact {
            frame_desc_off + n_frames * FRAME_REC_SZ
        } else if hmd3 {
            clips_off + n_clips * 4
        } else {
            20
        };
        let frame_stride = n_verts * 6;
        let tri_off = if compact {
            v_off + frame_data_len
        } else {
            v_off + n_frames * frame_stride
        };
        let tri_sz = if hmd6 { TRI_SZ_HMD6 } else { TRI_SZ };
        let base_frame_off = if compact {
            v_off + rd_u32(data, frame_desc_off) as usize
        } else {
            v_off
        };
        Model {
            data,
            n_verts,
            n_tris,
            n_frames,
            n_clips,
            clips_off,
            frame_desc_off,
            v_off,
            frame_stride,
            tri_off,
            tri_sz,
            tri_has_normals: hmd6,
            compact_frames: compact,
            base_frame_off,
            local_to_world_q12,
        }
    }

    #[inline]
    pub fn local_to_world_q12(&self) -> u16 {
        self.local_to_world_q12
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
    pub fn frame(&self, frame: usize) -> ModelFrame<'_> {
        let f = if frame < self.n_frames { frame } else { 0 };
        let (frame_off, mode) = if self.compact_frames {
            let desc = self.frame_desc_off + f * FRAME_REC_SZ;
            (
                self.v_off + rd_u32(self.data, desc) as usize,
                self.data[desc + 4],
            )
        } else {
            (self.v_off + f * self.frame_stride, 0)
        };
        ModelFrame {
            data: self.data,
            compact_frames: self.compact_frames,
            mode,
            frame_off,
            base_frame_off: self.base_frame_off,
        }
    }

    #[inline]
    pub fn tri(&self, t: usize) -> Tri {
        let o = self.tri_off + t * self.tri_sz;
        let d = self.data;
        Tri {
            idx: [rd_u16(d, o), rd_u16(d, o + 2), rd_u16(d, o + 4)],
            tex: rd_u16(d, o + 6) as usize,
            uv: [
                (d[o + 8], d[o + 9]),
                (d[o + 10], d[o + 11]),
                (d[o + 12], d[o + 13]),
            ],
            normal: if self.tri_has_normals {
                [d[o + 14] as i8, d[o + 15] as i8, d[o + 16] as i8]
            } else {
                [0; 3]
            },
        }
    }

    #[inline]
    pub fn tri_normal(&self, t: usize) -> [i8; 3] {
        if !self.tri_has_normals {
            return [0; 3];
        }
        let o = self.tri_off + t * self.tri_sz + 14;
        let d = self.data;
        [d[o] as i8, d[o + 1] as i8, d[o + 2] as i8]
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
}

impl ModelFrame<'_> {
    #[inline]
    pub fn vert(&self, i: usize) -> Vec3I16 {
        if self.compact_frames && self.mode == FRAME_MODE_BASE_I8 {
            let base = self.base_frame_off + i * 6;
            let delta = self.frame_off + i * 3;
            Vec3I16::new(
                rd_i16(self.data, base) + self.data[delta] as i8 as i16,
                rd_i16(self.data, base + 2) + self.data[delta + 1] as i8 as i16,
                rd_i16(self.data, base + 4) + self.data[delta + 2] as i8 as i16,
            )
        } else {
            let o = self.frame_off + i * 6;
            Vec3I16::new(
                rd_i16(self.data, o),
                rd_i16(self.data, o + 2),
                rd_i16(self.data, o + 4),
            )
        }
    }
}
