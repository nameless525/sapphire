//! Animation sources through a whole cook: sources of clips and no mesh, rigs they share, clips
//! chosen by name, retargets, and FBX and VRMA. How a retarget moves a character is tested beside it
//! (`clip::retarget::tests`).

use std::collections::BTreeMap;
use std::rc::Rc;

use glam::{Mat4, Quat, Vec3};
use sapphire_assets::clip::Clip;
use sapphire_assets::mesh::Mesh;
use sapphire_assets::rig::HumanBone;
use sapphire_assets::skeleton::{SKELETON_EXTENSION, Skeleton};
use serde_json::json;

use super::clip_tests::{Tree, by_id, move_hips, turn_arm};
use super::*;
use crate::clip::retarget::RetargetSettings;
use crate::clip::source::ClipSources;
use crate::clip::{ClipSettings, RootMotionSettings, rest_model};
use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet};
use crate::import::gltf::clips::tests::{ARM, HIPS, Track, animate, figure};
use crate::import::vrm::samples;
use crate::import::vrma::tests as vrma_files;
use crate::keypose::KeyPoseSettings;
use crate::meta::{self, ImportSettings, MeshSettings, sidecar_path};
use crate::test_util::{TempDir, read_aligned};

const OWNER: AssetId = AssetId::from_bytes([0x90; 16]);
const FOLLOWER: AssetId = AssetId::from_bytes([0x91; 16]);
pub(super) const CHARACTER: AssetId = AssetId::from_bytes([0x92; 16]);
pub(super) const HUMAN: AssetId = AssetId::from_bytes([0x93; 16]);

fn output_names(report: &CookReport) -> Vec<String> {
    let name = |asset: &CookedAsset| asset.output.file_name().unwrap().to_string_lossy().into_owned();
    let mut names: Vec<String> = report.assets.iter().map(name).collect();
    names.sort();
    names
}

fn by_extension<'a>(report: &'a CookReport, extension: &str) -> Vec<&'a CookedAsset> {
    report.assets.iter().filter(|asset| asset.output.extension().unwrap() == extension).collect()
}

/// What `error` says past the file it names, having checked it names `file`.
fn fault(error: &CookError, file: &str) -> String {
    assert!(error.to_string().contains(file), "{error}");
    std::error::Error::source(error).expect("a fault").to_string()
}

/// The figure's rig with no mesh in the file, so its joints are only what a clip moves.
fn bare_rig() -> GltfWriter {
    let mut writer = GltfWriter::default();
    let root =
        writer.add_node(json!({ "name": "Root", "translation": [0.0, 1.0, 0.0], "children": [HIPS, ARM] }));
    writer.add_node(json!({ "name": "Hips", "translation": [0.0, 0.25, 0.0] }));
    writer.add_node(json!({ "name": "Arm", "translation": [0.5, 0.0, 0.0] }));
    writer.add_root(root);
    writer
}

fn bare_rig_with_clips() -> Vec<u8> {
    let mut writer = bare_rig();
    animate(&mut writer, Some("Walk"), &[move_hips()]);
    animate(&mut writer, Some("Hit"), &[turn_arm()]);
    writer.to_glb()
}

/// The same rig as [`bare_rig`] but written the other way round, so its joints come in another
/// order, with a clip of `Swing` on the arm and `Bob` on the hips.
fn rig_in_another_order(arm_at: [f32; 3], extra: &str, arm_parent_is_hips: bool) -> Vec<u8> {
    let (arm, hips) = (0, 1);
    let mut writer = GltfWriter::default();
    writer.add_node(json!({ "name": "Arm", "translation": arm_at }));
    let mut hips_node = json!({ "name": "Hips", "translation": [0.0, 0.25, 0.0] });
    if arm_parent_is_hips {
        hips_node["children"] = json!([arm]);
    }
    writer.add_node(hips_node);
    let children = if arm_parent_is_hips { json!([hips]) } else { json!([arm, hips]) };
    let mut root_node = json!({ "name": "Root", "translation": [0.0, 1.0, 0.0], "children": children });
    let mut extra_track = None;
    if !extra.is_empty() {
        let tail = writer.add_node(json!({ "name": extra, "translation": [0.0, 0.0, 1.0] }));
        root_node["children"] = json!([arm, hips, tail]);
        extra_track = Some(tail);
    }
    let root = writer.add_node(root_node);
    writer.add_root(root);
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let swing = Track {
        node: arm,
        path: "rotation",
        interpolation: "LINEAR",
        times: &[0.0, 0.5],
        values: &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, half, half],
    };
    let bob = Track {
        node: hips,
        path: "translation",
        interpolation: "LINEAR",
        times: &[0.0, 0.5],
        values: &[0.0, 0.25, 0.0, 0.0, 0.5, 0.0],
    };
    animate(&mut writer, Some("Swing"), &[swing, bob]);
    if let Some(node) = extra_track {
        let tail = Track {
            node,
            path: "translation",
            interpolation: "LINEAR",
            times: &[0.0, 0.5],
            values: &[0.0, 0.0, 1.0, 0.0, 0.0, 2.0],
        };
        animate(&mut writer, Some("Wag"), &[tail]);
    }
    writer.to_glb()
}

pub(super) fn clips_of(settings: ClipSettings) -> ImportSettings {
    ImportSettings::GltfClips(settings)
}

#[test]
fn an_animation_source_cooks_a_skeleton_and_clips_and_never_a_mesh() {
    let with_a_mesh = {
        let mut writer = figure();
        animate(&mut writer, Some("Walk"), &[move_hips()]);
        animate(&mut writer, Some("Hit"), &[turn_arm()]);
        writer.to_glb()
    };
    for (name, bytes) in [("no_mesh", bare_rig_with_clips()), ("a_mesh", with_a_mesh)] {
        let tree = Tree::new(&format!("cook_animation_source_{name}"));
        let source = tree.write("moves.glb", &bytes);
        tree.sidecar(&source, OWNER, clips_of(ClipSettings::default()));
        let report = tree.try_cook(true).unwrap();
        assert_eq!(output_names(&report), ["moves.Hit.sclip", "moves.Walk.sclip", "moves.sskel"], "{name}");
        assert!(
            report.assets.iter().all(|asset| asset.id != OWNER),
            "{name}: nothing has the source's own ID"
        );

        let skeleton_id = meta::sub_asset_id(OWNER, SKELETON);
        let skeleton_bytes = read_aligned(&by_id(&report, skeleton_id).output);
        let skeleton = Skeleton::read(&skeleton_bytes).unwrap();
        for clip in by_extension(&report, CLIP_EXTENSION) {
            let bytes = read_aligned(&clip.output);
            let clip = Clip::read(&bytes).unwrap();
            assert_eq!(clip.skeleton.as_ref().copied(), Some(skeleton_id), "{name}");
            clip.check_skeleton(skeleton).unwrap();
        }
    }
}

#[test]
fn a_source_of_nothing_to_animate_says_so() {
    let tree = Tree::new("cook_animation_source_empty");
    let mut writer = GltfWriter::default();
    let node = writer.add_node(json!({ "name": "Prop" }));
    writer.add_root(node);
    let source = tree.write("prop.glb", &writer.to_glb());
    tree.sidecar(&source, OWNER, clips_of(ClipSettings::default()));
    let error = tree.try_cook(false).unwrap_err();
    assert!(fault(&error, "prop.glb").contains("no skeleton"), "{error:?}");
}

#[test]
fn a_sidecars_names_choose_the_clips_and_a_name_the_file_lacks_is_its_error() {
    let tree = Tree::new("cook_clip_names");
    let source = tree.write("moves.glb", &bare_rig_with_clips());
    let only = |names: &[&str]| {
        clips_of(ClipSettings {
            names: Some(names.iter().map(|name| (*name).to_owned()).collect()),
            ..ClipSettings::default()
        })
    };
    tree.sidecar(&source, OWNER, only(&["Hit"]));
    let report = tree.cook();
    assert_eq!(output_names(&report), ["moves.Hit.sclip", "moves.sskel"]);

    // None of its own: only its skeleton, which a shared rig still reads.
    tree.sidecar(&source, OWNER, only(&[]));
    assert_eq!(output_names(&tree.cook()), ["moves.sskel"]);

    tree.sidecar(&source, OWNER, only(&["Hit", "Fly"]));
    assert!(fault(&tree.try_cook(false).unwrap_err(), "moves.glb").contains("\"Fly\""));
    tree.sidecar(&source, OWNER, only(&["Hit", "Hit"]));
    assert!(fault(&tree.try_cook(false).unwrap_err(), "moves.glb").contains("twice"));
}

/// One skeleton, whichever file's order: the second source's tracks index the first's joints, and
/// a clip read through the second's order lands on the same joint by name.
#[test]
fn sources_sharing_a_rig_cook_one_skeleton_and_index_it() {
    let tree = Tree::new("cook_shared_rig");
    let owner = tree.write("owner.glb", &bare_rig_with_clips());
    let follower = tree.write("follower.glb", &rig_in_another_order([0.5, 0.0, 0.0], "", false));
    tree.sidecar(&owner, OWNER, clips_of(ClipSettings::default()));
    tree.sidecar(&follower, FOLLOWER, clips_of(ClipSettings { rig: Some(OWNER), ..ClipSettings::default() }));
    let report = tree.try_cook(true).unwrap();
    assert_eq!(by_extension(&report, SKELETON_EXTENSION).len(), 1);

    let skeleton_id = meta::sub_asset_id(OWNER, SKELETON);
    let skeleton_bytes = read_aligned(&by_id(&report, skeleton_id).output);
    let skeleton = Skeleton::read(&skeleton_bytes).unwrap();
    let names: Vec<&str> = skeleton.names.iter().map(|name| name.as_str()).collect();
    assert_eq!(names, ["Root", "Hips", "Arm"], "the owner's order");

    let clip_bytes = read_aligned(&by_id(&report, meta::sub_asset_id(FOLLOWER, "clip/Swing")).output);
    let clip = Clip::read(&clip_bytes).unwrap();
    assert_eq!(clip.skeleton.as_ref().copied(), Some(skeleton_id));
    clip.check_skeleton(skeleton).unwrap();
    let rotations: Vec<u16> = clip.rotations.iter().map(|track| track.joint.to_native()).collect();
    let translations: Vec<u16> = clip.translations.iter().map(|track| track.joint.to_native()).collect();
    assert_eq!((&rotations[..], &translations[..]), (&[2][..], &[1][..]), "by name, not by the file's order");
    // The follower's clip reads the owner's skeleton, so a second cook has to find it the same way.
    let second = tree.cook();
    assert!(second.assets.iter().all(|asset| asset.status == CookStatus::UpToDate), "{:?}", second.assets);
}

#[test]
fn a_rig_that_isnt_the_same_rig_is_an_error_naming_the_joint() {
    let rows: Vec<(&str, Vec<u8>, &str)> = vec![
        ("a joint it lacks", rig_in_another_order([0.5, 0.0, 0.0], "Tail", false), "\"Tail\" isn't in"),
        ("another parent", rig_in_another_order([0.5, 0.0, 0.0], "", true), "\"Arm\" is under"),
        ("another rest", rig_in_another_order([0.75, 0.0, 0.0], "", false), "\"Arm\" rests elsewhere"),
    ];
    let mut wrong = Vec::new();
    for (index, (name, bytes, says)) in rows.iter().enumerate() {
        let tree = Tree::new(&format!("cook_rig_mismatch_{index}"));
        let owner = tree.write("owner.glb", &bare_rig_with_clips());
        let follower = tree.write("follower.glb", bytes);
        tree.sidecar(&owner, OWNER, clips_of(ClipSettings::default()));
        tree.sidecar(
            &follower,
            FOLLOWER,
            clips_of(ClipSettings { rig: Some(OWNER), ..ClipSettings::default() }),
        );
        match tree.try_cook(false) {
            Ok(_) => wrong.push(format!("{name}: it cooked")),
            Err(error) => {
                let text = fault(&error, "follower.glb");
                if !text.contains(says) {
                    wrong.push(format!("{name}: said {text:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_rig_named_wrongly_is_the_sidecars_error() {
    let rows: Vec<(&str, ImportSettings, &str)> = vec![
        (
            "not in the tree",
            clips_of(ClipSettings { rig: Some(AssetId::from_bytes([0xee; 16])), ..ClipSettings::default() }),
            "isn't a source",
        ),
        (
            "at the end of a chain",
            clips_of(ClipSettings { rig: Some(HUMAN), ..ClipSettings::default() }),
            "isn't a source",
        ),
        (
            "on a mesh source",
            ImportSettings::Gltf(MeshSettings {
                clips: Some(ClipSettings { rig: Some(OWNER), ..ClipSettings::default() }),
                ..MeshSettings::default()
            }),
            "says `rig` on a mesh",
        ),
        (
            "and a map of its own",
            clips_of(ClipSettings {
                rig: Some(OWNER),
                humanoid: [(HumanBone::Hips, "Hips".to_owned())].into_iter().collect(),
                ..ClipSettings::default()
            }),
            "both `rig` and `humanoid`",
        ),
    ];
    let mut wrong = Vec::new();
    for (index, (name, settings, says)) in rows.iter().enumerate() {
        let tree = Tree::new(&format!("cook_rig_named_wrongly_{index}"));
        let owner = tree.write("owner.glb", &bare_rig_with_clips());
        tree.sidecar(&owner, OWNER, clips_of(ClipSettings::default()));
        let link = tree.write("link.glb", &bare_rig_with_clips());
        tree.sidecar(
            &link,
            CHARACTER,
            clips_of(ClipSettings { rig: Some(OWNER), ..ClipSettings::default() }),
        );
        let chain = tree.write("chain.glb", &bare_rig_with_clips());
        tree.sidecar(
            &chain,
            HUMAN,
            clips_of(ClipSettings { rig: Some(CHARACTER), ..ClipSettings::default() }),
        );
        let source = tree.write("follower.glb", &bare_rig_with_clips());
        tree.sidecar(&source, FOLLOWER, settings.clone());
        match tree.try_cook(false) {
            Ok(_) => wrong.push(format!("{name}: it cooked")),
            Err(error) => {
                let text = fault(&error, "follower.glb");
                if !text.contains(says) {
                    wrong.push(format!("{name}: said {text:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Each of a mesh's binds, on its joint of a shared skeleton at rest, is the identity if the binds
/// have found their own joints.
fn assert_rest_palette_is_identity(
    mesh: &sapphire_assets::mesh::ArchivedMesh,
    skeleton: &Skeleton,
    tolerance: f32,
) {
    let model = rest_model(skeleton);
    let skin = mesh.skin.as_ref().expect("skinned");
    assert_eq!(skin.joint_count.to_native() as usize, skeleton.parents.len());
    for bind in skin.binds.iter() {
        let joint = usize::from(bind.joint.to_native());
        let inverse = Mat4::from_cols_array_2d(
            &bind.inverse_bind_matrix.map(|column| column.map(|value| value.to_native())),
        );
        let palette = Mat4::from(model[joint]) * inverse;
        assert!(palette.abs_diff_eq(Mat4::IDENTITY, tolerance), "{}: {palette}", skeleton.names[joint],);
    }
}

#[test]
fn a_mesh_may_share_an_animation_sources_skeleton() {
    let tree = Tree::new("cook_mesh_shared_skeleton");
    // Figure's joints: Root, Hips, Arm, and binds of the rests' inverses. The owner's order is other.
    let owner = tree.write("owner.glb", &rig_in_another_order([0.5, 0.0, 0.0], "", false));
    tree.sidecar(&owner, OWNER, clips_of(ClipSettings::default()));
    let inverse = |at: Vec3| Mat4::from_translation(-at).to_cols_array();
    let geometry = Geometry {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        influences: vec![InfluenceSet {
            joints: vec![[0, 1, 2, 0]; 3],
            weights: vec![[0.5, 0.25, 0.25, 0.0]; 3],
        }],
        indices: vec![0, 1, 2],
        ..Geometry::default()
    };
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Body", &[(&geometry, None)]);
    let root = writer.add_node(json!({ "name": "Root", "translation": [0.0, 1.0, 0.0], "children": [1, 2] }));
    let hips = writer.add_node(json!({ "name": "Hips", "translation": [0.0, 0.25, 0.0] }));
    let arm = writer.add_node(json!({ "name": "Arm", "translation": [0.5, 0.0, 0.0] }));
    let binds = [
        inverse(Vec3::new(0.0, 1.0, 0.0)),
        inverse(Vec3::new(0.0, 1.25, 0.0)),
        inverse(Vec3::new(0.5, 1.0, 0.0)),
    ];
    writer.add_skin(&[root, hips, arm], Some(&binds));
    let body = writer.add_node(json!({ "name": "Body", "mesh": mesh, "skin": 0 }));
    writer.add_root(root);
    writer.add_root(body);
    let source = tree.write("body.glb", &writer.to_glb());
    let settings = MeshSettings { skeleton: Some(OWNER), ..MeshSettings::default() };
    tree.sidecar(&source, FOLLOWER, ImportSettings::Gltf(settings));

    let report = tree.try_cook(true).unwrap();
    assert_eq!(by_extension(&report, SKELETON_EXTENSION).len(), 1, "its own skeleton isn't cooked");
    let skeleton_id = meta::sub_asset_id(OWNER, SKELETON);
    let skeleton_bytes = read_aligned(&by_id(&report, skeleton_id).output);
    let skeleton = Skeleton::from_cooked(&skeleton_bytes).unwrap();
    let names: Vec<&str> = skeleton.names.iter().map(String::as_str).collect();
    assert_eq!(names, ["Root", "Arm", "Hips"], "not the mesh's own order");
    let mesh_bytes = read_aligned(&by_id(&report, FOLLOWER).output);
    let mesh = Mesh::read(&mesh_bytes).unwrap();
    assert_eq!(mesh.skin.as_ref().unwrap().skeleton.as_ref().copied(), Some(skeleton_id));
    assert_rest_palette_is_identity(mesh, &skeleton, 1e-5);

    // Its joints must be the shared skeleton's: the same check a clip has.
    let figure_alone = tree.write("body.glb", &figure().to_glb());
    tree.sidecar(
        &figure_alone,
        FOLLOWER,
        ImportSettings::Gltf(MeshSettings {
            skeleton: Some(AssetId::from_bytes([0xee; 16])),
            ..MeshSettings::default()
        }),
    );
    assert!(fault(&tree.try_cook(false).unwrap_err(), "body.glb").contains("isn't a source"));
}

/// The real enemy on the real rig, whatever `cook: false` says of it until O4 gives it materials:
/// its own order differs from the clips', and it must rest where it was modeled.
#[test]
fn the_enemys_mesh_rests_where_it_was_modeled_on_the_clips_skeleton() {
    let third_party = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/third_party");
    let tree = Tree::new("cook_enemy_skeleton");
    let owner_path = third_party.join("kaykit_character_animations/Rig_Medium/Rig_Medium_General.glb");
    let owner = tree.write("Rig_Medium_General.glb", &fs::read(&owner_path).unwrap());
    let owner_id = meta::read(&sidecar_path(&owner_path)).unwrap().id;
    tree.sidecar(
        &owner,
        owner_id,
        clips_of(ClipSettings { names: Some(Vec::new()), ..ClipSettings::default() }),
    );
    let enemy_path = third_party.join("kaykit_skeletons/characters/Skeleton_Warrior.glb");
    let enemy_meta = meta::read(&sidecar_path(&enemy_path)).unwrap();
    assert_eq!(enemy_meta.importer.shared_rig(), Some(owner_id), "the sidecar names the clips' rig");
    let enemy = tree.write("Skeleton_Warrior.glb", &fs::read(&enemy_path).unwrap());
    tree.sidecar(&enemy, enemy_meta.id, enemy_meta.importer.clone());

    let report = tree.try_cook(true).unwrap();
    assert_eq!(by_extension(&report, SKELETON_EXTENSION).len(), 1);
    let skeleton_bytes = read_aligned(&by_id(&report, meta::sub_asset_id(owner_id, SKELETON)).output);
    let skeleton = Skeleton::from_cooked(&skeleton_bytes).unwrap();
    let mesh_bytes = read_aligned(&by_id(&report, enemy_meta.id).output);
    let mesh = Mesh::read(&mesh_bytes).unwrap();
    // To the exporter's rounding: its inverse binds are written to six figures.
    assert_rest_palette_is_identity(mesh, &skeleton, 1e-3);
}

/// A humanoid in a skinned file, as KayKit's are: a mesh brings every joint in, moved or not.
pub(super) fn human_with_a_mesh() -> Vec<u8> {
    let mut writer = vrma_files::vrma();
    let geometry = Geometry {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        influences: vec![InfluenceSet {
            joints: vec![[0, 0, 0, 0]; 3],
            weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
        }],
        indices: vec![0, 1, 2],
        ..Geometry::default()
    };
    let mesh = writer.add_mesh("Body", &[(&geometry, None)]);
    let joints: Vec<usize> = (0..15).collect();
    writer.add_skin(&joints, None);
    let body = writer.add_node(json!({ "name": "Body", "mesh": mesh, "skin": 0 }));
    writer.add_root(body);
    vrma_files::lower_arm(&mut writer);
    writer.to_glb()
}

fn human_map() -> BTreeMap<HumanBone, String> {
    use HumanBone::*;
    [
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
    .map(|(bone, joint)| (bone, joint.to_owned()))
    .collect()
}

pub(super) fn to_the_character(names: &[&str]) -> ClipSettings {
    ClipSettings {
        humanoid: human_map(),
        retarget: vec![RetargetSettings {
            target: CHARACTER,
            names: names.iter().map(|name| (*name).to_owned()).collect(),
            scale: None,
            ground: true,
            adjust: BTreeMap::new(),
            gain: BTreeMap::new(),
            follow: Vec::new(),
        }],
        ..ClipSettings::default()
    }
}

/// One source of each new kind for [`clip_tests`](super::clip_tests)' run-to-run comparison: an FBX of
/// clips, a VRMA, a shared rig and a retarget, for the sample character that test's tree has.
pub(super) fn add_every_kind_of_animation_source(tree: &Tree) {
    let fbx = tree.write("every/fbx.fbx", &fixture("animated_clips.fbx"));
    tree.sidecar(&fbx, AssetId::from_bytes([0xa0; 16]), ImportSettings::FbxClips(ClipSettings::default()));
    let mut writer = vrma_files::vrma();
    vrma_files::lower_arm(&mut writer);
    let vrma = tree.write("every/wave.vrma", &writer.to_glb());
    tree.sidecar(&vrma, AssetId::from_bytes([0xa1; 16]), ImportSettings::Vrma(ClipSettings::default()));
    let owner = tree.write("every/owner.glb", &human_with_a_mesh());
    let on_the_sample = RetargetSettings {
        target: AssetId::from_bytes([0x77; 16]),
        names: vec!["Lower".to_owned()],
        scale: None,
        ground: true,
        adjust: BTreeMap::new(),
        gain: BTreeMap::new(),
        follow: Vec::new(),
    };
    let settings =
        ClipSettings { humanoid: human_map(), retarget: vec![on_the_sample], ..ClipSettings::default() };
    tree.sidecar(&owner, AssetId::from_bytes([0xa2; 16]), clips_of(settings));
    let follower = tree.write("every/follower.glb", &human_with_a_mesh());
    let settings = ClipSettings { rig: Some(AssetId::from_bytes([0xa2; 16])), ..ClipSettings::default() };
    tree.sidecar(&follower, AssetId::from_bytes([0xa3; 16]), clips_of(settings));
}

/// The sample character, and the humanoid with `settings`.
fn retarget_tree(name: &str, settings: ClipSettings) -> (Tree, PathBuf) {
    let tree = Tree::new(name);
    let character = tree.write("characters/sample.vrm", &samples::vrm1().to_glb());
    tree.sidecar(&character, CHARACTER, ImportSettings::for_new_sidecar(&character).unwrap());
    let source = tree.write("moves/human.glb", &human_with_a_mesh());
    tree.sidecar(&source, HUMAN, clips_of(settings));
    (tree, source)
}

#[test]
fn a_retarget_cooks_a_clip_of_its_own_for_the_targets_skeleton() {
    let settings = ClipSettings { names: Some(vec!["Lower".to_owned()]), ..to_the_character(&["Lower"]) };
    let (tree, source) = retarget_tree("cook_retarget", settings.clone());
    let report = tree.try_cook(true).unwrap();
    let (native, onto) = (
        meta::sub_asset_id(HUMAN, "clip/Lower"),
        meta::sub_asset_id(HUMAN, &format!("clip/Lower@{CHARACTER}")),
    );
    let out = tree.out().join("moves");
    assert_eq!(by_id(&report, native).output, out.join("human.Lower.sclip"));
    assert_eq!(by_id(&report, onto).output, out.join("human.Lower.92929292.sclip"));

    let target_skeleton_id = meta::sub_asset_id(CHARACTER, SKELETON);
    let target_bytes = read_aligned(&by_id(&report, target_skeleton_id).output);
    let target_skeleton = Skeleton::read(&target_bytes).unwrap();
    let clip_bytes = read_aligned(&by_id(&report, onto).output);
    let clip = Clip::read(&clip_bytes).unwrap();
    assert_eq!(clip.skeleton.as_ref().copied(), Some(target_skeleton_id));
    assert_eq!(clip.name.as_str(), "Lower");
    clip.check_skeleton(target_skeleton).unwrap();
    assert!(!clip.rotations.is_empty());
    let native_bytes = read_aligned(&by_id(&report, native).output);
    let native_clip = Clip::read(&native_bytes).unwrap();
    assert_eq!(native_clip.skeleton.as_ref().copied(), Some(meta::sub_asset_id(HUMAN, SKELETON)));

    // Nothing changed: a hit. What a retarget reads moved: it alone recooks.
    let second = tree.cook();
    assert!(second.assets.iter().all(|asset| asset.status == CookStatus::UpToDate), "{:?}", second.assets);
    let mut taller = samples::vrm1();
    taller.document["nodes"][samples::node::HIPS]["translation"] = json!([0.0, 1.0, 0.0]);
    tree.write("characters/sample.vrm", &taller.to_glb());
    let third = tree.cook();
    assert_eq!(by_id(&third, onto).status, CookStatus::Cooked, "the target's skeleton changed");
    assert_eq!(by_id(&third, native).status, CookStatus::UpToDate, "its own clip didn't");
    assert_ne!(by_id(&third, onto).key, by_id(&report, onto).key);

    let scaled = RetargetSettings { scale: Some(1.5), ..settings.retarget[0].clone() };
    tree.sidecar(&source, HUMAN, clips_of(ClipSettings { retarget: vec![scaled], ..settings.clone() }));
    let fourth = tree.cook();
    assert_eq!(by_id(&fourth, onto).status, CookStatus::Cooked, "the sidecar's scale changed");
}

/// A bad row fails the cook of the source it's in, and the error says the file and the entry.
#[test]
fn a_bad_retarget_says_which_file_and_which_entry() {
    type Edit = fn(&mut ClipSettings);
    let rows: Vec<(&str, Edit, &str)> = vec![
        (
            "a joint the file lacks",
            |s| drop(s.humanoid.insert(HumanBone::Chest, "Ribcage".into())),
            "\"Ribcage\"",
        ),
        ("a required bone left out", |s| drop(s.humanoid.remove(&HumanBone::LeftHand)), "no leftHand bone"),
        ("no map", |s| s.humanoid.clear(), "no humanoid map"),
        (
            "a target not in the tree",
            |s| s.retarget[0].target = AssetId::from_bytes([0xee; 16]),
            "isn't a source",
        ),
        ("a target with no map", |s| s.retarget[0].target = OWNER, "humanoid map"),
        ("a clip the file lacks", |s| s.retarget[0].names.push("Fly".into()), "\"Fly\""),
        ("a clip twice", |s| s.retarget[0].names.push("Lower".into()), "twice"),
        ("a scale of zero", |s| s.retarget[0].scale = Some(0.0), "out of range"),
        ("a scale that's no number", |s| s.retarget[0].scale = Some(f32::NAN), "out of range"),
        ("a shared rig beside a map", |s| s.rig = Some(OWNER), "both `rig` and `humanoid`"),
        (
            "root motion from a joint the file lacks",
            |s| {
                s.root_motion = Some(RootMotionSettings {
                    bone: "Tail".into(),
                    translation: Default::default(),
                    rotation: Default::default(),
                });
            },
            "\"Tail\"",
        ),
    ];
    let mut wrong = Vec::new();
    for (index, (name, edit, says)) in rows.iter().enumerate() {
        let mut settings = to_the_character(&["Lower"]);
        edit(&mut settings);
        let (tree, _) = retarget_tree(&format!("cook_bad_retarget_{index}"), settings);
        let owner = tree.write("owner.glb", &bare_rig_with_clips());
        tree.sidecar(&owner, OWNER, clips_of(ClipSettings::default()));
        match tree.try_cook(false) {
            Ok(_) => wrong.push(format!("{name}: it cooked")),
            Err(error) => {
                let text = fault(&error, "human.glb");
                if !text.contains(says) {
                    wrong.push(format!("{name}: said {text:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A source's joints may be what a rig's owner names, read through the owner's map.
#[test]
fn a_source_sharing_a_rig_retargets_by_the_owners_map() {
    let tree = Tree::new("cook_retarget_shared_rig");
    let character = tree.write("sample.vrm", &samples::vrm1().to_glb());
    tree.sidecar(&character, CHARACTER, ImportSettings::for_new_sidecar(&character).unwrap());
    let owner = tree.write("owner.glb", &human_with_a_mesh());
    let names_only =
        ClipSettings { humanoid: human_map(), names: Some(Vec::new()), ..ClipSettings::default() };
    tree.sidecar(&owner, HUMAN, clips_of(names_only));
    let follower = tree.write("follower.glb", &human_with_a_mesh());
    let settings =
        ClipSettings { rig: Some(HUMAN), humanoid: BTreeMap::new(), ..to_the_character(&["Lower"]) };
    tree.sidecar(&follower, FOLLOWER, clips_of(settings));
    let report = tree.try_cook(true).unwrap();
    let clip = by_id(&report, meta::sub_asset_id(FOLLOWER, &format!("clip/Lower@{CHARACTER}")));
    let bytes = read_aligned(&clip.output);
    assert!(!Clip::read(&bytes).unwrap().rotations.is_empty());
    // Its own (none) and the rig's skeleton, and the character's.
    assert_eq!(by_extension(&report, SKELETON_EXTENSION).len(), 2);

    // A new map on the owner reaches a retarget that reads it.
    let names_only = ClipSettings {
        humanoid: {
            let mut map = human_map();
            map.remove(&HumanBone::LeftLowerArm);
            map.insert(HumanBone::LeftLowerArm, "LowerArmR".to_owned());
            map
        },
        names: Some(Vec::new()),
        ..ClipSettings::default()
    };
    tree.sidecar(&owner, HUMAN, clips_of(names_only));
    let second = tree.cook();
    let clip = by_id(&second, meta::sub_asset_id(FOLLOWER, &format!("clip/Lower@{CHARACTER}")));
    assert_eq!(clip.status, CookStatus::Cooked);
}

pub(super) fn fixture(name: &str) -> Vec<u8> {
    fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fbx").join(name)).unwrap()
}

#[test]
fn an_fbx_cooks_its_stacks_as_clips_with_its_mesh_or_without() {
    let clips_in = |report: &CookReport| {
        let mut names: Vec<String> = by_extension(report, CLIP_EXTENSION)
            .iter()
            .map(|clip| {
                let bytes = read_aligned(&clip.output);
                Clip::read(&bytes).unwrap().name.as_str().to_owned()
            })
            .collect();
        names.sort();
        names
    };

    let tree = Tree::new("cook_fbx_with_mesh");
    let source = tree.write("moves.fbx", &fixture("animated_clips.fbx"));
    let settings = MeshSettings { clips: Some(ClipSettings::default()), ..MeshSettings::default() };
    tree.sidecar(&source, OWNER, ImportSettings::Fbx(settings));
    let report = tree.try_cook(true).unwrap();
    let names = clips_in(&report);
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names.iter().any(|name| name.ends_with("Stride")) && names.iter().any(|name| name.ends_with("Spin"))
    );
    assert_eq!(by_extension(&report, SKELETON_EXTENSION).len(), 1);
    assert_eq!(by_extension(&report, "smesh").len(), 1);
    let skeleton_bytes = read_aligned(&by_id(&report, meta::sub_asset_id(OWNER, SKELETON)).output);
    let skeleton = Skeleton::read(&skeleton_bytes).unwrap();
    for clip in by_extension(&report, CLIP_EXTENSION) {
        let bytes = read_aligned(&clip.output);
        Clip::read(&bytes).unwrap().check_skeleton(skeleton).unwrap();
    }

    let tree = Tree::new("cook_fbx_animation_only");
    let source = tree.write("moves.fbx", &fixture("animated_clips.fbx"));
    tree.sidecar(&source, OWNER, ImportSettings::FbxClips(ClipSettings::default()));
    let report = tree.try_cook(true).unwrap();
    assert_eq!(clips_in(&report), names);
    assert_eq!(by_extension(&report, "smesh").len(), 0);
    assert_eq!(by_extension(&report, SKELETON_EXTENSION).len(), 1);

    // A clip the file lacks is the same error as a glTF's.
    let only = ClipSettings { names: Some(vec!["Fly".to_owned()]), ..ClipSettings::default() };
    tree.sidecar(&source, OWNER, ImportSettings::FbxClips(only));
    assert!(fault(&tree.try_cook(false).unwrap_err(), "moves.fbx").contains("\"Fly\""));

    // A file that's nothing like one.
    tree.write("moves.fbx", b"Kaydara FBX Binary  nonsense");
    tree.sidecar(&source, OWNER, ImportSettings::FbxClips(ClipSettings::default()));
    assert!(fault(&tree.try_cook(false).unwrap_err(), "moves.fbx").contains("not a readable FBX"));
}

/// A VRMA is its own map: a first cook writes its sidecar, and a retarget needs nothing else.
#[test]
fn a_vrma_cooks_its_own_skeleton_and_retargets_by_its_own_map() {
    let vrma_bytes = || {
        let mut writer = vrma_files::vrma();
        vrma_files::lower_arm(&mut writer);
        writer.to_glb()
    };
    let tree = Tree::new("cook_vrma_new_sidecar");
    let source = tree.write("wave.vrma", &vrma_bytes());
    let report = tree.cook();
    assert_eq!(output_names(&report), ["wave.Lower.sclip", "wave.sskel"]);
    assert!(matches!(meta::read(&sidecar_path(&source)).unwrap().importer, ImportSettings::Vrma(_)));

    let tree = Tree::new("cook_vrma_retarget");
    let character = tree.write("sample.vrm", &samples::vrm1().to_glb());
    tree.sidecar(&character, CHARACTER, ImportSettings::for_new_sidecar(&character).unwrap());
    let source = tree.write("wave.vrma", &vrma_bytes());
    let settings =
        ClipSettings { names: Some(Vec::new()), humanoid: BTreeMap::new(), ..to_the_character(&["Lower"]) };
    tree.sidecar(&source, HUMAN, ImportSettings::Vrma(settings.clone()));
    let report = tree.try_cook(true).unwrap();
    let clip = by_id(&report, meta::sub_asset_id(HUMAN, &format!("clip/Lower@{CHARACTER}")));
    let clip_bytes = read_aligned(&clip.output);
    let clip = Clip::read(&clip_bytes).unwrap();
    let target = meta::sub_asset_id(CHARACTER, SKELETON);
    assert_eq!(clip.skeleton.as_ref().copied(), Some(target));

    // The file's map is the file's.
    let own_map = ClipSettings { humanoid: human_map(), ..settings };
    tree.sidecar(&source, HUMAN, ImportSettings::Vrma(own_map));
    assert!(fault(&tree.try_cook(false).unwrap_err(), "wave.vrma").contains("a VRMA's is the file's"));
}

#[test]
fn every_clip_of_a_source_reads_one_import() {
    let dir = TempDir::new("cook_one_import");
    let (a, b) = (dir.path().join("a.glb"), dir.path().join("b.glb"));
    let bytes = bare_rig_with_clips();
    let settings = clips_of(ClipSettings::default());
    let sources = ClipSources::default();
    let first = sources.get(&a, &bytes, &settings).unwrap();
    let again = sources.get(&a, &bytes, &settings).unwrap();
    assert!(Rc::ptr_eq(&first, &again), "the same file: the same import");
    assert_eq!(first.joints.len(), 3);
    let other = sources.get(&b, &bytes, &settings).unwrap();
    assert!(!Rc::ptr_eq(&first, &other), "another file: its own");
}

/// A joint cycle in a source with no mesh: a walk of its nodes must still end, in an error that
/// names the file.
#[test]
fn a_cycle_of_nodes_in_an_animation_source_is_an_error_not_a_hang() {
    let tree = Tree::new("cook_cycle");
    let mut writer = GltfWriter::default();
    let (a, b) = (0, 1);
    writer.add_node(json!({ "name": "A", "children": [b] }));
    writer.add_node(json!({ "name": "B", "children": [a] }));
    writer.add_root(a);
    animate(
        &mut writer,
        Some("Loop"),
        &[Track {
            node: a,
            path: "translation",
            interpolation: "LINEAR",
            times: &[0.0, 1.0],
            values: &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }],
    );
    let source = tree.write("loop.glb", &writer.to_glb());
    tree.sidecar(&source, OWNER, clips_of(ClipSettings::default()));
    let error = tree.try_cook(false).unwrap_err();
    assert!(error.to_string().contains("loop.glb"), "{error}");
}

/// Which clips KayKit's sidecars ask for, against the files as they are: what a real cook would
/// refuse, without cooking the character for it. `xtask content --locked` cooks them for real, on
/// every CI run.
#[test]
fn kaykits_sidecars_ask_for_what_its_files_have() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/third_party/kaykit_character_animations/Rig_Medium");
    let character_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/third_party/vrm1_constraint_twist_sample/VRM1_Constraint_Twist_Sample.vrm");
    let character = meta::read(&sidecar_path(&character_path)).unwrap().id;
    let warrior_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat/enemy/warrior.glb");
    let warrior = meta::read(&sidecar_path(&warrior_path)).unwrap().id;
    let rows = [
        ("General", 1, 0, 4),
        ("MovementBasic", 0, 0, 6),
        ("MovementAdvanced", 0, 0, 7),
        ("CombatMelee", 0, 0, 3),
        ("Special", 0, 0, 0),
    ];
    let mut owner = None;
    for (file, skeletons, native, retargeted) in rows {
        let path = folder.join(format!("Rig_Medium_{file}.glb"));
        let meta = meta::read(&sidecar_path(&path)).unwrap();
        assert!(meta.cook, "{file}");
        let clips = meta.importer.clips().unwrap();
        if clips.rig.is_none() {
            owner = Some(meta.id);
        } else {
            assert_eq!(clips.rig, owner, "{file} shares the first file's rig");
        }
        assert!(
            clips.retarget.iter().all(|retarget| [character, warrior].contains(&retarget.target)),
            "{file}"
        );
        let asset = SourceAsset { path, meta: meta.clone() };
        let parts = sub_assets(&asset).unwrap_or_else(|error| panic!("{file}: {error}"));
        let count = |what: fn(&Part) -> bool| parts.iter().filter(|planned| what(&planned.part)).count();
        assert_eq!(count(|part| matches!(part, Part::Skeleton)), skeletons, "{file}");
        assert_eq!(count(|part| matches!(part, Part::Clip { target: None, .. })), native, "{file}");
        assert_eq!(count(|part| matches!(part, Part::Clip { target: Some(_), .. })), retargeted, "{file}");
    }
}

/// A key-pose document may fit more than one rig: each its own clip, on its own skeleton, by its own
/// map, a sidecar's as well as a VRM's.
#[test]
fn a_key_pose_document_cooks_for_each_rig_it_names() {
    const DOCUMENT: &str = "(
        type: \"sapphire::pipeline::KeyPoseClip\",
        version: 1,
        data: (
            keys: [
                (tick: 0, pose: { LeftUpperArm: (z: -70) }),
                (tick: 10, pose: { LeftUpperArm: (z: 20), Neck: (z: 5) }),
            ],
        ),
    )";
    let tree = Tree::new("cook_keypose_also");
    let character = tree.write("sample.vrm", &samples::vrm1().to_glb());
    tree.sidecar(&character, CHARACTER, ImportSettings::for_new_sidecar(&character).unwrap());
    let human = tree.write("human.glb", &human_with_a_mesh());
    let map = ClipSettings { humanoid: human_map(), names: Some(Vec::new()), ..ClipSettings::default() };
    tree.sidecar(&human, HUMAN, clips_of(map));
    let document = tree.write("swing.keypose.ron", DOCUMENT.as_bytes());
    let settings = KeyPoseSettings { rig: Some(CHARACTER), also: vec![HUMAN], ..KeyPoseSettings::default() };
    tree.sidecar(&document, FOLLOWER, ImportSettings::KeyPose(settings));
    let report = tree.try_cook(true).unwrap();

    let on_character = by_id(&report, FOLLOWER);
    let on_human = by_id(&report, meta::sub_asset_id(FOLLOWER, &format!("rig/{HUMAN}")));
    assert_eq!(on_human.output, tree.out().join("swing.93939393.sclip"));

    let tracks_on = |clip: &CookedAsset, skeleton: AssetId| {
        let bytes = read_aligned(&clip.output);
        let clip = Clip::read(&bytes).unwrap();
        assert_eq!(clip.skeleton.as_ref().copied(), Some(skeleton));
        let skeleton_bytes = read_aligned(&by_id(&report, skeleton).output);
        let skeleton = Skeleton::read(&skeleton_bytes).unwrap();
        clip.check_skeleton(skeleton).unwrap();
        clip.rotations
            .iter()
            .map(|track| skeleton.names[usize::from(track.joint.to_native())].as_str().to_owned())
            .collect::<Vec<String>>()
    };
    let on_human_tracks = tracks_on(on_human, meta::sub_asset_id(HUMAN, SKELETON));
    assert_eq!(on_human_tracks, ["UpperArmL"], "it has no neck, which it skips");
    let on_character_tracks = tracks_on(on_character, meta::sub_asset_id(CHARACTER, SKELETON));
    assert_eq!(on_character_tracks.len(), 2, "{on_character_tracks:?}");

    // A rig that isn't humanoid is the sidecar's error, naming the document.
    let figure_source = tree.write("figure.glb", &figure().to_glb());
    let figure_id = AssetId::from_bytes([0x94; 16]);
    tree.sidecar(&figure_source, figure_id, ImportSettings::Gltf(MeshSettings::default()));
    let settings =
        KeyPoseSettings { rig: Some(CHARACTER), also: vec![figure_id], ..KeyPoseSettings::default() };
    tree.sidecar(&document, FOLLOWER, ImportSettings::KeyPose(settings));
    assert!(fault(&tree.try_cook(false).unwrap_err(), "swing.keypose.ron").contains("isn't a source"));
}

/// A document's morph weights, by name, on the mesh a rig's source cooks: a name the mesh lacks is
/// its error, and a rig with no mesh skips them, as it skips a bone it lacks. Its root's turn cooks
/// alongside them on either.
#[test]
fn a_key_pose_documents_weights_reach_the_mesh_of_its_rig() {
    const DOCUMENT: &str = "(
        type: \"sapphire::pipeline::KeyPoseClip\",
        version: 1,
        data: (
            keys: [
                (tick: 0, morphs: { \"blink\": 0.0 }),
                (tick: 10, root_turn: 90, morphs: { \"blink\": 1.0 }),
            ],
        ),
    )";
    let tree = Tree::new("cook_keypose_morphs");
    let character = tree.write("sample.vrm", &samples::vrm1().to_glb());
    tree.sidecar(&character, CHARACTER, ImportSettings::for_new_sidecar(&character).unwrap());
    let human = tree.write("human.glb", &human_with_a_mesh());
    let map = ClipSettings { humanoid: human_map(), names: Some(Vec::new()), ..ClipSettings::default() };
    tree.sidecar(&human, HUMAN, clips_of(map));
    let document = tree.write("blink.keypose.ron", DOCUMENT.as_bytes());
    let settings = KeyPoseSettings { rig: Some(CHARACTER), also: vec![HUMAN], ..KeyPoseSettings::default() };
    tree.sidecar(&document, FOLLOWER, ImportSettings::KeyPose(settings));
    let report = tree.try_cook(true).unwrap();

    let mesh_bytes = read_aligned(&by_id(&report, CHARACTER).output);
    let mesh = Mesh::read(&mesh_bytes).unwrap();
    let blink = mesh.morph_targets.iter().position(|target| target.name.as_str() == "blink").expect("blink");
    let clip_bytes = read_aligned(&by_id(&report, FOLLOWER).output);
    let clip = Clip::read(&clip_bytes).unwrap();
    clip.check_mesh(mesh).unwrap();
    assert_eq!(clip.weights.len(), 1);
    assert_eq!(usize::from(clip.weights[0].target.to_native()), blink);
    assert_eq!(clip.weight(&clip.weights[0], 10.0), 1.0);

    let human_bytes =
        read_aligned(&by_id(&report, meta::sub_asset_id(FOLLOWER, &format!("rig/{HUMAN}"))).output);
    let on_human = Clip::read(&human_bytes).unwrap();
    assert!(on_human.weights.is_empty(), "no mesh, no targets");
    for clip in [clip, on_human] {
        let root = clip.root.as_ref().expect("its turn");
        let (axis, angle) = Quat::from_array(clip.rotation(&root.rotation, 10.0)).to_axis_angle();
        assert!((angle.to_degrees() - 90.0).abs() < 0.5 && axis.y > 0.99, "{axis} {angle}");
    }

    tree.write("blink.keypose.ron", DOCUMENT.replace("\"blink\"", "\"frown\"").as_bytes());
    assert!(fault(&tree.try_cook(false).unwrap_err(), "blink.keypose.ron").contains("\"frown\""));
}
