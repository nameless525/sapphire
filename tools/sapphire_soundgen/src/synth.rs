//! The instruments and the voice: how each part sounds, played from what its spec says. Each is
//! written so that a spec of M3's constants makes M3's samples to the bit, which the committed files
//! hold: change an expression's order here and every stem it plays moves.

use std::f64::consts::TAU;

use crate::SAMPLE_RATE;
use crate::spec::{Bells, Drums, Line, Pad, Part, Pulse, Stem, Syllable, Toll, Vowel};

/// A stem's samples, its loop's length.
pub(crate) fn stem(stem: &Stem) -> Vec<f32> {
    let frames = stem.frames_per_beat() as usize * (stem.beats_per_bar * stem.bars) as usize;
    let beat = 60.0 / stem.tempo_bpm;
    let clock = Clock {
        beat,
        bar: beat * f64::from(stem.beats_per_bar),
        beats_per_bar: stem.beats_per_bar,
        bars: stem.bars,
    };
    let mut ring = Ring(vec![0.0; frames]);
    match &stem.part {
        Part::Pad(pad) => play_pad(&mut ring, &clock, pad),
        Part::Pulse(pulse) => play_pulse(&mut ring, &clock, pulse),
        Part::Drums(drums) => play_drums(&mut ring, &clock, drums),
        Part::Bells(bells) => play_bells(&mut ring, &clock, bells),
        Part::Toll(toll) => play_toll(&mut ring, &clock, toll),
    }
    ring.0
}

/// A stem's beat and bar, in seconds, and how many of each.
struct Clock {
    beat: f64,
    bar: f64,
    beats_per_bar: u32,
    bars: u32,
}

/// The hertz of MIDI note `note`.
fn hz(note: u8) -> f64 {
    440.0 * 2.0_f64.powf(f64::from(i32::from(note) - 69) / 12.0)
}

/// A ring the loop's length that notes are added into, wrapping.
struct Ring(Vec<f32>);

impl Ring {
    /// Adds `seconds` of `voice(t)`, from `start` seconds into the loop.
    fn add(&mut self, start: f64, seconds: f64, mut voice: impl FnMut(f64) -> f64) {
        let first = (start * f64::from(SAMPLE_RATE)).round() as usize;
        let frames = (seconds * f64::from(SAMPLE_RATE)) as usize;
        let length = self.0.len();
        for frame in 0..frames {
            let t = frame as f64 / f64::from(SAMPLE_RATE);
            self.0[(first + frame) % length] += voice(t) as f32;
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

/// Each chord held a bar, of six soft harmonics, swelling in over half a second and ringing 0.8 s into
/// the next bar.
fn play_pad(ring: &mut Ring, clock: &Clock, pad: &Pad) {
    const RELEASE: f64 = 0.8;
    let bar_length = clock.bar;
    for (bar, chord) in pad.chords.iter().enumerate() {
        for &note in chord {
            let base = hz(note);
            ring.add(bar as f64 * bar_length, bar_length + RELEASE, |t| {
                let swell = (t / 0.5).min(1.0)
                    * if t > bar_length { (1.0 - (t - bar_length) / RELEASE).max(0.0) } else { 1.0 };
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
}

/// Each bar's note with its octave, plucked `per_beat` times a beat, the off-beats at 0.7.
fn play_pulse(ring: &mut Ring, clock: &Clock, pulse: &Pulse) {
    for (bar, &note) in pulse.notes.iter().enumerate() {
        let root = hz(note);
        for pluck in 0..clock.beats_per_bar * pulse.per_beat {
            let start = bar as f64 * clock.bar + f64::from(pluck) * clock.beat / f64::from(pulse.per_beat);
            let accent = if pluck % pulse.per_beat == 0 { 1.0 } else { 0.7 };
            ring.add(start, 0.3, |t| {
                let tone = (TAU * root * t).sin() + 0.35 * (TAU * 2.0 * root * t).sin();
                tone * (-t / 0.12).exp() * (t / 0.004).min(1.0) * accent
            });
        }
    }
}

/// A kick falling from 125 Hz to 45 Hz, a snare of noise over a 190 Hz knock, and hats of the noise's
/// highs. Noise is drawn bar by bar, snares before hats, so a pattern's change moves what follows.
fn play_drums(ring: &mut Ring, clock: &Clock, drums: &Drums) {
    let rate = f64::from(SAMPLE_RATE);
    let mut noise = Noise(drums.seed);
    let kick = |t: f64| {
        // A sine falling from 125 Hz to 45 Hz, its phase the integral of that sweep.
        let phase = 45.0 * t + 80.0 * 0.04 * (1.0 - (-t / 0.04).exp());
        (TAU * phase).sin() * (-t / 0.25).exp()
    };
    for bar in 0..clock.bars {
        let top = f64::from(bar) * clock.bar;
        let fill = if bar + 1 == clock.bars { drums.fill.as_slice() } else { &[] };
        for &beat in drums.kick.iter().chain(fill) {
            ring.add(top + beat * clock.beat, 0.45, kick);
        }
        for &beat in &drums.snare {
            let burst: Vec<f64> = (0..(0.25 * rate) as usize).map(|_| noise.next()).collect();
            ring.add(top + beat * clock.beat, 0.25, |t| {
                let sample = burst[((t * rate) as usize).min(burst.len() - 1)];
                0.5 * sample * (-t / 0.09).exp() + 0.4 * (TAU * 190.0 * t).sin() * (-t / 0.06).exp()
            });
        }
        for hat in 0..clock.beats_per_bar * drums.hats_per_beat {
            let burst: Vec<f64> = (0..(0.08 * rate) as usize + 1).map(|_| noise.next()).collect();
            ring.add(top + f64::from(hat) * clock.beat / f64::from(drums.hats_per_beat), 0.08, |t| {
                // A first difference of noise: its highs.
                let at = ((t * rate) as usize).min(burst.len() - 2);
                0.18 * (burst[at + 1] - burst[at]) * (-t / 0.025).exp()
            });
        }
    }
}

/// A bell for each note of the tune, a beat apart, struck by frequency modulation and ringing 1.6 s.
fn play_bells(ring: &mut Ring, clock: &Clock, bells: &Bells) {
    for (beat, note) in bells.tune.iter().enumerate() {
        let Some(note) = *note else { continue };
        let carrier = hz(note);
        ring.add(beat as f64 * clock.beat, 1.6, |t| {
            let index = 2.5 * (-t / 0.3).exp();
            (TAU * carrier * t + index * (TAU * 3.5 * carrier * t).sin()).sin()
                * (-t / 0.7).exp()
                * (t / 0.002).min(1.0)
        });
    }
}

/// A great bell's partials, as multiples of its strike note, each with its share and how long it rings, in
/// seconds: the hum an octave down, the strike note, the minor third that makes a bell sound like one, the
/// fifth, the octave, and two upper partials that die first.
const TOLL_PARTIALS: [(f64, f64, f64); 7] = [
    (0.5, 0.6, 3.0),
    (1.0, 1.0, 2.2),
    (1.2, 0.5, 1.6),
    (1.5, 0.3, 1.2),
    (2.0, 0.45, 0.9),
    (2.5, 0.2, 0.5),
    (3.0, 0.15, 0.35),
];

/// A deep bell tolled on each bar's top, ringing on two bars.
fn play_toll(ring: &mut Ring, clock: &Clock, toll: &Toll) {
    for (bar, &note) in toll.notes.iter().enumerate() {
        let strike = hz(note);
        ring.add(bar as f64 * clock.bar, 2.0 * clock.bar, |t| {
            let tone: f64 = TOLL_PARTIALS
                .iter()
                .map(|&(ratio, share, rings)| share * (TAU * strike * ratio * t).sin() * (-t / rings).exp())
                .sum();
            tone * (t / 0.003).min(1.0)
        });
    }
}

/// A vowel's first two formants, in hertz.
fn formants(vowel: Vowel) -> (f64, f64) {
    match vowel {
        Vowel::A => (800.0, 1_250.0),
        Vowel::E => (500.0, 1_850.0),
        Vowel::I => (320.0, 2_300.0),
        Vowel::O => (480.0, 850.0),
        Vowel::U => (360.0, 1_100.0),
    }
}

/// A response peak at `formant` hertz, `width` wide.
fn resonance(frequency: f64, formant: f64, width: f64) -> f64 {
    1.0 / (1.0 + ((frequency - formant) / width).powi(2))
}

/// A line's syllables spoken, with a tenth of a second's quiet either side.
pub(crate) fn line(line: &Line) -> Vec<f32> {
    const GAP: f64 = 0.1;
    const BURST: f64 = 0.035;
    let syllables = &line.syllables;
    let rate = f64::from(SAMPLE_RATE);
    let seconds: f64 = 2.0 * GAP
        + syllables
            .iter()
            .map(|syllable| syllable.seconds + if syllable.burst { BURST } else { 0.0 })
            .sum::<f64>();
    let mut samples = vec![0.0_f32; (seconds * rate) as usize];
    let mut noise = Noise(line.seed);
    let mut at = (GAP * rate) as usize;
    let mut phase = 0.0_f64;
    for (index, &Syllable { vowel, seconds: length, pitch, burst }) in syllables.iter().enumerate() {
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
        let to = formants(vowel);
        let from = if index > 0 { formants(syllables[index - 1].vowel) } else { to };
        let next_pitch = syllables.get(index + 1).map_or(pitch * 0.92, |next| next.pitch);
        let frames = (length * rate) as usize;
        for frame in 0..frames {
            let t = frame as f64 / frames as f64;
            let glide = (t / 0.3).min(1.0);
            let (f1, f2) = (from.0 + (to.0 - from.0) * glide, from.1 + (to.1 - from.1) * glide);
            let vibrato = 1.0 + 0.012 * (TAU * 5.5 * (at + frame) as f64 / rate).sin();
            let f0 = line.speaker_hz * (pitch + (next_pitch - pitch) * t * t) * vibrato;
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
