//! A move from a clip through a whole cook: which clip it names, what recooks it, what it refuses, and
//! the cooked set played in the simulation against the pose the same clip draws. What a table holds and
//! the bytes it holds it in are `moves::tests`.

use glam::Vec3;
use sapphire_anim::{CharacterPose, root_motion};
use sapphire_assets::AssetKind;
use sapphire_assets::clip::Clip;
use sapphire_assets::rig::{CharacterRig, HumanBone};
use sapphire_assets::skeleton::Skeleton;
use sapphire_core::Name;
use sapphire_gameplay::{ActorSpawn, ActorTransform, ActorView, SimConfig, Simulation, TickActions};
use sapphire_math::det;
use sapphire_moves::RootSample;

use super::animation_tests::{CHARACTER, HUMAN, clips_of, fixture, human_with_a_mesh, to_the_character};
use super::clip_tests::{Tree, by_id};
use super::*;
use crate::clip::retarget::HumanMap;
use crate::import::vrm::samples;
use crate::keypose::KeyPoseSettings;
use crate::meta::ImportSettings;
use crate::moves::MoveSetSettings;
use crate::test_util::read_aligned;

const DOCUMENT: AssetId = AssetId::from_bytes([0xb0; 16]);
const SET: AssetId = AssetId::from_bytes([0xb1; 16]);

/// A swing of 20 ticks that travels, turns, and moves the arm its hit follows.
const SWING: &str = "(
    type: \"sapphire::pipeline::KeyPoseClip\",
    version: 1,
    data: (
        ticks: 20,
        keys: [
            (tick: 0, pose: { RightUpperArm: (y: -30, z: 40), Spine: (x: 5) }),
            (tick: 8, ease: Out, hips: (0, -0.05, 0), root: (0, 0, 0.5), root_turn: 45, pose: {
                RightUpperArm: (x: 20, y: 40, z: 60),
                RightLowerArm: (y: 30, z: 10),
                Spine: (x: -5, y: 25),
            }),
            (tick: 16, ease: Linear, root: (0.25, 0, 1.0), root_turn: 90, pose: {
                RightUpperArm: (y: 70, z: 30),
            }),
        ],
    ),
)";

/// A set of one move of the given phases, whose `motion` is `clip`, and a hit on ticks 8 to 15.
fn set_with(clip: &str, (startup, active, recovery): (u32, u32, u32)) -> String {
    format!(
        "(
            type: \"sapphire::moves::MoveSetDocument\",
            version: 1,
            data: (
                intents: [(name: \"light\", button: Light, buffer: 12)],
                moves: [(
                    name: \"swing\",
                    startup: {startup},
                    active: {active},
                    recovery: {recovery},
                    motion: (clip: {clip}),
                    hitboxes: [(name: \"swing_hit\", from: 8, to: 15, path: \"blade\", shape: Sphere(radius: 0.3))],
                )],
            ),
        )"
    )
}

/// The clip block of a move on `SWING`, with a socket on the right hand.
fn swing_clip() -> String {
    format!(
        "(source: \"{DOCUMENT}\", paths: [(name: \"blade\", follows: Bone(\"RightHand\"), offset: (0.0, 0.1, 0.9))])"
    )
}

/// The sample character, `SWING` for it and a set of moves named by `set`.
fn swing_tree(name: &str, set: &str) -> Tree {
    let tree = Tree::new(name);
    let character = tree.write("characters/sample.vrm", &samples::vrm1().to_glb());
    tree.sidecar(&character, CHARACTER, ImportSettings::for_new_sidecar(&character).unwrap());
    let document = tree.write("moves/swing.keypose.ron", SWING.as_bytes());
    let settings = KeyPoseSettings { rig: Some(CHARACTER), ..KeyPoseSettings::default() };
    tree.sidecar(&document, DOCUMENT, ImportSettings::KeyPose(settings));
    write_set(&tree, set);
    tree
}

fn write_set(tree: &Tree, set: &str) {
    let source = tree.write("moves/fighter.moves.ron", set.as_bytes());
    tree.sidecar(&source, SET, ImportSettings::MoveSet(MoveSetSettings::default()));
}

fn cooked_set(report: &CookReport) -> sapphire_serialize::cooked::AlignedVec {
    read_aligned(&by_id(report, SET).output)
}

/// What a cook's error says of a document, having checked it names the set's file.
fn says(error: &CookError) -> String {
    assert!(matches!(error, CookError::MoveSet { .. }), "{error:?}");
    assert!(error.to_string().contains("fighter.moves.ron"), "{error}");
    std::error::Error::source(error).expect("a fault").to_string()
}

/// Adding a move set and a clip is two files with sidecars: their names reach a pack, on the IDs the sidecars
/// say, and nothing in Rust lists them. What a game finds them by is this.
#[test]
fn a_new_set_and_a_new_clip_are_found_in_a_pack_by_their_names() {
    let tree = swing_tree("cook_named_in_pack", &set_with(&swing_clip(), (8, 7, 5)));
    let packs = tree.dir.path().join("packs");
    let named_in_packs = |report: &CookReport| {
        crate::pack::pack_cooked(report, &packs, None).expect("packs");
        let mut named = Vec::new();
        for entry in fs::read_dir(&packs).expect("packs").flatten() {
            let bytes = fs::read(entry.path()).expect("a pack");
            let reader = sapphire_assets::pack::PackReader::new(&bytes).expect("reads");
            for (asset, name) in reader.toc().named() {
                named.push((
                    name.to_owned(),
                    asset.kind,
                    asset.id,
                    reader.toc().dependencies(asset).to_vec(),
                ));
            }
        }
        named.sort_by(|a, b| a.0.cmp(&b.0));
        named
    };

    let report = tree.try_cook(false).unwrap();
    let skeleton = meta::sub_asset_id(CHARACTER, SKELETON);
    let first = named_in_packs(&report);
    assert_eq!(
        first,
        [
            ("fighter".to_owned(), AssetKind::MoveSet, SET, vec![]),
            ("swing".to_owned(), AssetKind::Clip, DOCUMENT, vec![skeleton]),
        ],
        "only the kinds asked for by name carry one",
    );

    let added = AssetId::from_bytes([0xb2; 16]);
    let document = tree.write("moves/lunge.keypose.ron", SWING.as_bytes());
    let settings = KeyPoseSettings { rig: Some(CHARACTER), ..KeyPoseSettings::default() };
    tree.sidecar(&document, added, ImportSettings::KeyPose(settings));
    let report = tree.try_cook(false).unwrap();
    let second = named_in_packs(&report);
    assert!(second.iter().any(|(name, kind, id, deps)| name == "lunge"
        && *kind == AssetKind::Clip
        && *id == added
        && deps == &[skeleton]));

    // A rename is a new name on the same ID, and it reaches a pack that holds the same bytes otherwise.
    fs::rename(&document, tree.content().join("moves/thrust.keypose.ron")).expect("renamed");
    fs::rename(
        tree.content().join("moves/lunge.keypose.ron.meta"),
        tree.content().join("moves/thrust.keypose.ron.meta"),
    )
    .expect("renamed");
    let report = tree.try_cook(false).unwrap();
    let third = named_in_packs(&report);
    assert!(third.iter().any(|(name, _, id, _)| name == "thrust" && *id == added));
    assert!(!third.iter().any(|(name, ..)| name == "lunge"));
}

/// Live tuning follows a set by its file's name, so two with one can't both be followed: said at the cook.
#[test]
fn two_move_sets_with_one_name_stop_the_pack_by_name() {
    let tree = swing_tree("cook_two_sets", &set_with(&swing_clip(), (8, 7, 5)));
    let other = tree.write("other/fighter.moves.ron", set_with(&swing_clip(), (8, 7, 5)).as_bytes());
    tree.sidecar(
        &other,
        AssetId::from_bytes([0xb3; 16]),
        ImportSettings::MoveSet(MoveSetSettings::default()),
    );
    let report = tree.try_cook(false).unwrap();
    let error = crate::pack::pack_cooked(&report, &tree.dir.path().join("packs"), None).unwrap_err();
    assert!(matches!(error, crate::pack::PackError::MoveSetNameClash { .. }), "{error}");
    let text = error.to_string();
    assert!(text.contains("moves/fighter.moves.ron") && text.contains("other/fighter.moves.ron"), "{text}");
}

/// The pose a clip draws at a tick: a joint's position with an offset and its rotation, in model space,
/// through the runtime's own functions, and the root at that tick.
struct OnScreen {
    pose: CharacterPose,
    joint: usize,
}

impl OnScreen {
    fn at(&mut self, clip: &sapphire_assets::clip::ArchivedClip, tick: u32) -> (Vec3, f32, Vec3) {
        let position = tick as f32;
        self.pose.sample(clip, position);
        self.pose.finish();
        let (_, rotation, translation) = self.pose.model()[self.joint].to_scale_rotation_translation();
        let root = root_motion(clip, position).expect("the swing has root motion");
        let socket = translation + rotation * Vec3::new(0.0, 0.1, 0.9);
        (root.translation, det::yaw_of_quat(root.rotation), socket)
    }
}

/// The brief's test: a move baked from a clip moves the root by the clip's motion and publishes hit
/// paths where the clip's bone is, through `MoveRead` and `ActiveVolume`, for an actor standing and
/// facing anywhere, with nothing in the step knowing a clip was involved.
#[test]
fn a_move_from_a_clip_plays_in_the_simulation_where_the_clip_draws_it() {
    let tree = swing_tree("cook_move_sim", &set_with(&swing_clip(), (8, 7, 5)));
    let report = tree.try_cook(false).unwrap();
    let set = cooked_set(&report);

    // What the same cook wrote for the screen: the clip and its skeleton.
    let clip_bytes = read_aligned(&by_id(&report, DOCUMENT).output);
    let clip = Clip::read(&clip_bytes).unwrap();
    let skeleton_bytes = read_aligned(&by_id(&report, meta::sub_asset_id(CHARACTER, SKELETON)).output);
    let skeleton = Skeleton::read(&skeleton_bytes).unwrap();
    let rig_bytes = read_aligned(&by_id(&report, meta::sub_asset_id(CHARACTER, vrm::RIG.name)).output);
    let rig = CharacterRig::from_cooked(&rig_bytes).unwrap();
    let hand = HumanMap::from_rig(&rig).joint(HumanBone::RightHand).expect("a right hand");
    let pose = CharacterPose::new(skeleton, &[], &rig.constraints, Vec::new());
    let mut screen = OnScreen { pose, joint: usize::from(hand) };

    let start = ActorTransform { position: Vec3::new(3.0, 0.5, -2.0), yaw: 0.7 };
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_cooked_move_set(&set).unwrap();
    let spawn =
        ActorSpawn { position: start.position, yaw: start.yaw, moves: Some(moves), ..ActorSpawn::default() };
    let actor = sim.spawn(spawn).unwrap();
    assert!(sim.force_move(actor, Name::new("swing")));

    let (mut read, mut hits) = (0u32, 0u32);
    for _ in 0..30 {
        sim.step(&TickActions::NEUTRAL);
        let view: ActorView = sim.actor(actor).unwrap();
        let Some(current) = view.moves.current else { break };
        let tick = u32::try_from(current.elapsed.whole_ticks()).unwrap();
        read = read.max(tick);

        // The root: the clip's motion, in the actor's frame at the move's start.
        let (root, turn, _) = screen.at(clip, tick);
        let expected = start.position + det::rotate_y(root, start.yaw);
        assert!(
            view.position.abs_diff_eq(expected, 1e-3),
            "tick {tick}: {} against {expected}",
            view.position
        );
        let turned = view.yaw - start.yaw;
        assert!(det::wrap_angle(turned - turn).abs() < 1e-3, "tick {tick}: {turned} against {turn}");

        // The hit: where the clip's bone is, in the world, by `ActiveVolume`.
        for volume in view.moves.volumes.iter() {
            let at = u32::try_from(volume.to.whole_ticks()).unwrap();
            assert_eq!(volume.path, Name::new("blade"));
            assert!((8..15).contains(&at), "tick {at} is outside its window");
            let (root, turn, socket) = screen.at(clip, at);
            let drawn = start.position + det::rotate_y(root + det::rotate_y(socket, turn), start.yaw);
            let here = ActorTransform { position: view.position, yaw: view.yaw };
            let published = here.point(volume.end.offset());
            assert!(published.abs_diff_eq(drawn, 2e-3), "tick {at}: {published} against {drawn}");
            hits += 1;
        }
    }
    assert_eq!(read, 19, "played to its last tick");
    assert_eq!(hits, 7, "ticks 8 to 14");
}

/// An animation source's clip by `name`, on its own rig or, by `rig`, on one its sidecar retargets it
/// to: the same clip on two rigs, so the same bone ends in two places.
#[test]
fn a_clip_is_named_as_its_sidecar_says() {
    let ticks_of = |report: &CookReport, id| {
        let bytes = read_aligned(&by_id(report, id).output);
        u32::from(Clip::read(&bytes).unwrap().ticks.to_native())
    };

    // A first cook says how long the clips are, as a person finds out from the error.
    let tree = Tree::new("cook_move_names");
    let character = tree.write("characters/sample.vrm", &samples::vrm1().to_glb());
    tree.sidecar(&character, CHARACTER, ImportSettings::for_new_sidecar(&character).unwrap());
    let human = tree.write("moves/human.glb", &human_with_a_mesh());
    tree.sidecar(&human, HUMAN, clips_of(to_the_character(&["Lower"])));
    let report = tree.cook();
    let own = sapphire_assets::graph::imported_clip_id(HUMAN, "Lower", None);
    let retargeted = sapphire_assets::graph::imported_clip_id(HUMAN, "Lower", Some(CHARACTER));
    let own_ticks = ticks_of(&report, own);
    assert_eq!(ticks_of(&report, retargeted), own_ticks);

    let hit = |name: &str| {
        format!("hitboxes: [(name: \"hit\", from: 0, to: 1, path: \"{name}\", shape: Sphere(radius: 0.1))],")
    };
    let moves = |clips: &[(&str, String)]| {
        let moves: String = clips
            .iter()
            .map(|(name, clip)| {
                format!(
                    "(name: \"{name}\", startup: 0, active: 1, recovery: {},
                     motion: (clip: {clip}), {})",
                    own_ticks - 1,
                    hit("tip")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "(type: \"sapphire::moves::MoveSetDocument\", version: 1, data: (
                intents: [(name: \"light\", buffer: 12)], moves: [{moves}]))"
        )
    };
    let tip = "paths: [(name: \"tip\", follows: Bone(\"LeftHand\"))]";
    write_set(
        &tree,
        &moves(&[
            ("on_its_own_rig", format!("(source: \"{HUMAN}\", name: \"Lower\", {tip})")),
            (
                "on_the_character",
                format!("(source: \"{HUMAN}\", name: \"Lower\", rig: \"{CHARACTER}\", {tip})"),
            ),
        ]),
    );
    let report = tree.cook();
    let bytes = cooked_set(&report);
    let set = MoveSet::from_cooked(&bytes).unwrap();
    let tip_of = |name: &str| {
        let (_, mv) = set.find(Name::intern(name)).unwrap();
        mv.tables.path(Name::new("tip")).unwrap().samples().last().copied().unwrap()
    };
    let (on_own, on_character) = (tip_of("on_its_own_rig"), tip_of("on_the_character"));
    assert_ne!(
        on_own.translation, on_character.translation,
        "they're two rigs of two proportions, and their hands end in different places"
    );
}

/// A move recooks when what it reads does, and only then.
#[test]
fn a_set_recooks_when_its_clip_or_the_rig_changes() {
    let tree = swing_tree("cook_move_keys", &set_with(&swing_clip(), (8, 7, 5)));
    let first = tree.try_cook(false).unwrap();
    let bytes = fs::read(&by_id(&first, SET).output).unwrap();

    // Nothing changed, and a locked cook finds every sidecar it needs.
    assert_eq!(by_id(&tree.try_cook(true).unwrap(), SET).status, CookStatus::UpToDate);

    // The document's clip changed: it recooks, and its table follows.
    tree.write(
        "moves/swing.keypose.ron",
        SWING.replace("root: (0, 0, 0.5)", "root: (0, 0, 0.75)").as_bytes(),
    );
    let second = tree.cook();
    assert_eq!(by_id(&second, SET).status, CookStatus::Cooked);
    let moved = fs::read(&by_id(&second, SET).output).unwrap();
    assert_ne!(moved, bytes);

    // The character's skeleton changed, and neither document did.
    let mut taller = samples::vrm1();
    taller.document["nodes"][samples::node::SPINE]["translation"] = serde_json::json!([0.0, 0.2, 0.0]);
    tree.write("characters/sample.vrm", &taller.to_glb());
    let third = tree.cook();
    assert_eq!(by_id(&third, SET).status, CookStatus::Cooked);
    assert_ne!(fs::read(&by_id(&third, SET).output).unwrap(), moved);

    // Back to the first of everything: the cache has it, byte for byte.
    tree.write("moves/swing.keypose.ron", SWING.as_bytes());
    tree.write("characters/sample.vrm", &samples::vrm1().to_glb());
    let fourth = tree.cook();
    assert_eq!(by_id(&fourth, SET).status, CookStatus::Cached);
    assert_eq!(fs::read(&by_id(&fourth, SET).output).unwrap(), bytes);
}

/// A bake reads its clip's source for itself, after the clip has cooked, and a set's key holds only the
/// clip's key. A save in between must not cook the old clip beside the new one's tables, nor leave those
/// tables in the cache for when the file is put back.
#[test]
fn a_clips_source_saved_before_a_move_bakes_fails_rather_than_leave_stale_tables() {
    let set = set_with(&swing_clip(), (8, 7, 5));
    let tree = swing_tree("cook_move_saved", &set);
    let document = tree.content().join("moves/swing.keypose.ron");
    let edited = SWING.replace("root: (0, 0, 0.5)", "root: (0, 0, 0.75)");
    let error = tree
        .try_cook_through(&|stage| {
            if stage == CookStage::ClipsCooked {
                tree.write_at(&document, edited.as_bytes());
            }
        })
        .map(|_| "it cooked")
        .unwrap_err();
    let source = tree.content().join("moves/fighter.moves.ron");
    assert!(matches!(&error, CookError::SourceChanged { path } if *path == source), "{error:?}");

    tree.write_at(&document, SWING.as_bytes());
    let report = tree.cook();
    let clean_tree = swing_tree("cook_move_saved_clean", &set);
    let clean = clean_tree.cook();
    let set_bytes = |report: &CookReport| fs::read(&by_id(report, SET).output).unwrap();
    assert_eq!(set_bytes(&report), set_bytes(&clean), "put back, it's the set of that file");
}

/// Two cooks in two trees write one set of bytes, and it's one value on every architecture: these are
/// the bytes `Simulation::tables_hash` hashes, so CI's `Test` job holds them on Linux and Windows too.
/// `moves::tests::baked_tables_and_clips_hash_the_same_on_every_architecture` says what to do when
/// only some of them fail. If the document here, a stage or `MoveSet`'s format moves this, update it.
#[test]
fn a_set_with_a_clip_cooks_to_one_set_of_bytes() {
    let cooked = |name| {
        let tree = swing_tree(name, &set_with(&swing_clip(), (8, 7, 5)));
        let report = tree.try_cook(false).unwrap();
        fs::read(&by_id(&report, SET).output).unwrap()
    };
    let bytes = cooked("cook_move_bytes_a");
    assert_eq!(bytes, cooked("cook_move_bytes_b"));
    assert_eq!(blake3::hash(&bytes).to_hex().as_str(), COOKED_SET);
}

const COOKED_SET: &str = "13c8897414ada980a66df225b8683ecc20543aa0e8728c1c13b824917be6cd3f";

/// A hit path through a joint a rig constraint drives, in a real cook: the sample character's twist
/// helper follows its hand, which a table couldn't.
#[test]
fn a_path_through_a_constraint_is_refused() {
    let clip = format!(
        "(source: \"{DOCUMENT}\", paths: [(name: \"blade\", follows: Joint(\"LeftLowerArmTwist\"))])"
    );
    let tree = swing_tree("cook_move_constraint", &set_with(&clip, (8, 7, 5)));
    let said = says(&tree.try_cook(false).unwrap_err());
    assert!(said.contains("move `swing`, track `motion`") && said.contains("LeftLowerArmTwist"), "{said}");
    assert!(said.contains("constraint"), "{said}");
}

/// What's wrong, the clip block and phases that say it, and what the cook must say back.
type FaultRow = (&'static str, String, (u32, u32, u32), &'static [&'static str]);

/// One bad reference per row: what it says names the move and the fault, and the whole cook fails, by
/// the file's name, with every issue at once.
#[test]
fn a_clip_that_cant_bake_says_why_by_move_and_track() {
    let source = |text: String| {
        format!("(source: \"{text}\", paths: [(name: \"blade\", follows: Bone(\"RightHand\"))])")
    };
    let unknown = AssetId::from_bytes([0xee; 16]);
    let rows: Vec<FaultRow> = vec![
        (
            "an ID that isn't",
            source("not an id".to_owned()),
            (8, 7, 5),
            &["`source`", "not an id", "isn't an asset ID"],
        ),
        (
            "a source that isn't there",
            source(unknown.to_string()),
            (8, 7, 5),
            &["isn't a source of this content tree"],
        ),
        ("a source that isn't a clip's", source(SET.to_string()), (8, 7, 5), &["cooks no clips"]),
        ("a character, which cooks none", source(CHARACTER.to_string()), (8, 7, 5), &["cooks no clips"]),
        (
            "a name on a key-pose document",
            format!("(source: \"{DOCUMENT}\", name: \"swing\")"),
            (8, 7, 5),
            &["takes no `name`"],
        ),
        (
            "a rig its sidecar doesn't name",
            format!("(source: \"{DOCUMENT}\", rig: \"{unknown}\")"),
            (8, 7, 5),
            &["doesn't cook this clip", "`also`"],
        ),
        (
            "a rig that isn't an ID",
            format!("(source: \"{DOCUMENT}\", rig: \"somebody\")"),
            (8, 7, 5),
            &["`rig`", "somebody"],
        ),
        (
            "a bone that isn't one",
            format!("(source: \"{DOCUMENT}\", paths: [(name: \"blade\", follows: Bone(\"Sword\"))])"),
            (8, 7, 5),
            &["`blade`", "Sword", "isn't a humanoid bone"],
        ),
        (
            "a joint the skeleton lacks",
            format!("(source: \"{DOCUMENT}\", paths: [(name: \"blade\", follows: Joint(\"Sword\"))])"),
            (8, 7, 5),
            &["`blade`", "Sword", "doesn't have"],
        ),
        (
            "a move shorter than its clip",
            swing_clip(),
            (8, 7, 4),
            &["the clip is 20 ticks long", "add up to 19"],
        ),
        (
            "a move longer than its clip",
            swing_clip(),
            (8, 7, 6),
            &["the clip is 20 ticks long", "add up to 21"],
        ),
    ];
    let tree = swing_tree("cook_move_faults", &set_with(&swing_clip(), (8, 7, 5)));
    tree.try_cook(false).expect("the good one cooks");
    for (what, clip, phases, expected) in &rows {
        write_set(&tree, &set_with(clip, *phases));
        let error = tree.try_cook(false).expect_err(what);
        let said = says(&error);
        for part in ["move `swing`", "track `motion`"].iter().chain(*expected) {
            assert!(said.contains(part), "{what}: no {part:?} in:\n{said}");
        }
    }
    // A document whose clip is fine and whose other faults are its own says them all.
    let two = set_with(&swing_clip(), (8, 7, 4)).replace("buffer: 12", "buffer: 99");
    write_set(&tree, &two);
    let said = says(&tree.try_cook(false).unwrap_err());
    assert!(said.contains("add up to 19") && said.contains("99 ticks"), "{said}");
}

/// An FBX's numbers come through ufbx's C, so a table refuses them by name, rather than differ by
/// architecture later.
#[test]
fn an_fbx_is_refused_for_a_table() {
    let fbx_id = AssetId::from_bytes([0xb2; 16]);
    let tree = swing_tree("cook_move_fbx", &set_with(&swing_clip(), (8, 7, 5)));
    let fbx = tree.write("moves/animated.fbx", &fixture("animated_clips.fbx"));
    tree.sidecar(&fbx, fbx_id, ImportSettings::FbxClips(ClipSettings::default()));
    let report = tree.try_cook(false).unwrap();
    let name = report
        .assets
        .iter()
        .find(|asset| asset.source == fbx && asset.output.extension().unwrap() == CLIP_EXTENSION)
        .map(|asset| {
            let bytes = read_aligned(&asset.output);
            Clip::read(&bytes).unwrap().name.as_str().to_owned()
        })
        .expect("a clip");

    write_set(&tree, &set_with(&format!("(source: \"{fbx_id}\", name: \"{name}\")"), (8, 7, 5)));
    let said = says(&tree.try_cook(false).unwrap_err());
    assert!(said.contains("is an FBX") && said.contains(&fbx_id.to_string()), "{said}");

    // As the rig a clip plays on, the same: a key-pose document here, whose rest pose would be ufbx's.
    let on_fbx = AssetId::from_bytes([0xb3; 16]);
    let humanoid = HumanBone::REQUIRED.iter().map(|bone| (*bone, "Hips".to_owned())).collect();
    let settings = ClipSettings { humanoid, ..ClipSettings::default() };
    tree.sidecar(&fbx, fbx_id, ImportSettings::FbxClips(settings));
    let one_bone = "(type: \"sapphire::pipeline::KeyPoseClip\", version: 1, data: (
        ticks: 20, keys: [(tick: 0, pose: { Hips: (x: 10) })]))";
    let document = tree.write("moves/on_fbx.keypose.ron", one_bone.as_bytes());
    let settings = KeyPoseSettings { rig: Some(fbx_id), ..KeyPoseSettings::default() };
    tree.sidecar(&document, on_fbx, ImportSettings::KeyPose(settings));
    write_set(&tree, &set_with(&format!("(source: \"{on_fbx}\")"), (8, 7, 5)));
    let said = says(&tree.try_cook(false).unwrap_err());
    assert!(said.contains("is an FBX") && said.contains(&fbx_id.to_string()), "{said}");
}

/// A table's root stands for a hand-keyed one in the document, over the clip's bones.
#[test]
fn root_keys_lie_over_a_clip_through_a_cook() {
    let keyed = set_with(&swing_clip(), (8, 7, 5))
        .replace("motion: (clip:", "motion: (root: [(at: 20, offset: (2.0, 0.0, 0.0))], clip:");
    let tree = swing_tree("cook_move_root_keys", &keyed);
    let report = tree.try_cook(false).unwrap();
    let bytes = cooked_set(&report);
    let set = MoveSet::from_cooked(&bytes).unwrap();
    let (_, mv) = set.find(Name::new("swing")).unwrap();
    assert_eq!(mv.tables.root_samples().last(), Some(&RootSample { translation: [2.0, 0.0, 0.0], yaw: 0.0 }));
    let blade = mv.tables.path(Name::new("blade")).unwrap();
    assert_ne!(blade.samples()[0], blade.samples()[12], "and the blade still swings");
}
