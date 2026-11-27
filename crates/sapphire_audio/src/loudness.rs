//! Measured loudness (M6 U10): ITU-R BS.1770-4's integrated loudness, EBU Tech 3342's loudness range and a true
//! peak, read off a stereo render by a pure function, [`measure`].
//!
//! - **K-weighting:** BS.1770's two stages, a high shelf near 1.7 kHz and a high pass near 38 Hz, derived from
//!   their analog prototypes at the render's own rate, so 44.1 kHz and 48 kHz read alike.
//! - **Integrated loudness:** 400 ms blocks every 100 ms, gated absolutely at −70 LUFS and then relatively, 10 LU
//!   under the loudness of the blocks the first gate kept. Silence and near-silence don't pull it down.
//! - **Loudness range:** short-term loudness over 3 s every 100 ms, gated at −70 LUFS and 20 LU under their mean,
//!   from the 10th to the 95th percentile.
//! - **True peak:** the render 4× oversampled, the largest magnitude of either channel, through a windowed-sinc
//!   interpolator of 48 taps in BS.1770-4 Annex 2's manner, not Annex 2's own filter. The limiter on the master
//!   ([`crate::Master`]) hears its peaks through the same interpolator, so what it holds to its ceiling is what
//!   this reads.
//!
//! Only two channels, each weighted 1, as BS.1770 weights left and right: the mixer's output is stereo.

/// How long a gating block lasts, in sub-blocks of 100 ms.
const BLOCK_STEPS: usize = 4;
/// How long a short-term window lasts, in sub-blocks of 100 ms.
const SHORT_TERM_STEPS: usize = 30;
/// The sub-block, a tenth of a second: each block's and each window's step.
const STEP_SECONDS: f64 = 0.1;
/// BS.1770's absolute gate.
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// BS.1770's relative gate, under the loudness of what the absolute gate kept.
const RELATIVE_GATE_LU: f64 = -10.0;
/// EBU Tech 3342's relative gate for the loudness range.
const RANGE_GATE_LU: f64 = -20.0;

/// What [`measure`] read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    /// Integrated loudness, gated, in LUFS: negative infinity if no block passed the gates.
    pub integrated_lufs: f64,
    /// The loudness range, in LU: 0 if fewer than two short-term windows passed its gates.
    pub range_lu: f64,
    /// The largest magnitude of the render 4× oversampled, in dB under full scale (dBTP).
    pub true_peak_db: f64,
    /// The largest sample's magnitude, in dB under full scale.
    pub sample_peak_db: f64,
    /// How many 400 ms blocks the render held, gated or not.
    pub blocks: usize,
}

/// Reads the loudness of `samples`, interleaved stereo at `sample_rate`, as the [module docs](self) say. A last
/// part shorter than a sub-block of 100 ms is left out of the loudness, but not of the peaks.
#[must_use]
pub fn measure(samples: &[f32], sample_rate: u32) -> Loudness {
    let rate = f64::from(sample_rate.max(1));
    let step = ((rate * STEP_SECONDS).round() as usize).max(1);
    let mut weighting = [KWeighting::new(rate), KWeighting::new(rate)];
    let mut peaks = TruePeak::new();
    let (mut true_peak, mut sample_peak) = (0.0_f32, 0.0_f32);
    // Each sub-block's mean square, summed over the two channels.
    let mut steps = Vec::with_capacity(samples.len() / 2 / step + 1);
    let mut energy = 0.0;
    let mut filled = 0;
    for frame in samples.as_chunks::<2>().0 {
        let (left, right) = (frame[0], frame[1]);
        sample_peak = sample_peak.max(left.abs()).max(right.abs());
        true_peak = true_peak.max(peaks.push(left, right));
        let left = weighting[0].filter(f64::from(left));
        let right = weighting[1].filter(f64::from(right));
        energy += left * left + right * right;
        filled += 1;
        if filled == step {
            steps.push(energy / step as f64);
            (energy, filled) = (0.0, 0);
        }
    }
    true_peak = true_peak.max(peaks.finish());

    let windows = |length: usize| -> Vec<f64> {
        if steps.len() < length {
            return Vec::new();
        }
        let sums = steps.windows(length).map(|window| window.iter().sum::<f64>() / length as f64);
        sums.collect()
    };
    let blocks = windows(BLOCK_STEPS);
    let short_term = windows(SHORT_TERM_STEPS);
    Loudness {
        integrated_lufs: integrated(&blocks),
        range_lu: range(&short_term),
        true_peak_db: decibels(f64::from(true_peak)),
        sample_peak_db: decibels(f64::from(sample_peak)),
        blocks: blocks.len(),
    }
}

/// The loudness of a mean square summed over the channels, in LUFS.
pub(crate) fn lufs(energy: f64) -> f64 {
    -0.691 + 10.0 * energy.log10()
}

fn decibels(magnitude: f64) -> f64 {
    20.0 * magnitude.log10()
}

/// The loudness of the mean of `energies`, in LUFS: negative infinity for none.
fn mean_lufs<'a>(energies: impl Iterator<Item = &'a f64>) -> f64 {
    let (sum, count) = energies.fold((0.0, 0_usize), |(sum, count), energy| (sum + energy, count + 1));
    if count == 0 { f64::NEG_INFINITY } else { lufs(sum / count as f64) }
}

/// BS.1770's gated loudness over the blocks' energies.
fn integrated(blocks: &[f64]) -> f64 {
    let heard = || blocks.iter().filter(|&&energy| lufs(energy) > ABSOLUTE_GATE_LUFS);
    let gate = mean_lufs(heard()) + RELATIVE_GATE_LU;
    mean_lufs(heard().filter(|&&energy| lufs(energy) > gate))
}

/// EBU Tech 3342's loudness range over the short-term windows' energies.
fn range(short_term: &[f64]) -> f64 {
    let heard = || short_term.iter().filter(|&&energy| lufs(energy) > ABSOLUTE_GATE_LUFS);
    let gate = mean_lufs(heard()) + RANGE_GATE_LU;
    let mut kept: Vec<f64> =
        heard().map(|&energy| lufs(energy)).filter(|&loudness| loudness > gate).collect();
    if kept.len() < 2 {
        return 0.0;
    }
    kept.sort_unstable_by(f64::total_cmp);
    let at = |share: f64| kept[((kept.len() - 1) as f64 * share).round() as usize];
    at(0.95) - at(0.10)
}

/// One biquad, in direct form I.
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    inputs: [f64; 2],
    outputs: [f64; 2],
}

impl Biquad {
    const fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self { b, a, inputs: [0.0; 2], outputs: [0.0; 2] }
    }

    fn filter(&mut self, input: f64) -> f64 {
        let output = self.b[0] * input + self.b[1] * self.inputs[0] + self.b[2] * self.inputs[1]
            - self.a[0] * self.outputs[0]
            - self.a[1] * self.outputs[1];
        // Silence decays the feedback toward subnormals, slow on some processors and far under anything heard.
        let output = if output.abs() < 1e-30 { 0.0 } else { output };
        self.inputs = [input, self.inputs[0]];
        self.outputs = [output, self.outputs[0]];
        output
    }
}

/// BS.1770's K-weighting for one channel: the head's high shelf, then the revised low-frequency B-curve's high pass,
/// each the bilinear transform of its analog prototype at the sample rate, which at 48 kHz gives the
/// recommendation's own coefficients.
#[derive(Clone, Copy, Debug)]
pub(crate) struct KWeighting {
    shelf: Biquad,
    high_pass: Biquad,
}

impl KWeighting {
    pub(crate) fn new(rate: f64) -> Self {
        // The shelf: about +4 dB above 1.7 kHz.
        let (f0, gain_db, q) = (1_681.974_450_955_533, 3.999_843_853_973_347, 0.707_175_236_955_419_6);
        let k = (std::f64::consts::PI * f0 / rate).tan();
        let vh = 10_f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.499_666_774_154_541_6);
        let a0 = 1.0 + k / q + k * k;
        let shelf = Biquad::new(
            [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
            [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        );
        // The high pass: second order near 38 Hz.
        let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
        let k = (std::f64::consts::PI * f0 / rate).tan();
        let a0 = 1.0 + k / q + k * k;
        let high_pass = Biquad::new([1.0, -2.0, 1.0], [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0]);
        Self { shelf, high_pass }
    }

    pub(crate) fn filter(&mut self, input: f64) -> f64 {
        self.high_pass.filter(self.shelf.filter(input))
    }
}

/// Taps either side of the point a phase interpolates: twelve a phase, 48 over the four.
const HALF_TAPS: usize = 6;
const TAPS: usize = 2 * HALF_TAPS;
/// The three points between two samples a 4× oversampling adds; the fourth is the sample itself.
const PHASES: usize = 3;

/// A true-peak detector in BS.1770-4 Annex 2's manner: each frame 4× oversampled through a windowed sinc, the largest
/// magnitude of either channel at the sample or the three points after it. A point between two samples needs
/// [`TruePeak::DELAY`] samples after it, so a frame's peak comes out that many frames late.
#[derive(Clone, Debug)]
pub(crate) struct TruePeak {
    /// Each phase's taps, over the samples from `HALF_TAPS - 1` before the point to `HALF_TAPS` after.
    taps: [[f32; TAPS]; PHASES],
    /// The last [`TAPS`] frames, oldest first, as a ring from `next`.
    history: [[f32; 2]; TAPS],
    next: usize,
    /// Frames pushed, up to `TAPS`: until there are, the oldest are silence.
    seen: usize,
}

impl TruePeak {
    /// How many frames late a frame's peak is known.
    pub(crate) const DELAY: usize = HALF_TAPS;

    pub(crate) fn new() -> Self {
        let mut taps = [[0.0; TAPS]; PHASES];
        for (phase, taps) in taps.iter_mut().enumerate() {
            let fraction = (phase + 1) as f64 / 4.0;
            for (tap, value) in taps.iter_mut().enumerate() {
                // How far this tap's sample lies from the point, in samples.
                let t = tap as f64 - (HALF_TAPS - 1) as f64 - fraction;
                let x = std::f64::consts::PI * t;
                let sinc = if t == 0.0 { 1.0 } else { x.sin() / x };
                // Blackman's window over the taps' span.
                let w = std::f64::consts::PI * t / HALF_TAPS as f64;
                let window = 0.42 + 0.5 * w.cos() + 0.08 * (2.0 * w).cos();
                *value = (sinc * window) as f32;
            }
            // Unity at DC, so a held level reads as itself between its samples.
            let sum: f32 = taps.iter().sum();
            taps.iter_mut().for_each(|value| *value /= sum);
        }
        Self { taps, history: [[0.0; 2]; TAPS], next: 0, seen: 0 }
    }

    /// Forgets every frame pushed.
    pub(crate) fn clear(&mut self) {
        (self.history, self.next, self.seen) = ([[0.0; 2]; TAPS], 0, 0);
    }

    /// Takes a frame in, and gives the true peak of the frame [`Self::DELAY`] before it: 0 until there is one.
    pub(crate) fn push(&mut self, left: f32, right: f32) -> f32 {
        self.history[self.next] = [left, right];
        self.next = (self.next + 1) % TAPS;
        self.seen = (self.seen + 1).min(TAPS);
        if self.seen <= Self::DELAY {
            return 0.0;
        }
        // The frame the peak is of, `HALF_TAPS - 1` into the ring from its oldest.
        let sample = self.history[(self.next + HALF_TAPS - 1) % TAPS];
        let mut peak = sample[0].abs().max(sample[1].abs());
        for taps in &self.taps {
            let (mut left, mut right) = (0.0, 0.0);
            for (tap, &weight) in taps.iter().enumerate() {
                let [l, r] = self.history[(self.next + tap) % TAPS];
                left += weight * l;
                right += weight * r;
            }
            peak = peak.max(left.abs()).max(right.abs());
        }
        peak
    }

    /// The peaks of the last [`Self::DELAY`] frames pushed, with silence after them: what a render's end holds.
    pub(crate) fn finish(&mut self) -> f32 {
        let pending = self.seen.min(Self::DELAY);
        let peak = (0..pending).map(|_| self.push(0.0, 0.0)).fold(0.0, f32::max);
        self.clear();
        peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// A stereo sine of `hz` at `peak_db` in both channels, `seconds` long.
    fn sine(hz: f64, peak_db: f64, seconds: f64, rate: u32) -> Vec<f32> {
        let amplitude = 10_f64.powf(peak_db / 20.0);
        let frames = (seconds * f64::from(rate)) as usize;
        let step = std::f64::consts::TAU * hz / f64::from(rate);
        (0..frames).flat_map(|n| [(amplitude * (step * n as f64).sin()) as f32; 2]).collect()
    }

    fn silence(seconds: f64) -> Vec<f32> {
        vec![0.0; (seconds * f64::from(RATE)) as usize * 2]
    }

    /// At 48 kHz the derived stages are the recommendation's own (BS.1770-4, table 1 and table 2).
    #[test]
    fn k_weighting_at_48_khz_is_the_recommendations() {
        let k = KWeighting::new(48_000.0);
        let near = |a: &[f64], b: &[f64]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-8);
        assert!(near(&k.shelf.b, &[1.535_124_859_586_97, -2.691_696_189_406_38, 1.198_392_810_852_85]));
        assert!(near(&k.shelf.a, &[-1.690_659_293_182_41, 0.732_480_774_215_85]));
        assert!(near(&k.high_pass.a, &[-1.990_047_454_833_98, 0.990_072_250_366_21]), "{:?}", k.high_pass.a);
    }

    /// A 997 Hz sine at a stated level in both channels reads that level in LUFS, within 0.1 LU, at either rate:
    /// one channel of a full-scale sine is −3.01 LUFS, and two are 3 dB louder.
    #[test]
    fn a_997_hz_sine_reads_its_stated_loudness() {
        for rate in [44_100, 48_000] {
            for level in [-23.0, -18.0, -36.0] {
                let read = measure(&sine(997.0, level, 10.0, rate), rate);
                assert!((read.integrated_lufs - level).abs() < 0.1, "{level} at {rate}: {read:?}");
                assert!((read.true_peak_db - level).abs() < 0.05, "{level} at {rate}: {read:?}");
            }
        }
    }

    /// Silence on either side of a tone, and a tone too quiet for the absolute gate, are gated out: the reading is
    /// the tone's alone, but for the few blocks that straddle its ends, partly silent and still within 10 LU of it,
    /// which take a tenth of a LU off a 10 s tone. Ungated, the 70 s would read 8.5 LU down.
    #[test]
    fn gated_silence_doesnt_pull_the_reading_down() {
        let tone = sine(997.0, -23.0, 10.0, RATE);
        let alone = measure(&tone, RATE).integrated_lufs;
        let mut padded = silence(20.0);
        padded.extend(&tone);
        padded.extend(silence(20.0));
        padded.extend(sine(997.0, -80.0, 20.0, RATE));
        let read = measure(&padded, RATE);
        assert!((read.integrated_lufs - alone).abs() < 0.2, "{} against {alone}", read.integrated_lufs);
        assert_eq!(measure(&silence(5.0), RATE).integrated_lufs, f64::NEG_INFINITY);
    }

    /// EBU Tech 3341's third and fourth cases: quieter passages more than 10 LU down are gated relatively, so the
    /// loud passage reads alone.
    #[test]
    fn quiet_passages_are_gated_relatively() {
        let mut program = sine(1_000.0, -36.0, 10.0, RATE);
        program.extend(sine(1_000.0, -23.0, 60.0, RATE));
        program.extend(sine(1_000.0, -36.0, 10.0, RATE));
        let read = measure(&program, RATE);
        assert!((read.integrated_lufs + 23.0).abs() < 0.1, "{read:?}");
        let mut program = sine(1_000.0, -72.0, 10.0, RATE);
        program.extend(sine(1_000.0, -36.0, 10.0, RATE));
        program.extend(sine(1_000.0, -23.0, 60.0, RATE));
        program.extend(sine(1_000.0, -36.0, 10.0, RATE));
        program.extend(sine(1_000.0, -72.0, 10.0, RATE));
        let read = measure(&program, RATE);
        assert!((read.integrated_lufs + 23.0).abs() < 0.1, "{read:?}");
    }

    /// EBU Tech 3342's first two cases: 20 s at one level then 20 s 10 LU or 5 LU lower read a range of 10 or 5,
    /// within 1 LU.
    #[test]
    fn two_levels_read_their_difference_as_the_range() {
        for (low, expected) in [(-30.0, 10.0), (-25.0, 5.0)] {
            let mut program = sine(1_000.0, -20.0, 20.0, RATE);
            program.extend(sine(1_000.0, low, 20.0, RATE));
            let read = measure(&program, RATE);
            assert!((read.range_lu - expected).abs() < 1.0, "{low}: {read:?}");
        }
        assert!(measure(&sine(1_000.0, -20.0, 10.0, RATE), RATE).range_lu < 0.1, "a steady tone has none");
    }

    /// EBU Tech 3342's third and fourth cases: 20 LU between two levels reads 20, and the range's relative gate
    /// leaves out what's more than 20 LU under the programme, so −50, −35, −20, −35, −50 reads 15 rather than 30.
    #[test]
    fn the_range_gates_relatively() {
        let mut program = sine(1_000.0, -40.0, 20.0, RATE);
        program.extend(sine(1_000.0, -20.0, 20.0, RATE));
        let read = measure(&program, RATE);
        assert!((read.range_lu - 20.0).abs() < 1.0, "{read:?}");
        let mut program = Vec::new();
        for level in [-50.0, -35.0, -20.0, -35.0, -50.0] {
            program.extend(sine(1_000.0, level, 20.0, RATE));
        }
        let read = measure(&program, RATE);
        assert!((read.range_lu - 15.0).abs() < 1.0, "{read:?}");
    }

    /// The absolute gate alone: 60 s at −71 LUFS and 6 s at −60 read −60, since −71 is under −70; ungated, the
    /// relative gate would keep both and read about −67.9.
    #[test]
    fn the_absolute_gate_leaves_out_what_is_under_minus_70() {
        let mut program = sine(1_000.0, -71.0, 60.0, RATE);
        program.extend(sine(1_000.0, -60.0, 6.0, RATE));
        let read = measure(&program, RATE);
        assert!((read.integrated_lufs + 60.0).abs() < 0.2, "{read:?}");
    }

    /// A sine at a quarter of the rate, sampled 45° off its crests, peaks 3 dB over its samples: the true peak
    /// finds the crests between them, which the sample peak misses.
    #[test]
    fn the_true_peak_finds_what_falls_between_samples() {
        let rate = f64::from(RATE);
        let step = std::f64::consts::TAU * (rate / 4.0) / rate;
        let samples: Vec<f32> = (0..RATE as usize)
            .flat_map(|n| [(0.5 * (step * n as f64 + 0.25 * std::f64::consts::PI).sin()) as f32; 2])
            .collect();
        let read = measure(&samples, RATE);
        let crest = decibels(0.5);
        assert!((read.sample_peak_db - (crest - 3.01)).abs() < 0.05, "{read:?}");
        assert!((read.true_peak_db - crest).abs() < 0.3, "{read:?} against {crest}");
    }
}
