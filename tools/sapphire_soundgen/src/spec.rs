//! A sound's spec, `<name>.sound.ron`: what it plays, read strictly. The crate's docs show one.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::SAMPLE_RATE;

/// A spec's file name ends with this.
pub const SPEC_EXTENSION: &str = ".sound.ron";

/// A stem's tempo, beats a bar and bars are held to what a score can play (`sapphire_assets::score`),
/// since a stem is only ever one of a score's.
const TEMPO_RANGE: std::ops::RangeInclusive<f64> = 30.0..=300.0;
const MAX_BEATS_PER_BAR: u32 = 16;
const MAX_BARS: u32 = 64;
/// The most a beat is split into by a pulse or the hats.
const MAX_PER_BEAT: u32 = 8;
/// A speaker's pitch, in hertz: from a low man's to a child's, with room either side.
const SPEAKER_RANGE: std::ops::RangeInclusive<f64> = 60.0..=600.0;
/// The longest syllable, in seconds, and the most a syllable's pitch moves from the speaker's.
const MAX_SYLLABLE_SECONDS: f64 = 2.0;
const PITCH_RANGE: std::ops::RangeInclusive<f64> = 0.25..=4.0;
/// The most syllables in a line: a placeholder bark, not a speech.
const MAX_SYLLABLES: usize = 64;

/// One placeholder sound, written to `path`: see the [crate docs](crate).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::soundgen"]
#[serde(deny_unknown_fields)]
pub struct Sound {
    /// Where it's written, relative to the content directory, with `/` between parts: an `.ogg`.
    pub path: String,
    /// What it is.
    pub sound: Synth,
}

impl Versioned for Sound {
    /// 1: M4 R4.
    const VERSION: u32 = 1;
}

/// What a sound is made as.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Synth {
    /// A music stem: one part, a loop of whole bars that a score plays with its other stems.
    Stem(Stem),
    /// A voice line: syllables with no words.
    Line(Line),
}

/// A loop of `bars` bars of `beats_per_bar` beats at `tempo_bpm`, which must be its score's. What rings
/// past its end comes round at its start, so it has no seam.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stem {
    pub tempo_bpm: f64,
    pub beats_per_bar: u32,
    pub bars: u32,
    /// What plays.
    pub part: Part,
}

/// A stem's instrument and what it plays. Notes are MIDI's (60 is middle C, 69 is A at 440 Hz). How each
/// instrument sounds is the generator's; what it plays is the spec's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Part {
    /// Held chords of soft detuned harmonics, one a bar, each swelling in and ringing into the next.
    Pad(Pad),
    /// A plucked bass note on each `per_beat`th of a beat, the off-beats softer: one note a bar.
    Pulse(Pulse),
    /// Kick, snare and hats on beats of each bar, their noise from `seed`.
    Drums(Drums),
    /// A bell struck by frequency modulation on each beat that has a note.
    Bells(Bells),
    /// A great bell tolled on each bar's top, ringing on through the next bar.
    Toll(Toll),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    /// Each bar's chord, its notes.
    pub chords: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pulse {
    /// Each bar's note.
    pub notes: Vec<u8>,
    /// How many times a beat it's plucked: 2 for eighth notes.
    pub per_beat: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drums {
    /// The kick's beats in every bar, counted from 0.
    pub kick: Vec<f64>,
    /// More kicks in the last bar, leading back into the loop's top.
    #[serde(default)]
    pub fill: Vec<f64>,
    /// The snare's beats in every bar.
    pub snare: Vec<f64>,
    /// How many hats a beat, evenly spaced from it: 0 for none.
    pub hats_per_beat: u32,
    /// The noise's seed, not 0. Two stems with one seed hiss alike.
    pub seed: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bells {
    /// A note or a rest (`None`) for every beat of the loop, in order.
    pub tune: Vec<Option<u8>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Toll {
    /// Each bar's strike note: the bell's hum sounds an octave below it.
    pub notes: Vec<u8>,
}

/// A line in a voice's shape, with no words: a pulse train at the speaker's pitch, shaped by two
/// formants gliding from vowel to vowel, a syllable at a time, with a tenth of a second's quiet either
/// side. Its length is its syllables'.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Line {
    /// The speaker's pitch, in hertz.
    pub speaker_hz: f64,
    pub syllables: Vec<Syllable>,
    /// The consonants' noise's seed, not 0.
    pub seed: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Syllable {
    pub vowel: Vowel,
    /// How long its vowel is held, in seconds.
    pub seconds: f64,
    /// Its pitch, as a multiple of the speaker's. The pitch glides to the next syllable's.
    pub pitch: f64,
    /// Whether a consonant's burst of noise leads it in.
    pub burst: bool,
}

/// A vowel, by the first two formants the generator gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Vowel {
    A,
    E,
    I,
    O,
    U,
}

/// Why a spec can't be read.
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("{path}: {source}")]
    Document { path: PathBuf, source: Box<DocumentError> },
    #[error("{path}: `{field}`: {problem}")]
    Field { path: PathBuf, field: String, problem: String },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

/// Reads and checks a spec.
pub fn read(path: &Path) -> Result<Sound, SpecError> {
    let text = fs::read_to_string(path).map_err(|source| SpecError::Io { path: path.to_owned(), source })?;
    let sound: Sound = sapphire_serialize::from_ron(&text)
        .map_err(|error| SpecError::Document { path: path.to_owned(), source: Box::new(error) })?;
    sound.check().map_err(|(field, problem)| SpecError::Field { path: path.to_owned(), field, problem })?;
    Ok(sound)
}

/// Every spec in `dir`, by file name.
pub fn specs(dir: &Path) -> Result<Vec<PathBuf>, SpecError> {
    let io = |source| SpecError::Io { path: dir.to_owned(), source };
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(io)? {
        let path = entry.map_err(io)?.path();
        if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(SPEC_EXTENSION))
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// A field that can't be played, by its path in the spec, and why.
type Problem = (String, String);

fn problem(field: impl Into<String>, why: impl Into<String>) -> Result<(), Problem> {
    Err((field.into(), why.into()))
}

impl Sound {
    /// Checks every value can be made, naming the first that can't.
    pub(crate) fn check(&self) -> Result<(), Problem> {
        let parts: Vec<&str> = self.path.split('/').collect();
        if !self.path.ends_with(".ogg")
            || parts
                .iter()
                .any(|part| part.is_empty() || *part == "." || *part == ".." || part.contains(['\\', ':']))
        {
            return problem(
                "path",
                format!("{:?} isn't an `.ogg` under the content directory, `/` between its parts", self.path),
            );
        }
        match &self.sound {
            Synth::Stem(stem) => stem.check(),
            Synth::Line(line) => line.check(),
        }
    }
}

impl Stem {
    /// Frames a beat: whole, or the loop drifts from its bars.
    pub(crate) fn frames_per_beat(&self) -> f64 {
        f64::from(SAMPLE_RATE) * 60.0 / self.tempo_bpm
    }

    fn check(&self) -> Result<(), Problem> {
        if !(self.tempo_bpm.is_finite() && TEMPO_RANGE.contains(&self.tempo_bpm)) {
            let (least, most) = (TEMPO_RANGE.start(), TEMPO_RANGE.end());
            return problem("sound.tempo_bpm", format!("{} isn't from {least} to {most}", self.tempo_bpm));
        }
        if self.frames_per_beat().fract() != 0.0 {
            return problem(
                "sound.tempo_bpm",
                format!(
                    "a beat at {} is {} frames at {SAMPLE_RATE} Hz, not a whole number, so the loop wouldn't keep to its bars",
                    self.tempo_bpm,
                    self.frames_per_beat()
                ),
            );
        }
        if !(1..=MAX_BEATS_PER_BAR).contains(&self.beats_per_bar) {
            return problem(
                "sound.beats_per_bar",
                format!("{} isn't from 1 to {MAX_BEATS_PER_BAR}", self.beats_per_bar),
            );
        }
        if !(1..=MAX_BARS).contains(&self.bars) {
            return problem("sound.bars", format!("{} isn't from 1 to {MAX_BARS}", self.bars));
        }
        let bars = self.bars as usize;
        let per_bar = |field: &str, count: usize| {
            if count == bars {
                Ok(())
            } else {
                problem(
                    format!("sound.part.{field}"),
                    format!("{count}, not one for each of the {bars} bars"),
                )
            }
        };
        match &self.part {
            Part::Pad(pad) => {
                per_bar("chords", pad.chords.len())?;
                for (bar, chord) in pad.chords.iter().enumerate() {
                    if chord.is_empty() {
                        return problem(format!("sound.part.chords[{bar}]"), "a chord with no notes");
                    }
                    notes(&format!("sound.part.chords[{bar}]"), chord)?;
                }
            }
            Part::Pulse(pulse) => {
                per_bar("notes", pulse.notes.len())?;
                notes("sound.part.notes", &pulse.notes)?;
                if !(1..=MAX_PER_BEAT).contains(&pulse.per_beat) {
                    return problem(
                        "sound.part.per_beat",
                        format!("{} isn't from 1 to {MAX_PER_BEAT}", pulse.per_beat),
                    );
                }
            }
            Part::Drums(drums) => {
                for (field, beats) in [("kick", &drums.kick), ("fill", &drums.fill), ("snare", &drums.snare)]
                {
                    for (index, &beat) in beats.iter().enumerate() {
                        if !(beat.is_finite() && (0.0..f64::from(self.beats_per_bar)).contains(&beat)) {
                            return problem(
                                format!("sound.part.{field}[{index}]"),
                                format!("beat {beat} isn't in a bar of {}", self.beats_per_bar),
                            );
                        }
                    }
                }
                if drums.hats_per_beat > MAX_PER_BEAT {
                    return problem(
                        "sound.part.hats_per_beat",
                        format!("{} is more than {MAX_PER_BEAT}", drums.hats_per_beat),
                    );
                }
                if drums.seed == 0 {
                    return problem("sound.part.seed", "0 makes no noise");
                }
            }
            Part::Bells(bells) => {
                let beats = (self.beats_per_bar * self.bars) as usize;
                if bells.tune.len() != beats {
                    return problem(
                        "sound.part.tune",
                        format!(
                            "{} notes and rests, not one for each of the loop's {beats} beats",
                            bells.tune.len()
                        ),
                    );
                }
                let played: Vec<u8> = bells.tune.iter().flatten().copied().collect();
                if played.is_empty() {
                    return problem("sound.part.tune", "every beat is a rest");
                }
                for (beat, note) in bells.tune.iter().enumerate() {
                    if let Some(note) = note {
                        notes(&format!("sound.part.tune[{beat}]"), &[*note])?;
                    }
                }
            }
            Part::Toll(toll) => {
                per_bar("notes", toll.notes.len())?;
                notes("sound.part.notes", &toll.notes)?;
            }
        }
        Ok(())
    }
}

/// MIDI's notes go to 127.
fn notes(field: &str, notes: &[u8]) -> Result<(), Problem> {
    match notes.iter().find(|&&note| note > 127) {
        Some(note) => problem(field, format!("note {note} is past MIDI's 127")),
        None => Ok(()),
    }
}

impl Line {
    fn check(&self) -> Result<(), Problem> {
        if !(self.speaker_hz.is_finite() && SPEAKER_RANGE.contains(&self.speaker_hz)) {
            let (least, most) = (SPEAKER_RANGE.start(), SPEAKER_RANGE.end());
            return problem("sound.speaker_hz", format!("{} isn't from {least} to {most}", self.speaker_hz));
        }
        if self.syllables.is_empty() || self.syllables.len() > MAX_SYLLABLES {
            return problem(
                "sound.syllables",
                format!("{} syllables isn't from 1 to {MAX_SYLLABLES}", self.syllables.len()),
            );
        }
        for (index, syllable) in self.syllables.iter().enumerate() {
            // At least two frames: the vowel's glide and envelope divide by its length.
            let least = 2.0 / f64::from(SAMPLE_RATE);
            if !(syllable.seconds.is_finite() && (least..=MAX_SYLLABLE_SECONDS).contains(&syllable.seconds)) {
                return problem(
                    format!("sound.syllables[{index}].seconds"),
                    format!("{} isn't from {least} to {MAX_SYLLABLE_SECONDS}", syllable.seconds),
                );
            }
            if !(syllable.pitch.is_finite() && PITCH_RANGE.contains(&syllable.pitch)) {
                let (least, most) = (PITCH_RANGE.start(), PITCH_RANGE.end());
                return problem(
                    format!("sound.syllables[{index}].pitch"),
                    format!("{} isn't from {least} to {most}", syllable.pitch),
                );
            }
        }
        if self.seed == 0 {
            return problem("sound.seed", "0 makes no noise");
        }
        Ok(())
    }
}
