//! Cues (`.cue.ron` documents, cooked to `.scue`; M2 E1, spec §5.15): what a designer writes to say
//! how a sound plays. Gameplay asks for a cue, never a sound.
//!
//! ```ron
//! (
//!     type: "sapphire::assets::Cue",
//!     version: 1,
//!     data: (
//!         // Footsteps on stone: one of these, picked each time.
//!         sounds: [
//!             "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c",
//!             "0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b",
//!         ],
//!         volume_db: (-6.0, -3.0),
//!         pitch: (0.95, 1.05),
//!         attenuation: Some((near: 1.0, far: 30.0, curve: Inverse)),
//!         bus: Sfx,
//!         hitstop: Duck(-12.0),
//!     ),
//! )
//! ```
//!
//! Only `sounds` is required. [`Cue::from_document`] reads a document and says what's wrong with it
//! by field, or by the document's own line; the cook refuses a cue whose `sounds` aren't sounds of
//! the content tree. The cooked form is the same type, so a cue loads as an asset whose dependencies
//! are its sounds.
//!
//! A cue names what is authored. Picking among its sounds and ranges, placing the voice and mixing
//! it are `sapphire_audio`'s.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use sapphire_serialize::cooked::{AlignedVec, CookedError};
use serde::{Deserialize, Serialize};

use crate::AssetId;

/// Conventional file extension for cooked cues.
pub const CUE_EXTENSION: &str = "scue";

/// The quietest gain that still counts as sound, in decibels: where a far attenuation ends.
pub const SILENCE_DB: f32 = -60.0;

/// The loudest a cue may ask for, in decibels.
const MAX_VOLUME_DB: f32 = 24.0;

/// The playback rates a cue may ask for: a tenth of the speed to eight times it.
const PITCH_RANGE: std::ops::RangeInclusive<f32> = 0.1..=8.0;

/// A cue. See the [module docs](self).
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
pub struct Cue {
    /// The sounds to pick from, by asset ID: at least one.
    pub sounds: Vec<AssetId>,
    /// The volume to pick from, least to most, in decibels: 0 is the sound as authored.
    #[serde(default)]
    pub volume_db: [f32; 2],
    /// The playback rate to pick from, least to most: 1 is the sound as authored. It moves pitch
    /// and speed together.
    #[serde(default = "authored_pitch")]
    pub pitch: [f32; 2],
    /// How the cue fades with distance from the listener, and whether it is placed in the world at
    /// all: `None` plays flat, at its volume and centered, wherever it was asked for (interface
    /// sounds).
    #[serde(default)]
    pub attenuation: Option<Attenuation>,
    /// The mixer bus it plays on.
    #[serde(default)]
    pub bus: Bus,
    /// What it does while the actor it is attached to is in hitstop.
    #[serde(default)]
    pub hitstop: HitstopResponse,
}

fn authored_pitch() -> [f32; 2] {
    [1.0, 1.0]
}

impl Versioned for Cue {
    const VERSION: u32 = 1;
}

/// How a placed cue fades with distance.
#[derive(
    Reflect,
    Clone,
    Copy,
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
pub struct Attenuation {
    /// Meters out to which the cue plays at full volume.
    pub near: f32,
    /// Meters at and beyond which it is silent.
    pub far: f32,
    /// The fade between them.
    #[serde(default)]
    pub curve: AttenuationCurve,
}

impl Attenuation {
    /// The gain at `distance` meters, in decibels: 0 out to [`Self::near`], [`SILENCE_DB`] from
    /// [`Self::far`] on, falling all the way between.
    pub fn gain_db(&self, distance: f32) -> f32 {
        if distance <= self.near {
            return 0.0;
        }
        if distance >= self.far {
            return SILENCE_DB;
        }
        let along = (distance - self.near) / (self.far - self.near);
        match self.curve {
            AttenuationCurve::Linear => SILENCE_DB * along,
            // The inverse distance law, then down to silence over the range's last quarter, so a
            // cue with a far `far` doesn't cut out while still audible.
            AttenuationCurve::Inverse => {
                // Held to silence: a far `far` would otherwise climb back up it, and a `near` of 0, which
                // reads as infinitely quiet, would come out of the sum below as NaN.
                let law = (20.0 * (self.near / distance).log10()).max(SILENCE_DB);
                let fade = ((along - 0.75) / 0.25).clamp(0.0, 1.0);
                law + (SILENCE_DB - law) * fade
            }
        }
    }
}

/// See [`Attenuation::gain_db`].
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
pub enum AttenuationCurve {
    /// Evenly in decibels from full to silent: a designer's fade.
    Linear,
    /// By the inverse distance law, as sound falls off in the open: most of the fall is close by.
    #[default]
    Inverse,
}

/// A mixer bus. The world's buses follow the world clock's snapshot; interface sounds don't.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
pub enum Bus {
    /// Combat and everything else that happens in the world.
    #[default]
    Sfx,
    /// The world's beds and details.
    Ambience,
    /// The interface: heard the same whatever the world's clock does.
    Ui,
}

impl Bus {
    /// Every bus, in a stable order. New buses (music and voice, M3) join at the end.
    pub const ALL: [Self; 3] = [Self::Sfx, Self::Ambience, Self::Ui];

    /// The bus's position in [`Self::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Whether the bus takes the world clock's snapshot (spec §5.15).
    pub const fn follows_world_clock(self) -> bool {
        matches!(self, Self::Sfx | Self::Ambience)
    }
}

/// What a voice does while the actor it is attached to is in hitstop.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
pub enum HitstopResponse {
    /// Plays on: the hit itself.
    #[default]
    Ignore,
    /// Plays on this many decibels quieter (not above 0): what shouldn't mask the hit.
    Duck(f32),
    /// Stops where it is and goes on when the hitstop ends: a swing that froze with the arm.
    Freeze,
}

/// A cue that can't be read.
#[derive(Debug, thiserror::Error)]
pub enum CueError {
    /// The document isn't a cue document at all: the wrong type or a version out of reach.
    #[error(transparent)]
    Document(Box<DocumentError>),
    /// Malformed RON, or a value of the wrong kind. `text` is the document's own line, which names
    /// the field.
    #[error("line {line}, column {column}: {message}: `{text}`")]
    Malformed { line: usize, column: usize, message: String, text: String },
    /// A value that parsed but can't be played.
    #[error("`{field}`: {problem}")]
    Field { field: &'static str, problem: String },
    #[error(transparent)]
    Cooked(#[from] CookedError),
}

impl Cue {
    /// Reads and checks a cue document.
    pub fn from_document(text: &str) -> Result<Self, CueError> {
        let cue = sapphire_serialize::from_ron::<Self>(text).map_err(|error| match error {
            DocumentError::Parse(spanned) => {
                let line = spanned.span.start.line;
                let quoted = text.lines().nth(line.saturating_sub(1)).unwrap_or_default().trim();
                CueError::Malformed {
                    line,
                    column: spanned.span.start.col,
                    message: spanned.code.to_string(),
                    text: quoted.to_owned(),
                }
            }
            other => CueError::Document(Box::new(other)),
        })?;
        cue.check()?;
        Ok(cue)
    }

    /// Checks that every value can be played, naming the first that can't.
    pub fn check(&self) -> Result<(), CueError> {
        let field = |field, problem: String| Err(CueError::Field { field, problem });
        if self.sounds.is_empty() {
            return field("sounds", "a cue names at least one sound".to_owned());
        }
        let [quiet, loud] = self.volume_db;
        if !(quiet.is_finite() && loud.is_finite()) || quiet > loud {
            return field("volume_db", format!("({quiet}, {loud}) isn't a range, least to most"));
        }
        if loud > MAX_VOLUME_DB {
            return field(
                "volume_db",
                format!("{loud} dB is above the {MAX_VOLUME_DB} dB a cue may ask for"),
            );
        }
        let [slow, fast] = self.pitch;
        if !(slow.is_finite() && fast.is_finite()) || slow > fast {
            return field("pitch", format!("({slow}, {fast}) isn't a range, least to most"));
        }
        if !(PITCH_RANGE.contains(&slow) && PITCH_RANGE.contains(&fast)) {
            return field(
                "pitch",
                format!("({slow}, {fast}) leaves {} to {}", PITCH_RANGE.start(), PITCH_RANGE.end()),
            );
        }
        if let Some(Attenuation { near, far, curve }) = self.attenuation {
            if !near.is_finite() || near < 0.0 {
                return field("attenuation.near", format!("{near} isn't a distance"));
            }
            if !far.is_finite() || far <= near {
                return field("attenuation.far", format!("{far} isn't beyond `near`, {near}"));
            }
            if near == 0.0 && curve == AttenuationCurve::Inverse {
                return field(
                    "attenuation.near",
                    "the inverse law falls from `near`, so 0 would leave nothing to hear".to_owned(),
                );
            }
        }
        if let HitstopResponse::Duck(db) = self.hitstop
            && !(db.is_finite() && db <= 0.0)
        {
            return field("hitstop", format!("Duck({db}) isn't quieter"));
        }
        Ok(())
    }

    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedCue, CueError> {
        let cue = sapphire_serialize::access::<Self>(bytes)?;
        cue.to_owned_cue().check()?;
        Ok(cue)
    }
}

impl ArchivedCue {
    /// The cue as plain data.
    pub fn to_owned_cue(&self) -> Cue {
        Cue {
            sounds: self.sounds.iter().copied().collect(),
            volume_db: self.volume_db.map(|value| value.to_native()),
            pitch: self.pitch.map(|value| value.to_native()),
            attenuation: self.attenuation.as_ref().map(|attenuation| Attenuation {
                near: attenuation.near.to_native(),
                far: attenuation.far.to_native(),
                curve: match attenuation.curve {
                    ArchivedAttenuationCurve::Linear => AttenuationCurve::Linear,
                    ArchivedAttenuationCurve::Inverse => AttenuationCurve::Inverse,
                },
            }),
            bus: match self.bus {
                ArchivedBus::Sfx => Bus::Sfx,
                ArchivedBus::Ambience => Bus::Ambience,
                ArchivedBus::Ui => Bus::Ui,
            },
            hitstop: match self.hitstop {
                ArchivedHitstopResponse::Ignore => HitstopResponse::Ignore,
                ArchivedHitstopResponse::Duck(db) => HitstopResponse::Duck(db.to_native()),
                ArchivedHitstopResponse::Freeze => HitstopResponse::Freeze,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOUND: &str = "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c";

    /// A cue document with these lines in its data.
    fn document(data: &str) -> String {
        format!("(\n    type: \"sapphire::assets::Cue\",\n    version: 1,\n    data: (\n{data}\n    ),\n)\n")
    }

    fn sounds() -> String {
        format!("        sounds: [\"{SOUND}\"],")
    }

    /// What reading a document with these lines after `sounds` says.
    fn error_of(lines: &str) -> String {
        let text = document(&format!("{}\n{lines}", sounds()));
        Cue::from_document(&text).expect_err("a malformed cue").to_string()
    }

    #[test]
    fn a_whole_document_reads_as_written() {
        let text = document(&format!(
            "{}
        volume_db: (-6.0, -3.0),
        pitch: (0.95, 1.05),
        attenuation: Some((near: 1.0, far: 30.0, curve: Linear)),
        bus: Ambience,
        hitstop: Duck(-12.0),",
            sounds()
        ));
        let cue = Cue::from_document(&text).unwrap();
        assert_eq!(
            cue,
            Cue {
                sounds: vec![SOUND.parse().unwrap()],
                volume_db: [-6.0, -3.0],
                pitch: [0.95, 1.05],
                attenuation: Some(Attenuation { near: 1.0, far: 30.0, curve: AttenuationCurve::Linear }),
                bus: Bus::Ambience,
                hitstop: HitstopResponse::Duck(-12.0),
            }
        );
    }

    #[test]
    fn only_the_sounds_are_required() {
        let cue = Cue::from_document(&document(&sounds())).unwrap();
        assert_eq!(cue.volume_db, [0.0, 0.0]);
        assert_eq!(cue.pitch, [1.0, 1.0]);
        assert_eq!(cue.attenuation, None);
        assert_eq!((cue.bus, cue.hitstop), (Bus::Sfx, HitstopResponse::Ignore));
    }

    /// Whatever the document says, a gain is a number, and it only ever falls with distance.
    #[test]
    fn the_gain_falls_from_near_to_far_and_stays_a_number() {
        let curves = [AttenuationCurve::Linear, AttenuationCurve::Inverse];
        for (near, far) in [(0.0, 30.0), (0.001, 30.0), (1.0, 30.0), (0.1, 10_000.0), (4.0, 4.5)] {
            for curve in curves {
                let attenuation = Attenuation { near, far, curve };
                let mut last = attenuation.gain_db(0.0);
                assert_eq!(last, 0.0, "{attenuation:?}");
                for step in 1..=400 {
                    let distance = far * step as f32 / 300.0;
                    let gain = attenuation.gain_db(distance);
                    assert!(gain.is_finite(), "{attenuation:?} at {distance} m: {gain}");
                    assert!(gain <= last + 1e-3, "{attenuation:?} at {distance} m: {last}, then {gain}");
                    assert!(gain >= SILENCE_DB, "{attenuation:?} at {distance} m: {gain}");
                    last = gain;
                }
                assert_eq!(last, SILENCE_DB, "{attenuation:?}");
            }
        }
    }

    #[test]
    fn an_inverse_fall_needs_somewhere_to_fall_from() {
        let error = error_of("        attenuation: Some((near: 0.0, far: 30.0)),");
        assert!(error.contains("attenuation.near"), "{error}");
        let text = document(&format!(
            "{}\n        attenuation: Some((near: 0.0, far: 30.0, curve: Linear)),",
            sounds()
        ));
        Cue::from_document(&text).expect("a designer's fade starts anywhere");
    }

    #[test]
    fn a_cooked_cue_reads_back_equal_and_names_its_sounds() {
        let cue = Cue::from_document(&document(&format!(
            "{}\n        attenuation: Some((near: 2.0, far: 8.0)),\n        hitstop: Freeze,",
            sounds()
        )))
        .unwrap();
        let cooked = cue.to_cooked().unwrap();
        assert_eq!(Cue::read(&cooked).unwrap().to_owned_cue(), cue);
    }

    #[test]
    fn what_a_read_refuses_is_refused_cooked_too() {
        let mut cue = Cue::from_document(&document(&sounds())).unwrap();
        cue.volume_db = [3.0, 1.0];
        let cooked = cue.to_cooked().unwrap();
        assert!(matches!(Cue::read(&cooked), Err(CueError::Field { field: "volume_db", .. })));
    }

    #[test]
    fn malformed_cues_name_the_bad_field() {
        // Each row: lines that break the document, and what the error must say.
        let rows: &[(&str, &[&str])] = &[
            // Fields that don't exist, and values of the wrong kind or variant.
            ("        volum_db: (-1.0, 0.0),", &["volum_db"]),
            ("        volume_db: \"loud\",", &["volume_db", "line 6"]),
            ("        pitch: 1.0,", &["pitch"]),
            ("        bus: Music,", &["bus", "Music"]),
            ("        hitstop: Duck(\"lots\"),", &["hitstop"]),
            ("        attenuation: Some((near: 1.0)),", &["attenuation", "far"]),
            // Values that parse but can't be played.
            ("        volume_db: (3.0, 1.0),", &["volume_db"]),
            ("        volume_db: (NaN, 1.0),", &["volume_db"]),
            ("        volume_db: (0.0, 100.0),", &["volume_db"]),
            ("        pitch: (1.2, 0.8),", &["pitch"]),
            ("        pitch: (0.0, 1.0),", &["pitch"]),
            ("        pitch: (1.0, inf),", &["pitch"]),
            ("        attenuation: Some((near: -1.0, far: 3.0)),", &["attenuation.near"]),
            ("        attenuation: Some((near: 5.0, far: 3.0)),", &["attenuation.far"]),
            ("        attenuation: Some((near: 1.0, far: inf)),", &["attenuation.far"]),
            ("        hitstop: Duck(6.0),", &["hitstop"]),
            ("        hitstop: Duck(NaN),", &["hitstop"]),
        ];
        let mut failures = Vec::new();
        for (lines, expected) in rows {
            let error = error_of(lines);
            if !expected.iter().all(|word| error.contains(word)) {
                failures.push(format!("{lines:?} said {error:?}, which lacks one of {expected:?}"));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn a_missing_or_empty_sound_list_says_so() {
        let error = Cue::from_document(&document("        bus: Ui,")).unwrap_err().to_string();
        assert!(error.contains("sounds"), "{error}");
        let error = Cue::from_document(&document("        sounds: [],")).unwrap_err().to_string();
        assert!(error.contains("sounds"), "{error}");
        let error =
            Cue::from_document(&document("        sounds: [\"not-an-id\"],")).unwrap_err().to_string();
        assert!(error.contains("sounds"), "{error}");
    }

    #[test]
    fn a_document_of_another_type_is_not_a_cue() {
        let text = "(type: \"sapphire::assets::Other\", version: 1, data: ())";
        let error = Cue::from_document(text).unwrap_err();
        assert!(
            matches!(&error, CueError::Document(error) if matches!(**error, DocumentError::WrongType { .. }))
        );
    }

    #[test]
    fn attenuation_falls_from_full_to_silence() {
        for curve in [AttenuationCurve::Linear, AttenuationCurve::Inverse] {
            let attenuation = Attenuation { near: 2.0, far: 40.0, curve };
            assert_eq!(attenuation.gain_db(0.0), 0.0);
            assert_eq!(attenuation.gain_db(2.0), 0.0);
            assert_eq!(attenuation.gain_db(40.0), SILENCE_DB);
            assert_eq!(attenuation.gain_db(1000.0), SILENCE_DB);
            let mut last = 0.0;
            for meters in 3..40 {
                let gain = attenuation.gain_db(meters as f32);
                assert!(gain < last, "{curve:?} at {meters} m: {gain} after {last}");
                assert!(gain > SILENCE_DB, "{curve:?} at {meters} m: {gain}");
                last = gain;
            }
        }
        // The inverse law: every doubling of the distance is 6 dB, until the fade at the far end.
        let attenuation = Attenuation { near: 1.0, far: 100.0, curve: AttenuationCurve::Inverse };
        assert!((attenuation.gain_db(2.0) + 6.02).abs() < 0.01);
        assert!((attenuation.gain_db(16.0) + 24.08).abs() < 0.01);
    }

    #[test]
    fn only_the_interface_leaves_the_world_clock() {
        for bus in Bus::ALL {
            assert_eq!(bus.follows_world_clock(), bus != Bus::Ui, "{bus:?}");
        }
        for (index, bus) in Bus::ALL.into_iter().enumerate() {
            assert_eq!(bus.index(), index);
        }
    }
}
