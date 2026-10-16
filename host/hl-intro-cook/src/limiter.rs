//! The gain stage in front of the XA encoder: a fixed gain, then a look-ahead
//! peak limiter so that the louder stream never clips, and the search for
//! the gain that makes the limited stream reach a loudness target.

use std::collections::VecDeque;

use crate::loudness;

/// Highest sample the limiter lets through, in full-scale units (-1 dBFS).
/// The ADPCM encoder adds quantisation error of a few hundred LSB on top.
pub const CEILING: f64 = 0.891_250_938_133_745_6;
/// The limiter looks this far ahead, and so ramps down this slowly.
const ATTACK_SECONDS: f64 = 0.005;
/// Time constant of the recovery to unity gain.
const RELEASE_SECONDS: f64 = 0.100;

/// What the limiter did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    /// Largest gain reduction applied by the limiter, in dB (0 for none).
    pub max_reduction_db: f64,
    /// Fraction of samples the limiter reduced by more than 0.1 dB.
    pub active: f64,
}

/// `samples * gain`, limited to [`CEILING`].
///
/// The gain each sample needs to stay under the ceiling is smoothed by a
/// moving minimum over the attack window and then a moving average of the
/// same length, so it ramps down before a peak arrives and never lets one
/// through; it recovers exponentially. Output is time-aligned with the input
/// (the look-ahead reads future input instead of delaying the output).
pub fn apply(samples: &[f64], gain: f64, fs: f64) -> (Vec<f64>, Report) {
    let amplified: Vec<f64> = samples.iter().map(|s| s * gain).collect();
    let window = ((ATTACK_SECONDS * fs).round() as usize).max(1);
    let n = amplified.len();
    let need: Vec<f64> = amplified
        .iter()
        .map(|s| {
            if s.abs() > CEILING {
                CEILING / s.abs()
            } else {
                1.0
            }
        })
        .collect();
    // m[k] = min of need over [k, k + window).
    let mut m = vec![1.0; n];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for k in (0..n).rev() {
        while queue.back().is_some_and(|&b| need[b] >= need[k]) {
            queue.pop_back();
        }
        queue.push_back(k);
        while queue.front().is_some_and(|&f| f >= k + window) {
            queue.pop_front();
        }
        m[k] = need[*queue.front().expect("k is in the window")];
    }
    // s[i] = mean of m over (i - window, i]: for every sample j, every m[k]
    // with k in (j - window, j] includes need[j], so s[j] <= need[j].
    let mut smooth = vec![1.0; n];
    let mut sum = 0.0;
    for i in 0..n {
        sum += m[i];
        if i >= window {
            sum -= m[i - window];
        }
        let count = (i + 1).min(window) as f64;
        // Before the first full window the missing history is m[0], which is
        // already no more than any need inside the first window.
        smooth[i] = (sum + (window as f64 - count) * m[0]) / window as f64;
    }
    let alpha = 1.0 - (-1.0 / (RELEASE_SECONDS * fs)).exp();
    let mut g = 1.0f64;
    let mut out = Vec::with_capacity(n);
    let (mut worst, mut active) = (1.0f64, 0usize);
    for i in 0..n {
        g = smooth[i].min(g + (1.0 - g) * alpha);
        worst = worst.min(g);
        if g < 0.988_5 {
            active += 1;
        }
        out.push(amplified[i] * g);
    }
    (
        out,
        Report {
            max_reduction_db: (-20.0 * worst.log10()).abs(),
            active: active as f64 / n.max(1) as f64,
        },
    )
}

/// The gain (linear) at which the limited stream reaches `target_lufs`, found
/// by bisection on the gain in dB, or `None` when the target is out of reach
/// within +-30 dB.
pub fn gain_for(samples: &[f64], fs: f64, target_lufs: f64) -> Option<f64> {
    let loudness_at = |db: f64| {
        let (limited, _) = apply(samples, 10f64.powf(db / 20.0), fs);
        loudness::integrated(&limited, fs)
    };
    let (mut lo, mut hi) = (-30.0f64, 30.0f64);
    if loudness_at(hi)? < target_lufs || loudness_at(lo)? > target_lufs {
        return None;
    }
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if loudness_at(mid)? < target_lufs {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(10f64.powf((lo + hi) / 2.0 / 20.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn tone(fs: f64, amplitude: f64, seconds: f64) -> Vec<f64> {
        (0..(fs * seconds) as usize)
            .map(|i| amplitude * (2.0 * PI * 440.0 * i as f64 / fs).sin())
            .collect()
    }

    #[test]
    fn a_stream_under_the_ceiling_passes_untouched_at_unity_gain() {
        let x = tone(37_800.0, 0.5, 1.0);
        let (y, report) = apply(&x, 1.0, 37_800.0);
        assert_eq!(x, y);
        assert_eq!(report.max_reduction_db, 0.0);
    }

    #[test]
    fn nothing_passes_the_ceiling_whatever_the_gain() {
        let mut x = tone(37_800.0, 0.2, 1.0);
        x[10_000] = 0.9; // a click
        for gain in [1.0, 4.0, 40.0] {
            let (y, _) = apply(&x, gain, 37_800.0);
            let peak = y.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            assert!(peak <= CEILING + 1e-12, "gain {gain}: {peak}");
        }
    }

    #[test]
    fn the_limiter_only_reduces_gain_around_the_peak() {
        let mut x = tone(37_800.0, 0.1, 2.0);
        x[37_800] = 0.95;
        let (y, report) = apply(&x, 1.0, 37_800.0);
        assert!(report.max_reduction_db > 0.0);
        // Far from the click the tone is untouched.
        assert_eq!(x[1000], y[1000]);
        assert!((x[75_000] - y[75_000]).abs() < 1e-6);
        assert!(report.active < 0.1);
    }

    #[test]
    fn the_search_hits_a_reachable_target() {
        let x = tone(37_800.0, 0.05, 3.0);
        let want = -20.0;
        let g = gain_for(&x, 37_800.0, want).unwrap();
        let (y, _) = apply(&x, g, 37_800.0);
        let got = loudness::integrated(&y, 37_800.0).unwrap();
        assert!((got - want).abs() < 0.01, "{got}");
        assert!(gain_for(&x, 37_800.0, 50.0).is_none());
    }
}
