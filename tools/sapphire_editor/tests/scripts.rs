//! Command scripts, as `--run` reads them: every kittest in `ui.rs`, `panes.rs` and `docking.rs` has a twin here,
//! by the same name in the module of the same name, that says in commands what the kittest says with a pointer and
//! keys, and reaches the same document bytes. What isn't a command (a window's close, a file changed by someone
//! else, the clock) is said as the kittest says it. Then a session recorded with `--record` while kittest input
//! drives it, replayed by the binary's `--run` to the same bytes and the same frame, and the palette.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use egui::{Key, Modifiers, Pos2, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use sapphire_editor::capture;
use sapphire_editor::commands::{Command, SCRIPT_SUFFIX, Script};
use sapphire_editor::link::Pushed;
use sapphire_editor::screen::{Aspect, StandIn};
use sapphire_editor::shell::Pane;
use sapphire_editor::{Editor, Options};
use sapphire_moves::MoveSetDocument;
use sapphire_time::Stopwatch;

const PLAYER: &str = include_str!("../../../content/combat/player/player.moves.ron");
const PLAYER_META: &str = include_str!("../../../content/combat/player/player.moves.ron.meta");
const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");
const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
const CUE: &str = include_str!("../../../content/audio/hit_heavy.cue.ron");
const PROVING_GROUND: &str = include_str!("../../../content/zones/proving_ground/proving_ground.zone.ron");
const SQUARE_LOOK: &str = include_str!("../../../content/sequences/square_look.timeline.ron");
const CHARACTER_SCREEN: &str = include_str!("../../../content/ui/character.ui.ron");
const CHARACTER_SCREEN_META: &str = include_str!("../../../content/ui/character.ui.ron.meta");
const CHARACTER_MODEL: &str = include_str!("../../../content/ui/character.model.ron");
const HUD_MODEL: &str = include_str!("../../../content/ui/hud.model.ron");

/// The first move's first hit volume and first cue, as a script names them.
const HIT: &str = "(mv: 0, track: Hitboxes, index: 0)";
const FIRST_CUE: &str = "(mv: 0, track: Cues, index: 0)";
/// The cue the typing kittests type over, and what they type.
const CUE_NAME: &str = "\"swing_windup\"";
const TYPED_NAME: &str = "\"typed_name\"";

/// A directory of this test's own, gone when it ends.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("sapphire_editor_scripts_{name}_{}_{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().expect("a parent")).expect("its folder");
        fs::write(&path, text).expect("a test file");
        path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An editor as `--run` starts one on `file`: no cook, and a port nobody has.
fn editor(dir: &TestDir, file: Option<&Path>) -> Editor {
    Editor::new(Options {
        file: file.map(Path::to_owned),
        content: Some(dir.0.join("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
        pipeline: None,
        game: None,
    })
}

/// Runs `commands`, a version-1 script's list as a person writes one, from a script file in `dir`, as `--run`
/// does: its documents are named from the script's own folder.
#[track_caller]
fn run(editor: &mut Editor, dir: &TestDir, commands: &str) {
    let path = dir.0.join(format!("twin{SCRIPT_SUFFIX}"));
    fs::write(&path, format!("(version: 1, commands: [{commands}])")).expect("a script");
    let script = Script::read(&path).unwrap_or_else(|error| panic!("{error}"));
    if let Err(error) = editor.run_script(&script) {
        panic!("{error}");
    }
}

/// Runs one command that the editor must refuse, and says why it did.
#[track_caller]
fn refused(editor: &mut Editor, command: &str) -> String {
    let command = Command::parse(command).unwrap_or_else(|error| panic!("{error}"));
    editor.run(command).expect_err("refused")
}

fn text_of(editor: &Editor) -> String {
    editor.current().expect("a file").text().to_owned()
}

fn file(path: &Path) -> String {
    fs::read_to_string(path).expect("the file")
}

fn parse(text: &str) -> MoveSetDocument {
    MoveSetDocument::parse(text).expect("it parses")
}

/// `text` with the only `old` in `move_name`'s entry replaced, as someone in a text editor would.
fn hand_edit(text: &str, move_name: &str, old: &str, new: &str) -> String {
    let start = text.find(&format!("name: \"{move_name}\"")).expect("the move's there");
    let at = start + text[start..].find(old).expect("the text's there");
    format!("{}{new}{}", &text[..at], &text[at + old.len()..])
}

/// Whether two whole files are the same, saying where they first aren't rather than printing both.
#[track_caller]
fn assert_same(actual: &str, expected: &str, what: &str) {
    if actual == expected {
        return;
    }
    let at = actual.bytes().zip(expected.bytes()).take_while(|(a, b)| a == b).count();
    let around = |text: &'_ str| -> String {
        let bytes = text.as_bytes();
        String::from_utf8_lossy(&bytes[at.saturating_sub(70)..(at + 70).min(bytes.len())]).into_owned()
    };
    panic!("{what}: differs at byte {at}\nactual:   {:?}\nexpected: {:?}", around(actual), around(expected));
}

/// A drag of the first move's first hit volume to `from..to`, as the timeline says one in commands.
fn drag_window(from: u32, to: u32) -> String {
    format!(r#"Begin("drag a window"), SetWindow(entry: {HIT}, from: {from}, to: {to}), End,"#)
}

/// Two passes of the interface, as a window's frames would draw, with no window.
fn draw(editor: &mut Editor) {
    let ctx = egui::Context::default();
    for pass in 0..2 {
        let mut output = ctx.run_ui(capture::raw_input(1400, 900, f64::from(pass) * 0.1), |ui| editor.ui(ui));
        output.textures_delta.clear();
    }
}

/// Polls the editor, a second later each time, until `done`.
fn poll_until(editor: &mut Editor, what: &str, done: impl Fn(&Editor) -> bool) {
    let started = Stopwatch::start();
    let mut now = 100.0;
    while !done(editor) {
        assert!(started.elapsed() < Duration::from_secs(30), "never saw {what}");
        now += 10.0;
        editor.poll(now);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The name of every kittest in `source`.
fn kittests(source: &str) -> Vec<&str> {
    source.split("#[test]\nfn ").skip(1).map(|rest| rest.split('(').next().expect("a name")).collect()
}

/// Every kittest has its twin, by its name, in the module of its file's name.
#[test]
fn every_kittest_has_a_command_script_twin() {
    let here = include_str!("scripts.rs");
    for (module, source) in [
        ("ui", include_str!("ui.rs")),
        ("panes", include_str!("panes.rs")),
        ("docking", include_str!("docking.rs")),
    ] {
        let start = here.find(&format!("\nmod {module} {{")).unwrap_or_else(|| panic!("mod {module}"));
        let body = &here[start..];
        let body = &body[..body[1..].find("\nmod ").map_or(body.len(), |end| end + 1)];
        let names = kittests(source);
        assert!(names.len() >= 12, "{module}: {names:?}");
        for name in names {
            assert!(body.contains(&format!("fn {name}()")), "{module}::{name} has no twin");
        }
    }
}

mod ui {
    use super::*;

    #[test]
    fn public_playback_calls_preserve_phase_across_rate_changes_without_ui() {
        let dir = TestDir::new("playback_clock");
        let path = dir.write("warrior.moves.ron", WARRIOR);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, "DummyDistance(30.0), Tick(3), Speed(0.25), Play");
        assert!(editor.is_playing());
        run(&mut editor, &dir, "Speed(1.0), Stop");
        assert!(!editor.is_playing());
        assert_eq!(editor.tick(), 3);
        assert_same(&text_of(&editor), WARRIOR, "playing edits nothing");
    }

    #[test]
    fn a_capture_of_the_timeline_reads() {
        let dir = TestDir::new("capture");
        let path = dir.write("warrior.moves.ron", WARRIOR);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"SelectMove("swipe"), Tick(20)"#);
        sapphire_editor::cli::write_frame(&mut editor, 1400, 900, &dir.0.join("timeline.png"))
            .expect("drawn");
        let swipe = parse(WARRIOR).moves.iter().position(|mv| mv.name == "swipe").expect("there");
        run(
            &mut editor,
            &dir,
            &format!("SetWindow(entry: (mv: {swipe}, track: Hitboxes, index: 0), from: 5, to: 200)"),
        );
        assert!(!editor.problems().is_empty());

        // The kittest's edit, by the call it makes.
        let mut by_call = self::editor(&dir, Some(&path));
        let entry = sapphire_editor::edits::EntryRef {
            mv: swipe,
            track: sapphire_editor::edits::Track::Hitboxes,
            index: 0,
        };
        let edit = by_call
            .moves_document_mut()
            .expect("a file")
            .together("too far", |text| sapphire_editor::edits::set_window(text, entry, 5, 200));
        edit.expect("an edit");
        assert_same(&text_of(&editor), &text_of(&by_call), "the text");
    }

    /// `--run` with `--capture` draws what `--capture` alone draws, for the same move and tick, to the byte.
    #[test]
    fn the_binary_captures_a_frame_with_no_window() {
        let dir = TestDir::new("binary");
        let path = dir.write("warrior.moves.ron", WARRIOR);
        let editor = |args: &[&str]| {
            Process::new(env!("CARGO_BIN_EXE_sapphire_editor"))
                .arg(&path)
                .args(args)
                .args(["--size", "800x500", "--link", "127.0.0.1:9", "--content"])
                .arg(dir.0.join("no cook here"))
                .current_dir(&dir.0)
                .output()
                .expect("it runs")
        };
        dir.write(
            &format!("swipe{SCRIPT_SUFFIX}"),
            r#"(version: 1, commands: [SelectMove("swipe"), Tick(12)])"#,
        );
        let output = editor(&["--run", &format!("swipe{SCRIPT_SUFFIX}"), "--capture", "ran.png"]);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let output = editor(&["--capture", "captured.png", "--move", "swipe", "--tick", "12"]);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let ran = fs::read(dir.0.join("ran.png")).expect("a PNG");
        assert!(ran.len() > 1000);
        assert!(ran == fs::read(dir.0.join("captured.png")).expect("a PNG"), "the same frame, to the byte");

        dir.write(
            &format!("none{SCRIPT_SUFFIX}"),
            r#"(version: 1, commands: [Tick(1), SelectMove("no such move")])"#,
        );
        let output = editor(&["--run", &format!("none{SCRIPT_SUFFIX}")]);
        assert!(!output.status.success());
        let said = String::from_utf8_lossy(&output.stderr);
        assert!(said.contains("command 2 of the script") && said.contains("no such move"), "{said}");
        assert_same(&file(&path), WARRIOR, "the file");
    }

    #[test]
    fn the_ui_opens_and_names_its_tracks() {
        let dir = TestDir::new("names");
        let path = dir.write("player.moves.ron", PLAYER);
        dir.write("player.moves.ron.meta", PLAYER_META);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"SelectMove("light_1"), AllTracks(true)"#);
        assert_eq!(editor.selected_move_name(), Some("light_1"));
        assert!(editor.current().is_some_and(|file| !file.is_dirty()));
        assert!(editor.frame().is_none());
        assert_same(&text_of(&editor), PLAYER, "the text");
    }

    #[test]
    fn a_drag_of_a_windows_end_writes_what_a_hand_edit_would() {
        let dir = TestDir::new("drag_end");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            &format!(
                r#"Begin("drag"), SetWindow(entry: {HIT}, from: 7, to: 12), Tick(11), SetWindow(entry: {HIT}, from: 7, to: 13), Tick(12), End"#
            ),
        );
        assert_same(&text_of(&editor), &hand_edit(PLAYER, "light_1", "to: 11,", "to: 13,"), "the text");
        assert_eq!(editor.tick(), 12);
        assert_same(&file(&path), PLAYER, "nothing's saved until it's asked for");
    }

    #[test]
    fn a_drag_on_the_warrior_compiles_to_what_a_hand_edit_compiles_to() {
        let dir = TestDir::new("drag_warrior");
        let path = dir.write("warrior.moves.ron", WARRIOR);
        let mut editor = editor(&dir, Some(&path));
        let document = parse(WARRIOR);
        let swipe = document.moves.iter().position(|mv| mv.name == "swipe").expect("there");
        let window = &document.moves[swipe].hitboxes[0];
        run(
            &mut editor,
            &dir,
            &format!(
                r#"SelectMove("swipe"), Begin("drag"), SetWindow(entry: (mv: {swipe}, track: Hitboxes, index: 0), from: {}, to: {}), End"#,
                window.from - 1,
                window.to
            ),
        );
        let hand = hand_edit(
            WARRIOR,
            "swipe",
            &format!("from: {},", window.from),
            &format!("from: {},", window.from - 1),
        );
        assert_same(&text_of(&editor), &hand, "the text");
    }

    #[test]
    fn a_windows_body_a_cue_and_a_phase_edge_drag_in_ticks() {
        let dir = TestDir::new("drag_more");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            &format!(
                r#"{} Begin("drag"), SetInstant(entry: (mv: 0, track: Cues, index: 1), at: 10), End,
                Begin("drag"), SetPhases(mv: 0, startup: 8, active: 3, recovery: 23), End"#,
                drag_window(8, 12)
            ),
        );
        let body = hand_edit(
            PLAYER,
            "light_1",
            "from: 7,\n                        to: 11,",
            "from: 8,\n                        to: 12,",
        );
        let cue = hand_edit(&body, "light_1", "(name: \"swing\", at: 7)", "(name: \"swing\", at: 10)");
        let edge = hand_edit(
            &cue,
            "light_1",
            "startup: 7,\n                active: 4,",
            "startup: 8,\n                active: 3,",
        );
        assert_same(&text_of(&editor), &edge, "the phase edge");
        run(&mut editor, &dir, "Undo, Undo, Undo");
        assert_same(&text_of(&editor), PLAYER, "the text");
        assert!(!editor.current().expect("a file").is_dirty());
    }

    #[test]
    fn a_refusal_shows_its_move_and_track_and_keeps_the_state() {
        let dir = TestDir::new("refusal");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &drag_window(7, 37));
        assert_same(
            &text_of(&editor),
            &hand_edit(PLAYER, "light_1", "to: 11,", "to: 37,"),
            "the edit stands",
        );
        let problems = editor.problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].in_move.as_deref(), Some("light_1"));
        assert_eq!(problems[0].track.as_deref(), Some("hitboxes"));
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), PLAYER, "the text");
        assert!(editor.problems().is_empty());
    }

    #[test]
    fn undo_and_redo_keys_walk_one_gesture_at_a_time() {
        let dir = TestDir::new("undo");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("{} {}", drag_window(7, 14), drag_window(9, 14)));
        let once = hand_edit(PLAYER, "light_1", "to: 11,", "to: 14,");
        let twice = hand_edit(
            &once,
            "light_1",
            "from: 7,\n                        to: 14,",
            "from: 9,\n                        to: 14,",
        );
        assert_same(&text_of(&editor), &twice, "the text");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), &once, "one undo, the second drag");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), PLAYER, "two, the first");
        assert!(!editor.current().expect("a file").is_dirty());
        run(&mut editor, &dir, "Redo");
        assert_same(&text_of(&editor), &once, "redo");
    }

    #[test]
    fn a_drag_held_through_an_undo_goes_on_as_one_step() {
        let dir = TestDir::new("undo_mid_drag");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            &format!(
                r#"Begin("drag"), SetWindow(entry: {HIT}, from: 7, to: 12), SetWindow(entry: {HIT}, from: 7, to: 13), Undo,
                Begin("drag"), SetWindow(entry: {HIT}, from: 7, to: 14), SetWindow(entry: {HIT}, from: 7, to: 16), End"#
            ),
        );
        assert_same(&text_of(&editor), &hand_edit(PLAYER, "light_1", "to: 11,", "to: 16,"), "the drag's end");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), PLAYER, "one undo for the rest of the drag");
    }

    #[test]
    fn a_drag_that_loses_its_button_ends() {
        let dir = TestDir::new("lost_button");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!(r#"Begin("drag"), SetWindow(entry: {HIT}, from: 7, to: 12)"#));
        assert!(editor.current().expect("a file").in_gesture());
        run(&mut editor, &dir, "End");
        assert!(!editor.current().expect("a file").in_gesture());
        assert_ne!(text_of(&editor), PLAYER);
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), PLAYER, "the text");
    }

    #[test]
    fn the_ruler_and_the_keys_scrub_tick_by_tick() {
        let dir = TestDir::new("scrub");
        let path = dir.write("warrior.moves.ron", WARRIOR);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"SelectMove("swipe")"#);
        let ticks = parse(WARRIOR).moves.iter().find(|mv| mv.name == "swipe").expect("there").ticks();
        for tick in [1, 13, 20, ticks - 1] {
            run(&mut editor, &dir, &format!("Tick({tick})"));
            assert_eq!(editor.tick(), tick);
            assert_eq!(editor.frame().expect("a frame").tick, tick, "the simulation's frame");
        }
        run(&mut editor, &dir, "Tick(0), Tick(1), Tick(2)");
        assert_eq!(editor.tick(), 2);
        run(&mut editor, &dir, "Tick(1)");
        assert_eq!(editor.tick(), 1);
        run(&mut editor, &dir, &format!("Tick({})", u32::MAX));
        assert_eq!(editor.tick(), ticks - 1);
        assert_same(&text_of(&editor), WARRIOR, "scrubbing edits nothing");
    }

    #[test]
    fn brackets_set_a_windows_ends_at_the_playhead() {
        let dir = TestDir::new("brackets");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("Select(Some({HIT})), Tick(5), StartAtPlayhead"));
        let moved = hand_edit(
            PLAYER,
            "light_1",
            "from: 7,\n                        to: 11,",
            "from: 5,\n                        to: 11,",
        );
        assert_same(&text_of(&editor), &moved, "the start, at the playhead");
        run(&mut editor, &dir, "Tick(8), EndAtPlayhead");
        assert_same(
            &text_of(&editor),
            &hand_edit(&moved, "light_1", "to: 11,", "to: 9,"),
            "the end, just past it",
        );
    }

    #[test]
    fn an_entry_added_and_deleted_leaves_the_file_as_it_was() {
        let dir = TestDir::new("add");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, "Tick(20), Add(Cues)");
        let after = parse(&text_of(&editor));
        assert_eq!(after.moves[0].cues.len(), parse(PLAYER).moves[0].cues.len() + 1);
        assert!(after.moves[0].cues.iter().any(|cue| cue.at == 20), "at the playhead");
        assert!(editor.selected_entry().is_some(), "and ready to name");
        run(&mut editor, &dir, "Delete");
        assert_same(&text_of(&editor), PLAYER, "the text");
    }

    #[test]
    fn play_from_frame_n_plays_and_returns_to_n() {
        let dir = TestDir::new("play");
        let path = dir.write("warrior.moves.ron", WARRIOR);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"SelectMove("swipe"), Tick(10), Play"#);
        assert!(editor.is_playing());
        run(&mut editor, &dir, "Stop, Speed(0.25), Play, Stop");
        assert!(!editor.is_playing());
        assert!(editor.tick() >= 10);
        assert_same(&text_of(&editor), WARRIOR, "playing edits nothing");
    }

    #[test]
    fn closing_with_unsaved_edits_asks_first() {
        let dir = TestDir::new("quit");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, "Quit");
        assert!(editor.may_quit(), "nothing to lose");

        let dir = TestDir::new("quit_dirty");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = self::editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("{} Quit", drag_window(7, 12)));
        assert!(!editor.may_quit());
        run(&mut editor, &dir, "CancelQuit");
        assert!(!editor.may_quit());
        run(&mut editor, &dir, "Quit, SaveAndQuit");
        assert!(editor.may_quit());
        assert_same(&file(&path), &text_of(&editor), "saved");
        assert_same(&file(&path), &hand_edit(PLAYER, "light_1", "to: 11,", "to: 12,"), "the drag");
    }

    #[test]
    fn opening_another_document_keeps_the_edits() {
        let dir = TestDir::new("open_dirty");
        dir.write("Cargo.toml", "");
        let path = dir.write("content/player.moves.ron", PLAYER);
        dir.write("content/warrior.moves.ron", WARRIOR);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!(r#"{} Open("content/warrior.moves.ron")"#, drag_window(7, 12)));
        assert_eq!(editor.current().expect("a file").file_name(), "warrior.moves.ron");
        assert_eq!(editor.selected_move_name(), Some("swipe"));
        assert_same(&file(&path), PLAYER, "nothing's written");
        assert!(editor.is_dirty(), "the other's edits count");
        run(&mut editor, &dir, r#"Open("content/player.moves.ron")"#);
        assert_same(&text_of(&editor), &hand_edit(PLAYER, "light_1", "to: 11,", "to: 12,"), "kept");
        assert_eq!(editor.selected_move_name(), Some("light_1"));
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), PLAYER, "and its history");
        assert!(!editor.is_dirty());
    }

    #[test]
    fn a_first_cook_reaches_an_editor_already_open() {
        let dir = TestDir::new("late_cook");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"SelectMove("light_1")"#);
        assert!(!editor.has_cook());
        let cook = dir.0.join("no cook here");
        fs::create_dir_all(&cook).expect("the cook's folder");
        let pack = sapphire_assets::pack::write_pack(Vec::new()).expect("an empty pack");
        fs::write(cook.join("shared.spack"), &pack).expect("a pack");
        poll_until(&mut editor, "the cook", Editor::has_cook);
        assert_same(&text_of(&editor), SLICE, "a cook edits nothing");
    }

    /// What a script leaves on a move of any length draws, and costs what it shows.
    fn draws(text: &str) {
        let dir = TestDir::new("huge");
        let path = dir.write("slice.moves.ron", text);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!(r#"SelectMove("light_1"), Tick({})"#, u32::MAX));
        draw(&mut editor);
        assert_same(&text_of(&editor), text, "drawing edits nothing");
    }

    #[test]
    fn a_move_longer_than_any_can_be_draws_only_what_shows() {
        for recovery in ["400000", "4000000000"] {
            draws(&SLICE.replacen("recovery: 18", &format!("recovery: {recovery}"), 1));
        }
    }

    #[test]
    fn the_largest_numbers_a_document_can_say_draw() {
        for (from, to) in [("startup: 5", "startup: 4294967295"), ("to: 8", "to: 4294967295")] {
            draws(&SLICE.replacen(from, to, 1));
        }
    }

    #[test]
    fn with_no_file_named_the_list_offers_the_repositorys_documents() {
        let dir = TestDir::new("no_file");
        let warrior =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat/enemy/warrior.moves.ron");
        let open = format!("Open({:?})", warrior.display().to_string());
        let mut editor = editor(&dir, None);
        run(&mut editor, &dir, &open);
        assert_eq!(editor.current().expect("a file").file_name(), "warrior.moves.ron");

        let mut editor = self::editor(&dir, Some(&dir.0.join("no such.moves.ron")));
        assert!(editor.notice().is_some());
        run(&mut editor, &dir, &open);
        assert_eq!(editor.current().expect("a file").file_name(), "warrior.moves.ron");
        assert_same(&text_of(&editor), WARRIOR, "opening edits nothing");
    }

    /// With no device the push fails; the save is the same.
    #[test]
    fn save_and_push_shows_what_the_device_says() {
        let dir = TestDir::new("push");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("{} SaveAndPush", drag_window(5, 9)));
        let first = hand_edit(SLICE, "light_1", "to: 8,", "to: 9,");
        assert_same(&file(&path), &first, "saved");
        run(&mut editor, &dir, &drag_window(5, 10));
        assert_same(&text_of(&editor), &hand_edit(SLICE, "light_1", "to: 8,", "to: 10,"), "the edit");
        run(&mut editor, &dir, "SaveAndPush, Undo");
        assert_same(&text_of(&editor), &first, "the text");
    }

    #[test]
    fn a_push_with_no_device_says_so() {
        let dir = TestDir::new("no_device");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("{} SaveAndPush", drag_window(7, 12)));
        assert_same(&file(&path), &text_of(&editor), "the file");
        assert!(!editor.current().expect("a file").is_dirty());
        poll_until(&mut editor, "the failure", |editor| {
            matches!(editor.link().last_push(), Some(Pushed::Failed(_)))
        });
    }

    fn typed() -> String {
        SLICE.replacen(CUE_NAME, TYPED_NAME, 1)
    }

    /// A name typed over the slice's first cue and let go, as a script says it.
    fn type_name(name: &str) -> String {
        format!(r#"Select(Some({FIRST_CUE})), SetValue(entry: {FIRST_CUE}, path: "name", value: {name:?}),"#)
    }

    #[test]
    fn escape_takes_back_what_is_being_typed() {
        let dir = TestDir::new("typed_escape");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("Select(Some({FIRST_CUE}))"));
        let why =
            refused(&mut editor, &format!(r#"SetValue(entry: {FIRST_CUE}, path: "name", value: "\"half")"#));
        assert!(!why.is_empty());
        assert_same(&text_of(&editor), SLICE, "the text");
        assert!(!editor.is_dirty());
        assert_eq!(editor.current().expect("a file").undo_label(), None);
        run(&mut editor, &dir, &type_name(TYPED_NAME));
        assert_same(&text_of(&editor), &typed(), "the text");
    }

    fn type_damage(digits: &[&str]) -> String {
        let mut script = format!(r#"Select(Some({HIT})), Begin("edit attack.damage"),"#);
        for value in digits {
            script += &format!(r#" SetValue(entry: {HIT}, path: "attack.damage", value: "{value}"),"#);
        }
        script
    }

    #[test]
    fn a_number_typed_in_the_inspector_is_one_step_of_undo() {
        let dir = TestDir::new("typed_number");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        let (twelve, seventy) =
            (SLICE.replacen("damage: 6,", "damage: 12,", 1), SLICE.replacen("damage: 6,", "damage: 70,", 1));
        run(&mut editor, &dir, &type_damage(&["1", "12"]));
        assert_same(&text_of(&editor), &twelve, "it lands as it's typed");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), SLICE, "one undo takes back all of it");
        assert!(!editor.is_dirty());
        assert_eq!(editor.current().expect("a file").undo_label(), None);

        run(
            &mut editor,
            &dir,
            &format!("{} End, {} End", type_damage(&["1", "12"]), type_damage(&["7", "70"])),
        );
        assert_same(&text_of(&editor), &seventy, "the second session");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), &twelve, "one undo, the second session");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), SLICE, "two, the first");

        run(&mut editor, &dir, &format!("{} End, {}", type_damage(&["1", "12"]), drag_window(5, 10)));
        assert_same(&text_of(&editor), &hand_edit(&twelve, "light_1", "to: 8,", "to: 10,"), "both landed");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), &twelve, "one undo, the drag");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), SLICE, "two, the number");
    }

    #[test]
    fn the_undo_button_takes_back_a_number_being_typed() {
        let dir = TestDir::new("typed_number_undo_button");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("{} Undo", type_damage(&["1", "12"])));
        assert_same(&text_of(&editor), SLICE, "one click");
    }

    #[test]
    fn escape_takes_back_a_number_being_typed() {
        let dir = TestDir::new("typed_number_escape");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("{} Cancel", type_damage(&["1", "12"])));
        assert_same(&text_of(&editor), SLICE, "the text");
        assert!(!editor.is_dirty());
        let file = editor.current().expect("a file");
        assert_eq!((file.undo_label(), file.redo_label()), (None, None), "and no step, either way");
    }

    #[test]
    fn saving_by_key_takes_what_is_being_typed() {
        let dir = TestDir::new("typed_key");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("{} SaveAndPush", type_name(TYPED_NAME)));
        assert_same(&file(&path), &typed(), "the file");
        assert_same(&text_of(&editor), &typed(), "the text");
        assert!(!editor.is_dirty());
    }

    #[test]
    fn saving_by_button_takes_what_is_being_typed() {
        let dir = TestDir::new("typed_button");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("{} SaveAndPush", type_name(TYPED_NAME)));
        assert_same(&file(&path), &typed(), "the file");
        assert!(!editor.is_dirty());
    }

    #[test]
    fn closing_while_typing_asks_first() {
        let dir = TestDir::new("typed_quit");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("{} Quit", type_name(TYPED_NAME)));
        assert!(!editor.may_quit(), "it would lose what's on the screen");
        run(&mut editor, &dir, "CancelQuit");
        assert!(!editor.may_quit());
        assert_same(&file(&path), SLICE, "nothing's saved by asking");
        run(&mut editor, &dir, "Quit, SaveAndQuit");
        assert!(editor.may_quit());
        assert_same(&file(&path), &typed(), "saved");

        let dir = TestDir::new("typed_quit_unsaved");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = self::editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("{} Quit, QuitWithoutSaving", type_name(TYPED_NAME)));
        assert!(editor.may_quit());
        assert_same(&file(&path), SLICE, "not saved");
    }

    /// A window's close isn't a command: it comes from the window, as the kittest says it.
    #[test]
    fn closing_a_window_that_cant_draw_saves_instead_of_asking() {
        let dir = TestDir::new("unseen_edits");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &type_name(TYPED_NAME));
        editor.request_quit_unseen();
        assert!(editor.may_quit());
        assert_same(&file(&path), &typed(), "saved");

        let dir = TestDir::new("unseen_refused");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = self::editor(&dir, Some(&path));
        run(&mut editor, &dir, &type_name(TYPED_NAME));
        let elsewhere = SLICE.replacen("startup: 5", "startup: 6", 1);
        fs::write(&path, &elsewhere).expect("someone else's save");
        editor.request_quit_unseen();
        assert!(!editor.may_quit(), "it would lose someone's save, or these edits");
        draw(&mut editor);
        editor.request_quit_unseen();
        assert!(editor.may_quit(), "the second close is an answer");
        assert_same(&file(&path), &elsewhere, "the file");
    }

    #[test]
    fn one_close_that_sdl_says_twice_isnt_a_second_close() {
        let dir = TestDir::new("unseen_twice");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &type_name(TYPED_NAME));
        let elsewhere = SLICE.replacen("startup: 5", "startup: 6", 1);
        fs::write(&path, &elsewhere).expect("someone else's save");
        editor.request_quit_unseen();
        editor.request_quit_unseen();
        assert!(!editor.may_quit(), "one click lost the edits");
        assert_same(&text_of(&editor), &typed(), "the edits");
        draw(&mut editor);
        editor.request_quit_unseen();
        editor.request_quit_unseen();
        assert!(editor.may_quit(), "the next click is the second close");
        assert_same(&file(&path), &elsewhere, "the file");
    }

    #[test]
    fn opening_another_document_takes_what_is_being_typed() {
        let dir = TestDir::new("typed_open");
        dir.write("Cargo.toml", "");
        let path = dir.write("content/slice.moves.ron", SLICE);
        dir.write("content/warrior.moves.ron", WARRIOR);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!(r#"{} Open("content/warrior.moves.ron")"#, type_name(TYPED_NAME)));
        assert_eq!(editor.current().expect("a file").file_name(), "warrior.moves.ron");
        assert_same(&file(&path), SLICE, "nothing's written");
        assert!(editor.is_dirty());
        run(&mut editor, &dir, r#"Open("content/slice.moves.ron")"#);
        assert_same(&text_of(&editor), &typed(), "it went in");
        assert_eq!(editor.current().expect("a file").undo_label(), Some("edit name"));
    }

    /// Text the document can't read is refused where a script says it: nothing goes in, so nothing is held.
    #[test]
    fn text_the_document_cant_read_stays_where_it_was_typed() {
        let dir = TestDir::new("typed_refused");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("Select(Some({FIRST_CUE}))"));
        refused(&mut editor, &format!(r#"SetValue(entry: {FIRST_CUE}, path: "name", value: "\"half")"#));
        assert_same(&text_of(&editor), SLICE, "nothing went in");
        run(&mut editor, &dir, &format!("{} Undo", type_name(TYPED_NAME)));
        assert_same(&text_of(&editor), SLICE, "one step back");
        run(&mut editor, &dir, "DropTyped");
        assert!(!editor.is_dirty());
        assert_same(&file(&path), SLICE, "nothing was saved");
    }

    #[test]
    fn a_fields_typing_is_one_step_of_undo() {
        let dir = TestDir::new("typed_undo");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &type_name(TYPED_NAME));
        assert_same(&text_of(&editor), &typed(), "taken when the field lets go");
        assert_eq!(editor.current().expect("a file").undo_label(), Some("edit name"));
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), SLICE, "one step");
        assert!(!editor.is_dirty());
        run(&mut editor, &dir, "Redo");
        assert_same(&text_of(&editor), &typed(), "and redo brings it back");
        run(&mut editor, &dir, &format!("Undo, {} Undo", type_name(TYPED_NAME)));
        assert_same(&text_of(&editor), SLICE, "the text");
        run(&mut editor, &dir, "Redo");
        assert_same(&text_of(&editor), &typed(), "and redo brings that back too");
    }

    #[test]
    fn a_change_on_disk_doesnt_land_under_what_is_being_typed() {
        let dir = TestDir::new("typed_disk");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &type_name(TYPED_NAME));
        let elsewhere = SLICE.replacen("startup: 5", "startup: 6", 1);
        fs::write(&path, &elsewhere).expect("someone else's save");
        editor.poll(100.0);
        assert_same(&text_of(&editor), &typed(), "it isn't taken over what's on the screen");
        refused(&mut editor, "SaveAndPush");
        assert_same(&file(&path), &elsewhere, "nothing's lost");
        assert_same(&text_of(&editor), &typed(), "and neither is what was typed");
        assert!(editor.current().expect("a file").conflict().is_some());
    }

    #[test]
    fn a_file_that_stopped_being_text_needs_saying_so_before_a_save() {
        let not_text = [0xff, 0xfe, 0x42];
        let dir = TestDir::new("not_text");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &drag_window(5, 9));
        let edited = text_of(&editor);
        fs::write(&path, not_text).expect("someone else's save");
        refused(&mut editor, "SaveAndPush");
        assert_eq!(fs::read(&path).expect("the file"), not_text);
        refused(&mut editor, "TakeFile");
        run(&mut editor, &dir, "KeepMine");
        assert_eq!(fs::read(&path).expect("the file"), not_text, "not until the next save");
        run(&mut editor, &dir, "SaveAndPush");
        assert_same(&file(&path), &edited, "saved");
    }

    #[test]
    fn what_is_typed_lands_on_its_own_entry_when_its_row_goes() {
        let dir = TestDir::new("typed_row");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!(r#"{} SelectMove("light_2")"#, type_name(TYPED_NAME)));
        assert_eq!(editor.selected_move_name(), Some("light_2"));
        assert_same(&text_of(&editor), &typed(), "the first move's cue");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), SLICE, "one step");

        run(&mut editor, &dir, &format!(r#"SelectMove("light_1"), {} Delete"#, type_name(TYPED_NAME)));
        let after = parse(&text_of(&editor));
        assert_eq!(after.moves[0].cues.len(), 1);
        assert_eq!(after.moves[0].cues[0].name, "swing", "the entry that was there, not the next one");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), &typed(), "the delete");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), SLICE, "and the name");

        run(&mut editor, &dir, &format!("{} {}", type_name(TYPED_NAME), drag_window(5, 9)));
        assert_same(&text_of(&editor), &hand_edit(&typed(), "light_1", "to: 8,", "to: 9,"), "both");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), &typed(), "the drag alone");
    }

    /// The slice's first cue moved to `at`, as the timeline says it, and saved and pushed.
    fn move_the_first_cue_and_save(at: u32) -> String {
        format!("SetInstant(entry: {FIRST_CUE}, at: {at}), Tick({at}), SaveAndPush,")
    }

    fn cue_at(at: u32) -> String {
        SLICE.replacen("(name: \"swing_windup\", at: 0)", &format!("(name: \"swing_windup\", at: {at})"), 1)
    }

    /// The device kittests' twins have no device: what they prove is what reaches the file.
    #[test]
    fn a_late_answer_doesnt_speak_for_newer_text() {
        let dir = TestDir::new("late_answer");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            &format!("{} {}", move_the_first_cue_and_save(1), move_the_first_cue_and_save(2)),
        );
        assert_same(&file(&path), &cue_at(2), "the file");
        assert!(!editor.problems().iter().any(|problem| problem.from_device));
    }

    #[test]
    fn saves_queued_are_all_waited_for() {
        let dir = TestDir::new("queued");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        let saves: String = (1..=3).map(move_the_first_cue_and_save).collect();
        run(&mut editor, &dir, &format!("{saves} SaveAndPush, SaveAndPush"));
        assert_same(&file(&path), &cue_at(3), "the file");
    }

    #[test]
    fn another_document_starts_with_nothing_said_of_it() {
        let dir = TestDir::new("switching");
        let path = dir.write("slice.moves.ron", SLICE);
        dir.write("warrior.moves.ron", WARRIOR);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            &format!(
                r#"{} Open("warrior.moves.ron"), Open("slice.moves.ron"), {} Open("warrior.moves.ron")"#,
                move_the_first_cue_and_save(1),
                move_the_first_cue_and_save(2)
            ),
        );
        assert_eq!(editor.link().last_push(), None);
        assert!(!editor.problems().iter().any(|problem| problem.from_device));
        assert_same(&file(&path), &cue_at(2), "the file");
        assert_same(&text_of(&editor), WARRIOR, "the other");
    }

    #[test]
    fn another_device_starts_with_nothing_said_of_it() {
        let dir = TestDir::new("targets");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            &format!(r#"Link("127.0.0.1:10"), {} Link("127.0.0.1:9")"#, move_the_first_cue_and_save(1)),
        );
        assert_eq!(editor.link().last_push(), None);
        assert!(editor.link().attempts().is_empty());
        assert_same(&file(&path), &cue_at(1), "the file");
        refused(&mut editor, r#"Link("nowhere")"#);
    }

    #[test]
    fn a_device_that_restarts_starts_its_attempts_over() {
        let dir = TestDir::new("restart");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"Link("127.0.0.1:10"), Link("127.0.0.1:9")"#);
        assert!(editor.link().attempts().is_empty());
        assert_same(&text_of(&editor), SLICE, "the text");
    }

    #[test]
    fn an_editor_that_stopped_has_its_unsaved_text_offered_back() {
        let dir = TestDir::new("journal");
        let path = dir.write("slice.moves.ron", SLICE);
        let journal = dir.0.join("journal");
        let keeping = |dir: &TestDir| {
            let mut editor = editor(dir, Some(&path));
            editor.keep_journal_at(&journal);
            editor
        };
        let mut first = keeping(&dir);
        run(&mut first, &dir, &drag_window(5, 9));
        let unsaved = text_of(&first);
        // Stopped with no quit: its journal isn't closed.
        drop(first);
        assert_same(&file(&path), SLICE, "nothing saved");

        let mut next = keeping(&dir);
        run(&mut next, &dir, r#"Recover("slice.moves.ron")"#);
        assert_same(&text_of(&next), &unsaved, "what wasn't saved");
        assert!(next.is_dirty());
        refused(&mut next, r#"Recover("slice.moves.ron")"#);
        run(&mut next, &dir, "Undo");
        assert_same(&text_of(&next), SLICE, "the drag is a step to undo");
        run(&mut next, &dir, "Redo");
        drop(next);

        let theirs = SLICE.replacen("startup: 5", "startup: 6", 1);
        fs::write(&path, &theirs).expect("someone else's save");
        let mut last = keeping(&dir);
        run(&mut last, &dir, r#"Recover("slice.moves.ron")"#);
        assert!(last.current().expect("a file").conflict().is_some());
        assert_same(&file(&path), &theirs, "not written over");
        run(&mut last, &dir, "MineOnTop");
        assert_same(
            &text_of(&last),
            &unsaved.replacen("startup: 5", "startup: 6", 1),
            "theirs, and the drag",
        );
        assert_same(&file(&path), &theirs, "until a save");
        drop(last);

        let mut gone = keeping(&dir);
        run(&mut gone, &dir, r#"DropRecovered("slice.moves.ron")"#);
        refused(&mut gone, r#"Recover("slice.moves.ron")"#);
        assert_same(&text_of(&gone), &theirs, "the file's");
        assert!(!gone.is_dirty());
    }
}

mod panes {
    use super::*;

    #[test]
    fn opening_another_file_keeps_new_conflict_notices_for_the_previous_document() {
        let dir = TestDir::new("open_conflict");
        let path = dir.write("slice.moves.ron", SLICE);
        dir.write("hit_heavy.cue.ron", CUE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &drag_window(5, 9));
        fs::write(&path, SLICE.replacen("startup: 5", "startup: 6", 1)).expect("external change");
        let _ = editor.run(Command::Open(dir.0.join("hit_heavy.cue.ron")));
        assert!(
            editor.notice().is_some_and(
                |notice| notice.trouble && notice.text.contains("slice.moves.ron changed on disk")
            )
        );
        assert_same(&text_of(&editor), CUE, "the cue, opened");
    }

    #[test]
    fn a_refused_rename_uses_the_previous_slots_successful_preview() {
        let dir = TestDir::new("refused_rename");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        let before = editor.preview_run().expect("run").clone();
        run(&mut editor, &dir, r#"SetProperty(path: "moves[\"light_1\"].name", value: "\"renamed\"")"#);
        assert!(!editor.problems().is_empty(), "neutral still names light_1");
        assert_eq!(editor.selected_move_name(), Some("renamed"));
        assert_eq!(editor.preview_run(), Some(&before));
        run(&mut editor, &dir, r#"SelectMove("skill")"#);
        assert_eq!(editor.preview_run().expect("another prior move").move_name, "skill");
        assert_same(
            &text_of(&editor),
            &SLICE.replacen("name: \"light_1\"", "name: \"renamed\"", 1),
            "the text",
        );
    }

    #[test]
    fn the_browser_opens_each_document_as_its_kind() {
        let dir = TestDir::new("browser");
        let path = dir.write("a/player.moves.ron", PLAYER);
        let cue = dir.write("a/hit_heavy.cue.ron", CUE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"Open("a/hit_heavy.cue.ron")"#);
        assert_eq!(editor.current().expect("a file").file_name(), "hit_heavy.cue.ron");
        assert_eq!(editor.selected_move_name(), Some("light_1"), "the timeline stays on the moves");
        assert_same(&text_of(&editor), CUE, "the cue");
        run(&mut editor, &dir, r#"RemoveEntry("play[0].of[0]"), Open("a/player.moves.ron")"#);
        assert_eq!(editor.current().expect("a file").file_name(), "player.moves.ron");
        assert!(editor.is_dirty(), "the cue's edit counts");
        assert_same(&file(&cue), CUE, "nothing's written");
    }

    #[test]
    fn an_edit_on_the_timeline_goes_to_its_document() {
        let dir = TestDir::new("back");
        let path = dir.write("player.moves.ron", PLAYER);
        dir.write("hit_heavy.cue.ron", CUE);
        let mut editor = editor(&dir, Some(&path));
        // What a recording says: the timeline's drag makes its document the current one first.
        run(
            &mut editor,
            &dir,
            &format!(r#"Open("hit_heavy.cue.ron"), Open("player.moves.ron"), {}"#, drag_window(7, 13)),
        );
        assert_eq!(editor.current().expect("a file").file_name(), "player.moves.ron");
        assert_ne!(text_of(&editor), PLAYER);
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), PLAYER, "the text");
    }

    #[test]
    fn the_browser_looks_on_a_timer_and_not_during_a_gesture() {
        let dir = TestDir::new("timer");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        let scans = |editor: &Editor| editor.documents().stats().scans;
        dir.write("later.cue.ron", CUE);
        let before = scans(&editor);
        run(&mut editor, &dir, r#"Open("later.cue.ron")"#);
        assert_eq!(scans(&editor), before + 1, "a new ID takes one look");
        let before = scans(&editor);
        run(
            &mut editor,
            &dir,
            r#"Open("player.moves.ron"), Open("later.cue.ron"), Open("player.moves.ron")"#,
        );
        assert_eq!(scans(&editor), before, "both are known");

        run(&mut editor, &dir, &format!(r#"Begin("drag"), SetWindow(entry: {HIT}, from: 7, to: 13)"#));
        dir.write("during.cue.ron", CUE);
        let before = scans(&editor);
        editor.poll(1000.0);
        assert_eq!(scans(&editor), before, "no look during a gesture");
        run(&mut editor, &dir, "End");
        editor.poll(2000.0);
        assert!(scans(&editor) > before, "the look comes once it's over");
        assert_same(&text_of(&editor), &hand_edit(PLAYER, "light_1", "to: 11,", "to: 13,"), "the drag");
    }

    #[test]
    fn a_copy_made_with_its_sidecar_says_so_once() {
        let dir = TestDir::new("copy");
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        let sidecar = fs::read_to_string(dir.0.join("player.moves.ron.meta")).expect("a sidecar");
        dir.write("copy.moves.ron", PLAYER);
        dir.write("copy.moves.ron.meta", &sidecar);
        poll_until(&mut editor, "the notice", |editor| {
            editor.notice().is_some_and(|notice| notice.text.contains("hold the same ID"))
        });
        assert_eq!(editor.current().expect("a file").file_name(), "player.moves.ron");
        run(&mut editor, &dir, r#"SetProperty(path: "moves[0].startup", value: "8")"#);
        assert_same(&text_of(&editor), &PLAYER.replacen("startup: 7,", "startup: 8,", 1), "still editable");
    }

    #[test]
    fn properties_edit_a_document_the_editor_knows_nothing_of() {
        let dir = TestDir::new("properties");
        let path = dir.write("hit_heavy.cue.ron", CUE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"OpenPane(Properties), SetProperty(path: "volume_db[0]", value: "2.00")"#);
        assert!(text_of(&editor).contains("volume_db: (2.00, -1.0)"), "{}", text_of(&editor));
        assert_eq!(editor.current().expect("a file").undo_label(), Some("edit 0"));
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), CUE, "the number, undone");

        run(&mut editor, &dir, r#"SetOption(path: "attenuation", on: false)"#);
        assert!(!text_of(&editor).contains("attenuation"), "{}", text_of(&editor));
        run(&mut editor, &dir, r#"Undo, PickVariant(path: "bus", variant: "Ambience")"#);
        assert!(text_of(&editor).contains("bus: Ambience,"), "{}", text_of(&editor));
        run(&mut editor, &dir, r#"Undo, RemoveEntry("play[0].of[0]")"#);
        assert!(!text_of(&editor).contains("777260c4") && text_of(&editor).contains("impactPunch_heavy_001"));
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), CUE, "all undone");

        refused(&mut editor, r#"AddEntry("play[0].of")"#);
        let first_sound = "\"777260c4-e322-4f49-9a56-69565c28b7a0\"";
        refused(
            &mut editor,
            &format!(r#"SetProperty(path: "play[0].of[0][0]", value: {:?})"#, format!("{first_sound}x")),
        );
        assert_same(&text_of(&editor), CUE, "nothing went in");
        assert!(!editor.is_dirty());
        let other = "\"11111111-2222-4333-8444-555555555555\"";
        run(&mut editor, &dir, &format!(r#"SetProperty(path: "play[0].of[0][0]", value: {other:?})"#));
        assert_same(&text_of(&editor), &CUE.replacen(first_sound, other, 1), "as typed");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), CUE, "the text");
    }

    #[test]
    fn a_capture_of_the_panes_reads() {
        let dir = TestDir::new("capture");
        dir.write("hit_heavy.cue.ron", CUE);
        let path = dir.write("player.moves.ron", PLAYER);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"Open("hit_heavy.cue.ron"), OpenPane(Properties)"#);
        sapphire_editor::cli::write_frame(&mut editor, 1400, 900, &dir.0.join("panes.png")).expect("drawn");
        assert_same(&text_of(&editor), CUE, "drawing edits nothing");
    }

    #[test]
    fn properties_add_an_entry_from_its_type() {
        let dir = TestDir::new("add");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"AddEntry("moves")"#);
        assert_eq!(editor.notice(), None);
        assert_eq!(parse(&text_of(&editor)).moves.len(), parse(SLICE).moves.len() + 1);
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), SLICE, "the text");
    }

    #[test]
    fn undo_past_the_cap_says_so() {
        let dir = TestDir::new("cap");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        let id = editor.current().expect("a file").id();
        editor.documents_mut().any_mut(id).expect("open").set_history_limit(600);
        let moves: String = (1..=9).map(|at| format!("SetInstant(entry: {FIRST_CUE}, at: {at}),")).collect();
        run(&mut editor, &dir, &moves);
        assert!(editor.documents().any(id).expect("open").forgotten() > 0, "the cap is low enough to test");
        while editor.current().expect("a file").undo_label().is_some() {
            run(&mut editor, &dir, "Undo");
        }
        let floor = text_of(&editor);
        assert_ne!(floor, SLICE, "what's older is gone");
        run(&mut editor, &dir, "Undo");
        assert!(editor.notice().is_some_and(|notice| notice.text.contains("nothing older to undo")));
        assert_eq!(text_of(&editor), floor, "and does nothing else");
        run(&mut editor, &dir, "Redo");
        assert_ne!(text_of(&editor), floor);
    }

    #[test]
    fn several_documents_with_edits_are_all_asked_about_and_saved() {
        let dir = TestDir::new("several");
        let path = dir.write("slice.moves.ron", SLICE);
        let cue = dir.write("hit_heavy.cue.ron", CUE);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            &format!(
                r#"SetInstant(entry: {FIRST_CUE}, at: 3), Open("hit_heavy.cue.ron"), RemoveEntry("play[0].of[0]"), Close, CancelClose"#
            ),
        );
        assert_eq!(editor.current().expect("a file").file_name(), "hit_heavy.cue.ron");
        run(&mut editor, &dir, "Close, CloseWithoutSaving");
        assert_eq!(editor.current().expect("a file").file_name(), "slice.moves.ron", "the moves are left");
        assert_same(&file(&cue), CUE, "not saved");
        run(&mut editor, &dir, r#"Open("hit_heavy.cue.ron")"#);
        assert_same(&text_of(&editor), CUE, "opened again from the file");
        run(&mut editor, &dir, r#"RemoveEntry("play[0].of[0]")"#);
        let less = text_of(&editor);
        run(&mut editor, &dir, "Quit");
        assert!(!editor.may_quit());
        run(&mut editor, &dir, "SaveAndQuit");
        assert!(editor.may_quit());
        assert_same(&file(&path), &ui_cue_at(3), "the moves");
        assert_same(&file(&cue), &less, "the cue");
    }

    #[test]
    fn a_quit_that_cant_save_one_saves_the_rest_and_stays() {
        let dir = TestDir::new("several_fail");
        let path = dir.write("slice.moves.ron", SLICE);
        let cue = dir.write("hit_heavy.cue.ron", CUE);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            &format!(
                r#"SetInstant(entry: {FIRST_CUE}, at: 3), Open("hit_heavy.cue.ron"), RemoveEntry("play[0].of[0]")"#
            ),
        );
        fs::write(&cue, "someone's").expect("a change on disk");
        run(&mut editor, &dir, "Quit");
        refused(&mut editor, "SaveAndQuit");
        assert!(!editor.may_quit());
        assert_same(&file(&path), &ui_cue_at(3), "the moves");
        assert_eq!(file(&cue), "someone's");
    }

    #[test]
    fn a_conflict_can_put_edits_back_on_the_files_text() {
        let dir = TestDir::new("rebase");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, &format!("SetInstant(entry: {FIRST_CUE}, at: 3)"));
        fs::write(&path, SLICE.replacen("damage: 6,", "damage: 9,", 1)).expect("someone's save");
        poll_until(&mut editor, "the conflict", |editor| {
            editor.current().is_some_and(|file| file.conflict().is_some())
        });
        run(&mut editor, &dir, "MineOnTop");
        let text = text_of(&editor);
        assert!(text.contains("damage: 9,"), "{text}");
        assert_eq!(parse(&text).moves[0].cues[0].at, 3);
        assert!(editor.current().expect("a file").conflict().is_none());
        assert!(editor.notice().is_some_and(|notice| notice.text.contains("1 edits went back")));
    }

    /// With no device, a cvar set goes nowhere and edits nothing.
    #[test]
    fn cvars_come_from_the_device_and_set_it() {
        let dir = TestDir::new("cvars");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            r#"OpenPane(Cvars), Cvar(name: "test.bright", value: "2.0"), Cvar(name: "test.fog", value: "false")"#,
        );
        assert_same(&text_of(&editor), SLICE, "a cvar edits no document");
    }

    #[test]
    fn cvars_with_no_device_say_so() {
        let dir = TestDir::new("cvars_none");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, "OpenPane(Cvars)");
        assert!(editor.shell().is_open(Pane::Cvars));
        assert!(editor.link().device().is_none());
        assert_same(&text_of(&editor), SLICE, "the text");
    }

    #[test]
    fn the_zone_pane_lists_placements_and_a_row_selects_one() {
        let dir = TestDir::new("zone");
        let path = dir.write("z/proving_ground.zone.ron", PROVING_GROUND);
        let mut editor = editor(&dir, Some(&path));
        assert_eq!(editor.selected_placement(), None);
        run(&mut editor, &dir, r#"SelectPlacement(Some("placement-2"))"#);
        assert_eq!(editor.selected_placement(), Some(2));
        assert_same(&text_of(&editor), PROVING_GROUND, "selecting edits nothing");
        run(&mut editor, &dir, "SelectPlacement(None)");
        assert_eq!(editor.selected_placement(), None);
    }

    const YARD_COLLIDERS: &str = r#"(
    type: "sapphire::colliders::ColliderDocument",
    version: 1,
    data: (
        colliders: [
            (name: "ground", shape: Box(half_extents: (24.0, 0.5, 24.0)), position: (0.0, -0.5, 0.0)),
            // The posts.
            (name: "post_a", shape: Box(half_extents: (0.5, 2.0, 0.5)), position: (-6.0, 2.0, -6.0)),
            (name: "post_b", shape: Box(half_extents: (0.5, 2.0, 0.5)), position: (6.35, 2.0, -6.0)),
        ],
    ),
)
"#;
    const YARD_COLLIDERS_META: &str = r#"(
    type: "sapphire::assets::AssetMeta",
    version: 1,
    data: (
        id: "0aec06e8-ef8a-4f60-8dce-1e159cc277fa",
        importer: Colliders(()),
    ),
)
"#;
    const YARD: &str = r#"(
    type: "sapphire::zone::ZoneDocument",
    version: 1,
    data: (
        grid: (cell_size: 16.0),
        placements: [
            (prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (-6.0, 0.0, -6.0), colliders: ["post_a"]),
            (prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (6.35, 0.0, -6.0), colliders: ["post_b"]),
            (prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (0.0, 0.0, 3.0)),
        ],
        colliders: Some("0aec06e8-ef8a-4f60-8dce-1e159cc277fa"),
    ),
)
"#;

    #[test]
    fn three_placements_selected_take_one_edit_and_one_undo() {
        let dir = TestDir::new("three_placements");
        let path = dir.write("y/yard.zone.ron", YARD);
        let colliders = dir.write("y/yard.colliders.ron", YARD_COLLIDERS);
        dir.write("y/yard.colliders.ron.meta", YARD_COLLIDERS_META);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            r#"SelectObjects([
                (document: "y/yard.zone.ron", path: "placements[0]"),
                (document: "y/yard.zone.ron", path: "placements[1]"),
                (document: "y/yard.zone.ron", path: "placements[2]"),
            ]),
            SetShared(path: "at[1]", value: "1.0"),
            Save"#,
        );
        assert_eq!(editor.selected_placements(), [0, 1, 2]);
        let raised = file(&path);
        for at in ["(-6.0, 1.0, -6.0)", "(6.35, 1.0, -6.0)", "(0.0, 1.0, 3.0)"] {
            assert!(raised.contains(at), "{at} in {raised}");
        }
        // A save of the zone alone writes the colliders it moved too, or the cook would refuse what it wrote.
        let moved = file(&colliders);
        assert!(moved.contains("position: (-6.0, 3.0, -6.0)"), "{moved}");
        assert!(moved.contains("position: (6.35, 3.0, -6.0)"), "{moved}");
        assert!(moved.contains("// The posts."), "its comments stay");
        run(&mut editor, &dir, "Undo, Save");
        assert_same(&file(&path), YARD, "one undo for the three");
        assert_same(&file(&colliders), YARD_COLLIDERS, "and their colliders");
        let ninth = format!(
            r#"SelectObjects([(document: {:?}, path: "placements[9]")])"#,
            path.display().to_string()
        );
        assert!(refused(&mut editor, &ninth).contains("nothing at"), "{}", refused(&mut editor, &ninth));
        let high = refused(&mut editor, r#"SetShared(path: "at[1]", value: "\"high\"")"#);
        assert!(high.contains("Expected float"), "{high}");
        assert_same(&file(&path), YARD, "a refused edit moves nothing");
    }

    #[test]
    fn the_sequencer_plays_and_scrubs_its_playhead() {
        let dir = TestDir::new("sequencer");
        let path = dir.write("s/square_look.timeline.ron", SQUARE_LOOK);
        let mut editor = editor(&dir, Some(&path));
        assert_eq!(editor.playhead(), Some(0));
        run(&mut editor, &dir, "PlayTimeline, PauseTimeline, Scrub(120)");
        assert_eq!(editor.playhead(), Some(120), "the middle of four seconds");
        assert_same(&text_of(&editor), SQUARE_LOOK, "scrubbing edits nothing");
    }

    #[test]
    fn the_screen_pane_lists_its_paths_and_picks_an_aspect() {
        let dir = TestDir::new("screen");
        let path = dir.write("u/character.ui.ron", CHARACTER_SCREEN);
        dir.write("u/character.ui.ron.meta", CHARACTER_SCREEN_META);
        dir.write("u/character.model.ron", CHARACTER_MODEL);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            r#"Aspect(Standard), Language("ja"), StandIn(path: "hp", value: Number(6240.0))"#,
        );
        let frame = editor.viewport_frame();
        let screen = frame.screen.expect("a screen");
        assert_eq!((screen.aspect, screen.language), (Aspect::Standard, "ja"));
        assert!(screen.values.contains(&("hp".to_owned(), StandIn::Number(6240.0))));
        refused(&mut editor, r#"StandIn(path: "no_such_path", value: Bool(true))"#);
        refused(&mut editor, r#"Language("xx")"#);
        assert_same(&text_of(&editor), CHARACTER_SCREEN, "previewing edits nothing");
    }

    #[test]
    fn the_screen_pane_follows_the_model_its_document_names() {
        let dir = TestDir::new("screen_model");
        let path = dir.write("u/character.ui.ron", CHARACTER_SCREEN);
        dir.write("u/character.ui.ron.meta", CHARACTER_SCREEN_META);
        dir.write("u/character.model.ron", CHARACTER_MODEL);
        dir.write("u/hud.model.ron", HUD_MODEL);
        let mut editor = editor(&dir, Some(&path));
        let reads = |editor: &mut Editor, path: &str| {
            editor.viewport_frame().screen.expect("a screen").values.iter().any(|(known, _)| known == path)
        };
        run(&mut editor, &dir, r#"StandIn(path: "level", value: Number(77.0))"#);
        assert!(reads(&mut editor, "crit"));
        run(&mut editor, &dir, r#"SetProperty(path: "model", value: "Some(\"hud.model.ron\")")"#);
        assert!(reads(&mut editor, "touch") && !reads(&mut editor, "crit"), "the hud's");
        run(&mut editor, &dir, "Undo");
        assert!(reads(&mut editor, "crit"), "undo names the character's again");
        assert_same(&text_of(&editor), CHARACTER_SCREEN, "the text");
    }

    #[test]
    fn the_screen_pane_reads_its_model_file_again_when_it_changes_on_disk() {
        let dir = TestDir::new("screen_model_disk");
        let path = dir.write("u/character.ui.ron", CHARACTER_SCREEN);
        dir.write("u/character.ui.ron.meta", CHARACTER_SCREEN_META);
        let model = dir.write("u/character.model.ron", CHARACTER_MODEL);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"StandIn(path: "level", value: Number(77.0))"#);
        fs::write(&model, CHARACTER_MODEL.replace("\"crit\"", "\"luck\"")).expect("an outside edit");
        editor.poll(100.0);
        let values = editor.viewport_frame().screen.expect("a screen").values.to_vec();
        assert!(values.iter().any(|(known, _)| known == "luck"), "{values:?}");
        assert!(values.contains(&("level".to_owned(), StandIn::Number(77.0))), "what was typed: {values:?}");
        assert_same(&text_of(&editor), CHARACTER_SCREEN, "following the model edits nothing");
    }
}

/// The slice with its first cue at `at`, for the panes' twins.
fn ui_cue_at(at: u32) -> String {
    SLICE.replacen("(name: \"swing_windup\", at: 0)", &format!("(name: \"swing_windup\", at: {at})"), 1)
}

mod docking {
    use super::*;

    fn titles(editor: &Editor) -> usize {
        Pane::ALL.into_iter().filter(|pane| editor.shell().is_open(*pane)).count()
    }

    #[test]
    fn every_pane_is_open_with_a_tab_and_those_showing_draw() {
        let dir = TestDir::new("all");
        let mut editor = editor(&dir, None);
        run(&mut editor, &dir, "");
        assert_eq!(titles(&editor), Pane::ALL.len());
        draw(&mut editor);
    }

    #[test]
    fn a_tab_switches_what_shows() {
        let dir = TestDir::new("tab");
        let mut editor = editor(&dir, None);
        run(&mut editor, &dir, "OpenPane(Properties)");
        assert_eq!(titles(&editor), Pane::ALL.len(), "opening what's open shows it");
    }

    #[test]
    fn a_pane_splits_by_call_and_the_tree_draws_it() {
        let dir = TestDir::new("split");
        let mut editor = editor(&dir, None);
        let before = editor.shell().tree.clone();
        run(&mut editor, &dir, "Split(pane: Device, of: Preview, direction: Vertical)");
        assert_eq!(titles(&editor), Pane::ALL.len(), "it moved");
        assert_ne!(editor.shell().tree, before);
        draw(&mut editor);
    }

    #[test]
    fn a_pane_closes_by_its_tabs_close_button_and_comes_back() {
        let dir = TestDir::new("close");
        let mut editor = editor(&dir, None);
        run(&mut editor, &dir, "ClosePane(Device)");
        assert!(!editor.shell().is_open(Pane::Device));
        assert_eq!(titles(&editor), Pane::ALL.len() - 1);
        run(&mut editor, &dir, "OpenPane(Device), OpenPane(Cvars)");
        assert_eq!(titles(&editor), Pane::ALL.len());
    }

    #[test]
    fn closing_everything_leaves_a_way_back() {
        let dir = TestDir::new("everything");
        let mut editor = editor(&dir, None);
        let all: String = Pane::ALL.iter().map(|pane| format!("ClosePane({pane:?}),")).collect();
        run(&mut editor, &dir, &all);
        assert_eq!(titles(&editor), 0);
        draw(&mut editor);
        run(&mut editor, &dir, "OpenPane(Timeline)");
        assert!(editor.shell().is_open(Pane::Timeline));
        draw(&mut editor);
    }

    #[test]
    fn a_tab_dragged_to_a_panes_edge_splits_it() {
        let dir = TestDir::new("dragged");
        let mut editor = editor(&dir, None);
        run(&mut editor, &dir, "Split(pane: Cvars, of: Preview, direction: Vertical)");
        draw(&mut editor);
        let shell = editor.shell();
        let mut tile = shell.find(Pane::Cvars).expect("still there");
        let split = loop {
            tile = shell.tree.tiles.parent_of(tile).expect("in a split");
            if let Some(egui_tiles::Container::Linear(linear)) = shell.tree.tiles.get_container(tile) {
                break linear;
            }
        };
        assert_eq!(split.dir, egui_tiles::LinearDir::Vertical, "{:#?}", shell.tree);
        assert!(split.children.iter().any(|child| Some(*child) == shell.find(Pane::Preview)
            || shell.tree.tiles.parent_of(shell.find(Pane::Preview).expect("open")) == Some(*child)));
    }

    #[test]
    fn a_layout_goes_to_ron_and_back_the_same() {
        let dir = TestDir::new("ron");
        let mut editor = editor(&dir, None);
        run(&mut editor, &dir, "Split(pane: Device, of: Preview, direction: Vertical), ClosePane(Cvars)");
        let saved = editor.shell().save_layout().expect("saves");
        let mut restored = self::editor(&dir, None);
        restored.shell_mut().restore_layout(&saved).expect("restores");
        assert_eq!(restored.shell().tree, editor.shell().tree, "the same tree");
    }

    #[test]
    fn a_layout_that_doesnt_read_leaves_the_one_there() {
        let dir = TestDir::new("unread");
        let mut editor = editor(&dir, None);
        run(&mut editor, &dir, "ClosePane(Cvars)");
        let tree = editor.shell().tree.clone();
        assert!(editor.shell_mut().restore_layout("(not a layout").is_err());
        assert_eq!(editor.shell().tree, tree);
    }

    /// A script that doesn't read is refused before any of it runs, as a layout is.
    #[test]
    fn malformed_layout_graphs_are_refused_before_traversal() {
        let dir = TestDir::new("malformed");
        let mut editor = editor(&dir, None);
        let tree = editor.shell().tree.clone();
        for bad in [
            "(version: 1, commands: [ClosePane(Device), ClosePane(Nowhere)])",
            "(version: 1, commands: [ClosePane(Device)], also: 1)",
            "(version: 2, commands: [ClosePane(Device)])",
            "(commands: [ClosePane(Device)])",
        ] {
            assert!(Script::parse(bad).is_err(), "{bad}");
        }
        let path =
            dir.write(&format!("bad{SCRIPT_SUFFIX}"), "(version: 1, commands: [ClosePane(Device), Explode])");
        assert!(Script::read(&path).is_err());
        assert_eq!(editor.shell().tree, tree);
        run(&mut editor, &dir, "");
        assert_eq!(editor.shell().tree, tree);
    }

    #[test]
    fn the_editors_layout_lasts_past_the_editor() {
        let dir = TestDir::new("lasts");
        let file = dir.0.join("layout.ron");
        let mut editor = editor(&dir, None);
        editor.keep_layout_at(file.clone());
        draw(&mut editor);
        editor.poll(100.0);
        assert!(!file.exists(), "nothing's changed, so nothing's written");
        run(&mut editor, &dir, "ClosePane(Device)");
        editor.poll(200.0);
        assert!(file.exists(), "kept");
        drop(editor);

        let mut editor = self::editor(&dir, None);
        editor.keep_layout_at(file);
        assert!(!editor.shell().is_open(Pane::Device));
        run(&mut editor, &dir, "OpenPane(Device)");
        assert!(editor.shell().is_open(Pane::Device));
    }

    #[test]
    fn a_kept_layout_that_doesnt_read_says_so_and_starts_from_the_usual() {
        let dir = TestDir::new("bad");
        let file = dir.write("layout.ron", "(not a layout");
        let mut editor = editor(&dir, None);
        editor.keep_layout_at(file.clone());
        run(&mut editor, &dir, "");
        editor.poll(100.0);
        assert!(editor.notice().is_some_and(|notice| notice.text.contains("layout.ron")));
        assert_eq!(titles(&editor), Pane::ALL.len());
        assert_eq!(fs::read_to_string(&file).expect("still there"), "(not a layout");
    }

    #[test]
    fn a_failed_layout_write_is_retried_without_another_layout_change() {
        let dir = TestDir::new("retry");
        let file = dir.0.join("layout.ron");
        let mut editor = editor(&dir, None);
        editor.keep_layout_at(file.clone());
        draw(&mut editor);
        fs::create_dir(&file).expect("a directory blocks replacement");
        run(&mut editor, &dir, "ClosePane(Device)");
        editor.poll(100.0);
        assert!(file.is_dir(), "the failure leaves the destination alone");
        fs::remove_dir(&file).expect("remove the transient obstacle");
        editor.poll(200.0);
        let saved = fs::read_to_string(&file).expect("the unchanged layout was retried");
        let mut shell = sapphire_editor::shell::Shell::new();
        shell.restore_layout(&saved).expect("reads");
        assert!(!shell.is_open(Pane::Device));
    }
}

/// What a person does in a window, kept by `--record` while kittest input drives the editor, and replayed by the
/// binary's `--run`: the same bytes saved, and the same frame drawn.
mod recorded {
    use super::*;

    fn focused_field<'a>(harness: &'a Harness<'static, Editor>) -> egui_kittest::Node<'a> {
        harness
            .get_all(By::new().role(Role::TextInput))
            .find(|field| field.is_focused())
            .expect("a field has the keyboard")
    }

    /// Opens the palette, types `text` and presses Enter.
    fn palette(harness: &mut Harness<'static, Editor>, text: &str) {
        harness.key_press_modifiers(Modifiers::COMMAND, Key::P);
        harness.run_steps(2);
        assert!(harness.state().palette_is_open());
        focused_field(harness).type_text(text);
        harness.run_steps(2);
        harness.key_press(Key::Enter);
        harness.run_steps(3);
    }

    fn drag(harness: &mut Harness<'static, Editor>, from: Pos2, to: Pos2) {
        harness.hover_at(from);
        harness.run_steps(2);
        harness.drag_at(from);
        harness.step();
        for part in 1..=4 {
            harness.hover_at(from.lerp(to, part as f32 / 4.0));
            harness.step();
        }
        harness.drop_at(to);
        harness.run_steps(3);
    }

    #[test]
    fn a_recorded_session_replays_to_the_same_bytes_and_frame() {
        let session = TestDir::new("session");
        let path = session.write("player.moves.ron", PLAYER);
        let script = session.0.join(format!("session{SCRIPT_SUFFIX}"));
        let mut editor = editor(&session, Some(&path));
        editor.record_to(script.clone());
        let mut harness = Harness::builder()
            .with_size((1400.0, 900.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), editor);
        harness.run_steps(3);

        // A drag of a window's end, two ticks on.
        let start = harness.get_by_label("hit volumes light_1_hit 7..11 start").rect().center();
        let end = harness.get_by_label("hit volumes light_1_hit 7..11 end").rect().center();
        let per_tick = (end.x - start.x + 2.0) / 4.0;
        drag(&mut harness, end, end + egui::vec2(2.0 * per_tick, 0.0));
        // Taken back by the palette, and brought back by the keys.
        palette(&mut harness, "Undo");
        assert_same(harness.state().current().expect("a file").text(), PLAYER, "undone");
        harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        harness.run_steps(2);
        // A cue renamed in the inspector.
        harness.get_by_label("cues swing at 7").click();
        harness.run_steps(3);
        harness.get(By::new().role(Role::TextInput).value("\"swing\"")).focus();
        harness.run_steps(1);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.run_steps(1);
        focused_field(&harness).type_text("\"swung\"");
        harness.run_steps(2);
        harness.key_press(Key::Enter);
        harness.run_steps(3);
        // The device pane closed by its tab's button: what it says of a link changes by the clock.
        let tab = harness.get_by_label("Device").rect();
        let close = Pos2::new(tab.right() - 14.0, tab.center().y);
        harness.hover_at(close);
        harness.run_steps(2);
        harness.drag_at(close);
        harness.run_steps(1);
        harness.drop_at(close);
        harness.run_steps(3);
        // The playhead by a key, and a save by the palette.
        harness.key_press(Key::Home);
        harness.run_steps(2);
        palette(&mut harness, "Save");
        assert!(!harness.state().palette_is_open());

        let edited = hand_edit(PLAYER, "light_1", "to: 11,", "to: 13,").replacen(
            "(name: \"swing\", at: 7)",
            "(name: \"swung\", at: 7)",
            1,
        );
        assert_same(&file(&path), &edited, "saved");
        let recorded = harness.state().recorded().expect("recording").clone();
        let names: Vec<_> = recorded.commands.iter().map(Command::name).collect();
        for name in
            ["Open", "Begin", "SetWindow", "End", "Undo", "Redo", "SetValue", "ClosePane", "Tick", "Save"]
        {
            assert!(names.contains(&name), "{name} in {names:?}");
        }
        let frame = session.0.join("session.png");
        sapphire_editor::cli::write_frame(harness.state_mut(), 1000, 700, &frame).expect("drawn");
        drop(harness);
        let kept = Script::read(&script).expect("the recording reads");
        assert_eq!(kept.commands.len(), recorded.commands.len(), "the file has every command");

        // Played back on the documents as they were, by the binary.
        let replay = TestDir::new("replay");
        replay.write("player.moves.ron", PLAYER);
        fs::copy(path.with_extension("ron.meta"), replay.0.join("player.moves.ron.meta"))
            .expect("its sidecar");
        fs::copy(&script, replay.0.join(format!("session{SCRIPT_SUFFIX}"))).expect("the recording");
        let output = Process::new(env!("CARGO_BIN_EXE_sapphire_editor"))
            .args(["player.moves.ron", "--run", &format!("session{SCRIPT_SUFFIX}"), "--capture"])
            .arg(session.0.join("replay.png"))
            .args(["--size", "1000x700", "--link", "127.0.0.1:9", "--content", "no cook here"])
            .current_dir(&replay.0)
            .output()
            .expect("it runs");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_same(&file(&replay.0.join("player.moves.ron")), &edited, "the same bytes");
        let (session_frame, replayed) =
            (fs::read(&frame).expect("a PNG"), fs::read(session.0.join("replay.png")).expect("a PNG"));
        assert!(session_frame == replayed, "the same frame, to the byte");
    }

    /// The palette finds by words, fills in what a command takes, and Escape closes it with nothing run.
    #[test]
    fn the_palette_runs_what_it_finds() {
        let dir = TestDir::new("palette");
        let path = dir.write("warrior.moves.ron", WARRIOR);
        let mut harness = Harness::builder()
            .with_size((1400.0, 900.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), editor(&dir, Some(&path)));
        harness.run_steps(3);
        palette(&mut harness, "Tick(5)");
        assert_eq!(harness.state().tick(), 5);
        assert!(!harness.state().palette_is_open());

        // A pick that takes arguments puts its template in the field, to fill in.
        harness.key_press_modifiers(Modifiers::COMMAND, Key::P);
        harness.run_steps(2);
        focused_field(&harness).type_text("select move");
        harness.run_steps(2);
        harness.get_by_label_contains("SelectMove:").click();
        harness.run_steps(3);
        assert!(harness.state().palette_is_open());
        let filled = harness.get_all(By::new().role(Role::TextInput)).any(|field| {
            field.accesskit_node().value().is_some_and(|value| value.starts_with("SelectMove("))
        });
        assert!(filled, "the template, to fill in");
        harness.key_press(Key::Escape);
        harness.run_steps(2);
        assert!(!harness.state().palette_is_open());
        assert_same(harness.state().current().expect("a file").text(), WARRIOR, "nothing ran");
    }
}

/// A zone's placements by their IDs (M6 T8a): a selection stays on what it named however its list shifts, and
/// `MigrateZone` writes a version 1 zone's IDs.
mod zone_ids {
    use sapphire_zone::ZoneDocument;

    use super::*;

    const POSTS: &str = r#"(
    type: "sapphire::zone::ZoneDocument",
    version: 2,
    data: (
        grid: (cell_size: 16.0),
        placements: [
            (id: "3fa91c07", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (-6.0, 0.0, -6.0)),
            (id: "8b2e5d40", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (6.0, 0.0, -6.0)),
            (id: "c41d0a9f", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (0.0, 0.0, 3.0)),
        ],
        spawners: [(name: "guard", position: (0.0, 0.0, 8.0), spawns: "warrior")],
    ),
)
"#;

    /// [`POSTS`] as someone else saved it: a placement put in first, so every other one is a place further on.
    fn with_one_first(text: &str) -> String {
        let first = r#"(id: "3fa91c07""#;
        let new = r#"(id: "0d17e6b2", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (0.0, 0.0, -9.0)),
            "#;
        text.replacen(first, &format!("{new}{first}"), 1)
    }

    fn value(editor: &Editor) -> ZoneDocument {
        ZoneDocument::from_ron(&text_of(editor)).expect("a zone")
    }

    fn placed(value: &ZoneDocument, id: &str) -> [f32; 3] {
        value.placements.iter().find(|placement| placement.id == id).expect(id).at
    }

    /// An edit of the placement selected, unsaved, when someone saves the zone with one put in before it: the
    /// edit goes back on top on the same placement, and the selection is still that placement, a place further.
    #[test]
    fn a_selection_survives_a_merge_that_shifts_its_place() {
        let dir = TestDir::new("zone_ids_merge");
        let path = dir.write("y/posts.zone.ron", POSTS);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            r#"SelectPlacement(Some("8b2e5d40")), SetShared(path: "at[1]", value: "2.0")"#,
        );
        assert_eq!(editor.selected_placements(), [1]);
        fs::write(&path, with_one_first(POSTS)).expect("someone's save");
        poll_until(&mut editor, "the conflict", |editor| {
            editor.current().is_some_and(|file| file.conflict().is_some())
        });
        run(&mut editor, &dir, "MineOnTop");
        let merged = value(&editor);
        assert_eq!(merged.placements[0].id, "0d17e6b2", "theirs is in");
        assert_eq!(placed(&merged, "8b2e5d40"), [6.0, 2.0, -6.0], "mine is on the same placement");
        assert_eq!(editor.selected_placements(), [2], "the selection follows it to its new place");

        // The next edit lands on it too, and so does one after an entry before it is taken out.
        run(&mut editor, &dir, r#"SetShared(path: "at[1]", value: "3.0"), RemoveEntry("placements[0]")"#);
        assert_eq!(editor.selected_placements(), [1]);
        run(&mut editor, &dir, r#"SetShared(path: "at[0]", value: "5.0"), Save"#);
        let saved = ZoneDocument::from_ron(&file(&path)).expect("a zone");
        assert_eq!(placed(&saved, "8b2e5d40"), [5.0, 3.0, -6.0]);
        assert_eq!(placed(&saved, "3fa91c07"), [-6.0, 0.0, -6.0], "the others are as they were");
        assert_eq!(placed(&saved, "c41d0a9f"), [0.0, 0.0, 3.0]);
        assert!(refused(&mut editor, r#"SelectPlacement(Some("0d17e6b2"))"#).contains("no placement"));
    }

    /// A placement added takes an ID of its own, and a selection made before it stays where it was.
    #[test]
    fn an_added_placement_takes_a_fresh_id() {
        let dir = TestDir::new("zone_ids_add");
        let path = dir.write("y/posts.zone.ron", POSTS);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"SelectPlacement(Some("c41d0a9f")), AddEntry("placements")"#);
        let added = value(&editor);
        assert_eq!(added.placements.len(), 4);
        let id = &added.placements[3].id;
        assert!(id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert!(added.placements[..3].iter().all(|placement| &placement.id != id));
        assert_eq!(editor.selected_placements(), [2]);
    }

    /// A version 1 zone refuses an edit that would shift its places' IDs until `MigrateZone` writes them, which
    /// saves the file as the migration writes it.
    #[test]
    fn migrate_zone_writes_a_version_one_zones_ids_and_saves() {
        let dir = TestDir::new("zone_ids_migrate");
        let old = POSTS
            .replace("version: 2", "version: 1")
            .replace(r#"id: "3fa91c07", "#, "")
            .replace(r#"id: "8b2e5d40", "#, "")
            .replace(r#"id: "c41d0a9f", "#, "");
        let path = dir.write("y/posts.zone.ron", &old);
        let mut editor = editor(&dir, Some(&path));
        let shifts = refused(&mut editor, r#"RemoveEntry("placements[0]")"#);
        assert!(shifts.contains("MigrateZone"), "{shifts}");
        run(&mut editor, &dir, "MigrateZone");
        let migrated = sapphire_document::zone::migrate(&old).expect("migrates").expect("at version 1");
        assert_same(&file(&path), &migrated, "saved as the migration writes it");
        assert!(migrated.contains(r#"(id: "placement-1", prefab"#), "{migrated}");
        assert!(editor.notice().is_some_and(|notice| notice.text.contains("version 2")));
        run(&mut editor, &dir, "MigrateZone");
        assert!(editor.notice().is_some_and(|notice| notice.text.contains("newest version already")));
        run(&mut editor, &dir, r#"RemoveEntry("placements[0]"), Save"#);
        let saved = ZoneDocument::from_ron(&file(&path)).expect("a zone");
        let ids: Vec<&str> = saved.placements.iter().map(|placement| placement.id.as_str()).collect();
        assert_eq!(ids, ["placement-1", "placement-2"], "the IDs stay with their placements");
    }

    /// An ID another entry has, or a `rotation` beside a `turn`, is refused by its field, the text as it was.
    #[test]
    fn a_shared_id_or_a_turn_beside_a_rotation_is_refused_by_its_field() {
        let dir = TestDir::new("zone_ids_refused");
        let path = dir.write("y/posts.zone.ron", POSTS);
        let mut editor = editor(&dir, Some(&path));
        let shared = refused(&mut editor, r#"SetProperty(path: "placements[1].id", value: "\"3fa91c07\"")"#);
        assert!(shared.contains("placements[1].id") && shared.contains("placements[0]"), "{shared}");
        run(&mut editor, &dir, r#"SetProperty(path: "placements[1].turn", value: "30.0")"#);
        let both = refused(
            &mut editor,
            r#"SetProperty(path: "placements[1].rotation", value: "Some((0.0, 30.0, 0.0))")"#,
        );
        assert!(both.contains("placements[1].rotation") && both.contains("turn"), "{both}");
        run(&mut editor, &dir, "Undo");
        assert_same(&text_of(&editor), POSTS, "only the turn went in, and it came out");
    }

    /// A session that adds a placement records the ID it drew (`AddEntryAs`), so its replay makes the same entry,
    /// selects it by that ID and moves it, to the same bytes; one asked for by a taken ID is refused.
    #[test]
    fn a_recorded_add_replays_to_the_same_bytes() {
        let dir = TestDir::new("zone_ids_recorded");
        let path = dir.write("y/posts.zone.ron", POSTS);
        let script = dir.0.join(format!("session{SCRIPT_SUFFIX}"));
        let mut first = editor(&dir, Some(&path));
        first.record_to(script.clone());
        run(&mut first, &dir, r#"AddEntry("placements")"#);
        let id = value(&first).placements[3].id.clone();
        let commands =
            format!(r#"SelectPlacement(Some("{id}")), SetShared(path: "at[0]", value: "4.0"), Save"#);
        run(&mut first, &dir, &commands);
        let saved = file(&path);
        let recorded = format!("{:?}", first.recorded().expect("recording").commands);
        assert!(
            recorded.contains(&format!("AddEntryAs {{ path: \"placements\", id: \"{id}\" }}")),
            "{recorded}"
        );
        drop(first);

        fs::write(&path, POSTS).expect("the zone as it was");
        let mut replay = editor(&dir, Some(&path));
        let kept = Script::read(&script).expect("the recording reads");
        replay.run_script(&kept).unwrap_or_else(|error| panic!("{error}"));
        assert_same(&file(&path), &saved, "the replay's save");
        assert_eq!(replay.selected_placements(), [3]);
        let taken = refused(&mut replay, &format!(r#"AddEntryAs(path: "placements", id: "{id}")"#));
        assert!(taken.contains(&id) && taken.contains("already"), "{taken}");
    }

    /// A selected placement given a new ID, and a selected spawner a new name, stay selected by it, and an undo
    /// keeps them selected by the old one.
    #[test]
    fn a_renamed_entry_stays_selected() {
        let dir = TestDir::new("zone_ids_rename");
        let path = dir.write("y/posts.zone.ron", POSTS);
        let mut editor = editor(&dir, Some(&path));
        run(
            &mut editor,
            &dir,
            r#"SelectPlacement(Some("8b2e5d40")), SetShared(path: "id", value: "\"gate_post\"")"#,
        );
        assert_eq!(editor.selected_placements(), [1]);
        assert_eq!(value(&editor).placements[1].id, "gate_post");
        run(&mut editor, &dir, r#"SetShared(path: "at[1]", value: "1.0"), Undo, Undo"#);
        assert_eq!(editor.selected_placements(), [1], "back to its old ID");
        assert_same(&text_of(&editor), POSTS, "both undone");

        let spawner = |name: &str| {
            format!(r#"SelectObjects([(document: "y/posts.zone.ron", path: "spawners[\"{name}\"]")])"#)
        };
        run(&mut editor, &dir, &spawner("guard"));
        run(
            &mut editor,
            &dir,
            r#"SetShared(path: "name", value: "\"sentry\""), SetShared(path: "position[0]", value: "2.0")"#,
        );
        let zone = value(&editor);
        assert_eq!((zone.spawners[0].name.as_str(), zone.spawners[0].position[0]), ("sentry", 2.0));
        refused(&mut editor, &spawner("guard"));
    }

    /// A merge that brings in an ID the zone's unsaved entry has: the entry doesn't go back on top, naming the ID
    /// and its list, and nothing is renumbered.
    #[test]
    fn a_merge_bringing_in_a_taken_id_is_refused_by_its_field() {
        let dir = TestDir::new("zone_ids_taken");
        let path = dir.write("y/posts.zone.ron", POSTS);
        let mut editor = editor(&dir, Some(&path));
        run(&mut editor, &dir, r#"AddEntryAs(path: "placements", id: "0d17e6b2")"#);
        fs::write(&path, with_one_first(POSTS)).expect("someone's save, with the same ID");
        poll_until(&mut editor, "the conflict", |editor| {
            editor.current().is_some_and(|file| file.conflict().is_some())
        });
        let said = match editor.run(Command::MineOnTop) {
            Err(text) => text,
            Ok(()) => editor.notice().map(|notice| notice.text.clone()).unwrap_or_default(),
        };
        let text = text_of(&editor);
        assert!(said.contains("`0d17e6b2` in `placements`") && said.contains("didn't"), "{said}");
        assert_eq!(text.matches("0d17e6b2").count(), 1, "never renumbered, and not put in twice: {text}");
    }
}
