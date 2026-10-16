//! Integrated loudness after ITU-R BS.1770 / EBU R128 for one channel: the
//! K-weighting filters (a high shelf, then a high-pass), 400 ms blocks every
//! 100 ms, the absolute gate at -70 LUFS and the relative gate 10 LU below
//! the ungated mean. Filter coefficients are derived for the stream's own
//! sample rate with the bilinear transform of the standard's analogue
//! prototypes.

use std::f64::consts::PI;

#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
}

impl Biquad {
    fn run(&self, input: &[f64]) -> Vec<f64> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        input
            .iter()
            .map(|&x| {
                let y = self.b[0] * x + self.b[1] * x1 + self.b[2] * x2
                    - self.a[0] * y1
                    - self.a[1] * y2;
                (x2, x1, y2, y1) = (x1, x, y1, y);
                y
            })
            .collect()
    }
}

/// The pre-filter: a high shelf of +4 dB around 1.68 kHz.
fn shelf(fs: f64) -> Biquad {
    let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (PI * f0 / fs).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    }
}

/// The RLB weighting: a second-order high-pass around 38 Hz.
fn high_pass(fs: f64) -> Biquad {
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    }
}

/// Integrated loudness in LUFS of mono `samples` (full scale is +-1.0) at
/// `fs` Hz, or `None` when the signal is too short or too quiet to gate.
pub fn integrated(samples: &[f64], fs: f64) -> Option<f64> {
    let weighted = high_pass(fs).run(&shelf(fs).run(samples));
    let block = (0.4 * fs).round() as usize;
    let step = (0.1 * fs).round() as usize;
    if weighted.len() < block {
        return None;
    }
    let mut energies = Vec::new();
    let mut start = 0;
    while start + block <= weighted.len() {
        let sum: f64 = weighted[start..start + block].iter().map(|v| v * v).sum();
        energies.push(sum / block as f64);
        start += step;
    }
    let lufs = |z: f64| -0.691 + 10.0 * z.log10();
    let mean = |zs: &[f64]| zs.iter().sum::<f64>() / zs.len() as f64;
    let loud: Vec<f64> = energies
        .iter()
        .copied()
        .filter(|z| lufs(*z) > -70.0)
        .collect();
    if loud.is_empty() {
        return None;
    }
    let gate = lufs(mean(&loud)) - 10.0;
    let kept: Vec<f64> = loud.iter().copied().filter(|z| lufs(*z) > gate).collect();
    Some(lufs(mean(&kept)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(fs: f64, hz: f64, amplitude: f64, seconds: f64) -> Vec<f64> {
        (0..(fs * seconds) as usize)
            .map(|i| amplitude * (2.0 * PI * hz * i as f64 / fs).sin())
            .collect()
    }

    #[test]
    fn a_full_scale_997_hz_sine_reads_minus_3_lufs_at_any_rate() {
        // BS.1770: a 0 dBFS sine at 997 Hz is -3.01 LUFS on one channel.
        for fs in [48_000.0, 37_800.0] {
            let l = integrated(&sine(fs, 997.0, 1.0, 5.0), fs).unwrap();
            assert!((l + 3.01).abs() < 0.1, "{fs}: {l}");
        }
    }

    #[test]
    fn six_db_less_amplitude_reads_six_lu_lower() {
        let a = integrated(&sine(37_800.0, 997.0, 0.5, 4.0), 37_800.0).unwrap();
        let b = integrated(&sine(37_800.0, 997.0, 0.25, 4.0), 37_800.0).unwrap();
        assert!((a - b - 6.02).abs() < 0.02, "{a} {b}");
    }

    #[test]
    fn silence_and_short_signals_have_no_loudness() {
        assert!(integrated(&vec![0.0; 40_000], 37_800.0).is_none());
        assert!(integrated(&[0.1; 100], 37_800.0).is_none());
    }

    #[test]
    fn the_relative_gate_ignores_a_much_quieter_tail() {
        let mut loud = sine(37_800.0, 997.0, 0.5, 3.0);
        let alone = integrated(&loud, 37_800.0).unwrap();
        loud.extend(sine(37_800.0, 997.0, 0.5 * 0.01, 3.0));
        let with_tail = integrated(&loud, 37_800.0).unwrap();
        assert!((alone - with_tail).abs() < 0.3, "{alone} {with_tail}");
    }
}
