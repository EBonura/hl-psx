//! GoldSrc side of the HMA1 animation cook (opt-in with `HMA1=1`).
//!
//! `cook_mdl` captures every requested sequence's local bone transforms at
//! every source frame; this module turns them into `psx_anim_cook` clip
//! sources in cooked space (HL axes permuted to [x, z, y], scaled by the
//! model's vertex scale), with GoldSrc's own quaternion slerp between source
//! frames and the cooker's floor anchoring applied to root translation.

use psx_anim_cook::{ClipSource, Mat34};

/// Local transforms of every bone at every source frame, HL space.
pub struct CapturedClip {
    pub key: usize,
    pub looping: bool,
    /// [frame][bone] = (quaternion xyzw, position)
    pub frames: Vec<Vec<([f32; 4], [f32; 3])>>,
    /// Cooked-unit floor shift per source frame (subtracted from root Y).
    pub floor_shift: Vec<f32>,
}

pub struct CookedClip<'a> {
    pub clip: &'a CapturedClip,
    pub parents: &'a [i32],
    pub scale: f32,
}

fn slerp(p: [f32; 4], q0: [f32; 4], t: f32) -> [f32; 4] {
    // GoldSrc QuaternionSlerp (mathlib.c), as StudioCalcRotations uses it.
    let mut q = q0;
    let a: f32 = (0..4).map(|i| (p[i] - q[i]) * (p[i] - q[i])).sum();
    let b: f32 = (0..4).map(|i| (p[i] + q[i]) * (p[i] + q[i])).sum();
    if a > b {
        q = [-q[0], -q[1], -q[2], -q[3]];
    }
    let cosom: f32 = (0..4).map(|i| p[i] * q[i]).sum();
    let (sp, sq) = if 1.0 + cosom > 1e-6 {
        if 1.0 - cosom > 1e-6 {
            let omega = cosom.acos();
            let sinom = omega.sin();
            (((1.0 - t) * omega).sin() / sinom, (t * omega).sin() / sinom)
        } else {
            (1.0 - t, t)
        }
    } else {
        let qq = [-q[1], q[0], -q[3], q[2]];
        let sp = ((1.0 - t) * 0.5 * core::f32::consts::PI).sin();
        let sq = (t * 0.5 * core::f32::consts::PI).sin();
        return [
            sp * p[0] + sq * qq[0],
            sp * p[1] + sq * qq[1],
            sp * p[2] + sq * qq[2],
            qq[3],
        ];
    };
    [
        sp * p[0] + sq * q[0],
        sp * p[1] + sq * q[1],
        sp * p[2] + sq * q[2],
        sp * p[3] + sq * q[3],
    ]
}

fn quat_mat(q: [f32; 4]) -> [[f64; 3]; 3] {
    let (x, y, z, w) = (q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

impl ClipSource for CookedClip<'_> {
    fn key(&self) -> usize {
        self.clip.key
    }
    fn n_int(&self) -> usize {
        self.clip.frames.len().saturating_sub(1).max(1)
    }
    fn looping(&self) -> bool {
        self.clip.looping
    }
    fn local_at(&self, pos: f64) -> Vec<Mat34> {
        let n = self.clip.frames.len();
        let pos = pos.clamp(0.0, (n.saturating_sub(1)) as f64);
        let f0 = (pos.floor() as usize).min(n - 1);
        let f1 = (f0 + 1).min(n - 1);
        let t = (pos - f0 as f64) as f32;
        let shift =
            self.clip.floor_shift[f0] + (self.clip.floor_shift[f1] - self.clip.floor_shift[f0]) * t;
        let perm = [0usize, 2, 1];
        let s = self.scale as f64;
        (0..self.parents.len())
            .map(|b| {
                let (qa, pa) = self.clip.frames[f0][b];
                let (qb, pb) = self.clip.frames[f1][b];
                let r = quat_mat(slerp(qa, qb, t));
                let p = [
                    pa[0] + (pb[0] - pa[0]) * t,
                    pa[1] + (pb[1] - pa[1]) * t,
                    pa[2] + (pb[2] - pa[2]) * t,
                ];
                let mut rc = [[0.0; 3]; 3];
                for i in 0..3 {
                    for j in 0..3 {
                        rc[i][j] = r[perm[i]][perm[j]];
                    }
                }
                let mut tc = [p[0] as f64 * s, p[2] as f64 * s, p[1] as f64 * s];
                if self.parents[b] < 0 {
                    tc[1] -= shift as f64;
                }
                (rc, tc)
            })
            .collect()
    }
}

/// The studio mouth controller as the tracks apply it: a rotation of the jaw
/// bone's local frame by the controller's full travel (`end - start`; the
/// captured frames already hold `start`). GoldSrc adds the controller to one
/// Euler angle and builds Rz * Ry * Rx, so a Z controller turns before the
/// local rotation and an X controller after it; Y cannot be factored out and
/// is left closed (no shipped model uses it).
pub fn jaw_record(
    mouth: &crate::MdlMouthController,
    hma_bones: &[usize],
    path: &str,
) -> Option<psx_anim_cook::JawRecord> {
    let bone = hma_bones.iter().position(|&b| b == mouth.bone)?;
    let (axis, post) = match mouth.kind & 0x7fff {
        0x0008 => (0usize, true),
        0x0020 => (2usize, false),
        other => {
            eprintln!("[hma1] {path}: mouth controller type {other:#x} is not a Z or X rotation; mouth stays closed");
            return None;
        }
    };
    let (sn, cs) = ((mouth.end - mouth.start) as f64).to_radians().sin_cos();
    let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
    let mut r = [[0.0f64; 3]; 3];
    r[axis][axis] = 1.0;
    r[a][a] = cs;
    r[b][b] = cs;
    r[a][b] = -sn;
    r[b][a] = sn;
    // HL axes to cooked [x, z, y]
    let perm = [0usize, 2, 1];
    let rc: [[f64; 3]; 3] = core::array::from_fn(|i| core::array::from_fn(|j| r[perm[i]][perm[j]]));
    Some(psx_anim_cook::JawRecord {
        bone: bone as u8,
        post,
        open: psx_anim_cook::quat_q12(&rc),
    })
}
