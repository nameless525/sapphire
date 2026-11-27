//! Effects as documents: an [`EffectLibrary`] (schema 5), named [`Effect`]s of emitters with modules
//! and curves, at most one trail, mesh layers and ground decals, checked whole however they were made, so
//! what plays is valid and an author's mistake names its field. A library is written as a `.effects.ron`
//! document and cooks as its own asset kind (M6 N1, `sapphire_assets::effects`), or is compiled in
//! ([`EffectLibrary::default_effects`]). An emitter may name a painted sheet by its source's ID (M6 N4a,
//! `sapphire_assets::effect_sheet`): the cook holds that it is one, and the library depends on it.

use std::collections::BTreeSet;

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use glam::Vec3;
use sapphire_look::vfx::stateless::{
    MAX_STATELESS_SLOTS, STATELESS_KEYS, StatelessEmitter, StatelessFacing, StatelessNoise, StatelessParams,
    StatelessSource, StatelessSpawn,
};
use sapphire_look::{VfxLight, VfxShape};
use sapphire_reflect::{Decoder, VersionError, Versioned};
use sapphire_serialize::{AssetId, DocumentError};
use serde::{Deserialize, Serialize};

use crate::curve::{Curve, MAX_KEYS};
use crate::decal::{DecalLayerDesc, MAX_DECAL_LAYERS};
use crate::events::{MAX_EVENTS, ParticleEvent, effect_problem};
use crate::forces::{Force, Ground, MAX_FORCES};
use crate::mesh::{MAX_MESH_LAYERS, MeshLayerDesc};

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

/// The most particles a [`Spawn::PerMeter`] fires a meter: a particle a millimeter. Past this it's a typo.
const MAX_PER_METER: f32 = 1000.0;

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
    /// `count` at once every `interval` seconds, `times` times, or until the effect is stopped where `times` is
    /// 0 (M6 N2): an ember's pulse. The first fires as the emitter starts; at most 245,760 a second.
    Repeat { count: u32, interval: f32, times: u32 },
    /// `per_meter` along the way the effect is [moved](crate::VfxWorld::set_placement), spread evenly along
    /// it, until the effect is stopped (M6 N2): dust behind a running character, none while it stands. At most
    /// 1,000 a meter. A move of more than 3 m in one step is a jump, a teleport or a respawn, and fires none.
    PerMeter { per_meter: f32 },
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
    /// On a level band `width` across, centred on a circle of `radius` (M6 N8a): a shock's ring of dust.
    Ring { radius: f32, width: f32 },
    /// Anywhere within a cone `length` long up the effect's +Y from its apex at the effect, opening `angle`
    /// degrees from its axis, under 90 (M6 N8a): a fountain's spray.
    Cone { angle: f32, length: f32 },
    /// Anywhere within a box of these half extents (M6 N8a): motes filling a room.
    Box([f32; 3]),
    /// On a line this long along the effect's X, centred on it (M6 N8a): a wall of embers.
    Line(f32),
}

/// Which path an emitter's particles take (M6 N8a).
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum Sim {
    /// Simulated on the CPU a step at a time, within its world's budget: what every module can do.
    #[default]
    Cpu,
    /// Each particle a closed-form function of its instance's seed and age, drawn by the GPU with no state
    /// kept (`sapphire_look::vfx::stateless`): many cheap particles, at a closed set of modules. A spawn along a
    /// move, a vortex, an attractor, a speed limit and a ground need state, and are refused.
    Stateless,
}

impl Sim {
    fn is_cpu(&self) -> bool {
        *self == Self::Cpu
    }
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
    /// Toward the camera and upright on screen, the quad's length up it, never turning: a shape whose
    /// way up says something, as a cue mark's does (M4 A5). It takes no random numbers.
    Upright,
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
    Chevron,
    Diamond,
}

impl From<Shape> for VfxShape {
    fn from(shape: Shape) -> Self {
        match shape {
            Shape::SoftDot => Self::SoftDot,
            Shape::Streak => Self::Streak,
            Shape::Star => Self::Star,
            Shape::Ring => Self::Ring,
            Shape::Chevron => Self::Chevron,
            Shape::Diamond => Self::Diamond,
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

pub(crate) fn default_soft() -> f32 {
    DEFAULT_SOFT
}

pub(crate) fn one() -> Curve<f32> {
    Curve(vec![(0.0, 1.0)])
}

pub(crate) fn one_share() -> f32 {
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
    /// What else moves its particles as they live, at most [`MAX_FORCES`], in order (M6 N2): after gravity,
    /// before drag. Left out of a document that has none, as an effect's `meshes` are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forces: Vec<Force>,
    /// The ground its particles meet, if they do (M6 N2): a level plane at the owner's ground, after they
    /// move. Left out where they fall through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collide: Option<Ground>,
    /// Which path its particles take (M6 N8a): the CPU's, left out, or the stateless one.
    #[serde(default, skip_serializing_if = "Sim::is_cpu")]
    pub sim: Sim,
    /// A stateless emitter's wrap box (M6 N8a): half extents of a box about a centre its instance follows (the
    /// camera, for ambient motes), which its particles wrap into and fade at the sides of. Left out where they
    /// stay where they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<[f32; 3]>,
    /// The painted sheet its quads show (M6 N4a), by the ID of its source (a `.sheet.ron`): a flipbook whose
    /// frames turn as the sheet says, in place of `shape` once the effects atlas draws it (N4b). Nothing draws
    /// one yet. Left out where its quads show `shape`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<AssetId>,
    /// What its particles start as they're born, die or first meet its ground (M6 N3, [`ParticleEvent`]): another
    /// emitter of its effect, a sub-emitter, fires its burst where each one is, at most [`MAX_EVENTS`]. Left out
    /// where it has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<ParticleEvent>,
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
    /// A cue mark (M4 A5): its random numbers come from a stream marks share apart from every other
    /// effect's, so a mark added to a cue moves no spark of the effects spawned after it.
    #[serde(default)]
    pub mark: bool,
    /// Its mesh layers (M5 L5), at most [`MAX_MESH_LAYERS`]: drawn where its sprites are, in the order
    /// written. Left out of a document that has none, so a library without them writes as it did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meshes: Vec<MeshLayerDesc>,
    /// Its ground decals (M5 L7), at most [`MAX_DECAL_LAYERS`]: flat on the ground where it plays, drawn in the
    /// main pass over the world. Left out of a document that has none, as `meshes` are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decals: Vec<DecalLayerDesc>,
}

impl Effect {
    /// Whether it plays until it's [stopped](crate::VfxWorld::stop): a trail records until then, and a
    /// rate with no duration emits. Only what keeps its [handle](crate::EffectHandle) can start one,
    /// or it holds its place in the budget for good.
    #[must_use]
    pub fn is_endless(&self) -> bool {
        // A mesh layer or a decal has a lifetime: it never makes an effect endless.
        self.trail.is_some()
            || self.emitters.iter().any(|emitter| match emitter.spawn {
                Spawn::Burst { .. } => false,
                Spawn::Rate { duration, .. } => duration <= 0.0,
                Spawn::Repeat { times, .. } => times == 0,
                Spawn::PerMeter { .. } => true,
            })
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
    #[error("effect `{0}`: it has no emitter, trail, mesh layer or decal")]
    Nothing(String),
    #[error("effect `{0}`: it has more than {MAX_EMITTERS} emitters")]
    TooManyEmitters(String),
    #[error("effect `{0}`: it has more than {MAX_MESH_LAYERS} mesh layers")]
    TooManyMeshLayers(String),
    #[error("effect `{0}`: it has more than {MAX_DECAL_LAYERS} decal layers")]
    TooManyDecalLayers(String),
    /// A field of an emitter, a trail, a mesh layer or a decal: `part` says which.
    #[error("effect `{effect}`, {part}: `{field}` {problem}")]
    Invalid { effect: String, part: String, field: &'static str, problem: &'static str },
}

/// Named effects (an `EffectLibrary` document, schema 5).
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
    /// 1: M2 E2. 2: M5 L5's mesh layers. 3: M5 L7's ground decals. 4: M5 L6's distortion, a mesh layer's
    /// `distortion`, and its `color` left out where it only bends the picture. 5: M6 N1, libraries cooked as
    /// their own asset kind; what M6's effects add after it (forces, events, painted sheets, stateless
    /// emitters, world effects) joins as fields an effect without them leaves out, written only where they
    /// aren't their defaults. An effect without any of them needn't say, so an older document decodes as it
    /// is, every value to the bit, and writes back as it was but for its version (`testdata/`).
    const VERSION: u32 = 5;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        match version {
            1..=4 => data.decode().map_err(VersionError::Decode),
            found => Err(VersionError::TooOld { found, oldest: 1 }),
        }
    }
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
            if effect.emitters.is_empty()
                && effect.trail.is_none()
                && effect.meshes.is_empty()
                && effect.decals.is_empty()
            {
                return Err(EffectLibraryError::Nothing(effect.name.clone()));
            }
            if effect.emitters.len() > MAX_EMITTERS {
                return Err(EffectLibraryError::TooManyEmitters(effect.name.clone()));
            }
            if effect.meshes.len() > MAX_MESH_LAYERS {
                return Err(EffectLibraryError::TooManyMeshLayers(effect.name.clone()));
            }
            if effect.decals.len() > MAX_DECAL_LAYERS {
                return Err(EffectLibraryError::TooManyDecalLayers(effect.name.clone()));
            }
            for (layer_index, layer) in effect.decals.iter().enumerate() {
                layer.problem().map_err(|(field, problem)| EffectLibraryError::Invalid {
                    effect: effect.name.clone(),
                    part: if layer.name.is_empty() {
                        format!("decal layer {layer_index}")
                    } else {
                        format!("decal layer `{}`", layer.name)
                    },
                    field,
                    problem,
                })?;
            }
            for (layer_index, layer) in effect.meshes.iter().enumerate() {
                layer.problem().map_err(|(field, problem)| EffectLibraryError::Invalid {
                    effect: effect.name.clone(),
                    part: if layer.name.is_empty() {
                        format!("mesh layer {layer_index}")
                    } else {
                        format!("mesh layer `{}`", layer.name)
                    },
                    field,
                    problem,
                })?;
            }
            let emitter_error = |emitter_index: usize, (field, problem): Problem| {
                let name = &effect.emitters[emitter_index].name;
                EffectLibraryError::Invalid {
                    effect: effect.name.clone(),
                    part: if name.is_empty() {
                        format!("emitter {emitter_index}")
                    } else {
                        format!("emitter `{name}`")
                    },
                    field,
                    problem,
                }
            };
            for (emitter_index, emitter) in effect.emitters.iter().enumerate() {
                emitter.problem().map_err(|problem| emitter_error(emitter_index, problem))?;
            }
            // The events among them (M6 N3), once each is valid on its own.
            effect_problem(effect)
                .map_err(|(emitter_index, problem)| emitter_error(emitter_index, problem))?;
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
    /// `warrior_trail`; and effect meshes (M5 L5): the skill's `skill_arc`, and `shock_ring`, `ember_dome` and
    /// `burst_card` for `--vfx`; and ground decals (M5 L7): a wind-up's `telegraph_ground` and a heavy landing's
    /// `scorch_mark`; and forces and the ground (M6 N2), for `--vfx forces`: `ember_drift`, `spark_bounce` and
    /// `charge_gather`; and events and sub-emitters (M6 N3), for `--vfx events`: `spark_burst` and `debris_splash`.
    /// Their colors and curves are starting points for art direction.
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

    /// Whether any of its effects has a mesh layer that draws light: what a scene declares so the renderer builds
    /// the mesh family ahead (M5 L5), and builds none of it otherwise. A layer that only bends the picture (M5
    /// L6) isn't one.
    #[must_use]
    pub fn has_meshes(&self) -> bool {
        self.effects.iter().flat_map(|effect| &effect.meshes).any(MeshLayerDesc::has_color)
    }

    /// Whether any of its effects has a mesh layer that bends the picture: what a scene declares so the renderer
    /// builds the distortion's pipelines ahead (M5 L6), and builds none of them otherwise.
    #[must_use]
    pub fn has_distortion(&self) -> bool {
        self.effects.iter().flat_map(|effect| &effect.meshes).any(|layer| layer.distortion.is_some())
    }

    /// Whether any of its effects has a ground decal: what a scene declares so the renderer builds the decal
    /// family ahead (M5 L7), and builds none of it otherwise.
    #[must_use]
    pub fn has_decals(&self) -> bool {
        self.effects.iter().any(|effect| !effect.decals.is_empty())
    }

    /// Its effects' names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.effects.iter().map(|effect| effect.name.as_str())
    }

    /// Its effects, in order: what an [`EffectId`] counts.
    #[must_use]
    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }

    /// The painted sheets its emitters name (M6 N4a), sorted, each once: what its cook depends on, and its
    /// effects atlas holds.
    #[must_use]
    pub fn sheets(&self) -> Vec<AssetId> {
        let sheets: BTreeSet<AssetId> = self
            .effects
            .iter()
            .flat_map(|effect| &effect.emitters)
            .filter_map(|emitter| emitter.sheet)
            .collect();
        sheets.into_iter().collect()
    }
}

/// Whether both ends are finite and in order.
fn range_ok((low, high): Range) -> bool {
    low.is_finite() && high.is_finite() && low <= high
}

pub(crate) fn share_ok(value: f32) -> bool {
    (0.0..=1.0).contains(&value)
}

pub(crate) type Problem = (&'static str, &'static str);

/// Fails with `field`'s name when `ok` doesn't hold.
pub(crate) fn require(ok: bool, field: &'static str, problem: &'static str) -> Result<(), Problem> {
    if ok { Ok(()) } else { Err((field, problem)) }
}

/// Fails with `field`'s name if the curve isn't valid.
pub(crate) fn curve_ok<T: crate::curve::CurveValue>(
    curve: &Curve<T>,
    field: &'static str,
) -> Result<(), Problem> {
    curve.problem().map_or(Ok(()), |problem| Err((field, problem)))
}

impl EmitterDesc {
    /// Whether it has forces or a ground (M6 N2): what a step resolves for it, and its particles run through. A
    /// stateless emitter's are its closed form's, never a step's.
    pub(crate) fn is_moved(&self) -> bool {
        !self.is_stateless() && (!self.forces.is_empty() || self.collide.is_some())
    }

    /// Whether its particles take the stateless path (M6 N8a), which a [`VfxWorld`](crate::VfxWorld) never
    /// simulates.
    #[must_use]
    pub fn is_stateless(&self) -> bool {
        self.sim == Sim::Stateless
    }

    /// Its stateless form, as the GPU and the CPU reference read it (M6 N8a), if it is stateless: its own
    /// fields resolved, the forces it may have as the closed form's wind and noise terms. Allocates nothing.
    #[must_use]
    pub fn stateless(&self) -> Option<StatelessEmitter> {
        if !self.is_stateless() {
            return None;
        }
        let spawn = self.stateless_spawn()?;
        let mut wind = 0.0;
        let mut noise = None;
        for force in &self.forces {
            match *force {
                Force::Wind { response } => wind = response,
                Force::Noise { strength, frequency, octaves, drift } => {
                    noise = Some(StatelessNoise { strength, frequency, octaves: u32::from(octaves), drift });
                }
                // A check refuses them on a stateless emitter.
                Force::Vortex { .. } | Force::Attract { .. } | Force::LimitSpeed { .. } => {}
            }
        }
        let facing = match self.facing {
            Facing::Camera { spin } => StatelessFacing::Camera { spin },
            Facing::Velocity { stretch } => StatelessFacing::Velocity { stretch },
            Facing::Upright => StatelessFacing::Upright,
        };
        // A curve holds at most `MAX_KEYS` keys, which the stateless form holds inline: nothing allocates.
        let mut size = [(0.0, 0.0); MAX_KEYS];
        let mut alpha = [(0.0, 0.0); MAX_KEYS];
        let mut color = [(0.0, [0.0; 3]); MAX_KEYS];
        let motion = &self.motion;
        Some(StatelessEmitter::new(&StatelessParams {
            spawn,
            delay: self.delay,
            lifetime: self.lifetime,
            source: self.source.stateless(),
            speed: motion.speed,
            direction: Vec3::from(motion.direction),
            spread: motion.spread,
            gravity: motion.gravity,
            drag: motion.drag,
            wind,
            noise,
            facing,
            size_scale: self.size_scale,
            size: copy_keys(&self.size, &mut size),
            alpha: copy_keys(&self.alpha, &mut alpha),
            color: copy_keys(&self.color, &mut color),
            wrap: self.wrap.map(Vec3::from),
            shape: self.shape.into(),
            light: self.light.into(),
            additive: self.additive,
            soft: self.soft,
        }))
    }

    /// Its spawn as the stateless form's, if it has one: a spawn along a move hasn't.
    fn stateless_spawn(&self) -> Option<StatelessSpawn> {
        match self.spawn {
            Spawn::Burst { count } => Some(StatelessSpawn::Burst { count }),
            Spawn::Rate { per_second, duration } => Some(StatelessSpawn::Rate { per_second, duration }),
            Spawn::Repeat { count, interval, times } => {
                Some(StatelessSpawn::Repeat { count, interval, times })
            }
            Spawn::PerMeter { .. } => None,
        }
    }

    /// What a stateless emitter can't do (M6 N8a): what needs a particle's state, and more particles alive at
    /// once than its slots hold.
    fn stateless_problem(&self) -> Result<(), Problem> {
        let spawn =
            self.stateless_spawn().ok_or(("spawn", "a stateless emitter can't spawn along a move"))?;
        require(
            spawn.slots(self.lifetime.1) <= u64::from(MAX_STATELESS_SLOTS),
            "spawn",
            "a stateless emitter keeps at most 4096 particles alive at once",
        )?;
        let winds = self.forces.iter().filter(|force| matches!(force, Force::Wind { .. })).count();
        let noises = self.forces.iter().filter(|force| force.is_noise()).count();
        require(
            winds <= 1 && noises <= 1 && winds + noises == self.forces.len(),
            "forces",
            "a stateless emitter takes at most a wind and a noise",
        )?;
        require(self.collide.is_none(), "collide", "a stateless emitter can't meet the ground")?;
        if let Some(half) = self.wrap {
            require(
                half.into_iter().all(|value| value.is_finite() && value > 0.0),
                "wrap",
                "a wrap's half extents must be above 0",
            )?;
        }
        Ok(())
    }

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
            Spawn::Repeat { count, interval, .. } => {
                require(count >= 1, "spawn", "a repeat must fire at least one")?;
                require(count <= MAX_BURST, "spawn", "a repeat fires at most 4096 at once")?;
                require(interval.is_finite() && interval > 0.0, "spawn", "an interval must be above 0")?;
                require(
                    count as f32 / interval <= MAX_RATE,
                    "spawn",
                    "a repeat fires at most 245760 a second",
                )?;
            }
            Spawn::PerMeter { per_meter } => {
                require(per_meter.is_finite() && per_meter > 0.0, "spawn", "a rate a meter must be above 0")?;
                require(per_meter <= MAX_PER_METER, "spawn", "a rate a meter is at most 1000")?;
            }
        }
        require(self.delay.is_finite() && self.delay >= 0.0, "delay", "can't be negative")?;
        let size_ok = |value: f32| value.is_finite() && value >= 0.0;
        match self.source {
            Source::Point => {}
            Source::Sphere(radius) | Source::Disc(radius) => {
                require(size_ok(radius), "source", "a radius can't be negative")?;
            }
            Source::Ring { radius, width } => {
                require(
                    size_ok(radius) && size_ok(width),
                    "source",
                    "a ring's radius and width can't be negative",
                )?;
            }
            Source::Cone { angle, length } => {
                require(
                    (0.0..90.0).contains(&angle),
                    "source",
                    "a cone's angle must be 0 to under 90 degrees",
                )?;
                require(size_ok(length), "source", "a cone's length can't be negative")?;
            }
            Source::Box(half) => {
                require(half.into_iter().all(size_ok), "source", "a box's half extents can't be negative")?;
            }
            Source::Line(length) => require(size_ok(length), "source", "a line's length can't be negative")?,
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
            Facing::Upright => {}
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
        require(self.forces.len() <= MAX_FORCES, "forces", "an emitter has at most 6")?;
        for force in &self.forces {
            force.problem()?;
        }
        if let Some(ground) = &self.collide {
            ground.problem()?;
        }
        if self.is_stateless() {
            self.stateless_problem()?;
        } else {
            require(self.wrap.is_none(), "wrap", "only a stateless emitter wraps")?;
        }
        require(self.events.len() <= MAX_EVENTS, "events", "an emitter has at most 3")?;
        require(
            self.events.is_empty() || !self.is_stateless(),
            "events",
            "a stateless emitter's particles fire no events",
        )?;
        for event in &self.events {
            event.problem(self.collide.is_some())?;
        }
        Ok(())
    }
}

impl Source {
    /// The stateless form's (M6 N8a).
    fn stateless(self) -> StatelessSource {
        match self {
            Self::Point => StatelessSource::Point,
            Self::Sphere(radius) => StatelessSource::Sphere { radius },
            Self::Disc(radius) => StatelessSource::Disc { radius },
            Self::Ring { radius, width } => StatelessSource::Ring { radius, width },
            Self::Cone { angle, length } => StatelessSource::Cone { angle, length },
            Self::Box(half) => StatelessSource::Box { half: Vec3::from(half) },
            Self::Line(length) => StatelessSource::Line { length },
        }
    }
}

// A stateless emitter holds every key a curve may have, so none is left out on the way.
const _: () = assert!(MAX_KEYS == STATELESS_KEYS, "a stateless emitter's curves hold a document's");

/// A curve's keys copied into `keys`, which holds as many as a curve may: the slice they fill.
fn copy_keys<'a, T: Copy>(curve: &Curve<T>, keys: &'a mut [(f32, T); MAX_KEYS]) -> &'a [(f32, T)] {
    let count = curve.0.len().min(MAX_KEYS);
    keys[..count].copy_from_slice(&curve.0[..count]);
    &keys[..count]
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
    use crate::events::{EventTrigger, MAX_EVENT_DEPTH};

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
            forces: Vec::new(),
            collide: None,
            sim: Sim::Cpu,
            wrap: None,
            sheet: None,
            events: Vec::new(),
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
        EffectLibrary::new(vec![Effect {
            name: "hit".to_owned(),
            emitters: vec![emitter],
            trail: None,
            mark: false,
            meshes: Vec::new(),
            decals: Vec::new(),
        }])
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
        let effect = Effect {
            name: "swing".to_owned(),
            emitters: Vec::new(),
            trail: Some(bad),
            mark: false,
            meshes: Vec::new(),
            decals: Vec::new(),
        };
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
        let first = Effect {
            name: "same".to_owned(),
            emitters: vec![emitter()],
            trail: None,
            mark: false,
            meshes: Vec::new(),
            decals: Vec::new(),
        };
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
                "warrior_trail",
                "telegraph_mark",
                "dodge_glint",
                "phase_mark",
                "skill_arc",
                "shock_ring",
                "ember_dome",
                "burst_card",
                "telegraph_ground",
                "scorch_mark",
                "ember_drift",
                "spark_bounce",
                "charge_gather",
                "spark_burst",
                "debris_splash"
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

    /// M6 N2's spawns, forces and ground: each bad value names its field, and the edges hold.
    #[test]
    fn every_bad_force_spawn_and_ground_is_named() {
        let repeat = |count, interval| Spawn::Repeat { count, interval, times: 3 };
        assert_eq!(emitter_error(|e| e.spawn = repeat(0, 0.1)), "spawn");
        assert_eq!(emitter_error(|e| e.spawn = repeat(5000, 0.1)), "spawn");
        assert_eq!(emitter_error(|e| e.spawn = repeat(4, 0.0)), "spawn");
        assert_eq!(emitter_error(|e| e.spawn = repeat(4096, 0.01)), "spawn");
        assert!(library_of(EmitterDesc { spawn: repeat(4096, 1.0), ..emitter() }).is_ok());
        assert_eq!(emitter_error(|e| e.spawn = Spawn::PerMeter { per_meter: 0.0 }), "spawn");
        assert_eq!(emitter_error(|e| e.spawn = Spawn::PerMeter { per_meter: 1001.0 }), "spawn");
        assert_eq!(emitter_error(|e| e.spawn = Spawn::PerMeter { per_meter: f32::NAN }), "spawn");
        let bad = [
            Force::Wind { response: -1.0 },
            Force::Noise { strength: f32::NAN, frequency: 1.0, octaves: 1, drift: 0.0 },
            Force::Noise { strength: 1.0, frequency: 0.0, octaves: 1, drift: 0.0 },
            Force::Noise { strength: 1.0, frequency: 1.0, octaves: 0, drift: 0.0 },
            Force::Noise { strength: 1.0, frequency: 1.0, octaves: 3, drift: 0.0 },
            Force::Noise { strength: 1.0, frequency: 1.0, octaves: 1, drift: -1.0 },
            Force::Vortex { axis: [0.0; 3], strength: 1.0, pull: 0.0 },
            Force::Vortex { axis: [0.0, 1.0, 0.0], strength: f32::INFINITY, pull: 0.0 },
            Force::Attract { point: [f32::NAN, 0.0, 0.0], strength: 1.0, kill_radius: 0.0 },
            Force::Attract { point: [0.0; 3], strength: 1.0, kill_radius: -0.1 },
            Force::LimitSpeed { max: 0.0 },
        ];
        for force in bad {
            assert_eq!(emitter_error(|e| e.forces = vec![force]), "forces", "{force:?}");
        }
        let wind = Force::Wind { response: 1.0 };
        assert_eq!(emitter_error(|e| e.forces = vec![wind; MAX_FORCES + 1]), "forces");
        assert!(library_of(EmitterDesc { forces: vec![wind; MAX_FORCES], ..emitter() }).is_ok());
        let ground = |bounce, friction| Some(Ground { bounce, friction, dies: false });
        assert_eq!(emitter_error(|e| e.collide = ground(1.5, 0.0)), "collide.bounce");
        assert_eq!(emitter_error(|e| e.collide = ground(0.5, -0.1)), "collide.friction");
        assert!(library_of(EmitterDesc { collide: ground(1.0, 1.0), ..emitter() }).is_ok());
    }

    /// What N2 adds is written only where an emitter has it, and reads back as it was.
    #[test]
    fn forces_and_a_ground_write_only_where_an_emitter_has_them() {
        let plain = library_of(emitter()).expect("valid").to_ron().expect("written");
        assert!(!plain.contains("forces") && !plain.contains("collide"), "{plain}");
        let moved = EmitterDesc {
            spawn: Spawn::Repeat { count: 3, interval: 0.25, times: 0 },
            forces: vec![
                Force::Noise { strength: 2.0, frequency: 1.5, octaves: 2, drift: 0.5 },
                Force::Vortex { axis: [0.0, 1.0, 0.0], strength: 3.0, pull: 1.0 },
                Force::LimitSpeed { max: 4.0 },
            ],
            collide: Some(Ground { bounce: 0.4, friction: 0.2, dies: false }),
            ..emitter()
        };
        let library = library_of(moved).expect("valid");
        let text = library.to_ron().expect("written");
        assert!(text.contains("forces") && text.contains("collide"), "{text}");
        assert_eq!(EffectLibrary::from_ron(&text).expect("reads"), library);
    }

    /// M6 N8a's sources take their sizes, and a stateless emitter takes only what its closed form can do:
    /// each refusal names its field.
    #[test]
    fn a_stateless_emitter_takes_only_its_closed_set() {
        let bad_sources = [
            Source::Ring { radius: -1.0, width: 0.1 },
            Source::Ring { radius: 1.0, width: f32::NAN },
            Source::Cone { angle: 90.0, length: 1.0 },
            Source::Cone { angle: 30.0, length: -1.0 },
            Source::Box([1.0, -0.5, 1.0]),
            Source::Line(f32::INFINITY),
        ];
        for source in bad_sources {
            assert_eq!(emitter_error(|e| e.source = source), "source", "{source:?}");
        }
        let stateless = || EmitterDesc { sim: Sim::Stateless, ..emitter() };
        let error = |edit: fn(&mut EmitterDesc)| {
            emitter_error(|e| {
                *e = stateless();
                edit(e);
            })
        };
        assert!(library_of(stateless()).is_ok());
        assert_eq!(error(|e| e.spawn = Spawn::PerMeter { per_meter: 4.0 }), "spawn");
        // 4096 alive at once at most: a rate of 2048 a second living two seconds fills it.
        let too_many = |e: &mut EmitterDesc| {
            e.spawn = Spawn::Rate { per_second: 2049.0, duration: 0.0 };
            e.lifetime = (1.0, 2.0);
        };
        assert_eq!(error(too_many), "spawn");
        assert!(
            library_of(EmitterDesc {
                spawn: Spawn::Rate { per_second: 2048.0, duration: 0.0 },
                lifetime: (1.0, 2.0),
                ..stateless()
            })
            .is_ok()
        );
        assert_eq!(error(|e| e.spawn = Spawn::Repeat { count: 1500, interval: 0.05, times: 0 }), "spawn");
        assert_eq!(error(|e| e.forces = vec![Force::LimitSpeed { max: 2.0 }]), "forces");
        assert_eq!(
            error(|e| e.forces = vec![Force::Vortex { axis: [0.0, 1.0, 0.0], strength: 1.0, pull: 0.0 }]),
            "forces"
        );
        assert_eq!(error(|e| e.forces = vec![Force::Wind { response: 1.0 }; 2]), "forces");
        assert_eq!(error(|e| e.collide = Some(Ground { bounce: 0.0, friction: 0.0, dies: true })), "collide");
        assert_eq!(error(|e| e.wrap = Some([4.0, 0.0, 4.0])), "wrap");
        assert_eq!(
            emitter_error(|e| e.wrap = Some([4.0, 2.0, 4.0])),
            "wrap",
            "only a stateless emitter wraps"
        );
        let full = EmitterDesc {
            forces: vec![
                Force::Noise { strength: 1.0, frequency: 0.5, octaves: 2, drift: 0.2 },
                Force::Wind { response: 2.0 },
            ],
            wrap: Some([8.0, 3.0, 8.0]),
            source: Source::Box([8.0, 3.0, 8.0]),
            ..stateless()
        };
        assert!(library_of(full).is_ok());
    }

    /// What N8a adds is written only where an emitter has it, and reads back as it was.
    #[test]
    fn a_stateless_emitter_writes_only_what_it_has() {
        let plain = library_of(emitter()).expect("valid").to_ron().expect("written");
        assert!(!plain.contains("sim") && !plain.contains("wrap"), "{plain}");
        let stateless = EmitterDesc {
            sim: Sim::Stateless,
            spawn: Spawn::Rate { per_second: 40.0, duration: 0.0 },
            source: Source::Cone { angle: 20.0, length: 1.5 },
            wrap: Some([6.0, 2.0, 6.0]),
            ..emitter()
        };
        let library = library_of(stateless).expect("valid");
        let text = library.to_ron().expect("written");
        assert!(
            text.contains("sim: Stateless") && text.contains("wrap:") && text.contains("Cone("),
            "{text}"
        );
        assert_eq!(EffectLibrary::from_ron(&text).expect("reads"), library);
        for source in
            [Source::Ring { radius: 2.0, width: 0.5 }, Source::Box([1.0, 2.0, 3.0]), Source::Line(4.0)]
        {
            let library = library_of(EmitterDesc { source, ..emitter() }).expect("valid");
            assert_eq!(EffectLibrary::from_ron(&library.to_ron().expect("written")).expect("reads"), library);
        }
    }

    /// What N4a adds is written only where an emitter names a sheet, reads back as it was, and the library lists
    /// its sheets sorted, each once, whichever emitters name them.
    #[test]
    fn a_sheet_writes_only_where_an_emitter_names_one() {
        let plain = library_of(emitter()).expect("valid");
        assert!(!plain.to_ron().expect("written").contains("sheet"));
        assert!(plain.sheets().is_empty());
        let [smoke, spark] = ["7c4b0a9e-3f1d-4c2a-9b8e-5d6f7a8b9c0d", "1d2e3f40-5a6b-4c7d-8e9f-a0b1c2d3e4f5"]
            .map(|text| text.parse::<AssetId>().expect("an ID"));
        let named =
            |name: &str, sheet| EmitterDesc { name: name.to_owned(), sheet: Some(sheet), ..emitter() };
        let effect = |name: &str, emitters| Effect {
            name: name.to_owned(),
            emitters,
            trail: None,
            mark: false,
            meshes: Vec::new(),
            decals: Vec::new(),
        };
        let library = EffectLibrary::new(vec![
            effect("puff", vec![named("a", smoke), emitter(), named("b", spark)]),
            effect("again", vec![named("c", smoke)]),
        ])
        .expect("valid");
        let text = library.to_ron().expect("written");
        assert!(text.contains(r#"sheet: Some("7c4b0a9e-3f1d-4c2a-9b8e-5d6f7a8b9c0d")"#), "{text}");
        assert_eq!(EffectLibrary::from_ron(&text).expect("reads"), library);
        assert_eq!(library.sheets(), [spark, smoke], "sorted, each once");
    }

    /// An emitter called `name` whose particles fire `events`, each `(trigger, sub-emitter)`.
    fn firing(name: &str, events: &[(EventTrigger, &str)]) -> EmitterDesc {
        let events = events.iter().map(|&(on, emitter)| ParticleEvent {
            on,
            emitter: emitter.to_owned(),
            inherit: 0.5,
        });
        EmitterDesc { name: name.to_owned(), events: events.collect(), ..emitter() }
    }

    /// The emitter and the field an effect of `emitters` makes the library name.
    fn events_error(emitters: Vec<EmitterDesc>) -> (String, &'static str) {
        let effect = Effect {
            name: "hit".to_owned(),
            emitters,
            trail: None,
            mark: false,
            meshes: Vec::new(),
            decals: Vec::new(),
        };
        match EffectLibrary::new(vec![effect]) {
            Err(EffectLibraryError::Invalid { part, field, .. }) => (part, field),
            other => panic!("expected an invalid emitter, got {other:?}"),
        }
    }

    /// M6 N3's closed set: an event fires only from a CPU emitter, a collision's only where it has a ground, at most
    /// three an emitter with a share of velocity from 0 to 1, naming a burst of its own effect with no delay on the
    /// CPU; events never lead back where they started nor chain past two. Each refusal names its emitter and field.
    #[test]
    fn events_take_only_their_closed_set_and_each_refusal_is_named() {
        use EventTrigger::{Birth, Collide, Death};
        let part = |name: &str| format!("emitter `{name}`");
        let sub = |name: &str| firing(name, &[]);
        let ok = |emitters: Vec<EmitterDesc>| {
            EffectLibrary::new(vec![Effect {
                name: "hit".to_owned(),
                emitters,
                trail: None,
                mark: false,
                meshes: Vec::new(),
                decals: Vec::new(),
            }])
            .is_ok()
        };
        assert!(ok(vec![firing("sparks", &[(Death, "embers"), (Birth, "embers")]), sub("embers")]));
        assert_eq!(events_error(vec![firing("sparks", &[(Death, "ashes")])]), (part("sparks"), "events"));
        let stateless = EmitterDesc { sim: Sim::Stateless, ..firing("sparks", &[(Death, "embers")]) };
        assert_eq!(events_error(vec![stateless, sub("embers")]), (part("sparks"), "events"));
        assert_eq!(
            events_error(vec![firing("sparks", &[(Collide, "embers")]), sub("embers")]),
            (part("sparks"), "events"),
            "a collision needs a ground"
        );
        let grounded = EmitterDesc {
            collide: Some(Ground { bounce: 0.5, friction: 0.0, dies: false }),
            ..firing("sparks", &[(Collide, "embers")])
        };
        assert!(ok(vec![grounded, sub("embers")]));
        for inherit in [-0.1, 1.5, f32::NAN] {
            let mut bad = firing("sparks", &[(Death, "embers")]);
            bad.events[0].inherit = inherit;
            assert_eq!(events_error(vec![bad, sub("embers")]), (part("sparks"), "events"), "{inherit}");
        }
        let four = firing("sparks", &[(Death, "embers"); MAX_EVENTS + 1]);
        assert_eq!(events_error(vec![four, sub("embers")]), (part("sparks"), "events"));
        assert!(ok(vec![firing("sparks", &[(Death, "embers"); MAX_EVENTS]), sub("embers")]));
        // The sub-emitter's own fields.
        let rate = EmitterDesc { spawn: Spawn::Rate { per_second: 10.0, duration: 1.0 }, ..sub("embers") };
        assert_eq!(
            events_error(vec![firing("sparks", &[(Death, "embers")]), rate]),
            (part("embers"), "spawn")
        );
        let late = EmitterDesc { delay: 0.1, ..sub("embers") };
        assert_eq!(
            events_error(vec![firing("sparks", &[(Death, "embers")]), late]),
            (part("embers"), "delay")
        );
        let gpu = EmitterDesc { sim: Sim::Stateless, ..sub("embers") };
        assert_eq!(events_error(vec![firing("sparks", &[(Death, "embers")]), gpu]), (part("embers"), "sim"));
        // Cycles, one emitter's and two's, and a chain past the cap.
        assert_eq!(events_error(vec![firing("sparks", &[(Death, "sparks")])]), (part("sparks"), "events"));
        let (a, b) = (firing("a", &[(Death, "b")]), firing("b", &[(Birth, "a")]));
        assert_eq!(events_error(vec![sub("root"), a.clone(), b.clone()]), (part("a"), "events"));
        let into_the_loop = firing("root", &[(Death, "a")]);
        assert_eq!(events_error(vec![into_the_loop, a, b]), (part("a"), "events"), "named where it loops");
        let twins = vec![firing("sparks", &[(Death, "embers")]), sub("embers"), sub("embers")];
        assert_eq!(events_error(twins), (part("embers"), "name"));
        let chain = |length: usize| {
            let names: Vec<String> = (0..=length).map(|link| format!("e{link}")).collect();
            (0..=length)
                .map(|link| match names.get(link + 1) {
                    Some(next) => firing(&names[link], &[(Death, next.as_str())]),
                    None => sub(&names[link]),
                })
                .collect::<Vec<_>>()
        };
        assert!(ok(chain(MAX_EVENT_DEPTH)));
        assert_eq!(events_error(chain(MAX_EVENT_DEPTH + 1)), (part("e0"), "events"));
    }

    /// What N3 adds is written only where an emitter has events, and reads back as it was.
    #[test]
    fn events_write_only_where_an_emitter_has_them() {
        let plain = library_of(emitter()).expect("valid").to_ron().expect("written");
        assert!(!plain.contains("events"), "{plain}");
        let effect = Effect {
            name: "hit".to_owned(),
            emitters: vec![firing("sparks", &[(EventTrigger::Death, "embers")]), firing("embers", &[])],
            trail: None,
            mark: false,
            meshes: Vec::new(),
            decals: Vec::new(),
        };
        let library = EffectLibrary::new(vec![effect]).expect("valid");
        let text = library.to_ron().expect("written");
        assert!(text.contains("on: Death") && text.contains(r#"emitter: "embers""#), "{text}");
        assert_eq!(EffectLibrary::from_ron(&text).expect("reads"), library);
    }

    /// A stateless emitter's form holds the document's curves: at any particle's life its size, opacity and
    /// color are what the document's curves give there, to the bit.
    #[test]
    fn the_stateless_form_holds_the_documents_curves_to_the_bit() {
        use glam::Quat;
        use sapphire_look::vfx::stateless::StatelessInstance;
        let keys: Vec<f32> = vec![0.0, 0.05, 0.2, 0.2, 0.45, 0.7, 0.9, 1.0];
        let emitter = EmitterDesc {
            sim: Sim::Stateless,
            spawn: Spawn::Rate { per_second: 60.0, duration: 0.0 },
            lifetime: (0.5, 1.5),
            size_scale: (1.0, 1.0),
            size: Curve(keys.iter().enumerate().map(|(i, &t)| (t, 0.01 + 0.07 * i as f32)).collect()),
            alpha: Curve(keys.iter().enumerate().map(|(i, &t)| (t, (i % 3) as f32 / 2.0)).collect()),
            color: Curve(
                keys.iter().enumerate().map(|(i, &t)| (t, [i as f32, 0.3, 2.0 - 0.2 * i as f32])).collect(),
            ),
            ..emitter()
        };
        library_of(emitter.clone()).expect("valid");
        assert!(self::emitter().stateless().is_none(), "a CPU emitter has no stateless form");
        let form = emitter.stateless().expect("stateless");
        assert_eq!(form.slots(), 90);
        let mut compared = 0;
        for age in [0.0, 0.4, 1.3, 2.9, 8.25] {
            let instance = StatelessInstance::new(Vec3::new(1.0, 2.0, 3.0), Quat::IDENTITY, age, 9, 0);
            for index in 0..form.slots() {
                let particle = form.particle(&instance, index);
                if !particle.alive {
                    continue;
                }
                let life = particle.life;
                assert_eq!(particle.half_width, emitter.size.sample(life), "{life}");
                assert_eq!(particle.alpha, emitter.alpha.sample(life), "{life}");
                assert_eq!(particle.color.to_array(), emitter.color.sample(life), "{life}");
                compared += 1;
            }
        }
        assert!(compared > 200, "{compared}");
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
        let effect = |name: &str| Effect {
            name: name.to_owned(),
            emitters: vec![emitter()],
            trail: None,
            mark: false,
            meshes: Vec::new(),
            decals: Vec::new(),
        };
        assert_eq!(EffectLibrary::new(vec![effect("")]), Err(EffectLibraryError::EmptyName(0)));
        assert_eq!(
            EffectLibrary::new(vec![effect("a"), effect("a")]),
            Err(EffectLibraryError::DuplicateName("a".to_owned()))
        );
        let nothing = Effect {
            name: "a".to_owned(),
            emitters: vec![],
            trail: None,
            mark: false,
            meshes: Vec::new(),
            decals: Vec::new(),
        };
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

    /// Libraries as each older version wrote them, with what that version brought: emitters and a trail at 1,
    /// a mesh layer at 2, a ground decal at 3 and distortion at 4 (the shipped `hit_spark`, `sword_trail`,
    /// `dust_puff`, `skill_arc`, `telegraph_ground` and `scorch_mark`). They stand for libraries on disk: never
    /// write them again.
    const OLDER: [(u32, &str); 4] = [
        (1, include_str!("../testdata/v1.effects.ron")),
        (2, include_str!("../testdata/v2.effects.ron")),
        (3, include_str!("../testdata/v3.effects.ron")),
        (4, include_str!("../testdata/v4.effects.ron")),
    ];

    /// A library at 1 to 4 reads under 5 and writes back exactly as it was but for its version. A float is
    /// written as the shortest text that reads back to its bits, `-0.0` included, so matching text holds every
    /// value to the bit, and nothing 5 adds is written for a library that doesn't use it.
    #[test]
    fn every_older_version_reads_to_the_bit_and_writes_back_but_for_its_version() {
        for (version, text) in OLDER {
            let library =
                EffectLibrary::from_ron(text).unwrap_or_else(|error| panic!("version {version}: {error}"));
            let written = library.to_ron().expect("written");
            let as_five = text.replacen(&format!("version: {version},"), "version: 5,", 1);
            assert_eq!(written, as_five, "version {version} wrote back otherwise");
            assert_eq!(EffectLibrary::from_ron(&written).expect("reads back"), library, "version {version}");
        }
    }

    /// The shipped library is a version 5 document, since M6 N2's forces: it reads, and its body under every
    /// version's header reads to the same library to the bit (their writes match), while a version past this
    /// build's, or 0, is refused.
    #[test]
    fn the_shipped_library_reads_the_same_under_every_versions_header() {
        let header = "    version: 5,\n";
        assert!(DEFAULT_EFFECTS.contains(header), "the shipped library is at version 5");
        let written = EffectLibrary::default_effects().to_ron().expect("written");
        for version in 1..=EffectLibrary::VERSION {
            let text = DEFAULT_EFFECTS.replacen(header, &format!("    version: {version},\n"), 1);
            let library =
                EffectLibrary::from_ron(&text).unwrap_or_else(|error| panic!("version {version}: {error}"));
            assert_eq!(library.to_ron().expect("written"), written, "version {version}");
        }
        for version in [0, EffectLibrary::VERSION + 1] {
            let text = DEFAULT_EFFECTS.replacen(header, &format!("    version: {version},\n"), 1);
            assert!(EffectLibrary::from_ron(&text).is_err(), "version {version} read");
        }
    }
}
