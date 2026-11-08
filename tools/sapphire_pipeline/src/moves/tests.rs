//! Move sets through a cook, and tables baked from clips: what a table holds, the bytes it holds it in
//! on every architecture, and how near it comes to the pose on screen. A clip's move through a whole
//! cook and into the simulation is `cook::move_tests`.

use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, PI};
use std::fs;

use sapphire_anim::{CharacterPose, root_motion};
use sapphire_assets::clip::Clip;
use sapphire_assets::rig::{ConstraintKind, HumanBone};
use sapphire_assets::skeleton::{Joint, JointTransform};
use sapphire_assets::{AssetId, AssetKind};
use sapphire_math::Transform;
use sapphire_moves::{MOVE_SET_EXTENSION, MoveSet};
use sapphire_serialize::cooked::AlignedVec;

use super::*;
use crate::clip::retarget::tests::{
    Kaykit, as_cooked, character, character_rig, sidecar_options, third_party, warrior,
};
use crate::clip::retarget::{Rig, retarget};
use crate::clip::{ClipTarget, ReductionError, SampledRoot};
use crate::cook::{Cook, CookError, CookOptions, CookStatus, cook_dir};
use crate::keypose;

const SLICE: &str = include_str!("../../../../crates/sapphire_moves/test_content/slice.moves.ron");

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sapphire_moves_cook_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn aligned(bytes: &[u8]) -> AlignedVec {
    let mut aligned = AlignedVec::new();
    aligned.extend_from_slice(bytes);
    aligned
}

#[test]
fn sources_are_known_by_their_suffix() {
    assert!(is_source(Path::new("content/fighter.moves.ron")));
    assert!(is_source(Path::new("FIGHTER.MOVES.RON")));
    assert!(!is_source(Path::new("fighter.ron")));
    assert!(!is_source(Path::new("fighter.material.ron")));
    assert_eq!(
        output_path(Path::new("characters/fighter.moves.ron"), MOVE_SET_EXTENSION),
        Path::new("characters/fighter.smoves")
    );
}

/// A fighter's `movement` goes through a cook, and a number past its limit stops one, saying what.
#[test]
fn a_cook_takes_a_fighters_movement_and_refuses_a_number_past_its_limit() {
    let cook = |name: &str, movement: &str| {
        let root = temp_dir(name);
        let (content, out) = (root.join("content"), root.join("out"));
        fs::create_dir_all(&content).unwrap();
        let text = SLICE.replacen("fighter: (", &format!("fighter: (\nmovement: ({movement}),"), 1);
        fs::write(content.join("fighter.moves.ron"), text).unwrap();
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false };
        let result = cook_dir(&content, &out, &options);
        let bytes = fs::read(out.join("fighter.smoves")).ok();
        let _ = fs::remove_dir_all(&root);
        (result.map(|_| ()), bytes)
    };
    let (cooked, bytes) = cook("movement", "run_speed: 7.5");
    cooked.expect("a good document");
    let set = MoveSet::from_cooked(&aligned(&bytes.expect("its output"))).unwrap();
    assert_eq!(set.movement().unwrap().run_speed, 7.5);

    let (refused, _) = cook("movement_bad", "run_speed: 600.0");
    let said = format!("{:?}", refused.expect_err("past its limit"));
    assert!(said.contains("run_speed") && said.contains("600"), "{said}");
    assert!(said.contains("fighter.moves.ron"), "it names the file: {said}");
}

/// Through the whole cook: a sidecar, an output by the source's name, the cache, and bytes the
/// simulation reads, identical to compiling the document directly.
#[test]
fn a_document_cooks_through_a_content_tree_and_its_cache() {
    let root = temp_dir("tree");
    let (content, out, cache) = (root.join("content"), root.join("out"), root.join("cache"));
    fs::create_dir_all(content.join("fighter")).unwrap();
    fs::write(content.join("fighter/slice.moves.ron"), SLICE).unwrap();
    let options = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked: false };

    let report = cook_dir(&content, &out, &options).unwrap();
    assert_eq!(report.assets.len(), 1);
    assert_eq!(report.assets[0].status, CookStatus::Cooked);
    assert_eq!(report.created_sidecars.len(), 1);
    let sidecar = fs::read_to_string(&report.created_sidecars[0]).unwrap();
    assert!(sidecar.contains("MoveSet"), "{sidecar}");
    let output = out.join("fighter/slice.smoves");
    assert_eq!(report.assets[0].output, output);

    let cooked = aligned(&fs::read(&output).unwrap());
    let set = MoveSet::from_cooked(&cooked).unwrap();
    let direct = MoveSetDocument::parse(SLICE).unwrap().compile().unwrap();
    assert_eq!(set, direct);
    assert_eq!(*cooked, *direct.to_cooked().unwrap());
    assert_eq!(AssetKind::MoveSet.references(&cooked).unwrap(), []);

    // Nothing changed: the output stands. A locked run finds the sidecar it needs.
    let locked = CookOptions { locked: true, ..options };
    let again = cook_dir(&content, &out, &locked).unwrap();
    assert_eq!(again.assets[0].status, CookStatus::UpToDate);

    // An edit cooks again, and a stale output goes with its source.
    let edited = SLICE.replacen("startup: 5", "startup: 6", 1);
    fs::write(content.join("fighter/slice.moves.ron"), edited).unwrap();
    let cooked_again = cook_dir(&content, &out, &locked).unwrap();
    assert_eq!(cooked_again.assets[0].status, CookStatus::Cooked);
    let cooked = aligned(&fs::read(&output).unwrap());
    assert_eq!(MoveSet::from_cooked(&cooked).unwrap().moves()[0].startup, 6);

    fs::remove_dir_all(root).unwrap();
}

/// The cook's error names the file, and the document's own errors name the move and track.
#[test]
fn a_bad_document_fails_the_cook_by_name() {
    let root = temp_dir("bad");
    let (content, out) = (root.join("content"), root.join("out"));
    fs::create_dir_all(&content).unwrap();
    let bad = SLICE.replacen("from: 10, to: 24", "from: 10, to: 40", 1);
    fs::write(content.join("bad.moves.ron"), bad).unwrap();
    let options = CookOptions { cook: Cook::Mobile, cache: None, locked: false };
    let error = cook_dir(&content, &out, &options).unwrap_err();
    assert!(matches!(error, CookError::MoveSet { .. }), "{error:?}");
    assert!(error.to_string().contains("bad.moves.ron"), "{error}");
    let source = std::error::Error::source(&error).unwrap().to_string();
    assert!(source.contains("light_1") && source.contains("cancels"), "{source}");
    assert!(source.contains("10..40"), "{source}");

    fs::write(content.join("bad.moves.ron"), "( this isn't RON").unwrap();
    let error = cook_dir(&content, &out, &options).unwrap_err();
    assert!(matches!(error, CookError::MoveSet { .. }), "{error:?}");

    fs::write(content.join("bad.moves.ron"), [0xff_u8, 0xfe]).unwrap();
    let error = cook_dir(&content, &out, &options).unwrap_err();
    assert!(matches!(error, CookError::MoveSet { .. }), "{error:?}");

    fs::remove_dir_all(root).unwrap();
}

/// Packs hold a move set like any other asset.
#[test]
fn a_cooked_set_packs() {
    let root = temp_dir("pack");
    let (content, out, cache) = (root.join("content"), root.join("out"), root.join("cache"));
    fs::create_dir_all(&content).unwrap();
    fs::write(content.join("slice.moves.ron"), SLICE).unwrap();
    let options = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked: false };
    let report = cook_dir(&content, &out, &options).unwrap();
    assert_eq!(crate::pack::kind(&report.assets[0].output), Some(AssetKind::MoveSet));
    assert_eq!(crate::pack::codec(AssetKind::MoveSet), sapphire_assets::pack::Codec::Lz4);
    fs::remove_dir_all(root).unwrap();
}

const HIPS: u16 = 0;
const ARM: u16 = 1;
const HAND: u16 = 2;

fn joint(name: &str, parent: Option<u16>, translation: [f32; 3]) -> Joint {
    Joint { name: name.to_owned(), parent, rest: JointTransform { translation, ..JointTransform::IDENTITY } }
}

/// Hips a metre up, an arm off them to +X and a hand at its end, at rest along +X.
fn figure() -> Skeleton {
    Skeleton::new(&[
        joint("hips", None, [0.0, 1.0, 0.0]),
        joint("arm", Some(HIPS), [0.5, 0.25, 0.0]),
        joint("hand", Some(ARM), [0.25, 0.0, 0.0]),
    ])
}

fn still(ticks: u16) -> SampledClip {
    SampledClip { name: "test".to_owned(), rate: DEFAULT_RATE, ticks, ..SampledClip::default() }
}

fn path(name: &str, follows: Follows) -> ClipPathDocument {
    ClipPathDocument { name: name.to_owned(), follows, offset: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0] }
}

fn motion(paths: Vec<ClipPathDocument>) -> ClipMotionDocument {
    ClipMotionDocument { source: String::new(), name: None, rig: None, root: RootSource::Clip, paths }
}

fn plain<'a>(sampled: &'a SampledClip, skeleton: &'a Skeleton) -> TableInput<'a> {
    TableInput { sampled, skeleton, map: None, constraints: &[] }
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    Vec3::from(a).abs_diff_eq(Vec3::from(b), 1e-5)
}

/// A joint's transform in the clip's pose, with the offset along its own axes: at tick 0 at rest, and
/// by tick 2 with the hips up half a metre and the arm a quarter turn up, which carries the hand along.
#[test]
fn a_path_is_its_joint_in_the_pose_with_its_offset() {
    let skeleton = figure();
    let quarter = det::quat_from_rotation_z(FRAC_PI_2);
    let mut sampled = still(2);
    sampled.rotations = vec![(ARM, vec![Quat::IDENTITY, Quat::IDENTITY, quarter])];
    let heights = [1.0, 1.0, 1.5].map(|y| Vec3::new(0.0, y, 0.0));
    sampled.translations = vec![(HIPS, heights.to_vec())];
    let tip = ClipPathDocument { offset: [0.1, 0.0, 0.0], ..path("tip", Follows::Joint("hand".to_owned())) };
    let tables = bake(&motion(vec![tip]), &plain(&sampled, &skeleton)).unwrap();
    assert_eq!(tables.ticks(), 2);
    let samples = tables.path(Name::new("tip")).expect("named").samples();
    assert_eq!(samples.len(), 3, "one per tick, and the last");
    assert!(near(samples[0].translation, [0.85, 1.25, 0.0]), "{:?}", samples[0]);
    assert_eq!(samples[0].rotation, [0.0, 0.0, 0.0, 1.0]);
    assert!(near(samples[2].translation, [0.5, 2.1, 0.0]), "{:?}", samples[2]);
    assert!(samples[2].orientation().abs_diff_eq(quarter, 1e-6), "{:?}", samples[2]);
}

/// A humanoid bone reads the rig's map, so one document fits any rig that has one.
#[test]
fn a_bone_is_whichever_joint_the_map_says() {
    let skeleton = figure();
    let sampled = still(1);
    let map = HumanMap::from_joints(BTreeMap::from([(HumanBone::RightHand, HAND)]));
    let input = TableInput { map: Some(&map), ..plain(&sampled, &skeleton) };
    let tables = bake(&motion(vec![path("fist", Follows::Bone("RightHand".to_owned()))]), &input).unwrap();
    assert!(near(tables.path(Name::new("fist")).unwrap().samples()[0].translation, [0.75, 1.25, 0.0]));
}

/// A rotation's rotation and a metre's metre, whatever scale a parent has: an armature scaled a
/// hundredth moves its joints as its centimetres say, and a socket's offset stays in metres.
#[test]
fn scale_above_a_joint_moves_it_and_stays_out_of_its_rotation_and_offset() {
    let mut joints =
        vec![joint("armature", None, [0.0, 0.0, 0.0]), joint("arm", Some(0), [50.0, 100.0, 0.0])];
    joints[0].rest.scale = [0.01; 3];
    let skeleton = Skeleton::new(&joints);
    let turn = det::quat_from_rotation_y(FRAC_PI_2);
    let mut sampled = still(1);
    sampled.rotations = vec![(1, vec![turn, turn])];
    let tip = ClipPathDocument { offset: [0.0, 0.0, 0.9], ..path("tip", Follows::Joint("arm".to_owned())) };
    let tables = bake(&motion(vec![tip]), &plain(&sampled, &skeleton)).unwrap();
    let sample = tables.path(Name::new("tip")).unwrap().samples()[0];
    assert!(near(sample.translation, [1.4, 1.0, 0.0]), "{sample:?}");
    assert!(sample.orientation().abs_diff_eq(turn, 1e-6), "{sample:?}");
}

/// Sample 0 is exactly zero, even for a root lifted from a bone, which starts within rounding of it;
/// translations are in the root's space at the first tick; and a spin of more than a turn unwraps.
#[test]
fn root_motion_starts_at_exactly_zero_and_its_yaw_unwraps() {
    let skeleton = figure();
    let mut sampled = still(20);
    let start = Vec3::new(1e-6, 0.0, -2e-6);
    let start_yaw = 1e-6;
    sampled.root = Some(SampledRoot {
        joint: HIPS,
        translation: (0..=20)
            .map(|tick| start + det::rotate_y(Vec3::new(0.0, 0.0, tick as f32 * 0.1), start_yaw))
            .collect(),
        rotation: (0..=20).map(|tick| det::quat_from_rotation_y(start_yaw + tick as f32 * 0.5)).collect(),
    });
    let tables = bake(&motion(Vec::new()), &plain(&sampled, &skeleton)).unwrap();
    let root = tables.root_samples();
    assert_eq!(root.len(), 21);
    assert_eq!(root[0].translation.map(f32::to_bits), [0; 3], "{:?}", root[0]);
    assert_eq!(root[0].yaw.to_bits(), 0, "{:?}", root[0]);
    for (tick, sample) in root.iter().enumerate() {
        let t = tick as f32;
        assert!((sample.yaw - t * 0.5).abs() < 1e-4, "tick {tick}: {sample:?}");
        assert!(near(sample.translation, [0.0, 0.0, t * 0.1]), "tick {tick}: {sample:?}");
    }
    assert!(root[20].yaw > 3.0 * PI, "past a turn and a half");

    // In place, whatever the track says, and no track is no motion.
    let in_place = ClipMotionDocument { root: RootSource::InPlace, ..motion(Vec::new()) };
    let tables = bake(&in_place, &plain(&sampled, &skeleton)).unwrap();
    assert_eq!(tables.root_samples(), [RootSample::ZERO; 21]);
    let tables = bake(&motion(Vec::new()), &plain(&still(20), &skeleton)).unwrap();
    assert_eq!(tables.root_samples(), [RootSample::ZERO; 21]);
}

/// `-0.0` and `0.0` are one number and different bytes, and NEON and SSE2 can reach either: a table
/// writes `0.0`, so every zero hashes alike.
#[test]
fn a_tables_zeros_have_one_sign() {
    let skeleton = Skeleton::new(&[joint("hips", None, [0.0, -0.0, 0.0])]);
    let mut sampled = still(1);
    sampled.rotations = vec![(0, vec![Quat::from_xyzw(-0.0, 0.0, -0.0, 1.0); 2])];
    sampled.root = Some(SampledRoot {
        joint: 0,
        translation: vec![Vec3::new(-0.0, 0.0, -0.0); 2],
        rotation: vec![Quat::from_xyzw(0.0, -0.0, 0.0, 1.0); 2],
    });
    let tables =
        bake(&motion(vec![path("hips", Follows::Joint("hips".to_owned()))]), &plain(&sampled, &skeleton));
    let tables = tables.unwrap();
    let sample = tables.path(Name::new("hips")).unwrap().samples()[1];
    assert_eq!(sample.translation.map(f32::to_bits), [0; 3], "{sample:?}");
    assert_eq!(sample.rotation[..3].iter().map(|c| c.to_bits()).collect::<Vec<_>>(), [0; 3], "{sample:?}");
    let root = tables.root_samples()[1];
    assert_eq!(root.translation.map(f32::to_bits), [0; 3], "{root:?}");
    assert_eq!(root.yaw.to_bits(), 0, "{root:?}");
}

/// What NEON and SSE2 make of `Quat::normalize`, written out in scalars.
fn neon_normalize(q: [f32; 4]) -> [f32; 4] {
    let [x, y, z, w] = q;
    let recip = 1.0 / ((x * x + y * y) + (z * z + w * w)).sqrt();
    q.map(|component| component * recip)
}

fn sse2_normalize(q: [f32; 4]) -> [f32; 4] {
    let [x, y, z, w] = q;
    let length = ((x * x + z * z) + (y * y + w * w)).sqrt();
    q.map(|component| component / length)
}

/// One sample of a table's path, in its exact bits: a rest, a track and a socket rotation that aren't
/// unit, all four components of each non-zero, where a sum's order and a division shows. A yaw or an
/// axis turn has zeros that hide them, which is how G2's first hash let a mutation past.
///
/// The bits were worked out apart from this code, by double rounding in Python, which IEEE single
/// precision's operations survive exactly, so they are any IEEE machine's. And the row tells the
/// backends apart: normalizing the product by NEON's or SSE2's arithmetic gives neither this nor each
/// other.
#[test]
fn a_samples_bits_are_any_machines() {
    let turned = [0.38, -0.55, 0.2, 0.73];
    let arm_turn = Quat::from_array([0.1, -0.2, 0.3, 0.65]);
    let socket = [-0.3, 0.15, 0.7, 0.65];
    let mut joints = vec![joint("hips", None, [0.5, 1.0, -0.25]), joint("arm", Some(0), [0.25, 0.5, 0.125])];
    joints[0].rest.rotation = turned;
    let skeleton = Skeleton::new(&joints);
    let mut sampled = still(1);
    sampled.rotations = vec![(1, vec![arm_turn; 2])];
    let tip = ClipPathDocument {
        offset: [0.05, -0.1, 0.9],
        rotation: socket,
        ..path("tip", Follows::Joint("arm".to_owned()))
    };
    let tables = bake(&motion(vec![tip]), &plain(&sampled, &skeleton)).unwrap();
    let sample = tables.path(Name::new("tip")).unwrap().samples()[1];

    let bits = |values: [f32; 4]| values.map(f32::to_bits);
    assert_eq!(bits(sample.rotation), [0xbf0c_6fc3, 0xbf42_bcfe, 0x3ea6_c2d7, 0x3df5_0ce5], "{sample:?}");
    assert_eq!(sample.translation.map(f32::to_bits), [0xbdd6_6048, 0x3eb8_d390, 0xbdb1_9f34], "{sample:?}");

    let before = [0xbf0c_6fc2_u32, 0xbf42_bcfd, 0x3ea6_c2d6, 0x3df5_0ce4].map(f32::from_bits);
    assert!(before.iter().all(|component| component.abs() > 0.05));
    assert_ne!(bits(neon_normalize(before)), bits(sample.rotation));
    assert_ne!(bits(sse2_normalize(before)), bits(sample.rotation));
    assert_ne!(bits(neon_normalize(before)), bits(sse2_normalize(before)));
}

/// One row per refusal, each naming what it refuses.
#[test]
fn what_cant_bake_says_why() {
    let skeleton = figure();
    let sampled = still(1);
    let map = HumanMap::from_joints(BTreeMap::from([(HumanBone::RightHand, HAND)]));
    let mapped = TableInput { map: Some(&map), ..plain(&sampled, &skeleton) };
    let bone = |name: &str| motion(vec![path("tip", Follows::Bone(name.to_owned()))]);
    let joint = |name: &str| motion(vec![path("tip", Follows::Joint(name.to_owned()))]);

    let refused = bake(&bone("rightHand"), &mapped).unwrap_err();
    assert!(matches!(refused, BakeError::UnknownBone { .. }), "VRM's spelling isn't a document's: {refused}");
    assert!(refused.to_string().contains("`tip`") && refused.to_string().contains("rightHand"), "{refused}");
    let refused = bake(&bone("Head"), &mapped).unwrap_err();
    assert!(matches!(refused, BakeError::NoBone { .. }), "{refused}");
    let refused = bake(&bone("RightHand"), &plain(&sampled, &skeleton)).unwrap_err();
    assert!(matches!(refused, BakeError::NoMap { .. }), "{refused}");
    let refused = bake(&joint("tail"), &mapped).unwrap_err();
    assert!(
        matches!(refused, BakeError::NoJoint { .. }) && refused.to_string().contains("tail"),
        "{refused}"
    );
    let mut twins = figure();
    twins.names[usize::from(ARM)] = "hand".to_owned();
    let refused = bake(&joint("hand"), &plain(&sampled, &twins)).unwrap_err();
    assert!(
        matches!(refused, BakeError::RepeatedJoint { .. }) && refused.to_string().contains("\"hand\""),
        "two joints share the name, so it says neither: {refused}"
    );
    assert!(bake(&joint("hips"), &plain(&sampled, &twins)).is_ok(), "the name that's still one joint's");

    let mut slow = still(1);
    slow.rate = 30;
    let refused = bake(&motion(Vec::new()), &plain(&slow, &skeleton)).unwrap_err();
    assert!(matches!(refused, BakeError::Rate { rate: 30 }), "{refused}");

    // A constraint on the joint, or anywhere above it, runs after a pose and not in a table.
    for driven in [HAND, ARM, HIPS] {
        let constraints = [Constraint {
            joint: driven,
            source: HIPS,
            weight: 1.0,
            kind: ConstraintKind::Roll { axis: sapphire_assets::rig::Axis::X },
        }];
        let input = TableInput { constraints: &constraints, ..plain(&sampled, &skeleton) };
        let refused = bake(&joint("hand"), &input).unwrap_err();
        assert!(matches!(refused, BakeError::ConstraintDriven { .. }), "{driven}: {refused}");
        assert!(refused.to_string().contains(skeleton.names[usize::from(driven)].as_str()), "{refused}");
    }
    // One beside the chain is none of its business.
    let constraints = [Constraint {
        joint: HAND,
        source: HIPS,
        weight: 1.0,
        kind: ConstraintKind::Roll { axis: sapphire_assets::rig::Axis::X },
    }];
    let input = TableInput { constraints: &constraints, ..plain(&sampled, &skeleton) };
    bake(&joint("arm"), &input).expect("the hand is below it");
}

/// A key-pose document for these tables: every kind of key and ease, on the bones a swing moves and
/// on the legs, so a hips offset, a turn in a bone's parent's frame, a root that travels and turns past
/// half a turn and every ease's arithmetic all reach the table. Written out here rather than read
/// from `content/`, so a look-dev edit doesn't move a pin.
const SIGNATURE: &str = "(
    type: \"sapphire::pipeline::KeyPoseClip\",
    version: 1,
    data: (
        ticks: 72,
        keys: [
            (tick: 0, pose: {
                LeftUpperArm: (y: -4, z: -76),
                RightUpperArm: (y: 4, z: 76),
                LeftLowerArm: (y: -10),
                RightLowerArm: (y: 10),
                RightHand: (x: 10),
                Head: (x: 4, y: -10),
            }),
            (tick: 14, ease: Out, hips: (0, -0.03, -0.02), pose: {
                Spine: (y: -14),
                RightUpperArm: (y: -50, z: 50),
                RightLowerArm: (y: 60),
                LeftUpperLeg: (x: 30),
            }),
            (tick: 18, ease: Anticipate(0.2), root: (0.01, 0, -0.05), pose: {
                Spine: (x: 8, y: -20, z: 3),
                RightLowerLeg: (x: -25),
            }),
            (tick: 24, ease: Overshoot(0.15), root: (0.1, 0, 0.7), root_turn: 200, hips: (0.02, -0.05, 0.03), pose: {
                Spine: (y: 20),
                RightUpperArm: (x: 12, y: 40, z: 62),
                RightLowerArm: (y: 8),
                Head: (x: 4, y: -22),
            }),
            (tick: 30, ease: Curve(0.2, 0.9, 0.3, 1.1), root_turn: 380, pose: {
                RightHand: (x: -20, y: 15, z: 5),
            }),
            (tick: 40, ease: Hold, root: (0.2, 0.1, 0.9), pose: {
                Chest: (x: -7),
            }),
            (tick: 46, ease: In, hips: (0, 0, 0), pose: {
                LeftUpperLeg: (x: 0),
                RightLowerLeg: (x: 0),
            }),
            (tick: 60, ease: Linear, root: (0.2, 0, 1.1), pose: {
                Spine: (y: 0),
                RightUpperArm: (y: 4, z: 76),
                RightLowerArm: (y: 10),
                Head: (x: 4, y: -10),
            }),
        ],
    ),
)";

fn signature() -> SampledClip {
    let (skeleton, map) = character();
    let document = keypose::read(SIGNATURE.as_bytes()).unwrap();
    keypose::sample(&document, "signature", skeleton, map, &[]).unwrap()
}

/// KayKit's dodge, retargeted onto the character: a glTF's slerps, root lifting and retargeting.
fn dodge() -> SampledClip {
    Kaykit::open("MovementAdvanced").on_character("Dodge_Forward").1
}

/// KayKit's two-handed spin, retargeted onto the character: a root that turns past a half turn, so
/// lifting its yaw and unwrapping it are on the path, which a dodge that doesn't turn can't show.
fn spin() -> SampledClip {
    Kaykit::open("CombatMelee").on_character("Melee_2H_Attack_Spin").1
}

/// The character's run as the sidecars cook it: their `adjust`, by bone.
fn run_as_cooked() -> SampledClip {
    as_cooked(&Kaykit::open("MovementBasic"), "Running_B")
}

/// The character's walk as the sidecars cook it: their `adjust` and `gain`.
fn walk_as_cooked() -> SampledClip {
    as_cooked(&Kaykit::open("MovementBasic"), "Walking_B")
}

/// A chop on the enemy's warrior as the sidecars cook it, and that rig: their `follow`.
fn chop_as_cooked() -> (SampledClip, Skeleton, HumanMap) {
    const CHOP: &str = "Melee_1H_Attack_Chop";
    let kaykit = Kaykit::open("CombatMelee");
    let (id, skeleton, map) = warrior();
    let rig = Rig { skeleton: &skeleton, map: &map };
    let out =
        retarget(&kaykit.sampled(CHOP), kaykit.rig(), rig, &sidecar_options(&kaykit, CHOP, id)).unwrap();
    (out, skeleton, map)
}

/// The cooked blade's visible edge stays inside the keyed hit capsule throughout each active window.
#[test]
fn warrior_blade_tracks_the_existing_attack_capsules() {
    const SCALE: f32 = 0.8;
    let (id, skeleton, map) = warrior();
    let folder = third_party().join("../combat/enemy");
    let set = MoveSetDocument::parse(&fs::read_to_string(folder.join("warrior.moves.ron")).unwrap())
        .unwrap()
        .compile()
        .unwrap();
    let target = ClipTarget { skeleton: Some((id, &skeleton)), morph_defaults: &[] };
    let hand = skeleton.names.iter().position(|name| name == "handslot.r").unwrap();
    let cooked_skeleton = skeleton.to_cooked().unwrap();
    let mut pose = CharacterPose::new(Skeleton::read(&cooked_skeleton).unwrap(), &[], &[], Vec::new());
    for name in ["swipe", "overhead", "lunge"] {
        let (_, movement) = set.find(Name::new(name)).unwrap();
        let expected = movement.tables.path(Name::new("blade")).unwrap();
        let document = keypose::read(&fs::read(folder.join(format!("{name}.keypose.ron"))).unwrap()).unwrap();
        let current = keypose::sample(&document, name, &skeleton, &map, &[]).unwrap();
        assert_eq!(u32::from(current.ticks), movement.ticks(), "{name}");
        let cooked = clip::cook(&current, &target, &ReductionError::default()).to_cooked().unwrap();
        let clip = Clip::read(&cooked).unwrap();
        for tick in movement.startup..=movement.startup + movement.active {
            let sample = expected.at(sapphire_time::SimDuration::from_ticks(u64::from(tick)));
            pose.sample(clip, tick as f32);
            pose.finish();
            let (_, turn, grip) = pose.model()[hand].to_scale_rotation_translation();
            // The blade's edge starts just beyond the guard and reaches 0.94 m along the socket.
            for distance in [0.04, 0.94] {
                let blade = grip * SCALE + turn * (Vec3::Y * distance);
                let capsule = sample.offset() + sample.orientation() * (Vec3::Y * (distance - 0.49));
                let error = blade.distance(capsule);
                assert!(error < 0.055, "{name} at tick {tick}, blade endpoint error {error:.4} m");
            }
        }
    }
}

/// The look-dev stage's own strike, as it stands.
fn strike() -> SampledClip {
    let (skeleton, map) = character();
    let path = third_party().join("../lookdev/character/strike.keypose.ron");
    let document = keypose::read(&fs::read(path).unwrap()).unwrap();
    keypose::sample(&document, "strike", skeleton, map, &[]).unwrap()
}

/// A path along every bone of the rig, each with a socket: what a table can reach of the pose.
fn every_bone(map: &HumanMap) -> ClipMotionDocument {
    motion(
        map.iter()
            .map(|(bone, _)| ClipPathDocument {
                name: bone.name().to_owned(),
                follows: Follows::Bone(format!("{bone:?}")),
                offset: SOCKET,
                rotation: SOCKET_TURN,
            })
            .collect(),
    )
}

const SOCKET: [f32; 3] = [0.05, -0.1, 0.25];
const SOCKET_TURN: [f32; 4] = [0.5, -0.5, 0.5, 0.5];

/// One replay file holds one hash of its move data, so these are one value on every architecture: CI's
/// `Test` job holds them on Linux and Windows as well as macOS. If only some fail, something on the
/// way from a source to a [`SampledClip`], or in [`bake`], has reached glam's SIMD types (`quat * vec`,
/// `Affine3A`) or a C math library, or a sign of zero by way of `f32::min`, and that's the bug: never
/// record a value per architecture. If all of them fail, the document here or a stage's output changed
/// on purpose: update the constant, and any replay that reads such a table.
///
/// The clips' own bytes are here too, since [`crate::clip`] says they hold, though no replay reads them.
///
/// Three of these read the sidecars' retarget options as shipped, so their constants move when those
/// are tuned or `warrior.glb` is regenerated. Keep one clip here for each option: a new option needs
/// one too, or this can't see what it does.
#[test]
fn baked_tables_and_clips_hash_the_same_on_every_architecture() {
    let (skeleton, map) = character();
    let hash = |bytes: &[u8]| blake3::hash(bytes).to_hex().to_string();
    let target = ClipTarget { skeleton: Some((AssetId::from_bytes([1; 16]), skeleton)), morph_defaults: &[] };
    let clip_hash = |sampled: &SampledClip| {
        hash(&clip::cook(sampled, &target, &ReductionError::default()).to_cooked().unwrap())
    };
    let input = |sampled| TableInput { sampled, skeleton, map: Some(map), constraints: &[] };
    let table_hash = |sampled| hash(&bake(&every_bone(map), &input(sampled)).unwrap().to_cooked().unwrap());

    let (signature, dodge, spin) = (signature(), dodge(), spin());
    let (run, walk) = (run_as_cooked(), walk_as_cooked());

    // `follow` turns joints no bone names, so a path has to follow one by name to see it.
    let (chop, warrior_skeleton, warrior_map) = chop_as_cooked();
    let mut warrior_paths = every_bone(&warrior_map);
    warrior_paths.paths.push(ClipPathDocument {
        name: "slot".to_owned(),
        follows: Follows::Joint("handslot.r".to_owned()),
        offset: SOCKET,
        rotation: SOCKET_TURN,
    });
    let on_the_warrior =
        TableInput { sampled: &chop, skeleton: &warrior_skeleton, map: Some(&warrior_map), constraints: &[] };
    let warrior_target =
        ClipTarget { skeleton: Some((AssetId::from_bytes([2; 16]), &warrior_skeleton)), morph_defaults: &[] };

    let found = [
        ("key-pose table", table_hash(&signature), SIGNATURE_TABLE),
        ("retargeted table", table_hash(&dodge), DODGE_TABLE),
        ("turning table", table_hash(&spin), SPIN_TABLE),
        ("key-pose clip", clip_hash(&signature), SIGNATURE_CLIP),
        ("retargeted clip", clip_hash(&dodge), DODGE_CLIP),
        ("turning clip", clip_hash(&spin), SPIN_CLIP),
        ("adjusted table", table_hash(&run), RUN_TABLE),
        ("adjusted clip", clip_hash(&run), RUN_CLIP),
        ("gained table", table_hash(&walk), WALK_TABLE),
        ("gained clip", clip_hash(&walk), WALK_CLIP),
        (
            "following table",
            hash(&bake(&warrior_paths, &on_the_warrior).unwrap().to_cooked().unwrap()),
            CHOP_TABLE,
        ),
        (
            "following clip",
            hash(&clip::cook(&chop, &warrior_target, &ReductionError::default()).to_cooked().unwrap()),
            CHOP_CLIP,
        ),
    ];
    let wrong: Vec<String> = found
        .iter()
        .filter(|(_, found, pinned)| found != pinned)
        .map(|(what, found, _)| format!("{what}: {found}"))
        .collect();
    assert!(wrong.is_empty(), "these hash differently here:\n{}", wrong.join("\n"));
}

const SIGNATURE_TABLE: &str = "43a13fa907351097b59c0884df22132a33163ae7744d84067253b5f7b9d3c4e4";
const DODGE_TABLE: &str = "5c85c8686d55e72dbaf5aaa0db5120c3b6ab3af119735259a8524f5a9ddfe77b";
const SIGNATURE_CLIP: &str = "45c52425cdd270d7996c7227fcb58da71bd774ab52fa136b0e0e7614aee3dee0";
const DODGE_CLIP: &str = "025d2b57737a7896bbf2795a372788a47328d3aa38dddbfaaa397948ff912b2d";
const SPIN_TABLE: &str = "20194ffbf790bbd46345cb28ebf6c4e48994ef6fc769353240653eefc3499653";
const SPIN_CLIP: &str = "8907b4559fdf81ecca4479aa5807629e0ed50ba8118f172224c3746e64ab5204";
const RUN_TABLE: &str = "890b9c7d65a08ea41af3c1cdcd0edefd98b78cc1f24e71ff5beaae4023776d57";
const RUN_CLIP: &str = "d0074377f507954b29d2ef1eabd7c914e73df59b2531a452e163ca752ca46812";
const WALK_TABLE: &str = "cc25a6ce65e82bd7171f598338caa76a9ef61bd6118c3232af5b76bafd641e01";
const WALK_CLIP: &str = "316c24894c9238bcca22efa52be576c277228c8a0c22770a537740a73315e139";
const CHOP_TABLE: &str = "51dd4432e0d1ce128208107574ba1b67bfebff46cbe8bdd63daa8a354cf0040e";
const CHOP_CLIP: &str = "96470e2dbaf9ff89d3d9d468c8a3b17a6a2e886caf2456fee85ddeca104784b3";

/// Each pinned clip above that stands for a retarget option really uses it, as the sidecars ship.
#[test]
fn the_pinned_clips_use_every_retarget_option() {
    let the_character = AssetId::from_text("6f8a81b6-29c4-485f-b161-847b001d561f");
    let basic = Kaykit::open("MovementBasic");
    assert!(!sidecar_options(&basic, "Running_B", the_character).adjust.is_empty());
    assert!(!sidecar_options(&basic, "Walking_B", the_character).gain.is_empty());
    let (warrior_id, ..) = warrior();
    let melee = Kaykit::open("CombatMelee");
    assert!(
        sidecar_options(&melee, "Melee_1H_Attack_Chop", warrior_id).follow.contains(&"handslot.r".to_owned())
    );
}

/// The most a table strays from the pose on screen, over every bone and tick of `sampled`: metres and
/// radians for its paths, metres and radians for its root.
fn strays_from_the_pose(sampled: &SampledClip) -> (f32, f32, f32, f32) {
    let (skeleton, map) = character();
    let rig = character_rig();
    let input = TableInput { sampled, skeleton, map: Some(map), constraints: &rig.constraints };
    let tables = bake(&every_bone(map), &input).expect("no bone goes through a twist helper");

    let target = ClipTarget { skeleton: Some((AssetId::from_bytes([1; 16]), skeleton)), morph_defaults: &[] };
    let cooked = clip::cook(sampled, &target, &ReductionError::default()).to_cooked().unwrap();
    let clip = Clip::read(&cooked).unwrap();
    let cooked_skeleton = skeleton.to_cooked().unwrap();
    let mut pose =
        CharacterPose::new(Skeleton::read(&cooked_skeleton).unwrap(), &[], &rig.constraints, Vec::new());

    let socket = Vec3::from(SOCKET);
    let socket_turn = Quat::from_array(SOCKET_TURN);
    let (mut path_distance, mut path_angle, mut root_distance, mut root_angle) =
        (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for tick in 0..=u32::from(sampled.ticks) {
        let position = tick as f32;
        pose.sample(clip, position);
        pose.finish();
        for (bone, joint) in map.iter() {
            let (_, rotation, translation) = pose.model()[usize::from(joint)].to_scale_rotation_translation();
            let sample = tables.path(Name::intern(bone.name())).unwrap().samples()[tick as usize];
            path_distance = path_distance.max(sample.offset().distance(translation + rotation * socket));
            path_angle = path_angle.max(sample.orientation().angle_between(rotation * socket_turn));
        }
        let sample = tables.root_samples()[tick as usize];
        let root = root_motion(clip, position).unwrap_or(Transform::IDENTITY);
        root_distance = root_distance.max(sample.offset().distance(root.translation));
        root_angle = root_angle.max(det::quat_from_rotation_y(sample.yaw).angle_between(root.rotation));
    }
    (path_distance, path_angle, root_distance, root_angle)
}

/// A table comes through scalars and a pose through `sapphire_anim`'s SIMD, from a clip key reduction
/// has been at, with the rig's constraints running. They agree on real clips within what the reduction
/// states, at every tick and bone, so a hit lands where the blade is drawn. The numbers are the worst
/// measured and a little.
#[test]
#[allow(clippy::print_stdout, reason = "with --nocapture, what these bounds were set from")]
fn a_table_agrees_with_the_pose_on_screen() {
    let degree = 1f32.to_radians();
    let clips = [
        ("key-pose", signature()),
        ("strike", strike()),
        ("retargeted", dodge()),
        ("turning", spin()),
        ("adjusted", run_as_cooked()),
        ("gained", walk_as_cooked()),
    ];
    for (name, sampled) in clips {
        let (distance, angle, root_distance, root_angle) = strays_from_the_pose(&sampled);
        println!(
            "{name}: paths {:.2} mm, {:.3}°, root {:.3} mm, {:.3}°",
            distance * 1e3,
            angle / degree,
            root_distance * 1e3,
            root_angle / degree
        );
        assert!(distance < 1e-3, "{name}: a path strays {distance} m");
        assert!(angle < 0.15 * degree, "{name}: a path turns {} degrees away", angle / degree);
        assert!(root_distance < 3e-4, "{name}: the root strays {root_distance} m");
        assert!(root_angle < 0.06 * degree, "{name}: the root turns {} degrees away", root_angle / degree);
    }
}

/// The retargeted clip's own numbers, that this measures something: a dodge travels, and its table says
/// so, as the document's motion does.
#[test]
fn real_clips_move_a_table() {
    let (skeleton, map) = character();
    let input = |sampled| TableInput { sampled, skeleton, map: Some(map), constraints: &[] };
    let dodge = dodge();
    let tables = bake(&every_bone(map), &input(&dodge)).unwrap();
    assert_eq!(tables.ticks(), u32::from(dodge.ticks));
    let end = tables.root_samples().last().unwrap().offset();
    assert!(end.z > 0.5 && end.x.abs() < 1e-3, "a forward dodge goes forward: {end}");

    let spin = spin();
    let tables = bake(&every_bone(map), &input(&spin)).unwrap();
    let yaws: Vec<f32> = tables.root_samples().iter().map(|sample| sample.yaw.to_degrees()).collect();
    let furthest = yaws.iter().copied().fold(f32::MAX, f32::min);
    assert!((furthest + 379.0).abs() < 1.0, "unwrapped past each half turn: {furthest}");
    assert!((yaws.last().unwrap() + 360.0).abs() < 0.5, "{:?}", yaws.last());

    let signature = signature();
    let tables = bake(&every_bone(map), &input(&signature)).unwrap();
    assert_eq!(tables.ticks(), 72);
    let last = tables.root_samples().last().unwrap();
    assert!(near(last.translation, [0.2, 0.0, 1.1]), "{last:?}");
    assert!((last.yaw - 380f32.to_radians()).abs() < 1e-4, "unwrapped past a turn: {last:?}");
}
