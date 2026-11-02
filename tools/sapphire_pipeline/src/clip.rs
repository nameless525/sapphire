//! Clips, the step between what a source authored and `sapphire_assets::clip`'s format: every
//! animated channel sampled once a tick ([`SampledClip`]), root motion lifted out, then key
//! reduction and packing into a [`Clip`].
//!
//! Two sources feed this ([`gltf::clips`](crate::import::gltf::clips) and [`keypose`]). Each samples
//! its own curves (glTF's step, linear and cubic segments, a document's eases) at every tick, so
//! what follows, and the runtime's one interpolation between keys, needn't know either.
//!
//! # Key reduction ([`cook`])
//!
//! Each track keeps the fewest keys this finds that stay within its tolerance at every tick, with
//! the keys as they will decode: Ramer–Douglas–Peucker, which keeps the two ends, then the sample
//! furthest from the line between its neighbors, until none strays. Keeping extremes first suits
//! authored action, whose key poses are exactly those. It is deterministic: it reads only the
//! samples, in order, and on a tie keeps the earliest.
//!
//! **The stated bound.** [`ReductionError::distance`] (0.25 mm by default) is how far one track may
//! move what it drives, at any tick:
//!
//! - a translation is within it in meters, as its parent's scale at rest takes it to model space
//!   (a track under an armature scaled a hundredth holds a hundred times as much in its own
//!   centimeters);
//! - a rotation is within `distance / reach` radians, and a scale within `distance / reach` as a
//!   ratio, where a joint's reach is its longest chain of bones down the skeleton at rest, plus
//!   [`SKIN_REACH`] for the skin past the last joint. So a track on the hips, a meter from the
//!   fingertips, holds 0.014°, and one on a finger up to 0.14°.
//!
//! Errors add along a chain, so a joint at depth `n` is within `3 × n × distance` of the source in
//! model space, and in practice well under, since they'd have to peak together at one tick and in
//! one direction. Morph weights are within [`ReductionError::weight`] (0.002). A key's own
//! quantization counts toward each of these, except where it alone exceeds a tolerance: the
//! tolerance then is the quantization's (a rotation key's 0.009°; half a step across a vector
//! track's range). Between ticks a clip blends linearly, so the bound there is the source's own
//! curvature over a tick. `clip::tests::the_reconstruction_stays_within_the_stated_bound` holds
//! all of this.
//!
//! A track within tolerance of the skeleton's rest (or a morph target's default) at every tick
//! needs no track at all; within tolerance of one value, one key.
//!
//! # Root motion ([`lift_root`])
//!
//! See `sapphire_assets::clip` for what the track means. The named bone's motion in model space
//! becomes `root(t)`, a turn about +Y and a translation from the first tick, and the bone's own
//! tracks keep the rest, so `root(t) × pose(t)` is the motion as authored, for everything under the
//! bone. (Joints outside its subtree move with `root(t)` too: name a bone above whatever should
//! travel.) That holds exactly under uniform scale along the bone's chain, which is what armatures
//! have; a shear the bone's own transform can't hold is lost from it.

use glam::{Affine3A, Quat, Vec3, Vec4};
use sapphire_assets::AssetId;
use sapphire_assets::clip::{
    Clip, DEFAULT_RATE, MAX_RATE, PackedRotation, RootMotion, RotationTrack, VectorRange, VectorTrack,
    WeightRange, WeightTrack, lerp3, nlerp,
};
use sapphire_assets::skeleton::{NO_PARENT, Skeleton};
use sapphire_math::det;
use serde::{Deserialize, Serialize};

use crate::cache::Stage;
use crate::import::gltf::clips as gltf_clips;
use crate::keypose;

/// Key reduction and the `.sclip` writer. Bump its version whenever they change output for the
/// same samples.
pub const COOKER: Stage = Stage { name: "sclip", version: 1 };

/// The `.sskel` writer. Bump its version whenever it changes output for the same import.
pub const SKELETON_COOKER: Stage = Stage { name: "sskel", version: 1 };

/// How far skin reaches past a joint with no joints below it, in meters: a fingertip's pad, a hair
/// strand's end. Without it a leaf joint's rotation would have no limit.
pub const SKIN_REACH: f32 = 0.1;

/// The most a rotation key's quantization is off by, in radians: a tighter tolerance only keeps
/// every key to no gain.
const ROTATION_KEY_ERROR: f32 = 1.5e-4;

/// The tightest a scale's tolerance goes, as a ratio.
const MIN_SCALE_TOLERANCE: f32 = 1e-5;

/// How close a clip must stay to its source: see the [module docs](self). Zero keeps what
/// quantization allows.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReductionError {
    /// How far one track may move what it drives, in meters.
    pub distance: f32,
    /// How far a morph weight may stray.
    pub weight: f32,
}

impl Default for ReductionError {
    fn default() -> Self {
        Self { distance: 0.000_25, weight: 0.002 }
    }
}

impl ReductionError {
    /// Whether both are finite and not negative.
    pub fn is_valid(&self) -> bool {
        self.distance.is_finite() && self.distance >= 0.0 && self.weight.is_finite() && self.weight >= 0.0
    }
}

/// How a mesh source's animations cook: `clips: (root_motion: (bone: "Hips"))` in its sidecar. Each
/// animation becomes a [`Clip`], a sub-asset with an ID derived from its name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipSettings {
    /// Ticks per second to sample at. 60, the simulation's tick, unless a source's motion needs
    /// more.
    pub rate: u16,
    pub error: ReductionError,
    /// Lifts this bone's motion into a track of its own, for every animation of the source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_motion: Option<RootMotionSettings>,
}

impl Default for ClipSettings {
    fn default() -> Self {
        Self { rate: DEFAULT_RATE, error: ReductionError::default(), root_motion: None }
    }
}

impl ClipSettings {
    /// Whether the rate and the error bounds are ones a cook can honor.
    pub fn is_valid(&self) -> bool {
        (1..=MAX_RATE).contains(&self.rate) && self.error.is_valid()
    }
}

/// Which bone root motion comes from, and how much of its motion. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootMotionSettings {
    /// The joint's name in the skeleton: the node's, in glTF.
    pub bone: String,
    #[serde(default)]
    pub translation: RootTranslation,
    #[serde(default)]
    pub rotation: RootRotation,
}

/// Which of the root bone's translation becomes root motion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootTranslation {
    /// Across the ground (X and Z): height stays in the bone's track, where a run's bob belongs.
    #[default]
    Ground,
    /// All of it, for a jump whose height is the character's own.
    All,
}

/// Which of the root bone's rotation becomes root motion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootRotation {
    /// Its turn about +Y.
    #[default]
    Yaw,
    /// None: facing doesn't change with the clip.
    None,
}

/// A clip with every animated channel sampled at each of its ticks, before reduction. Each list is
/// by increasing joint or target, and each track holds `ticks + 1` samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SampledClip {
    pub name: String,
    pub rate: u16,
    pub ticks: u16,
    pub rotations: Vec<(u16, Vec<Quat>)>,
    pub translations: Vec<(u16, Vec<Vec3>)>,
    pub scales: Vec<(u16, Vec<Vec3>)>,
    pub weights: Vec<(u16, Vec<f32>)>,
    pub root: Option<SampledRoot>,
}

/// Root motion, sampled like the tracks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SampledRoot {
    pub joint: u16,
    pub translation: Vec<Vec3>,
    pub rotation: Vec<Quat>,
}

/// Why a source can't become a clip, whichever kind of source it is.
#[derive(Debug, thiserror::Error)]
pub enum ClipCookError {
    #[error(transparent)]
    Gltf(#[from] gltf_clips::ClipImportError),
    #[error(transparent)]
    KeyPose(#[from] keypose::KeyPoseError),
    #[error("the cooked clip is invalid")]
    Invalid(#[from] sapphire_assets::clip::ClipError),
    #[error("the cooked skeleton is invalid")]
    InvalidSkeleton(#[from] sapphire_assets::skeleton::SkeletonError),
    #[error("writing the clip")]
    Cooked(#[from] sapphire_serialize::cooked::CookedError),
    #[error("the source has no skeleton to cook")]
    NoSkeleton,
    #[error("the sidecar's `clips` settings are for glTF meshes; {importer} sources don't cook clips yet")]
    ClipsUnsupported { importer: &'static str },
    #[error(
        "the sidecar's `clips` settings are out of range: a rate from 1 to {MAX_RATE}, errors at least 0"
    )]
    Settings,
}

/// What a clip's tracks index: the skeleton (and its ID), for a clip that animates joints, and the
/// default weight of each of the mesh's morph targets, for one that animates those.
#[derive(Clone, Copy, Debug)]
pub struct ClipTarget<'a> {
    pub skeleton: Option<(AssetId, &'a Skeleton)>,
    pub morph_defaults: &'a [f32],
}

/// The reach of each joint: the longest chain of bones below it at rest, in model space, plus
/// [`SKIN_REACH`]. See the [module docs](self).
pub fn reaches(skeleton: &Skeleton) -> Vec<f32> {
    let model = rest_model(skeleton);
    let mut chain = vec![0.0f32; skeleton.parents.len()];
    // Children come after their parents, so walking backward finishes each joint's chain first.
    for joint in (0..chain.len()).rev() {
        let parent = skeleton.parents[joint];
        if parent != NO_PARENT {
            let parent = usize::from(parent);
            let bone = Vec3::from(model[joint].translation).distance(Vec3::from(model[parent].translation));
            chain[parent] = chain[parent].max(chain[joint] + bone);
        }
    }
    chain.iter().map(|chain| chain + SKIN_REACH).collect()
}

/// Each joint's model-space rest transform.
pub(crate) fn rest_model(skeleton: &Skeleton) -> Vec<Affine3A> {
    let mut model: Vec<Affine3A> = Vec::with_capacity(skeleton.parents.len());
    for (joint, rest) in skeleton.rest.iter().enumerate() {
        let local = Affine3A::from_scale_rotation_translation(
            Vec3::from(rest.scale),
            Quat::from_array(rest.rotation).normalize(),
            Vec3::from(rest.translation),
        );
        let parent = skeleton.parents[joint];
        model.push(if parent == NO_PARENT { local } else { model[usize::from(parent)] * local });
    }
    model
}

/// A slerp of unit quaternions along the short way, for `weight` outside 0 to 1 as well, which an
/// ease's anticipation and overshoot reach. Built on [`det`], so a cook's output doesn't depend on
/// the platform's libm (`Quat::slerp` does).
pub(crate) fn slerp(a: Quat, b: Quat, weight: f32) -> Quat {
    let (a, b) = (Vec4::from(a), Vec4::from(b));
    let dot = a.dot(b);
    let (b, dot) = if dot < 0.0 { (-b, -dot) } else { (b, dot) };
    // Too close to divide by the angle's sine.
    if dot > 0.9995 {
        return Quat::from_vec4(a.lerp(b, weight)).normalize();
    }
    let angle = det::acos(dot.min(1.0));
    let sine = det::sin(angle);
    let (from, to) = (det::sin((1.0 - weight) * angle) / sine, det::sin(weight * angle) / sine);
    Quat::from_vec4(a * from + b * to).normalize()
}

/// Why a root bone's motion can't be lifted.
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error(
    "the bone or a joint above it has a scale of zero in the clip, so the rest of its motion has no value"
)]
pub struct RootLiftError;

/// Lifts the motion of `joint` out of `sampled`, as the settings say: see the [module docs](self).
/// Leaves `sampled` as it was if it can't.
pub fn lift_root(
    sampled: &mut SampledClip,
    skeleton: &Skeleton,
    joint: u16,
    translation: RootTranslation,
    rotation: RootRotation,
) -> Result<(), RootLiftError> {
    let count = usize::from(sampled.ticks) + 1;
    let index = usize::from(joint);
    // The joint and its ancestors, root first.
    let mut chain = vec![index];
    while let Some(parent) = parent_of(skeleton, chain[chain.len() - 1]) {
        chain.push(parent);
    }
    chain.reverse();
    let local = |sampled: &SampledClip, joint: usize, tick: usize| -> Affine3A {
        let rest = &skeleton.rest[joint];
        let id = joint as u16;
        let track = |list: &'_ [(u16, Vec<Vec3>)]| {
            list.iter().find(|(j, _)| *j == id).map(|(_, samples)| samples[tick])
        };
        let turned = sampled.rotations.iter().find(|(j, _)| *j == id).map(|(_, samples)| samples[tick]);
        Affine3A::from_scale_rotation_translation(
            track(&sampled.scales).unwrap_or(Vec3::from(rest.scale)),
            turned.unwrap_or(Quat::from_array(rest.rotation).normalize()),
            track(&sampled.translations).unwrap_or(Vec3::from(rest.translation)),
        )
    };
    let mut parents = Vec::with_capacity(count);
    let mut models = Vec::with_capacity(count);
    for tick in 0..count {
        let mut parent = Affine3A::IDENTITY;
        for &ancestor in &chain[..chain.len() - 1] {
            parent *= local(sampled, ancestor, tick);
        }
        parents.push(parent);
        models.push(parent * local(sampled, index, tick));
    }

    let (_, first_rotation, first_position) = models[0].to_scale_rotation_translation();
    let mut root_translation = Vec::with_capacity(count);
    let mut root_rotation = Vec::with_capacity(count);
    let mut bone_rotation = Vec::with_capacity(count);
    let mut bone_translation = Vec::with_capacity(count);
    for tick in 0..count {
        let (_, model_rotation, position) = models[tick].to_scale_rotation_translation();
        let turn = match rotation {
            RootRotation::Yaw => yaw_of(model_rotation * first_rotation.inverse()),
            RootRotation::None => Quat::IDENTITY,
        };
        // What would stay where it was, turning with the root: the rest of the way is the
        // translation, so the bone keeps its first position across whatever is lifted.
        let moved = position - turn * first_position;
        let lifted = match translation {
            RootTranslation::Ground => Vec3::new(moved.x, 0.0, moved.z),
            RootTranslation::All => moved,
        };
        let root = Affine3A::from_rotation_translation(turn, lifted);
        let residual = parents[tick].inverse() * root.inverse() * models[tick];
        let (_, kept_rotation, kept_translation) = residual.to_scale_rotation_translation();
        root_translation.push(lifted);
        root_rotation.push(turn);
        bone_rotation.push(kept_rotation);
        bone_translation.push(kept_translation);
    }
    let finite = root_translation.iter().chain(&bone_translation).all(|value| value.is_finite())
        && root_rotation.iter().chain(&bone_rotation).all(|value| value.is_finite());
    if !finite {
        return Err(RootLiftError);
    }
    set_track(&mut sampled.rotations, joint, bone_rotation);
    set_track(&mut sampled.translations, joint, bone_translation);
    sampled.root = Some(SampledRoot { joint, translation: root_translation, rotation: root_rotation });
    Ok(())
}

fn parent_of(skeleton: &Skeleton, joint: usize) -> Option<usize> {
    let parent = skeleton.parents[joint];
    (parent != NO_PARENT).then_some(usize::from(parent))
}

/// `rotation`'s turn about +Y: its swing-twist decomposition's twist.
fn yaw_of(rotation: Quat) -> Quat {
    let twist = Vec4::new(0.0, rotation.y, 0.0, rotation.w);
    if twist.length_squared() < 1e-12 { Quat::IDENTITY } else { Quat::from_vec4(twist).normalize() }
}

/// Replaces `joint`'s track in `tracks`, keeping them in joint order.
fn set_track<T>(tracks: &mut Vec<(u16, Vec<T>)>, joint: u16, samples: Vec<T>) {
    match tracks.binary_search_by_key(&joint, |(track, _)| *track) {
        Ok(found) => tracks[found].1 = samples,
        Err(place) => tracks.insert(place, (joint, samples)),
    }
}

/// The keys of `count` samples to keep, and the most any strays by. `error(a, b, i)` is how far
/// sample `i` lies from the blend between kept keys `a` and `b` (see the [module docs](self)).
fn simplify(count: usize, tolerance: f32, error: impl Fn(usize, usize, usize) -> f32) -> Vec<usize> {
    if (0..count).all(|sample| error(0, 0, sample) <= tolerance) {
        return vec![0];
    }
    let mut keep = vec![false; count];
    keep[0] = true;
    keep[count - 1] = true;
    let mut spans = vec![(0, count - 1)];
    while let Some((first, last)) = spans.pop() {
        let mut worst = (tolerance, None);
        for sample in first + 1..last {
            let strays = error(first, last, sample);
            if strays > worst.0 {
                worst = (strays, Some(sample));
            }
        }
        if let (_, Some(sample)) = worst {
            keep[sample] = true;
            spans.push((first, sample));
            spans.push((sample, last));
        }
    }
    (0..count).filter(|&sample| keep[sample]).collect()
}

/// How far from `first` to `last` key the sample at index `sample` lies.
fn blend(first: usize, last: usize, sample: usize) -> f32 {
    if last == first { 0.0 } else { (sample - first) as f32 / (last - first) as f32 }
}

/// The angle between unit quaternions in radians: from the chord between them, which keeps its
/// precision at the tiny angles tolerances are, where a dot product of `f32`s has none.
fn angle(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot: f32 = a.iter().zip(&b).map(|(a, b)| a * b).sum();
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let chord = a.iter().zip(&b).map(|(a, b)| (a - b * sign) * (a - b * sign)).sum::<f32>().sqrt();
    // 4·asin(c/2), to the terms that count below a few degrees.
    2.0 * chord * (1.0 + chord * chord / 24.0)
}

fn distance3(a: [f32; 3], b: [f32; 3]) -> f32 {
    Vec3::from(a).distance(Vec3::from(b))
}

/// What the tracks of a clip write to, as it is built.
#[derive(Default)]
struct KeyArrays {
    rotation_ticks: Vec<u16>,
    rotation_keys: Vec<PackedRotation>,
    vector_ticks: Vec<u16>,
    vector_keys: Vec<sapphire_assets::clip::PackedVector>,
    weight_ticks: Vec<u16>,
    weight_keys: Vec<u16>,
}

impl KeyArrays {
    /// Reduces rotation samples to a track, or to nothing if they stay within `tolerance` of `rest`.
    fn rotation(
        &mut self,
        joint: u16,
        samples: &[Quat],
        rest: Option<[f32; 4]>,
        tolerance: f32,
    ) -> Option<RotationTrack> {
        let source: Vec<[f32; 4]> = samples.iter().map(|q| q.normalize().to_array()).collect();
        let tolerance = tolerance.max(ROTATION_KEY_ERROR);
        if let Some(rest) = rest
            && source.iter().all(|&sample| angle(sample, rest) <= tolerance)
        {
            return None;
        }
        let keys: Vec<PackedRotation> = source.iter().map(|&q| PackedRotation::encode(q)).collect();
        let decoded: Vec<[f32; 4]> = keys.iter().map(|key| key.decode()).collect();
        let kept = simplify(source.len(), tolerance, |first, last, sample| {
            angle(nlerp(decoded[first], decoded[last], blend(first, last, sample)), source[sample])
        });
        let first = self.rotation_keys.len() as u32;
        for &sample in &kept {
            self.rotation_ticks.push(sample as u16);
            self.rotation_keys.push(keys[sample]);
        }
        Some(RotationTrack { joint, count: kept.len() as u16, first })
    }

    /// Reduces translation or scale samples to a track, or to nothing if they stay within
    /// `tolerance` of `rest`.
    fn vector(
        &mut self,
        joint: u16,
        samples: &[Vec3],
        rest: Option<[f32; 3]>,
        tolerance: f32,
    ) -> Option<VectorTrack> {
        let source: Vec<[f32; 3]> = samples.iter().map(|v| v.to_array()).collect();
        if let Some(rest) = rest
            && source.iter().all(|&sample| distance3(sample, rest) <= tolerance)
        {
            return None;
        }
        let range = VectorRange::enclosing(source.iter().copied());
        // Half a step along every axis is the least a key can promise.
        let step = Vec3::from(range.step).length();
        let tolerance = tolerance.max(step * 0.5);
        let keys: Vec<_> = source.iter().map(|&value| range.encode(value)).collect();
        let decoded: Vec<[f32; 3]> = keys.iter().map(|&key| range.decode(key)).collect();
        let kept = simplify(source.len(), tolerance, |first, last, sample| {
            distance3(lerp3(decoded[first], decoded[last], blend(first, last, sample)), source[sample])
        });
        let first = self.vector_keys.len() as u32;
        for &sample in &kept {
            self.vector_ticks.push(sample as u16);
            self.vector_keys.push(keys[sample]);
        }
        Some(VectorTrack { joint, count: kept.len() as u16, first, range })
    }

    /// Reduces weight samples to a track, or to nothing if they stay within `tolerance` of the
    /// target's default.
    fn weight(&mut self, target: u16, samples: &[f32], default: f32, tolerance: f32) -> Option<WeightTrack> {
        if samples.iter().all(|sample| (sample - default).abs() <= tolerance) {
            return None;
        }
        let range = WeightRange::enclosing(samples.iter().copied());
        let tolerance = tolerance.max(range.step * 0.5);
        let keys: Vec<u16> = samples.iter().map(|&value| range.encode(value)).collect();
        let decoded: Vec<f32> = keys.iter().map(|&key| range.decode(key)).collect();
        let kept = simplify(samples.len(), tolerance, |first, last, sample| {
            let (from, to) = (decoded[first], decoded[last]);
            (from + (to - from) * blend(first, last, sample) - samples[sample]).abs()
        });
        let first = self.weight_keys.len() as u32;
        for &sample in &kept {
            self.weight_ticks.push(sample as u16);
            self.weight_keys.push(keys[sample]);
        }
        Some(WeightTrack { target, count: kept.len() as u16, first, range })
    }
}

/// Reduces and packs `sampled` for `target`: see the [module docs](self).
pub fn cook(sampled: &SampledClip, target: &ClipTarget<'_>, error: &ReductionError) -> Clip {
    let skeleton = target.skeleton.map(|(_, skeleton)| skeleton);
    let reach = skeleton.map(reaches).unwrap_or_default();
    let rest = |joint: u16| skeleton.map(|skeleton| skeleton.rest[usize::from(joint)]);
    let within = |joint: u16| error.distance / reach.get(usize::from(joint)).copied().unwrap_or(SKIN_REACH);
    let model = skeleton.map(rest_model).unwrap_or_default();
    // A translation's tolerance in its own units: what its parent's scale at rest makes a meter.
    let travel = |joint: u16| {
        let parent = skeleton.and_then(|skeleton| parent_of(skeleton, usize::from(joint)));
        parent.map_or(error.distance, |parent| {
            let columns = model[parent].matrix3;
            let scale = columns.x_axis.length().max(columns.y_axis.length()).max(columns.z_axis.length());
            if scale > 0.0 { error.distance / scale } else { error.distance }
        })
    };
    let mut keys = KeyArrays::default();
    let rotations = sampled
        .rotations
        .iter()
        .filter_map(|(joint, samples)| {
            let at_rest = rest(*joint).map(|rest| Quat::from_array(rest.rotation).normalize().to_array());
            keys.rotation(*joint, samples, at_rest, within(*joint))
        })
        .collect();
    let translations = sampled
        .translations
        .iter()
        .filter_map(|(joint, samples)| {
            keys.vector(*joint, samples, rest(*joint).map(|rest| rest.translation), travel(*joint))
        })
        .collect();
    let scales = sampled
        .scales
        .iter()
        .filter_map(|(joint, samples)| {
            let tolerance = within(*joint).max(MIN_SCALE_TOLERANCE);
            keys.vector(*joint, samples, rest(*joint).map(|rest| rest.scale), tolerance)
        })
        .collect();
    let weights: Vec<WeightTrack> = sampled
        .weights
        .iter()
        .filter_map(|(morph, samples)| {
            let default = target.morph_defaults.get(usize::from(*morph)).copied().unwrap_or(0.0);
            keys.weight(*morph, samples, default, error.weight)
        })
        .collect();
    let root = sampled.root.as_ref().map(|root| {
        // The whole body turns with root motion: the longest reach there is.
        let reach = reach.iter().copied().fold(SKIN_REACH, f32::max);
        let translation = keys.vector(root.joint, &root.translation, None, error.distance);
        let rotation = keys.rotation(root.joint, &root.rotation, None, error.distance / reach);
        RootMotion {
            joint: root.joint,
            translation: translation.expect("a track with no rest is always kept"),
            rotation: rotation.expect("a track with no rest is always kept"),
        }
    });
    Clip {
        name: sampled.name.clone(),
        skeleton: target.skeleton.map(|(id, _)| id),
        joint_count: skeleton.map_or(0, |skeleton| skeleton.parents.len() as u32),
        morph_target_count: if weights.is_empty() { 0 } else { target.morph_defaults.len() as u32 },
        rate: sampled.rate,
        ticks: sampled.ticks,
        rotations,
        translations,
        scales,
        weights,
        root,
        rotation_ticks: keys.rotation_ticks,
        rotation_keys: keys.rotation_keys,
        vector_ticks: keys.vector_ticks,
        vector_keys: keys.vector_keys,
        weight_ticks: keys.weight_ticks,
        weight_keys: keys.weight_keys,
    }
}

#[cfg(test)]
pub(crate) mod tests;
