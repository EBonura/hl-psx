//! Pure rules of lane L5's two world effects, kept free of statics so the host
//! test runner can check them: the direction table a painted decal's axes are
//! cooked into, and how a light style's level becomes a colour change.
//!
//! GoldSrc references: `CLight::Spawn`/`Use` (lights.cpp: a targeted light of
//! style 32 or more switches between "a" and its pattern or "m"),
//! `CWorld::Precache` (styles 1 to 12), and `R_AnimateLight` (the engine steps
//! a pattern at 10 Hz and scales by `(letter - 'a') * 22`).

/// A cooked decal axis: one of the 26 neighbours of a cube as a unit Q12
/// vector, from Q6 components (host/hl-bsp l5.rs `decode_dir`; the code is
/// the ternary digits `(x + 1) + 3 (y + 1) + 9 (z + 1)`, 13 unused).
pub const DIR26: [[i8; 3]; 32] = {
    let mut t = [[0i8; 3]; 32];
    let mut c: i32 = 0;
    while c < 27 {
        let d: [i32; 3] = [c % 3 - 1, c / 3 % 3 - 1, c / 9 - 1];
        let k = d[0].abs() + d[1].abs() + d[2].abs();
        // 64 / sqrt(k), rounded
        let len = [0, 64, 45, 37][k as usize] as i8;
        t[c as usize] = [d[0] as i8 * len, d[1] as i8 * len, d[2] as i8 * len];
        c += 1;
    }
    t
};

#[inline(always)]
pub fn dir26(code: u16) -> [i32; 3] {
    let t = DIR26[(code & 31) as usize];
    [(t[0] as i32) << 6, (t[1] as i32) << 6, (t[2] as i32) << 6]
}

/// The level of one pattern letter, with "m" (the engine's 12 * 22 = 264)
/// normalised to 256, the unit the cooked lighting uses throughout.
#[inline(always)]
pub fn pattern_level(letter: u8) -> i32 {
    (letter.wrapping_sub(b'a') as i32) * 64 / 3
}

/// A light style's level at pattern `step` (10 Hz): dark when it is switched
/// off, steady "m" without a pattern, else the pattern letter for the step.
#[inline(always)]
pub fn style_level(off: bool, pattern: &[u8], step: u32) -> i32 {
    if off {
        0
    } else if pattern.is_empty() {
        256
    } else {
        pattern_level(pattern[(step % pattern.len() as u32) as usize])
    }
}

/// A style's gain: how far its `level` has moved from the `start` level the
/// baked lighting holds, in 1/256 of the stored plane delta. A style that
/// starts lit dims by `(level - start) / start` (negative when dimmer; the
/// cooker stored what the plane contributed at `start`), and one that starts
/// dark brightens by `level / 256`.
#[inline(always)]
pub fn style_gain(level: i32, start: i32) -> i32 {
    if start > 0 {
        (level - start) * 256 / start
    } else {
        level
    }
}

/// `base` (0x00BBGGRR) moved by `delta` (a palette magnitude, per channel)
/// times `gain` / 256, clamped to the channel range.
#[inline(always)]
pub fn shift_light(base: u32, delta: u32, gain: i32) -> u32 {
    let mut out = 0u32;
    let mut sh = 0;
    while sh < 24 {
        let b = ((base >> sh) & 0xff) as i32;
        let d = ((delta >> sh) & 0xff) as i32;
        out |= ((b + ((d * gain) >> 8)).clamp(0, 255) as u32) << sh;
        sh += 8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_are_unit_and_axial_ones_are_exact() {
        for code in (0..27u16).filter(|&c| c != 13) {
            let v = dir26(code);
            let len2 = (v[0] as i64).pow(2) + (v[1] as i64).pow(2) + (v[2] as i64).pow(2);
            let len = (len2 as f64).sqrt();
            assert!((len - 4096.0).abs() < 60.0, "code {code} length {len}");
        }
        assert_eq!(dir26(1 + 3 * 1 + 9 * 2), [0, 0, 4096]); // +z
        assert_eq!(dir26(3 * 2 + 9 + 1), [0, 4096, 0]); // +y
        assert_eq!(dir26(0 + 3 + 9), [-4096, 0, 0]); // -x
        assert_eq!(dir26(13), [0, 0, 0]);
    }

    #[test]
    fn pattern_levels_put_m_at_256() {
        assert_eq!(pattern_level(b'm'), 256);
        assert_eq!(pattern_level(b'a'), 0);
        assert_eq!(pattern_level(b'z'), 533);
    }

    #[test]
    fn a_switched_off_style_is_dark_and_a_steady_one_is_m() {
        assert_eq!(style_level(true, b"mmaa", 3), 0);
        assert_eq!(style_level(false, b"", 9), 256);
        assert_eq!(style_level(false, b"ma", 0), 256);
        assert_eq!(style_level(false, b"ma", 1), 0);
        assert_eq!(style_level(false, b"ma", 2), 256, "the pattern wraps");
    }

    #[test]
    fn gain_is_zero_at_the_start_level_and_full_at_the_other_end() {
        // Starts lit at "m": off removes the whole stored contribution.
        assert_eq!(style_gain(256, 256), 0);
        assert_eq!(style_gain(0, 256), -256);
        // Starts at an animated pattern's average: dips below it dim, peaks add.
        assert!(style_gain(100, 170) < 0);
        assert!(style_gain(300, 170) > 0);
        // Starts dark: on is the full stored delta.
        assert_eq!(style_gain(0, 0), 0);
        assert_eq!(style_gain(256, 0), 256);
        assert_eq!(style_gain(128, 0), 128);
    }

    #[test]
    fn shifting_a_colour_scales_each_channel_and_clamps() {
        let base = 0x0050_6070; // b 0x50, g 0x60, r 0x70
        let delta = 0x0020_2020;
        assert_eq!(shift_light(base, delta, 0), base);
        assert_eq!(shift_light(base, delta, 256), 0x0070_8090);
        assert_eq!(shift_light(base, delta, -256), 0x0030_4050);
        // Clamped at both ends.
        assert_eq!(shift_light(0x0000_0010, 0x0000_0040, -256), 0);
        assert_eq!(shift_light(0x0000_00f0, 0x0000_0040, 256), 0xff);
        // Half a gain moves half the delta.
        assert_eq!(shift_light(base, delta, 128), 0x0060_7080);
    }
}
