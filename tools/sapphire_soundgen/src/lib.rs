//! The placeholder music and voice lines (M3 A1): synthesized here from nothing but numbers, and
//! encoded to Ogg Vorbis, so the engine has stems to fade by phase and lines to route by language with
//! no recording and no download. Own work, under the workspace's license.
//!
//! - **Music:** one loop of four bars of 4/4 at 120 beats a minute, eight seconds, as four stems that
//!   start together: a pad, a pulse in the bass, drums and bells over A minor's i, VI, III, VII. Each
//!   is rendered into a ring the loop's length, so what rings past the end comes round at the start,
//!   and the loop has no seam but the codec's.
//! - **Voice:** two lines, `perfect-dodge` and `swap-in`, in English and in Japanese: no words, only
//!   a voice's shape. A pulse train at the speaker's pitch is shaped by two formants moving between
//!   vowels, a syllable at a time; English lines are timed by stress and Japanese ones by even morae
//!   with a falling pitch accent, so the two languages sound apart. No system text-to-speech voice is
//!   used: their licences don't allow redistribution.
//!
//! Everything is a pure function of constants. Noise comes from a seeded xorshift, the Ogg stream
//! serial is fixed, and libvorbis has no other input, so a run makes the same bytes as the last on
//! the same machine; another machine's floating point may differ in the last bits.
//!
//! # Does not own
//!
//! - The documents that name these sounds (`.score.ron`, `.voice.ron` and the lines' `.cue.ron`
//!   beside them), which are written by hand, nor their sidecars, which the first cook makes.

use std::f64::consts::TAU;
use std::num::{NonZeroU8, NonZeroU32};

use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};

/// Every file's sample rate: plenty for placeholders, and a quarter less to decode than 32 kHz.
pub const SAMPLE_RATE: u32 = 24_000;
/// The music's tempo, ...
pub const TEMPO_BPM: f64 = 120.0;
/// ... its bar, ...
pub const BEATS_PER_BAR: u32 = 4;
/// ... and its loop.
pub const BARS: u32 = 4;
/// The loop's length in frames: 8 s.
pub const LOOP_FRAMES: usize =
    (SAMPLE_RATE as f64 * 60.0 / TEMPO_BPM) as usize * (BEATS_PER_BAR * BARS) as usize;

/// Libvorbis's quality, from -0.2 to 1: about 40 kbit/s for these.
const QUALITY: f32 = 0.3;
/// Every stem's and line's loudest sample.
const PEAK: f32 = 0.7;

/// One file: where it goes under `content/`, and its samples, mono.
pub struct Generated {
    pub path: &'static str,
    pub samples: Vec<f32>,
}

/// Every file, in a fixed order.
pub fn generate() -> Vec<Generated> {
    let stems = [
        ("audio/generated/music/lantern_pad.ogg", pad()),
        ("audio/generated/music/lantern_pulse.ogg", pulse()),
        ("audio/generated/music/lantern_drums.ogg", drums()),
        ("audio/generated/music/lantern_bells.ogg", bells()),
    ];
    let lines = [
        ("audio/generated/voice/en/perfect-dodge.ogg", speak(EN_PERFECT_DODGE)),
        ("audio/generated/voice/en/swap-in.ogg", speak(EN_SWAP_IN)),
        ("audio/generated/voice/ja/perfect-dodge.ogg", speak(JA_PERFECT_DODGE)),
        ("audio/generated/voice/ja/swap-in.ogg", speak(JA_SWAP_IN)),
    ];
    stems
        .into_iter()
        .chain(lines)
        .map(|(path, samples)| Generated { path, samples: normalized(samples) })
        .collect()
}

impl Generated {
    /// The file's bytes, its stream serial [`serial_of`] its path. The decoder the engine uses (lewton)
    /// doesn't trim the last block to the stream's end, so a sound can come back a block longer than it
    /// went in: a line is given that much quiet at its end and encoded again, until it comes back as
    /// long as it is. A stem can't be: it must stay its bars, and this fails instead.
    pub fn encode(&mut self) -> anyhow::Result<Vec<u8>> {
        let is_stem = self.path.contains("/music/");
        for _ in 0..4 {
            let bytes = encode(&self.samples, serial_of(self.path))?;
            let frames = sapphire_assets::sound::decode_vorbis(&bytes)?.frames();
            if frames == self.samples.len() {
                return Ok(bytes);
            }
            anyhow::ensure!(
                !is_stem,
                "{} decodes to {frames} frames, not its {} frames of bars",
                self.path,
                self.samples.len()
            );
            self.samples.resize(frames, 0.0);
        }
        anyhow::bail!("{} never decodes to its own length", self.path)
    }
}

/// `samples` as an Ogg Vorbis stream, mono at [`SAMPLE_RATE`], with the stream serial `serial`.
pub fn encode(samples: &[f32], serial: i32) -> anyhow::Result<Vec<u8>> {
    let (Some(rate), Some(mono)) = (NonZeroU32::new(SAMPLE_RATE), NonZeroU8::new(1)) else {
        unreachable!("neither is zero")
    };
    let mut builder = VorbisEncoderBuilder::new_with_serial(rate, mono, Vec::new(), serial);
    builder
        .bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr { target_quality: QUALITY });
    let mut encoder = builder.build()?;
    for block in samples.chunks(4_096) {
        encoder.encode_audio_block([block])?;
    }
    Ok(encoder.finish()?)
}

/// A stable serial for the file at `path`: FNV-1a, folded to 31 bits.
pub fn serial_of(path: &str) -> i32 {
    let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    (hash & 0x7fff_ffff) as i32
}

fn normalized(mut samples: Vec<f32>) -> Vec<f32> {
    let peak = samples.iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if peak > 0.0 {
        for sample in &mut samples {
            *sample *= PEAK / peak;
        }
    }
    samples
}

/// Seconds of a beat.
const BEAT: f64 = 60.0 / TEMPO_BPM;
/// Seconds of a bar.
const BAR: f64 = BEAT * BEATS_PER_BAR as f64;

/// The hertz of MIDI note `note`.
fn hz(note: i32) -> f64 {
    440.0 * 2.0_f64.powf(f64::from(note - 69) / 12.0)
}

/// Each bar's chord, MIDI notes: A minor, F, C, G.
const CHORDS: [[i32; 3]; BARS as usize] = [[57, 60, 64], [53, 57, 60], [60, 64, 67], [55, 59, 62]];

/// A ring the loop's length that notes are added into, wrapping.
struct Ring(Vec<f32>);

impl Ring {
    fn new() -> Self {
        Self(vec![0.0; LOOP_FRAMES])
    }

    /// Adds `seconds` of `voice(t)`, from `start` seconds into the loop.
    fn add(&mut self, start: f64, seconds: f64, mut voice: impl FnMut(f64) -> f64) {
        let first = (start * f64::from(SAMPLE_RATE)).round() as usize;
        let frames = (seconds * f64::from(SAMPLE_RATE)) as usize;
        for frame in 0..frames {
            let t = frame as f64 / f64::from(SAMPLE_RATE);
            self.0[(first + frame) % LOOP_FRAMES] += voice(t) as f32;
        }
    }
}

/// Noise from a seeded xorshift: the same every run.
struct Noise(u64);

impl Noise {
    /// From -1 to 1.
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1_u64 << 52) as f64 - 1.0
    }
}

/// A held chord a bar long, of soft detuned harmonics, swelling in and ringing into the next bar.
fn pad() -> Vec<f32> {
    let mut ring = Ring::new();
    for (bar, chord) in CHORDS.iter().enumerate() {
        for &note in chord {
            let base = hz(note);
            ring.add(bar as f64 * BAR, BAR + 0.8, |t| {
                let swell = (t / 0.5).min(1.0) * if t > BAR { (1.0 - (t - BAR) / 0.8).max(0.0) } else { 1.0 };
                let tone: f64 = (1..=6)
                    .map(|harmonic| {
                        let h = f64::from(harmonic);
                        // Two oscillators 4 cents apart beat slowly, which reads as a pad.
                        ((TAU * base * h * t).sin() + (TAU * base * 1.0023 * h * t).sin()) / (h * h)
                    })
                    .sum();
                tone * swell
            });
        }
    }
    ring.0
}

/// The chord's root two octaves down, on every eighth note, plucked.
fn pulse() -> Vec<f32> {
    let mut ring = Ring::new();
    for (bar, chord) in CHORDS.iter().enumerate() {
        let root = hz(chord[0] - 24);
        for eighth in 0..BEATS_PER_BAR * 2 {
            let start = bar as f64 * BAR + f64::from(eighth) * BEAT / 2.0;
            let accent = if eighth % 2 == 0 { 1.0 } else { 0.7 };
            ring.add(start, 0.3, |t| {
                let tone = (TAU * root * t).sin() + 0.35 * (TAU * 2.0 * root * t).sin();
                tone * (-t / 0.12).exp() * (t / 0.004).min(1.0) * accent
            });
        }
    }
    ring.0
}

/// Kick on one and three, snare on two and four, hats on the eighths, and a pickup into the loop's top.
fn drums() -> Vec<f32> {
    let mut ring = Ring::new();
    let mut noise = Noise(0x5AFF_1BE5_0D15_C0DE);
    let kick = |t: f64| {
        // A sine falling from 125 Hz to 45 Hz, its phase the integral of that sweep.
        let phase = 45.0 * t + 80.0 * 0.04 * (1.0 - (-t / 0.04).exp());
        (TAU * phase).sin() * (-t / 0.25).exp()
    };
    for bar in 0..BARS {
        let top = f64::from(bar) * BAR;
        let mut kicks = vec![0.0, 2.0];
        if bar == BARS - 1 {
            kicks.push(3.5);
        }
        for beat in kicks {
            ring.add(top + beat * BEAT, 0.45, kick);
        }
        for beat in [1.0, 3.0] {
            let burst: Vec<f64> =
                (0..(0.25 * f64::from(SAMPLE_RATE)) as usize).map(|_| noise.next()).collect();
            ring.add(top + beat * BEAT, 0.25, |t| {
                let sample = burst[((t * f64::from(SAMPLE_RATE)) as usize).min(burst.len() - 1)];
                0.5 * sample * (-t / 0.09).exp() + 0.4 * (TAU * 190.0 * t).sin() * (-t / 0.06).exp()
            });
        }
        for eighth in 0..BEATS_PER_BAR * 2 {
            let burst: Vec<f64> =
                (0..(0.08 * f64::from(SAMPLE_RATE)) as usize + 1).map(|_| noise.next()).collect();
            ring.add(top + f64::from(eighth) * BEAT / 2.0, 0.08, |t| {
                // A first difference of noise: its highs.
                let at = ((t * f64::from(SAMPLE_RATE)) as usize).min(burst.len() - 2);
                0.18 * (burst[at + 1] - burst[at]) * (-t / 0.025).exp()
            });
        }
    }
    ring.0
}

/// A tune on A minor's pentatonic, a bell for each quarter note, struck by frequency modulation.
fn bells() -> Vec<f32> {
    // A rest is `None`. Sixteen quarter notes: four bars.
    const TUNE: [Option<i32>; 16] = [
        Some(76),
        None,
        Some(79),
        Some(81),
        Some(77),
        None,
        Some(76),
        Some(72),
        Some(79),
        None,
        Some(76),
        Some(74),
        Some(74),
        Some(76),
        None,
        Some(69),
    ];
    let mut ring = Ring::new();
    for (beat, note) in TUNE.iter().enumerate() {
        let Some(note) = *note else { continue };
        let carrier = hz(note);
        ring.add(beat as f64 * BEAT, 1.6, |t| {
            let index = 2.5 * (-t / 0.3).exp();
            (TAU * carrier * t + index * (TAU * 3.5 * carrier * t).sin()).sin()
                * (-t / 0.7).exp()
                * (t / 0.002).min(1.0)
        });
    }
    ring.0
}

/// A vowel by its first two formants, in hertz.
#[derive(Clone, Copy)]
struct Vowel(f64, f64);

const A: Vowel = Vowel(800.0, 1_250.0);
const E: Vowel = Vowel(500.0, 1_850.0);
const I: Vowel = Vowel(320.0, 2_300.0);
const O: Vowel = Vowel(480.0, 850.0);
const U: Vowel = Vowel(360.0, 1_100.0);

/// One syllable: its vowel, its length in seconds, its pitch over the speaker's, and whether a
/// consonant's burst of noise leads it in.
#[derive(Clone, Copy)]
struct Syllable(Vowel, f64, f64, bool);

/// The speaker's pitch, in hertz: the character's, in both languages.
const SPEAKER_HZ: f64 = 230.0;

/// Stress-timed: the long, low last syllable carries the line.
const EN_PERFECT_DODGE: &[Syllable] =
    &[Syllable(E, 0.13, 1.12, true), Syllable(E, 0.10, 1.0, true), Syllable(O, 0.30, 0.86, true)];
const EN_SWAP_IN: &[Syllable] = &[Syllable(A, 0.24, 1.15, true), Syllable(I, 0.30, 0.92, false)];
/// Even morae, high then falling after the accent.
const JA_PERFECT_DODGE: &[Syllable] = &[
    Syllable(I, 0.11, 1.0, true),
    Syllable(A, 0.11, 1.18, true),
    Syllable(U, 0.11, 1.18, true),
    Syllable(O, 0.11, 0.95, true),
    Syllable(A, 0.11, 0.9, false),
    Syllable(E, 0.14, 0.84, true),
];
const JA_SWAP_IN: &[Syllable] = &[
    Syllable(O, 0.11, 1.0, false),
    Syllable(A, 0.11, 1.2, true),
    Syllable(I, 0.11, 1.0, true),
    Syllable(E, 0.11, 0.9, true),
    Syllable(U, 0.15, 0.84, false),
];

/// A response peak at `formant` hertz, `width` wide.
fn resonance(frequency: f64, formant: f64, width: f64) -> f64 {
    1.0 / (1.0 + ((frequency - formant) / width).powi(2))
}

/// `syllables` spoken, with a tenth of a second's quiet either side.
fn speak(syllables: &[Syllable]) -> Vec<f32> {
    const GAP: f64 = 0.1;
    const BURST: f64 = 0.035;
    let rate = f64::from(SAMPLE_RATE);
    let seconds: f64 = 2.0 * GAP
        + syllables.iter().map(|syllable| syllable.1 + if syllable.3 { BURST } else { 0.0 }).sum::<f64>();
    let mut samples = vec![0.0_f32; (seconds * rate) as usize];
    let mut noise = Noise(0x0B0E_5EED_0000_0001);
    let mut at = (GAP * rate) as usize;
    let mut phase = 0.0_f64;
    for (index, &Syllable(vowel, length, pitch, burst)) in syllables.iter().enumerate() {
        if burst {
            let frames = (BURST * rate) as usize;
            let mut last = 0.0;
            for frame in 0..frames {
                let next = noise.next();
                let envelope = (frame as f64 / frames as f64 * std::f64::consts::PI).sin();
                samples[at + frame] += (0.25 * (next - last) * envelope) as f32;
                last = next;
            }
            at += frames;
        }
        // The vowel glides from the last one's formants, and the pitch to the next syllable's.
        let from = if index > 0 { syllables[index - 1].0 } else { vowel };
        let next_pitch = syllables.get(index + 1).map_or(pitch * 0.92, |next| next.2);
        let frames = (length * rate) as usize;
        for frame in 0..frames {
            let t = frame as f64 / frames as f64;
            let glide = (t / 0.3).min(1.0);
            let (f1, f2) = (from.0 + (vowel.0 - from.0) * glide, from.1 + (vowel.1 - from.1) * glide);
            let vibrato = 1.0 + 0.012 * (TAU * 5.5 * (at + frame) as f64 / rate).sin();
            let f0 = SPEAKER_HZ * (pitch + (next_pitch - pitch) * t * t) * vibrato;
            phase += f0 / rate;
            let mut sample = 0.0;
            let mut harmonic = 1.0;
            while harmonic * f0 < 3_400.0 {
                let shape = resonance(harmonic * f0, f1, 90.0) + 0.7 * resonance(harmonic * f0, f2, 130.0);
                sample += shape / harmonic * (TAU * harmonic * phase).sin();
                harmonic += 1.0;
            }
            let envelope = (t * length / 0.015).min(1.0) * ((1.0 - t) * length / 0.03).min(1.0);
            samples[at + frame] += (sample * envelope) as f32;
        }
        at += frames;
    }
    samples
}

#[cfg(test)]
mod tests {
    use sapphire_assets::sound::decode_vorbis;

    use super::*;

    #[test]
    fn every_file_comes_out_the_same_twice_and_as_long_as_it_must_be() {
        let mut first = generate();
        let mut again = generate();
        assert_eq!(first.len(), 8);
        for (file, twin) in first.iter_mut().zip(&mut again) {
            let bytes = file.encode().unwrap();
            assert_eq!(bytes, twin.encode().unwrap(), "{}", file.path);
            let decoded = decode_vorbis(&bytes).unwrap();
            assert_eq!((decoded.sample_rate, decoded.channels), (SAMPLE_RATE, 1), "{}", file.path);
            // What the cook holds a stem to: the loop, to the frame.
            assert_eq!(decoded.frames(), file.samples.len(), "{}", file.path);
            if file.path.contains("/music/") {
                assert_eq!(file.samples.len(), LOOP_FRAMES, "{}", file.path);
            }
        }
        assert_eq!(LOOP_FRAMES, 192_000);
    }

    #[test]
    fn the_languages_lines_differ_and_none_is_silent() {
        let files = generate();
        let line = |path: &str| &files.iter().find(|file| file.path.ends_with(path)).unwrap().samples;
        for path in ["en/perfect-dodge.ogg", "ja/perfect-dodge.ogg", "en/swap-in.ogg", "ja/swap-in.ogg"] {
            let peak = line(path).iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            assert!((peak - PEAK).abs() < 1e-6, "{path}: {peak}");
        }
        assert_ne!(line("en/perfect-dodge.ogg").len(), line("ja/perfect-dodge.ogg").len());
    }

    #[test]
    fn a_stem_loops_with_no_step_at_its_seam() {
        // What rings past the end came round to the start: the last sample and the first are close, as
        // any two neighbours are.
        for file in generate().iter().filter(|file| file.path.contains("/music/")) {
            let steps = file.samples.windows(2).map(|pair| (pair[1] - pair[0]).abs());
            let largest = steps.fold(0.0_f32, f32::max);
            let seam = (file.samples[0] - file.samples[LOOP_FRAMES - 1]).abs();
            assert!(
                seam <= largest,
                "{}: the seam steps {seam}, more than anywhere else ({largest})",
                file.path
            );
        }
    }
}
