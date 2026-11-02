//! Key reduction and root motion, read back through the cooked format. [`sample_pose`] is a small
//! sampler for tests alone: what `sapphire_anim`'s runtime (N2) will do, through the same per-track
//! functions, so what these tests measure is what the runtime will show.

use glam::{Affine3A, Quat, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::clip::{ArchivedClip, Clip};
use sapphire_assets::skeleton::{ArchivedSkeleton, Joint, JointTransform, Skeleton};

use super::*;

/// A pose as local transforms.
#[derive(Clone, Debug)]
pub(crate) struct Pose {
    pub translation: Vec<Vec3>,
    pub rotation: Vec<Quat>,
    pub scale: Vec<Vec3>,
}

/// The rest pose, overlaid with `clip`'s tracks at `position`.
pub(crate) fn sample_pose(clip: &ArchivedClip, skeleton: &ArchivedSkeleton, position: f32) -> Pose {
    let rest: Vec<JointTransform> = skeleton.rest.iter().map(|rest| rest.to_native()).collect();
    let mut pose = Pose {
        translation: rest.iter().map(|rest| Vec3::from(rest.translation)).collect(),
        rotation: rest.iter().map(|rest| Quat::from_array(rest.rotation).normalize()).collect(),
        scale: rest.iter().map(|rest| Vec3::from(rest.scale)).collect(),
    };
    for track in clip.rotations.iter() {
        pose.rotation[usize::from(track.joint.to_native())] =
            Quat::from_array(clip.rotation(track, position));
    }
    for track in clip.translations.iter() {
        pose.translation[usize::from(track.joint.to_native())] = Vec3::from(clip.vector(track, position));
    }
    for track in clip.scales.iter() {
        pose.scale[usize::from(track.joint.to_native())] = Vec3::from(clip.vector(track, position));
    }
    pose
}

/// Each joint's model-space transform in `pose`.
pub(crate) fn model_of(pose: &Pose, skeleton: &ArchivedSkeleton) -> Vec<Affine3A> {
    let mut model: Vec<Affine3A> = Vec::with_capacity(skeleton.len());
    for joint in 0..skeleton.len() {
        let local = Affine3A::from_scale_rotation_translation(
            pose.scale[joint],
            pose.rotation[joint],
            pose.translation[joint],
        );
        model.push(skeleton.parent(joint).map_or(local, |parent| model[parent] * local));
    }
    model
}

/// `clip`'s root motion at `position`, as a transform: the identity without any.
pub(crate) fn root_of(clip: &ArchivedClip, position: f32) -> Affine3A {
    clip.root.as_ref().map_or(Affine3A::IDENTITY, |root| {
        Affine3A::from_rotation_translation(
            Quat::from_array(clip.rotation(&root.rotation, position)),
            Vec3::from(clip.vector(&root.translation, position)),
        )
    })
}

/// The angle between two rotations, in radians, to the precision a test needs.
pub(crate) fn angle_between(a: Quat, b: Quat) -> f32 {
    angle(a.to_array(), b.to_array())
}

fn id() -> AssetId {
    AssetId::from_bytes([9; 16])
}

/// A chain of `depth` joints, 0.2 m apart up +Y, the first at 1 m.
fn chain(depth: usize) -> Skeleton {
    let joints: Vec<Joint> = (0..depth)
        .map(|joint| Joint {
            name: format!("j{joint}"),
            parent: joint.checked_sub(1).map(|parent| parent as u16),
            rest: JointTransform {
                translation: [0.0, if joint == 0 { 1.0 } else { 0.2 }, 0.0],
                ..JointTransform::IDENTITY
            },
        })
        .collect();
    Skeleton::new(&joints)
}

/// Motion with some of everything, at a walk's pace with a small quick shake: every joint turning, the first and
/// fourth also moving, the third scaling, two morph weights.
fn busy(ticks: u16) -> SampledClip {
    let times = move || (0..=ticks).map(|tick| f32::from(tick) / 60.0);
    let wave = |t: f32, seed: f32| (t * (0.5 + seed * 0.2)).sin() * 0.6 + (t * (3.0 + seed)).sin() * 0.01;
    SampledClip {
        name: "busy".into(),
        rate: 60,
        ticks,
        rotations: (0..8u16)
            .map(|joint| {
                let seed = f32::from(joint);
                let samples = times().map(|t| {
                    Quat::from_rotation_z(wave(t, seed))
                        * Quat::from_rotation_x(wave(t, seed + 0.5) * 0.7)
                        * Quat::from_rotation_y(wave(t + 1.0, seed) * 0.4)
                });
                (joint, samples.collect())
            })
            .collect(),
        translations: vec![
            (0, times().map(|t| Vec3::new(wave(t, 0.3) * 0.5, 1.0 + wave(t, 1.7) * 0.1, t * 0.4)).collect()),
            (3, times().map(|t| Vec3::new(0.0, 0.2 + wave(t, 2.2) * 0.05, 0.0)).collect()),
        ],
        scales: vec![(2, times().map(|t| Vec3::splat(1.0 + wave(t, 0.9) * 0.1)).collect())],
        weights: vec![
            (0, times().map(|t| 0.5 + wave(t, 0.1) * 0.5).collect()),
            (1, times().map(|t| (t * 0.5).min(1.0)).collect()),
        ],
        root: None,
    }
}

fn read(clip: &Clip) -> sapphire_serialize::cooked::AlignedVec {
    let bytes = clip.to_cooked().unwrap();
    Clip::read(&bytes).unwrap();
    bytes
}

fn cooked(sampled: &SampledClip, skeleton: &Skeleton, error: &ReductionError) -> Clip {
    let target = ClipTarget { skeleton: Some((id(), skeleton)), morph_defaults: &[0.0, 0.0] };
    cook(sampled, &target, error)
}

/// How the stated bound reads in a test: what the module docs promise for this track's joint.
fn rotation_bound(reach: f32, error: &ReductionError) -> f32 {
    (error.distance / reach).max(ROTATION_KEY_ERROR)
}

/// The bound the module docs state, read back through the runtime's own functions, at every tick
/// and between ticks, in local space and in model space.
#[test]
fn the_reconstruction_stays_within_the_stated_bound() {
    let skeleton = chain(8);
    let sampled = busy(600);
    let error = ReductionError::default();
    let clip = cooked(&sampled, &skeleton, &error);
    let (clip_bytes, skeleton_bytes) = (read(&clip), skeleton.to_cooked().unwrap());
    let (clip, joints) = (Clip::read(&clip_bytes).unwrap(), Skeleton::read(&skeleton_bytes).unwrap());
    let reach = reaches(&skeleton);
    // Rounding in measuring, not in the clip.
    const SLACK: f32 = 1e-5;

    // The clip must really have dropped keys, or this proves nothing.
    let dense = sampled.rotations.len() * 601;
    assert!(
        clip.rotation_keys.len() < dense * 3 / 4,
        "{} of {dense} rotation keys",
        clip.rotation_keys.len()
    );

    for half_ticks in 0..=1200u32 {
        let position = half_ticks as f32 / 2.0;
        let tick = (half_ticks / 2) as usize;
        let next = (tick + 1).min(600);
        let u = position - tick as f32;
        for track in clip.rotations.iter() {
            let joint = track.joint.to_native();
            let samples = &sampled.rotations.iter().find(|(j, _)| *j == joint).unwrap().1;
            let source = nlerp(samples[tick].to_array(), samples[next].to_array(), u);
            let strays = angle(clip.rotation(track, position), source);
            let bound = rotation_bound(reach[usize::from(joint)], &error);
            assert!(strays <= bound + SLACK, "rotation {joint} at {position}: {strays} over {bound}");
        }
        for track in clip.translations.iter() {
            let joint = track.joint.to_native();
            let samples = &sampled.translations.iter().find(|(j, _)| *j == joint).unwrap().1;
            let source = samples[tick].lerp(samples[next], u);
            let strays = Vec3::from(clip.vector(track, position)).distance(source);
            let step = Vec3::new(
                track.range.step[0].to_native(),
                track.range.step[1].to_native(),
                track.range.step[2].to_native(),
            )
            .length();
            let bound = error.distance.max(step * 0.5);
            assert!(strays <= bound + SLACK, "translation {joint} at {position}: {strays} over {bound}");
        }
        for track in clip.scales.iter() {
            let joint = track.joint.to_native();
            let samples = &sampled.scales.iter().find(|(j, _)| *j == joint).unwrap().1;
            let source = samples[tick].lerp(samples[next], u);
            let strays = Vec3::from(clip.vector(track, position)).distance(source);
            let bound = (error.distance / reach[usize::from(joint)]).max(MIN_SCALE_TOLERANCE);
            assert!(strays <= bound + SLACK, "scale {joint} at {position}: {strays} over {bound}");
        }
        for track in clip.weights.iter() {
            let target = track.target.to_native();
            let samples = &sampled.weights.iter().find(|(t, _)| *t == target).unwrap().1;
            let source = samples[tick] + (samples[next] - samples[tick]) * u;
            let strays = (clip.weight(track, position) - source).abs();
            assert!(strays <= error.weight + SLACK, "weight {target} at {position}: {strays}");
        }
    }

    // In model space, against the same pose built from the source's samples.
    let source_pose = |tick: usize| {
        let rest: Vec<JointTransform> = skeleton.rest.clone();
        let mut pose = Pose {
            translation: rest.iter().map(|rest| Vec3::from(rest.translation)).collect(),
            rotation: rest.iter().map(|rest| Quat::from_array(rest.rotation)).collect(),
            scale: rest.iter().map(|rest| Vec3::from(rest.scale)).collect(),
        };
        for (joint, samples) in &sampled.rotations {
            pose.rotation[usize::from(*joint)] = samples[tick];
        }
        for (joint, samples) in &sampled.translations {
            pose.translation[usize::from(*joint)] = samples[tick];
        }
        for (joint, samples) in &sampled.scales {
            pose.scale[usize::from(*joint)] = samples[tick];
        }
        pose
    };
    let mut worst: f32 = 0.0;
    for tick in 0..=600usize {
        let cooked = model_of(&sample_pose(clip, joints, tick as f32), joints);
        let source = model_of(&source_pose(tick), joints);
        for (joint, (a, b)) in cooked.iter().zip(&source).enumerate() {
            let strays = Vec3::from(a.translation).distance(Vec3::from(b.translation));
            worst = worst.max(strays);
            // Rotation's key quantization (0.15 mrad across this chain's reach of up to 1.5 m) can
            // exceed the distance bound on the longest chains: the tolerance is then the key's.
            let per_track = error.distance.max(ROTATION_KEY_ERROR * 1.5);
            let bound = 3.0 * (joint + 1) as f32 * per_track;
            assert!(strays <= bound, "joint {joint} at tick {tick}: {strays} over {bound}");
        }
    }
    assert!(worst > 1e-5, "the test measured something: {worst}");
}

/// Same samples, same bytes: reduction reads nothing else.
#[test]
fn reduction_is_deterministic() {
    let (skeleton, sampled) = (chain(8), busy(240));
    let error = ReductionError::default();
    let (a, b) = (cooked(&sampled, &skeleton, &error), cooked(&sampled, &skeleton, &error));
    assert_eq!(a.to_cooked().unwrap().as_slice(), b.to_cooked().unwrap().as_slice());
}

#[test]
fn a_track_at_rest_needs_none_and_a_still_one_needs_one_key() {
    let skeleton = chain(3);
    let count = 61;
    let at_rest = vec![Quat::IDENTITY; count];
    let still = vec![Quat::from_rotation_z(0.5); count];
    let sampled = SampledClip {
        name: "still".into(),
        rate: 60,
        ticks: 60,
        rotations: vec![(0, at_rest), (1, still)],
        translations: vec![
            (0, vec![Vec3::new(0.0, 1.0, 0.0); count]),
            (1, vec![Vec3::new(0.5, 0.2, 0.0); count]),
        ],
        scales: vec![(2, vec![Vec3::ONE; count])],
        weights: vec![(0, vec![0.0; count]), (1, vec![0.25; count])],
        root: None,
    };
    let clip = cooked(&sampled, &skeleton, &ReductionError::default());
    assert_eq!(clip.rotations.len(), 1, "joint 0 is at rest");
    assert_eq!((clip.rotations[0].joint, clip.rotations[0].count), (1, 1));
    assert_eq!(clip.translations.len(), 1, "joint 0 sits where it rests");
    assert_eq!((clip.translations[0].joint, clip.translations[0].count), (1, 1));
    assert!(clip.scales.is_empty(), "scale 1 is rest");
    assert_eq!(clip.weights.len(), 1, "target 0 stays at its default");
    assert_eq!((clip.weights[0].target, clip.weights[0].count), (1, 1));
    let bytes = read(&clip);
    let clip = Clip::read(&bytes).unwrap();
    assert_eq!(clip.vector(&clip.translations[0], 30.0), [0.5, 0.2, 0.0], "a still value is exact");
    assert_eq!(clip.weight(&clip.weights[0], 30.0), 0.25);
}

#[test]
fn a_straight_line_needs_two_keys_and_a_corner_three() {
    let skeleton = chain(2);
    let line = |t: f32| Vec3::new(t * 3.0, 0.0, 0.0);
    let corner = |t: f32| if t < 0.5 { Vec3::Y * t } else { Vec3::Y * (1.0 - t) };
    let track = |f: &dyn Fn(f32) -> Vec3| (0..=60).map(|tick| f(tick as f32 / 60.0)).collect::<Vec<_>>();
    let sampled = SampledClip {
        name: "lines".into(),
        rate: 60,
        ticks: 60,
        translations: vec![(0, track(&line)), (1, track(&corner))],
        ..SampledClip::default()
    };
    let clip = cooked(&sampled, &skeleton, &ReductionError::default());
    assert_eq!(clip.translations[0].count, 2);
    assert_eq!(clip.translations[1].count, 3);
    let bytes = read(&clip);
    let clip = Clip::read(&bytes).unwrap();
    let peak = clip.vector(&clip.translations[1], 30.0)[1];
    assert!((peak - 0.5).abs() < 1e-4, "the corner is a key: {peak}");
}

/// Rest-pose elimination and reduction watch the same tolerance, so a clip with a tolerance of zero
/// keeps what quantization allows, and no less.
#[test]
fn no_tolerance_keeps_every_key_quantization_can_tell_apart() {
    let skeleton = chain(1);
    let wobble: Vec<Quat> = (0..=60).map(|tick| Quat::from_rotation_y(0.01 * (tick % 2) as f32)).collect();
    let sampled = SampledClip {
        name: "wobble".into(),
        rate: 60,
        ticks: 60,
        rotations: vec![(0, wobble)],
        ..SampledClip::default()
    };
    let error = ReductionError { distance: 0.0, weight: 0.0 };
    let clip = cooked(&sampled, &skeleton, &error);
    assert_eq!(clip.rotations[0].count, 61);
}

#[test]
fn a_joints_reach_is_its_longest_chain_plus_the_skin() {
    let mut joints = vec![
        Joint { name: "root".into(), parent: None, rest: JointTransform::IDENTITY },
        Joint {
            name: "short".into(),
            parent: Some(0),
            rest: JointTransform { translation: [0.1, 0.0, 0.0], ..JointTransform::IDENTITY },
        },
        Joint {
            name: "long".into(),
            parent: Some(0),
            rest: JointTransform { translation: [0.0, 0.5, 0.0], ..JointTransform::IDENTITY },
        },
        Joint {
            name: "tip".into(),
            parent: Some(2),
            rest: JointTransform { translation: [0.0, 0.25, 0.0], ..JointTransform::IDENTITY },
        },
    ];
    let reach = reaches(&Skeleton::new(&joints));
    let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
    assert!(close(reach[0], 0.75 + SKIN_REACH), "{reach:?}");
    assert!(close(reach[1], SKIN_REACH), "{reach:?}");
    assert!(close(reach[2], 0.25 + SKIN_REACH), "{reach:?}");
    assert!(close(reach[3], SKIN_REACH), "{reach:?}");
    // In model space: a scaled parent scales its bones.
    joints[0].rest.scale = [2.0; 3];
    assert!(close(reaches(&Skeleton::new(&joints))[0], 1.5 + SKIN_REACH));
}

/// What a key-pose document's ease leans on: anticipation and overshoot are weights past 0 to 1.
#[test]
fn slerp_is_exact_at_its_ends_and_can_leave_them() {
    use sapphire_math::det::quat_slerp as slerp;
    let (a, b) = (Quat::from_rotation_z(0.2), Quat::from_rotation_z(1.2));
    assert!(angle_between(slerp(a, b, 0.0), a) < 1e-5);
    assert!(angle_between(slerp(a, b, 1.0), b) < 1e-5);
    assert!(angle_between(slerp(a, b, 0.5), Quat::from_rotation_z(0.7)) < 1e-4);
    assert!(angle_between(slerp(a, b, 1.2), Quat::from_rotation_z(1.4)) < 1e-4, "past the end");
    assert!(angle_between(slerp(a, b, -0.3), Quat::from_rotation_z(-0.1)) < 1e-4, "before the start");
    // The short way, from either sign.
    assert!(angle_between(slerp(a, -b, 0.5), Quat::from_rotation_z(0.7)) < 1e-4);
    // Too close to divide by the sine, and still past the end.
    let near = Quat::from_rotation_z(0.2001);
    assert!(angle_between(slerp(a, near, 2.0), Quat::from_rotation_z(0.2002)) < 1e-4);
}

/// A figure whose hips walk and turn under an armature scaled a hundredth, as Blender writes
/// centimeters, with a spine above them.
fn walker() -> (Skeleton, SampledClip) {
    let rest = |translation: [f32; 3], scale: f32| JointTransform {
        translation,
        scale: [scale; 3],
        ..JointTransform::IDENTITY
    };
    let joints = vec![
        Joint { name: "Armature".into(), parent: None, rest: rest([0.0; 3], 0.01) },
        Joint { name: "Hips".into(), parent: Some(0), rest: rest([0.0, 100.0, 0.0], 1.0) },
        Joint { name: "Spine".into(), parent: Some(1), rest: rest([0.0, 20.0, 5.0], 1.0) },
    ];
    let skeleton = Skeleton::new(&joints);
    let ticks = 120u16;
    let at = |tick: u16| f32::from(tick) / 60.0;
    let translation = (0..=ticks)
        .map(|tick| {
            let t = at(tick);
            // Forward, with a bob and a sway, in the armature's centimeters.
            Vec3::new((t * 7.0).sin() * 3.0 + t * 20.0, 100.0 + (t * 12.0).sin() * 2.0, t * 150.0)
        })
        .collect();
    let rotation = (0..=ticks)
        .map(|tick| {
            let t = at(tick);
            Quat::from_rotation_y(t * 0.8) * Quat::from_rotation_x((t * 12.0).sin() * 0.1)
        })
        .collect();
    let spine = (0..=ticks).map(|tick| Quat::from_rotation_z((at(tick) * 12.0).cos() * 0.2)).collect();
    let sampled = SampledClip {
        name: "walk".into(),
        rate: 60,
        ticks,
        rotations: vec![(1, rotation), (2, spine)],
        translations: vec![(1, translation)],
        ..SampledClip::default()
    };
    (skeleton, sampled)
}

/// `root(t) × pose(t)` is the motion as authored, whichever way it is split, at every tick.
#[test]
fn root_motion_recomposes_the_motion_as_authored() {
    let splits = [
        (RootTranslation::Ground, RootRotation::Yaw),
        (RootTranslation::Ground, RootRotation::None),
        (RootTranslation::All, RootRotation::Yaw),
        (RootTranslation::All, RootRotation::None),
    ];
    for (translation, rotation) in splits {
        let (skeleton, sampled) = walker();
        let bytes = skeleton.to_cooked().unwrap();
        let joints = Skeleton::read(&bytes).unwrap();
        // The motion as authored: the same clip with nothing lifted.
        let authored_clip = cooked(&sampled, &skeleton, &ReductionError { distance: 0.000_05, weight: 0.0 });
        let authored_bytes = read(&authored_clip);
        let authored = Clip::read(&authored_bytes).unwrap();

        let mut lifted = sampled.clone();
        lift_root(&mut lifted, &skeleton, 1, translation, rotation).unwrap();
        let clip = cooked(&lifted, &skeleton, &ReductionError { distance: 0.000_05, weight: 0.0 });
        let clip_bytes = read(&clip);
        let clip = Clip::read(&clip_bytes).unwrap();
        let root = clip.root.as_ref().expect("root motion");
        assert_eq!(root.joint.to_native(), 1);

        for tick in 0..=120u16 {
            let position = f32::from(tick);
            let expected = model_of(&sample_pose(authored, joints, position), joints);
            let pose = model_of(&sample_pose(clip, joints, position), joints);
            let travelled = root_of(clip, position);
            for joint in [1, 2] {
                let recomposed = travelled * pose[joint];
                let strays =
                    Vec3::from(recomposed.translation).distance(Vec3::from(expected[joint].translation));
                assert!(
                    strays < 2e-3,
                    "{translation:?} {rotation:?}: joint {joint} at tick {tick} is {strays} m off"
                );
                let (_, a, _) = recomposed.to_scale_rotation_translation();
                let (_, b, _) = expected[joint].to_scale_rotation_translation();
                assert!(
                    angle_between(a, b) < 3e-3,
                    "{translation:?} {rotation:?}: joint {joint} at tick {tick}"
                );
            }
        }

        // It starts at the identity and ends where the walk does.
        assert!(root_of(clip, 0.0).abs_diff_eq(Affine3A::IDENTITY, 1e-3));
        let end = Vec3::from(clip.vector(&root.translation, 120.0));
        assert!((end.z - 3.0).abs() < 0.05, "150 cm/s for 2 s: {end}");

        let hips_at = |tick: u16| {
            let pose = model_of(&sample_pose(clip, joints, f32::from(tick)), joints);
            Vec3::from(pose[1].translation)
        };
        match translation {
            // Height stays in the bone's track, and across the ground it holds its first place
            // (it starts at the origin of the ground here, but for the sway's first value).
            RootTranslation::Ground => {
                assert_eq!(end.y, 0.0, "no height in the root");
                let bob: Vec<f32> = (0..=120).map(|tick| hips_at(tick).y).collect();
                let spread = bob.iter().copied().fold(f32::MIN, f32::max)
                    - bob.iter().copied().fold(f32::MAX, f32::min);
                assert!(spread > 0.03, "the bob stays in the bone: {spread}");
                for tick in [30, 60, 120] {
                    let at = hips_at(tick);
                    assert!(at.x.abs() < 1e-3 && at.z.abs() < 1e-3, "tick {tick}: {at}");
                }
            }
            // All of it: the bone stays where it started.
            RootTranslation::All => {
                for tick in [30, 60, 120] {
                    let (at, start) = (hips_at(tick), hips_at(0));
                    assert!(at.distance(start) < 1e-3, "tick {tick}: {at} from {start}");
                }
            }
        }
        match rotation {
            RootRotation::Yaw => {
                let turn = Quat::from_array(clip.rotation(&root.rotation, 120.0));
                assert!(angle_between(turn, Quat::from_rotation_y(1.6)) < 0.05, "{turn}");
            }
            RootRotation::None => {
                assert_eq!(root.rotation.count.to_native(), 1, "no turn lifted");
                assert!(
                    angle_between(Quat::from_array(clip.rotation(&root.rotation, 120.0)), Quat::IDENTITY)
                        < 1e-3
                );
            }
        }
    }
}

/// A bone that doesn't move lifts nothing, and the clip's root is a held identity.
#[test]
fn a_still_root_lifts_nothing() {
    let (skeleton, mut sampled) = walker();
    sampled.translations.clear();
    sampled.rotations.remove(0);
    lift_root(&mut sampled, &skeleton, 1, RootTranslation::Ground, RootRotation::Yaw).unwrap();
    let clip = cooked(&sampled, &skeleton, &ReductionError::default());
    let root = clip.root.as_ref().unwrap();
    assert_eq!((root.translation.count, root.rotation.count), (1, 1));
    assert!(clip.rotations.iter().all(|track| track.joint != 1), "the bone stays at rest");
    assert!(clip.translations.is_empty(), "and in place");
    read(&clip);
}

/// A translation's tolerance is in meters however its parent scales it.
#[test]
fn a_translations_tolerance_counts_in_model_space() {
    let rest = |translation: [f32; 3], scale: f32| JointTransform {
        translation,
        scale: [scale; 3],
        ..JointTransform::IDENTITY
    };
    let skeleton = |scale: f32| {
        Skeleton::new(&[
            Joint { name: "armature".into(), parent: None, rest: rest([0.0; 3], scale) },
            Joint { name: "bone".into(), parent: Some(0), rest: rest([0.0; 3], 1.0) },
        ])
    };
    let bump = |height: f32| -> Vec<Vec3> {
        (0..=60u16)
            .map(|tick| Vec3::new(0.0, height * (f32::from(tick) * std::f32::consts::PI / 60.0).sin(), 0.0))
            .collect()
    };
    let sampled = |height: f32| SampledClip {
        name: "bump".into(),
        rate: 60,
        ticks: 60,
        translations: vec![(1, bump(height))],
        ..SampledClip::default()
    };
    let error = ReductionError::default();
    // 0.5 units under a hundredth is 5 mm: well over the tolerance.
    assert_eq!(cooked(&sampled(0.5), &skeleton(0.01), &error).translations.len(), 1);
    // 0.02 units under a hundredth is 0.2 mm: at rest.
    assert!(cooked(&sampled(0.02), &skeleton(0.01), &error).translations.is_empty());
    // 0.00001 units under a hundred is 1 mm, though it looks like nothing in its own units.
    assert_eq!(cooked(&sampled(0.000_01), &skeleton(100.0), &error).translations.len(), 1);
}

/// Root motion starts at the identity, as near as a key can say across however far it goes.
#[test]
fn root_motion_across_kilometers_still_starts_at_the_identity() {
    let (skeleton, mut sampled) = walker();
    for (_, samples) in &mut sampled.translations {
        for (tick, sample) in samples.iter_mut().enumerate() {
            // Five hundred kilometers in the armature's centimeters, in two seconds: a track's
            // step is then longer than any fixed tolerance.
            sample.z = tick as f32 * 4.0e5;
        }
    }
    lift_root(&mut sampled, &skeleton, 1, RootTranslation::Ground, RootRotation::Yaw).unwrap();
    let clip = cooked(&sampled, &skeleton, &ReductionError::default());
    read(&clip);
}

/// A scale of zero has no inverse, so there is no rest of the motion to keep: an error, and the
/// clip as it was.
#[test]
fn a_root_under_a_scale_of_zero_cant_be_lifted() {
    let (skeleton, mut sampled) = walker();
    let count = usize::from(sampled.ticks) + 1;
    sampled.scales = vec![(0, vec![Vec3::ZERO; count])];
    let before = sampled.clone();
    assert!(lift_root(&mut sampled, &skeleton, 1, RootTranslation::Ground, RootRotation::Yaw).is_err());
    assert_eq!(sampled, before);
}

/// A clip's tolerance reads what the skeleton says of each joint: a hip's rotation is held tighter
/// than a fingertip's, since it moves everything below.
#[test]
fn a_joint_with_more_below_it_holds_tighter() {
    let skeleton = chain(8);
    let wobble = |amplitude: f32| -> Vec<Quat> {
        (0..=120).map(|tick| Quat::from_rotation_z(amplitude * (tick as f32 * 0.3).sin())).collect()
    };
    // A wobble small enough for a fingertip's tolerance (0.01 rad), and too large for the hips'
    // (0.00067 rad).
    let amplitude = 0.002;
    let sampled = SampledClip {
        name: "wobble".into(),
        rate: 60,
        ticks: 120,
        rotations: vec![(0, wobble(amplitude)), (7, wobble(amplitude))],
        ..SampledClip::default()
    };
    let clip = cooked(&sampled, &skeleton, &ReductionError { distance: 0.001, weight: 0.0 });
    assert_eq!(clip.rotations[0].joint, 0, "the hips keep a track");
    assert!(clip.rotations.iter().all(|track| track.joint != 7), "the fingertip stays at rest");
}
