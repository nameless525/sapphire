//! Move set documents (`.moves.ron`) cook to `.smoves` (M2 G2, spec §5.27): frame data in ticks and by
//! name, as a person or the timeline editor writes it, checked and resolved into what the simulation
//! reads ([`sapphire_moves`]).
//!
//! A cook names everything wrong with a document at once, by move and track, so a typo in frame data
//! says where it is. What a document looks like is `sapphire_moves`' to say.
//!
//! # Tables from a clip (M2 N1)
//!
//! A move that names a clip ([`ClipMotionDocument`]) has its tables baked here, once per tick of the
//! clip, so the simulation reads plain numbers and never sees a pose ([`bake`]):
//!
//! - **Root motion** is the clip's root track, or none, as the document says: cumulative, in the
//!   root's space at the move's first tick, sample 0 exactly zero and yaw unwrapped.
//! - **A path** is its joint's transform in the clip's pose, in model space, with the document's
//!   offset. `root(t) × pose(t)` is the motion as authored ([`crate::clip`]), so a joint's model-space
//!   transform at a tick is already in the root's space at that same tick, which is what a path means,
//!   and it rides motion warp.
//! - **A move is as long as its clip.** `sapphire_moves` holds that, and says why.
//!
//! **What a table reads.** One replay file holds one hash of the move data for every architecture
//! (`Simulation::tables_hash`), so a table has to be the same bytes on all of them, from the clip's
//! source on. So it reads neither a cooked clip nor a pose:
//!
//! - **Not `sapphire_anim`'s `sample` and `local_to_model`, despite how the plan first read:** they run
//!   on `Quat` and `Affine3A`, whose last bit differs between NEON and SSE2.
//! - **Not the cooked `Clip`:** key reduction leaves it within 0.25 mm a track of what it reduced, and
//!   there's no reason for a table to start from less than the samples.
//! - **It reads the [`SampledClip`]**, the fork where a clip's cook goes to key reduction and a table
//!   comes here, so a table and a clip read the same numbers. The steps up to that fork went onto
//!   scalars and `det` for this ([`crate::clip`]'s "Byte identity"), with stage bumps, rather than
//!   running a second, scalar copy of retargeting and root lifting beside the first: two copies are
//!   two things to keep in step, and what this package has to promise is that they agree.
//!
//! So a table and the pose on screen agree within what key reduction and quantization allow, whichever
//! is read: `moves::tests::a_table_agrees_with_the_pose_on_screen` measures it on real clips, with
//! the rig's constraints running.
//!
//! **FBX is refused** ([`BakeError::FbxSource`]), as a clip's source or a rig's: `ufbx` evaluates in
//! C through its platform's libm. Convert it to glTF for a move, or key the move by hand.
//!
//! **Zeros:** `-0.0` and `0.0` are different bytes and the same number, and a path can reach either
//! by harmless routes, so a table writes `0.0`.
//!
//! **The cook's key** holds every clip a set names and those clips' rigs, by their own keys, so a
//! change to either recooks the set. A cooked set holds no IDs, so it needs no marker in
//! `cook::part_dependencies`: the document's own bytes name the clips.

use std::path::{Path, PathBuf};

use glam::{Affine3, Quat, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::clip::DEFAULT_RATE;
use sapphire_assets::rig::{Constraint, HumanBone};
use sapphire_assets::skeleton::{NO_PARENT, Skeleton};
use sapphire_core::Name;
use sapphire_math::det;
use sapphire_moves::{
    BakedPath, BakedTables, ClipMotionDocument, ClipPathDocument, Follows, MoveSetDocument, MoveSetError,
    PathSample, RootSample, RootSource, TablesError,
};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;
use crate::clip::retarget::HumanMap;
use crate::clip::{self, ClipCookError, JointName, SampledClip, joint_named};

/// Reading the document, as cache keys know it. Bump the version whenever its output changes for the
/// same file.
pub const IMPORTER: Stage = Stage { name: "move set document", version: 1 };

/// Checking and resolving it, and baking its keyed tables. Bump the version whenever they change
/// output for the same document.
pub const COOKER: Stage = Stage { name: "smoves", version: 6 };

/// What a move set source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".moves.ron";

/// Import settings for move sets: none yet. A struct, so settings can join without changing existing
/// sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MoveSetSettings {}

/// A document that didn't cook.
#[derive(Debug, thiserror::Error)]
pub enum MoveSetCookError {
    #[error("the document isn't UTF-8")]
    NotUtf8(#[source] std::str::Utf8Error),
    #[error(transparent)]
    Moves(#[from] MoveSetError),
}

/// Whether `path` names a move set source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a move set source at `relative` writes: `fighter.moves.ron` → `fighter.smoves`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Reads a document's bytes.
pub fn read(bytes: &[u8]) -> Result<MoveSetDocument, MoveSetCookError> {
    let text = std::str::from_utf8(bytes).map_err(MoveSetCookError::NotUtf8)?;
    Ok(MoveSetDocument::parse(text)?)
}

/// Cooks a document's bytes, with `tables` answering for each clip its moves name.
pub fn cook(
    bytes: &[u8],
    mut tables: impl FnMut(&ClipMotionDocument) -> Result<BakedTables, BakeError>,
) -> Result<Vec<u8>, MoveSetCookError> {
    let set = read(bytes)?.compile_with(|mv| match &mv.motion.clip {
        Some(clip) => tables(clip).map(Some).map_err(|error| error.to_string()),
        None => Ok(None),
    })?;
    Ok(set.to_cooked()?.into_vec())
}

/// What a move's clip gives its bake: the clip's samples, and the rig they're for.
#[derive(Clone, Copy, Debug)]
pub struct TableInput<'a> {
    pub sampled: &'a SampledClip,
    pub skeleton: &'a Skeleton,
    /// Which joint is which bone, if the rig says: what [`Follows::Bone`] reads.
    pub map: Option<&'a HumanMap>,
    /// The rig's constraints, which run after a pose on screen and aren't in a table.
    pub constraints: &'a [Constraint],
}

/// Why a move's clip can't bake.
#[derive(Debug, thiserror::Error)]
pub enum BakeError {
    #[error("the clip's `{field}` {text:?} isn't an asset ID")]
    NotAnId { field: &'static str, text: String },
    #[error("the clip's `source` {id} isn't a source of this content tree")]
    UnknownSource { id: AssetId },
    #[error(
        "the clip's `source` {id} cooks no clips: name a key-pose document or an animation source's sidecar ID"
    )]
    NotAClipSource { id: AssetId },
    #[error("the clip's `source` is a key-pose document, which is one clip, so it takes no `name`")]
    NameOnKeyPose,
    #[error("the clip's `source` holds many clips: say which with `name`")]
    NoName,
    #[error(
        "the clip's `source` {source_id} doesn't cook this clip for this rig: add it to that source's sidecar \
         (`names`, `retarget` or `also`)"
    )]
    NotCooked { source_id: AssetId },
    #[error(
        "{id} is an FBX, which `ufbx` evaluates in C through its platform's libm, so its numbers aren't the \
         same bytes on every architecture and one replay's hash can't hold them: convert it to glTF, or key \
         this move by hand"
    )]
    FbxSource { id: AssetId },
    #[error("failed to read the clip's source")]
    Read(#[source] std::io::Error),
    #[error(
        "the clip is at {rate} ticks a second, and a table is at {DEFAULT_RATE}, one sample per fixed step"
    )]
    Rate { rate: u16 },
    #[error("path `{path}` follows bone {bone:?}, which isn't a humanoid bone (`RightHand`, `Hips`, …)")]
    UnknownBone { path: String, bone: String },
    #[error("path `{path}` follows a bone, and the rig has no humanoid map: follow a `Joint` by its name")]
    NoMap { path: String },
    #[error("path `{path}` follows bone {bone:?}, which the rig doesn't have")]
    NoBone { path: String, bone: String },
    #[error("path `{path}` follows joint {joint:?}, which the skeleton doesn't have")]
    NoJoint { path: String, joint: String },
    #[error(
        "path `{path}` follows joint {joint:?}, and two joints have that name, so it says neither of them"
    )]
    RepeatedJoint { path: String, joint: String },
    #[error(
        "path `{path}` goes through joint {joint:?}, which a rig constraint drives: a table has no constraints \
         in it and the pose on screen does, so they'd disagree"
    )]
    ConstraintDriven { path: String, joint: String },
    #[error(transparent)]
    Tables(#[from] TablesError),
    #[error(transparent)]
    Clip(#[from] Box<ClipCookError>),
}

/// A joint's sign of zero, settled: see the [module docs](self). `x + 0.0` is `x` but for `-0.0`,
/// which IEEE arithmetic rounds to `0.0`.
#[inline]
fn settled(value: f32) -> f32 {
    value + 0.0
}

/// A humanoid bone by what a key-pose document calls it.
fn human_bone(name: &str) -> Option<HumanBone> {
    use serde::de::IntoDeserializer;
    HumanBone::deserialize(IntoDeserializer::<serde::de::value::Error>::into_deserializer(name)).ok()
}

/// A named path, resolved.
struct Followed<'a> {
    path: &'a ClipPathDocument,
    joint: usize,
    offset: Vec3,
    rotation: Quat,
}

/// `path`'s joint, by what it follows.
fn followed_joint(path: &ClipPathDocument, input: &TableInput<'_>) -> Result<usize, BakeError> {
    let name = || path.name.clone();
    match &path.follows {
        Follows::Bone(bone) => {
            let text = || bone.clone();
            let known =
                human_bone(bone).ok_or_else(|| BakeError::UnknownBone { path: name(), bone: text() })?;
            let map = input.map.ok_or_else(|| BakeError::NoMap { path: name() })?;
            let joint = map.joint(known).ok_or_else(|| BakeError::NoBone { path: name(), bone: text() })?;
            Ok(usize::from(joint))
        }
        Follows::Joint(joint) => joint_named(input.skeleton, joint).map_err(|fault| match fault {
            JointName::Unknown => BakeError::NoJoint { path: name(), joint: joint.clone() },
            JointName::Repeated => BakeError::RepeatedJoint { path: name(), joint: joint.clone() },
        }),
    }
}

/// `joint`'s parent, if it has one.
fn parent_of(skeleton: &Skeleton, joint: usize) -> Option<usize> {
    let parent = skeleton.parents[joint];
    (parent != NO_PARENT).then_some(usize::from(parent))
}

/// The clip's root track as a table has it: see the [module docs](self). A root lifted from a bone
/// starts within rounding of the identity, and a table starts on it, so what's left of its first
/// sample is taken out of all of them.
fn root_samples(sampled: &SampledClip, count: usize) -> Vec<RootSample> {
    let Some(root) = &sampled.root else { return vec![RootSample::ZERO; count] };
    let (start, start_yaw) = (root.translation[0], det::yaw_of_quat(root.rotation[0]));
    let mut unwrapped = start_yaw;
    root.translation
        .iter()
        .zip(&root.rotation)
        .map(|(at, turn)| {
            // Each tick turns less than half a turn from the last, whatever the whole clip does.
            unwrapped += det::wrap_angle(det::yaw_of_quat(*turn) - unwrapped);
            let offset = det::rotate_y(*at - start, -start_yaw);
            RootSample { translation: offset.to_array().map(settled), yaw: settled(unwrapped - start_yaw) }
        })
        .collect()
}

/// A move's tables from its clip: see the [module docs](self). They are the clip's own length, which
/// `sapphire_moves` holds against the move's.
pub fn bake(clip: &ClipMotionDocument, input: &TableInput<'_>) -> Result<BakedTables, BakeError> {
    let TableInput { sampled, skeleton, constraints, .. } = *input;
    if sampled.rate != DEFAULT_RATE {
        return Err(BakeError::Rate { rate: sampled.rate });
    }
    let count = usize::from(sampled.ticks) + 1;
    let root = match clip.root {
        RootSource::Clip => root_samples(sampled, count),
        RootSource::InPlace => vec![RootSample::ZERO; count],
    };

    if clip.paths.is_empty() {
        return Ok(BakedTables::new(root, Vec::new())?);
    }

    let joints = skeleton.parents.len();
    let mut followed = Vec::with_capacity(clip.paths.len());
    // Only joints on the way to a followed one need walking: a VRM's hair and skirts don't.
    let mut needed = vec![false; joints];
    for path in &clip.paths {
        let joint = followed_joint(path, input)?;
        let mut along = Some(joint);
        while let Some(at) = along {
            if let Some(driven) = constraints.iter().find(|constraint| usize::from(constraint.joint) == at) {
                let joint = skeleton.names[usize::from(driven.joint)].clone();
                return Err(BakeError::ConstraintDriven { path: path.name.clone(), joint });
            }
            needed[at] = true;
            along = parent_of(skeleton, at);
        }
        followed.push(Followed {
            path,
            joint,
            offset: Vec3::from(path.offset),
            rotation: det::quat_normalize(Quat::from_array(path.rotation)),
        });
    }

    let needed: Vec<usize> = (0..joints).filter(|&joint| needed[joint]).collect();
    let rest_rotations: Vec<Quat> =
        needed.iter().map(|&joint| clip::rest_rotation(skeleton, joint)).collect();
    let rotations = joint_tracks(&sampled.rotations, joints);
    let translations = joint_tracks(&sampled.translations, joints);
    let scales = joint_tracks(&sampled.scales, joints);
    let mut model = vec![(Affine3::IDENTITY, Quat::IDENTITY); joints];
    let mut samples: Vec<Vec<PathSample>> = (0..followed.len()).map(|_| Vec::with_capacity(count)).collect();
    for tick in 0..count {
        for (&joint, &rest_rotation) in needed.iter().zip(&rest_rotations) {
            let rest = &skeleton.rest[joint];
            let rotation =
                rotations[joint].map_or_else(|| rest_rotation, |track| det::quat_normalize(track[tick]));
            let local = Affine3::from_scale_rotation_translation(
                scales[joint].map_or(Vec3::from(rest.scale), |track| track[tick]),
                rotation,
                translations[joint].map_or(Vec3::from(rest.translation), |track| track[tick]),
            );
            // The turn apart from the matrix: a rotation, whatever scale a parent has.
            model[joint] = match parent_of(skeleton, joint) {
                Some(parent) => (model[parent].0 * local, det::quat_mul(model[parent].1, rotation)),
                None => (local, rotation),
            };
        }
        for (path, samples) in followed.iter().zip(&mut samples) {
            let (matrix, turn) = model[path.joint];
            let at = matrix.translation + det::quat_rotate(turn, path.offset);
            let rotation = det::quat_normalize(det::quat_mul(turn, path.rotation));
            samples.push(PathSample {
                translation: at.to_array().map(settled),
                rotation: rotation.to_array().map(settled),
            });
        }
    }
    let paths = followed
        .iter()
        .zip(samples)
        .map(|(path, samples)| BakedPath::new(Name::intern(&path.path.name), samples))
        .collect();
    Ok(BakedTables::new(root, paths)?)
}

/// Each joint's track of `tracks`, if it has one.
fn joint_tracks<T>(tracks: &[(u16, Vec<T>)], joints: usize) -> Vec<Option<&[T]>> {
    let mut found = vec![None; joints];
    for (joint, samples) in tracks {
        found[usize::from(*joint)] = Some(samples.as_slice());
    }
    found
}

#[cfg(test)]
mod tests;
