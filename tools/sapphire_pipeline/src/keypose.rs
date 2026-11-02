//! Key-pose documents (`.keypose.ron`, M2 N1): motion authored by hand, as poses at ticks, cooked to
//! the same [`Clip`](sapphire_assets::clip::Clip) as any other source. Nothing licensed has
//! anime-grade action, so this is the route to signature moves (`docs/m2-plan.md`, O3).
//!
//! ```ron
//! (
//!     type: "sapphire::pipeline::KeyPoseClip",
//!     version: 1,
//!     data: (
//!         keys: [
//!             // Standing: arms down, in each bone's rest frame.
//!             (tick: 0, pose: {
//!                 LeftUpperArm: (z: -76),
//!                 RightUpperArm: (z: 76),
//!             }),
//!             // Anticipation: pulls back, rising a little.
//!             (tick: 8, ease: Out, hips: (0, 0.02, 0), pose: {
//!                 RightUpperArm: (z: 40, y: -50),
//!             }),
//!             // The strike, past the mark and back, carried forward by root motion.
//!             (tick: 12, ease: Overshoot(0.15), root: (0, 0, 0.6), pose: {
//!                 RightUpperArm: (z: 76, y: 60),
//!             }),
//!         ],
//!     ),
//! )
//! ```
//!
//! # What a document says
//!
//! - **Keys** sit at ticks (60 a second), the first at tick 0. Each is a whole pose, by what it
//!   changes: **a bone or value a key doesn't name keeps what the key before it held**, so between
//!   two keys everything named moves together, and blocking stays short. A first key's unnamed
//!   bones are at rest. `ticks` makes the clip longer than its last key, which it then holds.
//! - **A turn** is what `sapphire_sandbox`'s held pose calls one: a bone's rotation in the model's
//!   rest space, about its own joint, in degrees. `leftUpperArm` turned −76° about +Z lowers that
//!   arm whatever the joint's local axes are, and a bone under a turned bone turns with it, then by
//!   its own turn. Axes apply X, then Y, then Z. Being in rest space and not in a joint's axes, one
//!   document fits any rig with a humanoid map whose rest pose is a T-pose, as VRM's is.
//!   Bones the rig doesn't have are skipped. A turn is a rotation, not an angle to accumulate: a
//!   bone blends along the short way, so a move past 180° in one segment needs a key on the way.
//! - **`hips`** moves the hips from rest, in model space: a crouch, a bob. **`root`** is root motion:
//!   where the character has travelled since tick 0, in model space (facing +Z). So the first key's
//!   root, if it names one, is zero.
//! - **`ease`** shapes the segment arriving at its key ([`KeyEase`]), `Smooth` if unnamed.
//!   Anticipation and overshoot are weights past 0 to 1: a segment can do them on its own, or a key
//!   of their own can pose them, as the example does.
//!
//! # The cook
//!
//! Which character it is for is the sidecar's to say, not the document's: `importer: KeyPose((rig:
//! "<the character source's ID>"))`. Its rig maps bones to joints and its skeleton gives their rest
//! frames, both by their keys in the document's cache key. Every bone named anywhere is sampled at
//! every tick and goes through the same reduction as a glTF's channels ([`crate::clip`]).
//!
//! The rig's constraints aren't in the clip: twist helpers follow at run time, as the held pose's
//! do. Easing is polynomial and blends go through [`sapphire_math::det`], so a cook's samples don't
//! depend on a platform's libm.
//!
//! # Later
//!
//! Hot reload (T2) and a timeline editor (T3) read these documents as they are. Morph weights in a
//! key, root turns, and per-bone timing within a segment wait for a move that needs them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use glam::{Quat, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::clip::{DEFAULT_RATE, MAX_TICKS};
use sapphire_assets::rig::{CharacterRig, HumanBone};
use sapphire_assets::skeleton::{NO_PARENT, Skeleton};
use sapphire_math::{Ease, det};
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::cache::Stage;
use crate::clip::{self, ReductionError, SampledClip, SampledRoot};

/// Reads documents, as cache keys know it. Bump the version whenever sampling changes output for
/// the same document.
pub const IMPORTER: Stage = Stage { name: "keypose", version: 1 };

const SOURCE_SUFFIX: &str = ".keypose.ron";

/// How large an anticipation or overshoot a document may ask for, as a share of its segment's
/// travel.
const MAX_BACK: f32 = 4.0;

/// Whether `path` is a key-pose document, by its compound extension.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The document's name without its suffix: its clip's.
pub fn stem(path: &Path) -> String {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    name[..name.len().saturating_sub(SOURCE_SUFFIX.len())].to_owned()
}

/// The cooked file a document at `relative` writes: `swing.keypose.ron` → `swing.sclip`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    relative.with_file_name(format!("{}.{extension}", stem(relative)))
}

/// A key-pose document's sidecar settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeyPoseSettings {
    /// The character source this document cooks for: the sidecar ID of a source that cooks a rig
    /// and a skeleton, as a VRM does. A sidecar created by a cook has none, and its next cook says
    /// to name one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rig: Option<AssetId>,
    pub error: ReductionError,
}

/// A key-pose document: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct KeyPoseClip {
    /// How long the clip is, if longer than its last key, which it then holds.
    #[serde(default)]
    pub ticks: Option<u16>,
    /// By increasing tick, the first at tick 0.
    pub keys: Vec<PoseKey>,
}

impl Versioned for KeyPoseClip {
    /// 1: M2 N1.
    const VERSION: u32 = 1;
}

/// One pose, at one tick. What it doesn't name, it keeps from the key before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoseKey {
    pub tick: u16,
    /// The shape of the segment arriving here. A first key's has nothing to shape.
    #[serde(default)]
    pub ease: KeyEase,
    /// Where the character has travelled since tick 0, in meters.
    #[serde(default)]
    pub root: Option<[f32; 3]>,
    /// The hips' offset from rest, in meters, in model space.
    #[serde(default)]
    pub hips: Option<[f32; 3]>,
    /// The bones this key turns.
    #[serde(default)]
    pub pose: BTreeMap<HumanBone, Turn>,
}

/// A bone's rotation in the model's rest space, about its joint: degrees, X first, then Y, then Z.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Turn {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Turn {
    /// The rotation.
    pub fn quat(self) -> Quat {
        let about = |axis: Vec3, degrees: f32| det::quat_from_axis_angle(axis, degrees.to_radians());
        (about(Vec3::Z, self.z) * about(Vec3::Y, self.y) * about(Vec3::X, self.x)).normalize()
    }
}

/// How a segment moves from one key's pose to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum KeyEase {
    /// Slow at both ends.
    #[default]
    Smooth,
    /// Constant speed.
    Linear,
    /// Starts slowly and arrives at full speed: a strike landing.
    In,
    /// Leaves at full speed and arrives slowly: a recovery.
    Out,
    /// Holds the previous pose, and jumps to this one on its tick: a cut.
    Hold,
    /// Pulls back first, by this share of the segment's travel, then goes all the way.
    Anticipate(f32),
    /// Goes past this key by this share of the segment's travel, and settles back onto it.
    Overshoot(f32),
    /// A cubic Bézier from (0, 0) to (1, 1) through handles (x1, y1) and (x2, y2): x is the
    /// segment's progress, within 0 to 1, y the pose's, free to leave 0 to 1. CSS's
    /// `cubic-bezier(x1, y1, x2, y2)`.
    Curve(f32, f32, f32, f32),
}

impl KeyEase {
    /// The weight of the arriving pose at progress `u`, from 0 to 1. Exactly 0 and 1 at the ends.
    #[must_use]
    pub fn weight(self, u: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        if u <= 0.0 {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        match self {
            Self::Smooth => Ease::Smooth.apply(u),
            Self::Linear => Ease::Linear.apply(u),
            Self::In => Ease::In.apply(u),
            Self::Out => Ease::Out.apply(u),
            Self::Hold => Ease::Hold.apply(u),
            Self::Anticipate(share) => back(u, back_strength(share)),
            Self::Overshoot(share) => 1.0 - back(1.0 - u, back_strength(share)),
            Self::Curve(x1, y1, x2, y2) => {
                let bezier = |t: f32, a: f32, b: f32| {
                    3.0 * (1.0 - t) * (1.0 - t) * t * a + 3.0 * (1.0 - t) * t * t * b + t * t * t
                };
                // x climbs from 0 to 1 with handles inside 0 to 1, so bisection finds its t.
                let (mut low, mut high) = (0.0, 1.0);
                for _ in 0..24 {
                    let middle: f32 = (low + high) * 0.5;
                    if bezier(middle, x1, x2) < u {
                        low = middle;
                    } else {
                        high = middle;
                    }
                }
                bezier((low + high) * 0.5, y1, y2)
            }
        }
    }

    /// Whether its numbers are ones it can honor.
    fn is_valid(self) -> bool {
        match self {
            Self::Smooth | Self::Linear | Self::In | Self::Out | Self::Hold => true,
            Self::Anticipate(share) | Self::Overshoot(share) => (0.0..=MAX_BACK).contains(&share),
            Self::Curve(x1, y1, x2, y2) => {
                (0.0..=1.0).contains(&x1) && (0.0..=1.0).contains(&x2) && y1.is_finite() && y2.is_finite()
            }
        }
    }
}

/// The classic "back" curve: `(s + 1)u³ − s·u²`. It dips below 0 before climbing to 1.
fn back(u: f32, strength: f32) -> f32 {
    (strength + 1.0) * u * u * u - strength * u * u
}

/// The strength that makes [`back`] dip by `share`. The dip is `4s³ / 27(s + 1)²`, which grows with
/// `s`, so bisection finds it; the whole search is IEEE arithmetic, the same on every target.
fn back_strength(share: f32) -> f32 {
    let dip = |s: f32| 4.0 * s * s * s / (27.0 * (s + 1.0) * (s + 1.0));
    let (mut low, mut high) = (0.0, 32.0);
    for _ in 0..32 {
        let middle: f32 = (low + high) * 0.5;
        if dip(middle) < share {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) * 0.5
}

/// Why a document can't cook.
#[derive(Debug, thiserror::Error)]
pub enum KeyPoseError {
    #[error("the document isn't UTF-8")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("the document can't be read")]
    Document(#[source] Box<DocumentError>),
    #[error("the sidecar names no `rig`: which character does this cook for?")]
    NoRig,
    #[error("the sidecar's `rig` {id} isn't a source of this content tree that cooks a rig and a skeleton")]
    UnknownRig { id: AssetId },
    #[error("failed to read the rig's cooked output")]
    Read(#[source] std::io::Error),
    #[error("the rig's cooked rig is invalid")]
    Rig(#[from] sapphire_assets::rig::RigError),
    #[error("the rig's cooked skeleton is invalid")]
    Skeleton(#[from] sapphire_assets::skeleton::SkeletonError),
    #[error("the rig has no humanoid map")]
    NotHumanoid,
    #[error("the document has no keys")]
    NoKeys,
    #[error("key {key} is at tick {tick}: the first key belongs at tick 0, and each after the one before")]
    KeyOrder { key: usize, tick: u16 },
    #[error("the clip is {ticks} ticks long, which ends before its last key, at {last}")]
    TooShort { ticks: u16, last: u16 },
    #[error("the clip is {0} ticks long, over the limit of {MAX_TICKS}")]
    TooLong(u32),
    #[error("key {key}'s {what} isn't finite")]
    NonFinite { key: usize, what: String },
    #[error("key {key}'s ease is out of range: shares from 0 to {MAX_BACK}, handles' x from 0 to 1")]
    Ease { key: usize },
    #[error("the first key's root is {0:?}: root motion starts at the identity")]
    RootStartsAway([f32; 3]),
}

/// Reads a document.
pub fn read(bytes: &[u8]) -> Result<KeyPoseClip, KeyPoseError> {
    sapphire_serialize::from_ron(std::str::from_utf8(bytes)?)
        .map_err(|error| KeyPoseError::Document(Box::new(error)))
}

/// Checks everything about `document` that holds whatever rig it cooks for.
fn validate(document: &KeyPoseClip) -> Result<u16, KeyPoseError> {
    if document.keys.is_empty() {
        return Err(KeyPoseError::NoKeys);
    }
    let mut previous = None;
    for (key, pose) in document.keys.iter().enumerate() {
        let tick = pose.tick;
        if previous.map_or(tick != 0, |previous| tick <= previous) {
            return Err(KeyPoseError::KeyOrder { key, tick });
        }
        previous = Some(tick);
        if !pose.ease.is_valid() {
            return Err(KeyPoseError::Ease { key });
        }
        let finite = |what: &str, values: &[f32]| {
            if values.iter().all(|value| value.is_finite()) {
                Ok(())
            } else {
                Err(KeyPoseError::NonFinite { key, what: what.to_owned() })
            }
        };
        finite("root", pose.root.as_ref().map_or(&[], |root| root.as_slice()))?;
        finite("hips", pose.hips.as_ref().map_or(&[], |hips| hips.as_slice()))?;
        for (bone, turn) in &pose.pose {
            finite(bone.name(), &[turn.x, turn.y, turn.z])?;
        }
    }
    if let Some(root) = document.keys[0].root
        && root != [0.0; 3]
    {
        return Err(KeyPoseError::RootStartsAway(root));
    }
    let last = document.keys[document.keys.len() - 1].tick;
    let ticks = document.ticks.unwrap_or(last);
    if ticks < last {
        return Err(KeyPoseError::TooShort { ticks, last });
    }
    if ticks > MAX_TICKS {
        return Err(KeyPoseError::TooLong(u32::from(ticks)));
    }
    Ok(ticks)
}

/// At one tick: the keys around it, and the weight of the second.
#[derive(Clone, Copy)]
struct Moment {
    from: usize,
    to: usize,
    weight: f32,
}

/// Where each of the clip's ticks falls among `keys`.
fn timeline(keys: &[PoseKey], ticks: u16) -> Vec<Moment> {
    let mut moments = Vec::with_capacity(usize::from(ticks) + 1);
    let mut from = 0;
    for tick in 0..=ticks {
        while from + 1 < keys.len() && keys[from + 1].tick <= tick {
            from += 1;
        }
        moments.push(if from + 1 == keys.len() {
            Moment { from, to: from, weight: 0.0 }
        } else {
            let (start, end) = (keys[from].tick, keys[from + 1].tick);
            let u = f32::from(tick - start) / f32::from(end - start);
            Moment { from, to: from + 1, weight: keys[from + 1].ease.weight(u) }
        });
    }
    moments
}

/// What each key holds of one value: `named`'s, or the key before's, or `start`.
fn held<T: Copy>(keys: &[PoseKey], start: T, named: impl Fn(&PoseKey) -> Option<T>) -> Vec<T> {
    let mut current = start;
    keys.iter()
        .map(|key| {
            current = named(key).unwrap_or(current);
            current
        })
        .collect()
}

/// Samples `document` for `rig` and `skeleton`, which must be cooked from one source. The result
/// is ready for [`clip::cook`].
pub fn sample(
    document: &KeyPoseClip,
    name: &str,
    skeleton: &Skeleton,
    rig: &CharacterRig,
) -> Result<SampledClip, KeyPoseError> {
    let ticks = validate(document)?;
    let hips = rig.human_joint(HumanBone::Hips).ok_or(KeyPoseError::NotHumanoid)?;
    let keys = &document.keys;
    let moments = timeline(keys, ticks);
    let model = clip::rest_model(skeleton);
    let parent_of = |joint: usize| {
        let parent = skeleton.parents[joint];
        (parent != NO_PARENT).then_some(usize::from(parent))
    };
    // Model-space rest rotations, built as the held pose builds them (`sapphire_sandbox::pose`):
    // from normalized local rotations, so a parent's scale doesn't reach a turn's frame.
    let mut rest_world: Vec<Quat> = Vec::with_capacity(skeleton.rest.len());
    for joint in 0..skeleton.rest.len() {
        let local = Quat::from_array(skeleton.rest[joint].rotation).normalize();
        rest_world.push(parent_of(joint).map_or(local, |parent| rest_world[parent] * local));
    }

    let bones: Vec<HumanBone> = {
        let mut named: Vec<HumanBone> = keys.iter().flat_map(|key| key.pose.keys().copied()).collect();
        named.sort_unstable();
        named.dedup();
        named
    };
    let mut rotations: Vec<(u16, Vec<Quat>)> = Vec::with_capacity(bones.len());
    for bone in bones {
        // A bone the rig doesn't have is skipped, so one document fits more rigs.
        let Some(joint) = rig.human_joint(bone) else { continue };
        let index = usize::from(joint);
        let held = held(keys, Quat::IDENTITY, |key| key.pose.get(&bone).map(|turn| turn.quat()));
        let local_rest = Quat::from_array(skeleton.rest[index].rotation).normalize();
        let parent = parent_of(index).map_or(Quat::IDENTITY, |parent| rest_world[parent]);
        // A turn in rest model space, seen from the joint's parent: `parentᵀ · turn · parent`.
        let samples = moments
            .iter()
            .map(|moment| {
                let turn = clip::slerp(held[moment.from], held[moment.to], moment.weight);
                (parent.inverse() * turn * parent * local_rest).normalize()
            })
            .collect();
        rotations.push((joint, samples));
    }
    rotations.sort_by_key(|(joint, _)| *joint);

    let blended = |values: &[[f32; 3]], moment: &Moment| {
        Vec3::from(values[moment.from]).lerp(Vec3::from(values[moment.to]), moment.weight)
    };
    let mut translations = Vec::new();
    if keys.iter().any(|key| key.hips.is_some()) {
        let offsets = held(keys, [0.0; 3], |key| key.hips);
        let rest = Vec3::from(skeleton.rest[usize::from(hips)].translation);
        let to_parent =
            parent_of(usize::from(hips)).map_or(glam::Affine3A::IDENTITY, |parent| model[parent].inverse());
        let samples = moments
            .iter()
            .map(|moment| rest + to_parent.transform_vector3(blended(&offsets, moment)))
            .collect();
        translations.push((hips, samples));
    }
    let root = keys.iter().any(|key| key.root.is_some()).then(|| {
        let positions = held(keys, [0.0; 3], |key| key.root);
        SampledRoot {
            joint: hips,
            translation: moments.iter().map(|moment| blended(&positions, moment)).collect(),
            rotation: vec![Quat::IDENTITY; moments.len()],
        }
    });
    Ok(SampledClip {
        name: name.to_owned(),
        rate: DEFAULT_RATE,
        ticks,
        rotations,
        translations,
        scales: Vec::new(),
        weights: Vec::new(),
        root,
    })
}

#[cfg(test)]
mod tests;
