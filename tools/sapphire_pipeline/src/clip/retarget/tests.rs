//! Small rigs for what each rule says, then KayKit's real clips on the character for whether their
//! motion survives. A real clip's bound is its measured value and a little: the comment on each says
//! what it means and what it leaves out.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use glam::{Affine3A, Quat, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::rig::HumanBone::{self, *};
use sapphire_assets::skeleton::{Joint, JointTransform, Skeleton};

use super::*;
use crate::clip::source::ClipSource;
use crate::clip::tests::{angle_between, root_of};
use crate::clip::{self, ClipCookError, ClipSettings, ClipTarget, ReductionError};
use crate::import::gltf::clips;
use crate::import::gltf::clips::ClipImportError;
use crate::import::vrm;
use crate::keypose::Turn;
use crate::meta::{self, sidecar_path};

const QUARTER: f32 = std::f32::consts::FRAC_PI_2;

/// One joint of a small rig, in model space: its name, parent, position, rotation and bone.
type Entry = (&'static str, Option<u16>, [f32; 3], Quat, Option<HumanBone>);

/// The rig `entries` describe, with their model-space rest as local rests.
fn rig(entries: &[Entry]) -> (Skeleton, HumanMap) {
    let joints: Vec<Joint> = entries
        .iter()
        .map(|(name, parent, position, rotation, _)| {
            let (above, turned) = parent.map_or((Vec3::ZERO, Quat::IDENTITY), |parent| {
                let parent = &entries[usize::from(parent)];
                (Vec3::from(parent.2), parent.3)
            });
            let inverse = turned.inverse();
            Joint {
                name: (*name).into(),
                parent: *parent,
                rest: JointTransform {
                    translation: (inverse * (Vec3::from(*position) - above)).to_array(),
                    rotation: (inverse * *rotation).to_array(),
                    scale: [1.0; 3],
                },
            }
        })
        .collect();
    let map = entries
        .iter()
        .enumerate()
        .filter_map(|(joint, entry)| entry.4.map(|bone| (bone, joint as u16)))
        .collect();
    (Skeleton::new(&joints), HumanMap::from_joints(map))
}

const SOURCE_HIPS: u16 = 0;
const SOURCE_CHEST: u16 = 2;
const SOURCE_UPPER_LEFT_ARM: u16 = 4;
const SOURCE_UPPER_LEFT_LEG: u16 = 10;
const SOURCE_UPPER_RIGHT_LEG: u16 = 13;

/// A T-posed figure whose joints all have the model's axes, with a chest and legs 0.9 long.
fn source_rig() -> (Skeleton, HumanMap) {
    let plain = Quat::IDENTITY;
    rig(&[
        ("Hips", None, [0.0, 1.0, 0.0], plain, Some(Hips)),
        ("Spine", Some(0), [0.0, 1.1, 0.0], plain, Some(Spine)),
        ("Chest", Some(1), [0.0, 1.3, 0.0], plain, Some(Chest)),
        ("Head", Some(2), [0.0, 1.6, 0.0], plain, Some(Head)),
        ("UpperArmL", Some(2), [0.2, 1.5, 0.0], plain, Some(LeftUpperArm)),
        ("LowerArmL", Some(4), [0.45, 1.5, 0.0], plain, Some(LeftLowerArm)),
        ("HandL", Some(5), [0.7, 1.5, 0.0], plain, Some(LeftHand)),
        ("UpperArmR", Some(2), [-0.2, 1.5, 0.0], plain, Some(RightUpperArm)),
        ("LowerArmR", Some(7), [-0.45, 1.5, 0.0], plain, Some(RightLowerArm)),
        ("HandR", Some(8), [-0.7, 1.5, 0.0], plain, Some(RightHand)),
        ("UpperLegL", Some(0), [0.1, 0.95, 0.0], plain, Some(LeftUpperLeg)),
        ("LowerLegL", Some(10), [0.1, 0.5, 0.0], plain, Some(LeftLowerLeg)),
        ("FootL", Some(11), [0.1, 0.05, 0.0], plain, Some(LeftFoot)),
        ("UpperLegR", Some(0), [-0.1, 0.95, 0.0], plain, Some(RightUpperLeg)),
        ("LowerLegR", Some(13), [-0.1, 0.5, 0.0], plain, Some(RightLowerLeg)),
        ("FootR", Some(14), [-0.1, 0.05, 0.0], plain, Some(RightFoot)),
    ])
}

const TARGET_ROOT: u16 = 0;
const TARGET_HIPS: u16 = 1;
const TARGET_NECK: u16 = 3;
const TARGET_HEAD: u16 = 4;
const TARGET_HAIR: u16 = 5;
const TARGET_UPPER_ARM: u16 = 6;

/// A T-posed figure twice the source's size under a turned root, without a chest, with a neck, a
/// joint no bone names and a finger, and every joint with axes of its own: arms along their +Y,
/// rolled, and legs along their -Y. Upper legs are 0.6 and lower legs 1.2, so legs are 1.8 long
/// but bend elsewhere.
fn target_rig() -> (Skeleton, HumanMap) {
    let turned = Quat::from_rotation_y(QUARTER);
    let along_x = Quat::from_rotation_z(-QUARTER) * Quat::from_rotation_y(2.0);
    let along_minus_x = Quat::from_rotation_z(QUARTER) * Quat::from_rotation_y(-1.0);
    let down = Quat::from_rotation_x(3.0);
    rig(&[
        ("Root", None, [0.0, 0.0, 0.0], turned, None),
        ("Hips", Some(0), [0.0, 2.0, 0.0], Quat::from_rotation_x(QUARTER), Some(Hips)),
        ("Spine", Some(1), [0.0, 2.2, 0.0], Quat::from_rotation_y(1.0), Some(Spine)),
        ("Neck", Some(2), [0.0, 3.0, 0.0], Quat::from_rotation_x(-0.5), Some(Neck)),
        ("Head", Some(3), [0.0, 3.2, 0.0], Quat::from_rotation_z(0.5), Some(Head)),
        ("Hair", Some(4), [0.0, 3.6, 0.0], Quat::IDENTITY, None),
        ("UpperArmL", Some(2), [0.4, 3.0, 0.0], along_x, Some(LeftUpperArm)),
        ("LowerArmL", Some(6), [0.9, 3.0, 0.0], along_x, Some(LeftLowerArm)),
        ("HandL", Some(7), [1.4, 3.0, 0.0], along_x, Some(LeftHand)),
        ("IndexL", Some(8), [1.5, 3.0, 0.0], along_x, Some(LeftIndexProximal)),
        ("UpperArmR", Some(2), [-0.4, 3.0, 0.0], along_minus_x, Some(RightUpperArm)),
        ("LowerArmR", Some(10), [-0.9, 3.0, 0.0], along_minus_x, Some(RightLowerArm)),
        ("HandR", Some(11), [-1.4, 3.0, 0.0], along_minus_x, Some(RightHand)),
        ("UpperLegL", Some(1), [0.2, 1.9, 0.0], down, Some(LeftUpperLeg)),
        ("LowerLegL", Some(13), [0.2, 1.3, 0.0], down, Some(LeftLowerLeg)),
        ("FootL", Some(14), [0.2, 0.1, 0.0], down, Some(LeftFoot)),
        ("UpperLegR", Some(1), [-0.2, 1.9, 0.0], down, Some(RightUpperLeg)),
        ("LowerLegR", Some(16), [-0.2, 1.3, 0.0], down, Some(RightLowerLeg)),
        ("FootR", Some(17), [-0.2, 0.1, 0.0], down, Some(RightFoot)),
    ])
}

/// A clip of `ticks` that holds the rest pose, on no tracks.
fn still(ticks: u16) -> SampledClip {
    SampledClip { name: "Still".into(), rate: 30, ticks, ..SampledClip::default() }
}

/// `clip`, with local rotation `turn` held on each of `joints`.
fn turned(mut clip: SampledClip, joints: &[(u16, Quat)]) -> SampledClip {
    let count = usize::from(clip.ticks) + 1;
    clip.rotations.extend(joints.iter().map(|&(joint, turn)| (joint, vec![turn; count])));
    clip.rotations.sort_by_key(|(joint, _)| *joint);
    clip
}

/// Model transforms of every joint at `tick`.
fn pose(clip: &SampledClip, skeleton: &Skeleton, tick: usize) -> Vec<Affine3A> {
    let mut model: Vec<Affine3A> = Vec::with_capacity(skeleton.parents.len());
    for joint in 0..skeleton.parents.len() {
        let rest = &skeleton.rest[joint];
        let id = joint as u16;
        let rotation = clip
            .rotations
            .iter()
            .find(|(track, _)| *track == id)
            .map_or(Quat::from_array(rest.rotation).normalize(), |(_, samples)| samples[tick].normalize());
        let vector = |tracks: &[(u16, Vec<Vec3>)], or: [f32; 3]| {
            tracks.iter().find(|(track, _)| *track == id).map_or(Vec3::from(or), |(_, samples)| samples[tick])
        };
        let local = Affine3A::from_scale_rotation_translation(
            vector(&clip.scales, rest.scale),
            rotation,
            vector(&clip.translations, rest.translation),
        );
        model.push(match skeleton.parents[joint] {
            NO_PARENT => local,
            parent => model[usize::from(parent)] * local,
        });
    }
    model
}

/// Model rotations of every joint at `tick`, built from normalized local rotations.
fn rotations(clip: &SampledClip, skeleton: &Skeleton, tick: usize) -> Vec<Quat> {
    let mut model: Vec<Quat> = Vec::with_capacity(skeleton.parents.len());
    for joint in 0..skeleton.parents.len() {
        let id = joint as u16;
        let local = clip
            .rotations
            .iter()
            .find(|(track, _)| *track == id)
            .map_or(Quat::from_array(skeleton.rest[joint].rotation).normalize(), |(_, samples)| {
                samples[tick].normalize()
            });
        model.push(match skeleton.parents[joint] {
            NO_PARENT => local,
            parent => model[usize::from(parent)] * local,
        });
    }
    model
}

fn at(model: &[Affine3A], joint: u16) -> Vec3 {
    Vec3::from(model[usize::from(joint)].translation)
}

fn position(model: &[Affine3A], map: &HumanMap, bone: HumanBone) -> Vec3 {
    at(model, map.joint(bone).unwrap())
}

fn along(model: &[Affine3A], map: &HumanMap, from: HumanBone, to: HumanBone) -> Vec3 {
    (position(model, map, to) - position(model, map, from)).normalize()
}

fn track<T>(tracks: &[(u16, Vec<T>)], joint: u16) -> Option<&Vec<T>> {
    tracks.iter().find(|(track, _)| *track == joint).map(|(_, samples)| samples)
}

fn retargeted(clip: &SampledClip, options: &RetargetOptions) -> (SampledClip, Skeleton, HumanMap) {
    let (source_skeleton, source_map) = source_rig();
    let (target_skeleton, target_map) = target_rig();
    let out = retarget(
        clip,
        Rig { skeleton: &source_skeleton, map: &source_map },
        Rig { skeleton: &target_skeleton, map: &target_map },
        options,
    )
    .unwrap();
    (out, target_skeleton, target_map)
}

const UNGROUNDED: RetargetOptions = RetargetOptions {
    scale: None,
    ground: false,
    adjust: BTreeMap::new(),
    gain: BTreeMap::new(),
    follow: Vec::new(),
};

#[test]
fn the_scale_is_the_ratio_of_the_rigs_legs() {
    let (source_skeleton, source_map) = source_rig();
    let (target_skeleton, target_map) = target_rig();
    let source = Rig { skeleton: &source_skeleton, map: &source_map };
    let target = Rig { skeleton: &target_skeleton, map: &target_map };
    assert!((leg_length(source) - 0.9).abs() < 1e-5, "{}", leg_length(source));
    assert!((leg_length(target) - 1.8).abs() < 1e-5, "{}", leg_length(target));
}

/// Every delta is the identity, so nothing in the target moves, whatever its axes.
#[test]
fn a_rest_pose_is_the_targets_rest_and_cooks_to_no_tracks() {
    let (out, skeleton, _) = retargeted(&still(10), &RetargetOptions::default());
    assert_eq!(out.ticks, 10);
    for (joint, samples) in &out.rotations {
        let rest = Quat::from_array(skeleton.rest[usize::from(*joint)].rotation);
        for sample in samples {
            assert!(angle_between(*sample, rest) < 1e-5, "joint {joint}: {sample} against {rest}");
        }
    }
    for sample in track(&out.translations, TARGET_HIPS).unwrap() {
        let rest = Vec3::from(skeleton.rest[usize::from(TARGET_HIPS)].translation);
        assert!(sample.abs_diff_eq(rest, 1e-5), "{sample} against {rest}");
    }
    let target =
        ClipTarget { skeleton: Some((AssetId::from_bytes([1; 16]), &skeleton)), morph_defaults: &[] };
    let cooked = clip::cook(&out, &target, &ReductionError::default());
    assert!(cooked.rotations.is_empty(), "{:?}", cooked.rotations);
    assert!(cooked.translations.is_empty(), "{:?}", cooked.translations);
}

/// The source's joints have the model's axes and the target's have none of them, so its local
/// rotations are nothing alike. Directions in model space are what's carried.
#[test]
fn a_turn_is_the_same_turn_in_model_space_whatever_a_bones_axes() {
    let down = Quat::from_rotation_z(-QUARTER);
    let forward = Quat::from_rotation_y(-QUARTER);
    let clip = turned(still(2), &[(SOURCE_UPPER_LEFT_ARM, down), (SOURCE_UPPER_LEFT_ARM + 1, forward)]);
    let (out, skeleton, map) = retargeted(&clip, &UNGROUNDED);
    let (source_skeleton, source_map) = source_rig();
    let (model, source_model) = (pose(&out, &skeleton, 2), pose(&clip, &source_skeleton, 2));

    let upper = along(&model, &map, LeftUpperArm, LeftLowerArm);
    assert!(upper.abs_diff_eq(Vec3::NEG_Y, 1e-5), "the upper arm hangs: {upper}");
    let lower = along(&model, &map, LeftLowerArm, LeftHand);
    let expected = along(&source_model, &source_map, LeftLowerArm, LeftHand);
    assert!(lower.abs_diff_eq(expected, 1e-5), "the lower arm: {lower} against {expected}");

    // Not a copy of the local rotation.
    let local = track(&out.rotations, TARGET_UPPER_ARM).unwrap()[2];
    assert!(angle_between(local, down) > 0.5, "{local}");
    // The other arm stays where the T-pose has it, as a track at rest.
    let right = along(&model, &map, RightUpperArm, RightLowerArm);
    assert!(right.abs_diff_eq(Vec3::NEG_X, 1e-5), "{right}");
}

/// The target's neck isn't in the source and the source's chest isn't in the target, and the head
/// still turns as the source's does.
#[test]
fn a_bone_one_rig_lacks_rests_under_its_parent_and_what_it_did_reaches_the_bones_below() {
    let turn = Quat::from_rotation_y(0.5);
    let clip = turned(still(1), &[(SOURCE_CHEST, turn)]);
    let (out, skeleton, map) = retargeted(&clip, &UNGROUNDED);

    assert!(track(&out.rotations, TARGET_ROOT).is_none(), "above the hips nothing moves");
    assert!(track(&out.rotations, TARGET_HAIR).is_none(), "a joint no bone names isn't written");
    // No track for the neck's own bone, since the source doesn't have it: the source's map decides.
    assert!(track(&out.rotations, TARGET_NECK).is_none(), "a bone the source lacks isn't written");

    let model = rotations(&out, &skeleton, 1);
    let rest = rotations(&still(0), &skeleton, 0);
    let head = model[usize::from(TARGET_HEAD)] * rest[usize::from(TARGET_HEAD)].inverse();
    assert!(angle_between(head, turn) < 1e-5, "{head} against {turn}");
    // The spine is the source's spine, which didn't turn.
    let spine = map.joint(Spine).unwrap();
    assert!(angle_between(model[usize::from(spine)], rest[usize::from(spine)]) < 1e-5);
}

#[test]
fn the_hips_and_only_the_hips_travel_by_the_scale() {
    let mut clip = still(1);
    clip.translations.push((SOURCE_HIPS, vec![Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.25, 1.1, -0.5)]));
    // A bone's own translation stays behind, as a length of the source's.
    clip.translations.push((SOURCE_UPPER_LEFT_ARM, vec![Vec3::ZERO, Vec3::ONE]));
    clip.scales.push((SOURCE_UPPER_LEFT_ARM, vec![Vec3::ONE, Vec3::splat(3.0)]));
    let (out, skeleton, map) = retargeted(&clip, &UNGROUNDED);
    assert_eq!(out.translations.len(), 1);
    assert!(out.scales.is_empty());
    let hips = position(&pose(&out, &skeleton, 1), &map, Hips);
    // In model space, through the turned root above the hips.
    assert!(hips.abs_diff_eq(Vec3::new(0.5, 2.2, -1.0), 1e-5), "{hips}");

    let (out, skeleton, map) = retargeted(&clip, &RetargetOptions { scale: Some(4.0), ..UNGROUNDED });
    let hips = position(&pose(&out, &skeleton, 1), &map, Hips);
    assert!(hips.abs_diff_eq(Vec3::new(1.0, 2.4, -2.0), 1e-5), "{hips}");
}

/// A lowered thigh on a rig whose thighs are a third of its legs, where the source's are half, with
/// the source's hips sunk just enough to keep its feet on the floor. The hips alone would sink the
/// target's feet 15 cm into it.
#[test]
fn grounding_puts_the_lowest_foot_where_the_sources_is() {
    let thigh = Quat::from_rotation_x(std::f32::consts::FRAC_PI_3);
    let unbend = thigh.inverse();
    let mut clip = turned(
        still(1),
        &[
            (SOURCE_UPPER_LEFT_LEG, thigh),
            (SOURCE_UPPER_LEFT_LEG + 1, unbend),
            (SOURCE_UPPER_RIGHT_LEG, thigh),
            (SOURCE_UPPER_RIGHT_LEG + 1, unbend),
        ],
    );
    clip.translations.push((SOURCE_HIPS, vec![Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.775, 0.0)]));
    let lowest = |out: &SampledClip, skeleton: &Skeleton, map: &HumanMap| {
        let model = pose(out, skeleton, 1);
        let rest = pose(&still(0), skeleton, 0);
        [LeftFoot, RightFoot]
            .map(|foot| position(&model, map, foot).y - position(&rest, map, foot).y)
            .into_iter()
            .fold(f32::MAX, f32::min)
    };

    let (out, skeleton, map) = retargeted(&clip, &UNGROUNDED);
    let sunk = lowest(&out, &skeleton, &map);
    assert!((sunk + 0.15).abs() < 1e-4, "without grounding: {sunk}");
    let (out, skeleton, map) = retargeted(&clip, &RetargetOptions::default());
    let grounded = lowest(&out, &skeleton, &map);
    assert!(grounded.abs() < 1e-4, "with it: {grounded}");

    // A foot off the floor stays as far off it as the source's, scaled. Nothing raises it to rest on
    // the floor for it: the source's flight is flight.
    let mut jump = still(1);
    jump.translations.push((SOURCE_HIPS, vec![Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 1.5, 0.0)]));
    let (out, skeleton, map) = retargeted(&jump, &RetargetOptions::default());
    let height = lowest(&out, &skeleton, &map);
    assert!((height - 1.0).abs() < 1e-4, "{height}");
}

#[test]
fn root_motion_carries_over_on_the_targets_hips_and_scales() {
    let (source, turn) = (Vec3::new(0.5, 0.0, 1.0), Quat::from_rotation_y(1.25));
    let mut clip = still(1);
    clip.root = Some(SampledRoot {
        joint: SOURCE_HIPS,
        translation: vec![Vec3::ZERO, source],
        rotation: vec![Quat::IDENTITY, turn],
    });
    let (out, _, _) = retargeted(&clip, &RetargetOptions::default());
    let root = out.root.expect("carried");
    assert_eq!(root.joint, TARGET_HIPS);
    assert_eq!(root.translation[0], Vec3::ZERO);
    assert!(root.translation[1].abs_diff_eq(source * 2.0, 1e-5), "{:?}", root.translation);
    assert_eq!(root.rotation, [Quat::IDENTITY, turn], "a turn doesn't scale");
}

/// A hand block as a source's map may have it, for a target with fingers: the block's turn reaches
/// both. The other way round a joint would have two rotations, so a target's can't.
#[test]
fn one_source_joint_may_stand_for_several_bones_and_one_target_joint_may_not() {
    let (source_skeleton, source_map) = source_rig();
    let (target_skeleton, target_map) = target_rig();
    let hand = source_map.joint(LeftHand).unwrap();
    let mut joints: BTreeMap<HumanBone, u16> = source_map.iter().collect();
    joints.insert(LeftIndexProximal, hand);
    let shared = HumanMap::from_joints(joints);
    let turn = Quat::from_rotation_x(0.75);
    let clip = turned(still(1), &[(hand, turn)]);
    let out = retarget(
        &clip,
        Rig { skeleton: &source_skeleton, map: &shared },
        Rig { skeleton: &target_skeleton, map: &target_map },
        &UNGROUNDED,
    )
    .unwrap();
    let model = rotations(&out, &target_skeleton, 1);
    let rest = rotations(&still(0), &target_skeleton, 0);
    for bone in [LeftHand, LeftIndexProximal] {
        let joint = usize::from(target_map.joint(bone).unwrap());
        let delta = model[joint] * rest[joint].inverse();
        assert!(angle_between(delta, turn) < 1e-5, "{}: {delta}", bone.name());
    }

    let mut joints: BTreeMap<HumanBone, u16> = target_map.iter().collect();
    joints.insert(RightIndexProximal, target_map.joint(LeftHand).unwrap());
    let error = retarget(
        &clip,
        Rig { skeleton: &source_skeleton, map: &source_map },
        Rig { skeleton: &target_skeleton, map: &HumanMap::from_joints(joints) },
        &UNGROUNDED,
    )
    .unwrap_err();
    assert!(matches!(error, RetargetError::SharedTargetJoint { .. }), "{error:?}");
    assert!(error.to_string().contains("leftHand"), "{error}");
}

#[test]
fn retargets_that_cant_be_are_errors_that_say_why() {
    let (source_skeleton, source_map) = source_rig();
    let (target_skeleton, target_map) = target_rig();
    let source = Rig { skeleton: &source_skeleton, map: &source_map };
    let target = Rig { skeleton: &target_skeleton, map: &target_map };
    let legless = HumanMap::from_joints([(Hips, 0)].into_iter().collect());
    let mut bad_hips = still(1);
    bad_hips.translations.push((SOURCE_HIPS, vec![Vec3::Y, Vec3::NAN]));

    let scaled = |scale| RetargetOptions { scale: Some(scale), ..RetargetOptions::default() };
    let cases: Vec<(&str, RetargetError)> = vec![
        ("a scale of 0", retarget(&still(1), source, target, &scaled(0.0)).unwrap_err()),
        ("a scale below 0", retarget(&still(1), source, target, &scaled(-1.0)).unwrap_err()),
        ("a scale that isn't a number", retarget(&still(1), source, target, &scaled(f32::NAN)).unwrap_err()),
        (
            "a source with no legs",
            retarget(
                &still(1),
                Rig { skeleton: &source_skeleton, map: &legless },
                target,
                &RetargetOptions::default(),
            )
            .unwrap_err(),
        ),
        (
            "a target with no legs",
            retarget(
                &still(1),
                source,
                Rig { skeleton: &target_skeleton, map: &legless },
                &RetargetOptions::default(),
            )
            .unwrap_err(),
        ),
        ("motion that isn't finite", retarget(&bad_hips, source, target, &UNGROUNDED).unwrap_err()),
    ];
    let says = |error: &RetargetError| match error {
        RetargetError::Scale(_) => "scale",
        RetargetError::NoLegLength { which: "the source" } => "source",
        RetargetError::NoLegLength { .. } => "target",
        RetargetError::NonFinite { .. } => "finite",
        RetargetError::SharedTargetJoint { .. } => "shared",
        RetargetError::UnknownJoint { .. } | RetargetError::FollowedBone { .. } => "follow",
    };
    let expected = ["scale", "scale", "scale", "source", "target", "finite"];
    for ((name, error), expected) in cases.iter().zip(expected) {
        assert_eq!(says(error), expected, "{name}: {error}");
        assert!(error.to_string().contains(expected), "{name}: {error}");
    }
}

#[test]
fn a_sidecars_map_names_its_bad_entry() {
    let (skeleton, _) = source_rig();
    let good: BTreeMap<HumanBone, String> = [
        (Hips, "Hips"),
        (Spine, "Spine"),
        (Head, "Head"),
        (LeftUpperLeg, "UpperLegL"),
        (LeftLowerLeg, "LowerLegL"),
        (LeftFoot, "FootL"),
        (RightUpperLeg, "UpperLegR"),
        (RightLowerLeg, "LowerLegR"),
        (RightFoot, "FootR"),
        (LeftUpperArm, "UpperArmL"),
        (LeftLowerArm, "LowerArmL"),
        (LeftHand, "HandL"),
        (RightUpperArm, "UpperArmR"),
        (RightLowerArm, "LowerArmR"),
        (RightHand, "HandR"),
    ]
    .into_iter()
    .map(|(bone, name)| (bone, name.to_owned()))
    .collect();
    let map = HumanMap::from_names(&good, &skeleton).unwrap();
    assert_eq!(map.joint(LeftHand), Some(6));
    assert_eq!(map.joint(Chest), None, "an optional bone may be left out");

    let mut wrong = good.clone();
    wrong.insert(Chest, "Ribcage".to_owned());
    let error = HumanMap::from_names(&wrong, &skeleton).unwrap_err();
    assert!(
        matches!(&error, HumanMapError::UnknownJoint { bone: "chest", joint } if joint == "Ribcage"),
        "{error}"
    );
    assert!(error.to_string().contains("\"Ribcage\""), "{error}");

    let mut short = good.clone();
    short.remove(&LeftHand);
    let error = HumanMap::from_names(&short, &skeleton).unwrap_err();
    assert_eq!(error, HumanMapError::MissingBone { bone: "leftHand" });
}

// Adjustments and gains, on the small rigs.

const TARGET_LEFT_LOWER_ARM: u16 = TARGET_UPPER_ARM + 1;
const TARGET_LEFT_INDEX: u16 = TARGET_UPPER_ARM + 3;

fn adjusted(bone: HumanBone, turn: Turn) -> RetargetOptions {
    RetargetOptions { adjust: BTreeMap::from([(bone, turn)]), ..UNGROUNDED }
}

/// Turned about +Z by `degrees`, as a document says it.
fn about_z(degrees: f32) -> Turn {
    Turn { z: degrees, ..Turn::default() }
}

#[test]
fn an_adjustment_turns_a_bone_and_what_hangs_below_it() {
    let (out, skeleton, map) = retargeted(&still(2), &adjusted(LeftUpperArm, about_z(-90.0)));
    let model = pose(&out, &skeleton, 2);
    let upper = along(&model, &map, LeftUpperArm, LeftLowerArm);
    assert!(upper.abs_diff_eq(Vec3::NEG_Y, 1e-5), "the T-pose's arm hangs: {upper}");
    let lower = along(&model, &map, LeftLowerArm, LeftHand);
    assert!(lower.abs_diff_eq(Vec3::NEG_Y, 1e-5), "and the forearm with it: {lower}");
    let right = along(&model, &map, RightUpperArm, RightLowerArm);
    assert!(right.abs_diff_eq(Vec3::NEG_X, 1e-5), "the other arm stays: {right}");
}

#[test]
fn an_adjustment_moves_with_the_body_above_it() {
    // The spine, which both rigs map, bows 90° forward, and the source's arm follows it.
    let bow = Quat::from_rotation_x(QUARTER);
    let clip = turned(still(2), &[(SOURCE_HIPS + 1, bow)]);
    let (out, skeleton, map) = retargeted(&clip, &adjusted(LeftUpperArm, about_z(-90.0)));
    let model = pose(&out, &skeleton, 2);
    // Rest's arm hangs from a chest at rest, so from one bowed forward it reaches along +Z.
    let upper = along(&model, &map, LeftUpperArm, LeftLowerArm);
    assert!(upper.abs_diff_eq(bow * Vec3::NEG_Y, 1e-5), "{upper}");
}

#[test]
fn an_adjustment_on_a_bone_only_the_target_has_is_a_fixed_pose() {
    let swing = Quat::from_rotation_z(-QUARTER);
    let clip = turned(still(2), &[(SOURCE_UPPER_LEFT_ARM, swing)]);
    let curl = adjusted(LeftIndexProximal, about_z(-30.0));
    let (out, skeleton, map) = retargeted(&clip, &curl);
    let keys = track(&out.rotations, TARGET_LEFT_INDEX).expect("a finger the source lacks has a track");
    assert!(keys.iter().all(|key| angle_between(*key, keys[0]) < 1e-3), "a fixed pose under its parent");
    // At the hand: the finger's own turn, on an arm hanging by the source's.
    let model = pose(&out, &skeleton, 2);
    let hand = rotations(&out, &skeleton, 2)[usize::from(map.joint(LeftHand).unwrap())];
    let finger = rotations(&out, &skeleton, 2)[usize::from(TARGET_LEFT_INDEX)];
    let rest = rest_rotations(&skeleton);
    let turn = finger
        * rest[usize::from(TARGET_LEFT_INDEX)].inverse()
        * (hand * rest[usize::from(map.joint(LeftHand).unwrap())].inverse()).inverse();
    let expected = about_z(-30.0).quat();
    assert!(angle_between(turn, swing * expected * swing.inverse()) < 1e-3, "{turn}");
    assert!(model[usize::from(TARGET_LEFT_INDEX)].is_finite());
}

#[test]
fn a_rig_without_the_bone_skips_its_adjustment() {
    // The target has no left little finger.
    let (out, _, _) = retargeted(&still(2), &adjusted(LeftLittleDistal, about_z(-30.0)));
    let (plain, _, _) = retargeted(&still(2), &UNGROUNDED);
    assert_eq!(out.rotations, plain.rotations);
}

#[test]
fn a_gain_keeps_that_share_of_a_bones_own_turn() {
    let bend = Quat::from_rotation_y(-QUARTER);
    let clip = turned(still(2), &[(SOURCE_UPPER_LEFT_ARM + 1, bend)]);
    let lower = |options: &RetargetOptions| {
        let (out, skeleton, map) = retargeted(&clip, options);
        along(&pose(&out, &skeleton, 2), &map, LeftLowerArm, LeftHand)
    };
    let gained = |gain| RetargetOptions { gain: BTreeMap::from([(LeftLowerArm, gain)]), ..UNGROUNDED };

    assert!(lower(&UNGROUNDED).abs_diff_eq(bend * Vec3::X, 1e-5));
    assert!(lower(&gained(1.0)).abs_diff_eq(bend * Vec3::X, 1e-5), "all of it");
    let half = Quat::from_rotation_y(-QUARTER * 0.5);
    assert!(lower(&gained(0.5)).abs_diff_eq(half * Vec3::X, 1e-5), "half of it");
    assert!(lower(&gained(0.0)).abs_diff_eq(Vec3::X, 1e-5), "none of it");

    // A gain on the bone above carries what's under it, as an adjustment does.
    let upper_gain = RetargetOptions { gain: BTreeMap::from([(LeftUpperArm, 0.5)]), ..UNGROUNDED };
    let swing = Quat::from_rotation_y(-QUARTER);
    let upper_clip = turned(still(2), &[(SOURCE_UPPER_LEFT_ARM, swing)]);
    let (out, skeleton, map) = retargeted(&upper_clip, &upper_gain);
    let hand = along(&pose(&out, &skeleton, 2), &map, LeftLowerArm, LeftHand);
    assert!(hand.abs_diff_eq(half * Vec3::X, 1e-5), "{hand}");
    assert_eq!(usize::from(TARGET_LEFT_LOWER_ARM), usize::from(map.joint(LeftLowerArm).unwrap()));
}

#[test]
fn settings_a_cook_cant_honor_are_invalid() {
    let settings = |adjust: Turn, gain: f32| RetargetSettings {
        target: AssetId::from_bytes([1; 16]),
        names: Vec::new(),
        scale: None,
        ground: true,
        adjust: BTreeMap::from([(Spine, adjust)]),
        gain: BTreeMap::from([(Spine, gain)]),
        follow: Vec::new(),
    };
    assert!(settings(Turn::default(), 0.5).is_valid());
    assert!(!settings(Turn { x: f32::NAN, ..Turn::default() }, 0.5).is_valid());
    assert!(!settings(Turn { z: f32::INFINITY, ..Turn::default() }, 0.5).is_valid());
    assert!(!settings(Turn::default(), -0.5).is_valid());
    assert!(!settings(Turn::default(), f32::NAN).is_valid());
}

/// Two rigs with one joint under a name in both and no humanoid bone on it: a hand block.
fn rigs_with_a_block() -> [(Skeleton, HumanMap); 2] {
    let plain = Quat::IDENTITY;
    let turned = Quat::from_rotation_x(QUARTER);
    let figure = |rest: Quat, length: f32| {
        rig(&[
            ("Hips", None, [0.0, length, 0.0], plain, Some(Hips)),
            ("Spine", Some(0), [0.0, 1.1 * length, 0.0], plain, Some(Spine)),
            ("Head", Some(1), [0.0, 1.6 * length, 0.0], plain, Some(Head)),
            ("Wrist", Some(1), [0.7, 1.5 * length, 0.0], plain, Some(LeftHand)),
            ("Block", Some(3), [0.8, 1.5 * length, 0.0], rest, None),
            ("Slot", Some(4), [0.9, 1.5 * length, 0.0], rest, None),
            ("Other", Some(3), [0.8, 1.4 * length, 0.0], plain, None),
        ])
    };
    [figure(plain, 1.0), figure(turned, 2.0)]
}

#[test]
fn a_joint_with_no_bone_follows_by_name() {
    let [(source, source_map), (target, target_map)] = rigs_with_a_block();
    let (block, slot, other) = (4, 5, 6);
    let curl = Quat::from_rotation_z(0.7);
    let clip = turned(still(2), &[(3, Quat::from_rotation_x(0.3)), (block, curl), (slot, curl)]);
    let cook = |names: &[&str]| {
        let options = RetargetOptions {
            scale: Some(1.0),
            follow: names.iter().map(|name| (*name).to_owned()).collect(),
            ..UNGROUNDED
        };
        let (source, target) =
            (Rig { skeleton: &source, map: &source_map }, Rig { skeleton: &target, map: &target_map });
        retarget(&clip, source, target, &options)
    };

    let plain = cook(&[]).unwrap();
    assert!(track(&plain.rotations, block).is_none(), "named by nothing, it rests under its parent");

    let followed = cook(&["Block", "Slot"]).unwrap();
    let (from, to) = (rotations(&clip, &source, 2), rotations(&followed, &target, 2));
    let rests = rest_rotations(&target);
    for joint in [usize::from(block), usize::from(slot)] {
        let turn_of = |model: &[Quat], rest: Quat| model[joint] * rest.inverse();
        let source_rest = rest_rotations(&source)[joint];
        let apart = angle_between(turn_of(&from, source_rest), turn_of(&to, rests[joint]));
        assert!(apart < 1e-5, "joint {joint} turns {apart} from the source's, on a rest of its own");
    }
    assert!(track(&followed.rotations, other).is_none(), "and only what it names");
    assert_eq!(
        track(&followed.rotations, 3),
        track(&plain.rotations, 3),
        "a bone's track doesn't change by what follows it"
    );

    assert!(matches!(cook(&["Nothing"]), Err(RetargetError::UnknownJoint { which: "the source", .. })));
    assert!(matches!(cook(&["Wrist"]), Err(RetargetError::FollowedBone { bone: "leftHand", .. })));
    let (fewer, fewer_map) = source_rig();
    let error = retarget(
        &still(2),
        Rig { skeleton: &source, map: &source_map },
        Rig { skeleton: &fewer, map: &fewer_map },
        &RetargetOptions { scale: Some(1.0), follow: vec!["Block".to_owned()], ..UNGROUNDED },
    );
    assert!(matches!(error, Err(RetargetError::UnknownJoint { which: "the target", .. })));
}

// KayKit's real clips, read through their real sidecars, on the character.

pub(crate) fn third_party() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/third_party")
}

/// The character's skeleton, map and rig.
fn imported_character() -> &'static (Skeleton, HumanMap, CharacterRig) {
    static CHARACTER: OnceLock<(Skeleton, HumanMap, CharacterRig)> = OnceLock::new();
    CHARACTER.get_or_init(|| {
        let path = third_party().join("vrm1_constraint_twist_sample/VRM1_Constraint_Twist_Sample.vrm");
        let import = vrm::import(&std::fs::read(path).unwrap(), None).unwrap();
        let map = HumanMap::from_rig(&import.rig);
        (Skeleton::new(&import.mesh.skin.unwrap().joints), map, import.rig)
    })
}

pub(crate) fn character() -> (&'static Skeleton, &'static HumanMap) {
    let (skeleton, map, _) = imported_character();
    (skeleton, map)
}

pub(crate) fn character_rig() -> &'static CharacterRig {
    &imported_character().2
}

const FILES: [&str; 5] = ["General", "MovementBasic", "MovementAdvanced", "CombatMelee", "Special"];

/// One of KayKit's files as its sidecar has it, with the map its rig's owner's sidecar gives.
pub(crate) struct Kaykit {
    skeleton: Skeleton,
    map: HumanMap,
    source: ClipSource,
    settings: ClipSettings,
    names: Vec<String>,
}

impl Kaykit {
    pub(crate) fn open(file: &str) -> Self {
        let sidecar_of = |file: &str| {
            let path =
                third_party().join(format!("kaykit_character_animations/Rig_Medium/Rig_Medium_{file}.glb"));
            let meta = meta::read(&sidecar_path(&path)).unwrap();
            (path, meta)
        };
        let (path, meta) = sidecar_of(file);
        let bytes = std::fs::read(&path).unwrap();
        let source = ClipSource::import(&path, &bytes, &meta.importer).unwrap();
        let skeleton = Skeleton::new(&source.joints);
        let (_, owner) = sidecar_of("General");
        let map = HumanMap::from_names(&owner.importer.clips().unwrap().humanoid, &skeleton).unwrap();
        let settings = meta.importer.clips().unwrap().clone();
        Self { skeleton, map, source, settings, names: clips::names(&bytes).unwrap() }
    }

    pub(crate) fn rig(&self) -> Rig<'_> {
        Rig { skeleton: &self.skeleton, map: &self.map }
    }

    pub(crate) fn sampled(&self, name: &str) -> SampledClip {
        let index = self.names.iter().position(|candidate| candidate == name).expect(name);
        self.source.sample(index, &self.settings).unwrap()
    }

    /// `name` and what it becomes on the character, by default options.
    pub(crate) fn on_character(&self, name: &str) -> (SampledClip, SampledClip) {
        let (skeleton, map) = character();
        let sampled = self.sampled(name);
        let out = retarget(&sampled, self.rig(), Rig { skeleton, map }, &RetargetOptions::default()).unwrap();
        (sampled, out)
    }
}

/// How high `model`'s two feet are above where they rest, each as the lower of the foot and its
/// toes, the lower of the two first.
fn feet(model: &[Affine3A], map: &HumanMap, rest: &[Affine3A]) -> (f32, f32) {
    let height = |bone| position(model, map, bone).y - position(rest, map, bone).y;
    let (left, right) = (height(LeftFoot).min(height(LeftToes)), height(RightFoot).min(height(RightToes)));
    (left.min(right), left.max(right))
}

fn distance_to_segment(point: Vec3, a: Vec3, b: Vec3) -> f32 {
    let along = b - a;
    let share = ((point - a).dot(along) / along.length_squared()).clamp(0.0, 1.0);
    point.distance(a + along * share)
}

/// The nearest either hand comes to the torso, as hips to upper chest to head.
fn hands_to_torso(model: &[Affine3A], map: &HumanMap) -> f32 {
    let (hips, chest, head) =
        (position(model, map, Hips), position(model, map, UpperChest), position(model, map, Head));
    [LeftHand, RightHand]
        .map(|hand| {
            let at = position(model, map, hand);
            distance_to_segment(at, hips, chest).min(distance_to_segment(at, chest, head))
        })
        .into_iter()
        .fold(f32::MAX, f32::min)
}

#[test]
fn kaykits_legs_are_about_half_the_characters() {
    let (skeleton, map) = character();
    let kaykit = Kaykit::open("General");
    let scale = leg_length(Rig { skeleton, map }) / leg_length(kaykit.rig());
    assert!((scale - 2.041).abs() < 0.001, "{scale}");
}

/// The T-pose clip of every file, whichever order its joints are in: not a track of it survives a
/// cook, and so the character stands exactly at its rest.
#[test]
fn a_t_pose_clip_cooks_to_the_characters_rest() {
    let (skeleton, _) = character();
    let target = ClipTarget { skeleton: Some((AssetId::from_bytes([1; 16]), skeleton)), morph_defaults: &[] };
    for file in FILES {
        let kaykit = Kaykit::open(file);
        let (_, out) = kaykit.on_character("T-Pose");
        let cooked = clip::cook(&out, &target, &ReductionError::default());
        assert!(cooked.rotations.is_empty(), "{file}: {:?}", cooked.rotations);
        assert!(cooked.translations.is_empty(), "{file}: {:?}", cooked.translations);
        // The sidecar lifts root motion, and there's none to lift.
        let root = out.root.as_ref().expect(file);
        assert!(root.translation.iter().all(|at| at.length() < 1e-5), "{file}: {:?}", root.translation);
        assert!(root.rotation.iter().all(|turn| angle_between(*turn, Quat::IDENTITY) < 1e-5), "{file}");
    }
}

/// Gaits, by the most their lowest foot lifts off the floor, each step included: a walk always has
/// a foot on the floor, near enough, and a run's bound is its flight. In each the lowest foot
/// reaches the floor and never goes through it (to a millimeter), and the feet don't cross. The
/// strafes are left out of the crossing: the rig holds their body turned 60 degrees.
#[test]
fn walks_and_runs_keep_a_foot_on_the_floor() {
    let (skeleton, map) = character();
    let rest = pose(&still(0), skeleton, 0);
    let rows: &[(&str, &str, f32)] = &[
        ("MovementBasic", "Walking_A", 0.05),
        ("MovementBasic", "Walking_B", 0.065),
        ("MovementBasic", "Walking_C", 0.03),
        ("MovementAdvanced", "Walking_Backwards", 0.035),
        ("MovementBasic", "Running_A", 0.25),
        ("MovementBasic", "Running_B", 0.29),
        ("MovementAdvanced", "Running_Strafe_Left", 0.27),
        ("MovementAdvanced", "Running_Strafe_Right", 0.27),
    ];
    let mut failures = Vec::new();
    for &(file, name, flight) in rows {
        let (_, out) = Kaykit::open(file).on_character(name);
        let (mut floor, mut lift, mut crossing) = (f32::MAX, f32::MIN, f32::MAX);
        for tick in 0..=usize::from(out.ticks) {
            let model = pose(&out, skeleton, tick);
            let (lower, _) = feet(&model, map, &rest);
            floor = floor.min(lower);
            lift = lift.max(lower);
            crossing = crossing.min(position(&model, map, LeftFoot).x - position(&model, map, RightFoot).x);
        }
        if floor.abs() > 0.001 {
            failures.push(format!("{name}: the lowest foot only reaches {floor} from the floor"));
        }
        if lift > flight {
            failures.push(format!("{name}: the lowest foot lifts {lift}, over {flight}"));
        }
        if !name.contains("Strafe") && crossing <= 0.0 {
            failures.push(format!("{name}: the feet cross by {crossing}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The idles: hands well clear of the body (0.32 m at the least, in fact: arms held wide, which only
/// eyes can judge), and both feet planted.
#[test]
fn idles_keep_their_hands_off_the_body_and_their_feet_down() {
    let (skeleton, map) = character();
    let rest = pose(&still(0), skeleton, 0);
    let kaykit = Kaykit::open("General");
    for name in ["Idle_A", "Idle_B"] {
        let (_, out) = kaykit.on_character(name);
        for tick in 0..=usize::from(out.ticks) {
            let model = pose(&out, skeleton, tick);
            let clear = hands_to_torso(&model, map);
            assert!(clear > 0.3, "{name} tick {tick}: a hand {clear} from the torso");
            let (lower, upper) = feet(&model, map, &rest);
            assert!(lower.abs() < 0.001 && upper.abs() < 0.005, "{name} tick {tick}: {lower} {upper}");
        }
    }
}

/// One bone of each kind, and the angle in degrees between it on the character and on KayKit's rig
/// at rest, measured, plus a little: a thigh is 3.2 off, a foot 16.4 since the character's points
/// down its shoe's slope, and the character's spine starts forward of its hips, 13.5 off.
const SEGMENTS: [(HumanBone, HumanBone, f32); 12] = [
    (LeftUpperLeg, LeftLowerLeg, 3.25),
    (RightUpperLeg, RightLowerLeg, 3.25),
    (LeftLowerLeg, LeftFoot, 6.8),
    (RightLowerLeg, RightFoot, 6.8),
    (LeftFoot, LeftToes, 16.4),
    (RightFoot, RightToes, 16.4),
    (LeftUpperArm, LeftLowerArm, 3.3),
    (RightUpperArm, RightLowerArm, 3.3),
    (LeftLowerArm, LeftHand, 3.2),
    (RightLowerArm, RightHand, 3.0),
    (Hips, Spine, 13.55),
    (Spine, Chest, 1.6),
];

/// How far past its rest's difference a bone may stray over a clip, in degrees.
const STRAY: f32 = 0.25;

/// A rest-relative turn holds the angle between a bone on the two rigs at what it is at rest,
/// whatever the clip does. So this holds every clip the sidecars retarget, at every tick, to the
/// rests' difference: a bone that strayed past it would mean a channel the method drops.
#[test]
fn mapped_bones_point_where_the_sources_do_within_what_their_rests_differ_by() {
    let (skeleton, map) = character();
    let rest = pose(&still(0), skeleton, 0);
    let mut failures = Vec::new();
    for file in FILES {
        let kaykit = Kaykit::open(file);
        let source_rest = pose(&still(0), &kaykit.skeleton, 0);
        for retarget in &kaykit.settings.retarget {
            for name in &retarget.names {
                let (sampled, out) = kaykit.on_character(name);
                for tick in 0..=usize::from(out.ticks) {
                    let (from, to) = (pose(&sampled, &kaykit.skeleton, tick), pose(&out, skeleton, tick));
                    for (upper, lower, bound) in SEGMENTS {
                        let angle = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
                        let at_rest = angle(
                            along(&source_rest, &kaykit.map, upper, lower),
                            along(&rest, map, upper, lower),
                        );
                        let now =
                            angle(along(&from, &kaykit.map, upper, lower), along(&to, map, upper, lower));
                        if at_rest > bound || now > at_rest + STRAY {
                            failures.push(format!(
                                "{name} tick {tick} {}: {now} degrees, {at_rest} at rest, bound {bound}",
                                upper.name()
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures, first {:#?}",
        failures.len(),
        &failures[..failures.len().min(12)]
    );
}

/// How far `turns` go about +Y, in degrees: each step the short way round, as a clip plays them.
fn yaw_travelled(turns: &[Quat]) -> Vec<f32> {
    let mut total = 0.0;
    let mut travelled = vec![0.0];
    for pair in turns.windows(2) {
        let step = pair[1] * pair[0].inverse();
        let step = if step.w < 0.0 { -step } else { step };
        total += 2.0 * step.y.atan2(step.w).to_degrees();
        travelled.push(total);
    }
    travelled
}

/// The spin the sidecars don't list, for how far a root turn can go: root motion takes it through a
/// half turn and on round to a whole one, overshooting by 19 degrees before it settles. On the
/// character it's the same turn on its hips, and in the world each bone has turned as the source's
/// has at every tick, in spite of the rigs' axes. Cooked, it plays the same way past each half turn,
/// where a quaternion changes its sign.
#[test]
fn a_spin_of_the_root_carries_over_and_the_body_turns_with_it() {
    let (skeleton, map) = character();
    let kaykit = Kaykit::open("CombatMelee");
    let (sampled, out) = kaykit.on_character("Melee_2H_Attack_Spin");
    let (from, to) = (sampled.root.as_ref().expect("lifted"), out.root.as_ref().expect("carried"));
    assert_eq!(to.joint, map.joint(Hips).unwrap());
    assert_eq!(from.rotation, to.rotation);
    let travelled = yaw_travelled(&to.rotation);
    let halfway = travelled.iter().position(|yaw| *yaw <= -180.0).expect("past a half turn");
    assert!(travelled[halfway] > -200.0, "one tick past it: {}", travelled[halfway]);
    let furthest = travelled.iter().copied().fold(f32::MAX, f32::min);
    assert!((furthest + 379.0).abs() < 1.0, "{furthest}");
    assert!((*travelled.last().unwrap() + 360.0).abs() < 0.5, "{:?}", travelled.last());

    let (source_rest, rest) = (rotations(&still(0), &kaykit.skeleton, 0), rotations(&still(0), skeleton, 0));
    for tick in 0..=usize::from(out.ticks) {
        let (a, b) = (rotations(&sampled, &kaykit.skeleton, tick), rotations(&out, skeleton, tick));
        for bone in [Hips, Head, LeftUpperArm, RightLowerLeg] {
            let (j, k) =
                (usize::from(kaykit.map.joint(bone).unwrap()), usize::from(map.joint(bone).unwrap()));
            let world_a = from.rotation[tick] * a[j] * source_rest[j].inverse();
            let world_b = to.rotation[tick] * b[k] * rest[k].inverse();
            assert!(angle_between(world_a, world_b) < 2e-3, "{} tick {tick}", bone.name());
        }
    }

    let target = ClipTarget { skeleton: Some((AssetId::from_bytes([1; 16]), skeleton)), morph_defaults: &[] };
    let cooked = clip::cook(&out, &target, &ReductionError::default()).to_cooked().unwrap();
    let cooked = sapphire_assets::clip::Clip::read(&cooked).unwrap();
    for (tick, turn) in to.rotation.iter().enumerate() {
        let (_, played, _) = root_of(cooked, tick as f32).to_scale_rotation_translation();
        assert!(angle_between(played, *turn) < 1.5f32.to_radians(), "tick {tick}: {played} against {turn}");
    }
}

/// Root motion's travel, on the character's scale: a dodge goes as far in its strides as KayKit's.
#[test]
fn a_dodges_root_travels_by_the_legs_ratio() {
    let (skeleton, map) = character();
    let kaykit = Kaykit::open("MovementAdvanced");
    let scale = leg_length(Rig { skeleton, map }) / leg_length(kaykit.rig());
    let rows = [("Dodge_Forward", 0.25), ("Dodge_Left", 0.5), ("Dodge_Right", 0.5), ("Dodge_Backward", 0.65)];
    for (name, distance) in rows {
        let (sampled, out) = kaykit.on_character(name);
        let (from, to) = (sampled.root.as_ref().expect(name), out.root.as_ref().expect(name));
        for (a, b) in from.translation.iter().zip(&to.translation) {
            assert!((*a * scale).abs_diff_eq(*b, 1e-5), "{name}: {a} {b}");
        }
        let travel = to.translation.last().unwrap().distance(to.translation[0]);
        assert!((travel - distance * scale).abs() < 0.01, "{name}: {travel}");
    }
}

/// Every clip of the five files: it retargets, or it's one of the two whose root starts at a scale
/// of zero and root motion can't read. Clips outside the sidecars' lists are held to this much so
/// that a move can name one without a surprise.
#[test]
fn every_clip_on_the_rig_retargets_but_the_two_that_start_at_no_size() {
    let (skeleton, map) = character();
    let mut refused = Vec::new();
    let mut count = 0;
    for file in FILES {
        let kaykit = Kaykit::open(file);
        for (index, name) in kaykit.names.iter().enumerate() {
            count += 1;
            match kaykit.source.sample(index, &kaykit.settings) {
                Ok(sampled) => {
                    let options = RetargetOptions::default();
                    let out = retarget(&sampled, kaykit.rig(), Rig { skeleton, map }, &options)
                        .unwrap_or_else(|error| panic!("{name}: {error}"));
                    let finite = |samples: &[Quat]| samples.iter().all(|sample| sample.is_finite());
                    assert!(out.rotations.iter().all(|(_, samples)| finite(samples)), "{name}");
                }
                Err(ClipCookError::Import(ClipImportError::RootLift { .. })) => refused.push(name.clone()),
                Err(error) => panic!("{name}: {error}"),
            }
        }
    }
    assert_eq!(count, 76);
    refused.sort();
    assert_eq!(refused, ["Spawn_Air", "Spawn_Ground"]);
}

// The enemy's derived warrior (M2 O4): this rig's joints and rest rotations, at other lengths.

/// The joints the sidecars' `follow` names: no humanoid bone stands for them.
const HAND_BLOCKS: [&str; 4] = ["hand.l", "hand.r", "handslot.l", "handslot.r"];

fn joint_named(skeleton: &Skeleton, name: &str) -> usize {
    skeleton.names.iter().position(|candidate| candidate == name).expect(name)
}

/// What a sidecar's entry asks of a retarget, as the cook reads it.
pub(crate) fn options_of(entry: &RetargetSettings) -> RetargetOptions {
    RetargetOptions {
        scale: entry.scale,
        ground: entry.ground,
        adjust: entry.adjust.clone(),
        gain: entry.gain.clone(),
        follow: entry.follow.clone(),
    }
}

/// The enemy's model as its sidecar has it, with the sidecar's ID and map.
pub(crate) fn warrior() -> (AssetId, Skeleton, HumanMap) {
    let path = third_party().join("../combat/enemy/warrior.glb");
    let meta = meta::read(&sidecar_path(&path)).unwrap();
    let source = ClipSource::import(&path, &std::fs::read(&path).unwrap(), &meta.importer).unwrap();
    let skeleton = Skeleton::new(&source.joints);
    let map = HumanMap::from_names(&meta.importer.clips().unwrap().humanoid, &skeleton).unwrap();
    (meta.id, skeleton, map)
}

/// Every clip the sidecars send to the warrior, with its source's own lengths left behind. Its legs
/// are the rig's stretched 2.1 times, and so is the travel, so the lowest foot reaches the floor and
/// lifts only as far as the clip does (the bound is the measured value and a little), and its rests
/// are the rig's, so each mapped bone turns exactly as the source's. `hand.l`, `hand.r` and the hand
/// slots under them have no bone, so the sidecars name them under `follow`, and they turn exactly as
/// the source's too: a blade held at `handslot.r` stays in the clip's grip. Left at rest they'd miss
/// it by 67 to 94 degrees, depending on the clip, which is what the option is for.
#[test]
fn the_rigs_clips_reach_the_warrior_at_its_own_lengths() {
    let (id, skeleton, map) = warrior();
    let rows = [
        ("General", "Hit_A", 0.005),
        ("General", "Hit_B", 0.005),
        ("MovementBasic", "Running_A", 0.26),
        ("CombatMelee", "Melee_1H_Attack_Chop", 0.155),
        ("CombatMelee", "Melee_1H_Attack_Slice_Diagonal", 0.025),
        ("CombatMelee", "Melee_1H_Attack_Stab", 0.005),
    ];
    let mapped = [
        Hips,
        Spine,
        Chest,
        Head,
        LeftUpperLeg,
        LeftLowerLeg,
        LeftFoot,
        LeftToes,
        RightUpperLeg,
        RightLowerLeg,
        RightFoot,
        RightToes,
        LeftUpperArm,
        LeftLowerArm,
        LeftHand,
        RightUpperArm,
        RightLowerArm,
        RightHand,
    ];
    let rest = pose(&still(0), &skeleton, 0);
    let target_rest = rotations(&still(0), &skeleton, 0);
    let mut failures = Vec::new();
    for file in FILES {
        let kaykit = Kaykit::open(file);
        let entries = || kaykit.settings.retarget.iter().filter(|entry| entry.target == id);
        let named: Vec<&str> = entries().flat_map(|entry| &entry.names).map(String::as_str).collect();
        let expected: Vec<&str> = rows.iter().filter(|row| row.0 == file).map(|row| row.1).collect();
        assert_eq!(named, expected, "{file}'s sidecar");
        for &(_, name, flight) in rows.iter().filter(|row| row.0 == file) {
            let sampled = kaykit.sampled(name);
            let target = Rig { skeleton: &skeleton, map: &map };
            let entry = entries().find(|entry| entry.names.iter().any(|n| n == name)).unwrap();
            let out = retarget(&sampled, kaykit.rig(), target, &options_of(entry)).unwrap();
            let unfollowed = RetargetOptions { follow: Vec::new(), ..options_of(entry) };
            let at_rest = retarget(&sampled, kaykit.rig(), target, &unfollowed).unwrap();
            let source_rest = rotations(&still(0), &kaykit.skeleton, 0);
            let (mut floor, mut lift, mut turned_apart) = (f32::MAX, f32::MIN, 0.0f32);
            let (mut grip, mut grip_at_rest) = (0.0f32, 0.0f32);
            for tick in 0..=usize::from(out.ticks) {
                let (lower, _) = feet(&pose(&out, &skeleton, tick), &map, &rest);
                floor = floor.min(lower);
                lift = lift.max(lower);
                let (from, to) =
                    (rotations(&sampled, &kaykit.skeleton, tick), rotations(&out, &skeleton, tick));
                for bone in mapped {
                    let (source, joint) = (kaykit.map.joint(bone).unwrap(), map.joint(bone).unwrap());
                    let (source, joint) = (usize::from(source), usize::from(joint));
                    let delta =
                        |model: &[Quat], rest: &[Quat], joint: usize| model[joint] * rest[joint].inverse();
                    let (a, b) = (delta(&from, &source_rest, source), delta(&to, &target_rest, joint));
                    turned_apart = turned_apart.max(angle_between(a, b));
                }
                let plain = rotations(&at_rest, &skeleton, tick);
                for block in HAND_BLOCKS {
                    let (source, joint) =
                        (joint_named(&kaykit.skeleton, block), joint_named(&skeleton, block));
                    grip = grip.max(angle_between(from[source], to[joint]));
                    grip_at_rest = grip_at_rest.max(angle_between(from[source], plain[joint]));
                }
            }
            if grip > 0.001 {
                failures.push(format!("{name}: a hand block misses the clip's grip by {grip} rad"));
            }
            if grip_at_rest < 1.0 {
                failures.push(format!("{name}: at rest they'd only miss by {grip_at_rest} rad"));
            }
            if floor.abs() > 0.001 {
                failures.push(format!("{name}: the lowest foot only reaches {floor} from the floor"));
            }
            if lift > flight {
                failures.push(format!("{name}: the lowest foot lifts {lift}, over {flight}"));
            }
            if turned_apart > 0.01 {
                failures.push(format!("{name}: a mapped bone turns {turned_apart} rad from the source's"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    let scale =
        leg_length(Rig { skeleton: &skeleton, map: &map }) / leg_length(Kaykit::open("General").rig());
    assert!((scale - 2.1).abs() < 1e-3, "the legs' stretch: {scale}");
}

/// What the sidecars ask of `name` on the rig `target`.
pub(crate) fn sidecar_options(kaykit: &Kaykit, name: &str, target: AssetId) -> RetargetOptions {
    let mut entries = kaykit.settings.retarget.iter().filter(|entry| entry.target == target);
    let entry = entries.find(|entry| entry.names.iter().any(|n| n == name));
    options_of(entry.unwrap_or_else(|| panic!("{name} doesn't cook for {target}")))
}

/// A clip on the character as the sidecars cook it, with their adjustments and gains.
pub(crate) fn as_cooked(kaykit: &Kaykit, name: &str) -> SampledClip {
    let (skeleton, map) = character();
    let the_character = AssetId::from_text("6f8a81b6-29c4-485f-b161-847b001d561f");
    let options = sidecar_options(kaykit, name, the_character);
    retarget(&kaykit.sampled(name), kaykit.rig(), Rig { skeleton, map }, &options).unwrap()
}

/// What the sheets judged, held as numbers, by each clip's measured value and a little: M2 O2's
/// sidecars bring the arms in from their 32 to 58° off straight down at the source's angles, without
/// burying a hand in the body, and leave a foot on the floor, under it by no more than a millimeter.
#[test]
fn as_cooked_the_arms_hang_close_and_a_foot_stays_down() {
    let (skeleton, map) = character();
    let rest = pose(&still(0), skeleton, 0);
    // A clip, the file it's in, the most its arms come within straight down at their closest, the least
    // a hand keeps from the torso, and the most its lowest foot leaves the floor.
    let table: [(&str, &str, f32, f32, f32); 7] = [
        ("Walking_B", "MovementBasic", 19.0, 0.2, 0.065),
        ("Running_B", "MovementBasic", 19.0, 0.09, 0.29),
        ("Walking_Backwards", "MovementAdvanced", 27.0, 0.2, 0.03),
        ("Running_Strafe_Left", "MovementAdvanced", 33.0, 0.25, 0.27),
        ("Running_Strafe_Right", "MovementAdvanced", 33.0, 0.25, 0.27),
        ("Hit_A", "General", 29.0, 0.2, 0.001),
        ("Hit_B", "General", 23.0, 0.2, 0.001),
    ];
    let mut failures = Vec::new();
    for (name, file, arm_limit, clear_limit, lift_limit) in table {
        let kaykit = Kaykit::open(file);
        let out = as_cooked(&kaykit, name);
        let (mut arm, mut clear, mut highest, mut lowest) = (f32::MAX, f32::MAX, f32::MIN, f32::MAX);
        for tick in 0..=usize::from(out.ticks) {
            let model = pose(&out, skeleton, tick);
            for (upper, lower) in [(LeftUpperArm, LeftLowerArm), (RightUpperArm, RightLowerArm)] {
                let cosine = along(&model, map, upper, lower).dot(Vec3::NEG_Y).clamp(-1.0, 1.0);
                arm = arm.min(cosine.acos().to_degrees());
            }
            clear = clear.min(hands_to_torso(&model, map));
            let (lower, _) = feet(&model, map, &rest);
            highest = highest.max(lower);
            lowest = lowest.min(lower);
        }
        if arm > arm_limit || clear < clear_limit || highest > lift_limit || lowest < -0.001 {
            failures.push(format!("{name}: arm {arm}° hand {clear} m foot {lowest} to {highest} m"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// What the animation graph matches its speed to: how fast the planted foot passes under the character
/// at 1×, as a median in meters a second. The run covers what the controller's run does (6 m/s).
#[test]
fn as_cooked_the_run_covers_the_ground_the_controller_does() {
    let (skeleton, map) = character();
    let rest = pose(&still(0), skeleton, 0);
    let planted_speed = |file: &str, name: &str| {
        let kaykit = Kaykit::open(file);
        let out = as_cooked(&kaykit, name);
        let models: Vec<Vec<Affine3A>> =
            (0..=usize::from(out.ticks)).map(|tick| pose(&out, skeleton, tick)).collect();
        let mut speeds = Vec::new();
        for pair in models.windows(2) {
            for bone in [LeftFoot, RightFoot] {
                let height = |model: &[Affine3A]| position(model, map, bone).y - position(&rest, map, bone).y;
                if height(&pair[0]) < 0.01 && height(&pair[1]) < 0.01 {
                    let step = position(&pair[1], map, bone) - position(&pair[0], map, bone);
                    speeds.push(Vec3::new(step.x, 0.0, step.z).length() * f32::from(out.rate));
                }
            }
        }
        speeds.sort_by(f32::total_cmp);
        speeds[speeds.len() / 2]
    };
    let run = planted_speed("MovementBasic", "Running_B");
    assert!((5.0..7.0).contains(&run), "{run}");
    let walk = planted_speed("MovementBasic", "Walking_B");
    assert!((0.7..1.5).contains(&walk), "{walk}");
}

/// `follow`, on a real clip: the rig retargeted onto itself, its hand blocks and slots named, takes
/// each block's local rotation as the clip has it, the grip a held blade hangs from.
#[test]
fn a_followed_hand_block_keeps_the_clips_grip() {
    let kaykit = Kaykit::open("CombatMelee");
    let sampled = kaykit.sampled("Melee_1H_Attack_Chop");
    let options = RetargetOptions {
        follow: HAND_BLOCKS.iter().map(|name| (*name).to_owned()).collect(),
        ..RetargetOptions::default()
    };
    let out = retarget(&sampled, kaykit.rig(), kaykit.rig(), &options).unwrap();
    let plain = retarget(&sampled, kaykit.rig(), kaykit.rig(), &RetargetOptions::default()).unwrap();
    let joint_of =
        |name: &str| kaykit.skeleton.names.iter().position(|candidate| candidate == name).expect(name) as u16;
    let (mut moved, mut without) = (0.0_f32, 0.0_f32);
    for name in HAND_BLOCKS {
        let joint = joint_of(name);
        let Some(source) = track(&sampled.rotations, joint) else { continue };
        let rest = Quat::from_array(kaykit.skeleton.rest[usize::from(joint)].rotation).normalize();
        assert!(track(&plain.rotations, joint).is_none(), "{name} rests without it");
        for (tick, key) in source.iter().enumerate() {
            let followed = rotations(&out, &kaykit.skeleton, tick)[usize::from(joint)];
            let parent = kaykit.skeleton.parents[usize::from(joint)];
            let above = rotations(&out, &kaykit.skeleton, tick)[usize::from(parent)];
            let local = (above.inverse() * followed).normalize();
            let apart = angle_between(local, key.normalize());
            assert!(apart < 0.002, "{name} tick {tick}: {apart} from the clip's");
            moved = moved.max(apart);
            without = without.max(angle_between(rest, key.normalize()));
        }
    }
    assert!(moved < without, "the option does something: {moved} against {without}");
    assert!(without > 1.0, "the clip's grip curls well past rest: {without}");
}
