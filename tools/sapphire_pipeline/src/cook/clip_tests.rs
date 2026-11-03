//! Skeletons and clips through a whole cook: sidecars, the cache, `--locked`, the packer and the
//! runtime's reads of what they write. What a clip holds is tested beside its sources
//! (`clip::tests`, `import::gltf::clips::tests`, `keypose::tests`).

use std::f32::consts::FRAC_1_SQRT_2;
use std::sync::Arc;

use sapphire_assets::clip::Clip;
use sapphire_assets::mesh::Mesh;
use sapphire_assets::pack::PackReader;
use sapphire_assets::rig::HumanBone;
use sapphire_assets::skeleton::Skeleton;
use sapphire_assets::{AssetBudgets, AssetId, AssetKind, AssetServer, Handle, MemoryPackSource};

use super::*;
use crate::clip::{ClipSettings, ReductionError, RootMotionSettings};
use crate::gltf_writer::GltfWriter;
use crate::import::gltf::clips::tests::{ARM, HIPS, Track, animate, figure};
use crate::import::vrm::samples;
use crate::keypose::KeyPoseSettings;
use crate::meta::{self, AssetMeta, ImportSettings, MeshSettings, sidecar_path};
use crate::pack::{SHARED, pack_cooked};
use crate::test_util::{TempDir, read_aligned};

/// A content tree beside its outputs and cache.
pub(super) struct Tree {
    pub(super) dir: TempDir,
}

impl Tree {
    pub(super) fn new(name: &str) -> Self {
        Self { dir: TempDir::new(name) }
    }

    pub(super) fn content(&self) -> PathBuf {
        self.dir.path().join("content")
    }

    pub(super) fn out(&self) -> PathBuf {
        self.dir.path().join("out")
    }

    pub(super) fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.content().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    /// Replaces a file [`Self::write`] wrote.
    pub(super) fn write_at(&self, path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
    }

    pub(super) fn sidecar(&self, source: &Path, id: AssetId, importer: ImportSettings) {
        let meta = AssetMeta::new(id, importer);
        fs::write(sidecar_path(source), sapphire_serialize::to_ron::<AssetMeta>(&meta).unwrap()).unwrap();
    }

    pub(super) fn try_cook(&self, locked: bool) -> Result<CookReport, CookError> {
        let cache = self.dir.path().join("cache");
        let options = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked };
        cook_dir(&self.content(), &self.out(), &options)
    }

    /// A cook on one worker that calls `reached` at each stage, to save a file there.
    pub(super) fn try_cook_through(&self, reached: &dyn Fn(CookStage)) -> Result<CookReport, CookError> {
        let cache = self.dir.path().join("cache");
        let options = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked: false };
        cook_dir_through(&self.content(), &self.out(), &options, 1, reached)
    }

    pub(super) fn cook(&self) -> CookReport {
        self.try_cook(false).unwrap()
    }
}

fn id_of(source: &Path) -> AssetId {
    meta::read(&sidecar_path(source)).unwrap().id
}

pub(super) fn by_id(report: &CookReport, id: AssetId) -> &CookedAsset {
    report.assets.iter().find(|asset| asset.id == id).unwrap_or_else(|| panic!("{id} wasn't cooked"))
}

fn statuses_of<'a>(report: &'a CookReport, source: &Path) -> Vec<(&'a Path, CookStatus)> {
    let assets = report.assets.iter().filter(|asset| asset.source == source);
    assets.map(|asset| (asset.output.as_path(), asset.status)).collect()
}

pub(super) fn move_hips<'a>() -> Track<'a> {
    Track {
        node: HIPS,
        path: "translation",
        interpolation: "LINEAR",
        times: &[0.0, 1.0],
        values: &[0.0, 0.25, 0.0, 1.0, 0.25, 3.0],
    }
}

pub(super) fn turn_arm<'a>() -> Track<'a> {
    Track {
        node: ARM,
        path: "rotation",
        interpolation: "LINEAR",
        times: &[0.0, 0.5],
        values: &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2],
    }
}

/// The skinned figure with a walk and a hit.
fn animated_figure() -> Vec<u8> {
    let mut writer: GltfWriter = figure();
    animate(&mut writer, Some("Walk"), &[move_hips()]);
    animate(&mut writer, Some("Hit"), &[turn_arm()]);
    writer.to_glb()
}

fn clips_on(clips: ClipSettings) -> ImportSettings {
    ImportSettings::Gltf(MeshSettings { clips: Some(clips), ..MeshSettings::default() })
}

#[test]
fn a_skinned_source_cooks_a_skeleton_its_mesh_names() {
    let tree = Tree::new("cook_skeleton");
    let source = tree.write("characters/figure.glb", &figure().to_glb());
    let report = tree.cook();
    let id = id_of(&source);
    let skeleton_id = meta::sub_asset_id(id, SKELETON);
    let [mesh_asset, skeleton_asset] = &report.assets[..] else { panic!("{:?}", report.assets) };
    assert_eq!((mesh_asset.id, skeleton_asset.id), (id, skeleton_id));
    assert_eq!(skeleton_asset.output, tree.out().join("characters/figure.sskel"));

    let mesh_bytes = read_aligned(&mesh_asset.output);
    let skeleton_bytes = read_aligned(&skeleton_asset.output);
    let mesh = Mesh::read(&mesh_bytes).unwrap();
    let skeleton = Skeleton::read(&skeleton_bytes).unwrap();
    let skin = mesh.skin.as_ref().expect("skinned");
    assert_eq!(skin.skeleton.as_ref().copied(), Some(skeleton_id));
    assert_eq!(skin.joint_count.to_native() as usize, skeleton.len());
    let names: Vec<&str> = skeleton.names.iter().map(|name| name.as_str()).collect();
    assert_eq!(names, ["Root", "Hips", "Arm"]);
    assert_eq!(skeleton.parent(2), Some(0));
    assert_eq!(skeleton.rest[1].translation.map(|value| value.to_native()), [0.0, 0.25, 0.0]);
}

#[test]
fn a_static_source_cooks_no_skeleton() {
    let tree = Tree::new("cook_no_skeleton");
    tree.write("mannequin.glb", &crate::samples::mannequin_glb());
    let report = tree.cook();
    let [mesh] = &report.assets[..] else { panic!("{:?}", report.assets) };
    let bytes = read_aligned(&mesh.output);
    assert!(Mesh::read(&bytes).unwrap().skin.is_none());
}

/// The cache's keys leave out a source's ID, so that a copy of a source is a hit. A cooked format
/// that holds an ID has to key it, or the second copy names the first's skeleton.
#[test]
fn copies_of_a_source_name_their_own_skeletons() {
    let tree = Tree::new("cook_skeleton_copies");
    let (first, second) =
        (tree.write("a/figure.glb", &figure().to_glb()), tree.write("b/figure.glb", &figure().to_glb()));
    let report = tree.cook();
    let named = |source: &Path| {
        let bytes = read_aligned(&by_id(&report, id_of(source)).output);
        Mesh::read(&bytes).unwrap().skin.as_ref().unwrap().skeleton.as_ref().copied().unwrap()
    };
    assert_eq!(named(&first), meta::sub_asset_id(id_of(&first), SKELETON));
    assert_eq!(named(&second), meta::sub_asset_id(id_of(&second), SKELETON));
    assert_ne!(named(&first), named(&second));

    // And the same through the cache: a clean output, cooked from nothing but the cache.
    fs::remove_dir_all(tree.out()).unwrap();
    let report = tree.cook();
    assert!(report.assets.iter().all(|asset| asset.status == CookStatus::Cached), "{:?}", report.assets);
    let named = |source: &Path| {
        let bytes = read_aligned(&by_id(&report, id_of(source)).output);
        Mesh::read(&bytes).unwrap().skin.as_ref().unwrap().skeleton.as_ref().copied().unwrap()
    };
    assert_eq!(named(&first), meta::sub_asset_id(id_of(&first), SKELETON));
    assert_eq!(named(&second), meta::sub_asset_id(id_of(&second), SKELETON));
}

#[test]
fn a_sources_animations_cook_only_when_its_sidecar_asks() {
    let tree = Tree::new("cook_clips_off");
    let source = tree.write("figure.glb", &animated_figure());
    let report = tree.cook();
    assert_eq!(statuses_of(&report, &source).len(), 2, "a mesh and a skeleton, as ever");
    assert!(report.assets.iter().all(|asset| asset.output.extension().unwrap() != CLIP_EXTENSION));
}

#[test]
fn clips_cook_beside_their_skeleton_each_an_asset_of_its_own() {
    let tree = Tree::new("cook_clips");
    let source = tree.write("characters/figure.glb", &animated_figure());
    let id = AssetId::from_bytes([0x11; 16]);
    let settings = ClipSettings {
        root_motion: Some(RootMotionSettings {
            bone: "Hips".into(),
            translation: Default::default(),
            rotation: Default::default(),
        }),
        ..ClipSettings::default()
    };
    tree.sidecar(&source, id, clips_on(settings));
    // `--locked` only needs the sidecar: nothing is written to the tree.
    let report = tree.try_cook(true).unwrap();
    let skeleton_id = meta::sub_asset_id(id, SKELETON);
    let (walk, hit) = (meta::sub_asset_id(id, "clip/Walk"), meta::sub_asset_id(id, "clip/Hit"));
    let out = tree.out().join("characters");
    assert_eq!(by_id(&report, walk).output, out.join("figure.Walk.sclip"));
    assert_eq!(by_id(&report, hit).output, out.join("figure.Hit.sclip"));

    let skeleton_bytes = read_aligned(&by_id(&report, skeleton_id).output);
    let skeleton = Skeleton::read(&skeleton_bytes).unwrap();
    let (walk_bytes, hit_bytes) =
        (read_aligned(&by_id(&report, walk).output), read_aligned(&by_id(&report, hit).output));
    let (walked, struck) = (Clip::read(&walk_bytes).unwrap(), Clip::read(&hit_bytes).unwrap());
    for clip in [walked, struck] {
        assert_eq!(clip.skeleton.as_ref().copied(), Some(skeleton_id));
        clip.check_skeleton(skeleton).unwrap();
    }
    assert_eq!((walked.name.as_str(), walked.ticks.to_native()), ("Walk", 60));
    assert_eq!((struck.name.as_str(), struck.ticks.to_native()), ("Hit", 30));
    let root = walked.root.as_ref().expect("root motion, from the sidecar");
    assert_eq!(clip_vector_end(walked, root), [1.0, 0.0, 3.0]);

    // A second cook touches nothing, a third with a new bound recooks clips and nothing else.
    let second = tree.cook();
    assert!(second.assets.iter().all(|asset| asset.status == CookStatus::UpToDate), "{:?}", second.assets);
    let tighter =
        ClipSettings { error: ReductionError { distance: 0.0001, weight: 0.001 }, ..ClipSettings::default() };
    tree.sidecar(&source, id, clips_on(tighter));
    let third = tree.cook();
    assert_eq!(by_id(&third, walk).status, CookStatus::Cooked);
    assert_eq!(by_id(&third, hit).status, CookStatus::Cooked);
    assert_ne!(by_id(&third, walk).key, by_id(&report, walk).key);
}

/// Where root motion ends, in meters to a millimeter's rounding.
fn clip_vector_end(
    clip: &sapphire_assets::clip::ArchivedClip,
    root: &sapphire_assets::clip::ArchivedRootMotion,
) -> [f32; 3] {
    let end = clip.vector(&root.translation, f32::from(clip.ticks.to_native()));
    end.map(|value| (value * 1000.0).round() / 1000.0)
}

#[test]
fn clips_with_names_a_filesystem_would_collapse_get_files_of_their_own() {
    let tree = Tree::new("cook_clip_files");
    let mut writer = figure();
    animate(&mut writer, Some("Hit"), &[move_hips()]);
    animate(&mut writer, Some("hit"), &[turn_arm()]);
    animate(&mut writer, Some("a/b"), &[move_hips()]);
    animate(&mut writer, None, &[turn_arm()]);
    let source = tree.write("figure.glb", &writer.to_glb());
    tree.sidecar(&source, AssetId::from_bytes([0x22; 16]), clips_on(ClipSettings::default()));
    let report = tree.cook();
    let mut files: Vec<String> = report
        .assets
        .iter()
        .filter(|asset| asset.output.extension().unwrap() == CLIP_EXTENSION)
        .map(|asset| asset.output.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["figure.Hit-0.sclip", "figure._3.sclip", "figure.a_b.sclip", "figure.hit-1.sclip"]);
}

/// A source's own names can match what parts two others: that's three clips and three files, each by
/// the ID its own name gives it, not a collision to refuse.
#[test]
fn a_clip_named_like_a_made_up_file_gets_a_file_of_its_own() {
    let tree = Tree::new("cook_clip_files_made_up");
    let mut writer = figure();
    animate(&mut writer, Some("a/b"), &[move_hips()]);
    animate(&mut writer, Some("a?b"), &[turn_arm()]);
    animate(&mut writer, Some("a_b-0"), &[move_hips()]);
    let source = tree.write("figure.glb", &writer.to_glb());
    let id = AssetId::from_bytes([0x22; 16]);
    tree.sidecar(&source, id, clips_on(ClipSettings::default()));
    let report = tree.try_cook(true).unwrap();
    let rows =
        [("a/b", "figure.a_b-0-0.sclip"), ("a?b", "figure.a_b-1.sclip"), ("a_b-0", "figure.a_b-0-2.sclip")];
    for (name, file) in rows {
        let asset = by_id(&report, meta::sub_asset_id(id, &format!("clip/{name}")));
        assert_eq!(asset.output, tree.out().join(file));
        let bytes = read_aligned(&asset.output);
        assert_eq!(Clip::read(&bytes).unwrap().name.as_str(), name);
    }
}

#[test]
fn clip_files_are_their_names_unless_two_would_be_one_file() {
    let rows: [(&[&str], &[&str]); 6] = [
        (&["Idle", "Walk"], &["Idle", "Walk"]),
        (&["Hit", "hit"], &["Hit-0", "hit-1"]),
        (&["??", "//"], &["__-0", "__-1"]),
        (&["a/b", "a?b", "a_b-0"], &["a_b-0-0", "a_b-1", "a_b-0-2"]),
        // Each round's suffixes meet a name of their own, and case doesn't part them.
        (
            &["x/y", "x?y", "x_y-0", "x_y-0-0", "X_Y-0-2"],
            &["x_y-0-0-0", "x_y-1", "x_y-0-2-2", "x_y-0-0-3", "X_Y-0-2-4"],
        ),
        (&["", "\u{e9}", "_"], &["", "_-1", "_-2"]),
    ];
    for (names, expected) in rows {
        let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
        assert_eq!(clip_files(&names), expected, "{names:?}");
    }
}

#[test]
fn a_clip_the_source_no_longer_has_is_pruned() {
    let tree = Tree::new("cook_clip_pruned");
    let source = tree.write("figure.glb", &animated_figure());
    tree.sidecar(&source, AssetId::from_bytes([0x33; 16]), clips_on(ClipSettings::default()));
    tree.cook();
    let hit = tree.out().join("figure.Hit.sclip");
    assert!(hit.exists());
    let mut writer = figure();
    animate(&mut writer, Some("Walk"), &[move_hips()]);
    tree.write("figure.glb", &writer.to_glb());
    let report = tree.cook();
    assert!(!hit.exists());
    assert!(report.removed.contains(&hit), "{:?}", report.removed);
}

/// The kinds and codecs, and the runtime's reads: the pack's references hold the mesh to its
/// skeleton and each clip to it, so a load brings it along.
#[test]
fn skeletons_and_clips_pack_and_load_by_their_ids() {
    let tree = Tree::new("cook_clip_pack");
    let source = tree.write("figure.glb", &animated_figure());
    let id = AssetId::from_bytes([0x44; 16]);
    tree.sidecar(&source, id, clips_on(ClipSettings::default()));
    let report = tree.cook();
    let packs = tree.dir.path().join("packs");
    pack_cooked(&report, &packs, None).unwrap();
    let bytes = fs::read(packs.join("shared.spack")).unwrap();
    let (skeleton_id, walk) = (meta::sub_asset_id(id, SKELETON), meta::sub_asset_id(id, "clip/Walk"));
    {
        let reader = PackReader::new(&bytes).unwrap();
        assert_eq!(reader.toc().find(skeleton_id).expect("packed").kind, AssetKind::Skeleton);
        assert_eq!(reader.toc().find(walk).expect("packed").kind, AssetKind::Clip);
    }
    let packs = Arc::new(MemoryPackSource::new());
    packs.insert(SHARED, bytes);
    let mut server = AssetServer::new(packs, AssetBudgets::DESKTOP);
    server.mount(SHARED).unwrap();
    let clip: Handle<Clip> = server.load(walk).unwrap();
    server.finish(&clip, |_| Err("clips need no upload".into())).unwrap();
    assert_eq!(server.get(&clip).unwrap().name.as_str(), "Walk");
    let skeleton: Handle<Skeleton> = server.load(skeleton_id).unwrap();
    server.finish(&skeleton, |_| Err("skeletons need no upload".into())).unwrap();
    server.get(&clip).unwrap().check_skeleton(server.get(&skeleton).unwrap()).unwrap();
}

#[test]
fn a_vrm_cooks_no_clips_and_says_so() {
    let tree = Tree::new("cook_clips_refused");
    let source = tree.write("sample.vrm", &samples::vrm1().to_glb());
    let mut vrm = VrmSettings::default();
    vrm.mesh.clips = Some(ClipSettings::default());
    tree.sidecar(&source, AssetId::from_bytes([0x55; 16]), ImportSettings::Vrm(vrm));
    let error = tree.try_cook(false).unwrap_err();
    assert!(error.to_string().contains("sample.vrm"), "{error}");
    assert!(
        std::error::Error::source(&error).unwrap().to_string().contains("sources don't cook clips"),
        "{error:?}"
    );
}

#[test]
fn clip_settings_out_of_range_are_errors() {
    let source_name = "figure.glb";
    for (name, settings) in [
        ("rate_zero", ClipSettings { rate: 0, ..ClipSettings::default() }),
        ("rate_huge", ClipSettings { rate: 9000, ..ClipSettings::default() }),
        (
            "error_negative",
            ClipSettings { error: ReductionError { distance: -1.0, weight: 0.0 }, ..ClipSettings::default() },
        ),
        (
            "error_nan",
            ClipSettings {
                error: ReductionError { distance: 0.0, weight: f32::NAN },
                ..ClipSettings::default()
            },
        ),
    ] {
        let tree = Tree::new(&format!("cook_clip_settings_{name}"));
        let source = tree.write(source_name, &animated_figure());
        tree.sidecar(&source, AssetId::from_bytes([0x66; 16]), clips_on(settings));
        let error = tree.try_cook(false).expect_err(name);
        assert!(error.to_string().contains(source_name), "{name}: {error}");
    }
}

const SWING: &str = "(
    type: \"sapphire::pipeline::KeyPoseClip\",
    version: 1,
    data: (
        keys: [
            (tick: 0, pose: { LeftUpperArm: (z: -70) }),
            (tick: 10, ease: Overshoot(0.1), root: (0, 0, 0.5), pose: { LeftUpperArm: (z: 20), Head: (y: 15) }),
        ],
    ),
)";

/// A tree with the sample character and a document for it.
fn key_pose_tree(name: &str) -> (Tree, PathBuf, AssetId, PathBuf, AssetId) {
    let tree = Tree::new(name);
    let character = tree.write("characters/sample.vrm", &samples::vrm1().to_glb());
    let character_id = AssetId::from_bytes([0x77; 16]);
    tree.sidecar(&character, character_id, ImportSettings::for_new_sidecar(&character).unwrap());
    let document = tree.write("moves/swing.keypose.ron", SWING.as_bytes());
    let document_id = AssetId::from_bytes([0x78; 16]);
    let settings = KeyPoseSettings { rig: Some(character_id), ..KeyPoseSettings::default() };
    tree.sidecar(&document, document_id, ImportSettings::KeyPose(settings));
    (tree, character, character_id, document, document_id)
}

#[test]
fn a_key_pose_document_cooks_for_the_character_its_sidecar_names() {
    let (tree, character, character_id, _, document_id) = key_pose_tree("cook_keypose");
    let report = tree.try_cook(true).expect("--locked, with both sidecars");
    let cooked = by_id(&report, document_id);
    assert_eq!(cooked.output, tree.out().join("moves/swing.sclip"));
    let clip_bytes = read_aligned(&cooked.output);
    let clip = Clip::read(&clip_bytes).unwrap();
    assert_eq!(clip.name.as_str(), "swing");
    let skeleton_id = meta::sub_asset_id(character_id, SKELETON);
    assert_eq!(clip.skeleton.as_ref().copied(), Some(skeleton_id));
    let skeleton_bytes = read_aligned(&by_id(&report, skeleton_id).output);
    clip.check_skeleton(Skeleton::read(&skeleton_bytes).unwrap()).unwrap();
    assert!(clip.root.is_some());
    // The tracks are the bones it names, by the character's rig.
    let rig = vrm::import_rig(&fs::read(&character).unwrap(), None).unwrap();
    let joints: Vec<u16> = clip.rotations.iter().map(|track| track.joint.to_native()).collect();
    let (arm, head) =
        (rig.human_joint(HumanBone::LeftUpperArm).unwrap(), rig.human_joint(HumanBone::Head).unwrap());
    assert_eq!(joints, {
        let mut expected = vec![arm, head];
        expected.sort_unstable();
        expected
    });

    // Nothing changed: a hit.
    let second = tree.cook();
    assert_eq!(by_id(&second, document_id).status, CookStatus::UpToDate);
    // The document changed: it recooks, alone.
    tree.write("moves/swing.keypose.ron", SWING.replace("z: 20", "z: 30").as_bytes());
    let third = tree.cook();
    assert_eq!(by_id(&third, document_id).status, CookStatus::Cooked);
    assert_eq!(by_id(&third, skeleton_id).status, CookStatus::UpToDate);
    // The character's skeleton changed: it recooks too, though the document didn't.
    let mut taller = samples::vrm1();
    taller.document["nodes"][samples::node::HIPS]["translation"] = serde_json::json!([0.0, 1.0, 0.0]);
    tree.write("characters/sample.vrm", &taller.to_glb());
    let fourth = tree.cook();
    assert_eq!(by_id(&fourth, skeleton_id).status, CookStatus::Cooked);
    assert_eq!(by_id(&fourth, document_id).status, CookStatus::Cooked);
}

#[test]
fn a_key_pose_document_says_when_its_character_cant_be_found() {
    // A first cook writes a sidecar with no character: its source says what's missing.
    let tree = Tree::new("cook_keypose_no_rig");
    let document = tree.write("swing.keypose.ron", SWING.as_bytes());
    let error = tree.try_cook(true).unwrap_err();
    assert!(matches!(error, CookError::Meta(MetaError::Missing { .. })), "--locked: {error:?}");
    let error = tree.try_cook(false).unwrap_err();
    assert!(error.to_string().contains("swing.keypose.ron"), "{error}");
    let says = std::error::Error::source(&error).unwrap().to_string();
    assert!(says.contains("names no `rig`"), "{says}");
    assert!(sidecar_path(&document).exists(), "the sidecar waits to be edited");

    // A source that isn't in the tree, or has no rig.
    let (tree, _, _, document, id) = key_pose_tree("cook_keypose_unknown_rig");
    let settings =
        KeyPoseSettings { rig: Some(AssetId::from_bytes([0x79; 16])), ..KeyPoseSettings::default() };
    tree.sidecar(&document, id, ImportSettings::KeyPose(settings));
    let error = tree.try_cook(false).unwrap_err();
    assert!(std::error::Error::source(&error).unwrap().to_string().contains("isn't a source"), "{error:?}");

    let (tree, _, _, document, id) = key_pose_tree("cook_keypose_no_humanoid_rig");
    let figure_source = tree.write("figure.glb", &figure().to_glb());
    let figure_id = AssetId::from_bytes([0x7a; 16]);
    tree.sidecar(&figure_source, figure_id, ImportSettings::Gltf(MeshSettings::default()));
    let settings = KeyPoseSettings { rig: Some(figure_id), ..KeyPoseSettings::default() };
    tree.sidecar(&document, id, ImportSettings::KeyPose(settings));
    let error = tree.try_cook(false).unwrap_err();
    assert!(std::error::Error::source(&error).unwrap().to_string().contains("isn't a source"), "{error:?}");

    // A document with a fault of its own names its file.
    let (tree, _, _, _, _) = key_pose_tree("cook_keypose_bad_document");
    tree.write("moves/swing.keypose.ron", SWING.replace("tick: 10", "tick: 0").as_bytes());
    let error = tree.try_cook(false).unwrap_err();
    assert!(error.to_string().contains("swing.keypose.ron"), "{error}");
    assert!(
        std::error::Error::source(&error).unwrap().to_string().contains("key 1 is at tick 0"),
        "{error:?}"
    );
}

/// Everything that's new, cooked twice from nothing: the same bytes, and so the same packs.
#[test]
fn a_cook_is_byte_identical_run_to_run() {
    let cook_all = |name: &str| {
        let (tree, _, _, _, _) = key_pose_tree(name);
        let source = tree.write("figure.glb", &animated_figure());
        tree.sidecar(&source, AssetId::from_bytes([0x88; 16]), clips_on(ClipSettings::default()));
        animation_tests::add_every_kind_of_animation_source(&tree);
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
        let report = cook_dir(&tree.content(), &tree.out(), &options).unwrap();
        let packs = tree.dir.path().join("packs");
        pack_cooked(&report, &packs, None).unwrap();
        let mut outputs: Vec<(AssetId, Vec<u8>)> =
            report.assets.iter().map(|asset| (asset.id, fs::read(&asset.output).unwrap())).collect();
        outputs.sort_by_key(|(id, _)| *id);
        (outputs, fs::read(packs.join("shared.spack")).unwrap())
    };
    let (first, first_pack) = cook_all("cook_identical_first");
    let (second, second_pack) = cook_all("cook_identical_second");
    assert!(first.len() >= 15, "{}", first.len());
    assert_eq!(first.len(), second.len());
    for ((id, a), (other, b)) in first.iter().zip(&second) {
        assert_eq!(id, other);
        assert!(a == b, "{id} differs between runs");
    }
    assert!(first_pack == second_pack, "the packs differ");
}
