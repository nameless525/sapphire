//! The editor's interface, driven as a person drives it with no window: `egui_kittest` finds its widgets
//! by what they're called and sends real pointer and key events, and everything goes through
//! [`Editor::ui`], as the window's frames do. Edits land in temporary copies of the real documents.

use std::collections::VecDeque;
use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{Key, Modifiers, Pos2, Role};
use egui_kittest::kittest::{By, Queryable};
use egui_kittest::{Harness, Node};
use sapphire_assets::MemoryPackSource;
use sapphire_editor::capture::{self, Textures};
use sapphire_editor::edits::{EntryRef, Track};
use sapphire_editor::file::judge;
use sapphire_editor::link::Pushed;
use sapphire_editor::timeline::Action;
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
    let mut harness = harness(&path);
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

/// What the preview says with no cook, `cargo xtask content`, works: the first cook reaches an editor
/// already open, wherever it found none, and a pack that doesn't read isn't the last word.
#[test]
fn a_first_cook_reaches_an_editor_already_open() {
    let empty_pack = sapphire_assets::pack::write_pack(Vec::new()).expect("an empty pack");

    let dir = TestDir::new("late_cook_named");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    harness.run_steps(6);
    assert!(!harness.state().has_cook());
    let cook = path.with_file_name("no cook here");
    fs::create_dir_all(&cook).expect("the cook's folder");
    fs::write(cook.join("shared.spack"), &empty_pack[..4]).expect("half a pack");
    harness.run_steps(6);
    assert!(!harness.state().has_cook());
    let half_written =
        fs::metadata(cook.join("shared.spack")).and_then(|file| file.modified()).expect("a time");
    fs::write(cook.join("shared.spack"), &empty_pack).expect("a pack");
    // A file's time can be as coarse as a second, and the same time isn't a new cook.
    fs::File::options()
        .write(true)
        .open(cook.join("shared.spack"))
        .and_then(|file| file.set_modified(half_written + Duration::from_secs(5)))
        .expect("a new time");
    until(&mut harness, "the cook", Editor::has_cook);

    let dir = TestDir::new("late_cook_found");
    let path = dir.write("slice.moves.ron", SLICE);
    let options =
        Options { file: Some(path), content: None, link: Some("127.0.0.1:9".parse().expect("an address")) };
    let mut harness = Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), Editor::new(options));
    harness.run_steps(6);
    assert!(!harness.state().has_cook());
    let cook = dir.0.join("target/content/desktop");
    fs::create_dir_all(&cook).expect("the cook's folder");
    fs::write(cook.join("shared.spack"), &empty_pack).expect("a pack");
    until(&mut harness, "the cook", Editor::has_cook);
}

/// A document can claim any length, and it's still the person's to open and fix: the timeline draws what
/// shows, so a slip of a few digits costs a frame what a real move costs, not a shape per tick claimed.
#[test]
fn a_move_longer_than_any_can_be_draws_only_what_shows() {
    let usual = shapes_drawn(SLICE);
    for recovery in ["400000", "4000000000"] {
        let text = SLICE.replacen("recovery: 18", &format!("recovery: {recovery}"), 1);
        assert_ne!(text, SLICE);
        let shapes = shapes_drawn(&text);
        assert!(shapes < usual * 2 + 1000, "recovery {recovery} drew {shapes} shapes, against {usual}");
    }
}

/// The largest number each kind of value can say draws, in the builds that check their sums: a panic here
/// would take any edits with it.
#[test]
fn the_largest_numbers_a_document_can_say_draw() {
    for (from, to) in [
        ("startup: 5", "startup: 4294967295"),
        ("active: 3", "active: 4294967295"),
        ("recovery: 18", "recovery: 4294967295"),
        ("from: 5", "from: 4294967295"),
        ("to: 8", "to: 4294967295"),
        ("\"swing_windup\", at: 0", "\"swing_windup\", at: 4294967295"),
    ] {
        assert!(SLICE.contains(from), "{from}");
        shapes_drawn(&SLICE.replacen(from, to, 1));
    }
}

/// What the interface draws of `text`, on `light_1`, with the playhead as far on as it goes.
fn shapes_drawn(text: &str) -> usize {
    let dir = TestDir::new("huge");
    let options = Options {
        file: Some(dir.write("slice.moves.ron", text)),
        content: Some(dir.0.join("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
    };
    let mut editor = Editor::new(options);
    assert!(editor.select_move("light_1"));
    editor.set_tick(u32::MAX);
    let ctx = egui::Context::default();
    let mut shapes = 0;
    for pass in 0..2 {
        let input = capture::raw_input(1400, 900, f64::from(pass) * 0.1);
        let mut output = ctx.run_ui(input, |ui| editor.ui(ui));
        output.textures_delta.clear();
        shapes = output.shapes.len();
    }
    shapes
}

/// `sapphire_editor` with no file, as its usage allows and `cargo xtask editor` says, still has something
/// to open: the list holds what's under the repository it was started in.
#[test]
fn with_no_file_named_the_list_offers_the_repositorys_documents() {
    let dir = TestDir::new("no_file");
    let options = Options {
        file: None,
        content: Some(dir.0.join("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
    };
    let mut harness = Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), Editor::new(options));
    harness.run_steps(2);
    harness.get_by_value("no file").click();
    harness.run_steps(2);
    harness.get_by_label("warrior.moves.ron").click();
    harness.run_steps(3);
    assert_eq!(harness.state().file().expect("a file").file_name(), "warrior.moves.ron");

    // A name that's wrong leaves the same way out, with the notice saying why.
    let options = Options {
        file: Some(dir.0.join("no such.moves.ron")),
        content: Some(dir.0.join("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
    };
    let mut harness = Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), Editor::new(options));
    harness.run_steps(2);
    assert!(harness.state().notice().is_some());
    harness.get_by_value("no file").click();
    harness.run_steps(2);
    harness.get_by_label("warrior.moves.ron").click();
    harness.run_steps(3);
    assert_eq!(harness.state().file().expect("a file").file_name(), "warrior.moves.ron");
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

/// The cue the typing tests type over, and what they type.
const CUE_NAME: &str = "\"swing_windup\"";
const TYPED_NAME: &str = "\"typed_name\"";

fn slice_with_typed_name() -> String {
    SLICE.replacen(CUE_NAME, TYPED_NAME, 1)
}

/// The text field showing `value`, rather than the run of text inside it that shows it too.
fn field<'a>(harness: &'a Harness<'static, Editor>, value: &'a str) -> Node<'a> {
    harness.get(By::new().role(Role::TextInput).value(value))
}

/// Selects the slice's first cue and types `typed` over its name, with the field left focused, as a person
/// does before reaching for Cmd+S or the window's Close.
fn type_over_the_cue_name(harness: &mut Harness<'static, Editor>, typed: &str) {
    harness.run_steps(3);
    let cue = EntryRef { mv: 0, track: Track::Cues, index: 0 };
    harness.state_mut().apply(Action::Select(Some(cue)));
    harness.run_steps(3);
    field(harness, CUE_NAME).focus();
    harness.run_steps(1);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.run_steps(1);
    field(harness, CUE_NAME).type_text(typed);
    harness.run_steps(2);
    assert!(field(harness, typed).is_focused(), "the field holds what's typed, and has the keyboard");
    assert_same(&text_of(harness), SLICE, "half a name isn't an edit yet");
    assert!(harness.state().is_dirty(), "and what's on screen isn't in the file");
}

/// Clicks a move's row in the list, whose click lands before the inspector draws in a pass.
fn click_the_slices_move(harness: &Harness<'static, Editor>, index: usize) {
    let document = parse(SLICE);
    let mv = &document.moves[index];
    harness.get_by_label(&format!("{}  ({})", mv.name, mv.ticks())).click();
}

fn on_device(path: &Path, device: &FakeDevice) -> Harness<'static, Editor> {
    let options = Options {
        file: Some(path.to_owned()),
        content: Some(path.with_file_name("no cook here")),
        link: Some(device.address),
    };
    Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .with_step_dt(0.05)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), Editor::new(options))
}

/// Escape takes back what's being typed, as it does in a text field anywhere: nothing goes into the text,
/// so nothing's left to save or undo.
#[test]
fn escape_takes_back_what_is_being_typed() {
    let dir = TestDir::new("typed_escape");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    type_over_the_cue_name(&mut harness, TYPED_NAME);

    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert_same(&text_of(&harness), SLICE, "the text");
    assert!(!harness.state().is_dirty());
    let _ = field(&harness, CUE_NAME);
    assert_eq!(harness.state().file().expect("a file").undo_label(), None, "there's no step to take back");

    // What the text refused goes the same way, and Enter still puts in what it takes.
    type_over_the_cue_name(&mut harness, "\"half");
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    let _ = harness.get_by_label_contains("nothing else can until");
    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("nothing else can until").is_none());
    assert!(!harness.state().is_dirty());
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert_same(&text_of(&harness), &slice_with_typed_name(), "the text");
}

/// Cmd+S with a field still focused saves and pushes what's on the screen, and the field keeps the keyboard.
#[test]
fn saving_by_key_takes_what_is_being_typed() {
    let device = FakeDevice::start();
    let dir = TestDir::new("typed_key");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = on_device(&path, &device);
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    let _ = harness.get_by_value("slice.moves.ron *");

    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run_steps(2);
    let typed = slice_with_typed_name();
    assert_same(&fs::read_to_string(&path).expect("the file"), &typed, "the file");
    assert_same(&text_of(&harness), &typed, "the text");
    assert!(!harness.state().is_dirty());
    let _ = harness.get_by_value("slice.moves.ron");
    let _ = harness.get_by_label("cues typed_name at 0");
    assert!(field(&harness, TYPED_NAME).is_focused(), "typing can go on");
    until(&mut harness, "the reply", |editor| editor.link().last_push().is_some());
    assert_eq!(device.documents(), [typed], "the bytes saved are the bytes pushed");
}

/// The Save button runs before the inspector in a pass, and a click takes focus only as it lands, so the
/// save comes before the field's own letting go.
#[test]
fn saving_by_button_takes_what_is_being_typed() {
    for same_pass in [false, true] {
        let device = FakeDevice::start();
        let dir = TestDir::new("typed_button");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut harness = on_device(&path, &device);
        type_over_the_cue_name(&mut harness, TYPED_NAME);

        if same_pass {
            harness.get_by_label("Save and push").click();
        } else {
            let save = harness.get_by_label("Save and push").rect().center();
            harness.hover_at(save);
            harness.run_steps(2);
            harness.drag_at(save);
            harness.step();
            harness.drop_at(save);
        }
        harness.run_steps(3);
        let typed = slice_with_typed_name();
        assert_same(&fs::read_to_string(&path).expect("the file"), &typed, "the file");
        assert!(!harness.state().is_dirty());
        until(&mut harness, "the reply", |editor| editor.link().last_push().is_some());
        assert_eq!(device.documents(), [typed], "same pass: {same_pass}");
    }
}

/// What's typed counts as an edit to a closed window, and each answer does what it says.
#[test]
fn closing_while_typing_asks_first() {
    let dir = TestDir::new("typed_quit");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    type_over_the_cue_name(&mut harness, TYPED_NAME);

    harness.state_mut().request_quit();
    assert!(!harness.state().may_quit(), "it would lose what's on the screen");
    harness.run_steps(2);
    harness.get_by_label("Keep editing").click();
    harness.run_steps(2);
    assert!(!harness.state().may_quit());
    assert_same(&fs::read_to_string(&path).expect("the file"), SLICE, "nothing's saved by asking");

    harness.state_mut().request_quit();
    harness.run_steps(2);
    harness.get_by_label("Save and quit").click();
    harness.run_steps(2);
    assert!(harness.state().may_quit());
    assert_same(&fs::read_to_string(&path).expect("the file"), &slice_with_typed_name(), "saved");

    let dir = TestDir::new("typed_quit_unsaved");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = self::harness(&path);
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    harness.state_mut().request_quit();
    harness.run_steps(2);
    harness.get_by_label("Quit without saving").click();
    harness.run_steps(2);
    assert!(harness.state().may_quit());
    assert_same(&fs::read_to_string(&path).expect("the file"), SLICE, "not saved");
}

/// A window with no GPU can't show its question, so closing it saves rather than asks, typing included. If
/// the file won't have them it stays open once, and a second close goes.
#[test]
fn closing_a_window_that_cant_draw_saves_instead_of_asking() {
    let dir = TestDir::new("unseen_clean");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    harness.run_steps(3);
    harness.state_mut().request_quit_unseen();
    assert!(harness.state().may_quit(), "nothing to lose");

    let dir = TestDir::new("unseen_edits");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = self::harness(&path);
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    harness.state_mut().request_quit_unseen();
    assert!(harness.state().may_quit());
    assert_same(&fs::read_to_string(&path).expect("the file"), &slice_with_typed_name(), "saved");

    let dir = TestDir::new("unseen_refused");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = self::harness(&path);
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    let elsewhere = SLICE.replacen("startup: 5", "startup: 6", 1);
    fs::write(&path, &elsewhere).expect("someone else's save");
    harness.state_mut().request_quit_unseen();
    assert!(!harness.state().may_quit(), "it would lose someone's save, or these edits");
    assert_same(&fs::read_to_string(&path).expect("the file"), &elsewhere, "the file");
    harness.run_steps(1);
    harness.state_mut().request_quit_unseen();
    assert!(harness.state().may_quit(), "the second close is an answer");
    assert_same(&fs::read_to_string(&path).expect("the file"), &elsewhere, "the file");
}

/// SDL follows the close of its last window with a quit, in the same pump, so one click on a window that
/// can't draw reaches the editor as two requests with no pass between them. That's one close: the edits
/// the file wouldn't have must still be here, and the pass after it, a close is a second one.
#[test]
fn one_close_that_sdl_says_twice_isnt_a_second_close() {
    let dir = TestDir::new("unseen_twice");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    let elsewhere = SLICE.replacen("startup: 5", "startup: 6", 1);
    fs::write(&path, &elsewhere).expect("someone else's save");

    harness.state_mut().request_quit_unseen();
    harness.state_mut().request_quit_unseen();
    assert!(!harness.state().may_quit(), "one click lost the edits");
    assert!(harness.state().is_dirty());

    harness.run_steps(1);
    harness.state_mut().request_quit_unseen();
    harness.state_mut().request_quit_unseen();
    assert!(harness.state().may_quit(), "the next click is the second close");
    assert_same(&fs::read_to_string(&path).expect("the file"), &elsewhere, "the file");
}

/// So it does to a picked document, and what's typed doesn't follow into the next one.
#[test]
fn opening_another_document_while_typing_asks_first() {
    let dir = TestDir::new("typed_open");
    dir.write("Cargo.toml", "");
    fs::create_dir_all(dir.0.join("content")).expect("a folder");
    let path = dir.write("content/slice.moves.ron", SLICE);
    dir.write("content/warrior.moves.ron", WARRIOR);
    let mut harness = harness(&path);
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    let pick_the_warrior = |harness: &mut Harness<'static, Editor>| {
        harness.get_by_value("slice.moves.ron *").click();
        harness.run_steps(2);
        harness.get_by_label("warrior.moves.ron").click();
        harness.run_steps(2);
    };

    pick_the_warrior(&mut harness);
    assert_eq!(harness.state().file().expect("a file").file_name(), "slice.moves.ron", "it asks");
    harness.get_by_label("Save and open").click();
    harness.run_steps(2);
    assert_eq!(harness.state().file().expect("a file").file_name(), "warrior.moves.ron");
    assert_same(&fs::read_to_string(&path).expect("the file"), &slice_with_typed_name(), "saved");
    assert!(!harness.state().is_dirty());

    fs::write(&path, SLICE).expect("the file again");
    harness.state_mut().open(&path);
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    pick_the_warrior(&mut harness);
    harness.get_by_label("Open without saving").click();
    harness.run_steps(2);
    assert_eq!(harness.state().file().expect("a file").file_name(), "warrior.moves.ron");
    assert_same(&fs::read_to_string(&path).expect("the file"), SLICE, "not saved");
    assert!(!harness.state().is_dirty(), "and it didn't come along");
}

/// Text the document couldn't read stops a save rather than being saved or dropped: it stays where it was
/// typed, the top bar says why, and fixing it or dropping it are both there.
#[test]
fn text_the_document_cant_read_stays_where_it_was_typed() {
    let half = "\"half";
    let dir = TestDir::new("typed_refused");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    type_over_the_cue_name(&mut harness, half);

    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run_steps(2);
    assert_same(&fs::read_to_string(&path).expect("the file"), SLICE, "nothing was saved");
    assert!(harness.state().notice().is_some_and(|notice| notice.trouble), "and it says so");
    let _ = field(&harness, half);
    let _ = harness.get_by_label_contains("nothing else can until");
    assert!(harness.state().is_dirty());

    // Closing asks, and saving from there can't go through either.
    harness.state_mut().request_quit();
    assert!(!harness.state().may_quit());
    harness.run_steps(2);
    harness.get_by_label("Save and quit").click();
    harness.run_steps(2);
    assert!(!harness.state().may_quit());
    let _ = field(&harness, half);

    // Nothing else goes in meanwhile: what's typed is about the text as it stands.
    harness.state_mut().nudge_selected(1);
    harness.run_steps(2);
    assert_same(&text_of(&harness), SLICE, "the nudge waits");

    // Leaving the move doesn't lose it, and the top bar goes back to it.
    click_the_slices_move(&harness, 1);
    harness.run_steps(3);
    assert!(harness.query(By::new().role(Role::TextInput).value(half)).is_none());
    assert!(harness.state().is_dirty());
    harness.get_by_label("Show it").click();
    harness.run_steps(3);
    let _ = field(&harness, half);
    assert_eq!(harness.state().selected_move_name(), Some("light_1"));

    // Fixed, it goes in as soon as it's left.
    field(&harness, half).focus();
    harness.run_steps(1);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.run_steps(1);
    field(&harness, half).type_text(TYPED_NAME);
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert_same(&text_of(&harness), &slice_with_typed_name(), "the text");
    assert!(harness.query_by_label_contains("nothing else can until").is_none());
    harness.state_mut().undo();
    harness.run_steps(2);
    assert_same(&text_of(&harness), SLICE, "one step back");

    // Dropped, it's gone, and there's nothing left to save.
    type_over_the_cue_name(&mut harness, half);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    harness.get_by_label("Drop it").click();
    harness.run_steps(3);
    assert!(!harness.state().is_dirty());
    let _ = field(&harness, CUE_NAME);
    assert_same(&text_of(&harness), SLICE, "the text");
}

/// A field's whole session is one step of undo, whatever it took to type and however it ended, and undo
/// reaches what's still being typed.
#[test]
fn a_fields_typing_is_one_step_of_undo() {
    let dir = TestDir::new("typed_undo");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    type_over_the_cue_name(&mut harness, "\"typed_");
    field(&harness, "\"typed_").type_text("name\"");
    harness.run_steps(2);
    let _ = field(&harness, TYPED_NAME);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    let typed = slice_with_typed_name();
    assert_same(&text_of(&harness), &typed, "taken when the field lets go");
    assert_eq!(harness.state().file().expect("a file").undo_label(), Some("edit name"));

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
    assert_same(&text_of(&harness), SLICE, "one step");
    assert!(!harness.state().is_dirty());
    let _ = field(&harness, CUE_NAME);
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    harness.run_steps(2);
    assert_same(&text_of(&harness), &typed, "and redo brings it back");
    harness.state_mut().undo();
    harness.run_steps(2);

    // With the field still focused, undo takes the typing.
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
    assert_same(&text_of(&harness), SLICE, "the text");
    assert!(!harness.state().is_dirty());
    let _ = field(&harness, CUE_NAME);
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    harness.run_steps(2);
    assert_same(&text_of(&harness), &typed, "and redo brings that back too");
}

/// While a value is being typed the text holds still, so it's still about the entry it was typed over: a
/// change on disk waits, and a save sees it.
#[test]
fn a_change_on_disk_doesnt_land_under_what_is_being_typed() {
    let dir = TestDir::new("typed_disk");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    type_over_the_cue_name(&mut harness, TYPED_NAME);

    let elsewhere = SLICE.replacen("startup: 5", "startup: 6", 1);
    fs::write(&path, &elsewhere).expect("someone else's save");
    // Past the second between looks at the file.
    harness.run_steps(8);
    assert_same(&text_of(&harness), SLICE, "it isn't taken over what's on the screen");
    let _ = field(&harness, TYPED_NAME);

    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run_steps(2);
    assert_same(&fs::read_to_string(&path).expect("the file"), &elsewhere, "nothing's lost");
    assert_same(&text_of(&harness), &slice_with_typed_name(), "and neither is what was typed");
    assert!(harness.state().file().expect("a file").conflict().is_some());
}

/// A file that stopped being text can't be taken and isn't saved over, and the top bar has the way past.
#[test]
fn a_file_that_stopped_being_text_needs_saying_so_before_a_save() {
    let not_text = [0xff, 0xfe, 0x42];
    let dir = TestDir::new("not_text");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    harness.run_steps(3);
    let (_, end, per_tick) = handles(&harness, "hit volumes light_1_hit 5..8", 3);
    drag(&mut harness, end, end + egui::vec2(per_tick, 0.0));
    let edited = text_of(&harness);
    fs::write(&path, not_text).expect("someone else's save");

    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run_steps(2);
    assert_eq!(fs::read(&path).expect("the file"), not_text);
    assert!(harness.state().notice().is_some_and(|notice| notice.trouble));
    assert!(harness.query_by_label("Take the file's").is_none());

    harness.get_by_label("Keep mine").click();
    harness.run_steps(8);
    assert_eq!(fs::read(&path).expect("the file"), not_text, "not until the next save");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run_steps(2);
    assert_same(&fs::read_to_string(&path).expect("the file"), &edited, "saved");
}

/// A field's row can go before it lets go: to another move, or with its entry. What was typed goes to the
/// entry it was typed over, ahead of what took it away.
#[test]
fn what_is_typed_lands_on_its_own_entry_when_its_row_goes() {
    let dir = TestDir::new("typed_row");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    click_the_slices_move(&harness, 1);
    harness.run_steps(3);
    assert_eq!(harness.state().selected_move_name(), Some("light_2"));
    let typed = slice_with_typed_name();
    assert_same(&text_of(&harness), &typed, "the first move's cue");
    assert_eq!(parse(&text_of(&harness)).moves[1].cues[0].name, "swing", "and no other");
    harness.state_mut().undo();
    harness.run_steps(2);
    assert_same(&text_of(&harness), SLICE, "one step");

    // Deleted with its name half changed: both are steps, and it can all come back.
    assert!(harness.state_mut().select_move("light_1"));
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    harness.get_by_label("Delete").click();
    harness.run_steps(3);
    let after = parse(&text_of(&harness));
    assert_eq!(after.moves[0].cues.len(), 1);
    assert_eq!(after.moves[0].cues[0].name, "swing", "the entry that was there, not the next one");
    harness.state_mut().undo();
    harness.run_steps(2);
    assert_same(&text_of(&harness), &typed, "the delete");
    harness.state_mut().undo();
    harness.run_steps(2);
    assert_same(&text_of(&harness), SLICE, "and the name");

    // A drag starts its own step: what's typed isn't part of it.
    type_over_the_cue_name(&mut harness, TYPED_NAME);
    let (_, end, per_tick) = handles(&harness, "hit volumes light_1_hit 5..8", 3);
    drag(&mut harness, end, end + egui::vec2(per_tick, 0.0));
    assert_same(&text_of(&harness), &hand_edit(&typed, "light_1", "to: 8,", "to: 9,"), "both");
    harness.state_mut().undo();
    harness.run_steps(2);
    assert_same(&text_of(&harness), &typed, "the drag alone");
}

/// A small edit of the slice, as the timeline makes it.
fn move_the_first_cue(harness: &mut Harness<'static, Editor>, at: u32) {
    let entry = EntryRef { mv: 0, track: Track::Cues, index: 0 };
    harness.state_mut().apply(Action::SetInstant { entry, at });
    harness.run_steps(1);
}

fn save_by_key(harness: &mut Harness<'static, Editor>) {
    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run_steps(2);
}

/// Runs the interface until the device has had `how_many` documents.
fn until_it_has_documents(harness: &mut Harness<'static, Editor>, device: &FakeDevice, how_many: usize) {
    let started = Stopwatch::start();
    while device.documents().len() < how_many {
        assert!(started.elapsed() < Duration::from_secs(30), "never saw document {how_many}");
        harness.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    // What the thread reported before sending this one has now been read.
    harness.run_steps(1);
}

fn has_a_device_problem(editor: &Editor) -> bool {
    editor.problems().iter().any(|problem| problem.from_device)
}

const SLICE_REFUSAL: &str = "slice.moves.ron: move `light_1`, track `cues`: the fight here can't take this";

/// An answer that comes after the text moved on is about what was, whichever way it went. It doesn't mark
/// this text with a refusal or read as this text having arrived, and the panel says which it's about.
#[test]
fn a_late_answer_doesnt_speak_for_newer_text() {
    for refused in [false, true] {
        let device = FakeDevice::start();
        let dir = TestDir::new("late_answer");
        let path = dir.write("slice.moves.ron", SLICE);
        let mut harness = on_device(&path, &device);
        harness.run_steps(3);
        device.hold_documents();
        if refused {
            device.refuse_with(SLICE_REFUSAL);
        }
        move_the_first_cue(&mut harness, 1);
        save_by_key(&mut harness);
        until_it_has_documents(&mut harness, &device, 1);
        assert!(harness.state().link().is_waiting());

        move_the_first_cue(&mut harness, 2);
        device.answer_documents(1);
        until(&mut harness, "the answer", |editor| !editor.link().is_waiting());
        assert_eq!(harness.state().link().last_push(), None, "refused: {refused}");
        assert!(!has_a_device_problem(harness.state()), "refused: {refused}");
        let _ = harness.get_by_label_contains("An earlier save of slice.moves.ron");
        assert!(harness.query_by_label_contains("Took it in").is_none());

        // The next save's answer is the one that counts, and the note goes with it.
        device.answer_documents(1);
        save_by_key(&mut harness);
        until(&mut harness, "the next answer", |editor| editor.link().last_push().is_some());
        assert!(harness.state().link().last_push().is_some());
        assert_eq!(has_a_device_problem(harness.state()), refused);
        assert!(harness.query_by_label_contains("An earlier save").is_none());
    }
}

/// With a save behind another, it's waiting until the last has an answer, and the last is what the panel
/// shows. One waiting is replaced by a newer save, which says so: the device gets the newest, not each.
#[test]
fn saves_queued_are_all_waited_for() {
    let device = FakeDevice::start();
    let dir = TestDir::new("queued");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = on_device(&path, &device);
    harness.run_steps(3);
    device.hold_documents();
    move_the_first_cue(&mut harness, 1);
    save_by_key(&mut harness);
    let first = text_of(&harness);
    until_it_has_documents(&mut harness, &device, 1);
    move_the_first_cue(&mut harness, 2);
    save_by_key(&mut harness);
    move_the_first_cue(&mut harness, 3);
    save_by_key(&mut harness);
    let second = text_of(&harness);
    harness.run_steps(1);
    let _ = harness.get_by_label_contains("An earlier save of slice.moves.ron");

    device.answer_documents(1);
    until_it_has_documents(&mut harness, &device, 2);
    assert!(harness.state().link().is_waiting(), "the second has no answer yet");
    let _ = harness.get_by_label("Pushing...");
    assert_eq!(harness.state().link().last_push(), None, "and the first was of the text before");

    device.answer_documents(1);
    until(&mut harness, "the second answer", |editor| !editor.link().is_waiting());
    assert!(matches!(harness.state().link().last_push(), Some(Pushed::Took { .. })));
    assert_eq!(device.documents(), [first, second]);

    // The same text saved twice: either answer is about this text.
    device.hold_documents();
    save_by_key(&mut harness);
    until_it_has_documents(&mut harness, &device, 3);
    save_by_key(&mut harness);
    device.refuse_with(SLICE_REFUSAL);
    device.answer_documents(1);
    until(&mut harness, "its refusal", has_a_device_problem);
    assert!(harness.state().link().is_waiting(), "the other is still out");
    device.answer_documents(1);
    until(&mut harness, "the last", |editor| !editor.link().is_waiting());
    assert!(has_a_device_problem(harness.state()));
}

/// Another document isn't marked by what a device said of the last, already said or still on its way.
#[test]
fn another_document_starts_with_nothing_said_of_it() {
    let device = FakeDevice::start();
    let dir = TestDir::new("switching");
    let path = dir.write("slice.moves.ron", SLICE);
    let other = dir.write("warrior.moves.ron", WARRIOR);
    let mut harness = on_device(&path, &device);
    harness.run_steps(3);
    device.refuse_with(SLICE_REFUSAL);
    move_the_first_cue(&mut harness, 1);
    save_by_key(&mut harness);
    until(&mut harness, "the refusal", has_a_device_problem);
    harness.state_mut().open(&other);
    harness.run_steps(2);
    assert_eq!(harness.state().link().last_push(), None);
    assert!(!has_a_device_problem(harness.state()));

    harness.state_mut().open(&path);
    harness.run_steps(2);
    device.hold_documents();
    move_the_first_cue(&mut harness, 2);
    save_by_key(&mut harness);
    until_it_has_documents(&mut harness, &device, 2);
    harness.state_mut().open(&other);
    harness.run_steps(2);
    device.answer_documents(1);
    until(&mut harness, "the answer", |editor| !editor.link().is_waiting());
    assert!(!has_a_device_problem(harness.state()));
    let _ = harness.get_by_label_contains("An earlier save of slice.moves.ron");
}

/// What the last device says isn't taken for the next one's, however late it arrives.
#[test]
fn another_device_starts_with_nothing_said_of_it() {
    let first = FakeDevice::playing(None, "first", &[(0, "light_1")]);
    let second = FakeDevice::playing(None, "second", &[(7, "swipe")]);
    let dir = TestDir::new("targets");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = on_device(&path, &first);
    harness.run_steps(3);
    let platform = |editor: &Editor| editor.link().device().map(|status| status.platform.clone());
    until(&mut harness, "the first", |editor| platform(editor).as_deref() == Some("first"));

    // A status on its way as the address changes.
    first.hold_statuses();
    let asked = first.statuses_asked();
    let started = Stopwatch::start();
    while first.statuses_asked() == asked {
        assert!(started.elapsed() < Duration::from_secs(30), "it never asked");
        harness.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.state_mut().set_link_address(second.address);
    assert_eq!(platform(harness.state()), None);
    assert!(harness.state().link().attempts().is_empty());
    first.answer_statuses(1);
    let started = Stopwatch::start();
    while platform(harness.state()).as_deref() != Some("second") {
        assert_ne!(platform(harness.state()).as_deref(), Some("first"), "the last device's word");
        assert!(started.elapsed() < Duration::from_secs(30), "never saw the second");
        harness.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    let attempts = harness.state().link().attempts();
    assert_eq!(attempts.iter().map(|attempt| attempt.actor).collect::<Vec<_>>(), [7]);

    // An answer on hand says nothing of the new one; the same address again is no change at all.
    second.refuse_with(SLICE_REFUSAL);
    move_the_first_cue(&mut harness, 1);
    save_by_key(&mut harness);
    until(&mut harness, "the refusal", has_a_device_problem);
    harness.state_mut().set_link_address(second.address);
    assert!(has_a_device_problem(harness.state()), "nothing changed");
    assert!(harness.state().link().device().is_some());
    harness.state_mut().set_link_address(first.address);
    assert_eq!(harness.state().link().last_push(), None);
    assert!(!has_a_device_problem(harness.state()));

    // A push on its way as the address changes: it went to the last device, and says so.
    first.answer_statuses(usize::MAX / 2);
    until(&mut harness, "the first again", |editor| platform(editor).as_deref() == Some("first"));
    first.hold_documents();
    first.refuse_with(SLICE_REFUSAL);
    save_by_key(&mut harness);
    until_it_has_documents(&mut harness, &first, 1);
    harness.state_mut().set_link_address(second.address);
    first.answer_documents(1);
    until(&mut harness, "the answer", |editor| !editor.link().is_waiting());
    assert_eq!(harness.state().link().last_push(), None);
    assert!(!has_a_device_problem(harness.state()));
    let _ = harness.get_by_label_contains("An earlier save of slice.moves.ron");
}

/// A device that started again is a new fight, with actor numbers that mean something else.
#[test]
fn a_device_that_restarts_starts_its_attempts_over() {
    let first = FakeDevice::playing(None, "here", &[(0, "light_1"), (1, "swipe")]);
    let address = first.address;
    let dir = TestDir::new("restart");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = on_device(&path, &first);
    harness.run_steps(3);
    until(&mut harness, "both", |editor| editor.link().attempts().len() == 2);

    drop(first);
    let _second = FakeDevice::playing(Some(address), "here", &[(0, "heavy")]);
    until(&mut harness, "the new fight", |editor| {
        editor.link().attempts().iter().any(|attempt| attempt.name == "heavy")
    });
    let attempts = harness.state().link().attempts();
    assert_eq!(attempts.iter().map(|attempt| attempt.name.as_str()).collect::<Vec<_>>(), ["heavy"]);
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
/// answers as a fight's app does, without the fight. It can sit on its answers, as a busy device does.
struct FakeDevice {
    address: SocketAddr,
    refusal: Arc<Mutex<Option<String>>>,
    documents: Arc<Mutex<Vec<String>>>,
    answers: Arc<Answers>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// How many more of each it will answer, and how many of them it has been asked for.
struct Answers {
    documents: AtomicUsize,
    statuses: AtomicUsize,
    statuses_asked: AtomicUsize,
}

/// The next answer to give, if it's allowed one.
fn next_answer<T>(allowed: &AtomicUsize, held: &mut VecDeque<T>) -> Option<T> {
    let take_one = |left: usize| left.checked_sub(1);
    if held.is_empty() || allowed.try_update(Ordering::Relaxed, Ordering::Relaxed, take_one).is_err() {
        return None;
    }
    held.pop_front()
}

impl FakeDevice {
    fn start() -> Self {
        Self::playing(None, "here", &[(0, "light_1")])
    }

    /// One that calls itself `platform`, playing a move on each actor it's given, at `address` if it's told
    /// one, where a device that was there a moment ago may still be letting go.
    fn playing(address: Option<SocketAddr>, platform: &'static str, actors: &'static [(u32, &str)]) -> Self {
        let address = address.unwrap_or_else(|| {
            let free = TcpListener::bind("127.0.0.1:0").expect("a port");
            free.local_addr().expect("its address")
        });
        let started = Stopwatch::start();
        let link = loop {
            let packs = Arc::new(LivePacks::new(MemoryPackSource::new()));
            match LiveLink::start(LinkMode::Listen(address), packs) {
                Ok(link) => break link,
                Err(error) => {
                    assert!(started.elapsed() < Duration::from_secs(10), "no listener: {error}");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };
        let refusal = Arc::new(Mutex::new(None::<String>));
        let documents = Arc::new(Mutex::new(Vec::new()));
        let answers = Arc::new(Answers {
            documents: AtomicUsize::new(usize::MAX),
            statuses: AtomicUsize::new(usize::MAX),
            statuses_asked: AtomicUsize::new(0),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (refusal, documents, answers, stop) =
                (Arc::clone(&refusal), Arc::clone(&documents), Arc::clone(&answers), Arc::clone(&stop));
            move || {
                let mut revision = 0;
                let (mut statuses, mut replies) = (VecDeque::new(), VecDeque::new());
                while !stop.load(Ordering::Relaxed) {
                    let push = link.poll();
                    let idle = push.is_none();
                    match push {
                        Some(Push::Status { reply }) => {
                            answers.statuses_asked.fetch_add(1, Ordering::Relaxed);
                            statuses.push_back(reply);
                        }
                        Some(Push::Document { text, reply, .. }) => {
                            documents.lock().expect("the list").push(text);
                            replies.push_back(reply);
                        }
                        Some(_) | None => {}
                    }
                    while let Some(reply) = next_answer(&answers.statuses, &mut statuses) {
                        let attempts = actors.iter().map(|(actor, name)| MoveAttempt {
                            actor: *actor,
                            name: (*name).to_owned(),
                            revision,
                            newest_revision: revision,
                            ..MoveAttempt::default()
                        });
                        reply.reply(ReplyBody::Status(DeviceStatus {
                            platform: platform.to_owned(),
                            cook: "desktop".to_owned(),
                            attempts: attempts.collect(),
                            ..DeviceStatus::default()
                        }));
                    }
                    while let Some(reply) = next_answer(&answers.documents, &mut replies) {
                        if let Some(why) = refusal.lock().expect("the refusal").clone() {
                            reply.refuse(why);
                        } else {
                            revision += 1;
                            let owner = format!("the fight's moves, at revision {revision}");
                            reply.reply(ReplyBody::DocumentApplied { owner });
                        }
                    }
                    if idle {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            }
        });
        Self { address, refusal, documents, answers, stop, thread: Some(thread) }
    }

    fn refuse_with(&self, why: &str) {
        *self.refusal.lock().expect("the refusal") = Some(why.to_owned());
    }

    fn documents(&self) -> Vec<String> {
        self.documents.lock().expect("the list").clone()
    }

    /// From here on it answers no document until told it may.
    fn hold_documents(&self) {
        self.answers.documents.store(0, Ordering::Relaxed);
    }

    fn answer_documents(&self, how_many: usize) {
        self.answers.documents.fetch_add(how_many, Ordering::Relaxed);
    }

    fn hold_statuses(&self) {
        self.answers.statuses.store(0, Ordering::Relaxed);
    }

    fn answer_statuses(&self, how_many: usize) {
        self.answers.statuses.fetch_add(how_many, Ordering::Relaxed);
    }

    fn statuses_asked(&self) -> usize {
        self.answers.statuses_asked.load(Ordering::Relaxed)
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
