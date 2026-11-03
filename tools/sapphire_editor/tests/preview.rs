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
use sapphire_editor::judge::judge;
use sapphire_editor::preview::{self, FigureCache, Overlays, PosedFigure, View};
use sapphire_editor::run::{Run, Setup};
use sapphire_editor::{Editor, Options};
use sapphire_gameplay::{ActorSpawn, ActorTransform, SimConfig, Simulation, TickActions, volume_pose};
use sapphire_math::Vec3;
use sapphire_moves::{Follows, MoveSet, MoveSetDocument};

fn moves_document(text: String) -> sapphire_editor::edits::MovesDocument {
    let registry = std::sync::Arc::new(sapphire_document::registry_of::<MoveSetDocument>());
    sapphire_document::Document::open(text, sapphire_document::kinds::moves(), registry)
}

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

/// A tick of [`lone_run`]: the tick, each live volume's pose and the root.
type LoneFrame = (u64, Vec<(Name, sapphire_physics::Pose)>, ActorTransform);

/// Where a run's volume is, by a simulation with no controller, no dummy and nothing else in the world, so
/// its root is the table's: the numbers a volume's path gives at a tick, by another road.
fn lone_run(set: &MoveSet, name: &str) -> Vec<LoneFrame> {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(set.clone());
    let actor = sim
        .spawn(ActorSpawn {
            position: Vec3::ZERO,
            yaw: 0.0,
            moves: Some(moves),
            controller: None,
            ..ActorSpawn::default()
        })
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
            let (_, lone_volumes, lone_root) =
                lone.iter().find(|(at, _, _)| *at == u64::from(tick)).expect("that tick");
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
        let hit =
            run.hits().next().unwrap_or_else(|| panic!("{name} lands at {} m", run.setup.dummy_distance));
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

#[test]
#[ignore = "needs `cargo xtask content`"]
fn fitting_the_selected_pose_keeps_the_entire_mesh_in_every_view_and_aspect() {
    for (path, name, tick) in [(player_path(), "light_1", 8), (warrior_path(), "swipe", 25)] {
        let mut editor = editor_on(&path);
        assert!(editor.select_move(name));
        editor.set_tick(tick);
        let figure = figure_of(&editor).expect("a figure");
        let (_, clip) = sapphire_editor::app::casting_of(&editor).expect("casting");
        let posed = PosedFigure { figure, clip, generation: 0 };
        let frame = editor.frame().expect("frame");
        let points = figure.skinned(figure.model_at(clip, frame.tick));
        assert!(!points.is_empty());
        for size in [egui::vec2(800.0, 400.0), egui::vec2(160.0, 500.0), egui::vec2(400.0, 100.0)] {
            let rect = egui::Rect::from_min_size(egui::pos2(20.0, 30.0), size);
            for mut view in [View::side(1.5), View::front(1.5), View::top(1.5)] {
                view.fit(rect, frame, Some(posed));
                for point in &points {
                    let projected = view.project(frame.root.point(*point), rect);
                    assert!(rect.shrink(1.0).contains(projected), "{name} {size:?} {view:?}: {projected:?}");
                }
            }
        }
    }
}

#[test]
#[ignore = "needs `cargo xtask content`"]
fn changing_figure_with_the_same_pose_key_matches_a_fresh_cache() {
    let player = editor_on(&player_path());
    let warrior = editor_on(&warrior_path());
    let frame = player.frame().expect("frame");
    let run = player.run().expect("run");
    let draw = |figure: &sapphire_editor::figure::Figure, cache: &mut FigureCache| {
        let ctx = egui::Context::default();
        capture::draw(&ctx, &mut Textures::default(), capture::raw_input(320, 300, 0.0), 320, 300, |ui| {
            preview::draw(
                ui.painter(),
                ui.max_rect(),
                &View::side(1.5),
                run,
                frame,
                Some(PosedFigure { figure, clip: None, generation: 0 }),
                cache,
                Overlays::default(),
            );
        })
    };
    let mut cache = FigureCache::default();
    let player_image = draw(figure_of(&player).expect("player figure"), &mut cache);
    let reused = draw(figure_of(&warrior).expect("warrior figure"), &mut cache);
    let fresh = draw(figure_of(&warrior).expect("warrior figure"), &mut FigureCache::default());
    assert_ne!(player_image, fresh, "the fixture must distinguish the two figures");
    assert_eq!(reused, fresh);
}

#[test]
#[ignore = "needs `cargo xtask content`"]
fn visiting_a_clip_does_not_invent_a_beside_document_fallback() {
    let cook = cook();
    let document =
        MoveSetDocument::parse(&fs::read_to_string(player_path()).expect("source")).expect("parse");
    let mut mv = document.moves[0].clone();
    let mut figures = sapphire_editor::figure::Figures::default();
    assert!(figures.cast(&mv, &cook).expect("cast clip").is_some());
    mv.motion.clip = None;
    assert_eq!(
        figures.cast(&mv, &cook).expect("cast clipless"),
        None,
        "no beside-document figure was loaded, regardless of clips visited"
    );
}

#[test]
#[ignore = "needs `cargo xtask content`"]
fn a_refused_clip_edit_keeps_authored_casting_and_simulation_in_one_successful_snapshot() {
    let mut editor = editor_on(&player_path());
    assert!(editor.select_move("light_1"));
    editor.set_tick(8);
    let casting = sapphire_editor::app::casting_of(&editor);
    let before = editor.run().expect("run").clone();
    let path = |suffix: &str| {
        format!("moves[\"light_1\"].motion.clip?.{suffix}")
            .parse::<sapphire_document::PropertyPath>()
            .expect("path")
    };
    editor
        .moves_document_mut()
        .expect("document")
        .together("refused clip edit", |document| {
            document.set_literal("source", &path("source"), "\"00000000-0000-4000-8000-000000000000\"")?;
            document.set_literal("rig", &path("rig"), "Some(\"00000000-0000-4000-8000-000000000001\")")?;
            sapphire_editor::edits::set_window(
                document,
                sapphire_editor::edits::EntryRef {
                    mv: 0,
                    track: sapphire_editor::edits::Track::Hitboxes,
                    index: 0,
                },
                12,
                11,
            )
        })
        .expect("parseable refused edit");
    editor.refresh();
    assert!(!editor.problems().is_empty());
    assert_eq!(sapphire_editor::app::casting_of(&editor), casting);
    assert_eq!(editor.run(), Some(&before));
    assert!(editor.select_move("skill"));
    let mut fresh = editor_on(&player_path());
    assert!(fresh.select_move("skill"));
    assert_eq!(sapphire_editor::app::casting_of(&editor), sapphire_editor::app::casting_of(&fresh));
    assert_eq!(editor.run(), fresh.run(), "other prior valid moves remain selectable");
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
            let output = ctx.run_ui(input, |ui| editor.ui(ui));
            if pass == 3 {
                last = Some(capture::rasterize(&ctx, &mut textures, output, width, height));
            } else {
                capture::settle(&ctx, &mut textures, output);
            }
        }
        last.expect("a capture").write_png(&dir.join(out)).expect("written");
    }
}

/// The real file, with a real cook: an edit made through the editor compiles over the running tables to
/// the same set a hand edit does, which is what `MoveSet`'s tables come from when it's pushed.
#[test]
#[ignore = "needs a cook"]
fn an_edit_of_the_players_moves_compiles_over_the_cook_as_a_hand_edit_does() {
    let source = fs::read_to_string(player_path()).expect("the file");
    let cook = cook();
    let running =
        cook.move_set_of(&player_path()).expect("the cook reads").expect("it holds the player's moves");

    let hand = source.replacen("to: 11,", "to: 13,", 1);
    assert_ne!(hand, source);
    let mut text = moves_document(source);
    let entry =
        sapphire_editor::edits::EntryRef { mv: 0, track: sapphire_editor::edits::Track::Hitboxes, index: 0 };
    text.together("an edit", |text| sapphire_editor::edits::set_window(text, entry, 7, 13)).expect("an edit");
    assert_eq!(text.text(), hand, "the text");

    let edited = judge(text.value().expect("it reads"), Some(&running));
    let by_hand = judge(&MoveSetDocument::parse(&hand).expect("parses"), Some(&running));
    assert!(edited.issues.is_empty(), "{:?}", edited.issues);
    let (edited, by_hand) = (edited.set.expect("compiles"), by_hand.set.expect("compiles"));
    assert_eq!(edited, by_hand);
    assert_eq!(*edited.to_cooked().expect("cooks"), *by_hand.to_cooked().expect("cooks"));
}

/// What an edit and a frame cost, on the real file. Run it optimized, as `cargo xtask editor` builds:
/// `cargo test -p sapphire_editor --profile tools --test preview -- --ignored --nocapture what_the_editor_costs`.
#[test]
#[ignore = "needs a cook, and prints costs"]
#[allow(clippy::print_stdout, reason = "the costs are for a person reading --nocapture")]
fn what_the_editor_costs() {
    /// The least of `rounds` runs, what's left once other work on the machine is taken out.
    fn least(rounds: u32, mut work: impl FnMut()) -> std::time::Duration {
        (0..rounds)
            .map(|_| {
                let started = sapphire_time::Stopwatch::start();
                work();
                started.elapsed()
            })
            .min()
            .expect("a round")
    }

    let source = fs::read_to_string(player_path()).expect("the file");
    let cook = cook();
    let set = cook.move_set_of(&player_path()).expect("the cook reads").expect("it holds the player's moves");
    println!("open the real file and cook: {:?}", least(5, || drop(editor_on(&player_path()))));
    let document = MoveSetDocument::parse(&source).expect("parses");
    println!("parse: {:?}", least(20, || drop(MoveSetDocument::parse(&source))));
    println!("compile over the cook: {:?}", least(20, || drop(judge(&document, Some(&set)))));
    for name in ["light_1", "skill"] {
        let simulate = || drop(Run::simulate(&set, name, Setup::default()).expect("runs"));
        println!("simulate {name}: {:?}", least(20, simulate));
    }

    let entry =
        sapphire_editor::edits::EntryRef { mv: 0, track: sapphire_editor::edits::Track::Hitboxes, index: 0 };
    let mut text = moves_document(source.clone());
    let mut to = 11;
    let edit = least(20, || {
        to = if to == 11 { 12 } else { 11 };
        text.together("an edit", |text| sapphire_editor::edits::set_window(text, entry, 7, to))
            .expect("an edit");
        let verdict = judge(text.value().expect("it reads"), Some(&set));
        let set = verdict.set.expect("compiles");
        drop(Run::simulate(&set, "light_1", Setup::default()).expect("runs"));
    });
    println!("one tick of a drag: splice, read again, compile and run: {edit:?}");

    let mut editor = editor_on(&player_path());
    editor.set_tick(8);
    let (width, height) = (1400, 900);
    let ctx = egui::Context::default();
    let mut passes = Vec::new();
    for pass in 0..200 {
        // A step of the playhead each pass, as playing does.
        editor.set_tick(pass % 34);
        let mut input = capture::raw_input(width, height, f64::from(pass) / 60.0);
        input.predicted_dt = 1.0 / 60.0;
        let started = sapphire_time::Stopwatch::start();
        let mut output = ctx.run_ui(input, |ui| editor.ui(ui));
        output.textures_delta.clear();
        let shapes = ctx.tessellate(output.shapes, output.pixels_per_point);
        let spent = started.elapsed();
        let triangles: usize = shapes
            .iter()
            .map(|shape| match &shape.primitive {
                egui::epaint::Primitive::Mesh(mesh) => mesh.indices.len() / 3,
                egui::epaint::Primitive::Callback(_) => 0,
            })
            .sum();
        passes.push((spent, triangles));
    }
    passes.sort_by_key(|(spent, _)| *spent);
    println!(
        "a frame with the figure: pass and tessellate {:?} median, {:?} at worst, {} triangles",
        passes[passes.len() / 2].0,
        passes[passes.len() - 1].0,
        passes[passes.len() / 2].1,
    );
}
