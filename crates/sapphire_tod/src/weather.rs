//! Weather states (M6 W8a; m6-plan §3, "One wind and one weather state"): the `WeatherDocument`, a
//! library of named states, and the schedule a zone picks them by.
//!
//! A **state** is a partial day plus the groups time of day has no word for:
//!
//! - **`look`**: a [`TodTimeline`] that keys only the groups the state changes (the clouds' cover and
//!   colours, the sky, the fog, ambient, the sun's light, exposure, the grade…), at as many hours as
//!   it likes, so overcast at dawn needn't be overcast at noon. Time of day blends it over the zone's
//!   day like any partial day, before atmosphere volumes ([`WeatherDocument::apply_looks`]). It may
//!   not key `weather` (the wet shares): those accumulate toward the state's `accumulation`.
//! - **The weather's own groups**, each optional: `precipitation`, `wind` (over the zone's),
//!   `lightning` (a rate), `accumulation` (targets and rises), `temperature`, `sea`, `audio` and
//!   `environment` (V's groups). They resolve into `sapphire_look`'s [`WeatherFrame`]
//!   ([`WeatherDocument::frame`]).
//!
//! At most two states are live at once, as a [`WeatherBlend`]: the outgoing one and the incoming one
//! by its weight. A change takes the document's `transitions` seconds, of world time, so an
//! accelerated clock changes weather faster. The world's `WeatherSystem` chooses and blends; the
//! [`WeatherSchedule`] it chooses by is a pure function of the day and the hour (a hash of the seed,
//! the day and the slot, with no floating point in the choice), so the same clock always shows the
//! same weather and nothing needs saving.
//!
//! ```ron
//! (
//!     type: "sapphire::world::WeatherDocument",
//!     version: 1,
//!     data: (
//!         states: [
//!             (name: "clear"),
//!             (
//!                 name: "rain",
//!                 look: Some((keys: [(time: 12.0, clouds: Some((lit: (0.6, 0.62, 0.66), shade: (0.3, 0.32, 0.36), coverage: 0.9, cirrus: 0.4)))])),
//!                 precipitation: Some((kind: Rain, density: 0.7)),
//!                 wind: Some((strength: 1.6, gustiness: 0.5, turbulence: 0.2)),
//!                 accumulation: Some((wetness: (target: 1.0, rise: 30.0), puddles: (target: 0.6, rise: 300.0))),
//!             ),
//!         ],
//!         transitions: (seconds: 30.0, pairs: [(from: "clear", to: "rain", seconds: 45.0)]),
//!     ),
//! )
//! ```

use bevy_reflect::TypePath;
use sapphire_look::{
    Accumulation, Precipitation, PrecipitationKind, ShareTarget, Temperature, WeatherAudio,
    WeatherEnvironment, WeatherFrame, WeatherWind,
};
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::{DAY_HOURS, TodGroup, TodGroups, TodLook, TodTimeline};

/// The most states a document may hold.
pub const MAX_STATES: usize = 64;
/// The longest a state's name may be, in bytes.
pub const MAX_NAME: usize = 64;
/// The longest a transition may take, in seconds of world time.
pub const MAX_TRANSITION: f32 = 600.0;
/// The most a state may multiply the zone's wind by.
pub const MAX_WIND_SCALE: f32 = 8.0;
/// The most a state may scale a kind's size or fall speed by.
pub const MAX_PRECIPITATION_SCALE: f32 = 8.0;
/// The most strikes a minute a state may average.
pub const MAX_LIGHTNING: f32 = 60.0;
/// The longest a share may take to rise, in seconds of world time: a day.
pub const MAX_RISE: f32 = 86_400.0;
/// The most a state may thicken the fog volumes.
pub const MAX_FOG_DENSITY: f32 = 16.0;
/// The most slots a schedule's day may hold, and choices a slot.
pub const MAX_SLOTS: usize = 48;
/// See [`MAX_SLOTS`].
pub const MAX_CHOICES: usize = MAX_STATES;

/// A document, a state or a schedule that isn't valid, by the field that isn't.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("`{field}` {problem}")]
pub struct WeatherError {
    pub field: String,
    pub problem: String,
}

impl WeatherError {
    pub fn new(field: impl Into<String>, problem: impl Into<String>) -> Self {
        Self { field: field.into(), problem: problem.into() }
    }
}

/// A library of weather states and how long changes between them take (a `WeatherDocument`, schema 1;
/// see the [module docs](self)). Valid however it was made, deserialization included: names unique and
/// in range, every value in range, every pair naming two of its states.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::world"]
#[serde(try_from = "UncheckedWeather")]
pub struct WeatherDocument {
    states: Vec<WeatherState>,
    #[serde(skip_serializing_if = "Transitions::is_default")]
    transitions: Transitions,
}

impl Versioned for WeatherDocument {
    const VERSION: u32 = 1;
}

/// A document as written, before [`WeatherDocument::new`] checks it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedWeather {
    states: Vec<WeatherState>,
    #[serde(default)]
    transitions: Transitions,
}

impl TryFrom<UncheckedWeather> for WeatherDocument {
    type Error = WeatherError;

    fn try_from(document: UncheckedWeather) -> Result<Self, WeatherError> {
        Self::new(document.states, document.transitions)
    }
}

/// One named state (see the [module docs](self)). A group it leaves out changes nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherState {
    /// Unique in its document: how a zone, a story flag, a cutscene or a tool names it.
    pub name: String,
    /// Its partial day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<TodTimeline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precipitation: Option<PrecipitationKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind: Option<WeatherWindKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightning: Option<LightningKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accumulation: Option<AccumulationKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<TemperatureKey>,
    /// The sea's state, 0 (calm) to 1 (a storm's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sea: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<WeatherAudioKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<WeatherEnvironmentKey>,
}

/// What falls ([`PrecipitationKind`]), as documents spell it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrecipitationKindKey {
    #[default]
    Rain,
    Snow,
    Hail,
    Dust,
}

impl From<PrecipitationKindKey> for PrecipitationKind {
    fn from(kind: PrecipitationKindKey) -> Self {
        match kind {
            PrecipitationKindKey::Rain => Self::Rain,
            PrecipitationKindKey::Snow => Self::Snow,
            PrecipitationKindKey::Hail => Self::Hail,
            PrecipitationKindKey::Dust => Self::Dust,
        }
    }
}

/// A state's precipitation ([`Precipitation`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrecipitationKey {
    pub kind: PrecipitationKindKey,
    /// 0 to 1.
    pub density: f32,
    /// Times the kind's size, 1 by default.
    #[serde(default = "one")]
    pub size: f32,
    /// Times the kind's fall speed, 1 by default.
    #[serde(default = "one")]
    pub speed: f32,
}

impl From<PrecipitationKey> for Precipitation {
    fn from(key: PrecipitationKey) -> Self {
        Self { kind: key.kind.into(), density: key.density, size: key.size, speed: key.speed }
    }
}

/// A state's wind over the zone's ([`WeatherWind`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherWindKey {
    /// Times the zone's strength.
    pub strength: f32,
    #[serde(default)]
    pub gustiness: f32,
    #[serde(default)]
    pub turbulence: f32,
}

impl From<WeatherWindKey> for WeatherWind {
    fn from(key: WeatherWindKey) -> Self {
        Self { strength: key.strength, gustiness: key.gustiness, turbulence: key.turbulence }
    }
}

/// A state's lightning: how often it strikes, on average (W8b draws the strikes).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightningKey {
    pub per_minute: f32,
}

/// One share's target and rise ([`ShareTarget`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareKey {
    /// 0 to 1.
    pub target: f32,
    /// Seconds of world time to rise about 63% of the way to the target.
    pub rise: f32,
}

impl From<ShareKey> for ShareTarget {
    fn from(key: ShareKey) -> Self {
        Self { target: key.target, rise: key.rise }
    }
}

/// What surfaces accumulate toward under a state ([`Accumulation`]): each share it names. A share
/// left out goes to 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccumulationKey {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wetness: Option<ShareKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub puddles: Option<ShareKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snow: Option<ShareKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sand: Option<ShareKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ice: Option<ShareKey>,
}

impl AccumulationKey {
    fn shares(&self) -> [(&'static str, Option<ShareKey>); 5] {
        [
            ("wetness", self.wetness),
            ("puddles", self.puddles),
            ("snow", self.snow),
            ("sand", self.sand),
            ("ice", self.ice),
        ]
    }
}

impl From<AccumulationKey> for Accumulation {
    fn from(key: AccumulationKey) -> Self {
        let share = |share: Option<ShareKey>| share.map_or_else(ShareTarget::default, ShareTarget::from);
        Self {
            wetness: share(key.wetness),
            puddles: share(key.puddles),
            snow: share(key.snow),
            sand: share(key.sand),
            ice: share(key.ice),
        }
    }
}

/// How cold a state is ([`Temperature`]), as documents spell it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TemperatureKey {
    Warm,
    Mild,
    Cold,
    Freezing,
}

impl From<TemperatureKey> for Temperature {
    fn from(key: TemperatureKey) -> Self {
        match key {
            TemperatureKey::Warm => Self::Warm,
            TemperatureKey::Mild => Self::Mild,
            TemperatureKey::Cold => Self::Cold,
            TemperatureKey::Freezing => Self::Freezing,
        }
    }
}

/// The audio's parameters under a state ([`WeatherAudio`]), each 0 to 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeatherAudioKey {
    pub precipitation: f32,
    pub wind: f32,
    pub thunder: f32,
}

impl From<WeatherAudioKey> for WeatherAudio {
    fn from(key: WeatherAudioKey) -> Self {
        Self { precipitation: key.precipitation, wind: key.wind, thunder: key.thunder }
    }
}

/// The environment's groups under a state ([`WeatherEnvironment`]); each left out is nothing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeatherEnvironmentKey {
    pub frost: f32,
    pub rings: f32,
    pub aurora: f32,
    pub fog_density: f32,
}

impl Default for WeatherEnvironmentKey {
    fn default() -> Self {
        let neutral = WeatherEnvironment::default();
        Self {
            frost: neutral.frost,
            rings: neutral.rings,
            aurora: neutral.aurora,
            fog_density: neutral.fog_density,
        }
    }
}

impl From<WeatherEnvironmentKey> for WeatherEnvironment {
    fn from(key: WeatherEnvironmentKey) -> Self {
        Self { frost: key.frost, rings: key.rings, aurora: key.aurora, fog_density: key.fog_density }
    }
}

/// How long changes between states take, in seconds of world time: `seconds` unless a pair names its
/// own, either way between its two states.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transitions {
    #[serde(default = "default_seconds")]
    pub seconds: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<TransitionPair>,
}

/// Half a minute: long enough to read as weather coming in, short enough to see it come.
fn default_seconds() -> f32 {
    30.0
}

impl Default for Transitions {
    fn default() -> Self {
        Self { seconds: default_seconds(), pairs: Vec::new() }
    }
}

impl Transitions {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A transition's own length between two states, either way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionPair {
    pub from: String,
    pub to: String,
    pub seconds: f32,
}

fn one() -> f32 {
    1.0
}

/// At most two live states (see the [module docs](self)): `to`, incoming at `weight`, over `from`, outgoing
/// at the rest, or over no state at all (the day as it is) with no `from`. One state alone is `from == to`, or
/// a weight of 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherBlend {
    pub from: Option<usize>,
    pub to: usize,
    /// The incoming state's weight, 0 to 1.
    pub weight: f32,
}

impl WeatherBlend {
    /// `state` alone.
    #[must_use]
    pub const fn settled(state: usize) -> Self {
        Self { from: Some(state), to: state, weight: 1.0 }
    }

    /// The weight held to 0 to 1, a NaN counting as 0.
    #[must_use]
    pub fn weight(&self) -> f32 {
        if self.weight > 0.0 { self.weight.min(1.0) } else { 0.0 }
    }

    /// The heavier state, the incoming one from an even split: none while no state outweighs the day.
    #[must_use]
    pub fn dominant(&self) -> Option<usize> {
        if self.weight() >= 0.5 { Some(self.to) } else { self.from }
    }

    /// The live states with their weights, the outgoing first: one entry with one state live, none with
    /// no state at any weight.
    pub fn live(&self) -> impl Iterator<Item = (usize, f32)> + use<> {
        let weight = self.weight();
        let to = self.to;
        let states: [(usize, f32); 2] = match self.from {
            None => [(to, weight), (to, 0.0)],
            Some(from) if from == to || weight >= 1.0 => [(to, 1.0), (to, 0.0)],
            Some(from) if weight <= 0.0 => [(from, 1.0), (from, 0.0)],
            Some(from) => [(from, 1.0 - weight), (to, weight)],
        };
        states.into_iter().filter(|(_, weight)| *weight > 0.0)
    }
}

impl WeatherDocument {
    /// A document of `states` and `transitions`, checked (see [`WeatherDocument`]).
    pub fn new(states: Vec<WeatherState>, transitions: Transitions) -> Result<Self, WeatherError> {
        if states.is_empty() {
            return Err(WeatherError::new("states", "there are none"));
        }
        if states.len() > MAX_STATES {
            return Err(WeatherError::new("states", format!("{} is more than {MAX_STATES}", states.len())));
        }
        for (index, state) in states.iter().enumerate() {
            check_name(&state.name)
                .map_err(|problem| WeatherError::new(format!("states[{index}].name"), problem))?;
            if states[..index].iter().any(|other| other.name == state.name) {
                let problem = format!("`{}` names another state too", state.name);
                return Err(WeatherError::new(format!("states[{index}].name"), problem));
            }
            state
                .check()
                .map_err(|(field, problem)| WeatherError::new(format!("states[{index}].{field}"), problem))?;
        }
        check_seconds(transitions.seconds)
            .map_err(|problem| WeatherError::new("transitions.seconds", problem))?;
        let document = Self { states, transitions };
        for (index, pair) in document.transitions.pairs.iter().enumerate() {
            let field = |name: &str| format!("transitions.pairs[{index}].{name}");
            for (name, state) in [("from", &pair.from), ("to", &pair.to)] {
                if document.state(state).is_none() {
                    return Err(WeatherError::new(field(name), format!("`{state}` isn't one of the states")));
                }
            }
            check_seconds(pair.seconds).map_err(|problem| WeatherError::new(field("seconds"), problem))?;
        }
        Ok(document)
    }

    /// Reads a weather document, upgrading older schema versions and checking it.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// The states, in the document's order: what a [`WeatherBlend`] and a [`WeatherSchedule`] index.
    pub fn states(&self) -> &[WeatherState] {
        &self.states
    }

    pub fn transitions(&self) -> &Transitions {
        &self.transitions
    }

    /// The state named `name`, by its index. Allocates nothing.
    pub fn state(&self, name: &str) -> Option<usize> {
        self.states.iter().position(|state| state.name == name)
    }

    /// The seconds of world time a change from `from` to `to` takes: their pair's, or the document's.
    pub fn seconds(&self, from: usize, to: usize) -> f32 {
        let (Some(from), Some(to)) = (self.states.get(from), self.states.get(to)) else {
            return self.transitions.seconds;
        };
        self.transitions
            .pairs
            .iter()
            .find(|pair| {
                (pair.from == from.name && pair.to == to.name)
                    || (pair.from == to.name && pair.to == from.name)
            })
            .map_or(self.transitions.seconds, |pair| pair.seconds)
    }

    /// Lays `blend`'s partial days at `hours` over `look`: the outgoing state's groups by `1 - weight`,
    /// those the incoming state keys too whole beneath it, then the incoming state's by `weight`. So a
    /// group both key reads `(1 - weight) × outgoing + weight × incoming`, the look beneath never showing
    /// through a transition, and a group only one keys fades in or out over what's beneath. Allocates
    /// nothing.
    pub fn apply_looks(&self, blend: WeatherBlend, look: &mut TodLook, hours: f32) {
        let mut live = blend.live().filter(|(state, _)| *state < self.states.len());
        let first = live.next();
        let second = live.next();
        match (first, second) {
            (Some((from, outgoing)), Some((to, incoming))) => {
                let over = self.states[to].look.as_ref().map_or(TodGroups::NONE, TodTimeline::groups);
                if let Some(day) = &self.states[from].look {
                    day.apply_by(
                        look,
                        hours,
                        |group: TodGroup| if over.contains(group) { 1.0 } else { outgoing },
                    );
                }
                if let Some(day) = &self.states[to].look {
                    day.apply(look, hours, incoming);
                }
            }
            (Some((state, weight)), None) => {
                if let Some(day) = &self.states[state].look {
                    day.apply(look, hours, weight);
                }
            }
            _ => {}
        }
    }

    /// The weather-only half of `blend` ([`WeatherFrame`], whose docs say how each group blends).
    /// Allocates nothing.
    pub fn frame(&self, blend: WeatherBlend) -> WeatherFrame {
        self.frame_over(Some(blend), None)
    }

    /// The weather-only half of the world's `blend` with a cutscene's `shot` state laid over it at its
    /// weight (M6 W8a): the world's states each weigh `1 - weight` of what they did and the shot's state
    /// `weight`, so the world's whole blend fades under the shot, never cut, and three states are live while
    /// a shot eases in or out over a transition. Allocates nothing.
    pub fn frame_over(&self, blend: Option<WeatherBlend>, shot: Option<(usize, f32)>) -> WeatherFrame {
        let mut live = [(0, 0.0); 3];
        let mut count = 0;
        let shot = shot.map(|(state, weight)| (state, if weight > 0.0 { weight.min(1.0) } else { 0.0 }));
        let under = 1.0 - shot.map_or(0.0, |(_, weight)| weight);
        for (state, weight) in blend.into_iter().flat_map(|blend| blend.live()) {
            live[count] = (state, weight * under);
            count += 1;
        }
        if let Some((state, weight)) = shot.filter(|(_, weight)| *weight > 0.0) {
            match live[..count].iter_mut().find(|(live, _)| *live == state) {
                Some(same) => same.1 += weight,
                None => {
                    live[count] = (state, weight);
                    count += 1;
                }
            }
        }
        let live = &live[..count];
        let mut frame = self.frame_of(live.iter().copied().filter(|(_, weight)| *weight > 0.0));
        if frame.live > 0 {
            frame.transition = match blend {
                Some(blend) if blend.from.is_none() || blend.live().count() == 2 => blend.weight(),
                _ => 1.0,
            };
        }
        // The heaviest state's, the later from an even split; none while the day as it is outweighs them.
        let day = 1.0 - live.iter().map(|(_, weight)| weight).sum::<f32>();
        let (dominant, _) = live.iter().fold((None, day), |(heaviest, most), &(state, weight)| {
            if weight > 0.0 && weight >= most { (Some(state), weight) } else { (heaviest, most) }
        });
        if let Some(dominant) = dominant.and_then(|state| self.states.get(state)) {
            frame.accumulation = dominant.accumulation.map(Accumulation::from);
            frame.temperature = dominant.temperature.map(Temperature::from);
        }
        frame
    }

    /// The states `live` at their weights, but for the dominant state's groups and the transition.
    fn frame_of(&self, live: impl Iterator<Item = (usize, f32)>) -> WeatherFrame {
        let mut frame = WeatherFrame::default();
        let mut wind = WeatherWind { strength: 0.0, gustiness: 0.0, turbulence: 0.0 };
        let mut environment = WeatherEnvironment { fog_density: 0.0, ..WeatherEnvironment::default() };
        let mut layers = 0;
        let mut total = 0.0;
        for (index, weight) in live {
            let Some(state) = self.states.get(index) else { continue };
            frame.live += 1;
            total += weight;
            if let Some(key) = state.wind {
                wind.strength += key.strength * weight;
                wind.gustiness += key.gustiness * weight;
                wind.turbulence += key.turbulence * weight;
                frame.wind_share += weight;
            }
            frame.lightning += state.lightning.map_or(0.0, |lightning| lightning.per_minute) * weight;
            frame.sea += state.sea.unwrap_or(0.0) * weight;
            let audio = state.audio.unwrap_or_default();
            frame.audio.precipitation += audio.precipitation * weight;
            frame.audio.wind += audio.wind * weight;
            frame.audio.thunder += audio.thunder * weight;
            let keyed = state.environment.unwrap_or_default();
            environment.frost += keyed.frost * weight;
            environment.rings += keyed.rings * weight;
            environment.aurora += keyed.aurora * weight;
            environment.fog_density += keyed.fog_density * weight;
            if let Some(key) = state.precipitation {
                let falling = Precipitation { density: key.density * weight, ..key.into() };
                let same = frame.precipitation[..layers]
                    .iter_mut()
                    .flatten()
                    .find(|layer| layer.kind == falling.kind);
                match same {
                    // The same kind in both: one layer, its size and speed by the two's weights.
                    Some(layer) => {
                        let total = layer.density + falling.density;
                        if total > 0.0 {
                            let share = falling.density / total;
                            layer.size += (falling.size - layer.size) * share;
                            layer.speed += (falling.speed - layer.speed) * share;
                        }
                        layer.density = total;
                    }
                    None => {
                        frame.precipitation[layers] = Some(falling);
                        layers += 1;
                    }
                }
            }
        }
        if frame.wind_share > 0.0 {
            let share = frame.wind_share;
            frame.wind = WeatherWind {
                strength: wind.strength / share,
                gustiness: wind.gustiness / share,
                turbulence: wind.turbulence / share,
            };
        }
        if frame.live > 0 {
            // What no state weighs is the day's: the environment's neutral values.
            let neutral = WeatherEnvironment::default();
            let rest = (1.0 - total).max(0.0);
            environment.frost += neutral.frost * rest;
            environment.rings += neutral.rings * rest;
            environment.aurora += neutral.aurora * rest;
            environment.fog_density += neutral.fog_density * rest;
            frame.environment = environment;
        }
        frame
    }
}

impl WeatherState {
    /// Checks every value the state sets is in range. Returns the field and what's wrong.
    fn check(&self) -> Result<(), (String, String)> {
        let field = |name: &str| name.to_owned();
        if let Some(look) = &self.look
            && look.groups().contains(TodGroup::Weather)
        {
            let problem = "keys `weather`: a state's wet shares accumulate toward its `accumulation`";
            return Err((field("look"), problem.to_owned()));
        }
        if let Some(falling) = self.precipitation {
            share(falling.density).map_err(|problem| (field("precipitation.density"), problem))?;
            scale(falling.size, MAX_PRECIPITATION_SCALE)
                .map_err(|problem| (field("precipitation.size"), problem))?;
            scale(falling.speed, MAX_PRECIPITATION_SCALE)
                .map_err(|problem| (field("precipitation.speed"), problem))?;
        }
        if let Some(wind) = self.wind {
            range(wind.strength, 0.0, MAX_WIND_SCALE).map_err(|problem| (field("wind.strength"), problem))?;
            share(wind.gustiness).map_err(|problem| (field("wind.gustiness"), problem))?;
            share(wind.turbulence).map_err(|problem| (field("wind.turbulence"), problem))?;
        }
        if let Some(lightning) = self.lightning {
            range(lightning.per_minute, 0.0, MAX_LIGHTNING)
                .map_err(|problem| (field("lightning.per_minute"), problem))?;
        }
        if let Some(accumulation) = self.accumulation {
            for (name, key) in accumulation.shares() {
                if let Some(key) = key {
                    share(key.target).map_err(|problem| (format!("accumulation.{name}.target"), problem))?;
                    if !(key.rise.is_finite() && key.rise > 0.0 && key.rise <= MAX_RISE) {
                        let problem = format!("{} s isn't above 0 and at most {MAX_RISE} s", key.rise);
                        return Err((format!("accumulation.{name}.rise"), problem));
                    }
                }
            }
        }
        if let Some(sea) = self.sea {
            share(sea).map_err(|problem| (field("sea"), problem))?;
        }
        if let Some(audio) = self.audio {
            for (name, value) in
                [("precipitation", audio.precipitation), ("wind", audio.wind), ("thunder", audio.thunder)]
            {
                share(value).map_err(|problem| (format!("audio.{name}"), problem))?;
            }
        }
        if let Some(environment) = self.environment {
            for (name, value) in
                [("frost", environment.frost), ("rings", environment.rings), ("aurora", environment.aurora)]
            {
                share(value).map_err(|problem| (format!("environment.{name}"), problem))?;
            }
            range(environment.fog_density, 0.0, MAX_FOG_DENSITY)
                .map_err(|problem| (field("environment.fog_density"), problem))?;
        }
        Ok(())
    }
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_NAME || name.trim() != name {
        return Err(format!("{name:?} isn't 1 to {MAX_NAME} bytes with no space at either end"));
    }
    Ok(())
}

fn check_seconds(seconds: f32) -> Result<(), String> {
    range(seconds, 0.0, MAX_TRANSITION)
}

fn range(value: f32, low: f32, high: f32) -> Result<(), String> {
    if value.is_finite() && (low..=high).contains(&value) {
        Ok(())
    } else {
        Err(format!("{value} isn't from {low} to {high}"))
    }
}

fn share(value: f32) -> Result<(), String> {
    range(value, 0.0, 1.0)
}

fn scale(value: f32, high: f32) -> Result<(), String> {
    if value.is_finite() && value > 0.0 && value <= high {
        Ok(())
    } else {
        Err(format!("{value} isn't above 0 and at most {high}"))
    }
}

/// When a zone's weather is which state (see the [module docs](self)): the day in slots, each from its
/// hour to the next slot's (the last to the first's, wrapping at midnight), each a weighted choice of
/// states. Which state a slot shows on a day is `hash(seed, day, slot) mod` the slot's total weight,
/// integer maths alone, so a day's weather is the same on every machine, at any frame rate and after
/// any jump of the clock, and a game could move it into the simulation unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeatherSchedule {
    seed: u32,
    slots: Vec<ScheduleSlot>,
}

/// One slot of a [`WeatherSchedule`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleSlot {
    /// Its first hour, in hundredths, `0 ≤ from < 2400`.
    from: u32,
    /// States by index, with their weights.
    choices: Vec<(usize, u32)>,
    total: u64,
}

impl WeatherSchedule {
    /// A schedule of `slots`, each its first hour and its choices of state (by index, among `states`)
    /// with integer weights, checked: at least one slot, hours within the day and increasing, every
    /// slot a choice, every weight above 0.
    pub fn new(seed: u32, slots: Vec<(f32, Vec<(usize, u32)>)>, states: usize) -> Result<Self, WeatherError> {
        if slots.is_empty() || slots.len() > MAX_SLOTS {
            return Err(WeatherError::new(
                "schedule",
                format!("{} slots isn't 1 to {MAX_SLOTS}", slots.len()),
            ));
        }
        let mut checked: Vec<ScheduleSlot> = Vec::with_capacity(slots.len());
        for (index, (from, choices)) in slots.into_iter().enumerate() {
            let field = |name: &str| format!("schedule[{index}].{name}");
            if !(from.is_finite() && (0.0..DAY_HOURS).contains(&from)) {
                return Err(WeatherError::new(field("from"), format!("{from} isn't 0 ≤ hours < 24")));
            }
            // Hundredths of an hour: whole, so where a slot starts compares exactly.
            let from = ((from * 100.0).round() as u32).min(2399);
            if checked.last().is_some_and(|last| last.from >= from) {
                return Err(WeatherError::new(
                    field("from"),
                    "hours must increase from one slot to the next",
                ));
            }
            if choices.is_empty() || choices.len() > MAX_CHOICES {
                return Err(WeatherError::new(
                    field("states"),
                    format!("{} isn't 1 to {MAX_CHOICES}", choices.len()),
                ));
            }
            for (choice, &(state, weight)) in choices.iter().enumerate() {
                if state >= states {
                    return Err(WeatherError::new(format!("{}[{choice}]", field("states")), "isn't a state"));
                }
                if weight == 0 {
                    return Err(WeatherError::new(
                        format!("{}[{choice}]", field("states")),
                        "weighs nothing",
                    ));
                }
            }
            let total = choices.iter().map(|&(_, weight)| u64::from(weight)).sum();
            checked.push(ScheduleSlot { from, choices, total });
        }
        Ok(Self { seed, slots: checked })
    }

    /// Every day the same `state`.
    #[must_use]
    pub fn always(state: usize) -> Self {
        Self { seed: 0, slots: vec![ScheduleSlot { from: 0, choices: vec![(state, 1)], total: 1 }] }
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// The slot `hours` of day `day` falls in, and the day that slot began on: the day before for an
    /// hour before the first slot, which belongs to the last slot of the night before.
    pub fn slot_at(&self, day: i64, hours: f32) -> (i64, usize) {
        let hundredths = if hours.is_finite() { (hours.rem_euclid(DAY_HOURS) * 100.0) as u32 } else { 0 };
        match self.slots.partition_point(|slot| slot.from <= hundredths) {
            0 => (day - 1, self.slots.len() - 1),
            next => (day, next - 1),
        }
    }

    /// The state slot `slot` shows on day `day`. Allocates nothing.
    pub fn pick(&self, day: i64, slot: usize) -> usize {
        let Some(slot_choices) = self.slots.get(slot) else { return 0 };
        let mut roll = u64::from(hash(self.seed, day, slot as u32)) % slot_choices.total;
        for &(state, weight) in &slot_choices.choices {
            if roll < u64::from(weight) {
                return state;
            }
            roll -= u64::from(weight);
        }
        slot_choices.choices[0].0
    }

    /// The state at `hours` of day `day`: a pure function of the two.
    pub fn state_at(&self, day: i64, hours: f32) -> usize {
        let (day, slot) = self.slot_at(day, hours);
        self.pick(day, slot)
    }
}

/// A 32-bit hash of the seed, the day and the slot: each word folded in by a multiply and the finalizer
/// of MurmurHash3, integer operations alone.
fn hash(seed: u32, day: i64, slot: u32) -> u32 {
    fn fold(state: u32, word: u32) -> u32 {
        let mut x = state ^ word.wrapping_mul(0x9e37_79b1);
        x ^= x >> 16;
        x = x.wrapping_mul(0x85eb_ca6b);
        x ^= x >> 13;
        x = x.wrapping_mul(0xc2b2_ae35);
        x ^ (x >> 16)
    }
    let day = day as u64;
    [day as u32, (day >> 32) as u32, slot].into_iter().fold(fold(0x5eed_0001, seed), fold)
}

#[cfg(test)]
mod tests;
