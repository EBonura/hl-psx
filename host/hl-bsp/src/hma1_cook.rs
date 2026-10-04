//! GoldSrc side of the HMA1 animation cook (opt-in with `HMA1=1`).
//!
//! `cook_mdl` captures every requested sequence's local bone transforms at
//! every source frame; this module turns them into `psx_anim_cook` clip
//! sources in cooked space (HL axes permuted to [x, z, y], scaled by the
//! model's vertex scale), with a quaternion slerp between source
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

/// Blend two unit quaternions at constant angular speed along the shorter
/// arc: `t` = 0 gives `p`, `t` = 1 gives `q`. Nearly parallel inputs blend
/// linearly (then renormalise), where the arc weights lose precision.
fn slerp(p: [f32; 4], q: [f32; 4], t: f32) -> [f32; 4] {
    let mut cos: f32 = (0..4).map(|i| p[i] * q[i]).sum();
    // `q` and `-q` are the same orientation; use the one on `p`'s side.
    let side = if cos < 0.0 { -1.0 } else { 1.0 };
    cos *= side;
    let nearly_parallel = cos > 0.9995;
    let (from_p, from_q) = if nearly_parallel {
        (1.0 - t, t)
    } else {
        let angle = cos.acos();
        let sin = angle.sin();
        (((1.0 - t) * angle).sin() / sin, (t * angle).sin() / sin)
    };
    let mut out: [f32; 4] = std::array::from_fn(|i| from_p * p[i] + from_q * side * q[i]);
    if nearly_parallel {
        let len = out.iter().map(|c| c * c).sum::<f32>().sqrt();
        out = out.map(|c| c / len);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::slerp;

    const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    /// Turn of `degrees` about Z.
    fn about_z(degrees: f32) -> [f32; 4] {
        let half = degrees.to_radians() * 0.5;
        [0.0, 0.0, half.sin(), half.cos()]
    }

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    fn length(q: [f32; 4]) -> f32 {
        q.iter().map(|c| c * c).sum::<f32>().sqrt()
    }

    #[test]
    fn slerp_hits_both_endpoints() {
        let q = about_z(70.0);
        assert!(close(slerp(IDENTITY, q, 0.0), IDENTITY));
        assert!(close(slerp(IDENTITY, q, 1.0), q));
    }

    #[test]
    fn slerp_halfway_through_a_quarter_turn_is_an_eighth_turn() {
        assert!(close(slerp(IDENTITY, about_z(90.0), 0.5), about_z(45.0)));
        // A third of the way.
        assert!(close(
            slerp(IDENTITY, about_z(90.0), 1.0 / 3.0),
            about_z(30.0)
        ));
    }

    #[test]
    fn slerp_takes_the_shorter_arc_whichever_sign_the_target_has() {
        let q = about_z(90.0);
        let flipped = q.map(|c| -c);
        let a = slerp(IDENTITY, q, 0.5);
        let b = slerp(IDENTITY, flipped, 0.5);
        assert!(close(a, b));
    }

    #[test]
    fn slerp_of_opposite_signs_stays_put_and_unit_length() {
        let opposite = IDENTITY.map(|c| -c);
        for t in [0.0, 0.25, 0.5, 1.0] {
            let r = slerp(IDENTITY, opposite, t);
            assert!((length(r) - 1.0).abs() < 1e-5, "t {t}");
            assert!(close(r, IDENTITY), "t {t}");
        }
    }

    #[test]
    fn slerp_of_nearly_identical_inputs_stays_unit_length() {
        let q = about_z(0.01);
        let r = slerp(IDENTITY, q, 0.5);
        assert!((length(r) - 1.0).abs() < 1e-6);
        assert!(close(r, about_z(0.005)));
    }
}
