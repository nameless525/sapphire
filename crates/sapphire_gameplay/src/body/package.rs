//! Immutable cooked inputs and policy for a fixed body. Binding is outside the simulation step.

use std::sync::Arc;

use sapphire_assets::clip::{ArchivedClip, Clip};
use sapphire_assets::graph::AnimGraph;
use sapphire_assets::rig::{ArchivedCharacterRig, ArchivedConstraint, CharacterRig, HumanBone};
use sapphire_assets::skeleton::Skeleton;
use sapphire_assets::{AssetId, CpuResident};
use sapphire_body_pose::{
    AuthoredHeadLook, BodyTuning, CharacterIk, CharacterPose, ClipSource, FixedBody, FixedBodyPolicy,
    FixedGraphHistory, GraphBinding, IkRig,
};
use sapphire_core::Name;
use sapphire_math::Vec3;

use crate::StateHasher;

/// Exact, checked CPU assets. The profile owns its clips for the whole run.
#[derive(Clone)]
pub struct BodyAssets {
    pub graph: BodyGraphAssets,
    pub rig: CpuResident<CharacterRig>,
}

/// A cooked graph and its skeleton and clips, also for a character without a VRM rig asset.
#[derive(Clone)]
pub struct BodyGraphAssets {
    pub graph: CpuResident<AnimGraph>,
    pub skeleton_id: AssetId,
    pub skeleton: CpuResident<Skeleton>,
    pub clips: Vec<(AssetId, CpuResident<Clip>)>,
}

#[derive(Clone, Copy)]
enum RigBinding<'a> {
    Cooked(&'a CpuResident<CharacterRig>),
    Named { bones: &'a [(HumanBone, usize)], eye: Vec3 },
}

impl<'a> RigBinding<'a> {
    fn cooked(self) -> Option<&'a ArchivedCharacterRig> {
        match self {
            Self::Cooked(rig) => Some(rig.view()),
            Self::Named { .. } => None,
        }
    }
}

/// Immutable authored-head policy. The envelope is in source clip ticks.
#[derive(Clone, Copy)]
pub struct HeadAuthority {
    pub movement: Name,
    pub retained: f32,
    pub source_ticks: [f32; 4],
}

/// Static CPU body metadata. Morph order must match the cooked rig and graph tracks.
pub struct BodyProfileOptions<'a> {
    /// How its feet and look solve: the table of the simulation it's for ([`crate::Simulation::body_tuning`]),
    /// which [`crate::Simulation::attach_body`] holds it to.
    pub tuning: BodyTuning,
    pub floor: f32,
    pub scale: f32,
    pub morphs: &'a [(String, f32)],
    pub contact_joints: &'a [&'a str],
    pub seed: u32,
    pub ground_turn: bool,
    pub grounded_gather: bool,
    pub look_height: f32,
    pub look_weight: f32,
    pub head_authority: Option<HeadAuthority>,
}

#[derive(Debug, thiserror::Error)]
pub enum BodyBindError {
    #[error("fixed body profile values are out of range")]
    Profile,
    #[error("the cooked rig cannot bind to the skeleton: {0}")]
    Rig(#[from] sapphire_assets::rig::RigError),
    #[error("the cooked graph could not be read: {0}")]
    Graph(#[from] sapphire_assets::graph::GraphError),
    #[error("the cooked graph cannot bind: {0}")]
    GraphBind(#[from] sapphire_body_pose::GraphBindError),
    #[error("the rig's constraints cannot bind: {0}")]
    Constraints(#[from] sapphire_body_pose::ConstraintCycle),
    #[error("the fixed body clip bank differs from the graph's unique clip IDs")]
    Clips,
    #[error("clip {0} was cooked for a different skeleton")]
    ClipSkeleton(AssetId),
    #[error("clip {0} refers to a morph target outside the profile")]
    ClipMorph(AssetId),
    #[error("the fixed body contact joint {0:?} is absent or ambiguous")]
    Contact(String),
}

pub(super) struct BodyClips(Vec<(AssetId, CpuResident<Clip>)>);

impl ClipSource for BodyClips {
    fn clip(&self, id: AssetId) -> Option<&ArchivedClip> {
        let index = self.0.binary_search_by_key(&id, |(candidate, _)| *candidate).ok()?;
        Some(self.0[index].1.view())
    }
}

/// A bound body profile with fresh fixed graph and support state.
pub struct BodyProfile {
    pub(super) owner: FixedBody<BodyClips>,
    pub(super) scale: f32,
    pub(super) look_height: f32,
    pub(super) look_weight: f32,
    pub(super) grounded_gather: bool,
    pub(super) tuning: BodyTuning,
    fingerprint: u64,
}

impl BodyProfile {
    /// Validates the full clip/rig binding and allocates the fixed owner's storage.
    pub fn bind(assets: BodyAssets, options: BodyProfileOptions<'_>) -> Result<Self, BodyBindError> {
        Self::bind_graph(assets.graph, RigBinding::Cooked(&assets.rig), options)
    }

    /// Binds a graph whose presentation finds humanoid joints by name and uses no VRM constraints.
    pub fn bind_named(
        assets: BodyGraphAssets,
        bones: &[(HumanBone, usize)],
        eye: Vec3,
        options: BodyProfileOptions<'_>,
    ) -> Result<Self, BodyBindError> {
        Self::bind_graph(assets, RigBinding::Named { bones, eye }, options)
    }

    fn bind_graph(
        mut assets: BodyGraphAssets,
        rig_binding: RigBinding<'_>,
        options: BodyProfileOptions<'_>,
    ) -> Result<Self, BodyBindError> {
        let skeleton = assets.skeleton.view();
        let rig = rig_binding.cooked();
        match rig_binding {
            RigBinding::Cooked(rig) => rig.view().check_skeleton(skeleton)?,
            RigBinding::Named { bones, eye } => {
                if !eye.is_finite()
                    || bones.is_empty()
                    || bones.iter().enumerate().any(|(i, &(bone, joint))| {
                        joint >= skeleton.len() || bones[..i].iter().any(|&(prior, _)| prior == bone)
                    })
                {
                    return Err(BodyBindError::Profile);
                }
            }
        }
        let unit = |weight: f32| (0.0..=1.0).contains(&weight);
        let head_valid = options.head_authority.is_none_or(|head| {
            unit(head.retained)
                && head.source_ticks.iter().all(|tick| tick.is_finite() && *tick >= 0.0)
                && head.source_ticks.windows(2).all(|ticks| ticks[0] <= ticks[1])
        });
        if !(options.floor.is_finite()
            && options.scale.is_finite()
            && options.scale > 0.0
            && options.look_height.is_finite()
            && unit(options.look_weight)
            && options.morphs.len() == rig.map_or(0, |rig| rig.morph_target_count.to_native() as usize)
            && !options.contact_joints.is_empty()
            && head_valid)
        {
            return Err(BodyBindError::Profile);
        }
        let mut morph_names = Vec::with_capacity(options.morphs.len());
        let mut morph_weights = Vec::with_capacity(options.morphs.len());
        for (name, weight) in options.morphs {
            if name.is_empty() || !unit(*weight) || morph_names.contains(name) {
                return Err(BodyBindError::Profile);
            }
            morph_names.push(name.clone());
            morph_weights.push(*weight);
        }

        let mut wanted: Vec<_> = assets.graph.view().clip_ids().collect();
        wanted.sort_unstable();
        wanted.dedup();
        assets.clips.sort_unstable_by_key(|(id, _)| *id);
        if !wanted.iter().copied().eq(assets.clips.iter().map(|(id, _)| *id)) {
            return Err(BodyBindError::Clips);
        }
        for (id, clip) in &assets.clips {
            if clip.view().skeleton.as_ref() != Some(&assets.skeleton_id) {
                return Err(BodyBindError::ClipSkeleton(*id));
            }
            if clip.view().weights.iter().any(|track| track.target.to_native() as usize >= morph_names.len())
            {
                return Err(BodyBindError::ClipMorph(*id));
            }
        }
        let mut contacts = Vec::with_capacity(options.contact_joints.len());
        for &name in options.contact_joints {
            let mut matching = skeleton
                .names
                .iter()
                .enumerate()
                .filter_map(|(joint, found)| (found.as_str() == name).then_some(joint));
            let joint = matching.next().filter(|_| matching.next().is_none());
            contacts.push(joint.ok_or_else(|| BodyBindError::Contact(name.to_owned()))?);
        }
        contacts.sort_unstable();
        contacts.dedup();
        let fingerprint = fingerprint(&assets, rig_binding, &options, &contacts);

        let graph = assets.graph.view().to_owned_graph()?;
        let clips = BodyClips(assets.clips);
        let binding = GraphBinding::new(&graph, skeleton, rig, morph_names.len(), &clips)?;
        let constraints: Vec<_> = rig
            .into_iter()
            .flat_map(|rig| rig.constraints.iter())
            .map(ArchivedConstraint::to_native_constraint)
            .collect();
        let pose = CharacterPose::try_new(skeleton, &[], &constraints, morph_weights)?;
        let pose = if rig.is_some() { pose.with_morph_names(morph_names) } else { pose };
        let graph = FixedGraphHistory::new(Arc::new(binding), pose, options.seed, clips);
        let (ik, spine) = match rig_binding {
            RigBinding::Cooked(rig) => {
                let rig = rig.view();
                let spine = rig
                    .humanoid
                    .iter()
                    .find(|entry| entry.bone == HumanBone::Spine)
                    .map(|entry| usize::from(entry.joint.to_native()));
                (IkRig::from_cooked(graph.pose(), rig, options.floor), spine)
            }
            RigBinding::Named { bones, eye } => {
                let spine =
                    bones.iter().find(|&&(bone, _)| bone == HumanBone::Spine).map(|&(_, joint)| joint);
                (IkRig::new(graph.pose(), bones, eye, None, options.floor), spine)
            }
        };
        let ik = CharacterIk::new(ik, options.tuning.feet, options.tuning.look);
        let policy = FixedBodyPolicy {
            ground_turn: options.ground_turn,
            spine,
            head_look: options
                .head_authority
                .map(|head| AuthoredHeadLook::new(head.movement, head.retained, head.source_ticks)),
        };
        let owner = FixedBody::new(graph, ik, policy, &contacts);
        Ok(Self {
            owner,
            scale: options.scale,
            look_height: options.look_height,
            look_weight: options.look_weight,
            grounded_gather: options.grounded_gather,
            tuning: options.tuning,
            fingerprint,
        })
    }

    /// The table its feet and look solve by.
    #[must_use]
    pub fn tuning(&self) -> &BodyTuning {
        &self.tuning
    }

    /// Hash of the exact cooked inputs and all static body policy. The table isn't in it: a simulation's
    /// `tables_hash` takes that once, and holds every body it binds to it.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

fn blob(hash: &mut StateHasher, bytes: &[u8]) {
    hash.u64(bytes.len() as u64);
    hash.bytes(bytes);
}

fn fingerprint(
    assets: &BodyGraphAssets,
    rig: RigBinding<'_>,
    options: &BodyProfileOptions<'_>,
    contacts: &[usize],
) -> u64 {
    let mut hash = StateHasher::new();
    hash.bytes(match rig {
        RigBinding::Cooked(_) => b"sapphire.fixed-body-profile.v1".as_slice(),
        RigBinding::Named { .. } => b"sapphire.fixed-named-body-profile.v1".as_slice(),
    });
    blob(&mut hash, assets.graph.cooked_bytes());
    hash.bytes(&assets.skeleton_id.to_bytes());
    blob(&mut hash, assets.skeleton.cooked_bytes());
    match rig {
        RigBinding::Cooked(rig) => blob(&mut hash, rig.cooked_bytes()),
        RigBinding::Named { bones, eye } => {
            hash.u32(bones.len() as u32);
            for &(bone, joint) in bones {
                hash.u8(bone as u8);
                hash.u32(joint as u32);
            }
            hash.f32s(&eye.to_array());
        }
    }
    hash.u32(assets.clips.len() as u32);
    for (id, clip) in &assets.clips {
        hash.bytes(&id.to_bytes());
        blob(&mut hash, clip.cooked_bytes());
    }
    hash.f32s(&[options.floor, options.scale, options.look_height, options.look_weight]);
    hash.u32(options.seed);
    hash.u8(u8::from(options.ground_turn));
    hash.u32(options.morphs.len() as u32);
    for (name, weight) in options.morphs {
        blob(&mut hash, name.as_bytes());
        hash.f32(*weight);
    }
    hash.u32(contacts.len() as u32);
    for &joint in contacts {
        hash.u32(joint as u32);
    }
    if let Some(head) = options.head_authority {
        hash.u8(1);
        hash.u64(head.movement.as_u64());
        hash.f32(head.retained);
        hash.f32s(&head.source_ticks);
    } else {
        hash.u8(0);
    }
    if !options.grounded_gather {
        hash.bytes(b"sapphire.fixed-body.no-gather");
    }
    hash.finish()
}
