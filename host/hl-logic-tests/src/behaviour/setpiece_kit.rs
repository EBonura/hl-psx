//! Scripted-world pieces the set-piece behaviour tests share: the random
//! source, a slab test for the player's box and flat walls to trace against.

use crate::setpiece_math::TraceHit;
use psx_math::int32::mul_div_i32;

/// The game's shared impact generator: the 32-bit glibc LCG, high half
/// folded over the low half, reduced by remainder. Goldens seed it with the
/// game's own seed so the recorded draws are the ones a real run makes.
#[derive(Clone, Copy, Debug)]
pub struct Lcg(pub u32);

impl Lcg {
    /// The seed the game starts its impact generator with.
    pub const GAME_SEED: u32 = 0x484c_5058;

    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (self.0 ^ (self.0 >> 16)) % n
    }
}

/// How a scripted world answers "a random integer below n".
#[derive(Clone, Copy, Debug)]
pub enum Pick {
    /// Always 0.
    Low,
    /// Always n - 1.
    High,
    /// The game's generator.
    Game(Lcg),
}

impl Pick {
    pub fn below(&mut self, n: u32) -> u32 {
        match self {
            Pick::Low => 0,
            Pick::High => n.saturating_sub(1),
            Pick::Game(r) => r.below(n),
        }
    }
}

/// Entry fraction (q12 of `p1 -> p2`) of a segment into a box, as the game
/// computes it for its player box.
pub fn segment_box_frac(p1: [i32; 3], p2: [i32; 3], mins: [i32; 3], maxs: [i32; 3]) -> Option<i32> {
    let (mut lo, mut hi) = (0i32, 4096i32);
    for axis in 0..3 {
        let d = p2[axis] - p1[axis];
        if d == 0 {
            if p1[axis] < mins[axis] || p1[axis] > maxs[axis] {
                return None;
            }
        } else {
            let a = mul_div_i32(mins[axis] - p1[axis], 4096, d);
            let b = mul_div_i32(maxs[axis] - p1[axis], 4096, d);
            lo = lo.max(a.min(b));
            hi = hi.min(a.max(b));
            if lo > hi {
                return None;
            }
        }
    }
    Some(lo)
}

/// An infinite flat wall: the plane where coordinate `axis` equals `at`.
#[derive(Clone, Copy, Debug)]
pub struct Wall {
    pub axis: usize,
    pub at: i32,
}

/// The nearest wall `from -> to` crosses, as a trace hit.
pub fn trace_walls(walls: &[Wall], from: [i32; 3], to: [i32; 3]) -> Option<TraceHit> {
    let mut best: Option<TraceHit> = None;
    for w in walls {
        let (a, b) = (from[w.axis], to[w.axis]);
        if (a < w.at) == (b < w.at) || a == b {
            continue;
        }
        let frac = (w.at - a) * 4096 / (b - a);
        if best.is_some_and(|h| h.frac <= frac) {
            continue;
        }
        let pos = [0, 1, 2].map(|k| from[k] + (((to[k] - from[k]) * frac) >> 12));
        let mut normal = [0; 3];
        normal[w.axis] = if b > a { -4096 } else { 4096 };
        best = Some(TraceHit { frac, pos, normal });
    }
    best
}
