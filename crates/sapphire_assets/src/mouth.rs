//! Mouth curves (M5 C6, spec §5.13 and §5.15): how open a voice's mouth is, and in what shape, sixty times
//! a second, cooked from the voice sound it speaks and kept in that sound's `.ssound`
//! ([`Sound::mouth`](crate::sound::Sound::mouth)).
//!
//! Lip-sync is cooked rather than heard (m5-plan §3, "Lip-sync"): a curve repeats, a golden can hold it, and
//! a face reads it at a line's playhead with nothing from the audio thread. Each frame of a curve has two
//! parts, in the order they can be trusted:
//!
//! - **The envelope** (`open`): the sound's loudness around that moment against its loudest, over the
//!   [`RANGE_DB`] below it. It's what a mouth flaps by, and on any voice it says when the mouth moves.
//! - **Five visemes** (`aa`, `ih`, `ou`, `ee`, `oh`, VRM's order): the envelope shared among the vowels by a
//!   coarse spectral guess. The first two formants are taken as the strongest parts of the spectrum's
//!   smoothed envelope in a low and a higher band, and each vowel's share falls with the distance from
//!   its typical adult formants. It has no phoneme data and no language model: consonants, nasals,
//!   whispers, voices far from those formants (a child's, a shout) and music under the voice all guess
//!   wrong, and a recording's lip-sync from phonemes would replace it. On the project's generated lines,
//!   whose vowels are formant pairs near those references, it finds the held vowels; that proves the
//!   timing and the arithmetic, not lip-sync on speech.
//!
//! **The same bytes on every machine.** [`MouthCurve::analyze`] runs at cook time on plain IEEE arithmetic in
//! a fixed order, with its two transcendentals through `sapphire_math::det` (a cosine for the window and the
//! filters, a logarithm for decibels), so the curve is a pure function of the decoded samples. Those come
//! from `lewton`, whose window and transform tables are built with `std`'s sine and cosine, so a platform
//! whose C library rounds one of them differently could decode a sample a bit or two apart. Every value
//! here is an average over hundreds of samples, quantized to 1/255, so such a difference moves no byte
//! unless a value sits within about a millionth of a step of a rounding edge: `the_curves_ignore_a_decoders_last_bits`
//! holds the committed lines away from that, and CI's cooks-agree job compares the bytes across machines.

use sapphire_math::det;

use crate::sound::DecodedSound;

/// Frames a second.
pub const MOUTH_RATE: u32 = 60;

/// How many visemes a frame holds.
pub const VISEMES: usize = 5;

/// The loudest level of anything in a frame.
pub const FULL: u8 = u8::MAX;

/// How far under the sound's loudest moment the envelope reaches: quieter is a closed mouth.
pub const RANGE_DB: f32 = 30.0;

/// Typical adult formants, F1 and F2 in hertz, of the visemes' vowels in [`MouthFrame::visemes`]' order:
/// a, i, u, e and o.
const FORMANTS: [(f64, f64); VISEMES] =
    [(800.0, 1_250.0), (320.0, 2_300.0), (360.0, 1_100.0), (500.0, 1_850.0), (480.0, 850.0)];
/// How far a formant may be from a vowel's and still count as near it, in hertz: F1's, then F2's.
const FORMANT_SPREAD: (f64, f64) = (150.0, 350.0);

/// The analysis' filters: every [`BIN_HZ`] hertz from [`LOWEST_HZ`] to [`HIGHEST_HZ`].
const LOWEST_HZ: u32 = 150;
const HIGHEST_HZ: u32 = 3_400;
const BIN_HZ: u32 = 50;
const BINS: usize = ((HIGHEST_HZ - LOWEST_HZ) / BIN_HZ + 1) as usize;
/// How many bins either side the spectrum's envelope averages over: past a voice's harmonic spacing.
const SMOOTHING: usize = 3;
/// Where F1 is looked for, in hertz, and how far above it F2 starts.
const F1_BAND: (u32, u32) = (250, 950);
const F2_GAP: f64 = 300.0;

/// A cooked curve. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct MouthCurve {
    /// Frame `i` is the mouth at `i / 60` seconds into the sound, from the first sample to just past the
    /// last: [`MouthCurve::frames_for`] of them.
    pub frames: Vec<MouthFrame>,
}

/// One moment of a curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct MouthFrame {
    /// The envelope, 0 (closed) to [`FULL`].
    pub open: u8,
    /// `aa`, `ih`, `ou`, `ee` and `oh`, each 0 to [`FULL`]; together about `open`, and all 0 where it is.
    pub visemes: [u8; VISEMES],
}

/// A curve read at a moment, each weight 0 to 1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MouthSample {
    pub open: f32,
    pub visemes: [f32; VISEMES],
}

impl MouthSample {
    /// A closed mouth.
    pub const CLOSED: Self = Self { open: 0.0, visemes: [0.0; VISEMES] };
}

impl MouthFrame {
    fn sample(self) -> MouthSample {
        let weight = |level: u8| f32::from(level) / f32::from(FULL);
        MouthSample { open: weight(self.open), visemes: self.visemes.map(weight) }
    }
}

impl MouthCurve {
    /// How many frames a curve of a sound `frames` long at `sample_rate` has: one for each sixtieth of a
    /// second it starts, and one past its end.
    #[must_use]
    pub const fn frames_for(frames: u32, sample_rate: u32) -> usize {
        let sixtieths = (frames as u64 * MOUTH_RATE as u64).div_ceil(sample_rate as u64);
        sixtieths as usize + 1
    }

    /// The curve of `sound`: the cook's analysis. See the [module docs](self).
    #[must_use]
    pub fn analyze(sound: &DecodedSound) -> Self {
        let rate = sound.sample_rate.max(1);
        let mono = mono(sound);
        let count = Self::frames_for(mono.len() as u32, rate);
        // Each frame looks a sixtieth of a second either side of its moment, under a Hann window.
        let half = (rate / MOUTH_RATE) as usize;
        let window: Vec<f64> = (0..=2 * half)
            .map(|at| {
                let x = (at as f64 - half as f64) / (half + 1) as f64;
                f64::from(0.5 + 0.5 * det::cos(std::f32::consts::PI * x as f32))
            })
            .collect();
        let window_sum: f64 = window.iter().sum();
        let coefficients: Vec<f64> = (0..BINS)
            .map(|bin| {
                let hz = (LOWEST_HZ + bin as u32 * BIN_HZ) as f64;
                2.0 * f64::from(det::cos((std::f64::consts::TAU * hz / f64::from(rate)) as f32))
            })
            .collect();

        let mut windowed = vec![0.0_f64; window.len()];
        let mut powers = Vec::with_capacity(count);
        let mut shapes = Vec::with_capacity(count);
        for frame in 0..count {
            let center = (frame as u64 * u64::from(rate) + u64::from(MOUTH_RATE / 2)) / u64::from(MOUTH_RATE);
            let mut power = 0.0;
            for (at, (weighted, &weight)) in windowed.iter_mut().zip(&window).enumerate() {
                let index = (center + at as u64).checked_sub(half as u64);
                let sample = index.and_then(|index| mono.get(index as usize)).copied().unwrap_or(0.0);
                *weighted = sample * weight;
                power += *weighted * sample;
            }
            powers.push(power / window_sum);
            shapes.push(shape(&windowed, &coefficients));
        }

        let loudest = powers.iter().copied().fold(0.0, f64::max);
        let frames = powers
            .iter()
            .zip(&shapes)
            .map(|(&power, shares)| {
                if loudest <= 0.0 || power <= 0.0 {
                    return MouthFrame::default();
                }
                // 10 log10 of the power ratio, through a pinned log2.
                let below_db =
                    10.0 / std::f64::consts::LOG2_10 * f64::from(det::log2((power / loudest) as f32));
                let open = (1.0 + below_db / f64::from(RANGE_DB)).clamp(0.0, 1.0);
                MouthFrame { open: level(open), visemes: shares.map(|share| level(open * share)) }
            })
            .collect();
        Self { frames }
    }

    /// The mouth `seconds` into the sound, between the frames either side: closed before it starts and
    /// after it ends. Allocates nothing.
    #[must_use]
    pub fn at(&self, seconds: f32) -> MouthSample {
        let place = seconds * MOUTH_RATE as f32;
        // Not a number, before the start, or past the end.
        if !(place >= 0.0 && place < self.frames.len() as f32) {
            return MouthSample::CLOSED;
        }
        let index = place as usize;
        let (Some(from), Some(to)) = (self.frames.get(index), self.frames.get(index + 1)) else {
            return MouthSample::CLOSED;
        };
        let (from, to) = (from.sample(), to.sample());
        let t = place - index as f32;
        let mix = |a: f32, b: f32| a + (b - a) * t;
        MouthSample {
            open: mix(from.open, to.open),
            visemes: std::array::from_fn(|viseme| mix(from.visemes[viseme], to.visemes[viseme])),
        }
    }
}

impl ArchivedMouthCurve {
    /// The curve, out of its pack.
    #[must_use]
    pub fn to_curve(&self) -> MouthCurve {
        let frames =
            self.frames.iter().map(|frame| MouthFrame { open: frame.open, visemes: frame.visemes }).collect();
        MouthCurve { frames }
    }
}

/// A weight 0 to 1 as a byte, rounded to the nearest.
fn level(weight: f64) -> u8 {
    (weight.clamp(0.0, 1.0) * f64::from(FULL) + 0.5) as u8
}

/// The sound's channels averaged, in a fixed order.
fn mono(sound: &DecodedSound) -> Vec<f64> {
    let channels = sound.channels.max(1) as usize;
    sound
        .samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().map(|&sample| f64::from(sample)).sum::<f64>() / channels as f64)
        .collect()
}

/// Each viseme's share of a frame whose windowed samples are `windowed`, together 1: the vowels nearest
/// the frame's two formants weigh most.
fn shape(windowed: &[f64], coefficients: &[f64]) -> [f64; VISEMES] {
    // Goertzel's filter at each bin.
    let mut spectrum = [0.0_f64; BINS];
    for (power, &coefficient) in spectrum.iter_mut().zip(coefficients) {
        let (mut last, mut before) = (0.0_f64, 0.0_f64);
        for &sample in windowed {
            let next = sample + coefficient * last - before;
            before = last;
            last = next;
        }
        *power = (last * last + before * before - coefficient * last * before).max(0.0);
    }
    // The envelope: each bin with its neighbours, so a voice's harmonics blur into its formants.
    let envelope: [f64; BINS] = std::array::from_fn(|bin| {
        let (first, last) = (bin.saturating_sub(SMOOTHING), (bin + SMOOTHING).min(BINS - 1));
        spectrum[first..=last].iter().sum::<f64>() / (last - first + 1) as f64
    });
    let first = peak(&envelope, f64::from(F1_BAND.0), f64::from(F1_BAND.1));
    let second = peak(&envelope, first + F2_GAP, f64::from(HIGHEST_HZ - BIN_HZ * SMOOTHING as u32));
    let near = FORMANTS.map(|(f1, f2)| {
        let (d1, d2) = ((first - f1) / FORMANT_SPREAD.0, (second - f2) / FORMANT_SPREAD.1);
        let nearness = 1.0 / (0.05 + d1 * d1 + d2 * d2);
        nearness * nearness
    });
    let total: f64 = near.iter().sum();
    near.map(|weight| weight / total)
}

/// Where `envelope` is strongest from `low` to `high` hertz: its centroid with each bin weighed by its square,
/// so a peak wins without a jump from bin to bin.
fn peak(envelope: &[f64; BINS], low: f64, high: f64) -> f64 {
    let (mut weighed, mut total) = (0.0, 0.0);
    for (bin, &power) in envelope.iter().enumerate() {
        let hz = f64::from(LOWEST_HZ + bin as u32 * BIN_HZ);
        if (low..=high).contains(&hz) {
            weighed += hz * power * power;
            total += power * power;
        }
    }
    if total > 0.0 { weighed / total } else { (low + high) / 2.0 }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::sound::Sound;

    const RATE: u32 = 24_000;

    /// A voice at 140 Hz holding a vowel with formants `f1` and `f2` from `from` to `to` seconds, silent
    /// around it, `length` seconds in all: every harmonic up to 3.4 kHz, each as loud as it's near a formant.
    fn vowel((f1, f2): (f64, f64), from: f64, to: f64, length: f64) -> DecodedSound {
        let pitch = 140.0;
        let near = |hz: f64, formant: f64| {
            let off = (hz - formant) / 90.0;
            1.0 / (1.0 + off * off)
        };
        let harmonics: Vec<(f64, f64)> = (1..)
            .map(|k| f64::from(k) * pitch)
            .take_while(|&hz| hz < 3_400.0)
            .map(|hz| (hz, near(hz, f1) + 0.6 * near(hz, f2)))
            .collect();
        let frames = (length * f64::from(RATE)) as usize;
        let samples = (0..frames)
            .map(|frame| {
                let t = frame as f64 / f64::from(RATE);
                if !(from..to).contains(&t) {
                    return 0.0;
                }
                let wave: f64 = harmonics
                    .iter()
                    .map(|&(hz, gain)| {
                        let turns = (hz * t).fract();
                        gain * f64::from(det::sin(std::f32::consts::TAU * turns as f32))
                    })
                    .sum();
                (wave * 0.1) as f32
            })
            .collect();
        DecodedSound { sample_rate: RATE, channels: 1, samples }
    }

    fn strongest(frame: MouthFrame) -> usize {
        (0..VISEMES).max_by_key(|&viseme| frame.visemes[viseme]).unwrap()
    }

    #[test]
    fn a_curve_has_a_frame_each_sixtieth_and_one_past_the_end() {
        assert_eq!(MouthCurve::frames_for(0, RATE), 1);
        assert_eq!(MouthCurve::frames_for(400, RATE), 2);
        assert_eq!(MouthCurve::frames_for(401, RATE), 3);
        assert_eq!(MouthCurve::frames_for(RATE, RATE), 61);
        let silent = DecodedSound { sample_rate: RATE, channels: 2, samples: vec![0.0; 960] };
        assert_eq!(MouthCurve::analyze(&silent).frames, vec![MouthFrame::default(); 3]);
        let empty = DecodedSound { sample_rate: RATE, channels: 1, samples: Vec::new() };
        assert_eq!(MouthCurve::analyze(&empty).frames.len(), 1);
    }

    #[test]
    fn the_envelope_opens_and_shuts_with_the_sound() {
        let curve = MouthCurve::analyze(&vowel(FORMANTS[0], 0.5, 1.0, 1.5));
        assert_eq!(curve.frames.len(), 91);
        for (index, frame) in curve.frames.iter().enumerate() {
            match index {
                // A frame's window reaches a sixtieth either side: frames 30 and 60 see half the tone.
                0..=29 | 61.. => assert_eq!(*frame, MouthFrame::default(), "frame {index}"),
                31..=59 => {
                    assert!(frame.open >= 250, "frame {index}: {frame:?}");
                    let shared: u32 = frame.visemes.iter().map(|&level| u32::from(level)).sum();
                    assert!(shared.abs_diff(u32::from(frame.open)) <= 3, "frame {index}: {frame:?}");
                }
                _ => assert!(frame.open < 250, "frame {index}: {frame:?}"),
            }
        }
    }

    /// The spectral guess on what it was made for: a held vowel near its reference formants.
    #[test]
    fn a_held_vowel_finds_its_viseme() {
        for (viseme, formants) in FORMANTS.into_iter().enumerate() {
            let curve = MouthCurve::analyze(&vowel(formants, 0.0, 0.5, 0.5));
            let held = curve.frames[15];
            assert_eq!(strongest(held), viseme, "{formants:?}: {held:?}");
            assert!(held.visemes[viseme] > 128, "{formants:?}: {held:?}");
        }
    }

    #[test]
    fn a_curve_reads_between_its_frames_and_shut_outside_them() {
        let open = MouthFrame { open: FULL, visemes: [FULL, 0, 0, 0, 0] };
        let curve = MouthCurve { frames: vec![MouthFrame::default(), open, MouthFrame::default()] };
        let half = curve.at(0.5 / 60.0);
        assert!((half.open - 0.5).abs() < 1e-6 && (half.visemes[0] - 0.5).abs() < 1e-6, "{half:?}");
        assert_eq!(curve.at(1.0 / 60.0), MouthSample { open: 1.0, visemes: [1.0, 0.0, 0.0, 0.0, 0.0] });
        for seconds in [2.0 / 60.0, 1.0, -0.001, f32::NAN, f32::INFINITY] {
            assert_eq!(curve.at(seconds), MouthSample::CLOSED, "{seconds}");
        }
    }

    /// Another machine's decoder could round a sample a bit or two apart (see the module docs): the committed
    /// voices' curves don't move for a few ulps either way on every sample.
    #[test]
    fn the_curves_ignore_a_decoders_last_bits() {
        let voices = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/audio/generated/voice");
        let mut state = 0x0B0E_5EED_u64;
        for line in ["perfect-dodge", "swap-in", "guide_greet"] {
            for language in ["en", "ja"] {
                let path = voices.join(language).join(format!("{line}.ogg"));
                let sound = Sound::from_vorbis(fs::read(&path).unwrap()).unwrap().decode().unwrap();
                let curve = MouthCurve::analyze(&sound);
                assert!(curve.frames.iter().any(|frame| frame.open == FULL), "{}", path.display());
                let mut nudged = sound.clone();
                for sample in &mut nudged.samples {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let ulps = (state % 7) as i32 - 3;
                    if *sample != 0.0 {
                        *sample = f32::from_bits(sample.to_bits().wrapping_add_signed(ulps));
                    }
                }
                assert_ne!(nudged.samples, sound.samples);
                assert_eq!(MouthCurve::analyze(&nudged), curve, "{}", path.display());
            }
        }
    }
}
