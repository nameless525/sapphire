//! glTF animations, sampled for the clip cook ([`crate::clip`]).
//!
//! A sidecar's `clips: (…)` on a mesh, or an `importer: GltfClips((…))` of its own, turns this on
//! for a source. Then the document's animations can become clips, named as a material slot is
//! (`#<index>` unnamed, `<name>#<index>` when two share a name), and every node a translation,
//! rotation or scale channel animates joins the skeleton (`animated_nodes`), so a rigid part
//! animated on its own node moves too. An animation source needs no geometry and reads none
//! (`import_rig`); a file's mesh is left where it lies.
//!
//! **Channels.** Translation, rotation and scale go to the node's joint, morph weights to the
//! targets its mesh's weights merged into. A channel on a node outside the imported scene moves
//! nothing, and is dropped. So is a morph channel whose target another channel already drives,
//! which merged targets across a mesh's parts can do: the first in the document wins, and a
//! file's parts should agree.
//!
//! **Time.** A clip starts at the animation's earliest key, wherever that is (Blender's actions may
//! start past 0), and is a whole number of ticks long, the last rounded up, so it never ends
//! before the source. Every channel is sampled at every tick by glTF's rules: held before its
//! first key and after its last, `STEP` and `LINEAR` as named (rotations as a slerp along the
//! short way, on [`sapphire_math::det`], so no platform's libm reaches a cook), `CUBICSPLINE` as a
//! Hermite segment, renormalized for rotations.
//!
//! **Malformed files** are errors, as in [the importer](super) as a whole. Accessors go through
//! the importer's checks before any reader sees them; counts against [`CLIP_BUDGET`] before data
//! is read, and the clip's length and what sampling it holds before anything is sampled, since a
//! few bytes can claim years of keys. Key times must be finite, at least 0 and strictly increasing,
//! as glTF requires, and values finite, and a rotation's key must have a length.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

use ::gltf::animation::util::ReadOutputs;
use ::gltf::animation::{Channel, Interpolation, Property};
use ::gltf::{Accessor, Document, Gltf};
use glam::{Quat, Vec3, Vec4};
use sapphire_assets::clip::MAX_TICKS;
use sapphire_assets::skeleton::{Joint, Skeleton};

use super::{GltfImportError, SceneImport, accessors, load_buffers, parse};
use crate::cache::Stage;
use crate::clip::{self, ClipSettings, SampledClip};
use crate::import;
use crate::import::fbx::FbxImportError;

/// This importer, as cache keys know it. Bump the version whenever sampling changes output for the
/// same file.
pub const IMPORTER: Stage = Stage { name: "gltf clips", version: 1 };

/// How much of an animation one source may bring into a cook, checked before data is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipBudget {
    /// Animations in one file, each a clip to plan and cook.
    pub animations: usize,
    /// Keys in one channel. Far past any real clip: ten minutes at 120 a second is 72,000.
    pub keys: usize,
    /// Floats read for one animation, every channel's times and values together. It is the sum that
    /// counts, not what the file holds: channels may share accessors, and a sparse accessor's values
    /// cost a file nothing.
    pub values: usize,
    /// Floats sampled for one animation, each held in memory until it is reduced: ticks × what the
    /// channels hold a tick, so a morph channel counts each of its targets.
    pub samples: usize,
}

/// The budget of every glTF source's clips. Ten minutes of every channel of a 155-joint character
/// at 60 ticks a second is 53 Mi samples.
pub const CLIP_BUDGET: ClipBudget =
    ClipBudget { animations: 4096, keys: 4 << 20, values: 64 << 20, samples: 64 << 20 };

/// Identifies a channel in error messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelRef {
    pub animation: String,
    pub channel: usize,
}

impl fmt::Display for ChannelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "animation {:?} channel {}", self.animation, self.channel)
    }
}

/// Why an animation can't be sampled.
#[derive(Debug, thiserror::Error)]
pub enum ClipImportError {
    #[error("not a valid glTF 2.0 file")]
    Parse(#[source] ::gltf::Error),
    #[error(transparent)]
    Scene(#[from] GltfImportError),
    #[error("animation {animation:?} has no channels")]
    NoChannels { animation: String },
    #[error("{at}: the {what} accessor {fault}")]
    Accessor { at: ChannelRef, what: &'static str, fault: accessors::AccessorFault },
    #[error("{at}: {found} {what} is over the budget of {budget}")]
    OverBudget { at: ChannelRef, what: &'static str, found: usize, budget: usize },
    #[error("{at}: key {key}'s time isn't finite, at least 0 and past the key before it")]
    KeyTime { at: ChannelRef, key: usize },
    #[error("{at}: {found} output values for {keys} keys, where {expected} are needed")]
    OutputCount { at: ChannelRef, found: usize, keys: usize, expected: usize },
    #[error("{at}: an output value of key {key} isn't finite")]
    NonFinite { at: ChannelRef, key: usize },
    #[error("{at}: key {key}'s rotation has no length")]
    ZeroRotation { at: ChannelRef, key: usize },
    #[error("{at}: animates morph weights on a node whose mesh has no morph targets")]
    NoMorphTargets { at: ChannelRef },
    #[error("{at}: a second channel for the same node and property")]
    DuplicateChannel { at: ChannelRef },
    #[error(
        "animation {animation:?} is {seconds} s long, {ticks} ticks at {rate} a second; a clip holds at \
         most {MAX_TICKS}"
    )]
    TooLong { animation: String, seconds: f32, ticks: f64, rate: u16 },
    #[error("animation {animation:?} needs {found} samples, over the budget of {budget}")]
    TooManySamples { animation: String, found: usize, budget: usize },
    #[error("{found} animations in one file is over the budget of {budget}")]
    TooManyAnimations { found: usize, budget: usize },
    #[error("a morph target index {0} doesn't fit a clip's u16")]
    TooManyTargets(u32),
    #[error("root motion names bone {bone:?}, but the skeleton has no joint of that name")]
    UnknownRootBone { bone: String },
    #[error("root motion from bone {bone:?}: {source}")]
    RootLift { bone: String, source: clip::RootLiftError },
    #[error("root motion needs a skeleton, and animation {animation:?}'s source has none")]
    NoSkeleton { animation: String },
    #[error(transparent)]
    Fbx(#[from] Box<FbxImportError>),
    #[error("animation {animation:?} has a time range from {begin} to {end} s, which no clip can hold")]
    TimeRange { animation: String, begin: f64, end: f64 },
    #[error("animation {animation:?}: joint {joint:?} isn't finite at tick {tick}")]
    NonFiniteSample { animation: String, joint: String, tick: usize },
    #[error("the VRMC_vrm_animation extension is invalid")]
    VrmaExtension(#[source] serde_json::Error),
    #[error("not a VRMA file: it has no VRMC_vrm_animation extension")]
    NotVrma,
    #[error("VRMC_vrm_animation version {0:?} is not supported; Sapphire reads 1.0")]
    VrmaVersion(String),
    #[error("humanoid bone {bone}: {problem}")]
    VrmaBone { bone: String, problem: String },
    #[error("the VRMA's humanoid map has no hips")]
    VrmaNoHips,
}

/// The clip names of `bytes`' animations, in document order: unique, and what each sub-asset's ID
/// is derived from. Called while planning a cook, so it parses and nothing more.
pub fn names(bytes: &[u8]) -> Result<Vec<String>, ClipImportError> {
    let Gltf { document, .. } = parse(bytes).map_err(ClipImportError::Parse)?;
    let found = document.animations().len();
    if found > CLIP_BUDGET.animations {
        return Err(ClipImportError::TooManyAnimations { found, budget: CLIP_BUDGET.animations });
    }
    Ok(names_of(&document))
}

fn names_of(document: &Document) -> Vec<String> {
    let names: Vec<Option<&str>> = document.animations().map(|animation| animation.name()).collect();
    import::slot_names(&names)
}

/// The nodes `document`'s animations move: what joins the skeleton when a source cooks clips, in
/// increasing order. Morph weight channels don't move their nodes.
pub(crate) fn animated_nodes(document: &Document) -> Vec<usize> {
    let mut nodes: Vec<usize> = document
        .animations()
        .flat_map(|animation| animation.channels().collect::<Vec<_>>())
        .filter(|channel| !matches!(channel.target().property(), Property::MorphTargetWeights))
        .map(|channel| channel.target().node().index())
        .collect();
    nodes.sort_unstable();
    nodes.dedup();
    nodes
}

/// A source's skeleton, the targets' default weights and every clip's samples come from one
/// import: this is the scene [`sample`] reads, for a mesh source that cooks clips.
pub(crate) fn import_scene(
    bytes: &[u8],
    base_dir: Option<&std::path::Path>,
) -> Result<(Document, Vec<Vec<u8>>, SceneImport), ClipImportError> {
    let Gltf { document, blob } = parse(bytes).map_err(ClipImportError::Parse)?;
    let buffers = load_buffers(&document, base_dir, blob)?;
    let scene = super::import_scene(&document, &buffers, &animated_nodes(&document))?;
    Ok((document, buffers, scene))
}

/// What [`sample`] reads of an imported document: the joints, and where the importer put its nodes
/// and morph targets.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ClipScene<'a> {
    pub joints: &'a [Joint],
    pub joint_of_node: &'a HashMap<usize, u16>,
    pub targets_of_morph: &'a HashMap<(usize, usize), Vec<u32>>,
}

#[cfg(test)]
impl SceneImport {
    /// The scene as [`sample`] reads it.
    pub(crate) fn clip_scene(&self) -> ClipScene<'_> {
        ClipScene {
            joints: self.mesh.skin.as_ref().map_or(&[], |skin| skin.joints.as_slice()),
            joint_of_node: &self.joint_of_node,
            targets_of_morph: &self.targets_of_morph,
        }
    }
}

/// A document imported for its animations alone: its joints, the skeleton its clips index, and no
/// mesh, whatever the file holds. It has no targets, so morph channels drive nothing.
pub(crate) struct RigImport {
    pub document: Document,
    pub buffers: Vec<Vec<u8>>,
    pub joints: Vec<Joint>,
    pub joint_of_node: HashMap<usize, u16>,
}

/// Imports the animated rig of `bytes`, with every node `extra_joints` finds in the parsed document
/// (a VRMA's humanoid) among its joints.
pub(crate) fn import_rig(
    bytes: &[u8],
    base_dir: Option<&std::path::Path>,
    extra_joints: impl FnOnce(&Document) -> Result<Vec<usize>, ClipImportError>,
) -> Result<RigImport, ClipImportError> {
    let Gltf { document, blob } = parse(bytes).map_err(ClipImportError::Parse)?;
    let buffers = load_buffers(&document, base_dir, blob)?;
    let mut extra = extra_joints(&document)?;
    extra.extend(animated_nodes(&document));
    extra.sort_unstable();
    extra.dedup();
    let (joints, joint_of_node) = super::scene_joints(&document, &extra)?.unwrap_or_default();
    Ok(RigImport { document, buffers, joints, joint_of_node })
}

/// What planning a cook needs from an animation source: its clips' names, and the joints of its
/// rig, with every node `extra_joints` finds among them. Reads no buffer.
pub(crate) fn plan(
    bytes: &[u8],
    extra_joints: impl FnOnce(&Document) -> Result<Vec<usize>, ClipImportError>,
) -> Result<(Vec<String>, Vec<Joint>), ClipImportError> {
    let Gltf { document, .. } = parse(bytes).map_err(ClipImportError::Parse)?;
    let found = document.animations().len();
    if found > CLIP_BUDGET.animations {
        return Err(ClipImportError::TooManyAnimations { found, budget: CLIP_BUDGET.animations });
    }
    let mut extra = extra_joints(&document)?;
    extra.extend(animated_nodes(&document));
    extra.sort_unstable();
    extra.dedup();
    let joints = super::scene_joints(&document, &extra)?;
    Ok((names_of(&document), joints.map(|(joints, _)| joints).unwrap_or_default()))
}

/// One channel's keys as read: `width` values a key, three times that for cubic segments.
struct Keys {
    times: Vec<f32>,
    values: Vec<f32>,
    width: usize,
    interpolation: Interpolation,
}

impl Keys {
    /// The value of key `key`, without its tangents.
    fn value(&self, key: usize) -> &[f32] {
        let at = match self.interpolation {
            Interpolation::CubicSpline => 3 * key + 1,
            Interpolation::Linear | Interpolation::Step => key,
        };
        &self.values[at * self.width..][..self.width]
    }

    /// Key `key`'s in and out tangents, per second.
    fn tangents(&self, key: usize) -> (&[f32], &[f32]) {
        (
            &self.values[3 * key * self.width..][..self.width],
            &self.values[(3 * key + 2) * self.width..][..self.width],
        )
    }

    /// The channel at `time`, into `out`, moving `cursor` (the last key at or before `time`)
    /// forward: a walk over increasing times visits every key once.
    fn sample(&self, time: f32, cursor: &mut usize, rotation: bool, out: &mut [f32]) {
        while *cursor + 1 < self.times.len() && self.times[*cursor + 1] <= time {
            *cursor += 1;
        }
        let key = *cursor;
        if time <= self.times[0] || key + 1 == self.times.len() {
            out.copy_from_slice(self.value(key));
            return;
        }
        let span = self.times[key + 1] - self.times[key];
        let u = (time - self.times[key]) / span;
        match self.interpolation {
            Interpolation::Step => out.copy_from_slice(self.value(key)),
            Interpolation::Linear if rotation => {
                let turned = clip::slerp(quat(self.value(key)), quat(self.value(key + 1)), u);
                out.copy_from_slice(&turned.to_array());
            }
            Interpolation::Linear => {
                for (slot, (a, b)) in out.iter_mut().zip(self.value(key).iter().zip(self.value(key + 1))) {
                    *slot = a + (b - a) * u;
                }
            }
            Interpolation::CubicSpline => {
                let (u2, u3) = (u * u, u * u * u);
                let (from, to) = (self.value(key), self.value(key + 1));
                let (_, leaving) = self.tangents(key);
                let (arriving, _) = self.tangents(key + 1);
                for component in 0..self.width {
                    out[component] = (2.0 * u3 - 3.0 * u2 + 1.0) * from[component]
                        + (u3 - 2.0 * u2 + u) * span * leaving[component]
                        + (-2.0 * u3 + 3.0 * u2) * to[component]
                        + (u3 - u2) * span * arriving[component];
                }
                if rotation {
                    let blended = Vec4::from_slice(out);
                    // A segment through the origin has no direction there: hold the key before.
                    let turned = if blended.length_squared() > 1e-12 {
                        blended.normalize()
                    } else {
                        Vec4::from_slice(from)
                    };
                    out.copy_from_slice(&turned.to_array());
                }
            }
        }
    }
}

fn quat(value: &[f32]) -> Quat {
    Quat::from_vec4(Vec4::from_slice(value))
}

/// What a channel drives.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Driven {
    Rotation(u16),
    Translation(u16),
    Scale(u16),
    /// A mesh's morph weights; its channel's keys hold one per target.
    Weights {
        mesh: usize,
    },
}

/// Samples animation `index` of a document with its `scene`, under `settings`. The result is ready
/// for [`clip::cook`], its root motion lifted.
pub(crate) fn sample(
    document: &Document,
    buffers: &[Vec<u8>],
    scene: ClipScene<'_>,
    index: usize,
    settings: &ClipSettings,
) -> Result<SampledClip, ClipImportError> {
    sample_within(document, buffers, scene, index, settings, CLIP_BUDGET)
}

/// [`sample`], within `budget`.
fn sample_within(
    document: &Document,
    buffers: &[Vec<u8>],
    scene: ClipScene<'_>,
    index: usize,
    settings: &ClipSettings,
    budget: ClipBudget,
) -> Result<SampledClip, ClipImportError> {
    let name = names_of(document).swap_remove(index);
    let animation = document.animations().nth(index).expect("names and animations are one list");
    if animation.channels().next().is_none() {
        return Err(ClipImportError::NoChannels { animation: name });
    }
    let mut tracks: Vec<(Driven, Keys)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut floats_read = 0;
    for channel in animation.channels() {
        let at = ChannelRef { animation: name.clone(), channel: channel.index() };
        let target = channel.target();
        let node = target.node();
        if !seen.insert((node.index(), property_name(&target.property()))) {
            return Err(ClipImportError::DuplicateChannel { at });
        }
        let driven = match target.property() {
            Property::Translation => scene.joint_of_node.get(&node.index()).copied().map(Driven::Translation),
            Property::Rotation => scene.joint_of_node.get(&node.index()).copied().map(Driven::Rotation),
            Property::Scale => scene.joint_of_node.get(&node.index()).copied().map(Driven::Scale),
            Property::MorphTargetWeights => {
                let targets = node.mesh().map_or(0, |mesh| morph_targets(&mesh));
                if targets == 0 {
                    return Err(ClipImportError::NoMorphTargets { at });
                }
                // Only a mesh the scene instantiates has targets to drive.
                node.mesh()
                    .filter(|mesh| scene.targets_of_morph.contains_key(&(mesh.index(), 0)))
                    .map(|mesh| Driven::Weights { mesh: mesh.index() })
            }
        };
        // Read even if it drives nothing, so a bad channel is reported whatever it targets.
        let keys = read_keys(&channel, &at, buffers, budget, &mut floats_read)?;
        if let Some(driven) = driven {
            tracks.push((driven, keys));
        }
    }

    let start = tracks.iter().map(|(_, keys)| keys.times[0]).fold(f32::INFINITY, f32::min);
    let end = tracks.iter().map(|(_, keys)| keys.times[keys.times.len() - 1]).fold(0.0, f32::max);
    let seconds = if tracks.is_empty() { 0.0 } else { end - start };
    let rate = f64::from(settings.rate);
    // A hair over a whole tick is the source's rounding, not a tick more.
    let ticks = (f64::from(seconds) * rate - 1e-3).ceil().max(0.0);
    if ticks > f64::from(MAX_TICKS) {
        return Err(ClipImportError::TooLong { animation: name, seconds, ticks, rate: settings.rate });
    }
    let ticks = ticks as u16;
    let count = usize::from(ticks) + 1;
    let per_tick: usize = tracks
        .iter()
        .map(|(driven, keys)| match driven {
            // The channel's, and then those of every target its mesh's weights merged into.
            Driven::Weights { mesh } => {
                let merged = (0..keys.width).filter_map(|morph| scene.targets_of_morph.get(&(*mesh, morph)));
                keys.width.saturating_add(merged.map(Vec::len).sum())
            }
            Driven::Rotation(_) | Driven::Translation(_) | Driven::Scale(_) => keys.width,
        })
        .sum();
    let samples = count.saturating_mul(per_tick);
    if samples > budget.samples {
        return Err(ClipImportError::TooManySamples {
            animation: name,
            found: samples,
            budget: budget.samples,
        });
    }

    let time_of = |tick: usize| start + (tick as f64 / rate) as f32;
    let mut rotations = BTreeMap::new();
    let mut translations = BTreeMap::new();
    let mut scales = BTreeMap::new();
    let mut weights = BTreeMap::new();
    for (driven, keys) in tracks {
        let mut cursor = 0;
        let rotation = matches!(driven, Driven::Rotation(_));
        let mut sampled = vec![0.0; count * keys.width];
        for (tick, value) in sampled.chunks_exact_mut(keys.width).enumerate() {
            keys.sample(time_of(tick), &mut cursor, rotation, value);
        }
        let values = || sampled.chunks_exact(keys.width);
        match driven {
            Driven::Rotation(joint) => {
                rotations.insert(joint, values().map(quat).collect());
            }
            Driven::Translation(joint) => {
                translations.insert(joint, values().map(Vec3::from_slice).collect());
            }
            Driven::Scale(joint) => {
                scales.insert(joint, values().map(Vec3::from_slice).collect());
            }
            Driven::Weights { mesh } => {
                for morph in 0..keys.width {
                    let Some(merged) = scene.targets_of_morph.get(&(mesh, morph)) else { continue };
                    for &target in merged {
                        let target =
                            u16::try_from(target).map_err(|_| ClipImportError::TooManyTargets(target))?;
                        if let Entry::Vacant(free) = weights.entry(target) {
                            free.insert(values().map(|value| value[morph]).collect());
                        }
                    }
                }
            }
        }
    }

    let mut clip = SampledClip {
        name,
        rate: settings.rate,
        ticks,
        rotations: rotations.into_iter().collect(),
        translations: translations.into_iter().collect(),
        scales: scales.into_iter().collect(),
        weights: weights.into_iter().collect(),
        root: None,
    };
    lift_settings_root(&mut clip, scene.joints, settings)?;
    Ok(clip)
}

/// Lifts root motion out of `clip`, if `settings` ask for it.
pub(crate) fn lift_settings_root(
    clip: &mut SampledClip,
    joints: &[Joint],
    settings: &ClipSettings,
) -> Result<(), ClipImportError> {
    let Some(root) = &settings.root_motion else { return Ok(()) };
    if joints.is_empty() {
        return Err(ClipImportError::NoSkeleton { animation: clip.name.clone() });
    }
    let skeleton = Skeleton::new(joints);
    let joint = skeleton.names.iter().position(|joint| *joint == root.bone);
    let Some(joint) = joint else {
        return Err(ClipImportError::UnknownRootBone { bone: root.bone.clone() });
    };
    clip::lift_root(clip, &skeleton, joint as u16, root.translation, root.rotation)
        .map_err(|source| ClipImportError::RootLift { bone: root.bone.clone(), source })
}

fn property_name(property: &Property) -> &'static str {
    match property {
        Property::Translation => "translation",
        Property::Rotation => "rotation",
        Property::Scale => "scale",
        Property::MorphTargetWeights => "weights",
    }
}

/// How many morph targets a mesh's primitives have: its first's, as glTF has them all agree.
fn morph_targets(mesh: &::gltf::Mesh<'_>) -> usize {
    mesh.primitives().next().map_or(0, |primitive| primitive.morph_targets().len())
}

/// `channel`'s keys, every value checked. Its accessors and its counts are checked before a reader
/// sees them.
fn read_keys(
    channel: &Channel<'_>,
    at: &ChannelRef,
    buffers: &[Vec<u8>],
    budget: ClipBudget,
    floats_read: &mut usize,
) -> Result<Keys, ClipImportError> {
    let sampler = channel.sampler();
    let (input, output): (Accessor<'_>, Accessor<'_>) = (sampler.input(), sampler.output());
    let interpolation = sampler.interpolation();
    let property = channel.target().property();
    let (kind, width) = match property {
        Property::Translation | Property::Scale => (&accessors::ANIMATION_VEC3, 3),
        Property::Rotation => (&accessors::ANIMATION_ROTATION, 4),
        Property::MorphTargetWeights => {
            let targets = channel.target().node().mesh().map_or(0, |mesh| morph_targets(&mesh));
            (&accessors::ANIMATION_WEIGHT, targets)
        }
    };
    let over = |what, found, budget| ClipImportError::OverBudget { at: at.clone(), what, found, budget };
    if input.count() > budget.keys {
        return Err(over("keys", input.count(), budget.keys));
    }
    let per_value = match property {
        Property::Translation | Property::Scale => 3,
        Property::Rotation => 4,
        Property::MorphTargetWeights => 1,
    };
    let floats = input.count().saturating_add(output.count().saturating_mul(per_value));
    *floats_read = floats_read.saturating_add(floats);
    if *floats_read > budget.values {
        return Err(over("floats read for its animation", *floats_read, budget.values));
    }
    let accessor = |what, fault| ClipImportError::Accessor { at: at.clone(), what, fault };
    accessors::check(&input, &accessors::ANIMATION_TIME, buffers)
        .map_err(|fault| accessor("input", fault))?;
    accessors::check(&output, kind, buffers).map_err(|fault| accessor("output", fault))?;
    let per_key = match interpolation {
        Interpolation::CubicSpline => 3,
        Interpolation::Linear | Interpolation::Step => 1,
    };
    // A vector's elements are its values; morph weights are scalars, `width` of them a key.
    let elements = if matches!(property, Property::MorphTargetWeights) { per_key * width } else { per_key };
    let expected = input.count().saturating_mul(elements);
    if output.count() != expected {
        return Err(ClipImportError::OutputCount {
            at: at.clone(),
            found: output.count(),
            keys: input.count(),
            expected,
        });
    }

    let reader = channel.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice));
    let unreadable = |what| accessor(what, accessors::AccessorFault::Empty);
    let times: Vec<f32> = reader.read_inputs().ok_or_else(|| unreadable("input"))?.collect();
    let mut previous = None;
    for (key, &time) in times.iter().enumerate() {
        if !time.is_finite() || time < 0.0 || previous.is_some_and(|previous| time <= previous) {
            return Err(ClipImportError::KeyTime { at: at.clone(), key });
        }
        previous = Some(time);
    }
    let mut values: Vec<f32> = match reader.read_outputs().ok_or_else(|| unreadable("output"))? {
        ReadOutputs::Translations(values) | ReadOutputs::Scales(values) => values.flatten().collect(),
        ReadOutputs::Rotations(values) => values.into_f32().flatten().collect(),
        ReadOutputs::MorphTargetWeights(values) => values.into_f32().collect(),
    };
    let stride = width * per_key;
    if let Some(bad) = values.iter().position(|value| !value.is_finite()) {
        return Err(ClipImportError::NonFinite { at: at.clone(), key: bad / stride.max(1) });
    }
    if matches!(property, Property::Rotation) {
        for (key, chunk) in values.chunks_exact_mut(stride).enumerate() {
            let value = match interpolation {
                Interpolation::CubicSpline => &mut chunk[4..8],
                Interpolation::Linear | Interpolation::Step => &mut chunk[..4],
            };
            let rotation = Vec4::from_slice(value);
            if rotation.length_squared() < 1e-6 {
                return Err(ClipImportError::ZeroRotation { at: at.clone(), key });
            }
            value.copy_from_slice(&rotation.normalize().to_array());
        }
    }
    Ok(Keys { times, values, width, interpolation })
}

#[cfg(test)]
pub(crate) mod tests;
