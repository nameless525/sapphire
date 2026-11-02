//! VRMA files built in code: no licensed VRMA can be committed.

use glam::Quat;
use sapphire_assets::rig::HumanBone;
use serde_json::{Value, json};

use super::*;
use crate::clip::source::ClipSource;
use crate::clip::{ClipSettings, RootMotionSettings};
use crate::gltf_writer::GltfWriter;
use crate::import::gltf::clips::tests::{Track, animate};
use crate::meta::ImportSettings;

/// A small humanoid in a T-pose, as VRMA has it: name, parent, translation and the bone.
const NODES: &[(&str, Option<usize>, [f32; 3], &str)] = &[
    ("Hips", None, [0.0, 1.0, 0.0], "hips"),
    ("Spine", Some(0), [0.0, 0.1, 0.0], "spine"),
    ("Head", Some(1), [0.0, 0.5, 0.0], "head"),
    ("UpperLegL", Some(0), [0.1, -0.05, 0.0], "leftUpperLeg"),
    ("LowerLegL", Some(3), [0.0, -0.45, 0.0], "leftLowerLeg"),
    ("FootL", Some(4), [0.0, -0.45, 0.0], "leftFoot"),
    ("UpperLegR", Some(0), [-0.1, -0.05, 0.0], "rightUpperLeg"),
    ("LowerLegR", Some(6), [0.0, -0.45, 0.0], "rightLowerLeg"),
    ("FootR", Some(7), [0.0, -0.45, 0.0], "rightFoot"),
    ("UpperArmL", Some(1), [0.2, 0.4, 0.0], "leftUpperArm"),
    ("LowerArmL", Some(9), [0.25, 0.0, 0.0], "leftLowerArm"),
    ("HandL", Some(10), [0.25, 0.0, 0.0], "leftHand"),
    ("UpperArmR", Some(1), [-0.2, 0.4, 0.0], "rightUpperArm"),
    ("LowerArmR", Some(12), [-0.25, 0.0, 0.0], "rightLowerArm"),
    ("HandR", Some(13), [-0.25, 0.0, 0.0], "rightHand"),
];

pub(crate) const HIPS: usize = 0;
pub(crate) const ARM: usize = 9;

/// The file's nodes and its extension, with no animation yet.
pub(crate) fn vrma() -> GltfWriter {
    let mut writer = GltfWriter::default();
    for (index, (name, _, translation, _)) in NODES.iter().enumerate() {
        let children: Vec<usize> = (0..NODES.len()).filter(|&child| NODES[child].1 == Some(index)).collect();
        let mut node = json!({ "name": name, "translation": translation });
        if !children.is_empty() {
            node["children"] = json!(children);
        }
        writer.add_node(node);
    }
    writer.add_root(HIPS);
    let bones: serde_json::Map<String, Value> = NODES
        .iter()
        .enumerate()
        .map(|(index, (_, _, _, bone))| ((*bone).to_owned(), json!({ "node": index })))
        .collect();
    writer.set_extension(
        VRMC_VRM_ANIMATION,
        json!({ "specVersion": "1.0", "humanoid": { "humanBones": bones } }),
    );
    writer
}

/// The arm lowered a quarter turn about +Z over half a second and the hips sinking 10 cm.
pub(crate) fn lower_arm(writer: &mut GltfWriter) {
    let half = std::f32::consts::FRAC_1_SQRT_2;
    animate(
        writer,
        Some("Lower"),
        &[
            Track {
                node: ARM,
                path: "rotation",
                interpolation: "LINEAR",
                times: &[0.0, 0.5],
                values: &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -half, half],
            },
            Track {
                node: HIPS,
                path: "translation",
                interpolation: "LINEAR",
                times: &[0.0, 0.5],
                values: &[0.0, 1.0, 0.0, 0.0, 0.9, 0.0],
            },
        ],
    );
}

fn imported(bytes: &[u8]) -> ClipSource {
    let settings = ImportSettings::Vrma(ClipSettings::default());
    ClipSource::import(std::path::Path::new("test.vrma"), bytes, &settings).unwrap()
}

#[test]
fn a_vrma_names_its_bones_and_samples_to_a_clip() {
    let mut writer = vrma();
    lower_arm(&mut writer);
    let source = imported(&writer.to_glb());

    let names: Vec<&str> = source.joints.iter().map(|joint| joint.name.as_str()).collect();
    assert_eq!(names.len(), NODES.len(), "every bone is a joint, moved or not: {names:?}");
    let map = source.map.as_ref().expect("the file's map");
    for (index, (name, _, _, bone)) in NODES.iter().enumerate() {
        let joint = map.joint(HumanBone::from_name(bone).unwrap()).unwrap();
        assert_eq!(source.joints[usize::from(joint)].name, *name, "{index}");
    }

    let clip = source.sample(0, &ClipSettings::default()).unwrap();
    assert_eq!((clip.name.as_str(), clip.ticks), ("Lower", 30));
    let arm = map.joint(HumanBone::LeftUpperArm).unwrap();
    let (_, samples) = clip.rotations.iter().find(|(joint, _)| *joint == arm).unwrap();
    let expected = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2);
    assert!(samples[30].dot(expected).abs() > 0.99999, "{:?}", samples[30]);
}

#[test]
fn root_motion_comes_from_the_joint_named_for_it() {
    let mut writer = vrma();
    let half = std::f32::consts::FRAC_1_SQRT_2;
    animate(
        &mut writer,
        Some("Turn"),
        &[Track {
            node: HIPS,
            path: "rotation",
            interpolation: "LINEAR",
            times: &[0.0, 0.5],
            values: &[0.0, 0.0, 0.0, 1.0, 0.0, half, 0.0, half],
        }],
    );
    let source = imported(&writer.to_glb());
    let settings = ClipSettings {
        root_motion: Some(RootMotionSettings {
            bone: "Hips".to_owned(),
            translation: Default::default(),
            rotation: Default::default(),
        }),
        ..ClipSettings::default()
    };
    let clip = source.sample(0, &settings).unwrap();
    let root = clip.root.expect("lifted");
    assert!(root.rotation[30].dot(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)).abs() > 0.99999);
}

/// Each edit breaks a good file; each must fail with its own message and no panic.
#[test]
fn a_bad_extension_is_an_error() {
    let cases: Vec<(&str, Value, &str)> = vec![
        ("no extension", Value::Null, "no VRMC_vrm_animation"),
        ("not an object", json!(3), "is invalid"),
        ("another version", json!({ "specVersion": "2.0" }), "version \"2.0\""),
        ("no humanoid", json!({ "specVersion": "1.0" }), "no hips"),
        (
            "no hips",
            json!({ "specVersion": "1.0", "humanoid": { "humanBones": { "head": { "node": 0 } } } }),
            "no hips",
        ),
        (
            "a bone past the nodes",
            json!({ "specVersion": "1.0", "humanoid": { "humanBones": { "hips": { "node": 99 } } } }),
            "isn't there",
        ),
        (
            "a bone with no node",
            json!({ "specVersion": "1.0", "humanoid": { "humanBones": { "hips": {} } } }),
            "names no node",
        ),
    ];
    let mut failures: Vec<String> = Vec::new();
    for (name, extension, expected) in cases {
        let mut writer = vrma();
        lower_arm(&mut writer);
        let mut document = writer.document(None);
        if extension.is_null() {
            document["extensions"].as_object_mut().unwrap().remove(VRMC_VRM_ANIMATION);
        } else {
            document["extensions"][VRMC_VRM_ANIMATION] = extension;
        }
        let bytes = crate::gltf_writer::glb(&document, writer.bin());
        let settings = ImportSettings::Vrma(ClipSettings::default());
        match ClipSource::import(std::path::Path::new("bad.vrma"), &bytes, &settings) {
            Ok(_) => failures.push(format!("{name}: imported")),
            Err(error) => {
                let text = error.to_string();
                if !text.contains(expected) {
                    failures.push(format!("{name}: {text}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn a_bone_outside_the_scene_is_an_error() {
    let mut writer = vrma();
    let stray = writer.add_node(json!({ "name": "Stray" }));
    let mut document = writer.document(None);
    document["extensions"][VRMC_VRM_ANIMATION]["humanoid"]["humanBones"]["jaw"] = json!({ "node": stray });
    let bytes = crate::gltf_writer::glb(&document, writer.bin());
    let settings = ImportSettings::Vrma(ClipSettings::default());
    let error = ClipSource::import(std::path::Path::new("bad.vrma"), &bytes, &settings).err().unwrap();
    assert!(error.to_string().contains("isn't in the scene"), "{error}");
}
