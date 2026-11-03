//! A source's animations, imported once for all its clips: a file of 15 clips would otherwise parse
//! and read its buffers 15 times, and the cook visits a source's parts one after another
//! (`ClipSources` keeps the last).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gltf::Document;
use sapphire_assets::skeleton::{Joint, Skeleton};

use super::retarget::HumanMap;
use super::{ClipCookError, ClipSettings, SampledClip};
use crate::import::fbx::clips::FbxClips;
use crate::import::gltf::clips::{self as gltf_clips, ClipScene};
use crate::import::vrma;
use crate::meta::ImportSettings;

/// How far a shared skeleton's rest may be from a source's before they aren't one rig: well over
/// an exporter's rounding, well under anything a pose would show.
const REST_TOLERANCE: f32 = 1e-4;

/// One file's animations and the rig they move.
pub struct ClipSource {
    /// The file's own joints, in its own order. Empty for a mesh of morph targets alone.
    pub joints: Vec<Joint>,
    /// The default weight of each of the mesh's morph targets.
    pub morph_defaults: Vec<f32>,
    /// A VRMA's humanoid map, which is the file's own.
    pub map: Option<HumanMap>,
    animations: Animations,
}

enum Animations {
    Gltf(Box<GltfAnimations>),
    Fbx(Box<FbxClips>),
}

struct GltfAnimations {
    document: Document,
    buffers: Vec<Vec<u8>>,
    joint_of_node: HashMap<usize, u16>,
    targets_of_morph: HashMap<(usize, usize), Vec<u32>>,
}

impl ClipSource {
    /// Imports the file at `path`, whose contents are `bytes`, as `importer` reads it. A mesh
    /// source imports its mesh too, since its targets are clips' to drive; an animation source reads
    /// no geometry.
    pub fn import(path: &Path, bytes: &[u8], importer: &ImportSettings) -> Result<Self, ClipCookError> {
        match importer {
            ImportSettings::Gltf(_) => {
                let (document, buffers, scene) = gltf_clips::import_scene(bytes, path.parent())?;
                let morph_defaults =
                    scene.mesh.morph_targets.iter().map(|target| target.default_weight).collect();
                let joints = scene.mesh.skin.map(|skin| skin.joints).unwrap_or_default();
                Ok(Self {
                    joints,
                    morph_defaults,
                    map: None,
                    animations: Animations::Gltf(Box::new(GltfAnimations {
                        document,
                        buffers,
                        joint_of_node: scene.joint_of_node,
                        targets_of_morph: scene.targets_of_morph,
                    })),
                })
            }
            ImportSettings::GltfClips(_) | ImportSettings::Vrma(_) => {
                let is_vrma = matches!(importer, ImportSettings::Vrma(_));
                let rig = gltf_clips::import_rig(bytes, path.parent(), |document| {
                    if is_vrma { vrma::humanoid_nodes(document) } else { Ok(Vec::new()) }
                })?;
                let map =
                    is_vrma.then(|| vrma::humanoid_map(&rig.document, &rig.joint_of_node)).transpose()?;
                Ok(Self {
                    joints: rig.joints,
                    morph_defaults: Vec::new(),
                    map,
                    animations: Animations::Gltf(Box::new(GltfAnimations {
                        document: rig.document,
                        buffers: rig.buffers,
                        joint_of_node: rig.joint_of_node,
                        targets_of_morph: HashMap::new(),
                    })),
                })
            }
            ImportSettings::Fbx(_) | ImportSettings::FbxClips(_) => {
                let with_mesh = matches!(importer, ImportSettings::Fbx(_));
                let clips = FbxClips::import(bytes, with_mesh)?;
                Ok(Self {
                    joints: clips.joints().to_vec(),
                    morph_defaults: clips.morph_defaults().to_vec(),
                    map: None,
                    animations: Animations::Fbx(Box::new(clips)),
                })
            }
            _ => Err(ClipCookError::Settings),
        }
    }

    /// Samples animation `index`, its root motion lifted as `settings` say.
    pub fn sample(&self, index: usize, settings: &ClipSettings) -> Result<SampledClip, ClipCookError> {
        match &self.animations {
            Animations::Gltf(gltf) => {
                let scene = ClipScene {
                    joints: &self.joints,
                    joint_of_node: &gltf.joint_of_node,
                    targets_of_morph: &gltf.targets_of_morph,
                };
                Ok(gltf_clips::sample(&gltf.document, &gltf.buffers, scene, index, settings)?)
            }
            Animations::Fbx(clips) => Ok(clips.sample(index, settings)?),
        }
    }
}

/// The cook's one remembered import: a source's parts cook one after another, and only one source's
/// data is ever held.
#[derive(Default)]
pub(crate) struct ClipSources {
    last: RefCell<Option<(PathBuf, Rc<ClipSource>)>>,
}

impl ClipSources {
    /// The import of the file at `path`.
    pub fn get(
        &self,
        path: &Path,
        bytes: &[u8],
        importer: &ImportSettings,
    ) -> Result<Rc<ClipSource>, ClipCookError> {
        if let Some((last, source)) = &*self.last.borrow()
            && last == path
        {
            return Ok(Rc::clone(source));
        }
        let source = Rc::new(ClipSource::import(path, bytes, importer)?);
        *self.last.borrow_mut() = Some((path.to_owned(), Rc::clone(&source)));
        Ok(source)
    }
}

/// Why a source's joints can't play on a skeleton they say they share.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RigMismatch {
    #[error("joint {joint:?} isn't in the skeleton it shares")]
    MissingJoint { joint: String },
    #[error("two joints are called {joint:?}, so its name can't say which of them is shared")]
    AmbiguousName { joint: String },
    #[error("joint {joint:?} is under {found:?} here and {expected:?} in the skeleton it shares")]
    Parent { joint: String, found: Option<String>, expected: Option<String> },
    #[error("joint {joint:?} rests elsewhere in the skeleton it shares")]
    Rest { joint: String },
}

/// Each of `joints`' index in `shared`, by name: the same rig in another order, as two exports of
/// one armature may be. Every joint must be there, once on each side, under the same parent, at the
/// same rest; `shared` may have more. glTF lets names repeat, and a skeleton of its own has no trouble
/// with that, but here a repeated name would land two joints on one, or one on the wrong one.
pub fn joint_remap(joints: &[Joint], shared: &Skeleton) -> Result<Vec<u16>, RigMismatch> {
    // `None`: more than one joint has the name.
    let mut by_name: HashMap<&str, Option<usize>> = HashMap::with_capacity(shared.names.len());
    for (index, name) in shared.names.iter().enumerate() {
        by_name.entry(name).and_modify(|found| *found = None).or_insert(Some(index));
    }
    let mut seen: HashSet<&str> = HashSet::with_capacity(joints.len());
    joints
        .iter()
        .map(|joint| {
            let name = || joint.name.clone();
            let found = *by_name
                .get(joint.name.as_str())
                .ok_or_else(|| RigMismatch::MissingJoint { joint: name() })?;
            let index = found
                .filter(|_| seen.insert(joint.name.as_str()))
                .ok_or_else(|| RigMismatch::AmbiguousName { joint: name() })?;
            let found = joint.parent.map(|parent| joints[usize::from(parent)].name.clone());
            let expected = (shared.parents[index] != sapphire_assets::skeleton::NO_PARENT)
                .then(|| shared.names[usize::from(shared.parents[index])].clone());
            if found != expected {
                return Err(RigMismatch::Parent { joint: name(), found, expected });
            }
            let (own, theirs) = (&joint.rest, &shared.rest[index]);
            let near = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(a, b)| (a - b).abs() <= REST_TOLERANCE);
            let flipped = theirs.rotation.map(|value| -value);
            let same = near(&own.translation, &theirs.translation)
                && near(&own.scale, &theirs.scale)
                && (near(&own.rotation, &theirs.rotation) || near(&own.rotation, &flipped));
            if !same {
                return Err(RigMismatch::Rest { joint: name() });
            }
            Ok(index as u16)
        })
        .collect()
}

/// `sampled`, with its joints renumbered by `remap`, each list back in joint order.
pub fn remap_tracks(mut sampled: SampledClip, remap: &[u16]) -> SampledClip {
    fn renumber<T>(tracks: &mut [(u16, T)], remap: &[u16]) {
        for (joint, _) in tracks.iter_mut() {
            *joint = remap[usize::from(*joint)];
        }
        tracks.sort_by_key(|(joint, _)| *joint);
    }
    renumber(&mut sampled.rotations, remap);
    renumber(&mut sampled.translations, remap);
    renumber(&mut sampled.scales, remap);
    if let Some(root) = &mut sampled.root {
        root.joint = remap[usize::from(root.joint)];
    }
    sampled
}
