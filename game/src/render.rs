//! Near-plane + guard-band clipping and software re-projection, ported from the
//! proven oot-psx `room.rs`. Triangles that straddle the camera near plane are
//! clipped in view space (Sutherland-Hodgman) and re-projected in software
//! instead of being dropped (which made geometry pop out near walls).
//!
//! Fixed-point only: the divide is always `(num << 12) / den` on i32 (the
//! mipsel target miscompiles `i64 / runtime`). View/screen coords stay well
//! under 2^19 so `num << 12` fits i32.

/// View-space near plane (world units). Verts closer than this are clipped.
pub const NEAR_Z: i32 = 16;
/// Screen centre (matches `set_screen_offset(160<<16, 120<<16)`).
pub const OFX: i32 = 160;
pub const OFY: i32 = 120;
/// Software projection focal length (matches `set_projection_plane(H_PROJ)`).
pub const SOFT_H: i32 = 160;
/// Guard band: keep screen coords within the GPU's span limits (±1023 / 511).
const GX0: i32 = -340;
const GX1: i32 = 660;
const GY0: i32 = -130;
const GY1: i32 = 370;

/// View-space vertex carried through near clipping (position + interpolated
/// per-corner colour and UV).
#[derive(Clone, Copy)]
pub struct CVert {
    pub v: [i32; 3],
    pub rgb: (i32, i32, i32),
    pub uv: (i32, i32),
}

/// Screen-space vertex (true, unclamped coords) carried through the guard-band
/// clip and into the emitter.
#[derive(Clone, Copy)]
pub struct SVert {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub rgb: (i32, i32, i32),
    pub uv: (i32, i32),
}

pub const EMPTY_CV: CVert = CVert { v: [0; 3], rgb: (0, 0, 0), uv: (0, 0) };
pub const EMPTY_SV: SVert = SVert { x: 0, y: 0, z: 0, rgb: (0, 0, 0), uv: (0, 0) };

#[inline]
fn t_q12(num: i32, den: i32) -> i32 {
    if den == 0 {
        return 0;
    }
    ((num << 12) / den).clamp(0, 4096)
}

#[inline]
fn mix(x0: i32, x1: i32, t: i32) -> i32 {
    x0 + (((x1 - x0) * t) >> 12)
}

fn lerp_cv_near(a: &CVert, b: &CVert) -> CVert {
    let t = t_q12(NEAR_Z - a.v[2], b.v[2] - a.v[2]);
    CVert {
        v: [mix(a.v[0], b.v[0], t), mix(a.v[1], b.v[1], t), NEAR_Z],
        rgb: (mix(a.rgb.0, b.rgb.0, t), mix(a.rgb.1, b.rgb.1, t), mix(a.rgb.2, b.rgb.2, t)),
        uv: (mix(a.uv.0, b.uv.0, t), mix(a.uv.1, b.uv.1, t)),
    }
}

/// Clip a triangle against `z >= NEAR_Z` in view space. Writes up to 4 verts.
pub fn near_clip(cv: &[CVert; 3], out: &mut [CVert; 4]) -> usize {
    let mut m = 0;
    for i in 0..3 {
        let cur = cv[i];
        let prev = cv[(i + 2) % 3];
        let cur_in = cur.v[2] >= NEAR_Z;
        let prev_in = prev.v[2] >= NEAR_Z;
        if cur_in != prev_in && m < 4 {
            out[m] = lerp_cv_near(&prev, &cur);
            m += 1;
        }
        if cur_in && m < 4 {
            out[m] = cur;
            m += 1;
        }
    }
    m
}

/// Project a clipped view-space vertex to true screen coords (one reciprocal
/// `H/z` in Q12 shared by X and Y).
pub fn project_soft(cv: &CVert) -> SVert {
    let z = cv.v[2].max(NEAR_Z);
    let inv = (SOFT_H << 12) / z; // Q12 H/z
    SVert {
        x: ((cv.v[0] * inv) >> 12) + OFX,
        y: ((cv.v[1] * inv) >> 12) + OFY,
        z: cv.v[2],
        rgb: cv.rgb,
        uv: cv.uv,
    }
}

/// Screen-space back-face test (cross product; >= 0 = back-facing).
#[inline]
pub fn back_facing(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> bool {
    (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1) >= 0
}

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    X,
    Y,
}

fn lerp_sv(a: &SVert, b: &SVert, axis: Axis, bound: i32) -> SVert {
    let (ca, cb) = if axis == Axis::X { (a.x, b.x) } else { (a.y, b.y) };
    let t = t_q12(bound - ca, cb - ca);
    SVert {
        x: mix(a.x, b.x, t),
        y: mix(a.y, b.y, t),
        z: mix(a.z, b.z, t),
        rgb: (mix(a.rgb.0, b.rgb.0, t), mix(a.rgb.1, b.rgb.1, t), mix(a.rgb.2, b.rgb.2, t)),
        uv: (mix(a.uv.0, b.uv.0, t), mix(a.uv.1, b.uv.1, t)),
    }
}

fn clip_edge(inp: &[SVert], n: usize, out: &mut [SVert; 8], axis: Axis, bound: i32, keep_ge: bool) -> usize {
    let coord = |s: &SVert| if axis == Axis::X { s.x } else { s.y };
    let inside = |s: &SVert| if keep_ge { coord(s) >= bound } else { coord(s) <= bound };
    let mut m = 0;
    for i in 0..n {
        let cur = inp[i];
        let prev = inp[(i + n - 1) % n];
        if inside(&cur) != inside(&prev) && m < 8 {
            out[m] = lerp_sv(&prev, &cur, axis, bound);
            m += 1;
        }
        if inside(&cur) && m < 8 {
            out[m] = cur;
            m += 1;
        }
    }
    m
}

/// Clip a convex screen polygon to the guard band. Returns vertices in `out`.
pub fn guard_clip(poly: &[SVert], n: usize, out: &mut [SVert; 8]) -> usize {
    // Fast path: entirely inside.
    if (0..n).all(|i| poly[i].x >= GX0 && poly[i].x <= GX1 && poly[i].y >= GY0 && poly[i].y <= GY1) {
        for i in 0..n {
            out[i] = poly[i];
        }
        return n;
    }
    let mut a = [EMPTY_SV; 8];
    for i in 0..n.min(8) {
        a[i] = poly[i];
    }
    let mut na = n.min(8);
    let mut b = [EMPTY_SV; 8];
    na = clip_edge(&a, na, &mut b, Axis::X, GX0, true);
    if na < 3 {
        return 0;
    }
    na = clip_edge(&b, na, &mut a, Axis::X, GX1, false);
    if na < 3 {
        return 0;
    }
    na = clip_edge(&a, na, &mut b, Axis::Y, GY0, true);
    if na < 3 {
        return 0;
    }
    na = clip_edge(&b, na, out, Axis::Y, GY1, false);
    if na < 3 {
        0
    } else {
        na
    }
}
