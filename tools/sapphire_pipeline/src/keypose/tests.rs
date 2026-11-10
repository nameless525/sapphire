//! Documents as a person writes them, cooked and read back through a clip's own format.

use std::f32::consts::FRAC_PI_2;
use std::fs;

use glam::{Affine3A, Quat, Vec3};
use sapphire_assets::clip::Clip;
use sapphire_assets::rig::{CharacterRig, HumanBone, HumanoidJoint};
use sapphire_assets::skeleton::{Joint, JointTransform};
use sapphire_math::det;
use sapphire_serialize::cooked::AlignedVec;

use super::*;
use crate::anim_graph::LocomotionBounds;
use crate::clip::tests::{angle_between, model_of, root_of, sample_pose};
use crate::clip::{ClipTarget, cook};
use crate::import::vrm;
use crate::meta;

const HIPS: usize = 1;
const SPINE: usize = 2;
const LEFT_UPPER_ARM: usize = 3;
const LEFT_LOWER_ARM: usize = 4;
const LEFT_HAND: usize = 5;
const RIGHT_UPPER_ARM: usize = 6;
const RIGHT_LOWER_ARM: usize = 7;

/// [`super::sample`], on a rig's map and with no morph targets.
fn sample(
    document: &KeyPoseClip,
    name: &str,
    skeleton: &Skeleton,
    rig: &CharacterRig,
) -> Result<SampledClip, KeyPoseError> {
    super::sample(document, name, skeleton, &HumanMap::from_rig(rig), &[])
}

/// A figure in a T-pose, facing +Z: arms along ±X. The left arm's joints have axes of their own (+Y
/// down the bone, as some armatures do), to show a turn doesn't read them.
fn figure() -> (Skeleton, CharacterRig) {
    figure_scaled(1.0)
}

/// [`figure`] under a root scaled by `scale`, the hips placed so they rest at the same height.
fn figure_scaled(scale: f32) -> (Skeleton, CharacterRig) {
    let at = |translation: [f32; 3]| JointTransform { translation, ..JointTransform::IDENTITY };
    let along_y = Quat::from_rotation_z(-FRAC_PI_2).to_array();
    let joints = vec![
        Joint {
            name: "Root".into(),
            parent: None,
            rest: JointTransform { scale: [scale; 3], ..JointTransform::IDENTITY },
        },
        Joint { name: "Hips".into(), parent: Some(0), rest: at([0.0, 1.0 / scale, 0.0]) },
        Joint { name: "Spine".into(), parent: Some(1), rest: at([0.0, 0.1, 0.0]) },
        Joint {
            name: "LeftUpperArm".into(),
            parent: Some(2),
            rest: JointTransform { translation: [0.1, 0.3, 0.0], rotation: along_y, scale: [1.0; 3] },
        },
        Joint { name: "LeftLowerArm".into(), parent: Some(3), rest: at([0.0, 0.3, 0.0]) },
        Joint { name: "LeftHand".into(), parent: Some(4), rest: at([0.0, 0.25, 0.0]) },
        Joint { name: "RightUpperArm".into(), parent: Some(2), rest: at([-0.1, 0.3, 0.0]) },
        Joint { name: "RightLowerArm".into(), parent: Some(6), rest: at([-0.3, 0.0, 0.0]) },
    ];
    let mut humanoid = vec![
        HumanoidJoint { bone: HumanBone::Hips, joint: HIPS as u16 },
        HumanoidJoint { bone: HumanBone::Spine, joint: SPINE as u16 },
        HumanoidJoint { bone: HumanBone::LeftUpperArm, joint: LEFT_UPPER_ARM as u16 },
        HumanoidJoint { bone: HumanBone::LeftLowerArm, joint: LEFT_LOWER_ARM as u16 },
        HumanoidJoint { bone: HumanBone::LeftHand, joint: LEFT_HAND as u16 },
        HumanoidJoint { bone: HumanBone::RightUpperArm, joint: RIGHT_UPPER_ARM as u16 },
        HumanoidJoint { bone: HumanBone::RightLowerArm, joint: RIGHT_LOWER_ARM as u16 },
    ];
    humanoid.sort_by_key(|entry| entry.bone);
    let rig = CharacterRig { joint_count: joints.len() as u32, humanoid, ..CharacterRig::default() };
    (Skeleton::new(&joints), rig)
}

/// `data`, a document's body as it would be written.
fn document(data: &str) -> KeyPoseClip {
    let text = format!("(type: \"sapphire::pipeline::KeyPoseClip\", version: 1, data: ({data}))");
    read(text.as_bytes()).unwrap_or_else(|error| panic!("{data}: {error:?}"))
}

/// A cooked clip and skeleton, ready to sample.
struct Cooked {
    clip: AlignedVec,
    skeleton: AlignedVec,
}

impl Cooked {
    fn new(document: &KeyPoseClip, skeleton: &Skeleton, rig: &CharacterRig) -> Self {
        let sampled = sample(document, "test", skeleton, rig).expect("it samples");
        let target =
            ClipTarget { skeleton: Some((AssetId::from_bytes([7; 16]), skeleton)), morph_defaults: &[] };
        let clip = cook(&sampled, &target, &ReductionError::default()).to_cooked().expect("cooked");
        Clip::read(&clip).expect("a valid clip");
        Self { clip, skeleton: skeleton.to_cooked().expect("cooked") }
    }

    /// Each joint's model transform at `position`, without root motion.
    fn model(&self, position: f32) -> Vec<Affine3A> {
        let (clip, skeleton) = (Clip::read(&self.clip).unwrap(), Skeleton::read(&self.skeleton).unwrap());
        model_of(&sample_pose(clip, skeleton, position), skeleton)
    }

    /// Where joint `joint` is at `position`.
    fn position(&self, joint: usize, position: f32) -> Vec3 {
        Vec3::from(self.model(position)[joint].translation)
    }

    /// The direction from joint `from` to joint `to` at `position`.
    fn along(&self, from: usize, to: usize, position: f32) -> Vec3 {
        (self.position(to, position) - self.position(from, position)).normalize()
    }
}

fn near(a: Vec3, b: Vec3, tolerance: f32) -> bool {
    a.distance(b) <= tolerance
}

/// The worst and the ends of an ease: a table of what each promises.
#[test]
fn eases_start_and_end_on_their_keys_and_shape_what_lies_between() {
    let all = [
        KeyEase::Smooth,
        KeyEase::Linear,
        KeyEase::In,
        KeyEase::Out,
        KeyEase::Hold,
        KeyEase::Anticipate(0.1),
        KeyEase::Overshoot(0.1),
        KeyEase::Curve(0.42, 0.0, 0.58, 1.0),
        KeyEase::Curve(0.3, 1.6, 0.6, 0.9),
    ];
    for ease in all {
        assert_eq!((ease.weight(0.0), ease.weight(1.0)), (0.0, 1.0), "{ease:?}");
        assert_eq!((ease.weight(-1.0), ease.weight(2.0)), (0.0, 1.0), "{ease:?}");
        for sample in 1..100 {
            assert!(ease.weight(sample as f32 / 100.0).is_finite(), "{ease:?}");
        }
    }
    assert_eq!(KeyEase::Linear.weight(0.25), 0.25);
    assert!((KeyEase::Smooth.weight(0.5) - 0.5).abs() < 1e-6);
    assert!(KeyEase::Smooth.weight(0.1) < 0.1, "slow to leave");
    assert!(KeyEase::Smooth.weight(0.9) > 0.9, "slow to arrive");
    assert!(KeyEase::In.weight(0.5) < 0.5, "arrives at speed");
    assert!(KeyEase::Out.weight(0.5) > 0.5, "leaves at speed");
    assert_eq!(KeyEase::Hold.weight(0.999), 0.0, "a cut");
    assert!((KeyEase::Curve(0.42, 0.0, 0.58, 1.0).weight(0.5) - 0.5).abs() < 1e-4);
    assert!((KeyEase::Curve(1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0).weight(0.3) - 0.3).abs() < 1e-4);
}

/// A share is what it says: the dip or the peak is that share of the travel.
#[test]
fn anticipation_and_overshoot_reach_past_the_keys_by_their_share() {
    for share in [0.05, 0.1, 0.25, 1.0] {
        let weights = |ease: KeyEase| (0..=1000).map(move |at| ease.weight(at as f32 / 1000.0));
        let dip = weights(KeyEase::Anticipate(share)).fold(f32::INFINITY, f32::min);
        assert!((dip + share).abs() < 2e-3, "a dip of {share}: {dip}");
        let peak = weights(KeyEase::Overshoot(share)).fold(f32::NEG_INFINITY, f32::max);
        assert!((peak - 1.0 - share).abs() < 2e-3, "a peak of {share}: {peak}");
    }
    assert_eq!(KeyEase::Anticipate(0.0).weight(0.5), 0.125, "no share is a plain ease in");
}

/// As the held pose does, and as its tests measure (`sapphire_stage`'s `pose`).
#[test]
fn a_turn_is_in_rest_model_space_whatever_a_joints_own_axes() {
    let (skeleton, rig) = figure();
    let lowered = document(
        "keys: [(tick: 0, pose: {
            LeftUpperArm: (z: -70),
            RightUpperArm: (z: 70),
        })]",
    );
    let cooked = Cooked::new(&lowered, &skeleton, &rig);
    let down = Quat::from_rotation_z(-70f32.to_radians());
    let elbow = Vec3::new(0.1, 1.4, 0.0) + down * Vec3::new(0.3, 0.0, 0.0);
    assert!(
        near(cooked.position(LEFT_LOWER_ARM, 0.0), elbow, 1e-3),
        "{}",
        cooked.position(LEFT_LOWER_ARM, 0.0)
    );
    // Each arm is lowered by its turn, though one's axes run down the bone and the other's don't.
    let (left, right) = (
        cooked.along(LEFT_UPPER_ARM, LEFT_LOWER_ARM, 0.0),
        cooked.along(RIGHT_UPPER_ARM, RIGHT_LOWER_ARM, 0.0),
    );
    assert!(near(left, Vec3::new(-right.x, right.y, 0.0), 1e-3), "{left} and {right}");
    assert!(left.y < -0.9 && left.x > 0.3, "{left}");
    // What hangs from the arm goes with it.
    assert!(near(cooked.along(LEFT_LOWER_ARM, LEFT_HAND, 0.0), left, 1e-3));
}

#[test]
fn a_bone_under_a_turned_bone_turns_with_it_then_by_its_own_turn() {
    let (skeleton, rig) = figure();
    let bent = document("keys: [(tick: 0, pose: { LeftUpperArm: (z: -90), LeftLowerArm: (z: 45) })]");
    let cooked = Cooked::new(&bent, &skeleton, &rig);
    assert!(near(cooked.along(LEFT_UPPER_ARM, LEFT_LOWER_ARM, 0.0), Vec3::NEG_Y, 1e-3));
    let forearm = (Quat::from_rotation_z(-FRAC_PI_2) * Quat::from_rotation_z(FRAC_PI_2 / 2.0)) * Vec3::X;
    let found = cooked.along(LEFT_LOWER_ARM, LEFT_HAND, 0.0);
    assert!(near(found, forearm, 1e-3), "{found} against {forearm}");
}

/// A turn's axes apply X, then Y, then Z, as `lookdev::turns` builds its own.
#[test]
fn axes_apply_x_then_y_then_z() {
    let turn = Turn { x: 10.0, y: -20.0, z: 30.0 }.quat();
    let expected = Quat::from_rotation_z(30f32.to_radians())
        * Quat::from_rotation_y((-20f32).to_radians())
        * Quat::from_rotation_x(10f32.to_radians());
    assert!(angle_between(turn, expected) < 1e-5);
}

/// What a document doesn't name, it keeps: blocking stays short.
#[test]
fn a_key_keeps_what_it_doesnt_name_and_a_first_key_starts_from_rest() {
    let (skeleton, rig) = figure();
    let held = document(
        "keys: [
            (tick: 0, pose: { LeftUpperArm: (z: -90) }),
            (tick: 10, ease: Linear, pose: { RightUpperArm: (z: 90) }),
            (tick: 20, ease: Linear, pose: { LeftUpperArm: (z: 0) }),
        ]",
    );
    let cooked = Cooked::new(&held, &skeleton, &rig);
    let left = |at: f32| cooked.along(LEFT_UPPER_ARM, LEFT_LOWER_ARM, at);
    let right = |at: f32| cooked.along(RIGHT_UPPER_ARM, RIGHT_LOWER_ARM, at);
    assert!(near(left(10.0), Vec3::NEG_Y, 1e-3), "the second key keeps the left arm: {}", left(10.0));
    assert!(near(right(0.0), Vec3::NEG_X, 1e-3), "a first key's unnamed bones rest: {}", right(0.0));
    assert!(near(right(20.0), Vec3::NEG_Y, 1e-3), "the third key keeps the right arm");
    assert!(near(left(20.0), Vec3::X, 1e-3), "and raises the left");
    // Halfway through a segment, linearly: 45° down.
    let halfway = Quat::from_rotation_z(-FRAC_PI_2 / 2.0) * Vec3::X;
    assert!(near(left(15.0), halfway, 1e-3), "{}", left(15.0));
}

/// Cooked and read back, against the curve the document describes: at keys, between them, and in
/// segments that leave the keys' range.
#[test]
fn a_cooked_clip_follows_its_eases_between_keys() {
    let (skeleton, rig) = figure();
    let moved = document(
        "keys: [
            (tick: 0, pose: { LeftUpperArm: (z: 0) }),
            (tick: 30, ease: Overshoot(0.25), pose: { LeftUpperArm: (z: -40) }),
            (tick: 60, ease: Anticipate(0.25), pose: { LeftUpperArm: (z: -80) }),
            (tick: 90, ease: Hold, pose: { LeftUpperArm: (z: -20) }),
        ]",
    );
    let cooked = Cooked::new(&moved, &skeleton, &rig);
    let angle_at = |tick: f32| {
        let arm = cooked.along(LEFT_UPPER_ARM, LEFT_LOWER_ARM, tick);
        arm.y.atan2(arm.x).to_degrees()
    };
    let expected = |tick: u16| -> f32 {
        let (from, to, u, ease) = match tick {
            0..30 => (0.0, -40.0, f32::from(tick) / 30.0, KeyEase::Overshoot(0.25)),
            30..60 => (-40.0, -80.0, f32::from(tick - 30) / 30.0, KeyEase::Anticipate(0.25)),
            60..90 => (-80.0, -20.0, f32::from(tick - 60) / 30.0, KeyEase::Hold),
            _ => (-20.0, -20.0, 0.0, KeyEase::Hold),
        };
        from + (to - from) * ease.weight(u)
    };
    for tick in 0..=90u16 {
        let (found, wanted) = (angle_at(f32::from(tick)), expected(tick));
        // A tenth of a degree holds every track's tolerance on this small a figure.
        assert!((found - wanted).abs() < 0.1, "tick {tick}: {found}° against {wanted}°");
    }
    // Past the mark and back.
    let peak = (0..30).map(|tick| angle_at(tick as f32)).fold(f32::INFINITY, f32::min);
    assert!((peak + 50.0).abs() < 0.2, "overshoots -40° by a quarter of its 40°: {peak}");
    let pull = (30..60).map(|tick| angle_at(tick as f32)).fold(f32::NEG_INFINITY, f32::max);
    assert!((pull + 30.0).abs() < 0.2, "pulls back past -40° by a quarter of its 40°: {pull}");
    // A cut holds to the tick before and lands on its tick.
    assert!((angle_at(89.0) + 80.0).abs() < 0.1);
    assert!((angle_at(90.0) + 20.0).abs() < 0.1);
}

#[test]
fn ticks_hold_the_last_key_to_the_end() {
    let (skeleton, rig) = figure();
    let short = document(
        "ticks: 40, keys: [(tick: 0, pose: {}), (tick: 10, ease: Linear, pose: { Spine: (x: 30) })]",
    );
    let cooked = Cooked::new(&short, &skeleton, &rig);
    assert_eq!(Clip::read(&cooked.clip).unwrap().ticks.to_native(), 40);
    let turned = |at: f32| cooked.model(at)[SPINE].to_scale_rotation_translation().1;
    assert!(angle_between(turned(10.0), turned(40.0)) < 1e-3);
    assert!(angle_between(turned(40.0), Quat::from_rotation_x(30f32.to_radians())) < 1e-3);
    let unset = document("keys: [(tick: 0, pose: {}), (tick: 10, ease: Linear, pose: { Spine: (x: 30) })]");
    let cooked = Cooked::new(&unset, &skeleton, &rig);
    assert_eq!(Clip::read(&cooked.clip).unwrap().ticks.to_native(), 10, "its last key's, unless said");
}

/// `hips` moves the hips in model space, whatever scale they sit under.
#[test]
fn hips_offset_from_rest_in_model_space() {
    for scale in [1.0, 0.01] {
        let (skeleton, rig) = figure_scaled(scale);
        let crouch = document(
            "keys: [
                (tick: 0),
                (tick: 10, ease: Linear, hips: (0.0, -0.2, 0.05)),
                (tick: 20, ease: Linear),
                (tick: 30, ease: Linear, hips: (0.0, 0.0, 0.0)),
            ]",
        );
        let cooked = Cooked::new(&crouch, &skeleton, &rig);
        let hips = |at: f32| cooked.position(HIPS, at);
        assert!(near(hips(0.0), Vec3::Y, 1e-3), "{scale}: {}", hips(0.0));
        assert!(near(hips(10.0), Vec3::new(0.0, 0.8, 0.05), 1e-3), "{scale}: {}", hips(10.0));
        assert!(near(hips(20.0), Vec3::new(0.0, 0.8, 0.05), 1e-3), "a key keeps it: {}", hips(20.0));
        assert!(near(hips(5.0), Vec3::new(0.0, 0.9, 0.025), 1e-3), "{}", hips(5.0));
        assert!(near(hips(30.0), Vec3::Y, 1e-3));
        // The body goes with them.
        assert!(near(cooked.position(SPINE, 10.0), Vec3::new(0.0, 0.8 + 0.1 * scale, 0.05), 1e-3));
    }
}

/// `root` is root motion: its own track, which leaves the pose alone.
#[test]
fn root_is_root_motion_in_a_track_of_its_own() {
    let (skeleton, rig) = figure();
    let lunge = document(
        "keys: [
            (tick: 0, hips: (0.0, -0.1, 0.0)),
            (tick: 12, ease: Overshoot(0.25), root: (0.0, 0.0, 0.6)),
            (tick: 24, ease: Linear),
        ]",
    );
    let cooked = Cooked::new(&lunge, &skeleton, &rig);
    let clip = Clip::read(&cooked.clip).unwrap();
    let root = clip.root.as_ref().expect("root motion");
    assert_eq!(usize::from(root.joint.to_native()), HIPS, "lifted from the hips, by name");
    assert_eq!(root.rotation.count.to_native(), 1, "no turn yet");
    let travelled = |at: f32| Vec3::from(root_of(clip, at).translation);
    assert!(near(travelled(0.0), Vec3::ZERO, 1e-3));
    assert!(near(travelled(12.0), Vec3::new(0.0, 0.0, 0.6), 1e-3), "{}", travelled(12.0));
    assert!(near(travelled(24.0), Vec3::new(0.0, 0.0, 0.6), 1e-3), "a key keeps it");
    let past = (0..12).map(|tick| travelled(tick as f32).z).fold(f32::NEG_INFINITY, f32::max);
    assert!((past - 0.75).abs() < 0.01, "an ease works on root motion too: {past}");
    assert!(near(cooked.position(HIPS, 12.0), Vec3::new(0.0, 0.9, 0.0), 1e-3), "the pose stays where it was");
}

/// A turn is an angle, not a rotation: in one segment it may go on past a half turn, the way it was
/// going, where blending two rotations would go the short way back.
#[test]
fn a_root_turn_is_an_angle_that_may_pass_a_half_turn_in_one_segment() {
    let (skeleton, rig) = figure();
    let spin = document("keys: [(tick: 0), (tick: 30, ease: Linear, root_turn: 270)]");
    let cooked = Cooked::new(&spin, &skeleton, &rig);
    let clip = Clip::read(&cooked.clip).unwrap();
    for tick in 0..=30 {
        let (_, turn, _) = root_of(clip, tick as f32).to_scale_rotation_translation();
        let expected = Quat::from_rotation_y((tick as f32 * 9.0).to_radians());
        assert!(
            angle_between(turn, expected) < 1.5f32.to_radians(),
            "tick {tick}: {turn} against {expected}"
        );
    }
    let root = clip.root.as_ref().expect("root motion");
    assert_eq!(root.translation.count.to_native(), 1, "no travel");
    // A key that doesn't name it keeps it, as with `root`, and an ease shapes it.
    let held = document(
        "keys: [(tick: 0), (tick: 10, ease: Hold, root_turn: 90), (tick: 20, pose: { Spine: (x: 1) })]",
    );
    let sampled = sample(&held, "test", &skeleton, &rig).unwrap();
    let turns = sampled.root.expect("root motion").rotation;
    let quarter = Quat::from_rotation_y(FRAC_PI_2);
    assert!(angle_between(turns[9], Quat::IDENTITY) < 1e-5, "{}", turns[9]);
    assert!(angle_between(turns[10], quarter) < 1e-5, "{}", turns[10]);
    assert!(angle_between(turns[20], quarter) < 1e-5, "{}", turns[20]);
}

#[test]
fn a_keys_morph_weights_start_from_the_meshs_and_go_where_keys_take_them() {
    let (skeleton, rig) = figure();
    let targets = [
        MorphTarget { name: "unnamed".into(), default_weight: 1.0 },
        MorphTarget { name: "blink".into(), default_weight: 0.25 },
        MorphTarget { name: "smile".into(), default_weight: 0.0 },
    ];
    let map = HumanMap::from_rig(&rig);
    let keyed = document(
        "keys: [
            (tick: 0),
            (tick: 10, ease: Linear, morphs: { \"smile\": 1.0 }),
            (tick: 20, ease: Linear, morphs: { \"blink\": 1.0 }),
        ]",
    );
    let sampled = super::sample(&keyed, "test", &skeleton, &map, &targets).unwrap();
    let (blink, smile) = (&sampled.weights[0], &sampled.weights[1]);
    assert_eq!(sampled.weights.len(), 2, "a target no key names has no track");
    assert_eq!((blink.0, smile.0), (1, 2));
    assert_eq!((blink.1[0], blink.1[10], blink.1[15], blink.1[20]), (0.25, 0.25, 0.625, 1.0));
    assert_eq!((smile.1[0], smile.1[5], smile.1[10], smile.1[20]), (0.0, 0.5, 1.0, 1.0));

    // A mesh that hasn't one: the document's error, naming it. No mesh at all: as a bone it lacks.
    let wrong = document("keys: [(tick: 0, morphs: { \"frown\": 1.0 })]");
    let error = super::sample(&wrong, "test", &skeleton, &map, &targets).unwrap_err();
    assert!(matches!(&error, KeyPoseError::UnknownMorph { name } if name == "frown"), "{error}");
    assert!(error.to_string().contains("\"frown\""), "{error}");
    let sampled = super::sample(&wrong, "test", &skeleton, &map, &[]).unwrap();
    assert!(sampled.weights.is_empty());
}

/// A document's root turn and weights go through scalars and `det`, for a move's tables to read, with
/// no `Quat` arithmetic. So they equal a formula of scalars bit for bit, which any `Quat` operator on
/// their way would miss on one architecture or the other. `moves::tests` pins the rest of a document.
#[test]
fn a_documents_turn_and_weights_come_through_scalars_alone() {
    let (skeleton, rig) = figure();
    let targets = [MorphTarget { name: "blink".into(), default_weight: 0.25 }];
    let keyed = document(
        "keys: [
            (tick: 0),
            (tick: 30, ease: Linear, root_turn: 270, morphs: { \"blink\": 1.0 }),
        ]",
    );
    let sampled = super::sample(&keyed, "test", &skeleton, &HumanMap::from_rig(&rig), &targets).unwrap();
    let turns = &sampled.root.as_ref().expect("root motion").rotation;
    let weights = &sampled.weights[0].1;
    for tick in 0..=30u16 {
        let at = usize::from(tick);
        let progress = f32::from(tick) / 30.0;
        let (sine, cosine) = det::sin_cos((270.0 * progress).to_radians() * 0.5);
        let expected = [0.0 * sine, 1.0 * sine, 0.0 * sine, cosine];
        assert_eq!(turns[at].to_array().map(f32::to_bits), expected.map(f32::to_bits), "tick {tick}");
        assert_eq!(weights[at].to_bits(), (0.25 + 0.75 * progress).to_bits(), "tick {tick}");
    }
}

/// One document fits more rigs: what a rig lacks is skipped.
#[test]
fn a_bone_the_rig_doesnt_have_is_skipped() {
    let (skeleton, rig) = figure();
    let long =
        document("keys: [(tick: 0, pose: { Head: (x: 10), LeftThumbProximal: (z: 5), Spine: (x: 1) })]");
    let sampled = sample(&long, "test", &skeleton, &rig).expect("it samples");
    let joints: Vec<u16> = sampled.rotations.iter().map(|(joint, _)| *joint).collect();
    assert_eq!(joints, [SPINE as u16]);
}

#[test]
fn a_rig_with_no_hips_isnt_humanoid() {
    let (skeleton, mut rig) = figure();
    rig.humanoid.clear();
    let error = sample(&document("keys: [(tick: 0)]"), "test", &skeleton, &rig).unwrap_err();
    assert!(matches!(error, KeyPoseError::NotHumanoid), "{error}");
}

/// Malformed documents, each with what its error must say.
#[test]
fn bad_documents_say_whats_wrong() {
    let (skeleton, rig) = figure();
    let cases = [
        ("no_keys", "keys: []", "no keys"),
        ("first_past_zero", "keys: [(tick: 3)]", "key 0 is at tick 3"),
        ("out_of_order", "keys: [(tick: 0), (tick: 8), (tick: 4)]", "key 2 is at tick 4"),
        ("same_tick", "keys: [(tick: 0), (tick: 8), (tick: 8)]", "key 2 is at tick 8"),
        ("too_short", "ticks: 5, keys: [(tick: 0), (tick: 8)]", "ends before its last key, at 8"),
        ("too_long", "ticks: 65535, keys: [(tick: 0)]", "over the limit"),
        ("a_nan_turn", "keys: [(tick: 0, pose: { Spine: (x: NaN) })]", "spine isn't finite"),
        ("an_infinite_root", "keys: [(tick: 0), (tick: 1, root: (0.0, inf, 0.0))]", "root isn't finite"),
        ("a_nan_hips", "keys: [(tick: 0, hips: (NaN, 0.0, 0.0))]", "hips isn't finite"),
        ("a_root_away", "keys: [(tick: 0, root: (1.0, 0.0, 0.0))]", "root motion starts at the identity"),
        ("a_nan_root_turn", "keys: [(tick: 0), (tick: 1, root_turn: NaN)]", "root_turn isn't finite"),
        ("a_turn_away", "keys: [(tick: 0, root_turn: 10.0)]", "root motion starts at the identity"),
        ("a_nan_weight", "keys: [(tick: 0, morphs: { \"blink\": NaN })]", "blink isn't finite"),
        ("a_huge_share", "keys: [(tick: 0), (tick: 1, ease: Overshoot(9.0))]", "ease is out of range"),
        ("a_negative_share", "keys: [(tick: 0), (tick: 1, ease: Anticipate(-0.1))]", "ease is out of range"),
        ("a_nan_share", "keys: [(tick: 0), (tick: 1, ease: Anticipate(NaN))]", "ease is out of range"),
        (
            "handles_past_time",
            "keys: [(tick: 0), (tick: 1, ease: Curve(1.5, 0.0, 0.5, 1.0))]",
            "ease is out of range",
        ),
        (
            "handles_nan",
            "keys: [(tick: 0), (tick: 1, ease: Curve(0.5, NaN, 0.5, 1.0))]",
            "ease is out of range",
        ),
    ];
    let mut wrong = Vec::new();
    for (name, data, says) in cases {
        let text = format!("(type: \"sapphire::pipeline::KeyPoseClip\", version: 1, data: ({data}))");
        let result = read(text.as_bytes()).and_then(|document| sample(&document, "test", &skeleton, &rig));
        match result {
            Ok(_) => wrong.push(format!("{name}: it sampled")),
            Err(error) if !error.to_string().contains(says) => {
                wrong.push(format!("{name}: said {error:?}, which doesn't mention {says:?}"));
            }
            Err(_) => {}
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// What a typo gets: the document's reader, naming what it couldn't read.
#[test]
fn unknown_fields_and_bones_are_refused() {
    let read_data = |data: &str| {
        let text = format!("(type: \"sapphire::pipeline::KeyPoseClip\", version: 1, data: ({data}))");
        read(text.as_bytes())
    };
    assert!(matches!(read_data("keys: [(tick: 0, posee: {})]"), Err(KeyPoseError::Document(_))));
    assert!(matches!(read_data("keys: [(tick: 0, pose: { Tail: (x: 1) })]"), Err(KeyPoseError::Document(_))));
    assert!(matches!(
        read_data("keys: [(tick: 0, pose: { Spine: (w: 1) })]"),
        Err(KeyPoseError::Document(_))
    ));
    assert!(matches!(read_data("keys: [(tick: 0, ease: Bouncy)]"), Err(KeyPoseError::Document(_))));
    assert!(matches!(read(&[0xff, 0xfe]), Err(KeyPoseError::Utf8(_))));
    let wrong_type = "(type: \"sapphire::pipeline::Other\", version: 1, data: (keys: []))";
    assert!(matches!(read(wrong_type.as_bytes()), Err(KeyPoseError::Document(_))));
}

#[test]
fn names_and_paths_come_from_the_compound_extension() {
    assert!(is_source(Path::new("moves/Swing.KeyPose.RON")));
    assert!(!is_source(Path::new("moves/swing.ron")));
    assert_eq!(stem(Path::new("moves/swing.keypose.ron")), "swing");
    assert_eq!(output_path(Path::new("moves/swing.keypose.ron"), "sclip"), Path::new("moves/swing.sclip"));
}

/// The document's first key is `sapphire_stage`'s held pose, as `lookdev::turns` has it.
const HELD: &[(HumanBone, Turn)] = &[
    (HumanBone::LeftUpperArm, Turn { x: 0.0, y: -4.0, z: -76.0 }),
    (HumanBone::RightUpperArm, Turn { x: 0.0, y: 4.0, z: 76.0 }),
    (HumanBone::LeftLowerArm, Turn { x: 0.0, y: -10.0, z: 0.0 }),
    (HumanBone::RightLowerArm, Turn { x: 0.0, y: 10.0, z: 0.0 }),
    (HumanBone::LeftHand, Turn { x: 10.0, y: 0.0, z: 0.0 }),
    (HumanBone::RightHand, Turn { x: 10.0, y: 0.0, z: 0.0 }),
    (HumanBone::Head, Turn { x: 4.0, y: -10.0, z: 0.0 }),
];

/// The committed document, on the character it is for: it reads, cooks small, starts at the held
/// pose, and has a strike that overshoots its mark.
#[test]
fn the_committed_document_poses_the_character() {
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let text = fs::read(content.join("lookdev/character/strike.keypose.ron")).expect("the document");
    let document = read(&text).expect("it reads");
    let vrm_bytes = fs::read(content.join("lookdev/character/character.vrm")).expect("the character");
    let rig = vrm::import_rig(&vrm_bytes, None).expect("the rig");
    let imported = vrm::import(&vrm_bytes, None).expect("the character");
    let skeleton = Skeleton::new(&imported.mesh.skin.as_ref().expect("skinned").joints);
    let cooked = Cooked::new(&document, &skeleton, &rig);
    let clip = Clip::read(&cooked.clip).unwrap();
    assert_eq!(clip.ticks.to_native(), document.ticks.expect("it says its length"));
    assert!(cooked.clip.len() < 8 * 1024, "{} bytes", cooked.clip.len());
    assert!(clip.root.is_some(), "it travels");

    // At tick 0, each joint where `pose::pose` puts it before constraints: `parentᵀ · turn · parent`
    // on the joint's own rotation, over rest model rotations.
    let mut rest_world: Vec<Quat> = Vec::with_capacity(skeleton.rest.len());
    let parent_of = |joint: usize| {
        let parent = skeleton.parents[joint];
        (parent != NO_PARENT).then_some(usize::from(parent))
    };
    for joint in 0..skeleton.rest.len() {
        let local = Quat::from_array(skeleton.rest[joint].rotation).normalize();
        rest_world.push(parent_of(joint).map_or(local, |parent| rest_world[parent] * local));
    }
    let mut local: Vec<Quat> =
        skeleton.rest.iter().map(|rest| Quat::from_array(rest.rotation).normalize()).collect();
    for &(bone, turn) in HELD {
        let joint = usize::from(rig.human_joint(bone).expect("the character has it"));
        let parent = parent_of(joint).map_or(Quat::IDENTITY, |parent| rest_world[parent]);
        local[joint] = (parent.inverse() * turn.quat() * parent * local[joint]).normalize();
    }
    let mut held: Vec<Affine3A> = Vec::with_capacity(local.len());
    for (joint, rotation) in local.iter().enumerate() {
        let rest = &skeleton.rest[joint];
        let transform = Affine3A::from_scale_rotation_translation(
            Vec3::from(rest.scale),
            *rotation,
            Vec3::from(rest.translation),
        );
        held.push(parent_of(joint).map_or(transform, |parent| held[parent] * transform));
    }
    let start = cooked.model(0.0);
    for (joint, (cooked, held)) in start.iter().zip(&held).enumerate() {
        let strays = Vec3::from(cooked.translation).distance(Vec3::from(held.translation));
        assert!(strays < 2e-3, "{}: {strays} m from the held pose", skeleton.names[joint]);
    }

    // The right hand swings through the strike: it pulls back, then passes its mark and returns.
    let hand = usize::from(rig.human_joint(HumanBone::RightHand).expect("the character has it"));
    let forward = |tick: f32| cooked.position(hand, tick).z;
    let ticks: Vec<f32> = document.keys.iter().map(|key| f32::from(key.tick)).collect();
    let [_, windup, strike, ..] = ticks[..] else { panic!("{ticks:?}") };
    assert!(forward(windup) < forward(0.0), "it pulls back: {} from {}", forward(windup), forward(0.0));
    assert!(forward(strike) > forward(0.0), "and strikes: {} from {}", forward(strike), forward(0.0));
    let reach = (windup as u16..=strike as u16).map(|tick| forward(f32::from(tick))).fold(f32::MIN, f32::max);
    assert!(reach > forward(strike) + 0.01, "past the mark: {reach} against {}", forward(strike));
}

/// A committed document of the character's, as it would cook.
fn on_the_character(name: &str) -> (KeyPoseClip, Cooked, Skeleton, CharacterRig) {
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    cook_on_the_character(&content, &content.join(format!("lookdev/character/{name}.keypose.ron")))
}

fn cook_on_the_character(content: &Path, path: &Path) -> (KeyPoseClip, Cooked, Skeleton, CharacterRig) {
    let text = fs::read(path).expect("the document");
    let document = read(&text).expect("it reads");
    let vrm_bytes = fs::read(content.join("lookdev/character/character.vrm")).expect("the character");
    let rig = vrm::import_rig(&vrm_bytes, None).expect("the rig");
    let imported = vrm::import(&vrm_bytes, None).expect("the character");
    let skeleton = Skeleton::new(&imported.mesh.skin.as_ref().expect("skinned").joints);
    let cooked = Cooked::new(&document, &skeleton, &rig);
    (document, cooked, skeleton, rig)
}

/// Standing pays nothing for posed bounds: the documents locomotion plays keep the skin inside the
/// sphere the bind pose's bounds make (`LocomotionBounds`).
#[test]
fn locomotions_documents_stay_inside_the_characters_bounds() {
    let gate = LocomotionBounds::of_the_character();
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let mut tree = meta::SourceTree::scan(&content).expect("the content tree");
    tree.sources.retain(|path| path.to_string_lossy().ends_with(".keypose.ron"));
    tree.sidecars.retain(|path| path.to_string_lossy().ends_with(".keypose.ron.meta"));
    let sources = meta::resolve(&tree, true).expect("all keypose sources have unique asset IDs");
    let mut named = 0;
    for clip in LocomotionBounds::clips().iter().filter(|clip| clip.clip.is_none()) {
        let source = sources
            .assets
            .iter()
            .find(|asset| asset.meta.id == clip.source)
            .unwrap_or_else(|| panic!("missing document for {} ({})", clip.name, clip.source));
        let (_, cooked, _, _) = cook_on_the_character(&content, &source.path);
        let ticks = Clip::read(&cooked.clip).expect("a valid clip").ticks.to_native();
        let reach =
            (0..=ticks).step_by(2).map(|tick| gate.reach(&cooked.model(tick as f32))).fold(0.0, f32::max);
        assert!(reach <= gate.radius, "{}: the skin reaches {reach} of {}", clip.name, gate.radius);
        named += 1;
    }
    assert!(named >= 1, "{named}");
}

/// What posed bounds are for: a death's whole body leaves the sphere (the renderer's
/// `a_body_lying_down_is_held_and_it_grows_from_the_cooked_bounds_with_no_jump`).
#[test]
fn a_death_leaves_the_characters_bounds() {
    let gate = LocomotionBounds::of_the_character();
    let (_, cooked, _, _) = on_the_character("death_back");
    let reach = (0..=80).step_by(4).map(|tick| gate.reach(&cooked.model(tick as f32))).fold(0.0, f32::max);
    assert!(reach > gate.radius, "the skin reaches {reach} of {}", gate.radius);
}

/// A sole's height above the floor, as the lower of the ankle and the toes against their rests.
fn sole_height(cooked: &Cooked, rig: &CharacterRig, position: f32, foot: HumanBone, toes: HumanBone) -> f32 {
    let model = cooked.model(position);
    let height = |bone: HumanBone, rest: f32| {
        model[usize::from(rig.human_joint(bone).expect("the character has it"))].translation.y - rest
    };
    height(foot, ANKLE_REST).min(height(toes, TOES_REST))
}

/// The character's ankle and toes at rest, in meters above the floor.
const ANKLE_REST: f32 = 0.1007;
const TOES_REST: f32 = 0.0376;

/// The idles: they loop on their own first pose, and their feet stay down and still, since nothing
/// plants them at run time. The bounds are the measured value and a little.
#[test]
fn the_idles_loop_with_their_feet_down() {
    use HumanBone::{LeftFoot, LeftToes, RightFoot, RightToes};
    for (name, lift, slide) in [("idle", 0.012, 0.012), ("idle_combat", 0.03, 0.012)] {
        let (document, cooked, _, rig) = on_the_character(name);
        let ticks = f32::from(document.ticks.expect("it says its length"));
        let (start, end) = (cooked.model(0.0), cooked.model(ticks));
        for (joint, (start, end)) in start.iter().zip(&end).enumerate() {
            let apart = Vec3::from(start.translation).distance(Vec3::from(end.translation));
            assert!(apart < 2e-3, "{name}: joint {joint} ends {apart} m from where it starts");
        }
        let joint = |bone| usize::from(rig.human_joint(bone).expect("the character has it"));
        let (first_left, first_right) =
            (cooked.position(joint(LeftFoot), 0.0), cooked.position(joint(RightFoot), 0.0));
        let (mut highest, mut lowest, mut moved) = (f32::MIN, f32::MAX, 0.0_f32);
        for tick in 0..=u16::try_from(ticks as u32).unwrap() {
            let at = f32::from(tick);
            for (foot, toes, first) in [(LeftFoot, LeftToes, first_left), (RightFoot, RightToes, first_right)]
            {
                let sole = sole_height(&cooked, &rig, at, foot, toes);
                highest = highest.max(sole);
                lowest = lowest.min(sole);
                let now = cooked.position(joint(foot), at);
                moved = moved.max(Vec3::new(now.x - first.x, 0.0, now.z - first.z).length());
            }
        }
        assert!(highest < lift && lowest > -0.006, "{name}: soles {lowest} to {highest} m");
        assert!(moved < slide, "{name}: a foot moved {moved} m");
    }
}

/// The deaths end lying on the floor: the hips within a body's depth of it, and nothing under it.
/// KayKit's, retargeted, lie 0.34 to 0.48 m up, which is why these exist.
#[test]
fn the_deaths_end_lying_on_the_floor() {
    for name in ["death_back", "death_forward"] {
        let (document, cooked, skeleton, rig) = on_the_character(name);
        let end = f32::from(document.ticks.expect("it says its length"));
        let model = cooked.model(end);
        let hips = usize::from(rig.human_joint(HumanBone::Hips).expect("the character has it"));
        assert!(model[hips].translation.y < 0.15, "{name}: hips at {}", model[hips].translation.y);
        // Every joint of the body proper; hair and other helpers bend with their springs at run time.
        let (mut lowest, mut low_bone) = (f32::MAX, None);
        for entry in rig.humanoid.iter() {
            let height = model[usize::from(entry.joint)].translation.y;
            if height < lowest {
                (lowest, low_bone) = (height, Some(entry.bone));
            }
        }
        assert!(lowest > 0.0, "{name}: {low_bone:?} is under the floor, at {lowest}");
        let highest = rig.humanoid.iter().map(|entry| model[usize::from(entry.joint)].translation.y);
        let highest = highest.fold(f32::MIN, f32::max);
        assert!(highest < 0.42, "{name}: something still stands {highest} m up");
        assert_eq!(skeleton.parents.len(), model.len());
        // It starts from the held pose, as every move does.
        let start = cooked.model(0.0);
        let hand = usize::from(rig.human_joint(HumanBone::LeftHand).expect("the character has it"));
        assert!(start[hand].translation.y > 0.7, "{name}: it starts on its feet");
    }
}

#[test]
fn prepared_timeline_matches_public_easing_bits() {
    for ease in [
        KeyEase::Linear,
        KeyEase::Smooth,
        KeyEase::Hold,
        KeyEase::In,
        KeyEase::Out,
        KeyEase::Anticipate(0.0),
        KeyEase::Anticipate(MAX_BACK),
        KeyEase::Overshoot(0.25),
        KeyEase::Overshoot(MAX_BACK),
        KeyEase::Curve(0.2, -0.3, 0.8, 1.2),
    ] {
        for end in [1, 17, 3600] {
            let key = |tick, ease| PoseKey {
                tick,
                ease,
                root: None,
                root_turn: None,
                hips: None,
                pose: BTreeMap::new(),
                morphs: BTreeMap::new(),
            };
            let keys = [key(0, KeyEase::Linear), key(end, ease), key(end + 2, KeyEase::Overshoot(0.1))];
            for (tick, moment) in timeline(&keys, end + 4).into_iter().enumerate() {
                let expected = if tick < usize::from(end) {
                    ease.weight(tick as f32 / f32::from(end))
                } else if tick < usize::from(end + 2) {
                    keys[2].ease.weight((tick as f32 - f32::from(end)) / 2.0)
                } else {
                    0.0
                };
                assert_eq!(moment.weight.to_bits(), expected.to_bits(), "{ease:?} tick {tick}");
            }
        }
    }
}

/// The graph show's simulation root and its authored strike must travel together.
/// MotionDocument interpolates linearly, so nonlinear clip intervals need tick samples.
#[test]
fn graph_show_strike_root_follows_the_key_pose() {
    let document = read(include_bytes!("../../../../content/lookdev/character/strike.keypose.ron"))
        .expect("the authored strike");
    let (skeleton, rig) = figure();
    let sampled = sample(&document, "strike", &skeleton, &rig).expect("the strike samples");
    let root = sampled.root.as_ref().expect("the strike travels");
    let show = sapphire_moves::MoveSetDocument::parse(include_str!(
        "../../../../content/lookdev/character/show.moves.ron"
    ))
    .expect("the live-editable show document")
    .compile()
    .expect("the show compiles without loading or baking a clip");
    let strike = show.find(sapphire_core::Name::new("strike")).expect("the strike").1;
    assert_eq!(root.translation.len(), 61, "the authored 60-tick fixture");
    assert_eq!(strike.tables.root_samples().len(), root.translation.len());
    let target =
        ClipTarget { skeleton: Some((AssetId::from_bytes([1; 16]), &skeleton)), morph_defaults: &[] };
    let bytes = cook(&sampled, &target, &ReductionError::default()).to_cooked().expect("the runtime clip");
    let clip = Clip::read(&bytes).expect("the runtime clip reads");
    // Keep regeneration usable from a normal checkout, without an external authoring tool.
    // Every mismatched coordinate is a copyable per-tick MotionDocument root key.
    let mut drift = Vec::new();
    for (tick, (expected, actual)) in root.translation.iter().zip(strike.tables.root_samples()).enumerate() {
        let offset = actual.offset();
        // Decimal serialization/pointwise arithmetic only: one micrometre.
        if !expected.is_finite() || !offset.is_finite() || expected.distance(offset) >= 1e-6 {
            drift.push(format!("(at: {tick}, offset: ({}, {}, {})),", expected.x, expected.y, expected.z));
        }
    }
    assert!(
        drift.is_empty(),
        "show.moves.ron's strike root drifted from strike.keypose.ron. Insert/replace these sampled root keys, then rerun graph_show_strike_root_follows_the_key_pose:\n{}",
        drift.join("\n")
    );
    for (tick, actual) in strike.tables.root_samples().iter().enumerate() {
        let drawn = sapphire_anim::root_motion(clip, tick as f32).expect("the drawn root");
        // The shipped default reduction permits 0.25 mm, plus packed-vector rounding.
        assert!(
            drawn.translation.distance(actual.offset()) < 0.000_26,
            "tick {tick}: show {:?}, runtime {:?}",
            actual.offset(),
            drawn.translation
        );
        assert_eq!(actual.yaw, 0.0);
        assert!(drawn.rotation.angle_between(Quat::IDENTITY) < 0.009f32.to_radians());
    }
}
