//! One character's pose, as a frame evaluates it, allocating nothing, and [`evaluate_all`], which
//! gives each character its own job.

use std::time::Duration;

use sapphire_assets::clip::ArchivedClip;
use sapphire_assets::mesh::SkinBind;
use sapphire_assets::rig::Constraint;
use sapphire_assets::skeleton::{ArchivedSkeleton, NO_PARENT};
use sapphire_math::{Affine3A, Quat, Transform};

use crate::constraints::Constraints;
use crate::model::{self, Bind};
use crate::sample;

/// One character's pose: the rig data it reads, copied out of its assets once, and the buffers each
/// step writes.
///
/// A frame's order: [`Self::sample`] a clip (or write [`Self::locals_mut`] from a graph, blending with
/// this crate's functions), then [`Self::finish`] runs the rig's constraints and local-to-model, and
/// [`Self::write_palette`] fills what the renderer reads.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterPose {
    parents: Vec<u16>,
    rest: Vec<Transform>,
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
}

impl CharacterPose {
    /// A pose at rest for `skeleton`, skinned by `binds`, following `constraints`, with
    /// `rest_weights` for its morph targets. Allocates; the rest of a frame's work doesn't.
    #[must_use]
    pub fn new(
        skeleton: &ArchivedSkeleton,
        binds: &[SkinBind],
        constraints: &[Constraint],
        rest_weights: Vec<f32>,
    ) -> Self {
        let parents: Vec<u16> = skeleton.parents.iter().map(|parent| parent.to_native()).collect();
        let rest = sample::rest_pose(skeleton);
        let mut rest_world: Vec<Quat> = Vec::with_capacity(rest.len());
        for (joint, local) in rest.iter().enumerate() {
            let parent = parents[joint];
            let above = if parent == NO_PARENT { Quat::IDENTITY } else { rest_world[usize::from(parent)] };
            rest_world.push(above * local.rotation);
        }
        let count = rest.len();
        let mut pose = Self {
            constraints: Constraints::new(&parents, constraints),
            binds: Bind::from_skin(binds),
            parents,
            locals: rest.clone(),
            world: rest_world.clone(),
            rest_world,
            rest,
            weights: rest_weights.clone(),
            rest_weights,
            model: vec![Affine3A::IDENTITY; count],
        };
        pose.finish();
        pose
    }

    /// How many joints it has.
    #[must_use]
    pub fn joint_count(&self) -> usize {
        self.parents.len()
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
        let parent = self.parents[joint];
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

    /// Runs the rig's constraints, then local-to-model. Allocates nothing.
    pub fn finish(&mut self) {
        self.constraints.apply(&self.parents, &self.rest, &mut self.locals, &mut self.world, &mut self.model);
        model::walk(self.parents.iter().copied(), &self.locals, &mut self.model);
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

/// Runs `evaluate` on each character, one job each (spec §5.13): a single character runs on the
/// calling thread. The job system allocates per call past one (`sapphire_jobs`' docs), so this
/// stays out of the fixed step, as all of presentation does.
pub fn evaluate_all<T: Send>(characters: &mut [T], evaluate: impl Fn(&mut T) + Sync) {
    sapphire_jobs::par_for_each_chunk_mut(characters, 1, |_, chunk| chunk.iter_mut().for_each(&evaluate));
}

/// What animation did in a frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnimStats {
    /// Characters with a clip to play.
    pub characters: u32,
    /// Characters whose pose was evaluated: those with a clip whose position moved.
    pub evaluated: u32,
    /// Joints in the poses evaluated.
    pub joints: u32,
    /// Characters whose spring bones wrote their pose ([`SpringBones`](crate::SpringBones)).
    pub springs: u32,
    /// Spring sub-steps run, all characters together.
    pub spring_substeps: u32,
    /// The time spring bones took, all characters together, including finishing their poses again.
    /// Within [`Self::time`], unless jobs ran side by side.
    pub spring_time: Duration,
    /// Wall time of the whole of it, the jobs and handing their results over.
    pub time: Duration,
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use sapphire_assets::clip::{
        Clip, DEFAULT_RATE, PackedRotation, RotationTrack, VectorRange, VectorTrack,
    };
    use sapphire_assets::rig::{AimAxis, Axis, ConstraintKind};
    use sapphire_assets::skeleton::{Joint, JointTransform, Skeleton};
    use sapphire_math::{Mat4, Vec3};

    use super::*;
    use crate::{local_to_model, overlay, root_motion};

    fn joint(name: &str, parent: Option<u16>, translation: [f32; 3]) -> Joint {
        Joint {
            name: name.to_owned(),
            parent,
            rest: JointTransform { translation, ..JointTransform::IDENTITY },
        }
    }

    /// A shoulder at the origin with an upper arm along +X, a forearm 0.3 m on, and two helpers
    /// beside the upper arm: one aiming its +X at the forearm, one rolling with it about X.
    fn arm_joints() -> Vec<Joint> {
        vec![
            joint("shoulder", None, [0.0, 1.4, 0.0]),
            joint("upper", Some(0), [0.1, 0.0, 0.0]),
            joint("lower", Some(1), [0.3, 0.0, 0.0]),
            joint("aim", Some(0), [0.1, 0.0, 0.0]),
            joint("roll", Some(1), [0.3, 0.0, 0.0]),
        ]
    }

    /// Inverse binds of the rest pose: each joint's model-space translation undone.
    fn arm_binds() -> Vec<SkinBind> {
        [[0.0, 1.4, 0.0], [0.1, 1.4, 0.0], [0.4, 1.4, 0.0], [0.1, 1.4, 0.0], [0.4, 1.4, 0.0]]
            .iter()
            .enumerate()
            .map(|(joint, &[x, y, z])| SkinBind {
                joint: joint as u16,
                inverse_bind_matrix: Mat4::from_translation(-Vec3::new(x, y, z)).to_cols_array_2d(),
            })
            .collect()
    }

    fn arm_constraints() -> [Constraint; 2] {
        [
            Constraint {
                joint: 3,
                source: 2,
                weight: 1.0,
                kind: ConstraintKind::Aim { axis: AimAxis::PositiveX },
            },
            Constraint { joint: 4, source: 2, weight: 0.5, kind: ConstraintKind::Roll { axis: Axis::X } },
        ]
    }

    fn arm() -> CharacterPose {
        let bytes = Skeleton::new(&arm_joints()).to_cooked().expect("cooked");
        let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
        CharacterPose::new(skeleton, &arm_binds(), &arm_constraints(), vec![0.25, 0.0])
    }

    /// The arm's clip: the upper arm turning a quarter about Z and the shoulder rising 0.1 m over
    /// 30 ticks, the forearm twisting about X, the second morph target from 0 to 1, and root motion.
    fn swing() -> Clip {
        let half = core::f32::consts::FRAC_1_SQRT_2;
        let rise = VectorRange::enclosing([[0.0, 1.4, 0.0], [0.0, 1.5, 0.0]]);
        let weight = sapphire_assets::clip::WeightRange::enclosing([0.0, 1.0]);
        let root = VectorRange::enclosing([[0.0, 0.0, 0.0], [0.0, 0.0, 3.0]]);
        let twist = Quat::from_rotation_x(0.6).to_array();
        Clip {
            name: "swing".into(),
            skeleton: Some(sapphire_assets::AssetId::from_bytes([1; 16])),
            joint_count: 5,
            morph_target_count: 2,
            rate: DEFAULT_RATE,
            ticks: 30,
            rotations: vec![
                RotationTrack { joint: 1, count: 2, first: 0 },
                RotationTrack { joint: 2, count: 2, first: 2 },
            ],
            translations: vec![VectorTrack { joint: 0, count: 2, first: 0, range: rise }],
            weights: vec![sapphire_assets::clip::WeightTrack {
                target: 1,
                count: 2,
                first: 0,
                range: weight,
            }],
            root: Some(sapphire_assets::clip::RootMotion {
                joint: 0,
                translation: VectorTrack { joint: 0, count: 2, first: 2, range: root },
                rotation: RotationTrack { joint: 0, count: 1, first: 4 },
            }),
            rotation_ticks: vec![0, 30, 0, 30, 0],
            rotation_keys: vec![
                PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
                PackedRotation::encode([0.0, 0.0, half, half]),
                PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
                PackedRotation::encode(twist),
                PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
            ],
            vector_ticks: vec![0, 30, 0, 30],
            vector_keys: vec![
                rise.encode([0.0, 1.4, 0.0]),
                rise.encode([0.0, 1.5, 0.0]),
                root.encode([0.0, 0.0, 0.0]),
                root.encode([0.0, 0.0, 3.0]),
            ],
            weight_ticks: vec![0, 30],
            weight_keys: vec![weight.encode(0.0), weight.encode(1.0)],
            ..Clip::default()
        }
    }

    /// Every output of a pose as bits.
    fn bytes_of(pose: &CharacterPose) -> Vec<u32> {
        let mut bits = Vec::new();
        for joint in pose.locals() {
            bits.extend(joint.translation.to_array().map(f32::to_bits));
            bits.extend(joint.rotation.to_array().map(f32::to_bits));
            bits.extend(joint.scale.to_array().map(f32::to_bits));
        }
        let mut palette = vec![Affine3A::IDENTITY; pose.palette_len()];
        pose.write_palette(&mut palette);
        for matrix in pose.model().iter().chain(&palette) {
            bits.extend(matrix.to_cols_array().map(f32::to_bits));
        }
        bits.extend(pose.weights().iter().map(|weight| weight.to_bits()));
        bits
    }

    #[test]
    fn the_rest_pose_draws_the_bind_pose() {
        let pose = arm();
        let mut palette = vec![Affine3A::ZERO; pose.palette_len()];
        pose.write_palette(&mut palette);
        assert!(palette.iter().all(|entry| entry.abs_diff_eq(Affine3A::IDENTITY, 1e-6)), "{palette:?}");
        assert_eq!(pose.weights(), [0.25, 0.0]);
    }

    #[test]
    fn a_clip_moves_the_joints_it_names_over_their_rest_and_helpers_follow() {
        let bytes = swing().to_cooked().expect("cooked");
        let clip = Clip::read(&bytes).expect("a valid clip");
        let mut pose = arm();
        pose.sample(clip, 30.0);
        pose.finish();
        // The shoulder rose, the upper arm turned a quarter up, and the elbow hangs from them.
        let shoulder = Vec3::from(pose.model()[0].translation);
        assert!(shoulder.abs_diff_eq(Vec3::new(0.0, 1.5, 0.0), 1e-4), "{shoulder}");
        let elbow = Vec3::from(pose.model()[2].translation);
        assert!(elbow.abs_diff_eq(Vec3::new(0.1, 1.8, 0.0), 1e-3), "{elbow}");
        // The aim helper points at the elbow from its own place beside the upper arm.
        let aim = pose.model()[3];
        let along = aim.transform_vector3(Vec3::X).normalize();
        let expected = (elbow - Vec3::from(aim.translation)).normalize();
        assert!(along.abs_diff_eq(expected, 1e-3), "{along} against {expected}");
        // The roll helper takes half the forearm's twist about X.
        let upper = pose.model()[1].matrix3;
        let roll = pose.model()[4].matrix3;
        let relative = Quat::from_mat3a(&(upper.inverse() * roll));
        assert!(relative.abs_diff_eq(Quat::from_rotation_x(0.3), 1e-3), "{relative}");
        // A track's weight goes over the rest weights, the other target's stays.
        assert!((pose.weights()[1] - 1.0).abs() < 1e-5);
        assert_eq!(pose.weights()[0], 0.25);
        // Skinning moves a bind-pose point on the forearm with it.
        let mut palette = vec![Affine3A::IDENTITY; pose.palette_len()];
        pose.write_palette(&mut palette);
        let skinned = palette[2].transform_point3(Vec3::new(0.6, 1.4, 0.0));
        assert!((skinned.distance(elbow) - 0.2).abs() < 1e-3, "{skinned}");
    }

    /// The constraints' scratch must not leak from one frame to the next: an aim's chain is
    /// rebuilt from the pose it reads, however the joints got there.
    #[test]
    fn the_same_clip_time_gives_the_same_bytes() {
        let bytes = swing().to_cooked().expect("cooked");
        let clip = Clip::read(&bytes).expect("a valid clip");
        let positions = [0.0, 0.5, 7.25, 15.0, 29.999, 30.0, 45.0];
        let mut a = arm();
        let expected: Vec<Vec<u32>> = positions
            .iter()
            .map(|&position| {
                a.sample(clip, position);
                a.finish();
                bytes_of(&a)
            })
            .collect();
        // Another pose, cooked again, visiting them in another order, repeatedly.
        let again = swing().to_cooked().expect("cooked");
        assert_eq!(bytes.as_slice(), again.as_slice(), "a clip cooks to the same bytes");
        let clip = Clip::read(&again).expect("a valid clip");
        let mut b = arm();
        for round in 0..3 {
            for (index, &position) in positions.iter().enumerate().rev() {
                b.sample(clip, position);
                b.finish();
                assert_eq!(bytes_of(&b), expected[index], "position {position}, round {round}");
            }
        }
        // And the plain functions a tool calls agree with the character's bits, before constraints.
        let cooked = Skeleton::new(&arm_joints()).to_cooked().expect("cooked");
        let skeleton = Skeleton::read(&cooked).expect("a valid skeleton");
        let mut model = vec![Affine3A::IDENTITY; skeleton.len()];
        local_to_model(skeleton, b.locals(), &mut model);
        assert_eq!(
            model.iter().map(|m| m.to_cols_array().map(f32::to_bits)).collect::<Vec<_>>(),
            b.model().iter().map(|m| m.to_cols_array().map(f32::to_bits)).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn root_motion_is_read_apart_from_the_pose() {
        let bytes = swing().to_cooked().expect("cooked");
        let clip = Clip::read(&bytes).expect("a valid clip");
        let root = root_motion(clip, 15.0).expect("it has root motion");
        assert!(root.translation.abs_diff_eq(Vec3::new(0.0, 0.0, 1.5), 1e-3), "{root:?}");
        assert!(root.rotation.abs_diff_eq(Quat::IDENTITY, 1e-3));
        assert_eq!(root.scale, Vec3::ONE);
        let plain = Clip { root: None, ..swing() };
        let bytes = plain.to_cooked().expect("cooked");
        assert_eq!(root_motion(Clip::read(&bytes).expect("a valid clip"), 15.0), None);
    }

    /// A clip that rests where it says, with no track for a joint, leaves that joint's rest bits.
    #[test]
    fn a_joint_with_no_track_keeps_its_rest_bits() {
        let bytes = swing().to_cooked().expect("cooked");
        let clip = Clip::read(&bytes).expect("a valid clip");
        let rest = arm().rest().to_vec();
        let mut locals = rest.clone();
        overlay(clip, 12.0, &mut locals);
        assert_eq!(locals[3], rest[3]);
        assert_eq!(locals[4], rest[4]);
        assert_ne!(locals[1], rest[1]);
    }

    #[test]
    fn a_constraint_waits_for_those_that_move_its_source() {
        // A helper aiming at the end of a chain whose first joint follows a turned bone: listed
        // first, it must still aim where the chain ends up.
        let joints = vec![
            joint("", None, [0.0; 3]),
            joint("", Some(0), [0.1, 0.0, 0.0]),
            joint("", Some(1), [0.3, 0.0, 0.0]),
            joint("", Some(0), [0.1, 0.0, 0.0]),
            joint("", Some(0), [0.0, 0.1, 0.0]),
        ];
        let constraints = [
            Constraint {
                joint: 3,
                source: 2,
                weight: 1.0,
                kind: ConstraintKind::Aim { axis: AimAxis::PositiveX },
            },
            Constraint { joint: 1, source: 4, weight: 1.0, kind: ConstraintKind::Rotation },
        ];
        let bytes = Skeleton::new(&joints).to_cooked().expect("cooked");
        let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
        let mut pose = CharacterPose::new(skeleton, &[], &constraints, Vec::new());
        pose.reset();
        pose.locals_mut()[4].rotation = Quat::from_rotation_z(FRAC_PI_2);
        pose.finish();
        let end = Vec3::from(pose.model()[2].translation);
        assert!(end.abs_diff_eq(Vec3::new(0.1, 0.3, 0.0), 1e-5), "{end}");
        let aim = pose.model()[3].transform_vector3(Vec3::X).normalize();
        assert!(aim.abs_diff_eq(Vec3::Y, 1e-4), "aims at the chain's end: {aim}");
    }

    /// Every character gets its own job and ends up as it would alone.
    #[test]
    fn every_character_is_evaluated_once() {
        let bytes = swing().to_cooked().expect("cooked");
        let clip = Clip::read(&bytes).expect("a valid clip");
        let mut poses = vec![arm(); 8];
        evaluate_all(&mut poses, |pose| {
            pose.sample(clip, 12.5);
            pose.finish();
        });
        let mut alone = arm();
        alone.sample(clip, 12.5);
        alone.finish();
        let expected = bytes_of(&alone);
        for pose in &poses {
            assert_eq!(bytes_of(pose), expected);
        }
    }
}
