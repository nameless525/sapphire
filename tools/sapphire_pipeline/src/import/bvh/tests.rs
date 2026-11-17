use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glam::{EulerRot, Quat, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::rig::HumanBone::{self, *};
use sapphire_assets::skeleton::Skeleton;

use super::*;
use crate::clip::contacts::{ContactSettings, PlantedRun, planted_runs};
use crate::clip::report::Positions;
use crate::clip::retarget::tests::{character, warrior};
use crate::clip::retarget::{HumanMap, RetargetOptions, Rig, leg_length, retarget};
use crate::clip::{self, ClipOptions, ClipTarget, RootMotionSettings, RootRotation, RootTranslation};

/// Two joints and an end site, three frames: line numbers matter to the malformed cases below.
const TINY: &str = "HIERARCHY
ROOT Hips
{
  OFFSET 1 2 3
  CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation
  JOINT Leg
  {
    OFFSET 0 -90 0
    CHANNELS 3 Zrotation Xrotation Yrotation
    End Site
    {
      OFFSET 0 -5 0
    }
  }
}
MOTION
Frames: 3
Frame Time: 0.0333333
200 95 100 0 60 0 10 20 30
200 95 103 0 0 0 0 40 0
200 95 106 0 0 0 0 0 0
";

fn import(text: &str, settings: &BvhSettings) -> Result<Capture, BvhError> {
    Capture::import(Path::new("moves/tiny.bvh"), text.as_bytes(), settings)
}

fn near(a: Quat, b: Quat) -> bool {
    a.dot(b).abs() > 1.0 - 1e-6
}

#[test]
fn a_hierarchy_reads_as_joints_and_its_frames_as_tracks_at_the_clips_rate() {
    let capture = import(TINY, &BvhSettings::default()).unwrap();
    assert_eq!(capture.name, "tiny");
    let names: Vec<&str> = capture.joints.iter().map(|joint| joint.name.as_str()).collect();
    assert_eq!(names, ["Hips", "Leg"], "an end site is no joint");
    assert_eq!(capture.joints[1].parent, Some(0));
    assert_eq!(capture.joints[1].rest.translation, [0.0, -0.9, 0.0], "centimeters, by the default scale");

    let sampled = capture.sample(&ClipSettings::default()).unwrap();
    // Two frame times of 1/30 s, a hair short, at 60 a second.
    assert_eq!((sampled.rate, sampled.ticks), (60, 4));
    assert_eq!(sampled.rotations.len(), 2, "every joint has a track");
    let leg = &sampled.rotations[1].1;
    // `Zrotation Xrotation Yrotation` is Z · X · Y.
    let euler = |z: f32, x: f32, y: f32| {
        Quat::from_euler(EulerRot::ZXY, z.to_radians(), x.to_radians(), y.to_radians())
    };
    assert!(near(leg[0], euler(10.0, 20.0, 30.0)), "{:?}", leg[0]);
    assert!(near(leg[2], euler(0.0, 40.0, 0.0)), "tick 2 is frame 1: {:?}", leg[2]);
    assert!(near(leg[4], Quat::IDENTITY), "the last frame: {:?}", leg[4]);
    assert!(leg[1].angle_between(leg[0]) > 0.05 && leg[1].angle_between(leg[2]) > 0.05, "tick 1 is between");

    // The channels replace the root's offset, and the first frame stands over the origin.
    let hips = &sampled.translations[0].1;
    assert!(hips[0].abs_diff_eq(Vec3::new(0.0, 0.95, 0.0), 1e-6), "{}", hips[0]);
    assert!(hips[2].abs_diff_eq(Vec3::new(0.0, 0.95, 0.03), 1e-4), "{}", hips[2]);
    assert!(hips[1].abs_diff_eq(Vec3::new(0.0, 0.95, 0.015), 1e-4), "a line between frames: {}", hips[1]);
}

#[test]
fn the_rest_is_the_calibration_frame_or_the_hierarchy_standing_where_the_first_frame_does() {
    let on_frame = |frame| BvhSettings {
        calibration: Calibration { frame: Some(frame), pose: CalibrationPose::TPose },
        ..BvhSettings::default()
    };
    let calibrated = import(TINY, &on_frame(1)).unwrap();
    let rest = &calibrated.joints;
    assert!(near(Quat::from_array(rest[1].rest.rotation), Quat::from_rotation_x(40f32.to_radians())));
    assert!(Vec3::from(rest[0].rest.translation).abs_diff_eq(Vec3::new(0.0, 0.95, 0.03), 1e-6));

    // The first frame's hips turn 60 degrees about X, which lifts its leg joint to 0.95 - 0.9 cos 60° =
    // 0.5 m; the hierarchy's hangs straight down, so its hips rise until that joint is as high.
    let hierarchy = import(TINY, &BvhSettings::default()).unwrap();
    let rest_hips = Vec3::from(hierarchy.joints[0].rest.translation);
    assert!(rest_hips.abs_diff_eq(Vec3::new(0.0, 1.4, 0.0), 1e-5), "{rest_hips}");
    assert_eq!(hierarchy.joints[1].rest.rotation, [0.0, 0.0, 0.0, 1.0]);

    // Every frame's channels are what they were: only the rest moved.
    let samples = |capture: &Capture| capture.sample(&ClipSettings::default()).unwrap();
    assert_eq!(samples(&calibrated).rotations, samples(&hierarchy).rotations);

    let past = import(TINY, &on_frame(3)).unwrap_err();
    assert!(matches!(past, BvhError::CalibrationFrame { frame: 3, last: 2 }), "{past}");
}

#[test]
fn an_a_pose_raises_the_arms_to_the_horizontal_and_leaves_the_motion() {
    let capture = synthetic();
    let skeleton = Skeleton::new(&capture.joints);
    let map = HumanMap::from_names(&synthetic_settings().clips.humanoid, &skeleton).unwrap();
    let model = clip::rest_model(&skeleton);
    let at = |bone| model[usize::from(map.joint(bone).unwrap())].translation;
    for (upper, lower, hand, out) in [
        (LeftUpperArm, LeftLowerArm, LeftHand, Vec3::X),
        (RightUpperArm, RightLowerArm, RightHand, Vec3::NEG_X),
    ] {
        for (from, to) in [(upper, lower), (lower, hand)] {
            let along = (at(to) - at(from)).normalize();
            assert!(along.abs_diff_eq(out, 1e-5), "{from:?} points along {along}");
        }
    }
    // It stands where the first frame does: the hips at standing height, the toes on the floor.
    let hips = capture.joints[0].rest.translation;
    assert!((hips[1] - 0.98).abs() < 0.001 && hips[0] == 0.0 && hips[2] == 0.0, "{hips:?}");

    let t_pose = BvhSettings { calibration: Calibration::default(), ..synthetic_settings() };
    let unraised = Capture::import(Path::new("synthetic_walk.bvh"), &synthetic_bytes(), &t_pose).unwrap();
    let samples = |capture: &Capture| capture.sample(&synthetic_settings().clips).unwrap();
    assert_eq!(samples(&capture), samples(&unraised), "the calibration is the rest's alone");

    let no_map = BvhSettings { clips: ClipSettings::default(), ..synthetic_settings() };
    let refused = Capture::import(Path::new("synthetic_walk.bvh"), &synthetic_bytes(), &no_map).unwrap_err();
    assert!(matches!(refused, BvhError::NoArms), "{refused}");
}

/// Replaces `from` in [`TINY`] with `to`, once.
fn tiny_with(from: &str, to: &str) -> String {
    assert_eq!(TINY.matches(from).count(), 1, "{from}");
    TINY.replacen(from, to, 1)
}

#[test]
fn a_malformed_file_is_refused_naming_its_line() {
    let cases = [
        (
            "a channel missing from CHANNELS",
            tiny_with("CHANNELS 3 Zrotation Xrotation Yrotation", "CHANNELS 3 Zrotation Xrotation"),
            9,
            "CHANNELS says 3 channels, and the line names 2",
        ),
        (
            "a channel missing from a frame",
            tiny_with("200 95 103 0 0 0 0 40 0", "200 95 103 0 0 0 0 40"),
            20,
            "frame 1 has 8 values, where the hierarchy's channels need 9",
        ),
        (
            "fewer frames than the header says",
            tiny_with("Frames: 3", "Frames: 5"),
            17,
            "the header says 5 frames, but 3 follow",
        ),
        ("more frames than the header says", tiny_with("Frames: 3", "Frames: 2"), 21, "a frame past the 2"),
        (
            "a frame count that isn't one",
            tiny_with("Frames: 3", "Frames: three"),
            17,
            "\"three\" isn't a frame count",
        ),
        ("no frames", tiny_with("Frames: 3", "Frames: 0"), 17, "no frames"),
        (
            "a value that isn't a number",
            tiny_with("200 95 106", "200 95 1o6"),
            21,
            "\"1o6\" isn't a finite number",
        ),
        (
            "a value that isn't finite",
            tiny_with("200 95 106", "200 NaN 106"),
            21,
            "\"NaN\" isn't a finite number",
        ),
        (
            "a frame time of nothing",
            tiny_with("Frame Time: 0.0333333", "Frame Time: 0"),
            18,
            "the frame time \"0\"",
        ),
        (
            "a channel that isn't one",
            tiny_with("Zrotation Xrotation Yrotation\n    End", "Zrotation Xrotation Wrotation\n    End"),
            9,
            "\"Wrotation\" isn't a channel name",
        ),
        (
            "a channel twice",
            tiny_with("Zrotation Xrotation Yrotation\n    End", "Zrotation Xrotation Zrotation\n    End"),
            9,
            "names channel Zrotation twice",
        ),
        (
            "a joint without its offset",
            tiny_with("    OFFSET 0 -90 0\n", ""),
            13,
            "\"Leg\" closes before its OFFSET",
        ),
        ("a second root", tiny_with("}\nMOTION", "}\nROOT Arm\nMOTION"), 16, "a second ROOT"),
        (
            "a hierarchy cut short",
            TINY.lines().take(10).collect::<Vec<_>>().join("\n"),
            10,
            "the file ends inside the hierarchy",
        ),
        ("a word where a keyword belongs", tiny_with("  JOINT Leg", "  BONE Leg"), 6, "found \"BONE\""),
    ];
    for (what, text, line, says) in cases {
        let error = import(&text, &BvhSettings::default()).unwrap_err();
        let message = error.to_string();
        assert!(matches!(error, BvhError::Line { line: found, .. } if found == line), "{what}: {message}");
        assert!(
            message.starts_with(&format!("line {line}: ")) && message.contains(says),
            "{what}: {message}"
        );
    }
    assert!(
        import(&format!("\u{feff}{TINY}"), &BvhSettings::default()).is_ok(),
        "a byte-order mark is nothing"
    );
    let refused =
        Capture::import(Path::new("bad.bvh"), &[0xff, 0xfe, 0x00], &BvhSettings::default()).unwrap_err();
    assert!(matches!(refused, BvhError::NotText), "{refused}");
    let unscaled = BvhSettings { scale: 0.0, ..BvhSettings::default() };
    assert!(matches!(import(TINY, &unscaled).unwrap_err(), BvhError::Scale(_)));
}

// The synthetic capture `tools/motion/capture.py` writes.

/// Where the slide is injected, frames at 30 a second: `capture.INJECTED`.
const INJECTED: (u16, u16) = (34, 44);
/// How far: `capture.SLIDE`, meters.
const SLIDE: f32 = 0.05;

fn synthetic_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/bvh/synthetic_walk.bvh")
}

pub(crate) fn synthetic_bytes() -> Vec<u8> {
    std::fs::read(synthetic_path()).unwrap()
}

/// The sidecar a capture like it would have: centimeters, an A-pose, root motion from the hips.
pub(crate) fn synthetic_settings() -> BvhSettings {
    let humanoid: BTreeMap<HumanBone, String> = [
        (Hips, "Hips"),
        (Spine, "Spine"),
        (Chest, "Spine1"),
        (Neck, "Neck"),
        (Head, "Head"),
        (LeftShoulder, "LeftShoulder"),
        (LeftUpperArm, "LeftArm"),
        (LeftLowerArm, "LeftForeArm"),
        (LeftHand, "LeftHand"),
        (RightShoulder, "RightShoulder"),
        (RightUpperArm, "RightArm"),
        (RightLowerArm, "RightForeArm"),
        (RightHand, "RightHand"),
        (LeftUpperLeg, "LeftUpLeg"),
        (LeftLowerLeg, "LeftLeg"),
        (LeftFoot, "LeftFoot"),
        (LeftToes, "LeftToeBase"),
        (RightUpperLeg, "RightUpLeg"),
        (RightLowerLeg, "RightLeg"),
        (RightFoot, "RightFoot"),
        (RightToes, "RightToeBase"),
    ]
    .into_iter()
    .map(|(bone, joint)| (bone, joint.to_owned()))
    .collect();
    BvhSettings {
        calibration: Calibration { frame: None, pose: CalibrationPose::APose },
        clips: ClipSettings {
            root_motion: Some(RootMotionSettings {
                bone: "Hips".to_owned(),
                translation: RootTranslation::Ground,
                rotation: RootRotation::Yaw,
            }),
            humanoid,
            ..ClipSettings::default()
        },
        ..BvhSettings::default()
    }
}

fn synthetic() -> Capture {
    Capture::import(&synthetic_path(), &synthetic_bytes(), &synthetic_settings()).unwrap()
}

/// The synthetic walk on its own rig, on the look-dev character and on the enemy's warrior, as the cook
/// would make each: the samples, the rig, and what translations scaled by.
fn on_every_rig(mut each: impl FnMut(&str, &SampledClip, Rig<'_>, f32)) {
    let capture = synthetic();
    let settings = synthetic_settings();
    let sampled = capture.sample(&settings.clips).unwrap();
    let skeleton = Skeleton::new(&capture.joints);
    let map = HumanMap::from_names(&settings.clips.humanoid, &skeleton).unwrap();
    let source = Rig { skeleton: &skeleton, map: &map };
    each("its own rig", &sampled, source, 1.0);
    let (character_skeleton, character_map) = character();
    let (_, warrior_skeleton, warrior_map) = warrior();
    for (who, target) in [
        ("the character", Rig { skeleton: character_skeleton, map: character_map }),
        ("the warrior", Rig { skeleton: &warrior_skeleton, map: &warrior_map }),
    ] {
        let moved = retarget(&sampled, source, target, &RetargetOptions::default()).unwrap();
        each(who, &moved, target, leg_length(target) / leg_length(source));
    }
}

/// A1's report finds the injected slide on the injected frames, and no planted foot sliding as far
/// anywhere else: on the capture's own rig, and retargeted onto the look-dev character and the enemy's
/// warrior, whose other proportions slide every planted foot a little of their own (about 7 mm and 2 cm
/// a stance; A4's to fix). `--nocapture` prints every planted run.
#[test]
fn the_synthetic_walks_slide_is_reported_on_the_injected_frames_and_nowhere_else() {
    #![allow(clippy::print_stdout, reason = "the runs, with --nocapture")]
    // Ticks at 60 a second: the frames' 30 doubled.
    let injected = (INJECTED.0 * 2, INJECTED.1 * 2);
    on_every_rig(|who, sampled, rig, scale| {
        let positions = Positions::of(sampled, rig.skeleton);
        let runs = planted_runs(sampled, rig.skeleton, rig.map, &positions, &ContactSettings::default());
        let expected = SLIDE * scale;
        println!("{who}: the injected slide is {expected:.4} m");
        for run in &runs {
            println!("  {:?} ticks {}..={}: {:.4} m", run.foot, run.first, run.last, run.slide);
        }
        // The runs the injection falls in: the left foot's, planted before it starts and into it. A
        // retarget's own stride can lift the foot a few ticks before it ends (the character's run ends
        // at 86 of 88).
        let (at_injection, elsewhere): (Vec<&PlantedRun>, Vec<&PlantedRun>) = runs
            .iter()
            .partition(|run| run.foot == LeftFoot && run.first <= injected.0 && run.last > injected.0);
        let worst = |runs: &[&PlantedRun]| runs.iter().map(|run| run.slide).fold(0.0f32, f32::max);
        let (injected_slide, own_slide) = (worst(&at_injection), worst(&elsewhere));
        // What the rig slides on its own, as a retarget of other proportions does, sits under every
        // run; the injection stands out above it by about what was injected, and nothing else does.
        println!("  the rig's own slide is {own_slide:.4} m, and the injection's run {injected_slide:.4} m");
        assert!(!at_injection.is_empty(), "{who}: no run holds the injection");
        let above = injected_slide - own_slide;
        assert!(
            above > expected * 0.5 && above < expected * 1.2,
            "{who}: the injection stands {above} m above"
        );
        let cooked = clip::cook(
            sampled,
            &ClipTarget {
                skeleton: Some((AssetId::from_bytes([1; 16]), rig.skeleton)),
                morph_defaults: &[],
                map: Some(rig.map),
            },
            &ClipOptions::default(),
        );
        let slide = |foot| cooked.contacts.iter().find(|track| track.foot == foot).unwrap().slide;
        assert_eq!(
            slide(LeftFoot),
            injected_slide,
            "{who}: the clip's left foot slides the injection's run's"
        );
        assert!(slide(RightFoot) <= own_slide, "{who}");
    });
}

/// The committed capture, its skeleton and its clip on each rig, by their bytes: the generator and this
/// importer are deterministic, so these hold on every architecture CI runs. A change to
/// `tools/motion/capture.py` that moves the file moves the first, on purpose.
#[test]
fn the_synthetic_walk_and_its_clips_hash_the_same_on_every_architecture() {
    let hash = |bytes: &[u8]| blake3::hash(bytes).to_hex().to_string();
    let mut found: Vec<(String, String)> = vec![
        ("capture".to_owned(), hash(&synthetic_bytes())),
        ("skeleton".to_owned(), hash(&Skeleton::new(&synthetic().joints).to_cooked().unwrap())),
    ];
    on_every_rig(|who, sampled, rig, _| {
        let target = ClipTarget {
            skeleton: Some((AssetId::from_bytes([1; 16]), rig.skeleton)),
            morph_defaults: &[],
            map: Some(rig.map),
        };
        found.push((
            who.to_owned(),
            hash(&clip::cook(sampled, &target, &ClipOptions::default()).to_cooked().unwrap()),
        ));
    });
    let pinned = [
        ("capture", CAPTURE),
        ("skeleton", SKELETON),
        ("its own rig", OWN_CLIP),
        ("the character", CHARACTER_CLIP),
        ("the warrior", WARRIOR_CLIP),
    ];
    let wrong: Vec<String> = found
        .iter()
        .zip(pinned)
        .filter(|((_, found), (_, pinned))| found != pinned)
        .map(|((what, found), _)| format!("{what}: {found}"))
        .collect();
    assert!(wrong.is_empty(), "these hash differently here:\n{}", wrong.join("\n"));
}

const CAPTURE: &str = "16dab6c3223fee036492d89ead65e5f2ced11665710fd8afb554deb3d15cb3f8";
const SKELETON: &str = "045a22f8e16679fa82569d55f1f090d61e374ce92f374108220942a21aa9eb27";
const OWN_CLIP: &str = "32c04e256df7bea260e453775a94df6b1291e70324aec48866229c036f2e7f97";
const CHARACTER_CLIP: &str = "67642fd302bf7b787349f30151d6f9380fee255bd5f946783d613a2b145115d2";
const WARRIOR_CLIP: &str = "c02bd7439502d7c968ad57f00d5ef73010629c39414402e34bd5b97c9d9d3312";
