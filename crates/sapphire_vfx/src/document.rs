//! Effects as documents: an [`EffectLibrary`] (schema 1), named [`Effect`]s of emitters with modules
//! and curves and at most one trail, checked whole however they were made, so what plays is valid
//! and an author's mistake names its field.

use std::collections::BTreeSet;

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use glam::Vec3;
use sapphire_look::{VfxLight, VfxShape};
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::curve::Curve;

/// The shipped effects (see [`EffectLibrary::default_effects`]).
const DEFAULT_EFFECTS: &str = include_str!("../default_effects.ron");

/// The most emitters an effect has: each playing effect keeps this many emission states inline, so
/// a spawn never allocates.
pub const MAX_EMITTERS: usize = 8;

/// How many socket samples a trail keeps at most, whatever its document asks: 48 is a fifth of a
/// second at 240 samples a second.
pub const MAX_TRAIL_SAMPLES: usize = 64;

/// The most particles a burst fires at once.
const MAX_BURST: u32 = 4096;

/// The most particles a rate fires a second: a burst's worth every tick at 60 Hz, which fills the
/// largest budget in one. Past this it's a typo.
pub(crate) const MAX_RATE: f32 = 245_760.0;

/// An inclusive range a particle draws from at birth: a document writes `(low, high)`, and equal
/// ends fix it.
pub type Range = (f32, f32);

/// When an emitter's particles appear.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum Spawn {
    /// `count` all at once.
    Burst { count: u32 },
    /// `per_second`, at most 245,760, for `duration` seconds, or until the effect is
    /// [stopped](crate::VfxWorld::stop) where `duration` is 0.
    Rate { per_second: f32, duration: f32 },
}

/// Where in the effect's space its particles are born. Fixed in the world from then on.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum Source {
    /// At the effect's position.
    #[default]
    Point,
    /// Anywhere within a ball of this radius.
    Sphere(f32),
    /// Anywhere on a level disc of this radius: dust at the feet.
    Disc(f32),
}

/// How an emitter's particles move.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Motion {
    /// Meters per second at birth.
    pub speed: Range,
    /// The direction they leave in, in the effect's space (normalized at spawn). The shipped hit
    /// spark leaves along +Y, so a spawn's rotation turns that away from the surface struck.
    #[serde(default = "up")]
    pub direction: [f32; 3],
    /// How far from `direction` they may leave, degrees: 180 is any way at all.
    #[serde(default)]
    pub spread: f32,
    /// Meters per second² downward along the world's -Y; negative rises.
    #[serde(default)]
    pub gravity: f32,
    /// The share of their speed they lose per second, as `e^(-drag t)`.
    #[serde(default)]
    pub drag: f32,
}

fn up() -> [f32; 3] {
    [0.0, 1.0, 0.0]
}

impl Default for Motion {
    fn default() -> Self {
        Self { speed: (0.0, 0.0), direction: up(), spread: 0.0, gravity: 0.0, drag: 0.0 }
    }
}

/// How an emitter's quads turn.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum Facing {
    /// Toward the camera, turning at `spin` radians per second, each from a random angle.
    Camera { spin: Range },
    /// Lengthwise along their velocity: half their length is their size plus `stretch` seconds of
    /// their speed.
    Velocity { stretch: f32 },
}

impl Default for Facing {
    fn default() -> Self {
        Self::Camera { spin: (0.0, 0.0) }
    }
}

/// A quad's shape, by the generated map's layers ([`VfxShape`]).
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum Shape {
    #[default]
    SoftDot,
    Streak,
    Star,
    Ring,
}

impl From<Shape> for VfxShape {
    fn from(shape: Shape) -> Self {
        match shape {
            Shape::SoftDot => Self::SoftDot,
            Shape::Streak => Self::Streak,
            Shape::Star => Self::Star,
            Shape::Ring => Self::Ring,
        }
    }
}

/// Where a quad's light comes from ([`VfxLight`]).
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum Light {
    #[default]
    Emissive,
    Lit,
}

impl From<Light> for VfxLight {
    fn from(light: Light) -> Self {
        match light {
            Light::Emissive => Self::Emissive,
            Light::Lit => Self::Lit,
        }
    }
}

/// Over how many meters a quad fades into the surface behind it, where nothing else says.
const DEFAULT_SOFT: f32 = 0.02;

fn default_soft() -> f32 {
    DEFAULT_SOFT
}

fn one() -> Curve<f32> {
    Curve(vec![(0.0, 1.0)])
}

fn one_share() -> f32 {
    1.0
}

/// An emitter: its modules (when, from where, how they move, how they look) and its curves over a
/// particle's life.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitterDesc {
    pub name: String,
    pub spawn: Spawn,
    /// Seconds on the owner's clock before it starts.
    #[serde(default)]
    pub delay: f32,
    #[serde(default)]
    pub source: Source,
    /// How long each particle lives, seconds.
    pub lifetime: Range,
    #[serde(default)]
    pub motion: Motion,
    #[serde(default)]
    pub shape: Shape,
    #[serde(default)]
    pub facing: Facing,
    #[serde(default)]
    pub light: Light,
    /// 1 is pure light over what's behind, 0 a surface that covers it (`VfxSprite::additive`).
    #[serde(default = "one_share")]
    pub additive: f32,
    /// Over how many meters it fades into the surface behind it.
    #[serde(default = "default_soft")]
    pub soft: f32,
    /// A random factor on each particle's size.
    #[serde(default = "same_size")]
    pub size_scale: Range,
    /// Half its width in meters, over its life.
    pub size: Curve<f32>,
    /// Linear color over its life (relative to the display if emissive; an albedo if lit).
    pub color: Curve<[f32; 3]>,
    /// Its opacity over its life.
    #[serde(default = "one")]
    pub alpha: Curve<f32>,
}

fn same_size() -> Range {
    (1.0, 1.0)
}

/// A trail's crisp bright band across the strip (`VfxRibbonPoint::core`).
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrailCore {
    /// Where its middle sits across the strip: 0 at the base socket, 1 at the tip.
    pub position: f32,
    /// Half its width, as a share of the strip's.
    pub half_width: f32,
    /// Its color over a point's age.
    pub color: Curve<[f32; 3]>,
}

/// A sword trail: a strip between two sockets' paths, sampled by the caller
/// ([`VfxWorld::sample_trail`](crate::VfxWorld::sample_trail)), smoothed through the samples, whose
/// look follows each point's age.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrailDesc {
    /// Seconds on the owner's clock a point lasts.
    pub lifetime: f32,
    /// The samples it keeps, at most [`MAX_TRAIL_SAMPLES`]. A full trail overwrites its oldest.
    pub max_samples: u32,
    /// The strip's width over a point's age, as a share of the sockets' distance, narrowing toward
    /// `anchor`.
    #[serde(default = "one")]
    pub width: Curve<f32>,
    /// What the strip narrows toward: 0 the base socket, 1 the tip.
    #[serde(default = "one_share")]
    pub anchor: f32,
    /// The glow's color over a point's age.
    pub color: Curve<[f32; 3]>,
    /// The strip's opacity over a point's age. Must end at 0, or the tail pops as points retire.
    pub alpha: Curve<f32>,
    pub core: TrailCore,
    /// How far the glow reaches from the core, as a share of the strip's width.
    pub glow_reach: f32,
    /// As an emitter's.
    #[serde(default = "one_share")]
    pub additive: f32,
    /// As an emitter's.
    #[serde(default = "default_soft")]
    pub soft: f32,
    /// Seconds from the effect's start over which the strip grows from nothing, in width and
    /// opacity, so a swing's first stretch ends in a point, not the blade's straight edge, while
    /// its age hasn't faded it yet. 0 starts it whole.
    #[serde(default)]
    pub start_fade: f32,
}

/// A named effect: what a spawn plays.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effect {
    pub name: String,
    #[serde(default)]
    pub emitters: Vec<EmitterDesc>,
    #[serde(default)]
    pub trail: Option<TrailDesc>,
}

impl Effect {
    /// Whether it plays until it's [stopped](crate::VfxWorld::stop): a trail records until then, and a
    /// rate with no duration emits. Only what keeps its [handle](crate::EffectHandle) can start one,
    /// or it holds its place in the budget for good.
    #[must_use]
    pub fn is_endless(&self) -> bool {
        self.trail.is_some()
            || self
                .emitters
                .iter()
                .any(|emitter| matches!(emitter.spawn, Spawn::Rate { duration, .. } if duration <= 0.0))
    }
}

/// An effect of an [`EffectLibrary`], by its place in the library.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EffectId(pub u16);

/// A library that isn't valid.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum EffectLibraryError {
    #[error("there are no effects")]
    Empty,
    #[error("effect {0}: a name can't be empty")]
    EmptyName(usize),
    #[error("effect `{0}` appears twice")]
    DuplicateName(String),
    #[error("more effects than an effect ID can name")]
    TooMany,
    #[error("effect `{0}`: it has neither an emitter nor a trail")]
    Nothing(String),
    #[error("effect `{0}`: it has more than {MAX_EMITTERS} emitters")]
    TooManyEmitters(String),
    /// A field of an emitter or a trail: `part` says which.
    #[error("effect `{effect}`, {part}: `{field}` {problem}")]
    Invalid { effect: String, part: String, field: &'static str, problem: &'static str },
}

/// Named effects (an `EffectLibrary` document, schema 1).
///
/// A library has at least one effect, its names are distinct and not empty, and every emitter and
/// trail is valid: it is valid however it was made, deserialization included.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[type_path = "sapphire::vfx"]
#[serde(try_from = "UncheckedLibrary")]
pub struct EffectLibrary {
    effects: Vec<Effect>,
}

impl Versioned for EffectLibrary {
    const VERSION: u32 = 1;
}

/// A library as written, before [`EffectLibrary::new`] checks it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedLibrary {
    effects: Vec<Effect>,
}

impl TryFrom<UncheckedLibrary> for EffectLibrary {
    type Error = EffectLibraryError;

    fn try_from(library: UncheckedLibrary) -> Result<Self, EffectLibraryError> {
        Self::new(library.effects)
    }
}

impl EffectLibrary {
    /// A library of `effects`, checked.
    pub fn new(effects: Vec<Effect>) -> Result<Self, EffectLibraryError> {
        if effects.is_empty() {
            return Err(EffectLibraryError::Empty);
        }
        if effects.len() > usize::from(u16::MAX) + 1 {
            return Err(EffectLibraryError::TooMany);
        }
        let mut names = BTreeSet::new();
        for (index, effect) in effects.iter().enumerate() {
            if effect.name.is_empty() {
                return Err(EffectLibraryError::EmptyName(index));
            }
            if !names.insert(effect.name.as_str()) {
                return Err(EffectLibraryError::DuplicateName(effect.name.clone()));
            }
            if effect.emitters.is_empty() && effect.trail.is_none() {
                return Err(EffectLibraryError::Nothing(effect.name.clone()));
            }
            if effect.emitters.len() > MAX_EMITTERS {
                return Err(EffectLibraryError::TooManyEmitters(effect.name.clone()));
            }
            for (emitter_index, emitter) in effect.emitters.iter().enumerate() {
                emitter.problem().map_err(|(field, problem)| EffectLibraryError::Invalid {
                    effect: effect.name.clone(),
                    part: if emitter.name.is_empty() {
                        format!("emitter {emitter_index}")
                    } else {
                        format!("emitter `{}`", emitter.name)
                    },
                    field,
                    problem,
                })?;
            }
            if let Some(trail) = &effect.trail {
                trail.problem().map_err(|(field, problem)| EffectLibraryError::Invalid {
                    effect: effect.name.clone(),
                    part: "the trail".to_owned(),
                    field,
                    problem,
                })?;
            }
        }
        Ok(Self { effects })
    }

    /// Sapphire's sample effects (`default_effects.ron` beside this crate's manifest), compiled in:
    /// `hit_spark`, `sword_trail` and `dust_puff`, and what a fight's cues play (M2 E3): `cut_spark`,
    /// `break_ring`, `heavy_spark` and `finish_burst`, and what a fight's blades leave: `blade_trail` and
    /// `warrior_trail`. Their colors and curves are starting points for art direction.
    pub fn default_effects() -> Self {
        Self::from_ron(DEFAULT_EFFECTS).expect("the shipped effects are checked by this module's tests")
    }

    /// Reads a library document, upgrading older schema versions and checking it. A bad field fails
    /// here, named ([`EffectLibraryError::Invalid`]), with the document's line and column.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// The effect called `name`.
    pub fn find(&self, name: &str) -> Option<EffectId> {
        self.effects.iter().position(|effect| effect.name == name).map(|index| EffectId(index as u16))
    }

    /// The effect `id` names, if this library has one.
    pub fn effect(&self, id: EffectId) -> Option<&Effect> {
        self.effects.get(usize::from(id.0))
    }

    /// Its effects' names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.effects.iter().map(|effect| effect.name.as_str())
    }
}

/// Whether both ends are finite and in order.
fn range_ok((low, high): Range) -> bool {
    low.is_finite() && high.is_finite() && low <= high
}

fn share_ok(value: f32) -> bool {
    (0.0..=1.0).contains(&value)
}

type Problem = (&'static str, &'static str);

/// Fails with `field`'s name when `ok` doesn't hold.
fn require(ok: bool, field: &'static str, problem: &'static str) -> Result<(), Problem> {
    if ok { Ok(()) } else { Err((field, problem)) }
}

/// Fails with `field`'s name if the curve isn't valid.
fn curve_ok<T: crate::curve::CurveValue>(curve: &Curve<T>, field: &'static str) -> Result<(), Problem> {
    curve.problem().map_or(Ok(()), |problem| Err((field, problem)))
}

impl EmitterDesc {
    fn problem(&self) -> Result<(), Problem> {
        match self.spawn {
            Spawn::Burst { count } => {
                require(count >= 1, "spawn", "a burst must fire at least one")?;
                require(count <= MAX_BURST, "spawn", "a burst fires at most 4096")?;
            }
            Spawn::Rate { per_second, duration } => {
                require(per_second.is_finite() && per_second > 0.0, "spawn", "a rate must be above 0")?;
                require(per_second <= MAX_RATE, "spawn", "a rate fires at most 245760 a second")?;
                require(duration.is_finite() && duration >= 0.0, "spawn", "a duration can't be negative")?;
            }
        }
        require(self.delay.is_finite() && self.delay >= 0.0, "delay", "can't be negative")?;
        match self.source {
            Source::Point => {}
            Source::Sphere(radius) | Source::Disc(radius) => {
                require(radius.is_finite() && radius >= 0.0, "source", "a radius can't be negative")?;
            }
        }
        require(range_ok(self.lifetime), "lifetime", "must be (low, high), finite and in order")?;
        require(self.lifetime.0 > 0.0, "lifetime", "must be above 0")?;
        self.motion.problem()?;
        match self.facing {
            Facing::Camera { spin } => {
                require(range_ok(spin), "facing", "`spin` must be (low, high), finite and in order")?;
            }
            Facing::Velocity { stretch } => {
                require(stretch.is_finite() && stretch >= 0.0, "facing", "`stretch` can't be negative")?;
            }
        }
        require(share_ok(self.additive), "additive", "must be 0 to 1")?;
        require(self.soft.is_finite() && self.soft > 0.0, "soft", "must be above 0")?;
        require(range_ok(self.size_scale), "size_scale", "must be (low, high), finite and in order")?;
        require(self.size_scale.0 >= 0.0, "size_scale", "can't be negative")?;
        curve_ok(&self.size, "size")?;
        require(self.size.never_negative(), "size", "can't be negative")?;
        curve_ok(&self.color, "color")?;
        require(self.color.never_negative(), "color", "can't be negative")?;
        curve_ok(&self.alpha, "alpha")?;
        require(self.alpha.never_negative() && self.alpha.at_most_one(), "alpha", "must be 0 to 1")?;
        Ok(())
    }
}

impl Motion {
    fn problem(&self) -> Result<(), Problem> {
        require(range_ok(self.speed), "motion.speed", "must be (low, high), finite and in order")?;
        require(self.speed.0 >= 0.0, "motion.speed", "can't be negative")?;
        let direction = Vec3::from(self.direction);
        require(
            direction.is_finite() && direction.length_squared() > 1e-8,
            "motion.direction",
            "must be finite and not zero",
        )?;
        require((0.0..=180.0).contains(&self.spread), "motion.spread", "must be 0 to 180 degrees")?;
        require(self.gravity.is_finite(), "motion.gravity", "must be finite")?;
        require(self.drag.is_finite() && self.drag >= 0.0, "motion.drag", "can't be negative")?;
        Ok(())
    }
}

impl TrailDesc {
    fn problem(&self) -> Result<(), Problem> {
        require(self.lifetime.is_finite() && self.lifetime > 0.0, "lifetime", "must be above 0")?;
        require(
            (2..=MAX_TRAIL_SAMPLES as u32).contains(&self.max_samples),
            "max_samples",
            "must be 2 to 64",
        )?;
        curve_ok(&self.width, "width")?;
        require(self.width.never_negative(), "width", "can't be negative")?;
        require(share_ok(self.anchor), "anchor", "must be 0 to 1")?;
        curve_ok(&self.color, "color")?;
        require(self.color.never_negative(), "color", "can't be negative")?;
        curve_ok(&self.alpha, "alpha")?;
        require(self.alpha.never_negative() && self.alpha.at_most_one(), "alpha", "must be 0 to 1")?;
        require(
            self.alpha.0.last().is_some_and(|(_, value)| *value == 0.0),
            "alpha",
            "must end at 0, or the tail pops as points retire",
        )?;
        require(share_ok(self.core.position), "core.position", "must be 0 to 1")?;
        require(share_ok(self.core.half_width), "core.half_width", "must be 0 to 1")?;
        curve_ok(&self.core.color, "core.color")?;
        require(self.core.color.never_negative(), "core.color", "can't be negative")?;
        require(self.glow_reach.is_finite() && self.glow_reach > 0.0, "glow_reach", "must be above 0")?;
        require(self.start_fade.is_finite() && self.start_fade >= 0.0, "start_fade", "must be 0 or more")?;
        require(share_ok(self.additive), "additive", "must be 0 to 1")?;
        require(self.soft.is_finite() && self.soft > 0.0, "soft", "must be above 0")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emitter() -> EmitterDesc {
        EmitterDesc {
            name: "sparks".to_owned(),
            spawn: Spawn::Burst { count: 4 },
            delay: 0.0,
            source: Source::Point,
            lifetime: (0.1, 0.2),
            motion: Motion::default(),
            shape: Shape::Streak,
            facing: Facing::Velocity { stretch: 0.01 },
            light: Light::Emissive,
            additive: 1.0,
            soft: 0.02,
            size_scale: (1.0, 1.0),
            size: Curve(vec![(0.0, 0.05)]),
            color: Curve(vec![(0.0, [1.0, 1.0, 1.0])]),
            alpha: Curve(vec![(0.0, 1.0), (1.0, 0.0)]),
        }
    }

    fn trail() -> TrailDesc {
        TrailDesc {
            lifetime: 0.2,
            max_samples: 48,
            width: Curve(vec![(0.0, 1.0)]),
            anchor: 1.0,
            color: Curve(vec![(0.0, [0.2, 0.5, 1.0])]),
            alpha: Curve(vec![(0.0, 1.0), (1.0, 0.0)]),
            core: TrailCore { position: 0.9, half_width: 0.05, color: Curve(vec![(0.0, [4.0, 4.0, 4.0])]) },
            glow_reach: 0.8,
            start_fade: 0.0,
            additive: 0.8,
            soft: 0.02,
        }
    }

    fn library_of(emitter: EmitterDesc) -> Result<EffectLibrary, EffectLibraryError> {
        EffectLibrary::new(vec![Effect { name: "hit".to_owned(), emitters: vec![emitter], trail: None }])
    }

    /// The field an edit of a valid emitter makes the library name, with the emitter's name.
    fn emitter_error(edit: impl FnOnce(&mut EmitterDesc)) -> &'static str {
        let mut bad = emitter();
        edit(&mut bad);
        match library_of(bad) {
            Err(EffectLibraryError::Invalid { effect, part, field, .. }) => {
                assert_eq!(effect, "hit");
                assert_eq!(part, "emitter `sparks`");
                field
            }
            other => panic!("expected an invalid emitter, got {other:?}"),
        }
    }

    fn trail_error(edit: impl FnOnce(&mut TrailDesc)) -> &'static str {
        let mut bad = trail();
        edit(&mut bad);
        let effect = Effect { name: "swing".to_owned(), emitters: Vec::new(), trail: Some(bad) };
        match EffectLibrary::new(vec![effect]) {
            Err(EffectLibraryError::Invalid { effect, part, field, .. }) => {
                assert_eq!(effect, "swing");
                assert_eq!(part, "the trail");
                field
            }
            other => panic!("expected an invalid trail, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_names_keep_row_validation_precedence() {
        let first = Effect { name: "same".to_owned(), emitters: vec![emitter()], trail: None };
        let mut duplicate = first.clone();
        duplicate.emitters.clear();
        duplicate.trail = None;
        assert!(matches!(EffectLibrary::new(vec![first.clone(), duplicate]),
            Err(EffectLibraryError::DuplicateName(name)) if name == "same"));
        let mut invalid = first;
        invalid.emitters.clear();
        invalid.trail = None;
        assert!(matches!(EffectLibrary::new(vec![invalid.clone(), invalid]),
            Err(EffectLibraryError::Nothing(name)) if name == "same"));
    }

    #[test]
    fn the_shipped_effects_are_valid_and_named() {
        let library = EffectLibrary::default_effects();
        let names: Vec<&str> = library.names().collect();
        assert_eq!(
            names,
            [
                "hit_spark",
                "sword_trail",
                "dust_puff",
                "cut_spark",
                "break_ring",
                "heavy_spark",
                "finish_burst",
                "blade_trail",
                "warrior_trail"
            ]
        );
        let trail = library.find("sword_trail").and_then(|id| library.effect(id)).expect("its trail");
        assert!(trail.trail.is_some());
        assert!(library.find("nothing").is_none());
    }

    #[test]
    fn a_library_round_trips_through_its_document() {
        let library = EffectLibrary::default_effects();
        let text = library.to_ron().expect("written");
        assert_eq!(EffectLibrary::from_ron(&text).expect("read"), library);
    }

    #[test]
    fn every_bad_emitter_field_is_named() {
        assert_eq!(emitter_error(|e| e.spawn = Spawn::Burst { count: 0 }), "spawn");
        assert_eq!(emitter_error(|e| e.spawn = Spawn::Burst { count: 5000 }), "spawn");
        let rate = Spawn::Rate { per_second: 0.0, duration: 1.0 };
        assert_eq!(emitter_error(|e| e.spawn = rate), "spawn");
        let rate = Spawn::Rate { per_second: 1.0, duration: -1.0 };
        assert_eq!(emitter_error(|e| e.spawn = rate), "spawn");
        let rate = Spawn::Rate { per_second: 1e12, duration: 0.0 };
        assert_eq!(emitter_error(|e| e.spawn = rate), "spawn");
        let rate = Spawn::Rate { per_second: MAX_RATE, duration: 0.0 };
        assert!(library_of(EmitterDesc { spawn: rate, ..emitter() }).is_ok(), "the most it may hold");
        assert_eq!(emitter_error(|e| e.delay = -1.0), "delay");
        assert_eq!(emitter_error(|e| e.source = Source::Sphere(-1.0)), "source");
        assert_eq!(emitter_error(|e| e.source = Source::Disc(f32::NAN)), "source");
        assert_eq!(emitter_error(|e| e.lifetime = (0.0, 1.0)), "lifetime");
        assert_eq!(emitter_error(|e| e.lifetime = (2.0, 1.0)), "lifetime");
        assert_eq!(emitter_error(|e| e.lifetime = (1.0, f32::INFINITY)), "lifetime");
        assert_eq!(emitter_error(|e| e.motion.speed = (-1.0, 1.0)), "motion.speed");
        assert_eq!(emitter_error(|e| e.motion.direction = [0.0; 3]), "motion.direction");
        assert_eq!(emitter_error(|e| e.motion.spread = 200.0), "motion.spread");
        assert_eq!(emitter_error(|e| e.motion.gravity = f32::NAN), "motion.gravity");
        assert_eq!(emitter_error(|e| e.motion.drag = -1.0), "motion.drag");
        let spin = Facing::Camera { spin: (2.0, 1.0) };
        assert_eq!(emitter_error(|e| e.facing = spin), "facing");
        let stretch = Facing::Velocity { stretch: -1.0 };
        assert_eq!(emitter_error(|e| e.facing = stretch), "facing");
        assert_eq!(emitter_error(|e| e.additive = 1.5), "additive");
        assert_eq!(emitter_error(|e| e.soft = 0.0), "soft");
        assert_eq!(emitter_error(|e| e.size_scale = (-1.0, 1.0)), "size_scale");
        assert_eq!(emitter_error(|e| e.size = Curve(vec![])), "size");
        assert_eq!(emitter_error(|e| e.size = Curve(vec![(0.0, -1.0)])), "size");
        assert_eq!(emitter_error(|e| e.color = Curve(vec![(2.0, [1.0; 3])])), "color");
        assert_eq!(emitter_error(|e| e.color = Curve(vec![(0.0, [-1.0; 3])])), "color");
        assert_eq!(emitter_error(|e| e.alpha = Curve(vec![(0.0, 2.0)])), "alpha");
    }

    #[test]
    fn every_bad_trail_field_is_named() {
        assert_eq!(trail_error(|t| t.lifetime = 0.0), "lifetime");
        assert_eq!(trail_error(|t| t.max_samples = 1), "max_samples");
        assert_eq!(trail_error(|t| t.max_samples = 65), "max_samples");
        assert_eq!(trail_error(|t| t.width = Curve(vec![(0.0, -1.0)])), "width");
        assert_eq!(trail_error(|t| t.anchor = 2.0), "anchor");
        assert_eq!(trail_error(|t| t.color = Curve(vec![])), "color");
        assert_eq!(trail_error(|t| t.alpha = Curve(vec![(0.0, 1.0)])), "alpha");
        assert_eq!(trail_error(|t| t.core.position = -0.1), "core.position");
        assert_eq!(trail_error(|t| t.core.half_width = 2.0), "core.half_width");
        assert_eq!(trail_error(|t| t.core.color = Curve(vec![(0.0, [-1.0; 3])])), "core.color");
        assert_eq!(trail_error(|t| t.glow_reach = 0.0), "glow_reach");
        assert_eq!(trail_error(|t| t.start_fade = -1.0), "start_fade");
        assert_eq!(trail_error(|t| t.additive = -0.1), "additive");
        assert_eq!(trail_error(|t| t.soft = f32::NAN), "soft");
    }

    #[test]
    fn libraries_must_hold_named_distinct_effects() {
        assert_eq!(EffectLibrary::new(vec![]), Err(EffectLibraryError::Empty));
        let effect = |name: &str| Effect { name: name.to_owned(), emitters: vec![emitter()], trail: None };
        assert_eq!(EffectLibrary::new(vec![effect("")]), Err(EffectLibraryError::EmptyName(0)));
        assert_eq!(
            EffectLibrary::new(vec![effect("a"), effect("a")]),
            Err(EffectLibraryError::DuplicateName("a".to_owned()))
        );
        let nothing = Effect { name: "a".to_owned(), emitters: vec![], trail: None };
        assert_eq!(EffectLibrary::new(vec![nothing]), Err(EffectLibraryError::Nothing("a".to_owned())));
        let many = Effect { emitters: vec![emitter(); MAX_EMITTERS + 1], ..effect("a") };
        assert_eq!(EffectLibrary::new(vec![many]), Err(EffectLibraryError::TooManyEmitters("a".to_owned())));
    }

    /// A document's error names the field and where it is.
    #[test]
    fn a_documents_bad_field_is_named_in_its_error() {
        let text = r#"(
            type: "sapphire::vfx::EffectLibrary",
            version: 1,
            data: (
                effects: [
                    (
                        name: "hit",
                        emitters: [
                            (
                                name: "sparks",
                                spawn: Burst(count: 4),
                                lifetime: (0.5, 0.1),
                                size: [(0.0, 0.05)],
                                color: [(0.0, (1.0, 1.0, 1.0))],
                            ),
                        ],
                    ),
                ],
            ),
        )"#;
        let error = EffectLibrary::from_ron(text).expect_err("a backward lifetime").to_string();
        assert!(error.contains("`lifetime`") && error.contains("sparks"), "{error}");
    }

    #[test]
    fn a_documents_unknown_field_is_named() {
        let text = r#"(
            type: "sapphire::vfx::EffectLibrary",
            version: 1,
            data: (effects: [(name: "a", emiters: [])]),
        )"#;
        let error = EffectLibrary::from_ron(text).expect_err("a misspelled field").to_string();
        assert!(error.contains("emiters"), "{error}");
    }

    #[test]
    fn fields_left_out_take_their_defaults() {
        let text = r#"(
            type: "sapphire::vfx::EffectLibrary",
            version: 1,
            data: (
                effects: [
                    (
                        name: "hit",
                        emitters: [
                            (
                                name: "dots",
                                spawn: Rate(per_second: 10.0, duration: 0.0),
                                lifetime: (1.0, 1.0),
                                size: [(0.0, 0.05)],
                                color: [(0.0, (1.0, 1.0, 1.0))],
                            ),
                        ],
                    ),
                ],
            ),
        )"#;
        let library = EffectLibrary::from_ron(text).expect("valid");
        let emitter = &library.effect(EffectId(0)).expect("an effect").emitters[0];
        assert_eq!(emitter.additive, 1.0);
        assert_eq!(emitter.alpha, Curve(vec![(0.0, 1.0)]));
        assert_eq!(emitter.size_scale, (1.0, 1.0));
        assert_eq!(emitter.source, Source::Point);
        assert_eq!(emitter.motion, Motion::default());
    }
}
