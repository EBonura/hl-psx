//! Decoding of GP0 polygon packets and rasterisation with perspective-correct depth.

pub const WIDTH: i32 = 320;
pub const HEIGHT: i32 = 240;

/// A decoded screen polygon: one triangle or a quad drawn as (0,1,2) then (1,2,3).
#[derive(Clone, Copy, Debug)]
pub struct Poly {
    pub verts: [(i32, i32); 4],
    pub n: usize,
    pub semi: bool,
}

fn sext11(v: u32) -> i32 {
    ((v & 0x7ff) as i32) << 21 >> 21
}

/// Decode a packet's words (after the tag). Returns `None` for anything that is not a polygon.
pub fn decode(words: &[u32]) -> Option<Poly> {
    let mut i = 0;
    while i < words.len() && (0xe0..=0xef).contains(&(words[i] >> 24)) {
        i += 1;
    }
    let cmd = words.get(i)? >> 24;
    if cmd & 0xe0 != 0x20 {
        return None;
    }
    let gouraud = cmd & 0x10 != 0;
    let textured = cmd & 0x04 != 0;
    let n = if cmd & 0x08 != 0 { 4 } else { 3 };
    let mut verts = [(0, 0); 4];
    i += 1;
    for (k, vert) in verts.iter_mut().enumerate().take(n) {
        if k > 0 && gouraud {
            i += 1;
        }
        let w = *words.get(i)?;
        *vert = (sext11(w), sext11(w >> 16));
        i += 1;
        if textured {
            i += 1;
        }
    }
    Some(Poly {
        verts,
        n,
        semi: cmd & 0x02 != 0,
    })
}

fn edge(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> f64 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

/// Rasterise one triangle with a top-left fill rule (either winding) and call `emit(x, y, z)` for
/// every covered pixel of the viewport. Depth is perspective correct: 1/z is affine on screen.
pub fn triangle(v: [(i32, i32); 3], z: [f32; 3], mut emit: impl FnMut(i32, i32, f32)) {
    let p = v.map(|q| (q.0 as f64, q.1 as f64));
    let area = edge(p[0], p[1], p[2]);
    if area == 0.0 {
        return;
    }
    let inv_z = z.map(|d| 1.0 / f64::from(d.max(1.0)));
    let min_x = v.iter().map(|q| q.0).min().unwrap_or(0).max(0);
    let max_x = v.iter().map(|q| q.0).max().unwrap_or(0).min(WIDTH - 1);
    let min_y = v.iter().map(|q| q.1).min().unwrap_or(0).max(0);
    let max_y = v.iter().map(|q| q.1).max().unwrap_or(0).min(HEIGHT - 1);
    let sign = if area > 0.0 { 1.0 } else { -1.0 };
    let top_left = |a: (f64, f64), b: (f64, f64)| {
        let (dx, dy) = ((b.0 - a.0) * sign, (b.1 - a.1) * sign);
        dy < 0.0 || (dy == 0.0 && dx > 0.0)
    };
    let owns = [
        top_left(p[1], p[2]),
        top_left(p[2], p[0]),
        top_left(p[0], p[1]),
    ];
    let a = area * sign;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let c = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let w = [
                edge(p[1], p[2], c) * sign,
                edge(p[2], p[0], c) * sign,
                edge(p[0], p[1], c) * sign,
            ];
            if !(0..3).all(|k| w[k] > 0.0 || (w[k] == 0.0 && owns[k])) {
                continue;
            }
            let inv = (w[0] * inv_z[0] + w[1] * inv_z[1] + w[2] * inv_z[2]) / a;
            emit(x, y, (1.0 / inv.max(1e-9)) as f32);
        }
    }
}

/// Rasterise a polygon (a quad as two triangles, as the GPU does).
pub fn polygon(poly: &Poly, z: [f32; 4], mut emit: impl FnMut(i32, i32, f32)) {
    triangle(
        [poly.verts[0], poly.verts[1], poly.verts[2]],
        [z[0], z[1], z[2]],
        &mut emit,
    );
    if poly.n == 4 {
        triangle(
            [poly.verts[1], poly.verts[2], poly.verts[3]],
            [z[1], z[2], z[3]],
            &mut emit,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(x: i32, y: i32) -> u32 {
        (x as u32 & 0x7ff) | ((y as u32 & 0x7ff) << 16)
    }

    #[test]
    fn decodes_a_gouraud_textured_quad_after_its_window_word() {
        // GP0 E2 window word, then 0x3c quad: colour|cmd, v0, uv0, colour, v1, uv1, ...
        let words = [
            0xe200_0000,
            0x3c80_8080,
            word(10, 20),
            0,
            0x8080_80,
            word(30, 20),
            0,
            0x8080_80,
            word(10, 40),
            0,
            0x8080_80,
            word(30, 40),
            0,
        ];
        let poly = decode(&words).expect("polygon");
        assert_eq!(poly.n, 4);
        assert_eq!(poly.verts[0], (10, 20));
        assert_eq!(poly.verts[3], (30, 40));
        assert!(!poly.semi);
    }

    #[test]
    fn sign_extends_eleven_bit_coordinates() {
        let words = [0x2000_0000, word(-5, -7), word(5, -7), word(0, 9)];
        let poly = decode(&words).expect("polygon");
        assert_eq!(poly.verts[0], (-5, -7));
    }

    #[test]
    fn a_triangle_covers_its_area_once_and_depth_is_perspective_correct() {
        let mut covered = 0;
        let mut nearest = f32::MAX;
        let mut farthest = 0.0f32;
        triangle(
            [(0, 0), (16, 0), (0, 16)],
            [100.0, 100.0, 400.0],
            |_, _, z| {
                covered += 1;
                nearest = nearest.min(z);
                farthest = farthest.max(z);
            },
        );
        // Half of a 16x16 square, up to the fill rule's edge pixels.
        assert!((120..=136).contains(&covered), "{covered}");
        assert!(nearest >= 100.0 && farthest <= 400.0);
        assert!(farthest > 200.0, "the far corner must pull the depth out");
    }

    #[test]
    fn shared_edges_of_two_triangles_are_not_covered_twice() {
        let mut hits = std::collections::HashMap::new();
        for tri in [[(0, 0), (8, 0), (0, 8)], [(8, 0), (8, 8), (0, 8)]] {
            triangle(tri, [1.0; 3], |x, y, _| {
                *hits.entry((x, y)).or_insert(0) += 1
            });
        }
        assert!(hits.values().all(|&n| n == 1));
        assert_eq!(hits.len(), 64);
    }
}
