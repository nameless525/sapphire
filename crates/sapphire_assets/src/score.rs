//! Scores (`.score.ron` documents, cooked to `.sscore`; M3 A1, spec §5.15): music as stems that loop
//! together, and which of them play in each phase of a fight.
//!
//! ```ron
//! (
//!     type: "sapphire::assets::Score",
//!     version: 1,
//!     data: (
//!         tempo_bpm: 120.0,
//!         beats_per_bar: 4,
//!         // Every stem is exactly this many bars long, and they loop together.
//!         bars: 4,
//!         stems: [
//!             (name: "pad", sound: "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c"),
//!             (name: "drums", sound: "0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"),
//!         ],
//!         // By the names a game gives its phases: an encounter's are `waiting`, `running` and `cleared`.
//!         phases: [
//!             (name: "waiting", play: ["pad"]),
//!             (name: "running", play: ["pad", "drums"]),
//!         ],
//!         fade_beats: 2.0,
//!         volume_db: -6.0,
//!         duck_db: -10.0,
//!     ),
//! )
//! ```
//!
//! A phase change lands on the next bar, where the stems it starts fade in and those it ends fade out,
//! over `fade_beats`. That, and holding stems to their bars, is `sapphire_audio`'s; the cook checks that
//! each stem is a sound of whole bars. The cooked form is the same type, so a score loads as an asset
//! whose dependencies are its stems.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_reflect::{TypePath, Versioned};
use sapphire_serialize::DocumentError;
use sapphire_serialize::cooked::{AlignedVec, CookedError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::AssetId;
use crate::cue::MAX_VOLUME_DB;

/// Conventional file extension for cooked scores.
pub const SCORE_EXTENSION: &str = "sscore";

/// The most stems a score may have: a mixer plays every one all the time, most of them silent.
pub const MAX_STEMS: usize = 8;

/// The most phases a score may name.
pub const MAX_PHASES: usize = 16;

/// The tempos a score may have, in beats a minute.
const TEMPO_RANGE: std::ops::RangeInclusive<f32> = 30.0..=300.0;

/// The most beats in a bar.
const MAX_BEATS_PER_BAR: u32 = 16;

/// The most bars a loop may be: a stem is decoded into memory whole.
const MAX_BARS: u32 = 64;

/// A score. See the [module docs](self).
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct Score {
    /// Beats a minute.
    pub tempo_bpm: f32,
    /// Beats in a bar: where a phase change may land.
    pub beats_per_bar: u32,
    /// The loop's length in bars, which every stem is.
    pub bars: u32,
    /// What plays together: at least one.
    pub stems: Vec<Stem>,
    /// Which stems play in each phase: at least one.
    pub phases: Vec<Phase>,
    /// How long a stem takes to come in or go, from the bar a change lands on, in beats.
    #[serde(default = "one_beat")]
    pub fade_beats: f32,
    /// The whole score's gain, in decibels.
    #[serde(default)]
    pub volume_db: f32,
    /// How far it goes down while a voice line plays, in decibels: 0 or below.
    #[serde(default = "voice_duck")]
    pub duck_db: f32,
}

fn one_beat() -> f32 {
    1.0
}

fn voice_duck() -> f32 {
    -10.0
}

impl Versioned for Score {
    const VERSION: u32 = 1;
}

/// One of a score's stems.
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct Stem {
    /// What phases call it.
    pub name: String,
    /// The sound, by asset ID: the loop's length exactly.
    pub sound: AssetId,
}

/// One of a score's phases.
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct Phase {
    /// What a game calls it.
    pub name: String,
    /// The stems that play in it, by name. The rest are silent.
    pub play: Vec<String>,
}

/// What's wrong with a score or a voice-line document, said as a cue's errors say it.
#[derive(Debug, thiserror::Error)]
pub enum DocumentProblem {
    /// The document isn't one of these at all: the wrong type or a version out of reach.
    #[error(transparent)]
    Document(Box<DocumentError>),
    /// Malformed RON, or a value of the wrong kind. `text` is the document's own line, which names
    /// the field.
    #[error("line {line}, column {column}: {message}: `{text}`")]
    Malformed { line: usize, column: usize, message: String, text: String },
    /// A value that parsed but can't be played.
    #[error("`{field}`: {problem}")]
    Field { field: &'static str, problem: String },
}

/// Reads a document, saying where it's malformed by its own line.
pub(crate) fn read_document<T: Versioned + DeserializeOwned + TypePath>(
    text: &str,
) -> Result<T, DocumentProblem> {
    sapphire_serialize::from_ron::<T>(text).map_err(|error| match error {
        DocumentError::Parse(spanned) => {
            let line = spanned.span.start.line;
            let quoted = text.lines().nth(line.saturating_sub(1)).unwrap_or_default().trim();
            DocumentProblem::Malformed {
                line,
                column: spanned.span.start.col,
                message: spanned.code.to_string(),
                text: quoted.to_owned(),
            }
        }
        other => DocumentProblem::Document(Box::new(other)),
    })
}

/// A score that can't be read.
#[derive(Debug, thiserror::Error)]
pub enum ScoreError {
    #[error(transparent)]
    Document(#[from] DocumentProblem),
    #[error(transparent)]
    Cooked(#[from] CookedError),
}

impl Score {
    /// Reads and checks a score document.
    pub fn from_document(text: &str) -> Result<Self, ScoreError> {
        let score: Self = read_document(text)?;
        score.check()?;
        Ok(score)
    }

    /// Checks that every value can be played, naming the first that can't.
    pub fn check(&self) -> Result<(), DocumentProblem> {
        let field = |field, problem: String| Err(DocumentProblem::Field { field, problem });
        if !(self.tempo_bpm.is_finite() && TEMPO_RANGE.contains(&self.tempo_bpm)) {
            let (least, most) = (TEMPO_RANGE.start(), TEMPO_RANGE.end());
            return field("tempo_bpm", format!("{} isn't from {least} to {most}", self.tempo_bpm));
        }
        if !(1..=MAX_BEATS_PER_BAR).contains(&self.beats_per_bar) {
            let beats = self.beats_per_bar;
            return field("beats_per_bar", format!("{beats} isn't from 1 to {MAX_BEATS_PER_BAR}"));
        }
        if !(1..=MAX_BARS).contains(&self.bars) {
            return field("bars", format!("{} isn't from 1 to {MAX_BARS}", self.bars));
        }
        if self.stems.is_empty() || self.stems.len() > MAX_STEMS {
            return field("stems", format!("{} stems isn't from 1 to {MAX_STEMS}", self.stems.len()));
        }
        for (index, stem) in self.stems.iter().enumerate() {
            if stem.name.is_empty() {
                return field("stems", format!("stems[{index}] has no name"));
            }
            if self.stems[..index].iter().any(|earlier| earlier.name == stem.name) {
                return field("stems", format!("`{}` is named twice", stem.name));
            }
        }
        if self.phases.is_empty() || self.phases.len() > MAX_PHASES {
            return field("phases", format!("{} phases isn't from 1 to {MAX_PHASES}", self.phases.len()));
        }
        for (index, phase) in self.phases.iter().enumerate() {
            if phase.name.is_empty() {
                return field("phases", format!("phases[{index}] has no name"));
            }
            if self.phases[..index].iter().any(|earlier| earlier.name == phase.name) {
                return field("phases", format!("`{}` is named twice", phase.name));
            }
            if let Some(unknown) = phase.play.iter().find(|name| self.stem(name).is_none()) {
                return field("phases", format!("`{}` plays `{unknown}`, which isn't a stem", phase.name));
            }
        }
        let loop_beats = (self.beats_per_bar * self.bars) as f32;
        if !(self.fade_beats.is_finite() && (0.0..=loop_beats).contains(&self.fade_beats)) {
            let fade = self.fade_beats;
            return field("fade_beats", format!("{fade} isn't from 0 to the loop's {loop_beats}"));
        }
        if !(self.volume_db.is_finite() && self.volume_db <= MAX_VOLUME_DB) {
            return field("volume_db", format!("{} isn't up to {MAX_VOLUME_DB}", self.volume_db));
        }
        if !(self.duck_db.is_finite() && self.duck_db <= 0.0) {
            return field("duck_db", format!("{} isn't quieter", self.duck_db));
        }
        Ok(())
    }

    /// The stem called `name`, by its place in [`Self::stems`].
    pub fn stem(&self, name: &str) -> Option<usize> {
        self.stems.iter().position(|stem| stem.name == name)
    }

    /// The phase called `name`, by its place in [`Self::phases`].
    pub fn phase(&self, name: &str) -> Option<usize> {
        self.phases.iter().position(|phase| phase.name == name)
    }

    /// The loop's length in seconds.
    pub fn loop_seconds(&self) -> f64 {
        f64::from(self.beats_per_bar * self.bars) * 60.0 / f64::from(self.tempo_bpm)
    }

    /// How many frames a stem at `sample_rate` must be: the loop's length, if that's a whole number of
    /// frames to within a hundredth of one. Otherwise no stem at that rate loops in step with the bars.
    pub fn stem_frames(&self, sample_rate: u32) -> Option<u64> {
        let frames = self.loop_seconds() * f64::from(sample_rate);
        let whole = frames.round();
        ((frames - whole).abs() < 0.01).then_some(whole as u64)
    }

    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedScore, ScoreError> {
        let score = sapphire_serialize::access::<Self>(bytes)?;
        score.to_owned_score().check()?;
        Ok(score)
    }
}

impl ArchivedScore {
    /// The score as plain data.
    pub fn to_owned_score(&self) -> Score {
        Score {
            tempo_bpm: self.tempo_bpm.to_native(),
            beats_per_bar: self.beats_per_bar.to_native(),
            bars: self.bars.to_native(),
            stems: self
                .stems
                .iter()
                .map(|stem| Stem { name: stem.name.as_str().to_owned(), sound: stem.sound })
                .collect(),
            phases: self
                .phases
                .iter()
                .map(|phase| Phase {
                    name: phase.name.as_str().to_owned(),
                    play: phase.play.iter().map(|name| name.as_str().to_owned()).collect(),
                })
                .collect(),
            fade_beats: self.fade_beats.to_native(),
            volume_db: self.volume_db.to_native(),
            duck_db: self.duck_db.to_native(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAD: &str = "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c";
    const DRUMS: &str = "0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b";

    /// A good score document, with `field`'s line replaced by `line`.
    fn document(field: &str, line: &str) -> String {
        let stems = format!(r#"stems: [(name: "pad", sound: "{PAD}"), (name: "drums", sound: "{DRUMS}")],"#);
        let phases =
            r#"phases: [(name: "waiting", play: ["pad"]), (name: "running", play: ["pad", "drums"])],"#;
        let lines = [
            ("tempo_bpm", "tempo_bpm: 120.0,"),
            ("beats_per_bar", "beats_per_bar: 4,"),
            ("bars", "bars: 4,"),
            ("stems", stems.as_str()),
            ("phases", phases),
            ("fade_beats", ""),
            ("volume_db", ""),
            ("duck_db", ""),
        ];
        let data: String = lines
            .iter()
            .map(|&(name, default)| if name == field { line } else { default })
            .map(|line| format!("        {line}\n"))
            .collect();
        format!("(\n    type: \"sapphire::assets::Score\",\n    version: 1,\n    data: (\n{data}    ),\n)\n")
    }

    fn error_of(field: &str, line: &str) -> String {
        Score::from_document(&document(field, line)).expect_err(line).to_string()
    }

    fn good() -> Score {
        Score::from_document(&document("", "")).unwrap()
    }

    #[test]
    fn a_score_reads_with_its_defaults_and_cooks_back() {
        let score = good();
        assert_eq!((score.fade_beats, score.volume_db, score.duck_db), (1.0, 0.0, -10.0));
        assert_eq!(score.phase("running"), Some(1));
        assert_eq!(score.stem("drums"), Some(1));
        let cooked = score.to_cooked().unwrap();
        assert_eq!(Score::read(&cooked).unwrap().to_owned_score(), score);
    }

    #[test]
    fn a_loop_of_whole_frames_says_how_long_a_stem_is() {
        let score = good();
        assert_eq!(score.loop_seconds(), 8.0);
        assert_eq!(score.stem_frames(32_000), Some(256_000));
        let odd = Score { tempo_bpm: 97.0, ..score };
        assert_eq!(odd.stem_frames(44_100), None, "16 beats at 97 isn't a whole number of frames");
    }

    #[test]
    fn a_bad_value_names_its_field() {
        for (field, line) in [
            ("tempo_bpm", "tempo_bpm: 1000.0,"),
            ("beats_per_bar", "beats_per_bar: 0,"),
            ("bars", "bars: 65,"),
            ("fade_beats", "fade_beats: -1.0,"),
            ("volume_db", "volume_db: 40.0,"),
            ("duck_db", "duck_db: 3.0,"),
            ("stems", "stems: [],"),
            ("phases", "phases: [],"),
        ] {
            let message = error_of(field, line);
            assert!(message.contains(&format!("`{field}`")), "{line}: {message}");
        }
    }

    #[test]
    fn a_phase_plays_only_stems_the_score_has() {
        let message = error_of("phases", r#"phases: [(name: "running", play: ["bass"])],"#);
        assert!(message.contains("`running` plays `bass`, which isn't a stem"), "{message}");
    }

    #[test]
    fn a_name_twice_is_refused() {
        let stems = format!(r#"stems: [(name: "pad", sound: "{PAD}"), (name: "pad", sound: "{DRUMS}")],"#);
        assert!(error_of("stems", &stems).contains("`pad` is named twice"));
        let phases = r#"phases: [(name: "waiting", play: []), (name: "waiting", play: [])],"#;
        assert!(error_of("phases", phases).contains("`waiting` is named twice"));
    }

    #[test]
    fn a_malformed_document_quotes_its_line() {
        let message = error_of("bars", r#"bars: "four","#);
        assert!(message.contains(r#"`bars: "four",`"#), "{message}");
    }
}
