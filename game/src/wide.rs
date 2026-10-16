//! The one place this crate forms a product or a sum in 64 bits.
//!
//! The R3000 has `mult`/`multu`, which write a 64-bit HI:LO pair in one
//! instruction, and 64-bit add, subtract, compare and shift are a few 32-bit
//! instructions each: none of that calls a helper. Where a quantity is
//! inherently that wide (a dot product of two world-sized vectors, a squared
//! distance, the cross product of two screen edges) the call sites hold a
//! [`Wide`] and never name a 64-bit type, so `tools/no64_guard.rs` can fail the
//! build on any other `i64`/`u64` in guest source. This file is the single
//! allow-listed exception, in the same spirit as the marked primitives in the
//! SDK's `psx_math::int32` (`mul_shr_i32`, `mul_div_i32`): use those first, and
//! use [`Wide`] only when the value really needs 64 bits. There is no division
//! here: a divide goes through `mul_div_i32` / `mul_div_u32`, never `/` on a
//! 64-bit value, which is the one operation that links a software helper.
//!
//! Arithmetic wraps like the release-mode `i64` it replaces.
#![allow(dead_code)]

use core::cmp::Ordering;
use core::ops::{Add, Neg, Shl, Shr, Sub};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Wide(i64);

impl Wide {
    pub const ZERO: Wide = Wide(0);
    pub const MAX: Wide = Wide(i64::MAX);

    /// A 32-bit value widened (sign-extending).
    #[inline(always)]
    pub const fn of(a: i32) -> Wide {
        Wide(a as i64)
    }
    /// A 32-bit unsigned value widened.
    #[inline(always)]
    pub const fn of_u(a: u32) -> Wide {
        Wide(a as i64)
    }
    /// The full signed product: one `mult`.
    #[inline(always)]
    pub const fn mul(a: i32, b: i32) -> Wide {
        Wide(a as i64 * b as i64)
    }
    /// The full unsigned product: one `multu`.
    #[inline(always)]
    pub const fn mul_u(a: u32, b: u32) -> Wide {
        Wide(a as i64 * b as i64)
    }
    /// `self + a * b`.
    #[inline(always)]
    pub const fn mul_add(self, a: i32, b: i32) -> Wide {
        Wide(self.0.wrapping_add(a as i64 * b as i64))
    }
    /// `self - a * b`.
    #[inline(always)]
    pub const fn mul_sub(self, a: i32, b: i32) -> Wide {
        Wide(self.0.wrapping_sub(a as i64 * b as i64))
    }
    /// `self + o`, usable in `const fn`.
    #[inline(always)]
    pub const fn add_w(self, o: Wide) -> Wide {
        Wide(self.0.wrapping_add(o.0))
    }
    /// `self - o`, usable in `const fn`.
    #[inline(always)]
    pub const fn sub_w(self, o: Wide) -> Wide {
        Wide(self.0.wrapping_sub(o.0))
    }
    /// `self * o` (low 64 bits).
    #[inline(always)]
    pub const fn mul_w(self, o: Wide) -> Wide {
        Wide(self.0.wrapping_mul(o.0))
    }
    /// `self * k`.
    #[inline(always)]
    pub const fn times(self, k: i32) -> Wide {
        Wide(self.0.wrapping_mul(k as i64))
    }
    /// `self + k` for a 32-bit `k`.
    #[inline(always)]
    pub const fn plus(self, k: i32) -> Wide {
        Wide(self.0.wrapping_add(k as i64))
    }
    /// The squared length of a vector.
    #[inline(always)]
    pub const fn dot3(a: [i32; 3], b: [i32; 3]) -> Wide {
        Wide(
            (a[0] as i64 * b[0] as i64)
                .wrapping_add(a[1] as i64 * b[1] as i64)
                .wrapping_add(a[2] as i64 * b[2] as i64),
        )
    }
    /// `(self >> n)` truncated to its low 32 bits (`as i32`), for `n < 64`.
    #[inline(always)]
    pub const fn shr_i32(self, n: u32) -> i32 {
        (self.0 >> n) as i32
    }
    /// The low 32 bits (`as i32`).
    #[inline(always)]
    pub const fn low(self) -> i32 {
        self.0 as i32
    }
    /// The value clamped into `i32`.
    #[inline(always)]
    pub const fn saturate_i32(self) -> i32 {
        if self.0 > i32::MAX as i64 {
            i32::MAX
        } else if self.0 < i32::MIN as i64 {
            i32::MIN
        } else {
            self.0 as i32
        }
    }
    /// The value as `i32` if it fits.
    #[inline(always)]
    pub const fn to_i32(self) -> Option<i32> {
        if self.0 >= i32::MIN as i64 && self.0 <= i32::MAX as i64 {
            Some(self.0 as i32)
        } else {
            None
        }
    }
    #[inline(always)]
    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }
    #[inline(always)]
    pub const fn is_positive(self) -> bool {
        self.0 > 0
    }
    #[inline(always)]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
    #[inline(always)]
    pub const fn abs(self) -> Wide {
        Wide(self.0.wrapping_abs())
    }
    #[inline(always)]
    pub fn max(self, o: Wide) -> Wide {
        if self.0 >= o.0 {
            self
        } else {
            o
        }
    }
    #[inline(always)]
    pub fn min(self, o: Wide) -> Wide {
        if self.0 <= o.0 {
            self
        } else {
            o
        }
    }
    /// Clamp to `lo..=hi`.
    #[inline(always)]
    pub fn clamp(self, lo: Wide, hi: Wide) -> Wide {
        self.max(lo).min(hi)
    }
    /// Compare with a 32-bit value.
    #[inline(always)]
    pub fn cmp_i32(self, k: i32) -> Ordering {
        self.0.cmp(&(k as i64))
    }
    /// `floor(sqrt(self))`, 0 for a negative value: the SDK's 64-bit root.
    #[inline]
    pub fn isqrt(self) -> u32 {
        if self.0 <= 0 {
            0
        } else {
            psx_math::int32::isqrt_u64(self.0 as u64)
        }
    }
    /// The 32-bit quotient `self / d` truncated toward zero, when `self`'s
    /// magnitude is below `d * 2^32`. One `divu` or two (see `div_u64_by_u32`).
    pub fn div_u32(self, d: u32) -> u32 {
        let v = self.0 as u64;
        psx_math::int32::div_u64_by_u32((v >> 32) as u32, v as u32, d)
    }
}

/// A set of up to 64 flags in two words (what a `u64` bit mask was used for).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mask64([u32; 2]);

impl Mask64 {
    pub const EMPTY: Mask64 = Mask64([0; 2]);
    #[inline(always)]
    pub fn set(&mut self, bit: usize) {
        self.0[bit >> 5] |= 1 << (bit & 31);
    }
    #[inline(always)]
    pub fn clear(&mut self, bit: usize) {
        self.0[bit >> 5] &= !(1 << (bit & 31));
    }
    #[inline(always)]
    pub fn assign(&mut self, bit: usize, value: bool) {
        if value {
            self.set(bit);
        } else {
            self.clear(bit);
        }
    }
    /// Is `bit` set? A bit past 63 is not.
    #[inline(always)]
    pub fn test(&self, bit: usize) -> bool {
        bit < 64 && self.0[bit >> 5] >> (bit & 31) & 1 != 0
    }
}

/// The unsigned counterpart: a `u32 * u32` product, which needs all 64 bits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct WideU(u64);

impl WideU {
    #[inline(always)]
    pub const fn mul(a: u32, b: u32) -> WideU {
        WideU(a as u64 * b as u64)
    }
    #[inline(always)]
    pub const fn of(a: u32) -> WideU {
        WideU(a as u64)
    }
    #[inline(always)]
    pub const fn add(self, o: WideU) -> WideU {
        WideU(self.0.wrapping_add(o.0))
    }
    #[inline(always)]
    pub const fn mul_add(self, a: u32, b: u32) -> WideU {
        WideU(self.0.wrapping_add(a as u64 * b as u64))
    }
    #[inline(always)]
    pub const fn saturating_add(self, o: WideU) -> WideU {
        WideU(self.0.saturating_add(o.0))
    }
    /// `self * k`, wrapping.
    #[inline(always)]
    pub const fn times(self, k: u32) -> WideU {
        WideU(self.0.wrapping_mul(k as u64))
    }
    /// `floor(sqrt(self))`.
    #[inline]
    pub const fn isqrt(self) -> u32 {
        psx_math::int32::isqrt_u64(self.0)
    }
    /// The value as `u32` if it fits.
    #[inline(always)]
    pub const fn to_u32(self) -> Option<u32> {
        if self.0 <= u32::MAX as u64 {
            Some(self.0 as u32)
        } else {
            None
        }
    }
    #[inline(always)]
    pub const fn saturate_u32(self) -> u32 {
        if self.0 > u32::MAX as u64 {
            u32::MAX
        } else {
            self.0 as u32
        }
    }
    /// `self <= k * k` for a 32-bit `k`.
    #[inline(always)]
    pub const fn le_square(self, k: u32) -> bool {
        self.0 <= k as u64 * k as u64
    }
}

impl Add for Wide {
    type Output = Wide;
    #[inline(always)]
    fn add(self, o: Wide) -> Wide {
        Wide(self.0.wrapping_add(o.0))
    }
}
impl Sub for Wide {
    type Output = Wide;
    #[inline(always)]
    fn sub(self, o: Wide) -> Wide {
        Wide(self.0.wrapping_sub(o.0))
    }
}
impl Neg for Wide {
    type Output = Wide;
    #[inline(always)]
    fn neg(self) -> Wide {
        Wide(self.0.wrapping_neg())
    }
}
impl Shr<u32> for Wide {
    type Output = Wide;
    #[inline(always)]
    fn shr(self, n: u32) -> Wide {
        Wide(self.0 >> n)
    }
}
impl Shl<u32> for Wide {
    type Output = Wide;
    #[inline(always)]
    fn shl(self, n: u32) -> Wide {
        Wide(self.0.wrapping_shl(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_matches_i64() {
        let vals = [
            i32::MIN,
            i32::MIN + 1,
            -65_536,
            -1,
            0,
            1,
            4096,
            65_537,
            i32::MAX,
        ];
        for &a in &vals {
            for &b in &vals {
                assert_eq!(Wide::mul(a, b).0, a as i64 * b as i64);
                assert_eq!(
                    Wide::mul_u(a as u32, b as u32).0,
                    a as u32 as i64 * b as u32 as i64
                );
                assert_eq!(Wide::of(a).mul_add(a, b).0, a as i64 + a as i64 * b as i64);
                assert_eq!((Wide::of(a) - Wide::of(b)).0, a as i64 - b as i64);
                assert_eq!(
                    Wide::mul(a, b).shr_i32(12),
                    ((a as i64 * b as i64) >> 12) as i32
                );
                assert_eq!(
                    Wide::mul(a, b).saturate_i32(),
                    (a as i64 * b as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32
                );
            }
        }
        // The renderer's guard band is 1000 by 500 px: its cross products fit i32.
        let (w, h) = (1000i32, 500i32);
        assert!(((w * h) as i64) * 2 < i32::MAX as i64);
        let mut mask = Mask64::EMPTY;
        for bit in [0usize, 1, 31, 32, 33, 63] {
            mask.set(bit);
            assert!(mask.test(bit));
        }
        assert!(!mask.test(2) && !mask.test(64) && !mask.test(200));
        mask.assign(32, false);
        mask.clear(0);
        assert!(!mask.test(32) && !mask.test(0) && mask.test(33) && mask.test(63));
        assert_eq!(WideU::mul(u32::MAX, u32::MAX).saturate_u32(), u32::MAX);
        assert_eq!(WideU::mul(65_535, 65_537).to_u32(), Some(u32::MAX));
        assert!(WideU::mul(3, 4).le_square(4) && !WideU::mul(5, 4).le_square(4));
        assert_eq!(Wide::of(-5).isqrt(), 0);
        assert_eq!(Wide::mul(i32::MAX, i32::MAX).isqrt(), i32::MAX as u32);
        assert_eq!(Wide::dot3([3, 4, 12], [3, 4, 12]).isqrt(), 13);
    }
}
