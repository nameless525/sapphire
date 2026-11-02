//! The preview against the simulation and the pose, on the real character and the real moves, read from a
//! real cook. They need `cargo xtask content`, so they're `#[ignore]`d:
//! `cargo test -p sapphire_editor --test preview -- --ignored`.
//!
//! The same document through `sapphire_editor`'s run with no cook is `run`'s own unit tests, which run
//! everywhere on hand-keyed moves.

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::rig::HumanBone;
use sapphire_core::Name;
use sapphire_editor::app::figure_of;
use sapphire_editor::capture::{self, Textures};
use sapphire_editor::cooked::{Cook, sidecar_id};
use sapphire_editor::file::judge;
use sapphire_editor::run::{Run, Setup};
use sapphire_editor::{Editor, Options};
use sapphire_gameplay::{ActorSpawn, ActorTransform, SimConfig, Simulation, TickActions, volume_pose};
use sapphire_math::Vec3;
use sapphire_moves::{Follows, MoveSet, MoveSetDocument};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cook_dir() -> PathBuf {
    repo().join("target/content/desktop")
}

fn player_path() -> PathBuf {
    repo().join("content/combat/player/player.moves.ron")
}

fn warrior_path() -> PathBuf {
    repo().join("content/combat/enemy/warrior.moves.ron")
}

fn cook() -> Cook {
    Cook::open(&cook_dir()).expect("a cook: run `cargo xtask content` first")
}

fn editor_on(path: &Path) -> Editor {
    Editor::new(Options {
        file: Some(path.to_owned()),
        content: Some(cook_dir()),
        link: Some("127.0.0.1:9".parse().expect("an address")),
    })
}

/// Where a run's volume is, by a simulation with no controller, no dummy and nothing else in the world, so
/// its root is the table's: the numbers a volume's path gives at a tick, by another road.
fn lone_run(set: &MoveSet, name: &str) -> Vec<(u64, Vec<(Name, sapphire_physics::Pose)>, ActorTransform)> {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(set.clone());
    let actor = sim
        .spawn(ActorSpawn { position: Vec3::ZERO, yaw: 0.0, moves: Some(moves), controller: None })
        .expect("spawns");
    sim.step(&TickActions::NEUTRAL);
    sim.force_move(actor, Name::intern(name));
    let mut frames = Vec::new();
    loop {
        sim.step(&TickActions::NEUTRAL);
        let view = sim.actor(actor).expect("there");
        let Some(current) = view.moves.current else { break };
        let root = ActorTransform { position: view.position, yaw: view.yaw };
        let volumes =
            view.moves.volumes.iter().map(|volume| (volume.name, volume_pose(root, volume.end))).collect();
        frames.push((current.tick(), volumes, root));
    }
    frames
}

/// The brief's own test: at a chosen tick the preview shows what the simulation has, in pose and in volumes.
///
/// Each attack of the real `player.moves.ron`, from a cook, with the dummy out of reach so nothing stops
/// it: the frame's volume against a second simulation's at the same tick, and against the pose, where the
/// hand holding the blade is by `sapphire_anim`'s reading of the clip. The table comes from the clip by the
/// cook's scalars and the pose from it by `sapphire_anim`, so these two agreeing to a millimetre is what
/// says the character on screen holds the volume that hits.
#[test]
#[ignore = "needs `cargo xtask content`"]
fn a_preview_at_a_tick_matches_the_simulations_pose_and_volumes() {
    let cook = cook();
    let path = player_path();
    let text = fs::read_to_string(&path).expect("the document");
    let document = MoveSetDocument::parse(&text).expect("parses");
    let cooked = cook.move_set_of(&path).expect("reads").expect("the cook has the player's moves");
    let set = judge(&document, Some(&cooked)).set.expect("the document compiles over its cook");

    let mut editor = editor_on(&path);
    editor.set_dummy_distance(30.0);
    editor.set_warp_to_dummy(false);
    for name in ["light_1", "light_2", "light_3", "dash_attack", "skill"] {
        let mv = document.moves.iter().find(|mv| mv.name == name).expect("the move");
        assert!(editor.select_move(name));
        let lone = lone_run(&set, name);
        let hit = &mv.hitboxes[0];
        let clip = mv.motion.clip.as_ref().expect("a clip");
        let socket = clip.paths.iter().find(|path| path.name == hit.path).expect("its path");
        assert!(matches!(socket.follows, Follows::Bone(ref bone) if bone == "RightHand"));
        let mut compared = 0;
        for tick in hit.from..hit.to {
            editor.set_tick(tick);
            let frame = editor.frame().expect("a frame");
            assert_eq!(frame.tick, tick, "{name}");

            // The volume, against the other simulation's.
            let volume = frame.volumes.iter().find(|volume| volume.name == hit.name);
            let (_, lone_volumes, lone_root) = lone.iter().find(|(at, _, _)| *at == u64::from(tick)).expect("that tick");
            let lone_volume = lone_volumes.iter().find(|(name, _)| *name == Name::intern(&hit.name));
            if tick == 0 {
                continue;
            }
            let (volume, (_, lone_pose)) = (volume.expect("live"), lone_volume.expect("live"));
            let apart = volume.pose.position.distance(lone_pose.position);
            assert!(apart < 1e-3, "{name} tick {tick}: {apart} m from the lone run's");
            assert!(frame.root.position.distance(lone_root.position) < 1e-3, "{name} tick {tick}");

            // And against the pose: the hand by the clip, the socket on it.
            let figure = figure_of(&editor).expect("a figure");
            let casting = sapphire_editor::app::casting_of(&editor).expect("a casting");
            let hand = figure.joint_of_bone(HumanBone::RightHand).expect("a right hand");
            let model = figure.model_at(casting.1, tick);
            let socket_at = model[hand].transform_point3(Vec3::from_array(socket.offset));
            let in_world = frame.root.point(socket_at);
            let apart = in_world.distance(volume.pose.position);
            assert!(apart < 2e-3, "{name} tick {tick}: the pose has the blade {apart} m from the volume");
            compared += 1;
        }
        assert!(compared > 0, "{name}");
    }
}

/// The real moves against a dummy where a fight would have one: each attack lands, and scrubbing reads the
/// frame the hit landed on.
#[test]
#[ignore = "needs `cargo xtask content`"]
fn the_real_attacks_land_on_a_dummy_in_reach() {
    let cook = cook();
    let path = player_path();
    let document = MoveSetDocument::parse(&fs::read_to_string(&path).expect("the document")).expect("parses");
    let cooked = cook.move_set_of(&path).expect("reads").expect("the cook has the player's moves");
    let set = judge(&document, Some(&cooked)).set.expect("compiles");
    for name in ["light_1", "light_2", "light_3", "dash_attack", "skill"] {
        let run = Run::simulate(&set, name, Setup::default()).expect("plays");
        let hit = run.hits().next().unwrap_or_else(|| panic!("{name} lands at {} m", run.setup.dummy_distance));
        assert!(hit.on_dummy, "{name}");
        let index = run.frame_at_tick(hit.tick).expect("a frame");
        assert_eq!(run.frames[index].tick, hit.tick, "{name}");
    }
}

/// A sidecar's ID finds the cook's set, and the cook reads the figure's mesh by its skeleton.
#[test]
#[ignore = "needs `cargo xtask content`"]
fn the_cook_answers_for_the_real_documents() {
    let cook = cook();
    for path in [player_path(), warrior_path()] {
        assert!(sidecar_id(&path).is_some(), "{}", path.display());
        assert!(cook.move_set_of(&path).expect("reads").is_some(), "{}", path.display());
    }
    let editor = editor_on(&player_path());
    let figure = figure_of(&editor).expect("a figure");
    assert!(figure.triangles().len() > 1_000, "{} triangles", figure.triangles().len());
    let model = figure.model_at(None, 0);
    let skinned = figure.skinned(model);
    let top = skinned.iter().map(|point| point.y).fold(f32::MIN, f32::max);
    assert!((1.4..1.9).contains(&top), "the character stands {top} m tall");
}

/// What the real thing looks like, for an agent to read: the player's first cut as it lands, and the
/// warrior's swipe with its mesh at rest.
#[test]
#[ignore = "needs `cargo xtask content`"]
fn captures_of_the_real_moves() {
    let dir = std::env::var_os("SAPPHIRE_EDITOR_CAPTURES")
        .map_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")), PathBuf::from);
    fs::create_dir_all(&dir).expect("a folder for captures");
    for (path, name, tick, out) in [
        (player_path(), "light_1", 8, "player_light_1.png"),
        (player_path(), "skill", 30, "player_skill.png"),
        (warrior_path(), "swipe", 25, "warrior_swipe.png"),
    ] {
        let mut editor = editor_on(&path);
        assert!(editor.select_move(name));
        editor.set_tick(tick);
        let ctx = egui::Context::default();
        let mut textures = Textures::default();
        let (width, height) = (1400, 900);
        let mut last = None;
        for pass in 0..4 {
            let input = capture::raw_input(width, height, f64::from(pass) * 0.1);
            last = Some(capture::draw(&ctx, &mut textures, input, width, height, |ui| editor.ui(ui)));
        }
        last.expect("a capture").write_png(&dir.join(out)).expect("written");
    }
}
