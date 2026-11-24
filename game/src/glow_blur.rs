//! The title menu's halo blur (menu.rs `upload_glow_atlas`): one 3x3 box pass over a glyph cell,
//! kept free of statics and hardware so the host test runner can check it against the plain
//! nine-tap form it replaces.
//!
//! The box sum is separable: three horizontal neighbours first, then three of those sums
//! vertically, with the cell's outside read as zero in both. That is the same integer sum the
//! nine-tap loop takes (so the same floor of the mean, texel for texel), at six additions a texel
//! instead of nine bounds-checked reads. 96 cells, twice, at every menu entry: on PSoXide the atlas
//! took 29 VBlanks with the nine-tap form, 15 with this one (11-24; the blur is ~60% of those).

/// Scratch entries [`box3_pass`] needs for a `width` x `height` cell: the row sums with a zero
/// row above and below.
pub const fn scratch_len(width: usize, height: usize) -> usize {
    width * (height + 2)
}

/// One 3x3 box blur pass over a `width` x `height` cell, outside the cell read as zero: every
/// texel becomes the floor of the mean of its nine neighbours. `rows` is scratch of at least
/// [`scratch_len`] entries.
///
/// # Panics
///
/// When `width` is zero, `cell` holds fewer than `width * height` entries or `rows` fewer than
/// [`scratch_len`].
// Out of line, kept small and in the title menu overlay on the scene core: inlined with the menu's
// constant sizes, LLVM unrolls it past the overlay's 16 KiB window. Iterators instead of indexing,
// so the size-optimised loops carry no bounds checks.
#[inline(never)]
#[optimize(size)]
#[cfg_attr(
    feature = "scene-core",
    link_section = ".text.overlay.1.menu_glow_box3_pass"
)]
pub fn box3_pass(cell: &mut [u8], rows: &mut [u16], width: usize, height: usize) {
    let cell = &mut cell[..width * height];
    let rows = &mut rows[..scratch_len(width, height)];
    let (top, rest) = rows.split_at_mut(width);
    let (sums, bottom) = rest.split_at_mut(width * height);
    top.fill(0);
    bottom.fill(0);
    // Each texel's sum with its left and right neighbours.
    for (row, out) in cell.chunks_exact(width).zip(sums.chunks_exact_mut(width)) {
        let mut left = 0u16;
        let mut middle = u16::from(row[0]);
        let rights = row[1..]
            .iter()
            .map(|&texel| u16::from(texel))
            .chain(Some(0));
        for (sum, right) in out.iter_mut().zip(rights) {
            *sum = left + middle + right;
            left = middle;
            middle = right;
        }
    }
    // Then three of those sums down a column: the row above, this one, the row below.
    let above = rows.chunks_exact(width);
    let here = rows[width..].chunks_exact(width);
    let below = rows[2 * width..].chunks_exact(width);
    for (out, ((up, middle), down)) in cell.chunks_exact_mut(width).zip(above.zip(here).zip(below))
    {
        for (texel, ((&a, &b), &c)) in out.iter_mut().zip(up.iter().zip(middle).zip(down)) {
            *texel = ((u32::from(a) + u32::from(b) + u32::from(c)) / 9) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nine-tap pass menu.rs used before, as it was written.
    fn reference(alpha: &mut [u8], width: usize, height: usize) {
        let blurred = alpha.to_vec();
        for y in 0..height {
            for x in 0..width {
                let mut sum = 0u32;
                for dy in 0..3 {
                    for dx in 0..3 {
                        let sy = (y + dy).wrapping_sub(1);
                        let sx = (x + dx).wrapping_sub(1);
                        if sy < height && sx < width {
                            sum += blurred[sy * width + sx] as u32;
                        }
                    }
                }
                alpha[y * width + x] = (sum / 9) as u8;
            }
        }
    }

    fn check(cell: &[u8], width: usize, height: usize) {
        let mut want = cell.to_vec();
        let mut got = cell.to_vec();
        let mut rows = vec![0u16; scratch_len(width, height)];
        // Two passes, as the menu runs them.
        for _ in 0..2 {
            reference(&mut want, width, height);
            box3_pass(&mut got, &mut rows, width, height);
            assert_eq!(got, want, "cell {cell:?}");
        }
    }

    #[test]
    fn matches_the_nine_tap_pass_on_menu_cells() {
        const W: usize = 16;
        const H: usize = 24;
        // Every single lit texel (corners and edges included), at the menu's ink level.
        for at in 0..W * H {
            let mut cell = [0u8; W * H];
            cell[at] = 15;
            check(&cell, W, H);
        }
        // Pseudo-random cells over the whole byte range, and over the 4-bit levels.
        let mut state = 0x1234_5678u32;
        for round in 0..2_000 {
            let mut cell = [0u8; W * H];
            for texel in cell.iter_mut() {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *texel = if round % 2 == 0 {
                    (state & 15) as u8
                } else {
                    state as u8
                };
            }
            check(&cell, W, H);
        }
    }

    #[test]
    fn matches_on_small_and_thin_cells() {
        for (width, height) in [(1, 1), (1, 5), (5, 1), (2, 2), (3, 7)] {
            let cell: Vec<u8> = (0..width * height).map(|i| (i * 37 % 256) as u8).collect();
            check(&cell, width, height);
        }
    }
}
