//! Animations read back from cooked clips, against the source's own values at key times and between
//! keys. Crafted bad files are in `malformed_tests`, beside the importer's other tables.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use glam::{Affine3A, Mat4, Quat, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::clip::Clip;
use sapphire_assets::skeleton::{JointTransform, Skeleton};
use serde_json::json;

use super::*;
use crate::clip::tests::{angle_between, model_of, root_of, sample_pose};
use crate::clip::{ClipTarget, ReductionError, RootMotionSettings, RootRotation, RootTranslation};
use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet, MorphGeometry};

/// A channel: the node it animates, the property, and its sampler's interpolation, times and values.
pub(crate) struct Track<'a> {
    pub node: usize,
    pub path: &'a str,
    pub interpolation: &'a str,
    pub times: &'a [f32],
    pub values: &'a [f32],
}

/// Adds an animation of `tracks`.
pub(crate) fn animate(writer: &mut GltfWriter, name: Option<&str>, tracks: &[Track<'_>]) {
    let mut channels = Vec::new();
    let mut samplers = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        let kind = match track.path {
            "translation" | "scale" => "VEC3",
            "rotation" => "VEC4",
            _ => "SCALAR",
        };
        let input = writer.add_floats("SCALAR", track.times);
        let output = writer.add_floats(kind, track.values);
        samplers.push(json!({ "input": input, "output": output, "interpolation": track.interpolation }));
        channels.push(json!({ "sampler": index, "target": { "node": track.node, "path": track.path } }));
    }
    let mut animation = json!({ "channels": channels, "samplers": samplers });
    if let Some(name) = name {
        animation["name"] = name.into();
    }
    writer.add_animation(animation);
}

/// The figure's nodes.
const ROOT: usize = 0;
pub(crate) const HIPS: usize = 1;
pub(crate) const ARM: usize = 2;
const BODY: usize = 3;

/// A skinned triangle on a three-joint chain (`Root` at 1 m, `Hips` above it, `Arm` to its side),
/// its mesh with one morph target.
pub(crate) fn figure() -> GltfWriter {
    let geometry = Geometry {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        influences: vec![InfluenceSet {
            joints: vec![[0, 0, 0, 0]; 3],
            weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
        }],
        targets: vec![MorphGeometry { positions: vec![[0.0, 0.1, 0.0]; 3], normals: Vec::new() }],
        indices: vec![0, 1, 2],
        ..Geometry::default()
    };
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Body", &[(&geometry, None)]);
    let root =
        writer.add_node(json!({ "name": "Root", "translation": [0.0, 1.0, 0.0], "children": [HIPS, ARM] }));
    let hips = writer.add_node(json!({ "name": "Hips", "translation": [0.0, 0.25, 0.0] }));
    let arm = writer.add_node(json!({ "name": "Arm", "translation": [0.5, 0.0, 0.0] }));
    assert_eq!((root, hips, arm), (ROOT, HIPS, ARM));
    let matrix = Mat4::IDENTITY.to_cols_array();
    writer.add_skin(&[root, hips, arm], Some(&[matrix; 3]));
    let body = writer.add_node(json!({ "name": "Body", "mesh": mesh, "skin": 0 }));
    assert_eq!(body, BODY);
    writer.add_root(root);
    writer.add_root(body);
    writer
}

/// `bytes` animation `index`, cooked and read back, with the skeleton.
struct Cooked {
    clip: sapphire_serialize::cooked::AlignedVec,
    skeleton: Option<sapphire_serialize::cooked::AlignedVec>,
}

impl Cooked {
    fn new(bytes: &[u8], index: usize, settings: &ClipSettings) -> Self {
        let (document, buffers, scene) = import_scene(bytes, None).unwrap();
        let sampled = sample(&document, &buffers, scene.clip_scene(), index, settings).unwrap();
        let skeleton = scene.mesh.skin.as_ref().map(|skin| Skeleton::new(&skin.joints));
        let defaults: Vec<f32> =
            scene.mesh.morph_targets.iter().map(|target| target.default_weight).collect();
        let target = ClipTarget {
            skeleton: skeleton.as_ref().map(|skeleton| (AssetId::from_bytes([3; 16]), skeleton)),
            morph_defaults: &defaults,
        };
        let clip = clip::cook(&sampled, &target, &settings.error).to_cooked().unwrap();
        Clip::read(&clip).unwrap();
        Self { clip, skeleton: skeleton.map(|skeleton| skeleton.to_cooked().unwrap()) }
    }

    fn clip(&self) -> &sapphire_assets::clip::ArchivedClip {
        Clip::read(&self.clip).unwrap()
    }

    fn skeleton(&self) -> &sapphire_assets::skeleton::ArchivedSkeleton {
        Skeleton::read(self.skeleton.as_ref().unwrap()).unwrap()
    }
}

/// Tighter than reduction's default, so differences here are the importer's.
fn exact() -> ClipSettings {
    ClipSettings { error: ReductionError { distance: 0.0, weight: 0.0 }, ..ClipSettings::default() }
}

#[test]
fn channels_reach_their_joints_and_targets_at_keys_and_between() {
    let mut writer = figure();
    let quarter = Quat::from_rotation_z(FRAC_PI_2);
    animate(
        &mut writer,
        Some("Walk"),
        &[
            Track {
                node: HIPS,
                path: "translation",
                interpolation: "LINEAR",
                times: &[0.0, 1.0],
                values: &[0.0, 0.25, 0.0, 1.0, 0.25, 2.0],
            },
            Track {
                node: ARM,
                path: "rotation",
                interpolation: "LINEAR",
                times: &[0.0, 1.0],
                values: &[0.0, 0.0, 0.0, 1.0, quarter.x, quarter.y, quarter.z, quarter.w],
            },
            Track {
                node: ROOT,
                path: "scale",
                interpolation: "LINEAR",
                times: &[0.0, 0.5, 1.0],
                values: &[1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0],
            },
            Track {
                node: BODY,
                path: "weights",
                interpolation: "LINEAR",
                times: &[0.0, 0.5],
                values: &[0.0, 1.0],
            },
        ],
    );
    let cooked = Cooked::new(&writer.to_glb(), 0, &exact());
    let (clip, joints) = (cooked.clip(), cooked.skeleton());
    assert_eq!((clip.rate.to_native(), clip.ticks.to_native()), (60, 60));
    assert_eq!(clip.name.as_str(), "Walk");
    assert_eq!(clip.joint_count.to_native(), 3);
    assert_eq!(clip.morph_target_count.to_native(), 1);

    let close = |a: Vec3, b: Vec3| a.distance(b) < 1e-3;
    // Keys, and halfway between them.
    let pose = sample_pose(clip, joints, 0.0);
    assert!(close(pose.translation[1], Vec3::new(0.0, 0.25, 0.0)));
    assert!(angle_between(pose.rotation[2], Quat::IDENTITY) < 1e-3);
    assert!(close(pose.scale[0], Vec3::ONE));

    let pose = sample_pose(clip, joints, 30.0);
    assert!(close(pose.translation[1], Vec3::new(0.5, 0.25, 1.0)), "{}", pose.translation[1]);
    assert!(angle_between(pose.rotation[2], Quat::from_rotation_z(FRAC_PI_4)) < 1e-3, "a slerp");
    assert!(close(pose.scale[0], Vec3::splat(2.0)), "a key");

    let pose = sample_pose(clip, joints, 15.0);
    assert!(close(pose.translation[1], Vec3::new(0.25, 0.25, 0.5)));
    assert!(angle_between(pose.rotation[2], Quat::from_rotation_z(FRAC_PI_4 / 2.0)) < 1e-3);
    assert!(close(pose.scale[0], Vec3::splat(1.5)));

    let pose = sample_pose(clip, joints, 60.0);
    assert!(close(pose.translation[1], Vec3::new(1.0, 0.25, 2.0)));
    assert!(angle_between(pose.rotation[2], quarter) < 1e-3);
    assert!(close(pose.scale[0], Vec3::ONE));

    // Between ticks, too: a sampler's blend of the two around it.
    let pose = sample_pose(clip, joints, 7.5);
    assert!(close(pose.translation[1], Vec3::new(0.125, 0.25, 0.25)), "{}", pose.translation[1]);

    // Morph weights, whose channel ends early: held from there on.
    let weight = |position: f32| clip.weight(&clip.weights[0], position);
    assert_eq!(clip.weights[0].target.to_native(), 0);
    assert!(weight(0.0).abs() < 1e-3);
    assert!((weight(15.0) - 0.5).abs() < 1e-3);
    assert!((weight(30.0) - 1.0).abs() < 1e-3);
    assert!((weight(60.0) - 1.0).abs() < 1e-3);

    // And what a skeleton's model space then shows: joints move through their parents.
    let model = model_of(&sample_pose(clip, joints, 30.0), joints);
    assert!(
        close(Vec3::from(model[2].translation), Vec3::new(1.0, 1.0, 0.0)),
        "scaled twice over: {:?}",
        model[2]
    );
}

#[test]
fn step_holds_until_the_next_key() {
    let mut writer = figure();
    animate(
        &mut writer,
        None,
        &[Track {
            node: HIPS,
            path: "translation",
            interpolation: "STEP",
            times: &[0.0, 0.5, 1.0],
            values: &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0, 0.0],
        }],
    );
    let cooked = Cooked::new(&writer.to_glb(), 0, &exact());
    let (clip, joints) = (cooked.clip(), cooked.skeleton());
    let x = |tick: f32| sample_pose(clip, joints, tick).translation[1].x;
    assert!(x(29.0).abs() < 1e-3, "{}", x(29.0));
    assert!((x(30.0) - 1.0).abs() < 1e-3);
    assert!((x(59.0) - 1.0).abs() < 1e-3);
    assert!((x(60.0) - 2.0).abs() < 1e-3);
    // The cut is a blend over one tick: what a clip's one interpolation can say, on every tick.
    assert!((x(29.5) - 0.5).abs() < 1e-3);
}

#[test]
fn cubic_segments_follow_their_tangents() {
    // A segment from 0 to 1 m over 1 s, leaving at 2 m/s and arriving at 0: glTF's Hermite spline.
    let spline = |u: f32| (u * u * u - 2.0 * u * u + u) * 2.0 + (-2.0 * u * u * u + 3.0 * u * u);
    let mut writer = figure();
    animate(
        &mut writer,
        None,
        &[Track {
            node: HIPS,
            path: "translation",
            interpolation: "CUBICSPLINE",
            times: &[0.0, 1.0],
            #[rustfmt::skip]
            values: &[
                // in tangent, value, out tangent; then the next key's.
                0.0, 0.0, 0.0,  0.0, 0.0, 0.0,  2.0, 0.0, 0.0,
                0.0, 0.0, 0.0,  1.0, 0.0, 0.0,  0.0, 0.0, 0.0,
            ],
        }],
    );
    let cooked = Cooked::new(&writer.to_glb(), 0, &exact());
    let (clip, joints) = (cooked.clip(), cooked.skeleton());
    for tick in [0, 7, 15, 30, 45, 52, 60] {
        let x = sample_pose(clip, joints, tick as f32).translation[1].x;
        let expected = spline(tick as f32 / 60.0);
        assert!((x - expected).abs() < 1e-3, "tick {tick}: {x} against {expected}");
    }

    // A rotation's spline is renormalized.
    let mut writer = figure();
    let quarter = Quat::from_rotation_y(FRAC_PI_2);
    animate(
        &mut writer,
        None,
        &[Track {
            node: ARM,
            path: "rotation",
            interpolation: "CUBICSPLINE",
            times: &[0.0, 1.0],
            #[rustfmt::skip]
            values: &[
                0.0, 0.0, 0.0, 0.0,  0.0, 0.0, 0.0, 1.0,  0.0, 0.0, 0.0, 0.0,
                0.0, 0.0, 0.0, 0.0,  quarter.x, quarter.y, quarter.z, quarter.w,  0.0, 0.0, 0.0, 0.0,
            ],
        }],
    );
    let cooked = Cooked::new(&writer.to_glb(), 0, &exact());
    let (clip, joints) = (cooked.clip(), cooked.skeleton());
    let middle = sample_pose(clip, joints, 30.0).rotation[2];
    assert!((middle.length() - 1.0).abs() < 1e-5);
    assert!(angle_between(middle, Quat::from_rotation_y(FRAC_PI_4)) < 1e-3, "{middle}");
}

#[test]
fn normalized_integer_rotations_read_as_rotations() {
    let mut writer = figure();
    let input = writer.add_floats("SCALAR", &[0.0, 1.0]);
    let output = writer.add_rotations_normalized(&[0, 0, 0, 32767, 0, 23170, 0, 23170]);
    writer.add_animation(json!({
        "channels": [{ "sampler": 0, "target": { "node": ARM, "path": "rotation" } }],
        "samplers": [{ "input": input, "output": output }],
    }));
    let cooked = Cooked::new(&writer.to_glb(), 0, &exact());
    let (clip, joints) = (cooked.clip(), cooked.skeleton());
    let end = sample_pose(clip, joints, 60.0).rotation[2];
    assert!(angle_between(end, Quat::from_rotation_y(FRAC_PI_2)) < 1e-3, "{end}");
}

#[test]
fn a_clip_starts_at_its_first_key_and_ends_on_a_whole_tick() {
    let mut writer = figure();
    animate(
        &mut writer,
        None,
        &[
            // The earliest key is at 2 s, as a Blender action starting past frame 1 writes.
            Track {
                node: HIPS,
                path: "translation",
                interpolation: "LINEAR",
                times: &[2.0, 3.0],
                values: &[0.0, 0.25, 0.0, 1.0, 0.25, 0.0],
            },
            // Starts later, ends a third of a tick past the last whole one; held before and after.
            Track {
                node: ARM,
                path: "translation",
                interpolation: "LINEAR",
                times: &[2.5, 3.5 + 1.0 / 180.0],
                values: &[0.5, 0.0, 0.0, 1.5, 0.0, 0.0],
            },
        ],
    );
    let cooked = Cooked::new(&writer.to_glb(), 0, &exact());
    let (clip, joints) = (cooked.clip(), cooked.skeleton());
    assert_eq!(clip.ticks.to_native(), 91, "1.5 s and a third of a tick, rounded up");
    let pose = sample_pose(clip, joints, 0.0);
    assert!((pose.translation[1].x).abs() < 1e-3, "starts at the earliest key");
    assert!((pose.translation[2].x - 0.5).abs() < 1e-3, "held before its first key");
    let pose = sample_pose(clip, joints, 60.0);
    assert!((pose.translation[1].x - 1.0).abs() < 1e-3);
    assert!((sample_pose(clip, joints, 91.0).translation[1].x - 1.0).abs() < 1e-3, "held after its last");
    assert!((sample_pose(clip, joints, 91.0).translation[2].x - 1.5).abs() < 1e-3, "the end is the source's");

    // A duration that's whole but for float rounding is that many ticks, not one more.
    let mut writer = figure();
    animate(
        &mut writer,
        None,
        &[Track {
            node: HIPS,
            path: "translation",
            interpolation: "LINEAR",
            times: &[0.0, 0.1],
            values: &[0.0, 0.25, 0.0, 1.0, 0.25, 0.0],
        }],
    );
    let cooked = Cooked::new(&writer.to_glb(), 0, &exact());
    assert_eq!(cooked.clip().ticks.to_native(), 6);
}

#[test]
fn a_rate_of_the_sidecars_choosing_counts_ticks_at_that_rate() {
    let mut writer = figure();
    animate(
        &mut writer,
        None,
        &[Track {
            node: HIPS,
            path: "translation",
            interpolation: "LINEAR",
            times: &[0.0, 1.0],
            values: &[0.0, 0.25, 0.0, 1.0, 0.25, 0.0],
        }],
    );
    let settings = ClipSettings { rate: 120, ..exact() };
    let cooked = Cooked::new(&writer.to_glb(), 0, &settings);
    let clip = cooked.clip();
    assert_eq!((clip.rate.to_native(), clip.ticks.to_native()), (120, 120));
    assert_eq!(clip.duration(), 1.0);
    assert_eq!(clip.position(0.5), 60.0);
}

#[test]
fn every_animation_is_a_clip_with_a_name_of_its_own() {
    let mut writer = figure();
    let track = |values: &'static [f32]| Track {
        node: HIPS,
        path: "translation",
        interpolation: "LINEAR",
        times: &[0.0, 1.0],
        values,
    };
    let values: &'static [f32] = &[0.0, 0.25, 0.0, 1.0, 0.25, 0.0];
    animate(&mut writer, Some("Run"), &[track(values)]);
    animate(&mut writer, None, &[track(values)]);
    animate(&mut writer, Some("Hit"), &[track(values)]);
    animate(&mut writer, Some("Hit"), &[track(values)]);
    assert_eq!(names(&writer.to_glb()).unwrap(), ["Run", "#1", "Hit#2", "Hit#3"]);
    assert_eq!(names(&figure().to_glb()).unwrap(), Vec::<String>::new());
    let cooked = Cooked::new(&writer.to_glb(), 2, &exact());
    assert_eq!(cooked.clip().name.as_str(), "Hit#2");
}

/// A node an animation moves joins the skeleton, so a part animated on its own node moves too, and
/// the mesh's own binding follows it.
#[test]
fn an_animated_node_joins_the_skeleton() {
    let geometry = Geometry {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        indices: vec![0, 1, 2],
        ..Geometry::default()
    };
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Door", &[(&geometry, None)]);
    let door = writer.add_node(json!({ "name": "Door", "mesh": mesh, "translation": [2.0, 0.0, 0.0] }));
    writer.add_root(door);
    animate(
        &mut writer,
        Some("Open"),
        &[Track {
            node: door,
            path: "translation",
            interpolation: "LINEAR",
            times: &[0.0, 1.0],
            values: &[2.0, 0.0, 0.0, 2.0, 3.0, 0.0],
        }],
    );
    let bytes = writer.to_glb();
    // Without clips, a static mesh as before.
    assert!(super::super::import_slice(&bytes, None).unwrap().skin.is_none());
    assert!(!super::super::has_skeleton(&bytes, false).unwrap());
    // With them, a rigid part of a skeleton of one joint.
    assert!(super::super::has_skeleton(&bytes, true).unwrap());
    let (_, _, scene) = import_scene(&bytes, None).unwrap();
    let skin = scene.mesh.skin.as_ref().expect("a skin");
    assert_eq!(skin.joints.iter().map(|joint| joint.name.as_str()).collect::<Vec<_>>(), ["Door"]);
    let cooked = Cooked::new(&bytes, 0, &exact());
    let (clip, joints) = (cooked.clip(), cooked.skeleton());
    assert_eq!(joints.names[0].as_str(), "Door");
    let lifted = sample_pose(clip, joints, 60.0).translation[0];
    assert!(lifted.distance(Vec3::new(2.0, 3.0, 0.0)) < 1e-3, "{lifted}");
}

#[test]
fn a_channel_outside_the_scene_is_dropped() {
    let mut writer = figure();
    let stray = writer.add_node(json!({ "name": "Stray" }));
    animate(
        &mut writer,
        None,
        &[
            Track {
                node: stray,
                path: "translation",
                interpolation: "LINEAR",
                times: &[0.0, 9.0],
                values: &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            },
            Track {
                node: HIPS,
                path: "translation",
                interpolation: "LINEAR",
                times: &[0.0, 1.0],
                values: &[0.0, 0.25, 0.0, 1.0, 0.25, 0.0],
            },
        ],
    );
    let cooked = Cooked::new(&writer.to_glb(), 0, &exact());
    let (clip, joints) = (cooked.clip(), cooked.skeleton());
    assert_eq!(clip.ticks.to_native(), 60, "it doesn't lengthen the clip either");
    assert_eq!(joints.len(), 3, "and isn't a joint");
    assert_eq!(clip.translations.len(), 1);
}

/// Parts of a character whose morph targets merge by name are driven together, by the first channel.
#[test]
fn a_morph_channel_drives_every_target_its_mesh_merged_into() {
    let shape = || MorphGeometry { positions: vec![[0.0, 0.1, 0.0]; 3], normals: Vec::new() };
    let geometry = |targets: Vec<MorphGeometry>| Geometry {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        targets,
        indices: vec![0, 1, 2],
        ..Geometry::default()
    };
    let mut writer = GltfWriter::default();
    let face = writer.add_mesh("Face", &[(&geometry(vec![shape(), shape()]), None)]);
    writer.set_mesh_field(face, "extras", json!({ "targetNames": ["blink", "smile"] }));
    let lashes = writer.add_mesh("Lashes", &[(&geometry(vec![shape()]), None)]);
    writer.set_mesh_field(lashes, "extras", json!({ "targetNames": ["blink"] }));
    let face_node = writer.add_node(json!({ "name": "Face", "mesh": face }));
    let lashes_node = writer.add_node(json!({ "name": "Lashes", "mesh": lashes }));
    writer.add_root(face_node);
    writer.add_root(lashes_node);
    animate(
        &mut writer,
        Some("Blink"),
        &[
            Track {
                node: face_node,
                path: "weights",
                interpolation: "LINEAR",
                times: &[0.0, 1.0],
                values: &[0.0, 0.25, 1.0, 0.75],
            },
            Track {
                node: lashes_node,
                path: "weights",
                interpolation: "LINEAR",
                times: &[0.0, 1.0],
                values: &[0.0, 0.5],
            },
        ],
    );
    let cooked = Cooked::new(&writer.to_glb(), 0, &exact());
    assert!(cooked.skeleton.is_none(), "morph weights move no node");
    let clip = cooked.clip();
    assert!(clip.skeleton.is_none());
    assert_eq!(clip.morph_target_count.to_native(), 2);
    let tracks: Vec<u16> = clip.weights.iter().map(|track| track.target.to_native()).collect();
    assert_eq!(tracks, [0, 1], "blink and smile, by the face's channel");
    assert!((clip.weight(&clip.weights[0], 60.0) - 1.0).abs() < 1e-3, "the first channel wins");
    assert!((clip.weight(&clip.weights[1], 60.0) - 0.75).abs() < 1e-3);
    assert!((clip.weight(&clip.weights[1], 0.0) - 0.25).abs() < 1e-3);
}

#[test]
fn root_motion_comes_from_the_named_bone() {
    let mut writer = figure();
    animate(
        &mut writer,
        Some("Dash"),
        &[Track {
            node: HIPS,
            path: "translation",
            interpolation: "LINEAR",
            times: &[0.0, 1.0],
            values: &[0.0, 0.25, 0.0, 0.0, 0.5, 3.0],
        }],
    );
    let settings = ClipSettings {
        root_motion: Some(RootMotionSettings {
            bone: "Hips".into(),
            translation: RootTranslation::Ground,
            rotation: RootRotation::Yaw,
        }),
        ..exact()
    };
    let cooked = Cooked::new(&writer.to_glb(), 0, &settings);
    let (clip, joints) = (cooked.clip(), cooked.skeleton());
    let root = clip.root.as_ref().expect("root motion");
    assert_eq!(root.joint.to_native(), 1);
    assert!(Vec3::from(clip.vector(&root.translation, 60.0)).distance(Vec3::new(0.0, 0.0, 3.0)) < 1e-3);
    let pose = sample_pose(clip, joints, 60.0);
    assert!(pose.translation[1].distance(Vec3::new(0.0, 0.5, 0.0)) < 1e-3, "height stays in the bone");
    let hips = root_of(clip, 60.0) * model_of(&pose, joints)[1];
    assert!(
        Vec3::from(hips.translation).distance(Vec3::new(0.0, 1.5, 3.0)) < 1e-3,
        "the whole motion, recomposed: {hips:?}"
    );
    assert!(root_of(clip, 0.0).abs_diff_eq(Affine3A::IDENTITY, 1e-3));

    let settings = ClipSettings {
        root_motion: Some(RootMotionSettings {
            bone: "Tail".into(),
            translation: RootTranslation::Ground,
            rotation: RootRotation::Yaw,
        }),
        ..exact()
    };
    let (document, buffers, scene) = import_scene(&writer.to_glb(), None).unwrap();
    let error = sample(&document, &buffers, scene.clip_scene(), 0, &settings).unwrap_err();
    assert!(error.to_string().contains("Tail"), "{error}");
}

fn long_animation(writer: &mut GltfWriter, keys: usize) {
    let times: Vec<f32> = (0..keys).map(|key| key as f32 * 0.25).collect();
    let values: Vec<f32> = (0..keys).flat_map(|key| [key as f32, 0.25, 0.0]).collect();
    animate(
        writer,
        None,
        &[Track { node: HIPS, path: "translation", interpolation: "LINEAR", times: &times, values: &values }],
    );
}

#[test]
fn budgets_hold_before_data_is_read() {
    let mut writer = figure();
    long_animation(&mut writer, 40);
    let bytes = writer.to_glb();
    let (document, buffers, scene) = import_scene(&bytes, None).unwrap();
    let settings = ClipSettings::default();
    let within = |budget| {
        sample_within(&document, &buffers, scene.clip_scene(), 0, &settings, budget).map(|clip| clip.ticks)
    };
    let roomy = CLIP_BUDGET;
    assert_eq!(within(roomy).unwrap(), 585, "39 × 0.25 s");
    let error = within(ClipBudget { keys: 39, ..roomy }).unwrap_err();
    assert!(matches!(error, ClipImportError::OverBudget { what: "keys", found: 40, .. }), "{error}");
    // 40 times, and 40 values of 3.
    let error = within(ClipBudget { values: 159, ..roomy }).unwrap_err();
    let ClipImportError::OverBudget { found: 160, .. } = error else { panic!("{error}") };
    within(ClipBudget { values: 160, ..roomy }).expect("at the budget");
    // 586 ticks of 3.
    let error = within(ClipBudget { samples: 1757, ..roomy }).unwrap_err();
    assert!(matches!(error, ClipImportError::TooManySamples { found: 1758, .. }), "{error}");
    within(ClipBudget { samples: 1758, ..roomy }).expect("at the budget");
}

/// Channels may share an accessor, and a sparse one costs a file nothing, so the sum is what counts.
#[test]
fn the_values_budget_counts_every_channel_of_an_animation() {
    let mut writer = figure();
    let track = |node| Track {
        node,
        path: "translation",
        interpolation: "LINEAR",
        times: &[0.0, 1.0],
        values: &[0.0, 0.25, 0.0, 1.0, 0.25, 0.0],
    };
    animate(&mut writer, None, &[track(HIPS), track(ARM)]);
    let (document, buffers, scene) = import_scene(&writer.to_glb(), None).unwrap();
    let settings = ClipSettings::default();
    let within = |budget| {
        sample_within(&document, &buffers, scene.clip_scene(), 0, &settings, budget).map(|clip| clip.ticks)
    };
    // Each channel is 2 times and 2 values of 3.
    let error = within(ClipBudget { values: 15, ..CLIP_BUDGET }).unwrap_err();
    let ClipImportError::OverBudget { at, found: 16, .. } = error else { panic!("{error}") };
    assert_eq!(at.channel, 1, "the channel that went over");
    within(ClipBudget { values: 16, ..CLIP_BUDGET }).expect("at the budget");
}

/// A few bytes of JSON can give a mesh many targets, and a channel on it holds one each a tick.
#[test]
fn the_samples_budget_counts_every_morph_target() {
    let shape = || MorphGeometry { positions: vec![[0.0, 0.1, 0.0]; 3], normals: Vec::new() };
    let geometry = Geometry {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        targets: vec![shape(), shape(), shape()],
        indices: vec![0, 1, 2],
        ..Geometry::default()
    };
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Face", &[(&geometry, None)]);
    let node = writer.add_node(json!({ "name": "Face", "mesh": mesh }));
    writer.add_root(node);
    animate(
        &mut writer,
        None,
        &[Track {
            node,
            path: "weights",
            interpolation: "LINEAR",
            times: &[0.0, 1.0],
            values: &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        }],
    );
    let (document, buffers, scene) = import_scene(&writer.to_glb(), None).unwrap();
    let settings = ClipSettings::default();
    let within = |budget| {
        sample_within(&document, &buffers, scene.clip_scene(), 0, &settings, budget)
            .map(|clip| clip.weights.len())
    };
    // 61 ticks of the channel's 3, and of the 3 targets they go to.
    let error = within(ClipBudget { samples: 365, ..CLIP_BUDGET }).unwrap_err();
    assert!(matches!(error, ClipImportError::TooManySamples { found: 366, .. }), "{error}");
    assert_eq!(within(ClipBudget { samples: 366, ..CLIP_BUDGET }).expect("at the budget"), 3);
}

#[test]
fn the_animations_budget_holds_before_a_cook_plans_them() {
    let mut writer = figure();
    let track = || Track {
        node: HIPS,
        path: "translation",
        interpolation: "LINEAR",
        times: &[0.0, 1.0],
        values: &[0.0, 0.25, 0.0, 1.0, 0.25, 0.0],
    };
    animate(&mut writer, None, &[track()]);
    let mut document = writer.document(None);
    let animation = document["animations"][0].clone();
    document["animations"] = json!(vec![animation; CLIP_BUDGET.animations + 1]);
    let bytes = crate::gltf_writer::glb(&document, writer.bin());
    let error = names(&bytes).unwrap_err();
    assert!(matches!(error, ClipImportError::TooManyAnimations { found: 4097, .. }), "{error}");
}

/// A root under a scale of zero has no rest of its motion: an error naming the bone.
#[test]
fn a_root_bone_under_a_scale_of_zero_is_an_error() {
    let mut writer = figure();
    animate(
        &mut writer,
        None,
        &[Track {
            node: ROOT,
            path: "scale",
            interpolation: "LINEAR",
            times: &[0.0, 1.0],
            values: &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        }],
    );
    let settings = ClipSettings {
        root_motion: Some(RootMotionSettings {
            bone: "Hips".into(),
            translation: RootTranslation::Ground,
            rotation: RootRotation::Yaw,
        }),
        ..ClipSettings::default()
    };
    let (document, buffers, scene) = import_scene(&writer.to_glb(), None).unwrap();
    let error = sample(&document, &buffers, scene.clip_scene(), 0, &settings).unwrap_err();
    assert!(error.to_string().contains("\"Hips\"") && error.to_string().contains("scale of zero"), "{error}");
}

/// glTF lets two joints share a name, and then the name says neither of them: root motion from whichever
/// came first would move the wrong bone without a word.
#[test]
fn root_motion_from_a_name_two_joints_share_is_refused() {
    let joint = |name: &str, parent| Joint { name: name.to_owned(), parent, rest: JointTransform::IDENTITY };
    let joints = [joint("Root", None), joint("Twin", Some(0)), joint("Twin", Some(0))];
    let settings = |bone: &str| ClipSettings {
        root_motion: Some(RootMotionSettings {
            bone: bone.to_owned(),
            translation: RootTranslation::Ground,
            rotation: RootRotation::Yaw,
        }),
        ..ClipSettings::default()
    };
    let mut clip = SampledClip { name: "walk".to_owned(), rate: 60, ticks: 1, ..SampledClip::default() };
    let error = lift_settings_root(&mut clip, &joints, &settings("Twin")).unwrap_err();
    assert!(matches!(&error, ClipImportError::RepeatedRootBone { bone } if bone == "Twin"), "{error}");
    assert!(clip.root.is_none(), "it leaves the clip as it was");
    assert!(lift_settings_root(&mut clip, &joints, &settings("Root")).is_ok(), "the name that's one joint's");
}

/// The length is checked before a track is sampled: a few bytes can claim a year.
#[test]
fn a_clip_too_long_for_a_u16_of_ticks_is_refused() {
    let mut writer = figure();
    animate(
        &mut writer,
        None,
        &[Track {
            node: HIPS,
            path: "translation",
            interpolation: "LINEAR",
            times: &[0.0, 3.0e7],
            values: &[0.0, 0.25, 0.0, 1.0, 0.25, 0.0],
        }],
    );
    let (document, buffers, scene) = import_scene(&writer.to_glb(), None).unwrap();
    let error = sample(&document, &buffers, scene.clip_scene(), 0, &ClipSettings::default()).unwrap_err();
    assert!(matches!(error, ClipImportError::TooLong { .. }), "{error}");
}

#[test]
fn morph_channels_follow_selected_nodes_and_their_split_defaults() {
    for excluded_first in [false, true] {
        let mut writer = GltfWriter::default();
        let geometry = Geometry {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            targets: vec![MorphGeometry { positions: vec![[0.0, 0.1, 0.0]; 3], normals: Vec::new() }],
            indices: vec![0, 1, 2],
            ..Geometry::default()
        };
        let mesh = writer.add_mesh("Face", &[(&geometry, None)]);
        let excluded = writer.add_node(json!({ "name": "Excluded", "mesh": mesh }));
        let first = writer.add_node(json!({ "name": "First", "mesh": mesh, "weights": [0.0] }));
        let second = writer.add_node(json!({ "name": "Second", "mesh": mesh, "weights": [0.5] }));
        writer.add_root(first);
        writer.add_root(second);
        let track = |node, values| Track {
            node,
            path: "weights",
            interpolation: "LINEAR",
            times: &[0.0, 1.0],
            values,
        };
        let mut tracks = vec![track(first, &[0.0, 0.25]), track(second, &[0.5, 0.75])];
        if excluded_first {
            tracks.insert(0, track(excluded, &[0.0, 1.0]));
        } else {
            tracks.push(track(excluded, &[0.0, 1.0]));
        }
        animate(&mut writer, Some("Expression"), &tracks);
        let cooked = Cooked::new(&writer.to_glb(), 0, &exact());
        let clip = cooked.clip();
        assert_eq!(clip.weights.len(), 2);
        assert!((clip.weight(&clip.weights[0], 60.0) - 0.25).abs() < 1e-3);
        assert!((clip.weight(&clip.weights[1], 60.0) - 0.75).abs() < 1e-3);
    }
}

#[test]
fn finite_keys_cannot_publish_nonfinite_samples() {
    let mut writer = figure();
    animate(
        &mut writer,
        Some("Overflow"),
        &[Track {
            node: HIPS,
            path: "translation",
            interpolation: "LINEAR",
            times: &[0.0, 1.0],
            values: &[-f32::MAX, 0.0, 0.0, f32::MAX, 0.0, 0.0],
        }],
    );
    let (document, buffers, scene) = import_scene(&writer.to_glb(), None).unwrap();
    let error = sample(&document, &buffers, scene.clip_scene(), 0, &exact()).unwrap_err();
    assert!(matches!(error, ClipImportError::NonFiniteChannelSample { tick: 1, .. }));
}

#[test]
fn shifting_a_clip_to_a_large_source_time_preserves_subtick_motion() {
    let sampled = |start: f32| {
        let mut writer = figure();
        animate(
            &mut writer,
            Some("Shifted"),
            &[Track {
                node: HIPS,
                path: "translation",
                interpolation: "LINEAR",
                times: &[start, start + 2.0],
                values: &[0.0, 0.0, 0.0, 120.0, 0.0, 0.0],
            }],
        );
        let (document, buffers, scene) = import_scene(&writer.to_glb(), None).unwrap();
        sample(&document, &buffers, scene.clip_scene(), 0, &exact()).unwrap()
    };
    let reference = sampled(0.0);
    for start in [1.0, 4096.0, 16_777_216.0] {
        let shifted = sampled(start);
        assert_eq!(shifted.ticks, reference.ticks);
        assert_eq!(shifted.translations, reference.translations);
    }
}

#[test]
fn equal_default_mesh_instances_keep_independent_animation_and_static_siblings() {
    for (animate_first, animate_second) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut writer = GltfWriter::default();
        let geometry = Geometry {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            targets: vec![MorphGeometry { positions: vec![[0.0, 0.1, 0.0]; 3], normals: Vec::new() }],
            indices: vec![0, 1, 2],
            ..Geometry::default()
        };
        // Both primitives of a node must reuse its one target, while another node's channel
        // must never animate these vertices, including when that other node is excluded.
        let mesh = writer.add_mesh("Face", &[(&geometry, None), (&geometry, None)]);
        let excluded = writer.add_node(json!({ "name": "Excluded", "mesh": mesh }));
        let first = writer.add_node(json!({ "name": "First", "mesh": mesh, "weights": [0.0] }));
        let second = writer.add_node(json!({ "name": "Second", "mesh": mesh, "weights": [0.0] }));
        writer.add_root(first);
        writer.add_root(second);
        let track = |node, values| Track {
            node,
            path: "weights",
            interpolation: "LINEAR",
            times: &[0.0, 1.0],
            values,
        };
        let mut tracks = vec![track(excluded, &[0.0, 0.5])];
        if animate_first {
            tracks.push(track(first, &[0.0, 1.0]));
        }
        if animate_second {
            tracks.push(track(second, &[0.0, -1.0]));
        }
        animate(&mut writer, Some("Independent"), &tracks);
        let (document, buffers, scene) = import_scene(&writer.to_glb(), None).unwrap();
        let first_target = scene.targets_of_node[&(first, 0)];
        let second_target = scene.targets_of_node[&(second, 0)];
        assert!(!scene.targets_of_node.contains_key(&(excluded, 0)));
        let animated = animate_first || animate_second;
        assert_eq!(first_target != second_target, animated);
        assert_eq!(scene.mesh.morph_targets.len(), if animated { 2 } else { 1 });
        assert_eq!(scene.targets_of_morph[&(mesh, 0)].len(), if animated { 2 } else { 1 });
        if animated {
            let first_deltas = &scene.mesh.morph_targets[first_target as usize].deltas;
            let second_deltas = &scene.mesh.morph_targets[second_target as usize].deltas;
            assert_eq!(
                first_deltas.iter().map(|delta| delta.vertex).collect::<Vec<_>>(),
                (0..6).collect::<Vec<_>>()
            );
            assert_eq!(
                second_deltas.iter().map(|delta| delta.vertex).collect::<Vec<_>>(),
                (6..12).collect::<Vec<_>>()
            );
        } else {
            assert_eq!(scene.mesh.morph_targets[0].deltas.len(), 12, "unanimated equal defaults still merge");
        }
        let sampled = sample(&document, &buffers, scene.clip_scene(), 0, &exact()).unwrap();
        for (target, animated, end) in
            [(first_target, animate_first, 1.0), (second_target, animate_second, -1.0)]
        {
            let track = sampled.weights.iter().find(|(index, _)| u32::from(*index) == target);
            if animated {
                assert_eq!(*track.unwrap().1.last().unwrap(), end);
            } else {
                assert!(track.is_none(), "a static instance has no animation track");
            }
        }
    }
}
