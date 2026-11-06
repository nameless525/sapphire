//! The timeline document (`.timeline.ron`): a cutscene or a cinematic, written by hand or by the editor.
//!
//! ```ron
//! (
//!     type: "sapphire::timeline::TimelineDocument",
//!     version: 1,
//!     data: (
//!         // Where the simulation is let go, in seconds. A blend out runs on past it.
//!         length: 6.0,
//!         // `Real` holds the simulation; `Steps` runs it beneath, input held.
//!         clock: Real,
//!         camera: Some((
//!             // Half-lives, as every blend of the director's is. 0 is a cut.
//!             blend_in: 0.3,
//!             blend_out: 0.25,
//!             // `Exact` is the author's framing, walls or no walls; `Clear` keeps the eye out of them.
//!             eye: Exact,
//!             keys: [
//!                 (at: 0.0, eye: (0.0, 1.6, -4.0), look_at: (0.0, 1.4, 0.0), fov_degrees: 35.0),
//!                 (at: 2.5, eye: (1.5, 1.6, -3.0), look_at: (0.0, 1.4, 0.0), fov_degrees: 30.0, ease: Smooth),
//!                 // A cut: the shot before holds until here, then this one starts.
//!                 (at: 3.0, eye: (-2.0, 1.2, 2.0), look_at: (0.0, 1.5, 0.0), fov_degrees: 40.0, ease: Cut),
//!             ],
//!         )),
//!         characters: [(
//!             role: "hero",
//!             // A row of the character's animation graph, played from its start at `from`.
//!             plays: [(row: "bow", from: 0.5, to: 2.0)],
//!             places: [(at: 0.0, position: (0.0, 0.0, 0.0), yaw_degrees: 180.0)],
//!         )],
//!         // A row of the cue table: its effects, sounds, shake and screen effect, all at once.
//!         cues: [(at: 1.0, cue: "bell", place: Some((0.0, 2.0, 0.0)))],
//!         // How far from what the look resolved, ending on 0.
//!         post: [(parameter: Exposure, keys: [(at: 0.0, value: 0.0), (at: 1.0, value: -0.5), (at: 5.0, value: 0.0)])],
//!         // Keys for `sapphire_loc`, never text.
//!         subtitles: [(from: 1.0, to: 3.0, key: "intro-first-line", speaker: Some("hero-name"))],
//!         // `Steps` alone, in thousandths of normal speed.
//!         time_scale: [],
//!         events: [(at: 5.5, event: "boss_wakes")],
//!     ),
//! )
//! ```
//!
//! [`TimelineDocument::check`] names the field of everything wrong (`camera.keys[2].at`), and unknown
//! fields are errors. Everything is plain and reflected, so the editor can edit a document by a
//! reflection path (`camera.keys[1].eye[1]`) and splice the text. Every position is in the timeline's own
//! space, which whoever plays it places: a cutscene at the zone's origin, a cinematic on who cast it.

use std::collections::BTreeSet;

use bevy_reflect::Reflect;
use sapphire_core::Name;
use sapphire_reflect::Versioned;
use sapphire_serialize::{CookedError, DocumentError};
use sapphire_time::TICK_HZ;
use serde::{Deserialize, Serialize};

use crate::cooked::{
    Beat, BeatKind, CookedCharacter, CookedCue, CookedPlay, MAX_BEATS, Timeline, TimelinePresentation,
    TimelineSimulation,
};

/// The most entries any list of a timeline may hold.
pub const MAX_ENTRIES: usize = 1 << 16;
/// The longest name or key, in bytes.
pub const MAX_NAME: usize = 256;
/// The longest timeline, in seconds: an hour is past any cutscene, and keeps a tick well inside a `u32`.
pub const MAX_LENGTH: f32 = 3600.0;
/// How far from the origin a position may be, in metres: as far as a zone's coordinates reach, and past
/// any shot's.
pub const MAX_COORDINATE: f32 = 1_000_000.0;
/// The longest blend's half-life, in seconds.
pub const MAX_BLEND: f32 = 60.0;
/// The fastest a timeline may run the world, in thousandths of normal speed.
pub const MAX_PER_MILLE: u32 = 10_000;
/// How far a post parameter may move: more than any of them means anything at.
pub const MAX_POST_VALUE: f32 = 32.0;
/// Normal speed, in thousandths: where a timeline's first ramp starts from.
const NORMAL_PER_MILLE: u32 = 1000;

/// Which clock a timeline's playhead reads. See the [crate docs](crate).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq))]
#[type_path = "sapphire::timeline"]
pub enum TimelineClock {
    /// Real seconds, with the simulation held at a step's edge: a cutscene.
    Real,
    /// The simulation's steps, unscaled, with it running beneath and input held: a cinematic in a fight.
    Steps,
}

/// How a key is arrived at, from the key before it.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq))]
#[type_path = "sapphire::timeline"]
pub enum Ease {
    #[default]
    Linear,
    /// Leaving the last key and reaching this one at no speed.
    Smooth,
    /// The key before holds until this one's time, then this one starts.
    Cut,
}

impl Ease {
    /// How far from the key before to this one, `share` of the way through the time between them.
    #[must_use]
    pub fn eased(self, share: f32) -> f32 {
        match self {
            Self::Linear => share,
            Self::Smooth => share * share * (3.0 - 2.0 * share),
            Self::Cut => 0.0,
        }
    }
}

/// What an authored shot asks of the eye.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq))]
#[type_path = "sapphire::timeline"]
pub enum EyeRule {
    /// The eye goes exactly where the key says, through anything: for a cutscene, placed by its author in
    /// a zone they can see.
    #[default]
    Exact,
    /// The eye keeps the collision's rule, stopping and sliding: for a cinematic, which plays wherever a
    /// fight happens to be.
    Clear,
}

/// A timeline document (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct TimelineDocument {
    /// In seconds: where the simulation is let go. A camera's blend out runs on past it.
    pub length: f32,
    pub clock: TimelineClock,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraTrack>,
    #[serde(default)]
    pub characters: Vec<CharacterTrack>,
    /// Effects and sound, as rows of the cue table, in time order.
    #[serde(default)]
    pub cues: Vec<CueKey>,
    #[serde(default)]
    pub post: Vec<PostTrack>,
    /// In time order, one at a time.
    #[serde(default)]
    pub subtitles: Vec<SubtitleKey>,
    /// The world clock's speed, in time order. [`TimelineClock::Steps`] alone.
    #[serde(default)]
    pub time_scale: Vec<ScaleKey>,
    /// What gameplay is told, in time order.
    #[serde(default)]
    pub events: Vec<EventKey>,
}

impl Versioned for TimelineDocument {
    /// 1: M3 Q1.
    const VERSION: u32 = 1;
}

/// The camera's shots, and how they blend with the gameplay camera.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct CameraTrack {
    /// In from the gameplay camera, as a half-life in seconds; 0 is a cut.
    #[serde(default)]
    pub blend_in: f32,
    /// Back to the gameplay camera, past the timeline's end, as a half-life in seconds; 0 is a cut.
    #[serde(default)]
    pub blend_out: f32,
    #[serde(default)]
    pub eye: EyeRule,
    /// At least one, by strictly later times.
    pub keys: Vec<CameraKey>,
}

/// One key of a camera's shot.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct CameraKey {
    /// In seconds.
    pub at: f32,
    /// Where it stands.
    pub eye: [f32; 3],
    /// What it looks at.
    pub look_at: [f32; 3],
    /// About where it looks, in degrees.
    #[serde(default)]
    pub roll_degrees: f32,
    /// The full vertical field of view.
    pub fov_degrees: f32,
    #[serde(default)]
    pub ease: Ease,
}

/// A character's part: what its animation graph plays, and where it stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct CharacterTrack {
    /// Unique among the timeline's characters: who plays it is for whoever plays the timeline to say.
    pub role: String,
    /// By strictly later, non-overlapping times. Outside them its graph goes on as it would have.
    #[serde(default)]
    pub plays: Vec<PlayRange>,
    /// By strictly later times. With none it stays where it is.
    #[serde(default)]
    pub places: Vec<PlaceKey>,
}

/// A row of a character's animation graph, played from its start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct PlayRange {
    /// A combat row's move name in the character's graph, which is what binds its clip.
    pub row: String,
    pub from: f32,
    pub to: f32,
}

/// Where a character stands.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct PlaceKey {
    pub at: f32,
    /// Where its feet are.
    pub position: [f32; 3],
    /// About +Y; 0 faces +Z.
    #[serde(default)]
    pub yaw_degrees: f32,
    #[serde(default)]
    pub ease: Ease,
}

/// A row of the cue table, fired once as the playhead reaches it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct CueKey {
    pub at: f32,
    pub cue: String,
    /// Where its effects and sound happen: the timeline's origin with none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<[f32; 3]>,
}

/// What a post track moves. Each animates for free: nothing here bakes a grading LUT, whose inputs are
/// meant to change at keys of a day rather than every frame.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq))]
#[type_path = "sapphire::timeline"]
pub enum PostParameter {
    /// Stops.
    Exposure,
    /// Bloom's intensity.
    Bloom,
    /// The vignette's intensity.
    Vignette,
    /// The wide glow's intensity.
    Diffusion,
}

/// One parameter's track: how far it stands from what the look resolved. Offsets rather than values, so
/// a track reads the same over any hour, and the look has nothing to jump back from where one ends on 0.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct PostTrack {
    /// Each at most once.
    pub parameter: PostParameter,
    /// At least one, by strictly later times, the last at 0.
    pub keys: Vec<ScalarKey>,
}

/// A number at a time.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct ScalarKey {
    pub at: f32,
    pub value: f32,
    #[serde(default)]
    pub ease: Ease,
}

/// A line of subtitles. It carries keys, which `sapphire_loc` resolves, and no text.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct SubtitleKey {
    pub from: f32,
    pub to: f32,
    /// The line's key.
    pub key: String,
    /// The key of who's speaking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// The world clock's speed from a time on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct ScaleKey {
    /// In seconds, which cook to the nearest step: a time scale is the simulation's.
    pub at: f32,
    /// Thousandths of normal speed.
    pub per_mille: u32,
    /// Over how many seconds from the speed before, cooked to a beat on each step. 0 is at once.
    #[serde(default)]
    pub ramp: f32,
}

/// Something gameplay is told, by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::timeline"]
#[serde(deny_unknown_fields)]
pub struct EventKey {
    pub at: f32,
    pub event: String,
    /// What it carries, for whoever knows the name.
    #[serde(default)]
    pub value: i32,
}

/// A timeline document that can't cook, or a cooked timeline the runtime refuses.
#[derive(Debug, thiserror::Error)]
pub enum TimelineError {
    #[error(transparent)]
    Document(#[from] Box<DocumentError>),
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("`{field}`: {problem}")]
    Invalid { field: String, problem: String },
}

impl TimelineError {
    pub(crate) fn invalid(field: impl Into<String>, problem: impl Into<String>) -> Self {
        Self::Invalid { field: field.into(), problem: problem.into() }
    }
}

/// The whole step `seconds` falls nearest, by IEEE's exact operations alone.
#[must_use]
pub(crate) fn tick_of(seconds: f32) -> u32 {
    (seconds * TICK_HZ as f32).round() as u32
}

/// `value`, if it's a number, within `0..=limit`.
pub(crate) fn check_time(
    field: impl FnOnce() -> String,
    value: f32,
    limit: f32,
) -> Result<(), TimelineError> {
    if !value.is_finite() {
        return Err(TimelineError::invalid(field(), "not a number"));
    }
    if !(0.0..=limit).contains(&value) {
        return Err(TimelineError::invalid(field(), format!("{value} is outside 0 to {limit}")));
    }
    Ok(())
}

pub(crate) fn check_position(
    field: impl FnOnce() -> String,
    position: [f32; 3],
) -> Result<(), TimelineError> {
    if position.iter().any(|value| !value.is_finite() || value.abs() > MAX_COORDINATE) {
        return Err(TimelineError::invalid(field(), format!("{position:?} is no place")));
    }
    Ok(())
}

fn check_finite(field: impl FnOnce() -> String, value: f32) -> Result<(), TimelineError> {
    if value.is_finite() { Ok(()) } else { Err(TimelineError::invalid(field(), "not a number")) }
}

fn check_name(field: impl FnOnce() -> String, name: &str) -> Result<(), TimelineError> {
    if name.is_empty() || name.len() > MAX_NAME {
        return Err(TimelineError::invalid(field(), format!("a name is 1 to {MAX_NAME} bytes")));
    }
    Ok(())
}

/// A key `sapphire_loc` can look up: a Fluent identifier, optionally with an attribute after a dot.
fn check_key(field: impl FnOnce() -> String, key: &str) -> Result<(), TimelineError> {
    let good = (1..=MAX_NAME).contains(&key.len())
        && key.split('.').count() <= 2
        && key.split('.').all(|part| {
            let mut bytes = part.bytes();
            bytes.next().is_some_and(|first| first.is_ascii_alphabetic())
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        });
    if good {
        Ok(())
    } else {
        Err(TimelineError::invalid(
            field(),
            format!("{key:?} is no localization key: subtitles carry keys, not text"),
        ))
    }
}

/// That `times` rise, strictly unless `ties`, and all fall within `length`.
pub(crate) fn check_times(
    list: &str,
    times: impl Iterator<Item = f32>,
    length: f32,
    ties: bool,
) -> Result<(), TimelineError> {
    let mut last = None;
    for (index, at) in times.enumerate() {
        let field = || format!("{list}[{index}].at");
        check_time(field, at, length)?;
        if last.is_some_and(|last| if ties { at < last } else { at <= last }) {
            let order = if ties { "in time order" } else { "strictly later than the one before" };
            return Err(TimelineError::invalid(field(), format!("{at} isn't {order}")));
        }
        last = Some(at);
    }
    Ok(())
}

pub(crate) fn check_count(list: &str, count: usize) -> Result<(), TimelineError> {
    if count > MAX_ENTRIES {
        return Err(TimelineError::invalid(
            list,
            format!("{count} entries, past the limit of {MAX_ENTRIES}"),
        ));
    }
    Ok(())
}

pub(crate) fn check_camera(camera: &CameraTrack, length: f32) -> Result<(), TimelineError> {
    check_time(|| "camera.blend_in".to_owned(), camera.blend_in, MAX_BLEND)?;
    check_time(|| "camera.blend_out".to_owned(), camera.blend_out, MAX_BLEND)?;
    check_count("camera.keys", camera.keys.len())?;
    if camera.keys.is_empty() {
        return Err(TimelineError::invalid("camera.keys", "a camera track has at least one key"));
    }
    check_times("camera.keys", camera.keys.iter().map(|key| key.at), length, false)?;
    for (index, key) in camera.keys.iter().enumerate() {
        check_position(|| format!("camera.keys[{index}].eye"), key.eye)?;
        check_position(|| format!("camera.keys[{index}].look_at"), key.look_at)?;
        check_finite(|| format!("camera.keys[{index}].roll_degrees"), key.roll_degrees)?;
        if !(key.fov_degrees > 1.0 && key.fov_degrees < 170.0) {
            let problem = format!("{} isn't a field of view: above 1 and below 170 degrees", key.fov_degrees);
            return Err(TimelineError::invalid(format!("camera.keys[{index}].fov_degrees"), problem));
        }
        let along: f32 = (0..3)
            .map(|axis| {
                let distance = key.look_at[axis] - key.eye[axis];
                distance * distance
            })
            .sum();
        if along < 1e-6 {
            let problem = "it looks at where it stands";
            return Err(TimelineError::invalid(format!("camera.keys[{index}].look_at"), problem));
        }
    }
    Ok(())
}

pub(crate) fn check_places(field: &str, places: &[PlaceKey], length: f32) -> Result<(), TimelineError> {
    check_count(field, places.len())?;
    check_times(field, places.iter().map(|key| key.at), length, false)?;
    for (index, key) in places.iter().enumerate() {
        check_position(|| format!("{field}[{index}].position"), key.position)?;
        check_finite(|| format!("{field}[{index}].yaw_degrees"), key.yaw_degrees)?;
    }
    Ok(())
}

pub(crate) fn check_post(post: &[PostTrack], length: f32) -> Result<(), TimelineError> {
    check_count("post", post.len())?;
    for (index, track) in post.iter().enumerate() {
        let keys = format!("post[{index}].keys");
        if post[..index].iter().any(|other| other.parameter == track.parameter) {
            let problem = format!("{:?} has a track already", track.parameter);
            return Err(TimelineError::invalid(format!("post[{index}].parameter"), problem));
        }
        check_count(&keys, track.keys.len())?;
        check_times(&keys, track.keys.iter().map(|key| key.at), length, false)?;
        for (at, key) in track.keys.iter().enumerate() {
            if !key.value.is_finite() || key.value.abs() > MAX_POST_VALUE {
                let problem = format!("{} is outside ±{MAX_POST_VALUE}", key.value);
                return Err(TimelineError::invalid(format!("{keys}[{at}].value"), problem));
            }
        }
        let Some(last) = track.keys.last() else {
            return Err(TimelineError::invalid(keys, "a post track has at least one key"));
        };
        if last.value != 0.0 {
            let problem = "a post track ends on 0, or the look jumps back when the timeline lets go of it";
            return Err(TimelineError::invalid(format!("{keys}[{}].value", track.keys.len() - 1), problem));
        }
    }
    Ok(())
}

pub(crate) fn check_subtitles(subtitles: &[SubtitleKey], length: f32) -> Result<(), TimelineError> {
    check_count("subtitles", subtitles.len())?;
    let mut last = 0.0;
    for (index, line) in subtitles.iter().enumerate() {
        check_time(|| format!("subtitles[{index}].from"), line.from, length)?;
        check_time(|| format!("subtitles[{index}].to"), line.to, length)?;
        if line.to <= line.from {
            return Err(TimelineError::invalid(
                format!("subtitles[{index}].to"),
                "a line ends after it starts",
            ));
        }
        if line.from < last {
            let problem = format!("{} starts before the line before it ends, at {last}", line.from);
            return Err(TimelineError::invalid(format!("subtitles[{index}].from"), problem));
        }
        last = line.to;
        check_key(|| format!("subtitles[{index}].key"), &line.key)?;
        if let Some(speaker) = &line.speaker {
            check_key(|| format!("subtitles[{index}].speaker"), speaker)?;
        }
    }
    Ok(())
}

impl TimelineDocument {
    /// Reads a document from RON text and checks it.
    pub fn from_ron(text: &str) -> Result<Self, TimelineError> {
        let document: Self = sapphire_serialize::from_ron(text).map_err(Box::new)?;
        document.check()?;
        Ok(document)
    }

    /// How many steps long its simulation half is.
    #[must_use]
    pub fn ticks(&self) -> u32 {
        match self.clock {
            TimelineClock::Real => 0,
            TimelineClock::Steps => tick_of(self.length),
        }
    }

    /// Checks everything, naming the field.
    pub fn check(&self) -> Result<(), TimelineError> {
        check_time(|| "length".to_owned(), self.length, MAX_LENGTH)?;
        if self.length <= 0.0 {
            return Err(TimelineError::invalid("length", "a timeline takes some time"));
        }
        if self.clock == TimelineClock::Steps && self.ticks() == 0 {
            return Err(TimelineError::invalid("length", "a `Steps` timeline is at least a step long"));
        }
        if let Some(camera) = &self.camera {
            check_camera(camera, self.length)?;
        }
        self.check_characters()?;
        check_count("cues", self.cues.len())?;
        check_times("cues", self.cues.iter().map(|key| key.at), self.length, true)?;
        for (index, key) in self.cues.iter().enumerate() {
            check_name(|| format!("cues[{index}].cue"), &key.cue)?;
            if let Some(place) = key.place {
                check_position(|| format!("cues[{index}].place"), place)?;
            }
        }
        check_post(&self.post, self.length)?;
        check_subtitles(&self.subtitles, self.length)?;
        self.check_time_scale()?;
        check_count("events", self.events.len())?;
        check_times("events", self.events.iter().map(|key| key.at), self.length, true)?;
        for (index, key) in self.events.iter().enumerate() {
            check_name(|| format!("events[{index}].event"), &key.event)?;
            if self.clock == TimelineClock::Steps && tick_of(key.at) >= self.ticks() {
                let problem = "it falls on the timeline's end, which no step runs at";
                return Err(TimelineError::invalid(format!("events[{index}].at"), problem));
            }
        }
        self.beat_count().map(|_| ())
    }

    fn check_characters(&self) -> Result<(), TimelineError> {
        check_count("characters", self.characters.len())?;
        let mut roles = BTreeSet::new();
        for (index, track) in self.characters.iter().enumerate() {
            check_name(|| format!("characters[{index}].role"), &track.role)?;
            if !roles.insert(track.role.as_str()) {
                let problem = format!("{:?} has a track already", track.role);
                return Err(TimelineError::invalid(format!("characters[{index}].role"), problem));
            }
            let plays = format!("characters[{index}].plays");
            check_count(&plays, track.plays.len())?;
            let mut last = None;
            for (at, play) in track.plays.iter().enumerate() {
                check_name(|| format!("{plays}[{at}].row"), &play.row)?;
                check_time(|| format!("{plays}[{at}].from"), play.from, self.length)?;
                check_time(|| format!("{plays}[{at}].to"), play.to, self.length)?;
                if play.to <= play.from {
                    return Err(TimelineError::invalid(
                        format!("{plays}[{at}].to"),
                        "it ends after it starts",
                    ));
                }
                if last.is_some_and(|last| play.from < last) {
                    let problem = "it starts before the one before it ends";
                    return Err(TimelineError::invalid(format!("{plays}[{at}].from"), problem));
                }
                last = Some(play.to);
            }
            check_places(&format!("characters[{index}].places"), &track.places, self.length)?;
        }
        Ok(())
    }

    fn check_time_scale(&self) -> Result<(), TimelineError> {
        check_count("time_scale", self.time_scale.len())?;
        if !self.time_scale.is_empty() && self.clock == TimelineClock::Real {
            let problem = "a `Real` timeline holds the simulation, so no step runs under its time scale";
            return Err(TimelineError::invalid("time_scale", problem));
        }
        let ticks = self.ticks();
        let mut free_from = 0;
        for (index, key) in self.time_scale.iter().enumerate() {
            check_time(|| format!("time_scale[{index}].at"), key.at, self.length)?;
            check_time(|| format!("time_scale[{index}].ramp"), key.ramp, self.length)?;
            if key.per_mille > MAX_PER_MILLE {
                let problem = format!("{} is past the limit of {MAX_PER_MILLE}", key.per_mille);
                return Err(TimelineError::invalid(format!("time_scale[{index}].per_mille"), problem));
            }
            let (at, ramp) = (tick_of(key.at), tick_of(key.ramp));
            if at < free_from {
                let problem =
                    format!("step {at} is before the key before it has finished, at step {free_from}");
                return Err(TimelineError::invalid(format!("time_scale[{index}].at"), problem));
            }
            let end = at + ramp;
            if end >= ticks {
                let problem = format!("it finishes on step {end}, past the timeline's last, {}", ticks - 1);
                return Err(TimelineError::invalid(format!("time_scale[{index}].at"), problem));
            }
            free_from = end + 1;
        }
        Ok(())
    }

    /// Called after temporal validation, so each ramp is bounded by the timeline's tick count.
    fn beat_count(&self) -> Result<usize, TimelineError> {
        let count = self
            .time_scale
            .iter()
            .fold(self.events.len(), |count, key| count + tick_of(key.ramp).max(1) as usize);
        if count > MAX_BEATS {
            let problem = format!("{count} beats, past the limit of {MAX_BEATS}");
            return Err(TimelineError::invalid("time_scale", problem));
        }
        Ok(count)
    }

    /// The beats by step, a step's time scale before its events and events in the document's order, with
    /// a refusal if there are more than a run should walk past.
    fn beats(&self) -> Result<Vec<Beat>, TimelineError> {
        let mut beats = Vec::with_capacity(self.beat_count()?);
        let mut speed = NORMAL_PER_MILLE;
        for key in &self.time_scale {
            let (at, ramp) = (tick_of(key.at), tick_of(key.ramp));
            // Whole numbers of per mille, so a ramp holds nothing a platform could round.
            let (from, to) = (i64::from(speed), i64::from(key.per_mille));
            if ramp == 0 {
                beats.push(Beat { tick: at, what: BeatKind::Scale { per_mille: key.per_mille } });
            }
            for step in 1..=ramp {
                let per_mille = from + (to - from) * i64::from(step) / i64::from(ramp);
                beats.push(Beat { tick: at + step, what: BeatKind::Scale { per_mille: per_mille as u32 } });
            }
            speed = key.per_mille;
        }
        for key in &self.events {
            // A timeline of no steps has one place for them: the first step after it.
            let tick = if self.ticks() == 0 { 0 } else { tick_of(key.at) };
            beats.push(Beat {
                tick,
                what: BeatKind::Event { event: Name::new(&key.event).as_u64(), value: key.value },
            });
        }
        // Stable, which is what keeps a step's time scale first and its events in order.
        beats.sort_by_key(|beat| beat.tick);
        Ok(beats)
    }

    /// The cooked form, after checking everything again: a document reflected into by an editor hasn't been
    /// through [`Self::from_ron`].
    pub fn cook(&self) -> Result<Timeline, TimelineError> {
        self.check()?;
        let timeline = Timeline {
            simulation: TimelineSimulation { ticks: self.ticks(), beats: self.beats()? },
            presentation: TimelinePresentation {
                length: self.length,
                clock: self.clock,
                camera: self.camera.clone(),
                characters: self
                    .characters
                    .iter()
                    .map(|track| CookedCharacter {
                        role: Name::new(&track.role).as_u64(),
                        plays: track
                            .plays
                            .iter()
                            .map(|play| CookedPlay {
                                row: Name::new(&play.row).as_u64(),
                                from: play.from,
                                to: play.to,
                            })
                            .collect(),
                        places: track.places.clone(),
                    })
                    .collect(),
                cues: self
                    .cues
                    .iter()
                    .map(|key| CookedCue {
                        at: key.at,
                        cue: Name::new(&key.cue).as_u64(),
                        place: key.place.unwrap_or([0.0; 3]),
                    })
                    .collect(),
                post: self.post.clone(),
                subtitles: self.subtitles.clone(),
            },
        };
        timeline.validate()?;
        Ok(timeline)
    }
}
