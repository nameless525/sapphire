//! Rig layout and CPU pose storage shared by sampling, graph evaluation and IK.

use std::sync::Arc;

use sapphire_assets::clip::ArchivedClip;
use sapphire_assets::mesh::SkinBind;
use sapphire_assets::rig::Constraint;
use sapphire_assets::skeleton::{ArchivedSkeleton, NO_PARENT};
use sapphire_math::{Affine3A, Quat, Transform};

use crate::constraints::Constraints;
use crate::ik::SolvePose;
use crate::model::{self, Bind};
use crate::pose_math::PoseMath;
use crate::sample;

/// Immutable joint identities and rest data, shared with secondary motion for reload checks.
#[derive(Clone, Debug)]
#[doc(hidden)]
pub struct JointLayout {
    pub(crate) parents: Vec<u16>,
    pub(crate) names: Vec<String>,
    legacy_rest: Arc<[Transform]>,
    raw_rest_rotations: Vec<Quat>,
}

impl JointLayout {
    fn same_cooked_layout(&self, other: &Self) -> bool {
        let rotation = |value: &Quat| value.to_array().map(f32::to_bits);
        let rest = |value: &Transform| {
            (value.translation.to_array().map(f32::to_bits), value.scale.to_array().map(f32::to_bits))
        };
        self.parents == other.parents
            && self.names == other.names
            && self.raw_rest_rotations.iter().map(rotation).eq(other.raw_rest_rotations.iter().map(rotation))
            && self.legacy_rest.iter().map(rest).eq(other.legacy_rest.iter().map(rest))
    }
}

impl PartialEq for JointLayout {
    fn eq(&self, other: &Self) -> bool {
        self.parents == other.parents && self.names == other.names && self.legacy_rest == other.legacy_rest
    }
}

/// One character's pose: the rig data it reads, copied out of its assets once, and the buffers each
/// step writes.
///
/// A frame's order: [`Self::sample`] a clip (or write [`Self::locals_mut`] from a graph, blending with
/// this crate's functions), then [`Self::finish`] runs the rig's constraints and local-to-model, and
/// [`Self::write_palette`] fills what the renderer reads.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterPose {
    layout: Arc<JointLayout>,
    rest: Arc<[Transform]>,
    math: PoseMath,
    morph_names: Option<Vec<String>>,
    /// Each joint's rest rotation in model space.
    rest_world: Vec<Quat>,
    binds: Vec<Bind>,
    constraints: Constraints,
    rest_weights: Vec<f32>,
    locals: Vec<Transform>,
    /// Scratch for the constraints' aims: rotations down their chains.
    world: Vec<Quat>,
    model: Vec<Affine3A>,
    weights: Vec<f32>,
    /// Set only on a presentation pose populated by a fixed body read.
    cosmetic_protection: Option<Arc<[bool]>>,
}

/// Immutable locals and model transforms after body IK and rig constraints, with the morph
/// channels from the same solve. The borrow prevents cosmetic writers from changing this sample.
pub struct SolvedBody<'a> {
    pose: &'a CharacterPose,
}

impl SolvedBody<'_> {
    /// Joint-local transforms in skeleton order.
    #[must_use]
    pub fn locals(&self) -> &[Transform] {
        self.pose.locals()
    }

    /// Post-constraint model transforms in skeleton order.
    #[must_use]
    pub fn model(&self) -> &[Affine3A] {
        self.pose.model()
    }

    /// Morph identities and values in binding order. An old unnamed rig retains its values.
    #[must_use]
    pub fn morphs(&self) -> (Option<&[String]>, &[f32]) {
        (self.pose.morph_names.as_deref(), self.pose.weights())
    }
}

impl CharacterPose {
    /// A pose at rest for `skeleton`, skinned by `binds`, following `constraints`, with
    /// `rest_weights` for its morph targets. Allocates; the rest of a frame's work doesn't.
    ///
    /// # Panics
    ///
    /// If the rig and skeleton have a dependency cycle. Use [`Self::try_new`] for authored assets.
    #[must_use]
    pub fn new(
        skeleton: &ArchivedSkeleton,
        binds: &[SkinBind],
        constraints: &[Constraint],
        rest_weights: Vec<f32>,
    ) -> Self {
        Self::try_new(skeleton, binds, constraints, rest_weights)
            .expect("constraints and skeleton must have no dependency cycle")
    }

    /// Builds a pose, refusing cycles in the combined skeleton and constraint graph.
    pub fn try_new(
        skeleton: &ArchivedSkeleton,
        binds: &[SkinBind],
        constraints: &[Constraint],
        rest_weights: Vec<f32>,
    ) -> Result<Self, crate::constraints::ConstraintCycle> {
        let parents: Vec<u16> = skeleton.parents.iter().map(|parent| parent.to_native()).collect();
        let rest: Arc<[Transform]> = sample::rest_pose(skeleton).into();
        let raw_rest_rotations =
            skeleton.rest.iter().map(|joint| Quat::from_array(joint.to_native().rotation)).collect();
        let rest_world = rest_rotations(&parents, &rest, PoseMath::Legacy);
        let count = rest.len();
        let mut pose = Self {
            constraints: Constraints::try_new(&parents, constraints)?,
            binds: Bind::from_skin(binds),
            locals: rest.to_vec(),
            rest: rest.clone(),
            math: PoseMath::Legacy,
            layout: Arc::new(JointLayout {
                parents,
                names: skeleton.names.iter().map(|name| name.as_str().to_owned()).collect(),
                legacy_rest: rest,
                raw_rest_rotations,
            }),
            morph_names: None,
            world: rest_world.clone(),
            rest_world,
            weights: rest_weights.clone(),
            rest_weights,
            model: vec![Affine3A::IDENTITY; count],
            cosmetic_protection: None,
        };
        pose.finish();
        Ok(pose)
    }

    /// Rebinds the rest pose from archived rotation components. The joint identity stays
    /// shared with presentation, whose legacy rest and cosmetic policy remain separate.
    pub(crate) fn into_fixed(mut self) -> Self {
        self.math = PoseMath::Pinned;
        self.constraints.set_math(self.math);
        let mut rest = self.rest.to_vec();
        for (joint, &raw) in rest.iter_mut().zip(&self.layout.raw_rest_rotations) {
            joint.rotation = self.math.normalize(raw);
        }
        self.rest_world = rest_rotations(&self.layout.parents, &rest, self.math);
        self.rest = rest.into();
        self.world.copy_from_slice(&self.rest_world);
        self.cosmetic_protection = None;
        self.reset();
        self.finish();
        self
    }

    pub(crate) fn math(&self) -> PoseMath {
        self.math
    }

    /// Records the mesh's ordered morph identities for reload compatibility. Call when a pose
    /// has morph targets; counts alone cannot establish that indices name the same targets.
    ///
    /// # Panics
    ///
    /// If `names` does not have one entry per morph target.
    #[must_use]
    pub fn with_morph_names(mut self, names: Vec<String>) -> Self {
        assert_eq!(names.len(), self.rest_weights.len(), "one name per morph target");
        self.morph_names = Some(names);
        self
    }

    /// Whether pose history can be copied by index across a character reload. Unknown morph
    /// identities conservatively refuse transfer when either pose has morph targets.
    #[must_use]
    pub fn compatible_with(&self, other: &Self) -> bool {
        self.layout == other.layout
            && self.rest_weights == other.rest_weights
            && (self.rest_weights.is_empty()
                || self.morph_names.as_ref().zip(other.morph_names.as_ref()).is_some_and(|(a, b)| a == b))
    }

    pub(crate) fn bind_body_layout(&mut self, layout: &Arc<JointLayout>) -> bool {
        if !self.layout.same_cooked_layout(layout) {
            return false;
        }
        self.layout = layout.clone();
        true
    }

    pub(crate) fn adopt_body_sample(
        &mut self,
        joints: &Arc<JointLayout>,
        protected: &Arc<[bool]>,
        locals: &[Transform],
        model: &[Affine3A],
        weights: &[f32],
    ) -> bool {
        if !Arc::ptr_eq(joints, &self.layout) || self.weights.len() != weights.len() {
            return false;
        }
        self.locals.copy_from_slice(locals);
        self.model.copy_from_slice(model);
        self.weights.copy_from_slice(weights);
        self.cosmetic_protection = Some(protected.clone());
        true
    }

    /// Returns a detached presentation pose to ordinary constraint evaluation.
    pub fn clear_body_presentation(&mut self) {
        self.cosmetic_protection = None;
    }

    /// Names in the immutable skeleton order.
    #[must_use]
    pub fn joint_names(&self) -> &[String] {
        &self.layout.names
    }

    pub(crate) fn contact_closure(&self, contacts: &[usize]) -> Vec<bool> {
        self.constraints.contact_closure(&self.layout.parents, contacts)
    }

    pub(crate) fn morph_names(&self) -> Option<&[String]> {
        self.morph_names.as_deref()
    }

    /// Layout identity used by the secondary-motion adapter.
    #[doc(hidden)]
    pub fn joint_layout(&self) -> Arc<JointLayout> {
        self.layout.clone()
    }

    /// How many joints it has.
    #[must_use]
    pub fn joint_count(&self) -> usize {
        self.layout.parents.len()
    }

    /// How many entries its palette has.
    #[must_use]
    pub fn palette_len(&self) -> usize {
        self.binds.len()
    }

    /// The skeleton's rest pose.
    #[must_use]
    pub fn rest(&self) -> &[Transform] {
        &self.rest
    }

    /// `joint`'s parent, or `None` for a root.
    #[must_use]
    pub fn parent(&self, joint: usize) -> Option<usize> {
        let parent = self.layout.parents[joint];
        (parent != NO_PARENT).then_some(usize::from(parent))
    }

    /// `joint`'s rest rotation in model space.
    #[must_use]
    pub fn rest_rotation(&self, joint: usize) -> Quat {
        self.rest_world[joint]
    }

    /// Back to the rest pose and rest weights, before [`Self::finish`].
    pub fn reset(&mut self) {
        self.locals.copy_from_slice(&self.rest);
        self.weights.copy_from_slice(&self.rest_weights);
    }

    /// `clip` at `position` over the rest pose and rest weights ([`sample()`],
    /// [`sample::overlay_weights`]), before [`Self::finish`]. Allocates nothing.
    pub fn sample(&mut self, clip: &ArchivedClip, position: f32) {
        sample(clip, position, &self.rest, &mut self.locals);
        self.weights.copy_from_slice(&self.rest_weights);
        sample::overlay_weights(clip, position, &mut self.weights);
    }

    /// The morph weights it rests at.
    #[must_use]
    pub fn rest_weights(&self) -> &[f32] {
        &self.rest_weights
    }

    /// The local pose being made, for whatever blends or solves it before [`Self::finish`].
    #[must_use]
    pub fn locals(&self) -> &[Transform] {
        &self.locals
    }

    pub fn locals_mut(&mut self) -> &mut [Transform] {
        &mut self.locals
    }

    /// The morph weights being made.
    #[must_use]
    pub fn weights(&self) -> &[f32] {
        &self.weights
    }

    pub fn weights_mut(&mut self) -> &mut [f32] {
        &mut self.weights
    }

    /// The view of this pose that solvers work through ([`crate::two_bone`], [`crate::aim`]), between
    /// sampling and [`Self::finish`]. Nothing in it is up to date until it's refreshed
    /// ([`SolvePose::refresh`]), except what the last `finish` left.
    pub fn solving(&mut self) -> SolvePose<'_> {
        SolvePose::with_math(&self.layout.parents, &mut self.locals, &mut self.model, self.math)
    }

    /// [`Self::solving`], with the morph weights being made beside it.
    pub(crate) fn solving_with_weights(&mut self) -> (SolvePose<'_>, &mut [f32]) {
        (
            SolvePose::with_math(&self.layout.parents, &mut self.locals, &mut self.model, self.math),
            &mut self.weights,
        )
    }

    /// Runs the rig's constraints, then local-to-model. Allocates nothing.
    pub fn finish(&mut self) {
        if let Some(protected) = self.cosmetic_protection.as_deref() {
            self.constraints.apply_excluding(
                &self.layout.parents,
                &self.rest,
                &mut self.locals,
                &mut self.world,
                &mut self.model,
                protected,
            );
            for (joint, &parent) in self.layout.parents.iter().enumerate() {
                if !protected[joint] {
                    self.model[joint] = model::model_of(parent, &self.locals[joint], &self.model);
                }
            }
            return;
        }
        self.constraints.apply(
            &self.layout.parents,
            &self.rest,
            &mut self.locals,
            &mut self.world,
            &mut self.model,
        );
        model::walk(self.layout.parents.iter().copied(), &self.locals, &mut self.model);
    }

    /// Finishes the body once and exposes its solved frame without copying or reopening the pose.
    #[must_use]
    pub fn finish_body(&mut self) -> SolvedBody<'_> {
        self.finish();
        SolvedBody { pose: self }
    }

    /// Every joint's transform in the model's space, as of the last [`Self::finish`]: the sockets
    /// that hold weapons and effects read this.
    #[must_use]
    pub fn model(&self) -> &[Affine3A] {
        &self.model
    }

    /// The skinning palette, as of the last [`Self::finish`] ([`model::palette`]). `out` holds
    /// [`Self::palette_len`] entries. Allocates nothing.
    pub fn write_palette(&self, out: &mut [Affine3A]) {
        model::palette(&self.model, &self.binds, out);
    }
}

fn rest_rotations(parents: &[u16], rest: &[Transform], math: PoseMath) -> Vec<Quat> {
    let mut world = Vec::with_capacity(rest.len());
    for (&parent, local) in parents.iter().zip(rest) {
        let above = if parent == NO_PARENT { Quat::IDENTITY } else { world[usize::from(parent)] };
        world.push(math.multiply(above, local.rotation));
    }
    world
}

#[cfg(test)]
mod tests {
    use sapphire_assets::skeleton::{Joint, JointTransform, Skeleton};
    use sapphire_math::{Vec3, det};

    use super::*;

    #[test]
    fn binds_identical_independent_layout_and_rejects_different_raw_rest() {
        let pose = |w| {
            let bytes = Skeleton::new(&[Joint {
                name: "joint".to_owned(),
                parent: None,
                rest: JointTransform { rotation: [0.0, 0.0, 0.0, w], ..JointTransform::IDENTITY },
            }])
            .to_cooked()
            .unwrap();
            CharacterPose::new(Skeleton::read(&bytes).unwrap(), &[], &[], Vec::new())
        };
        let owner = pose(1.0).into_fixed();
        let mut identical = pose(1.0);
        let mut changed = pose(2.0);
        assert!(!Arc::ptr_eq(&owner.joint_layout(), &identical.joint_layout()));
        assert!(identical.bind_body_layout(&owner.joint_layout()));
        assert!(Arc::ptr_eq(&owner.joint_layout(), &identical.joint_layout()));
        assert!(changed.compatible_with(&identical), "legacy normalized rest matches");
        assert!(!changed.bind_body_layout(&owner.joint_layout()));
        assert!(!Arc::ptr_eq(&owner.joint_layout(), &changed.joint_layout()));
    }

    #[test]
    fn fixed_rest_uses_archived_rotations_and_preserves_presentation_layout() {
        let raw_parent = Quat::from_xyzw(0.13, -0.2, 0.56, 1.25);
        let raw_child = Quat::from_xyzw(-0.4, 0.19, 0.63, 1.08);
        let joint = |name: &str, parent, rotation: Quat| Joint {
            name: name.to_owned(),
            parent,
            rest: JointTransform {
                translation: Vec3::Y.to_array(),
                rotation: rotation.to_array(),
                ..JointTransform::IDENTITY
            },
        };
        let bytes = Skeleton::new(&[joint("parent", None, raw_parent), joint("child", Some(0), raw_child)])
            .to_cooked()
            .unwrap();
        let legacy = CharacterPose::new(Skeleton::read(&bytes).unwrap(), &[], &[], vec![0.25])
            .with_morph_names(vec!["sleeve".to_owned()]);
        let legacy_rest_bits = legacy.rest()[1].rotation.to_array().map(f32::to_bits);
        let fixed = legacy.clone().into_fixed();
        let parent = det::quat_normalize(raw_parent);
        let child = det::quat_normalize(raw_child);
        let expected = det::quat_mul(det::quat_mul(Quat::IDENTITY, parent), child);
        assert_eq!(fixed.rest()[1].rotation.to_array().map(f32::to_bits), child.to_array().map(f32::to_bits));
        assert_eq!(
            fixed.rest_rotation(1).to_array().map(f32::to_bits),
            expected.to_array().map(f32::to_bits)
        );
        assert_eq!(legacy.rest()[1].rotation.to_array().map(f32::to_bits), legacy_rest_bits);
        assert!(Arc::ptr_eq(&legacy.joint_layout(), &fixed.joint_layout()));
        assert_eq!(fixed.weights(), &[0.25]);
        let bind = |pose| {
            crate::CharacterIk::new(
                crate::IkRig::new(pose, &[], Vec3::ZERO, None, 0.0),
                crate::FootSettings::default(),
                crate::LookSettings::default(),
            )
        };
        assert!(bind(&fixed).matches_body(&fixed));
        assert!(!bind(&legacy).matches_body(&fixed), "legacy IK cannot seed fixed support");
        let other = CharacterPose::new(Skeleton::read(&bytes).unwrap(), &[], &[], vec![0.25]).into_fixed();
        assert!(!bind(&other).matches_body(&fixed), "another layout is not this package's rig");
    }
}
