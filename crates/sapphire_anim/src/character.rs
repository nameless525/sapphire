//! Parallel animation work and presentation counters.

use std::time::Duration;

/// How many jobs a frame's characters split into for each thread that runs them, the calling thread
/// among them: more than one, so a dear character (the one on field, with its springs) doesn't leave
/// the others waiting, and few enough that the job system's allocations don't grow with a crowd.
const JOBS_PER_THREAD: usize = 2;

/// Runs `evaluate` on each character in jobs on the compute pool (spec §5.13), each character once,
/// and returns how many jobs that took: 1 for a slice that fits one, which runs on the calling thread
/// and allocates nothing. Characters go to jobs in runs, two for each of the pool's
/// threads and the caller, so the job system's allocations (`sapphire_jobs`' docs: about 3 a call and
/// 1 a job) stay bounded however many characters there are. Nothing here runs in the fixed step, as
/// nothing in presentation does. What one character's evaluation writes must be its own, so the
/// results are those of a plain loop, whatever the thread count.
pub fn evaluate_all<T: Send>(characters: &mut [T], evaluate: impl Fn(&mut T) + Sync) -> usize {
    let threads = sapphire_jobs::system().compute_pool().thread_num() + 1;
    let jobs = characters.len().min(JOBS_PER_THREAD * threads).max(1);
    let run = characters.len().div_ceil(jobs).max(1);
    sapphire_jobs::par_for_each_chunk_mut(characters, run, |_, chunk| chunk.iter_mut().for_each(&evaluate));
    characters.len().div_ceil(run).max(1)
}

/// `list`, emptied, as a list of `U`, keeping its allocation: a frame's list of jobs whose items borrow
/// the frame, kept from one frame to the next as an empty list of the same items at another lifetime,
/// so a frame's list allocates nothing once it has held as many as a frame asks for. `T` and `U` must
/// be laid out alike, which they are when they differ only in lifetimes; the standard library collects
/// such a list in place, and the stage's allocation tests hold that it still does.
#[must_use]
pub fn reuse_list<T, U>(mut list: Vec<T>) -> Vec<U> {
    const {
        assert!(size_of::<T>() == size_of::<U>() && align_of::<T>() == align_of::<U>());
    }
    list.clear();
    list.into_iter().map(|_| unreachable!("the list was emptied")).collect()
}

/// What animation did in a frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnimStats {
    /// Characters with a clip to play.
    pub characters: u32,
    /// Characters whose pose was evaluated: those with a clip whose position moved.
    pub evaluated: u32,
    /// Characters whose pose a level held this frame (M5 C1): off screen, or between the updates of a
    /// band farther than the nearest. Nothing of theirs was evaluated or stepped.
    pub held: u32,
    /// Characters posed by a level below full this frame, held or updated: past the nearest band, on an
    /// interval, or with their IK or springs off by distance.
    pub thinned: u32,
    /// Of [`Self::held`], those no view draws.
    pub off_screen: u32,
    /// The jobs the frame's characters ran in: 0 when they ran one after another on the frame's thread.
    pub jobs: u32,
    /// Joints in the poses evaluated.
    pub joints: u32,
    /// Characters whose spring bones wrote their pose ([`SpringBones`](crate::SpringBones)).
    pub springs: u32,
    /// Spring sub-steps run, all characters together.
    pub spring_substeps: u32,
    /// The time spring bones took, all characters together, including finishing their poses again.
    /// Within [`Self::time`], unless jobs ran side by side.
    pub spring_time: Duration,
    /// How many times these characters have jumped rather than moved, all told: what secondary motion
    /// starts over for ([`GraphPlayer::teleports`](crate::GraphPlayer::teleports)).
    pub teleports: u32,
    /// Poses evaluated that solved IK.
    pub ik: u32,
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

    use sapphire_assets::mesh::SkinBind;
    use sapphire_assets::rig::Constraint;
    use sapphire_math::{Affine3A, Quat};

    use super::*;
    use crate::CharacterPose;
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
    fn reload_compatibility_requires_joint_and_morph_identities() {
        let bytes = Skeleton::new(&arm_joints()).to_cooked().unwrap();
        let skeleton = Skeleton::read(&bytes).unwrap();
        let build = || CharacterPose::new(skeleton, &[], &[], vec![0.0, 0.0]);
        assert!(!build().compatible_with(&build()), "unknown morph layouts cannot prove identity");
        let named = || build().with_morph_names(vec!["smile".into(), "blink".into()]);
        assert!(named().compatible_with(&named()));
        assert!(!named().compatible_with(&build().with_morph_names(vec!["blink".into(), "smile".into()])));
        let mut reordered = Skeleton::new(&arm_joints());
        reordered.names.swap(0, 1);
        let bytes = reordered.to_cooked().unwrap();
        let changed = CharacterPose::new(Skeleton::read(&bytes).unwrap(), &[], &[], vec![0.0, 0.0])
            .with_morph_names(vec!["smile".into(), "blink".into()]);
        assert!(!named().compatible_with(&changed));
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

    #[test]
    fn solved_body_exports_constraints_and_named_active_morphs_together() {
        let bytes = swing().to_cooked().expect("cooked");
        let clip = Clip::read(&bytes).expect("a valid clip");
        let mut pose = arm().with_morph_names(vec!["rest".into(), "swing".into()]);
        pose.sample(clip, 30.0);
        let mut legacy = pose.clone();
        legacy.finish();

        let body = pose.finish_body();
        assert_eq!(body.locals(), legacy.locals());
        assert_eq!(body.model(), legacy.model());
        let (names, weights) = body.morphs();
        assert_eq!(names.unwrap(), ["rest", "swing"]);
        assert_eq!(weights, legacy.weights());
        assert_eq!(weights[0], 0.25);
        assert!((weights[1] - 1.0).abs() < 1e-5);
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

    /// However many characters there are, each is evaluated exactly once, at its own position, and the
    /// jobs stay bounded by the pool rather than the crowd: one for one character.
    #[test]
    fn every_character_of_a_crowd_is_evaluated_once_in_bounded_jobs() {
        let bytes = swing().to_cooked().expect("cooked");
        let clip = Clip::read(&bytes).expect("a valid clip");
        let threads = sapphire_jobs::system().compute_pool().thread_num() + 1;
        for count in [1, 2, 7, 15, 60, 65] {
            let mut crowd: Vec<(f32, u32, CharacterPose)> =
                (0..count).map(|index| (index as f32 * 0.5, 0, arm())).collect();
            let jobs = evaluate_all(&mut crowd, |(position, visits, pose)| {
                *visits += 1;
                pose.sample(clip, *position);
                pose.finish();
            });
            assert!(jobs <= JOBS_PER_THREAD * threads && jobs <= count, "{count}: {jobs} jobs");
            assert_eq!(jobs == 1, count == 1, "{count}: {jobs} jobs");
            for (position, visits, pose) in &crowd {
                let mut alone = arm();
                alone.sample(clip, *position);
                alone.finish();
                assert_eq!(*visits, 1, "{count} at {position}");
                assert_eq!(bytes_of(pose), bytes_of(&alone), "{count} at {position}");
            }
        }
    }

    /// A list given back keeps its allocation, whatever it held, so a frame's list of jobs costs the heap
    /// nothing once it has held a frame's worth.
    #[test]
    fn a_reused_list_keeps_its_allocation() {
        let mut list: Vec<&u32> = Vec::with_capacity(16);
        let values = [1, 2, 3];
        list.extend(&values);
        let place = list.as_ptr().cast::<u8>();
        let again: Vec<&'static u32> = reuse_list(list);
        assert!(again.is_empty());
        assert_eq!(again.capacity(), 16);
        assert_eq!(again.as_ptr().cast::<u8>(), place);
    }
}
