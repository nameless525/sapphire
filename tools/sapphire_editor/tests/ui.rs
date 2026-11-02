//! The editor's interface, driven as a person drives it with no window: `egui_kittest` finds its widgets
//! by what they're called and sends real pointer and key events, and everything goes through
//! [`Editor::ui`], as the window's frames do. Edits land in temporary copies of the real documents.

use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{Key, Modifiers, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use sapphire_assets::MemoryPackSource;
use sapphire_editor::capture::{self, Textures};
use sapphire_editor::edits::{EntryRef, Track};
use sapphire_editor::file::judge;
use sapphire_editor::link::Pushed;
use sapphire_editor::{Editor, Options};
use sapphire_livelink::protocol::{DeviceStatus, MoveAttempt, ReplyBody};
use sapphire_livelink::{LinkMode, LiveLink, LivePacks, Push};
use sapphire_moves::MoveSetDocument;
use sapphire_time::Stopwatch;

const PLAYER: &str = include_str!("../../../content/combat/player/player.moves.ron");
const PLAYER_META: &str = include_str!("../../../content/combat/player/player.moves.ron.meta");
const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");
const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");

/// A directory of this test's own, gone when it ends.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("sapphire_editor_ui_{name}_{}_{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).expect("a test file");
        path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An editor on `path`, talking to a port nobody has, on a window's worth of interface.
fn harness(path: &Path) -> Harness<'static, Editor> {
    let options = Options {
        file: Some(path.to_owned()),
        content: Some(path.with_file_name("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
    };
    let editor = Editor::new(options);
    Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), editor)
}

fn parse(text: &str) -> MoveSetDocument {
    MoveSetDocument::parse(text).expect("it parses")
}

/// Where a test's captures go: a folder a person names, or the test's own.
fn capture_path(name: &str) -> PathBuf {
    let dir = std::env::var_os("SAPPHIRE_EDITOR_CAPTURES")
        .map_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")), PathBuf::from);
    fs::create_dir_all(&dir).expect("a folder for captures");
    dir.join(name)
}

/// What the editor looks like, with no window: for a person, or an agent, to read.
#[test]
fn a_capture_of_the_timeline_reads() {
    let dir = TestDir::new("capture");
    let path = dir.write("warrior.moves.ron", WARRIOR);
    let options = Options {
        file: Some(path),
        content: Some(dir.0.join("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
    };
    let mut editor = Editor::new(options);
    assert!(editor.select_move("swipe"));
    editor.set_tick(20);
    let ctx = egui::Context::default();
    let mut textures = Textures::default();
    let (width, height) = (1400, 900);
    let mut last = None;
    for pass in 0..4 {
        let input = capture::raw_input(width, height, f64::from(pass) * 0.1);
        last = Some(capture::draw(&ctx, &mut textures, input, width, height, |ui| editor.ui(ui)));
    }
    let image = last.expect("a capture");
    image.write_png(&capture_path("timeline.png")).expect("written");
    // The interface has drawn: the timeline's background and the preview's differ from black.
    let lit = image.pixels.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] > 20).count();
    assert!(lit > 100_000, "{lit} lit pixels");

    // The same with a window past the end, to read how a refusal looks.
    let swipe = parse(WARRIOR).moves.iter().position(|mv| mv.name == "swipe").expect("there");
    let entry = EntryRef { mv: swipe, track: Track::Hitboxes, index: 0 };
    let edit = editor.file_mut().expect("a file").edit("too far", |text| text.set_window(entry, 5, 200));
    edit.expect("an edit");
    editor.refresh();
    assert!(!editor.problems().is_empty());
    let input = capture::raw_input(width, height, 1.0);
    let image = capture::draw(&ctx, &mut textures, input, width, height, |ui| editor.ui(ui));
    image.write_png(&capture_path("timeline_refused.png")).expect("written");
}

/// The binary's one way of running with no window, which is how an agent looks at a move.
#[test]
fn the_binary_captures_a_frame_with_no_window() {
    let dir = TestDir::new("binary");
    let path = dir.write("warrior.moves.ron", WARRIOR);
    let png = dir.0.join("frame.png");
    let run = |name: &str| {
        Command::new(env!("CARGO_BIN_EXE_sapphire_editor"))
            .arg(&path)
            .arg("--capture")
            .arg(&png)
            .args(["--size", "800x500", "--tick", "12", "--move", name, "--content"])
            .arg(dir.0.join("no cook here"))
            .output()
            .expect("it runs")
    };
    let output = run("swipe");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(fs::metadata(&png).expect("a PNG").len() > 1000);
    let output = run("no such move");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no such move"));
}

#[test]
fn the_ui_opens_and_names_its_tracks() {
    let dir = TestDir::new("names");
    let path = dir.write("player.moves.ron", PLAYER);
    dir.write("player.moves.ron.meta", PLAYER_META);
    let mut harness = harness(&path);
    harness.run_steps(3);
    // A window on the timeline, by the name its row and ticks give it.
    let _ = harness.get_by_label("hit volumes light_1_hit 7..11");
    let _ = harness.get_by_label("cancels dodge 2..34");
    let _ = harness.get_by_label("cues swing at 7");
    assert_eq!(harness.state().selected_move_name(), Some("light_1"));
    assert!(harness.state().file().is_some_and(|file| !file.is_dirty()));
    // With no cook it can't preview a move with a clip, and says so; the windows are there all the same.
    assert!(harness.state().frame().is_none());
    assert_eq!(MoveSetDocument::parse(PLAYER).expect("parses").moves.len(), 12);
}

/// Presses at `from`, moves to `to` over a few frames and lets go, as a hand does.
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

/// What the window called `label` has for handles: where each is and how wide its ticks are, read off the
/// interface as a hand would. A handle's middle sits a point inside the edge it moves.
fn handles(harness: &Harness<'static, Editor>, label: &str, ticks: u32) -> (Pos2, Pos2, f32) {
    let start = harness.get_by_label(&format!("{label} start")).rect().center();
    let end = harness.get_by_label(&format!("{label} end")).rect().center();
    (start, end, (end.x - start.x + 2.0) / ticks as f32)
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

fn text_of(harness: &Harness<'static, Editor>) -> String {
    harness.state().file().expect("a file").text().to_owned()
}

/// `text` with the only `old` in `move_name`'s entry replaced, as someone in a text editor would: found
/// from the move's name, so a second match later on in the file is no help to a wrong edit.
fn hand_edit(text: &str, move_name: &str, old: &str, new: &str) -> String {
    let start = text.find(&format!("name: \"{move_name}\"")).expect("the move's there");
    let at = start + text[start..].find(old).expect("the text's there");
    format!("{}{new}{}", &text[..at], &text[at + old.len()..])
}

/// A real file, a real drag: the file then says what typing the number would have made it say, in those
/// bytes, and the same thing comes of it. The player's moves name clips, so with no cook all there is to
/// compare is what `check` says; `tests/preview.rs` compiles them over a cook, and the warrior below
/// compiles here.
#[test]
fn a_drag_of_a_windows_end_writes_what_a_hand_edit_would() {
    let dir = TestDir::new("drag_end");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(3);

    let label = "hit volumes light_1_hit 7..11";
    let (_, end, per_tick) = handles(&harness, label, 4);
    drag(&mut harness, end, end + egui::vec2(2.0 * per_tick, 0.0));

    let hand = hand_edit(PLAYER, "light_1", "to: 11,", "to: 13,");
    assert_same(&text_of(&harness), &hand, "the drag's text");
    let (dragged, by_hand) = (parse(&text_of(&harness)), parse(&hand));
    assert_eq!(judge(&dragged, None).issues, judge(&by_hand, None).issues);
    assert!(judge(&dragged, None).issues.is_empty());
    // What the timeline shows is what's now in the file.
    let _ = harness.get_by_label("hit volumes light_1_hit 7..13");
    assert_eq!(harness.state().tick(), 12, "the playhead follows the dragged edge, onto its last live tick");
    assert_same(
        &fs::read_to_string(&path).expect("the file"),
        PLAYER,
        "nothing's saved until it's asked for",
    );
}

/// A move with no clips compiles with no cook, so here the two are compared as what the simulation reads.
#[test]
fn a_drag_on_the_warrior_compiles_to_what_a_hand_edit_compiles_to() {
    let dir = TestDir::new("drag_warrior");
    let path = dir.write("warrior.moves.ron", WARRIOR);
    let mut harness = harness(&path);
    harness.run_steps(3);
    assert!(harness.state_mut().select_move("swipe"));
    harness.run_steps(3);

    let window =
        parse(WARRIOR).moves.iter().find(|mv| mv.name == "swipe").expect("there").hitboxes[0].clone();
    let label = format!("hit volumes {} {}..{}", window.name, window.from, window.to);
    let (start, _, per_tick) = handles(&harness, &label, window.to - window.from);
    drag(&mut harness, start, start - egui::vec2(per_tick, 0.0));

    let hand = hand_edit(
        WARRIOR,
        "swipe",
        &format!("from: {},", window.from),
        &format!("from: {},", window.from - 1),
    );
    assert_same(&text_of(&harness), &hand, "the text");
    let dragged = judge(&parse(&text_of(&harness)), None).set.expect("it compiles");
    let by_hand = judge(&parse(&hand), None).set.expect("it compiles");
    assert_eq!(dragged, by_hand);
    assert_eq!(*dragged.to_cooked().expect("cooks"), *by_hand.to_cooked().expect("cooks"));
    assert_ne!(dragged, judge(&parse(WARRIOR), None).set.expect("it compiles"), "and the edit did something");
}

/// A window's body takes both its ends; a cue moves as one tick; a phase's edge trades ticks with the next.
#[test]
fn a_windows_body_a_cue_and_a_phase_edge_drag_in_ticks() {
    let dir = TestDir::new("drag_more");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(3);
    let (start, end, per_tick) = handles(&harness, "hit volumes light_1_hit 7..11", 4);

    let middle = start.lerp(end, 0.5);
    drag(&mut harness, middle, middle + egui::vec2(per_tick, 0.0));
    let body = hand_edit(
        PLAYER,
        "light_1",
        "from: 7,\n                        to: 11,",
        "from: 8,\n                        to: 12,",
    );
    assert_same(&text_of(&harness), &body, "the body");

    let cue = harness.get_by_label("cues swing at 7").rect().center();
    drag(&mut harness, cue, cue + egui::vec2(3.0 * per_tick, 0.0));
    let moved_cue = hand_edit(&body, "light_1", "(name: \"swing\", at: 7)", "(name: \"swing\", at: 10)");
    assert_same(&text_of(&harness), &moved_cue, "the cue");

    let edge = harness.get_by_label("phases startup end 7").rect().center();
    drag(&mut harness, edge, edge + egui::vec2(per_tick, 0.0));
    let moved_edge = hand_edit(
        &moved_cue,
        "light_1",
        "startup: 7,\n                active: 4,",
        "startup: 8,\n                active: 3,",
    );
    assert_same(&text_of(&harness), &moved_edge, "the phase edge");

    // Three gestures, three steps back, to the very bytes it opened with.
    for _ in 0..3 {
        harness.state_mut().undo();
    }
    assert_same(&text_of(&harness), PLAYER, "the text");
    assert!(!harness.state().file().expect("a file").is_dirty());
}

/// Past what the document allows, a drag still goes where it's dragged: the document's own words say what's
/// wrong and where, on the screen, and nothing else moves until a person says so.
#[test]
fn a_refusal_shows_its_move_and_track_and_keeps_the_state() {
    let dir = TestDir::new("refusal");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(3);
    assert!(harness.state().problems().is_empty());

    let label = "hit volumes light_1_hit 7..11";
    let (_, end, per_tick) = handles(&harness, label, 4);
    // Three ticks past the move's end at 34, which leaves the file with 7..37.
    drag(&mut harness, end, end + egui::vec2(26.0 * per_tick, 0.0));
    let hand = hand_edit(PLAYER, "light_1", "to: 11,", "to: 37,");
    assert_same(&text_of(&harness), &hand, "the edit stands");

    let problems = harness.state().problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].in_move.as_deref(), Some("light_1"));
    assert_eq!(problems[0].track.as_deref(), Some("hitboxes"));
    assert!(!problems[0].from_device);
    // In the document's words, not a copy of them.
    let issues = parse(&hand).check();
    assert_eq!(problems[0].message, issues[0].message);
    assert!(problems[0].message.contains("34"), "{}", problems[0].message);
    // And on the screen, naming them.
    let shown = harness.get_by_label_contains("move `light_1`, track `hitboxes`");
    assert!(shown.rect().width() > 0.0);

    // Selected, on the same move and at the same tick; the file on disk is as it was.
    assert_eq!(harness.state().selected_move_name(), Some("light_1"));
    assert!(harness.state().selected_entry().is_some());
    assert_same(&fs::read_to_string(&path).expect("the file"), PLAYER, "the file on disk");

    // One step back, and it's all gone.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
    assert_same(&text_of(&harness), PLAYER, "the text");
    assert!(harness.state().problems().is_empty());
    assert!(harness.query_by_label_contains("track `hitboxes`").is_none());
}

/// Undo and redo from the keyboard: one drag is one step, however many ticks it crossed.
#[test]
fn undo_and_redo_keys_walk_one_gesture_at_a_time() {
    let dir = TestDir::new("undo");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(3);

    let (_, end, per_tick) = handles(&harness, "hit volumes light_1_hit 7..11", 4);
    drag(&mut harness, end, end + egui::vec2(3.0 * per_tick, 0.0));
    let once = hand_edit(PLAYER, "light_1", "to: 11,", "to: 14,");
    assert_same(&text_of(&harness), &once, "the text");

    let (start, _, per_tick) = handles(&harness, "hit volumes light_1_hit 7..14", 7);
    drag(&mut harness, start, start + egui::vec2(2.0 * per_tick, 0.0));
    let twice = hand_edit(
        &once,
        "light_1",
        "from: 7,\n                        to: 14,",
        "from: 9,\n                        to: 14,",
    );
    assert_same(&text_of(&harness), &twice, "the text");

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
    assert_same(&text_of(&harness), &once, "one undo, the second drag");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
    assert_same(&text_of(&harness), PLAYER, "two, the first");
    assert!(!harness.state().file().expect("a file").is_dirty());
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    harness.run_steps(2);
    assert_same(&text_of(&harness), &once, "redo");
    let _ = harness.get_by_label("hit volumes light_1_hit 7..14");
}

/// A drag whose button went away (the window lost focus under it) is over: the next edit is its own step.
#[test]
fn a_drag_that_loses_its_button_ends() {
    let dir = TestDir::new("lost_button");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(3);
    let (_, end, per_tick) = handles(&harness, "hit volumes light_1_hit 7..11", 4);
    harness.hover_at(end);
    harness.run_steps(2);
    harness.drag_at(end);
    harness.step();
    harness.hover_at(end + egui::vec2(per_tick, 0.0));
    harness.step();
    assert!(harness.state().file().expect("a file").in_gesture());
    // What the window does when focus goes.
    harness.ctx.input_mut_for(egui::ViewportId::ROOT, |input| input.pointer = egui::PointerState::default());
    harness.ctx.stop_dragging();
    harness.run_steps(2);
    assert!(!harness.state().file().expect("a file").in_gesture());
    let moved = text_of(&harness);
    assert_ne!(moved, PLAYER);
    harness.state_mut().undo();
    assert_same(&text_of(&harness), PLAYER, "the text");
}

/// The timeline's ruler and the keys go tick by tick, and the simulation's frame comes with them.
#[test]
fn the_ruler_and_the_keys_scrub_tick_by_tick() {
    let dir = TestDir::new("scrub");
    let path = dir.write("warrior.moves.ron", WARRIOR);
    let mut harness = harness(&path);
    harness.run_steps(3);
    assert!(harness.state_mut().select_move("swipe"));
    harness.run_steps(3);
    let ticks = parse(WARRIOR).moves.iter().find(|mv| mv.name == "swipe").expect("there").ticks();

    // The widget starts at the row titles, so ticks are read off a window's handles.
    let window =
        parse(WARRIOR).moves.iter().find(|mv| mv.name == "swipe").expect("there").hitboxes[0].clone();
    let label = format!("hit volumes {} {}..{}", window.name, window.from, window.to);
    let (start, _, per_tick) = handles(&harness, &label, window.to - window.from);
    let tick_zero = start.x - 1.0 - window.from as f32 * per_tick;
    let top = harness.get_by_label("timeline ruler").rect().top();
    for tick in [1, 13, 20, ticks - 1] {
        let at = Pos2::new(tick_zero + (tick as f32 + 0.5) * per_tick, top + 6.0);
        harness.hover_at(at);
        harness.run_steps(2);
        harness.drag_at(at);
        harness.step();
        harness.drop_at(at);
        harness.run_steps(2);
        assert_eq!(harness.state().tick(), tick, "a click on the ruler");
        assert_eq!(harness.state().frame().expect("a frame").tick, tick, "the simulation's frame");
    }

    harness.key_press(Key::Home);
    harness.run_steps(2);
    assert_eq!(harness.state().tick(), 0);
    harness.key_press(Key::Period);
    harness.key_press(Key::ArrowRight);
    harness.run_steps(3);
    assert_eq!(harness.state().tick(), 2);
    harness.key_press(Key::Comma);
    harness.run_steps(2);
    assert_eq!(harness.state().tick(), 1);
    harness.key_press(Key::End);
    harness.run_steps(2);
    assert_eq!(harness.state().tick(), ticks - 1);
}

/// `[` and `]` put a window's ends at the playhead.
#[test]
fn brackets_set_a_windows_ends_at_the_playhead() {
    let dir = TestDir::new("brackets");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(3);
    harness.get_by_label("hit volumes light_1_hit 7..11").click();
    harness.run_steps(3);
    harness.state_mut().set_tick(5);
    harness.key_press(Key::OpenBracket);
    harness.run_steps(2);
    let moved = hand_edit(
        PLAYER,
        "light_1",
        "from: 7,\n                        to: 11,",
        "from: 5,\n                        to: 11,",
    );
    assert_same(&text_of(&harness), &moved, "the start, at the playhead");
    harness.state_mut().set_tick(8);
    harness.key_press(Key::CloseBracket);
    harness.run_steps(2);
    // A window runs up to but not including its `to`, so the playhead's own tick stays in it.
    let moved = hand_edit(&moved, "light_1", "to: 11,", "to: 9,");
    assert_same(&text_of(&harness), &moved, "the end, just past it");
}

/// Added through the interface and taken away again, to the same bytes.
#[test]
fn an_entry_added_and_deleted_leaves_the_file_as_it_was() {
    let dir = TestDir::new("add");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(3);
    harness.state_mut().set_tick(20);
    harness.run_steps(1);
    let before = parse(PLAYER).moves[0].cues.len();
    harness.get_by_label("add to cues").click();
    harness.run_steps(3);
    let after = parse(&text_of(&harness));
    assert_eq!(after.moves[0].cues.len(), before + 1);
    assert!(after.moves[0].cues.iter().any(|cue| cue.at == 20), "at the playhead");
    assert!(harness.state().problems().is_empty(), "{:?}", harness.state().problems());
    assert!(harness.state().selected_entry().is_some(), "and ready to name");

    harness.key_press(Key::Delete);
    harness.run_steps(3);
    assert_same(&text_of(&harness), PLAYER, "the text");
}

/// Play from frame N: it runs on the interface's own clock, and comes back to N at the end.
#[test]
fn play_from_frame_n_plays_and_returns_to_n() {
    let dir = TestDir::new("play");
    let path = dir.write("warrior.moves.ron", WARRIOR);
    let options = Options {
        file: Some(path),
        content: Some(dir.0.join("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
    };
    let mut harness = Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), Editor::new(options));
    harness.run_steps(3);
    assert!(harness.state_mut().select_move("swipe"));
    let ticks = parse(WARRIOR).moves.iter().find(|mv| mv.name == "swipe").expect("there").ticks();
    harness.state_mut().set_tick(10);
    harness.run_steps(2);

    harness.get_by_label("Play from frame 10").click();
    harness.run_steps(3);
    assert!(harness.state().is_playing());
    harness.run_steps(20);
    let during = harness.state().tick();
    assert!((20..=40).contains(&during), "tick {during}: about 60 ticks a second from 10");
    // Past the end, and any hitstop the run has in it.
    harness.run_steps(ticks as usize + 240);
    assert!(!harness.state().is_playing());
    assert_eq!(harness.state().tick(), 10, "back where it started");

    // Slower, and stopped by hand: it stays where it stopped.
    harness.state_mut().set_speed(0.25);
    harness.key_press(Key::Space);
    harness.run_steps(25);
    assert!(harness.state().is_playing());
    let slow = harness.state().tick();
    assert!((12..=20).contains(&slow), "tick {slow}: a quarter of the speed");
    harness.key_press(Key::Space);
    harness.run_steps(2);
    assert!(!harness.state().is_playing());
    assert!(harness.state().tick() >= slow);
}

/// Edits aren't lost to a closed window: it asks, and each answer does what it says.
#[test]
fn closing_with_unsaved_edits_asks_first() {
    let dir = TestDir::new("quit");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(3);
    harness.state_mut().request_quit();
    assert!(harness.state().may_quit(), "nothing to lose");

    let dir = TestDir::new("quit_dirty");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = self::harness(&path);
    harness.run_steps(3);
    let (_, end, per_tick) = handles(&harness, "hit volumes light_1_hit 7..11", 4);
    drag(&mut harness, end, end + egui::vec2(per_tick, 0.0));
    harness.state_mut().request_quit();
    assert!(!harness.state().may_quit());
    harness.run_steps(2);
    harness.get_by_label("Keep editing").click();
    harness.run_steps(2);
    assert!(!harness.state().may_quit());
    assert!(harness.query_by_label("Keep editing").is_none());

    harness.state_mut().request_quit();
    harness.run_steps(2);
    harness.get_by_label("Save and quit").click();
    harness.run_steps(2);
    assert!(harness.state().may_quit());
    assert_eq!(fs::read_to_string(&path).expect("the file"), text_of(&harness));
}

/// Picking a document in the list with edits here asks first, as closing does, and each answer does what
/// it says. The list holds the open file too, and picking that would read it over the edits.
#[test]
fn opening_a_document_with_unsaved_edits_asks_first() {
    let dir = TestDir::new("open_dirty");
    dir.write("Cargo.toml", "");
    fs::create_dir_all(dir.0.join("content")).expect("a folder");
    let path = dir.write("content/player.moves.ron", PLAYER);
    let other = dir.write("content/warrior.moves.ron", WARRIOR);
    let name_of = |harness: &Harness<'static, Editor>| harness.state().file().expect("a file").file_name();
    let mut harness = self::harness(&path);
    harness.run_steps(3);
    let (_, end, per_tick) = handles(&harness, "hit volumes light_1_hit 7..11", 4);
    drag(&mut harness, end, end + egui::vec2(per_tick, 0.0));
    let edited = text_of(&harness);
    assert_ne!(edited, PLAYER);

    for picked in ["warrior.moves.ron", "player.moves.ron"] {
        harness.get_by_value("player.moves.ron *").click();
        harness.run_steps(2);
        harness.get_by_label(picked).click();
        harness.run_steps(2);
        assert_eq!(text_of(&harness), edited, "picking {picked} lost the edits");
        harness.get_by_label("Keep editing").click();
        harness.run_steps(2);
        assert_eq!(name_of(&harness), "player.moves.ron");
        assert_eq!(text_of(&harness), edited, "keep editing kept them");
        assert!(harness.query_by_label("Keep editing").is_none());
    }

    harness.get_by_value("player.moves.ron *").click();
    harness.run_steps(2);
    harness.get_by_label("warrior.moves.ron").click();
    harness.run_steps(2);
    harness.get_by_label("Save and open").click();
    harness.run_steps(2);
    assert_eq!(name_of(&harness), "warrior.moves.ron");
    assert_eq!(fs::read_to_string(&path).expect("the file"), edited, "saved");

    // With nothing to lose it opens at once, and the answer that throws edits away does so.
    harness.state_mut().open(&path);
    harness.run_steps(2);
    let (_, end, per_tick) = handles(&harness, "hit volumes light_1_hit 7..12", 5);
    drag(&mut harness, end, end + egui::vec2(per_tick, 0.0));
    assert_ne!(text_of(&harness), edited);
    harness.get_by_value("player.moves.ron *").click();
    harness.run_steps(2);
    harness.get_by_label("warrior.moves.ron").click();
    harness.run_steps(2);
    harness.get_by_label("Open without saving").click();
    harness.run_steps(2);
    assert_eq!(name_of(&harness), "warrior.moves.ron");
    assert_eq!(text_of(&harness), fs::read_to_string(&other).expect("the file"));
    assert_eq!(fs::read_to_string(&path).expect("the file"), edited, "not saved");
    harness.get_by_value("warrior.moves.ron").click();
    harness.run_steps(2);
    harness.get_by_label("player.moves.ron").click();
    harness.run_steps(2);
    assert_eq!(name_of(&harness), "player.moves.ron", "nothing to lose, so nothing to ask");
}

/// `Cmd+S` saves and pushes in one gesture, against a device of the test's own speaking live link's real
/// protocol over a real socket: what it replaced is shown, what it refused is shown by move and track with
/// the edit kept, and the attempts on the device show up too. `tests/sandbox_push.rs` does the same with a
/// real fight.
#[test]
fn save_and_push_shows_what_the_device_says() {
    let device = FakeDevice::start();
    let dir = TestDir::new("push");
    let path = dir.write("slice.moves.ron", SLICE);
    let options = Options {
        file: Some(path.clone()),
        content: Some(dir.0.join("no cook here")),
        link: Some(device.address),
    };
    let mut harness = Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .with_step_dt(0.05)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), Editor::new(options));
    harness.run_steps(3);
    until(&mut harness, "the device's status", |editor| editor.link().device().is_some());
    until(&mut harness, "its attempts", |editor| !editor.link().attempts().is_empty());
    let _ = harness.get_by_label_contains("actor 0: light_1 rev 0");

    // A good edit, saved and pushed by the key a person uses.
    let (_, end, per_tick) = handles(&harness, "hit volumes light_1_hit 5..8", 3);
    drag(&mut harness, end, end + egui::vec2(per_tick, 0.0));
    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run_steps(2);
    until(&mut harness, "the reply", |editor| {
        !editor.link().is_waiting() && editor.link().last_push().is_some()
    });
    assert_eq!(fs::read_to_string(&path).expect("saved"), text_of(&harness));
    let Some(Pushed::Took { owner, .. }) = harness.state().link().last_push() else {
        panic!("{:?}", harness.state().link().last_push())
    };
    assert!(owner.contains("revision 1"), "{owner}");
    let shown = owner.clone();
    let _ = harness.get_by_label_contains(&shown);
    assert_eq!(device.documents(), [text_of(&harness)], "the file's own bytes reached the device");

    // The device says no to something the editor sees no fault in, by move and track.
    device.refuse_with("slice.moves.ron: move `light_1`, track `hitboxes`: the fight here can't take this");
    let before = text_of(&harness);
    let (_, end, per_tick) = handles(&harness, "hit volumes light_1_hit 5..9", 4);
    drag(&mut harness, end, end + egui::vec2(per_tick, 0.0));
    let edited = text_of(&harness);
    let tick = harness.state().tick();
    assert_ne!(before, edited);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run_steps(2);
    until(&mut harness, "the refusal", |editor| {
        matches!(editor.link().last_push(), Some(Pushed::Refused(_)))
    });
    let problems = harness.state().problems();
    let refused = problems.iter().find(|problem| problem.from_device).expect("the device's problem");
    assert_eq!(refused.in_move.as_deref(), Some("light_1"));
    assert_eq!(refused.track.as_deref(), Some("hitboxes"));
    assert_eq!(refused.message, "the fight here can't take this");
    let _ = harness.get_by_label_contains("device refused: move `light_1`, track `hitboxes`");
    assert_same(&text_of(&harness), &edited, "the edit stands");
    assert_eq!(harness.state().tick(), tick);
    assert_eq!(harness.state().selected_move_name(), Some("light_1"));

    // The next change to the text takes the refusal away: it was about what's now gone.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
    assert!(harness.state().problems().iter().all(|problem| !problem.from_device));
}

/// A push with no device says so, and the file is saved all the same.
#[test]
fn a_push_with_no_device_says_so() {
    let dir = TestDir::new("no_device");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(3);
    let (_, end, per_tick) = handles(&harness, "hit volumes light_1_hit 7..11", 4);
    drag(&mut harness, end, end + egui::vec2(per_tick, 0.0));
    harness.state_mut().save_and_push();
    harness.run_steps(2);
    assert_same(&fs::read_to_string(&path).expect("saved"), &text_of(&harness), "the file");
    assert!(!harness.state().file().expect("a file").is_dirty());
    until(&mut harness, "the failure", |editor| matches!(editor.link().last_push(), Some(Pushed::Failed(_))));
}

/// Runs the interface until `done`, with a real clock's patience: the link's replies come from a thread.
fn until(harness: &mut Harness<'static, Editor>, what: &str, done: impl Fn(&Editor) -> bool) {
    let started = Stopwatch::start();
    while !done(harness.state()) {
        assert!(started.elapsed() < Duration::from_secs(30), "never saw {what}");
        harness.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(1);
}

/// A device of the test's own: real live link and a real socket on a private port, and a thread that
/// answers as a fight's app does, without the fight.
struct FakeDevice {
    address: SocketAddr,
    refusal: Arc<Mutex<Option<String>>>,
    documents: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeDevice {
    fn start() -> Self {
        let free = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = free.local_addr().expect("its address");
        drop(free);
        let link =
            LiveLink::start(LinkMode::Listen(address), Arc::new(LivePacks::new(MemoryPackSource::new())))
                .expect("a listener");
        let refusal = Arc::new(Mutex::new(None::<String>));
        let documents = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (refusal, documents, stop) =
                (Arc::clone(&refusal), Arc::clone(&documents), Arc::clone(&stop));
            move || {
                let mut revision = 0;
                while !stop.load(Ordering::Relaxed) {
                    match link.poll() {
                        Some(Push::Status { reply }) => {
                            let attempt = MoveAttempt {
                                name: "light_1".to_owned(),
                                revision,
                                newest_revision: revision,
                                ..MoveAttempt::default()
                            };
                            reply.reply(ReplyBody::Status(DeviceStatus {
                                platform: "here".to_owned(),
                                cook: "desktop".to_owned(),
                                attempts: vec![attempt],
                                ..DeviceStatus::default()
                            }));
                        }
                        Some(Push::Document { text, reply, .. }) => {
                            documents.lock().expect("the list").push(text);
                            if let Some(why) = refusal.lock().expect("the refusal").clone() {
                                reply.refuse(why);
                            } else {
                                revision += 1;
                                let owner = format!("the fight's moves, at revision {revision}");
                                reply.reply(ReplyBody::DocumentApplied { owner });
                            }
                        }
                        Some(_) => {}
                        None => std::thread::sleep(Duration::from_millis(2)),
                    }
                }
            }
        });
        Self { address, refusal, documents, stop, thread: Some(thread) }
    }

    fn refuse_with(&self, why: &str) {
        *self.refusal.lock().expect("the refusal") = Some(why.to_owned());
    }

    fn documents(&self) -> Vec<String> {
        self.documents.lock().expect("the list").clone()
    }
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
