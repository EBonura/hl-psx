//! The brightness curve: pure arithmetic, shared by the options (`settings`)
//! and tested on the host (`host/hl-logic-tests`).
//!
//! Level 4 (zero-based) preserves cooked lighting; lower levels darken and
//! higher levels lift mid-tones.

use crate::options_record::{BRIGHTNESS_LEVELS, DEFAULT_BRIGHTNESS};

/// Q8 blend weight from `v` toward the curve target for each level: negative
/// pulls toward `v^2` (darker), positive toward `sqrt(v)` (brighter), and zero
/// is the untouched palette. Both targets are exact at 0 and 255, so no level
/// crushes black or blows out white.
/// Levels 7 and 8 continue the bright side at the same 112 spacing as level 6.
/// The weight may exceed 256: the blend stays monotonic and reaches exactly 255
/// at v=255 for any weight up to about 512, so nothing wraps through `as u8`.
/// Past that the curve overshoots and clips, so do not extend this table
/// further without redoing that check.
pub const BRIGHTNESS_MIX: [i16; BRIGHTNESS_LEVELS as usize] =
    [-224, -168, -112, -56, 0, 112, 224, 336];

/// The default level's mix, what a fresh boot starts on.
pub const DEFAULT_MIX: i16 = BRIGHTNESS_MIX[DEFAULT_BRIGHTNESS as usize];

/// Integer square root of `n` (`n < 2^16`).
#[inline(always)]
const fn isqrt16(n: u32) -> i32 {
    psx_math::int32::isqrt_i32(n as i32)
}

/// The curve at mix `mix` (nonzero) for one 8-bit light or shade channel. Off
/// every per-frame path: the palette expansion, one call per model draw, and
/// the viewmodel table ([`vm_shade_table`]) call it. A `const fn` so the
/// default level's table is built at compile time with the same arithmetic.
#[optimize(size)]
#[inline(never)]
#[cfg_attr(target_arch = "mips", link_section = ".psx_cold.brightness")]
pub const fn bright_curve(v: u8, mix: i16) -> u8 {
    let x = v as i32;
    // sqrt(255 * x) is 255 * sqrt(x / 255) without leaving integers; the dark
    // target is the matching x^2 / 255. Both bracket x, so the blend stays in
    // range without a clamp.
    let target = if mix > 0 {
        isqrt16((x as u32) * 255)
    } else {
        (x * x + 127) / 255
    };
    let weight = mix.unsigned_abs() as i32;
    (x + (((target - x) * weight) >> 8)) as u8
}

/// The viewmodel's per-triangle shades (`vm_normal_shade` in main.rs): every
/// value it returns lies in `VM_SHADE_MIN..=VM_SHADE_MAX`.
pub const VM_SHADE_MIN: u8 = 64;
/// See [`VM_SHADE_MIN`].
pub const VM_SHADE_MAX: u8 = 144;
/// Entries of [`vm_shade_table`].
pub const VM_SHADE_LEN: usize = (VM_SHADE_MAX - VM_SHADE_MIN) as usize + 1;

/// The curve at mix `mix` of every viewmodel shade (mix 0: the shade itself).
#[optimize(size)]
pub const fn vm_shade_table(mix: i16) -> [u8; VM_SHADE_LEN] {
    let mut table = [0u8; VM_SHADE_LEN];
    let mut i = 0;
    while i < VM_SHADE_LEN {
        let v = VM_SHADE_MIN + i as u8;
        table[i] = if mix == 0 { v } else { bright_curve(v, mix) };
        i += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `settings::bright_curve` as it was before the table, verbatim but for
    /// the square root, which is the same `psx_math` call.
    fn curve_before(v: u8, mix: i16) -> u8 {
        let x = v as i32;
        let target = if mix > 0 {
            psx_math::int32::isqrt_i32(((x as u32) * 255) as i32)
        } else {
            (x * x + 127) / 255
        };
        let weight = mix.unsigned_abs() as i32;
        (x + (((target - x) * weight) >> 8)) as u8
    }

    /// `settings::bright` before the table: the gate, then the curve.
    fn bright_before(v: u8, mix: i16) -> u8 {
        if mix == 0 {
            v
        } else {
            curve_before(v, mix)
        }
    }

    #[test]
    fn the_curve_is_unchanged_at_every_level() {
        for mix in BRIGHTNESS_MIX {
            for v in 0..=255u8 {
                if mix != 0 {
                    assert_eq!(
                        bright_curve(v, mix),
                        curve_before(v, mix),
                        "v {v} mix {mix}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_viewmodel_table_is_the_curve_at_every_level_and_shade() {
        for mix in BRIGHTNESS_MIX {
            let table = vm_shade_table(mix);
            for shade in VM_SHADE_MIN..=VM_SHADE_MAX {
                assert_eq!(
                    table[(shade - VM_SHADE_MIN) as usize],
                    bright_before(shade, mix),
                    "shade {shade} mix {mix}"
                );
            }
        }
    }

    #[test]
    fn the_compile_time_table_is_the_default_levels() {
        const AT_BUILD: [u8; VM_SHADE_LEN] = vm_shade_table(DEFAULT_MIX);
        assert_eq!(
            AT_BUILD,
            vm_shade_table(BRIGHTNESS_MIX[DEFAULT_BRIGHTNESS as usize])
        );
        assert!(BRIGHTNESS_MIX.len() == BRIGHTNESS_LEVELS as usize);
    }
}
