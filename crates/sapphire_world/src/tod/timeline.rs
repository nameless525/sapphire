//! [`TodTimeline`]: keys through the day, wrapping at midnight.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use super::TodError;
use super::key::{TodGroups, TodKey};
use super::look::{TodLook, celestial_sphere};

/// Hours in a day: timeline keys sit in `0 ≤ time < DAY_HOURS`.
pub const DAY_HOURS: f32 = 24.0;

/// The shipped default day (see [`TodTimeline::default_day`]).
const DEFAULT_DAY: &str = include_str!("../../default_day.ron");

/// A day of looks: [`TodKey`]s at times of day, blended between neighbors and wrapping from the
/// last key of the day to the first of the next (a `TodTimeline` document, schema 1).
///
/// The base timeline sets every group in every key but the optional ones
/// ([`Self::is_complete`]); a zone's timeline may set only the groups it overrides. Either way every key sets the same groups, the keys'
/// times increase, and every value is in range: a timeline is valid however it was made,
/// deserialization included. It has a stable type path but no `Reflect`, which could build or
/// edit one without the checks; an editor edits a [`TodKey`] and makes a new timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::world"]
#[serde(try_from = "UncheckedTimeline")]
pub struct TodTimeline {
    keys: Vec<TodKey>,
    #[serde(skip)]
    groups: TodGroups,
}

impl Versioned for TodTimeline {
    const VERSION: u32 = 1;
}

/// A timeline as written, before [`TodTimeline::new`] checks it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedTimeline {
    keys: Vec<TodKey>,
}

impl TryFrom<UncheckedTimeline> for TodTimeline {
    type Error = TodError;

    fn try_from(timeline: UncheckedTimeline) -> Result<Self, TodError> {
        Self::new(timeline.keys)
    }
}

impl TodTimeline {
    /// A timeline of `keys`, checked: at least one, at increasing times within the day, every
    /// key setting the same groups, every value in range.
    pub fn new(keys: Vec<TodKey>) -> Result<Self, TodError> {
        let groups = check_keys(&keys, KeyTimes::Day)?;
        Ok(Self { keys, groups })
    }

    /// Sapphire's default day (`default_day.ron` beside this crate's manifest), compiled in:
    /// eight keys from midnight through a blue hour, sunrise, the day, golden hour, sunset and
    /// dusk. At 10:00 it is exactly the renderer's default look (the sun the sandbox has always
    /// drawn with, [`sapphire_render::SkyGradient::default`] and
    /// [`sapphire_render::PostParams::default`]), so a scene that doesn't choose a time looks as
    /// it did before time of day, but for its fog (M1 W3): a light haze the renderer's default
    /// leaves out.
    pub fn default_day() -> Self {
        Self::from_ron(DEFAULT_DAY).expect("the shipped default day is checked by this module's tests")
    }

    /// Reads a timeline document, upgrading older schema versions and checking the timeline.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// The keys, in time order.
    pub fn keys(&self) -> &[TodKey] {
        &self.keys
    }

    /// The groups every key sets.
    pub fn groups(&self) -> TodGroups {
        self.groups
    }

    /// Whether the keys set every group the base timeline must: all but the optional ones
    /// ([`TodGroup::OPTIONAL`](super::TodGroup::OPTIONAL): the sky's and post's layers, which draw
    /// nothing where unset, and the emission scale, which shows emission as authored).
    pub fn is_complete(&self) -> bool {
        TodGroups::REQUIRED.first_missing_from(self.groups).is_none()
    }

    /// Where `hours` falls: the key at or before it, the next key (wrapping at midnight), and
    /// the eased weight of the next key. Times outside the day wrap into it.
    pub fn segment(&self, hours: f32) -> Segment {
        segment(&self.keys, hours, Some(DAY_HOURS))
    }

    /// The look at `hours`: the groups this timeline sets, over the renderer's defaults for the
    /// rest, with the celestial sphere turned to the hour.
    pub fn evaluate(&self, hours: f32) -> TodLook {
        let mut look = TodLook { sphere: celestial_sphere(hours), ..TodLook::default() };
        self.apply(&mut look, hours, 1.0);
        look
    }

    /// Blends this timeline's groups at `hours` over `look` with `weight` (1 replaces them).
    pub(crate) fn apply(&self, look: &mut TodLook, hours: f32, weight: f32) {
        let Segment { from, to, weight: segment } = self.segment(hours);
        look.blend_keys(&self.keys[from], &self.keys[to], segment, weight);
    }
}

/// Where a time falls among keys: blend `weight` of the way from key `from` to key `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub from: usize,
    pub to: usize,
    /// The eased weight of `to`, 0 to 1. Exactly 0 on a key.
    pub weight: f32,
}

/// Finds `time` among `keys` (sorted, at least one): wrapping around `period` if given, else
/// holding the first and last keys' looks before and after them.
pub(crate) fn segment(keys: &[TodKey], time: f32, period: Option<f32>) -> Segment {
    let last = keys.len() - 1;
    let hold = |index: usize| Segment { from: index, to: index, weight: 0.0 };
    if last == 0 {
        return hold(0);
    }
    let time = match period {
        Some(period) => {
            // `rem_euclid` can round up to the period itself for tiny negative times.
            let wrapped = if time.is_finite() { time.rem_euclid(period) } else { 0.0 };
            if wrapped >= period { 0.0 } else { wrapped }
        }
        None if time.is_nan() || time <= keys[0].time => return hold(0),
        None if time >= keys[last].time => return hold(last),
        None => time,
    };
    // The first key after `time`; the one before it starts the segment (the day's last key,
    // before the first key of the day).
    let next = keys.partition_point(|key| key.time <= time);
    let from = if next == 0 { last } else { next - 1 };
    let to = if next > last { 0 } else { next };
    let start = keys[from].time;
    let (elapsed, span) = match period {
        Some(period) => ((time - start).rem_euclid(period), (keys[to].time - start).rem_euclid(period)),
        None => (time - start, keys[to].time - start),
    };
    let ease: sapphire_math::Ease = keys[from].ease.into();
    Segment { from, to, weight: ease.apply(elapsed / span) }
}

/// The times a run of keys may take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum KeyTimes {
    /// A timeline's: hours, `0 ≤ time < 24`.
    Day,
    /// A cutscene's: seconds into a shot of this many, `0 ≤ time ≤ duration`.
    Shot(f32),
}

impl KeyTimes {
    fn contains(self, time: f32) -> bool {
        match self {
            Self::Day => (0.0..DAY_HOURS).contains(&time),
            Self::Shot(duration) => (0.0..=duration).contains(&time),
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Day => "0 ≤ hours < 24",
            Self::Shot(_) => "0 ≤ seconds ≤ the shot's duration",
        }
    }
}

/// Checks `keys`: at least one, times within `times` and strictly increasing, the same groups
/// in every key, and every value in range. Returns the groups.
pub(crate) fn check_keys(keys: &[TodKey], times: KeyTimes) -> Result<TodGroups, TodError> {
    let first = keys.first().ok_or(TodError::NoKeys)?;
    let groups = first.groups();
    let mut previous = None;
    for (index, key) in keys.iter().enumerate() {
        if !times.contains(key.time) {
            return Err(TodError::TimeOutOfRange { key: index, time: key.time, expected: times.describe() });
        }
        if previous.is_some_and(|previous| key.time <= previous) {
            return Err(TodError::Unordered { key: index });
        }
        previous = Some(key.time);
        let found = key.groups();
        if found != groups {
            return Err(TodError::MixedGroups { key: index, found, expected: groups });
        }
        key.validate().map_err(|(field, problem)| TodError::Invalid { key: index, field, problem })?;
    }
    Ok(groups)
}
