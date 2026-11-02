//! FBX stacks against what their glTF twins sample to (`tools/blender/fbx_fixtures.py`), and
//! hand-written files for what Blender doesn't write: other axes and units, pre- and post-rotations,
//! blend weights and bad files.

use std::path::Path;

use glam::{Quat, Vec3};
use sapphire_assets::skeleton::{Joint, Skeleton};

use super::*;
use crate::clip::tests::angle_between;
use crate::clip::{ClipSettings, RootMotionSettings};
use crate::import::fbx::tests::{Y_UP, ascii_fbx};
use crate::import::gltf::clips as gltf_clips;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fbx").join(name)).unwrap()
}

/// A joint's sampled local transform at `tick`, its rest where the clip has no track for it.
fn local_at(clip: &SampledClip, joints: &[Joint], joint: usize, tick: usize) -> (Vec3, Quat, Vec3) {
    let id = joint as u16;
    let rest = &joints[joint].rest;
    let vector = |tracks: &[(u16, Vec<Vec3>)], or: [f32; 3]| {
        tracks.iter().find(|(track, _)| *track == id).map_or(Vec3::from(or), |(_, samples)| samples[tick])
    };
    let rotation = clip
        .rotations
        .iter()
        .find(|(track, _)| *track == id)
        .map_or(Quat::from_array(rest.rotation), |(_, samples)| samples[tick]);
    (vector(&clip.translations, rest.translation), rotation.normalize(), vector(&clip.scales, rest.scale))
}

/// The glTF twin of an animated fixture: its joints and each animation, sampled.
fn gltf_twin(name: &str, settings: &ClipSettings) -> (Vec<Joint>, Vec<SampledClip>) {
    let bytes = fixture(name);
    let (document, buffers, scene) = gltf_clips::import_scene(&bytes, None).unwrap();
    let clips = (0..document.animations().len())
        .map(|index| gltf_clips::sample(&document, &buffers, scene.clip_scene(), index, settings).unwrap())
        .collect();
    (scene.mesh.skin.unwrap().joints, clips)
}

/// Each joint's model-space position and rotation in `clip` at `tick`. Blender writes an FBX's armature
/// turned to Z up, with its bones' axes to match, and each bone's node at the pose of the frame it
/// exported on, where its glTF has the armature's rest. So two files agree in model space, and only up
/// to a turn per joint that stays the same over a clip.
fn model_at(clip: &SampledClip, joints: &[Joint], tick: usize) -> Vec<(Vec3, Quat)> {
    let mut model: Vec<(glam::Affine3A, Quat)> = Vec::with_capacity(joints.len());
    for (joint, entry) in joints.iter().enumerate() {
        let (translation, rotation, scale) = local_at(clip, joints, joint, tick);
        let local = glam::Affine3A::from_scale_rotation_translation(scale, rotation, translation);
        model.push(match entry.parent {
            Some(parent) => {
                let (affine, turned) = model[usize::from(parent)];
                (affine * local, turned * rotation)
            }
            None => (local, rotation),
        });
    }
    model.into_iter().map(|(affine, rotation)| (Vec3::from(affine.translation), rotation)).collect()
}

#[test]
fn an_fbx_samples_to_what_its_gltf_twin_does() {
    let settings = ClipSettings::default();
    let clips = FbxClips::import(&fixture("animated_clips.fbx"), true).unwrap();
    let (gltf_joints, gltf_clips) = gltf_twin("animated_clips.glb", &settings);

    // The same rig: names and parents.
    let by_name = |joints: &[Joint], name: &str| joints.iter().position(|joint| joint.name == name);
    assert_eq!(clips.joints().len(), gltf_joints.len());
    for joint in clips.joints() {
        let twin = &gltf_joints[by_name(&gltf_joints, &joint.name).expect(&joint.name)];
        let parent = |joints: &[Joint], joint: &Joint| {
            joint.parent.map(|parent| joints[usize::from(parent)].name.clone())
        };
        assert_eq!(parent(clips.joints(), joint), parent(&gltf_joints, twin), "{}", joint.name);
    }

    assert_eq!(names_of(&clips.scene).len(), gltf_clips.len());
    // The turn between a joint's two sets of axes, which is all a rotation may differ by.
    let mut axes: Vec<Option<Quat>> = vec![None; clips.joints().len()];
    for (index, name) in names_of(&clips.scene).iter().enumerate() {
        let twin = gltf_clips.iter().find(|clip| name.ends_with(&clip.name)).expect(name);
        let clip = clips.sample(index, &settings).unwrap();
        assert_eq!(clip.ticks, twin.ticks, "{name}");
        assert!(clip.ticks > 0, "{name}");
        // Both write every frame, at 30 a second: every other tick is on a key.
        for tick in (0..=usize::from(clip.ticks)).step_by(2) {
            let (own, other) = (model_at(&clip, clips.joints(), tick), model_at(twin, &gltf_joints, tick));
            for (joint, entry) in clips.joints().iter().enumerate() {
                let twin_joint = by_name(&gltf_joints, &entry.name).unwrap();
                let (position, rotation) = own[joint];
                let (twin_position, twin_rotation) = other[twin_joint];
                let at = format!("{name} {} tick {tick}", entry.name);
                assert!(position.distance(twin_position) < 2e-4, "{at}: {position} against {twin_position}");
                let turn = twin_rotation.inverse() * rotation;
                let first = *axes[joint].get_or_insert(turn);
                assert!(angle_between(turn, first) < 2e-3, "{at}: {turn} against {first}");
            }
        }
    }
    // What this proves depends on the motion being there.
    let stride = gltf_clips.iter().find(|clip| clip.name == "Stride").unwrap();
    let hips = by_name(&gltf_joints, "Hips").unwrap();
    let moved =
        model_at(stride, &gltf_joints, 30)[hips].0.distance(model_at(stride, &gltf_joints, 0)[hips].0);
    assert!(moved > 0.2, "the hips travel: {moved}");
    let arm = by_name(&gltf_joints, "Arm").unwrap();
    let stretched = local_at(stride, &gltf_joints, arm, 30).2;
    assert!((stretched.y - 1.4).abs() < 1e-3, "the arm stretches: {stretched}");
}

#[test]
fn a_half_turn_lifts_into_root_motion_as_the_gltf_twins_does() {
    let settings = ClipSettings {
        root_motion: Some(RootMotionSettings {
            bone: "Hips".to_owned(),
            translation: Default::default(),
            rotation: Default::default(),
        }),
        ..ClipSettings::default()
    };
    let clips = FbxClips::import(&fixture("animated_clips.fbx"), true).unwrap();
    let (_, gltf_clips) = gltf_twin("animated_clips.glb", &settings);
    let spin = names_of(&clips.scene).iter().position(|name| name.ends_with("Spin")).unwrap();
    let clip = clips.sample(spin, &settings).unwrap();
    let root = clip.root.as_ref().expect("lifted");
    let twin = gltf_clips.iter().find(|clip| clip.name == "Spin").unwrap().root.as_ref().unwrap();
    assert_eq!(root.rotation.len(), twin.rotation.len());
    for (tick, (own, other)) in root.rotation.iter().zip(&twin.rotation).enumerate().step_by(2) {
        assert!(angle_between(*own, *other) < 2e-3, "tick {tick}: {own} against {other}");
    }
    let end = *root.rotation.last().unwrap();
    assert!(end.dot(Quat::from_rotation_y(std::f32::consts::PI)).abs() > 0.999, "a half turn: {end}");
}

#[test]
fn a_mesh_with_clips_has_every_animated_node_in_its_skeleton() {
    // Its mesh and its clips must agree on the joints, whichever import reads them.
    let bytes = fixture("animated_clips.fbx");
    let clips = FbxClips::import(&bytes, true).unwrap();
    let mesh = import_mesh(&bytes).unwrap();
    let names = |joints: &[Joint]| joints.iter().map(|joint| joint.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(clips.joints()), names(&mesh.skin.unwrap().joints));
    let (_, planned) = plan(&bytes).unwrap();
    assert_eq!(names(clips.joints()), names(&planned));
}

/// An ASCII FBX with one node, `Mover`, whose curves are `curves` (their objects) over a stack of
/// `begin` to `end` seconds. FBX counts time in ticks of 1/46186158000 of a second.
fn one_node(objects: &str, connections: &str, range: Option<(f64, f64)>) -> Vec<u8> {
    let ktime = |seconds: f64| (seconds * 46_186_158_000.0) as i64;
    let local = range.map_or(String::new(), |(begin, end)| {
        format!(
            "\t\t\tP: \"LocalStart\", \"KTime\", \"Time\", \"\",{}\n\t\t\tP: \"LocalStop\", \"KTime\", \"Time\", \"\",{}\n",
            ktime(begin),
            ktime(end)
        )
    });
    let stack = format!(
        "\tAnimationStack: 900, \"AnimStack::Take\", \"\" {{\n\t\tProperties70:  {{\n{local}\t\t}}\n\t}}\n\
         \tAnimationLayer: 901, \"AnimLayer::Base\", \"\" {{\n\t}}\n"
    );
    let objects = format!("{stack}{objects}");
    let connections = format!("\tC: \"OO\",901,900\n{connections}");
    ascii_fbx(Y_UP, 100.0, &objects, &connections)
}

/// A curve node for `property` of model `model` with curves on X, Y and Z, each given as seconds and
/// values: its objects and connections.
fn curves(
    id: u64,
    model: u64,
    property: &str,
    x: &[(f64, f64)],
    y: &[(f64, f64)],
    z: &[(f64, f64)],
) -> (String, String) {
    let ktime = |seconds: f64| (seconds * 46_186_158_000.0) as i64;
    let mut objects = format!(
        "\tAnimationCurveNode: {id}, \"AnimCurveNode::{property}\", \"\" {{\n\t\tProperties70:  {{\n\
         \t\t\tP: \"d|X\", \"Number\", \"\", \"A\",0\n\t\t\tP: \"d|Y\", \"Number\", \"\", \"A\",0\n\
         \t\t\tP: \"d|Z\", \"Number\", \"\", \"A\",0\n\t\t}}\n\t}}\n"
    );
    let mut connections = format!("\tC: \"OO\",{id},901\n\tC: \"OP\",{id},{model}, \"{property}\"\n");
    for (axis, (name, keys)) in [("d|X", x), ("d|Y", y), ("d|Z", z)].into_iter().enumerate() {
        let curve = id + 1 + axis as u64;
        let times: Vec<String> = keys.iter().map(|(time, _)| ktime(*time).to_string()).collect();
        let values: Vec<String> = keys.iter().map(|(_, value)| value.to_string()).collect();
        objects += &format!(
            "\tAnimationCurve: {curve}, \"AnimCurve::\", \"\" {{\n\t\tDefault: 0\n\t\tKeyVer: 4008\n\
             \t\tKeyTime: *{n} {{\n\t\t\ta: {}\n\t\t}}\n\t\tKeyValueFloat: *{n} {{\n\t\t\ta: {}\n\t\t}}\n\
             \t\tKeyAttrFlags: *1 {{\n\t\t\ta: 4\n\t\t}}\n\t\tKeyAttrDataFloat: *4 {{\n\t\t\ta: 0,0,0,0\n\t\t}}\n\
             \t\tKeyAttrRefCount: *1 {{\n\t\t\ta: {n}\n\t\t}}\n\t}}\n",
            times.join(","),
            values.join(","),
            n = keys.len(),
        );
        connections += &format!("\tC: \"OP\",{curve},{id}, \"{name}\"\n");
    }
    (objects, connections)
}

/// A model with a translation and whatever `properties` it needs besides.
fn model(id: u64, name: &str, properties: &str) -> String {
    format!(
        "\tModel: {id}, \"Model::{name}\", \"LimbNode\" {{\n\t\tVersion: 232\n\t\tProperties70:  {{\n{properties}\t\t}}\n\t}}\n"
    )
}

#[test]
fn a_stack_runs_over_its_own_range_or_its_keys() {
    let (objects, connections) =
        curves(200, 100, "Lcl Translation", &[(0.5, 0.0), (1.5, 100.0)], &[(0.5, 0.0)], &[(0.5, 0.0)]);
    let objects = model(100, "Mover", "") + &objects;
    let connections = format!("\tC: \"OO\",100,0\n{connections}");

    // No range: its keys', starting at the earliest.
    let clips = FbxClips::import(&one_node(&objects, &connections, None), false).unwrap();
    let clip = clips.sample(0, &ClipSettings::default()).unwrap();
    assert_eq!(clip.ticks, 60);
    let (_, samples) = &clip.translations[0];
    // A unit of a meter here (100 cm a unit): a key's 100 is 100 m.
    assert!(samples[0].distance(Vec3::ZERO) < 1e-3, "{}", samples[0]);
    assert!(samples[60].distance(Vec3::new(100.0, 0.0, 0.0)) < 1e-2, "{}", samples[60]);

    // A range of its own: what plays. A clip is cut to it.
    let clips = FbxClips::import(&one_node(&objects, &connections, Some((1.0, 1.25))), false).unwrap();
    let clip = clips.sample(0, &ClipSettings::default()).unwrap();
    assert_eq!(clip.ticks, 15);
    let (_, samples) = &clip.translations[0];
    assert!(samples[0].distance(Vec3::new(50.0, 0.0, 0.0)) < 1e-2, "{}", samples[0]);
    assert!(samples[15].distance(Vec3::new(75.0, 0.0, 0.0)) < 1e-2, "{}", samples[15]);
}

#[test]
fn units_axes_and_pre_and_post_rotations_reach_a_track_as_they_reach_the_rest() {
    // Z up and in centimeters, with a joint under a parent, its rest turned by pre- and post-rotations
    // and a pivot of the sort other tools write: each tick's whole-body pose must be the one the same
    // numbers in a rest pose would be.
    let z_up = [2, 1, 1, -1, 0, 1];
    let properties = "\t\t\tP: \"PreRotation\", \"Vector3D\", \"Vector\", \"\",0,0,90\n\
                      \t\t\tP: \"PostRotation\", \"Vector3D\", \"Vector\", \"\",0,30,0\n\
                      \t\t\tP: \"RotationActive\", \"bool\", \"\", \"\",1\n\
                      \t\t\tP: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",10,20,30\n";
    let (objects, connections) = curves(
        200,
        101,
        "Lcl Rotation",
        &[(0.0, 0.0), (1.0, 40.0)],
        &[(0.0, 0.0), (1.0, 0.0)],
        &[(0.0, 0.0), (1.0, 0.0)],
    );
    let objects =
        model(100, "Top", "\t\t\tP: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",100,0,0\n")
            + &model(101, "Mover", properties)
            + &objects;
    let connections = format!("\tC: \"OO\",100,0\n\tC: \"OO\",101,100\n{connections}");
    let ktime = |seconds: f64| (seconds * 46_186_158_000.0) as i64;
    let stack = format!(
        "\tAnimationStack: 900, \"AnimStack::Take\", \"\" {{\n\t\tProperties70:  {{\n\
         \t\t\tP: \"LocalStart\", \"KTime\", \"Time\", \"\",0\n\
         \t\t\tP: \"LocalStop\", \"KTime\", \"Time\", \"\",{}\n\t\t}}\n\t}}\n\
         \tAnimationLayer: 901, \"AnimLayer::Base\", \"\" {{\n\t}}\n",
        ktime(1.0)
    );
    let bytes =
        ascii_fbx(z_up, 1.0, &format!("{stack}{objects}"), &format!("\tC: \"OO\",901,900\n{connections}"));
    let clips = FbxClips::import(&bytes, false).unwrap();
    let skeleton = Skeleton::new(clips.joints());
    let names: Vec<&str> = skeleton.names.iter().map(String::as_str).collect();
    assert_eq!(names, ["Top", "Mover"]);

    // At rest the animation is at its first key, which holds the file's own rotation of 0, so the
    // first tick is the rest pose: the conversion and the rotations reach a track as they do a rest.
    let clip = clips.sample(0, &ClipSettings::default()).unwrap();
    let (translation, rotation, scale) = local_at(&clip, clips.joints(), 1, 0);
    let rest = &clips.joints()[1].rest;
    assert!(translation.distance(Vec3::from(rest.translation)) < 1e-5);
    assert!(angle_between(rotation, Quat::from_array(rest.rotation)) < 1e-4);
    assert!(scale.distance(Vec3::from(rest.scale)) < 1e-5);

    // The top node carries the unit: the joint's translation is in centimeters under its 0.01.
    let top = &clips.joints()[0].rest;
    assert!((top.scale[0] - 0.01).abs() < 1e-6, "{:?}", top.scale);
    assert!(Vec3::from(top.translation).distance(Vec3::new(1.0, 0.0, 0.0)) < 1e-5, "{:?}", top.translation);
    assert!(
        Vec3::from(rest.translation).distance(Vec3::new(10.0, 20.0, 30.0)) < 1e-4,
        "{:?}",
        rest.translation
    );

    // And the key's turn lands between the pre- and post-rotations: R = pre · Rx(40°) · post⁻¹.
    let pre = Quat::from_rotation_z(90f32.to_radians());
    let post = Quat::from_rotation_y(30f32.to_radians());
    let (_, end, _) = local_at(&clip, clips.joints(), 1, 60);
    let expected = pre * Quat::from_rotation_x(40f32.to_radians()) * post.inverse();
    assert!(angle_between(end, expected) < 1e-3, "{end} against {expected}");
}

/// The named blend shape target's blend channel on a small mesh, with `keys` on its `DeformPercent`.
fn morphing_mesh(keys: &[(f64, f64)]) -> Vec<u8> {
    let ktime = |seconds: f64| (seconds * 46_186_158_000.0) as i64;
    let times: Vec<String> = keys.iter().map(|(time, _)| ktime(*time).to_string()).collect();
    let values: Vec<String> = keys.iter().map(|(_, value)| value.to_string()).collect();
    let objects = format!(
        "\tGeometry: 101, \"Geometry::Plate\", \"Mesh\" {{\n\
         \t\tVertices: *9 {{ a: 0,0,0,1,0,0,0,1,0 }}\n\
         \t\tPolygonVertexIndex: *3 {{ a: 0,1,-3 }}\n\t}}\n\
         \tModel: 100, \"Model::Plate\", \"Mesh\" {{\n\t\tVersion: 232\n\t}}\n\
         \tDeformer: 300, \"Deformer::Shapes\", \"BlendShape\" {{\n\t\tVersion: 100\n\t}}\n\
         \tDeformer: 301, \"SubDeformer::Smile\", \"BlendShapeChannel\" {{\n\t\tVersion: 100\n\t\tDeformPercent: 0\n\t\tFullWeights: *1 {{ a: 100 }}\n\t}}\n\
         \tGeometry: 302, \"Geometry::Smile\", \"Shape\" {{\n\t\tVersion: 100\n\
         \t\tIndexes: *1 {{ a: 0 }}\n\t\tVertices: *3 {{ a: 0,0,0.5 }}\n\t}}\n\
         \tAnimationStack: 900, \"AnimStack::Take\", \"\" {{\n\t}}\n\
         \tAnimationLayer: 901, \"AnimLayer::Base\", \"\" {{\n\t}}\n\
         \tAnimationCurveNode: 400, \"AnimCurveNode::DeformPercent\", \"\" {{\n\t\tProperties70:  {{\n\
         \t\t\tP: \"d|DeformPercent\", \"Number\", \"\", \"A\",0\n\t\t}}\n\t}}\n\
         \tAnimationCurve: 401, \"AnimCurve::\", \"\" {{\n\t\tDefault: 0\n\t\tKeyVer: 4008\n\
         \t\tKeyTime: *{n} {{\n\t\t\ta: {}\n\t\t}}\n\t\tKeyValueFloat: *{n} {{\n\t\t\ta: {}\n\t\t}}\n\
         \t\tKeyAttrFlags: *1 {{\n\t\t\ta: 4\n\t\t}}\n\t\tKeyAttrDataFloat: *4 {{\n\t\t\ta: 0,0,0,0\n\t\t}}\n\
         \t\tKeyAttrRefCount: *1 {{\n\t\t\ta: {n}\n\t\t}}\n\t}}\n",
        times.join(","),
        values.join(","),
        n = keys.len(),
    );
    let connections = "\tC: \"OO\",100,0\n\tC: \"OO\",101,100\n\tC: \"OO\",300,101\n\tC: \"OO\",301,300\n\
                       \tC: \"OO\",302,301\n\tC: \"OO\",901,900\n\tC: \"OO\",400,901\n\
                       \tC: \"OP\",400,301, \"DeformPercent\"\n\tC: \"OP\",401,400, \"d|DeformPercent\"\n";
    ascii_fbx(Y_UP, 100.0, &objects, connections)
}

#[test]
fn a_blend_channels_curve_is_a_morph_weight_track() {
    let bytes = morphing_mesh(&[(0.0, 0.0), (1.0, 50.0)]);
    let clips = FbxClips::import(&bytes, true).unwrap();
    assert_eq!(clips.morph_defaults(), [0.0]);
    assert!(clips.joints().is_empty(), "nothing moves a node");
    let clip = clips.sample(0, &ClipSettings::default()).unwrap();
    assert_eq!(clip.ticks, 60);
    let [(0, weights)] = &clip.weights[..] else { panic!("{:?}", clip.weights) };
    assert!(weights[0].abs() < 1e-5, "{}", weights[0]);
    assert!((weights[30] - 0.25).abs() < 1e-4, "{}", weights[30]);
    assert!((weights[60] - 0.5).abs() < 1e-5, "{}", weights[60]);
}

/// One bad FBX or setting, and what its error must say.
struct Case {
    name: &'static str,
    bytes: Vec<u8>,
    says: &'static str,
}

#[test]
fn malformed_animations_are_errors_not_panics() {
    let mover = |keys: &[(f64, f64)], range| {
        let (objects, connections) = curves(200, 100, "Lcl Translation", keys, &[(0.0, 0.0)], &[(0.0, 0.0)]);
        let objects = model(100, "Mover", "") + &objects;
        let connections = format!("\tC: \"OO\",100,0\n{connections}");
        one_node(&objects, &connections, range)
    };
    let cases = vec![
        Case {
            name: "a range of years",
            bytes: mover(&[(0.0, 0.0), (1.0, 1.0)], Some((0.0, 10_000_000.0))),
            says: "a clip holds at most",
        },
        Case {
            name: "not an FBX",
            bytes: b"Kaydara FBX Binary  nonsense".to_vec(),
            says: "not a readable FBX",
        },
        Case { name: "empty", bytes: Vec::new(), says: "not a readable FBX" },
    ];
    let mut wrong = Vec::new();
    for Case { name, bytes, says } in cases {
        let settings = ClipSettings::default();
        let result = FbxClips::import(&bytes, false).and_then(|clips| clips.sample(0, &settings).map(|_| ()));
        match result {
            Ok(()) => wrong.push(format!("{name}: it sampled")),
            Err(error) if !error.to_string().contains(says) => {
                wrong.push(format!("{name}: said {error}, which doesn't mention {says:?}"));
            }
            Err(_) => {}
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn the_samples_budget_holds_before_a_stack_is_evaluated() {
    let (objects, connections) =
        curves(200, 100, "Lcl Translation", &[(0.0, 0.0), (10.0, 1.0)], &[(0.0, 0.0)], &[(0.0, 0.0)]);
    let objects = model(100, "Mover", "") + &objects;
    let connections = format!("\tC: \"OO\",100,0\n{connections}");
    let clips = FbxClips::import(&one_node(&objects, &connections, None), false).unwrap();
    let settings = ClipSettings::default();
    // 601 ticks of 10 floats and a joint: 6,010.
    let budget = |samples| ClipBudget { samples, ..CLIP_BUDGET };
    assert!(clips.sample_within(0, &settings, budget(6010)).is_ok());
    let error = clips.sample_within(0, &settings, budget(6009)).unwrap_err();
    assert!(matches!(error, ClipImportError::TooManySamples { found: 6010, budget: 6009, .. }), "{error}");
}

#[test]
fn a_cycle_of_nodes_is_no_panic() {
    // ufbx refuses to follow what isn't a tree, in one way or another: this holds that it never
    // reaches the importer as a walk that doesn't end.
    let objects = model(100, "A", "") + &model(101, "B", "");
    let connections = "\tC: \"OO\",100,0\n\tC: \"OO\",101,100\n\tC: \"OO\",100,101\n";
    let bytes = ascii_fbx(Y_UP, 100.0, &objects, connections);
    if let Ok(clips) = FbxClips::import(&bytes, false) {
        assert!(clips.joints().is_empty(), "nothing is animated");
    }
    assert!(plan(&bytes).is_ok() || FbxClips::import(&bytes, false).is_err());
}

#[test]
fn the_importer_names_the_locked_ufbx_version() {
    let lock = include_str!("../../../../../../Cargo.lock");
    let version = lock
        .split("[[package]]")
        .find(|package| package.contains("\nname = \"ufbx\"\n"))
        .and_then(|package| package.split("\nversion = \"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .expect("ufbx in Cargo.lock");
    assert_eq!(IMPORTER.name, format!("fbx clips (ufbx {version})"));
}
