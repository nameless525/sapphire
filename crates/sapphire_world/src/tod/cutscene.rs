//! [`TodCutscene`]: a shot's time-of-day track.

use bevy_reflect::{Reflect, TypePath};
use sapphire_math::Ease;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use sapphire_tod::{DAY_HOURS, KeyTimes, TodError, TodGroups, TodKey, TodLook, check_keys, segment};

use crate::light_rig::LightRig;

/// A cutscene's time-of-day track (spec §5.23): for one shot, it sets the time of day, overrides
/// values over the shot's seconds, or both, and blends in at the start and out at the end (a
/// `TodCutscene` document, schema 1).
///
/// - **The time** ([`ShotTime`]): the look crossfades from the world's time of day to the shot's
///   (the base timeline and zones evaluated at the shot's hour), so a shot set at sunset blends
///   in from noon rather than sweeping the sun across the sky. The world's clock keeps running
///   underneath.
/// - **The values** ([`Self::keys`]): keys at seconds into the shot, each setting the same
///   groups, blended over the look like a zone's; between keys they blend like a timeline's,
///   and before the first key and after the last they hold.
/// - **The character light rig** ([`Self::light_rig`], M1 C2): a preset for the shot, which
///   every character takes over its own (a key fixed to the camera for a close-up, say).
/// - **The weather** ([`Self::weather`], M6 W8a): a weather state, by name, over the world's
///   (`crate::weather::WeatherSystem`): a shot set in a storm, whatever the sky outside it.
///
/// All four follow [`Self::weight`]: 0 outside the shot, easing in over `blend_in` seconds and
/// out over `blend_out`. A cutscene is valid however it was made, deserialization included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::world"]
#[serde(try_from = "UncheckedCutscene")]
pub struct TodCutscene {
    duration: f32,
    blend_in: f32,
    blend_out: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    time: Option<ShotTime>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    keys: Vec<TodKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    light_rig: Option<LightRig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weather: Option<String>,
    #[serde(skip)]
    groups: TodGroups,
}

impl Versioned for TodCutscene {
    const VERSION: u32 = 1;
}

/// The time of day a shot takes place at.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct ShotTime {
    /// The time of day as the shot begins, `0 ≤ hours < 24`.
    pub hours: f32,
    /// How fast the shot's time of day runs, in hours per second of the shot: 0 holds it, and a
    /// time-lapse shot runs it fast.
    #[serde(default)]
    pub hours_per_second: f32,
}

/// A cutscene as written, before [`TodCutscene::new`] checks it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedCutscene {
    duration: f32,
    #[serde(default)]
    blend_in: f32,
    #[serde(default)]
    blend_out: f32,
    #[serde(default)]
    time: Option<ShotTime>,
    #[serde(default)]
    keys: Vec<TodKey>,
    #[serde(default)]
    light_rig: Option<LightRig>,
    #[serde(default)]
    weather: Option<String>,
}

impl TryFrom<UncheckedCutscene> for TodCutscene {
    type Error = TodError;

    fn try_from(cutscene: UncheckedCutscene) -> Result<Self, TodError> {
        let UncheckedCutscene { duration, blend_in, blend_out, time, keys, light_rig, weather } = cutscene;
        Self::checked(duration, blend_in, blend_out, time, keys, light_rig, weather)
    }
}

impl TodCutscene {
    /// A track for a shot `duration` seconds long, checked: a positive duration, blends that fit
    /// in it, a shot time within the day, keys within the shot (as a timeline's: increasing,
    /// the same groups, values in range), a light rig in range, and at least one of a time, keys
    /// and a light rig. [`Self::with_weather`] adds a weather state.
    pub fn new(
        duration: f32,
        blend_in: f32,
        blend_out: f32,
        time: Option<ShotTime>,
        keys: Vec<TodKey>,
        light_rig: Option<LightRig>,
    ) -> Result<Self, TodError> {
        Self::checked(duration, blend_in, blend_out, time, keys, light_rig, None)
    }

    /// The same track with the weather state named `state` over the world's (M6 W8a), checked: a name a
    /// weather document's state may take. Whether a document has it is the weather system's to say, which
    /// shows none for a name it doesn't know.
    pub fn with_weather(self, state: impl Into<String>) -> Result<Self, TodError> {
        let Self { duration, blend_in, blend_out, time, keys, light_rig, .. } = self;
        Self::checked(duration, blend_in, blend_out, time, keys, light_rig, Some(state.into()))
    }

    fn checked(
        duration: f32,
        blend_in: f32,
        blend_out: f32,
        time: Option<ShotTime>,
        keys: Vec<TodKey>,
        light_rig: Option<LightRig>,
        weather: Option<String>,
    ) -> Result<Self, TodError> {
        let invalid = |field, problem| Err(TodError::Cutscene { field, problem });
        if !(duration.is_finite() && duration > 0.0) {
            return invalid("duration", "must be positive and finite");
        }
        if !(blend_in.is_finite() && blend_in >= 0.0 && blend_out.is_finite() && blend_out >= 0.0) {
            return invalid("blend_in", "and `blend_out` must be finite and not negative");
        }
        if blend_in + blend_out > duration {
            return invalid("blend_in", "and `blend_out` must fit in the duration together");
        }
        if let Some(time) = &time {
            if !(0.0..DAY_HOURS).contains(&time.hours) {
                return invalid("time.hours", "must be within 0 ≤ hours < 24");
            }
            if !time.hours_per_second.is_finite() {
                return invalid("time.hours_per_second", "must be finite");
            }
        }
        if let Some(rig) = &light_rig {
            rig.validate().map_err(|(field, problem)| TodError::CutsceneRig { field, problem })?;
        }
        if let Some(state) = &weather
            && (state.is_empty() || state.len() > sapphire_tod::weather::MAX_NAME)
        {
            return invalid("weather", "must name a state: 1 to 64 bytes");
        }
        let groups = if keys.is_empty() {
            if time.is_none() && light_rig.is_none() && weather.is_none() {
                return invalid(
                    "keys",
                    "can't be empty when the cutscene sets no time, no light rig and no weather: it would \
                     change nothing",
                );
            }
            TodGroups::NONE
        } else {
            check_keys(&keys, KeyTimes::Shot(duration))?
        };
        Ok(Self { duration, blend_in, blend_out, time, keys, light_rig, weather, groups })
    }

    /// Reads a cutscene document, upgrading older schema versions and checking it.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// The shot's length in seconds.
    pub fn duration(&self) -> f32 {
        self.duration
    }

    /// Seconds over which the track eases in at the start of the shot.
    pub fn blend_in(&self) -> f32 {
        self.blend_in
    }

    /// Seconds over which the track eases out at the end of the shot.
    pub fn blend_out(&self) -> f32 {
        self.blend_out
    }

    /// The time of day the shot takes place at, if the track sets one.
    pub fn time(&self) -> Option<ShotTime> {
        self.time
    }

    /// The value keys, at seconds into the shot.
    pub fn keys(&self) -> &[TodKey] {
        &self.keys
    }

    /// The groups the value keys set.
    pub fn groups(&self) -> TodGroups {
        self.groups
    }

    /// The character light rig every character takes during the shot, if the track sets one.
    pub fn light_rig(&self) -> Option<&LightRig> {
        self.light_rig.as_ref()
    }

    /// The weather state the shot shows over the world's, by name, if the track sets one.
    pub fn weather(&self) -> Option<&str> {
        self.weather.as_deref()
    }

    /// How strongly the track overrides at `seconds` into the shot: 0 before and after the
    /// shot, easing smoothly up over `blend_in` and down over `blend_out`, 1 between.
    pub fn weight(&self, seconds: f32) -> f32 {
        if !(0.0..=self.duration).contains(&seconds) {
            return 0.0;
        }
        let ramp = |elapsed: f32, length: f32| if length > 0.0 { elapsed / length } else { 1.0 };
        let fade = ramp(seconds, self.blend_in).min(ramp(self.duration - seconds, self.blend_out));
        Ease::Smooth.apply(fade)
    }

    /// The time of day the shot shows at `seconds` into it, if the track sets one.
    pub fn hours(&self, seconds: f32) -> Option<f32> {
        let time = self.time?;
        let hours = time.hours + time.hours_per_second * seconds.clamp(0.0, self.duration);
        // `rem_euclid` can round up to 24 itself for a time just short of midnight going back.
        Some(if hours.is_finite() { hours.rem_euclid(DAY_HOURS) % DAY_HOURS } else { time.hours })
    }

    /// Blends the value keys at `seconds` over `look` with `weight`.
    pub(crate) fn apply_values(&self, look: &mut TodLook, seconds: f32, weight: f32) {
        if self.keys.is_empty() {
            return;
        }
        let at = segment(&self.keys, seconds, None);
        look.blend_keys(&self.keys[at.from], &self.keys[at.to], at.weight, weight);
    }
}
