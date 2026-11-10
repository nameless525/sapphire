//! The panes T3 didn't have, driven as a person drives them with no window: the browser, the inspector any
//! document gets, a device's cvars, and more than one document open at once.

use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{Key, Modifiers, Role};
use egui_kittest::kittest::{By, NodeT, Queryable};
use egui_kittest::{Harness, Node};
use sapphire_assets::MemoryPackSource;
use sapphire_core::cvar::{Cvar, CvarRegistry};
use sapphire_editor::capture::{self, Textures};
use sapphire_editor::edits;
use sapphire_editor::screen::{Aspect, StandIn};
use sapphire_editor::shell::Pane;
use sapphire_editor::{Editor, Options};
use sapphire_livelink::protocol::{DeviceStatus, ReplyBody};
use sapphire_livelink::{LinkMode, LiveLink, LivePacks, Push, list_cvars, set_cvar};
use sapphire_moves::MoveSetDocument;
use sapphire_time::Stopwatch;

const PLAYER: &str = include_str!("../../../content/combat/player/player.moves.ron");
const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
const CUE: &str = include_str!("../../../content/audio/hit_heavy.cue.ron");

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("sapphire_editor_panes_{name}_{}_{unique}", std::process::id()));
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

/// An editor on `path`, whose interface's clock runs at half a second a step, and a port nobody has.
fn harness(path: &Path) -> Harness<'static, Editor> {
    on_link(path, "127.0.0.1:9".parse().expect("an address"), 0.5)
}

fn on_link(path: &Path, link: SocketAddr, step: f32) -> Harness<'static, Editor> {
    let options = Options {
        file: Some(path.to_owned()),
        content: Some(path.with_file_name("no cook here")),
        link: Some(link),
    };
    Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .with_step_dt(step)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), Editor::new(options))
}

fn row<'a>(harness: &'a Harness<'static, Editor>, name: &'a str) -> Node<'a> {
    harness.get(By::new().role(Role::Button).label(name))
}

fn has_row(harness: &Harness<'static, Editor>, name: &str) -> bool {
    harness.query(By::new().role(Role::Button).label(name)).is_some()
}

fn text_of(harness: &Harness<'static, Editor>) -> String {
    harness.state().current().expect("a file").text().to_owned()
}

#[test]
fn opening_another_file_keeps_new_conflict_notices_for_the_previous_document() {
    let dir = TestDir::new("open_conflict");
    let path = dir.write("slice.moves.ron", SLICE);
    let cue = dir.write("hit_heavy.cue.ron", CUE);
    let mut editor = Editor::new(Options {
        file: Some(path.clone()),
        content: Some(dir.0.join("no cook")),
        link: Some("127.0.0.1:9".parse().expect("address")),
    });
    editor
        .moves_document_mut()
        .expect("moves")
        .together("edit", |document| {
            edits::set_window(
                document,
                edits::EntryRef { mv: 0, track: edits::Track::Hitboxes, index: 0 },
                5,
                9,
            )
        })
        .expect("edit");
    fs::write(&path, SLICE.replacen("startup: 5", "startup: 6", 1)).expect("external change");
    editor.open(&cue);
    assert!(
        editor
            .notice()
            .is_some_and(|notice| notice.trouble && notice.text.contains("slice.moves.ron changed on disk"))
    );
}

#[test]
fn a_refused_rename_uses_the_previous_slots_successful_preview() {
    let dir = TestDir::new("refused_rename");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut editor = Editor::new(Options {
        file: Some(path),
        content: Some(dir.0.join("no cook")),
        link: Some("127.0.0.1:9".parse().expect("address")),
    });
    let before = editor.run().expect("run").clone();
    let property = "moves[\"light_1\"].name".parse::<sapphire_document::PropertyPath>().expect("path");
    editor
        .moves_document_mut()
        .expect("document")
        .set_literal("rename", &property, "\"renamed\"")
        .expect("edit");
    editor.refresh();
    assert!(!editor.problems().is_empty(), "neutral still names light_1");
    assert_eq!(editor.selected_move_name(), Some("renamed"));
    assert_eq!(editor.run(), Some(&before));
    assert!(editor.select_move("skill"));
    assert_eq!(editor.run().expect("another prior move").move_name, "skill");
}

fn name_of(harness: &Harness<'static, Editor>) -> String {
    harness.state().current().expect("a file").file_name()
}

fn undo(harness: &mut Harness<'static, Editor>) {
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
}

/// The widget of `role` on the row of the label `name`.
fn beside<'a>(harness: &'a Harness<'static, Editor>, name: &'a str, role: Role) -> Node<'a> {
    // With the same words twice, as both finds, it's the one further right.
    let label = harness
        .get_all_by_label(name)
        .map(|label| label.rect())
        .max_by(|a, b| a.left().total_cmp(&b.left()))
        .unwrap_or_else(|| panic!("no {name}"));
    harness
        .get_all(By::new().role(role))
        .find(|node| {
            let rect = node.rect();
            rect.left() >= label.left() && (rect.center().y - label.center().y).abs() < 8.0
        })
        .unwrap_or_else(|| panic!("nothing of {role:?} beside {name}"))
}

/// Opens a closed place of the Properties pane, by the arrow left of its name.
fn expand(harness: &mut Harness<'static, Editor>, name: &str) {
    let label = harness.get_by_label(name).rect();
    let arrow = egui::pos2(label.left() - 10.0, label.center().y);
    harness.hover_at(arrow);
    harness.run_steps(1);
    harness.drag_at(arrow);
    harness.run_steps(1);
    harness.drop_at(arrow);
    harness.run_steps(4);
}

/// The browser, a section a kind: a click opens each as what it is, and what's open says so in its row.
#[test]
fn the_browser_opens_each_document_as_its_kind() {
    let dir = TestDir::new("browser");
    let path = dir.write("a/player.moves.ron", PLAYER);
    dir.write("a/hit_heavy.cue.ron", CUE);
    dir.write("a/.hidden/not_listed.cue.ron", CUE);
    let mut harness = harness(&path);
    // Its own folder is where it looks, with no repository around it.
    harness.state_mut().documents_mut().set_roots(vec![dir.0.clone()]);
    harness.state_mut().documents_mut().rescan().expect("a look");
    harness.run_steps(2);
    assert!(harness.get_all_by_label("Moves").count() >= 2, "a section of the browser's, and a tab");
    let _ = harness.get_by_label("Cues");
    assert!(has_row(&harness, "player.moves.ron"));

    row(&harness, "hit_heavy.cue.ron").click();
    harness.run_steps(3);
    assert_eq!(name_of(&harness), "hit_heavy.cue.ron");
    assert_eq!(harness.state().selected_move_name(), Some("light_1"), "the timeline stays on the moves");
    let current = harness.state().current().expect("a file");
    assert_eq!(current.text(), CUE);

    // A row says what's unsaved, in any document, from any document.
    let id = current.id();
    let path_of_sound = "sounds[0]".parse().expect("a path");
    harness
        .state_mut()
        .documents_mut()
        .any_mut(id)
        .expect("open")
        .remove_entry("x", &path_of_sound)
        .expect("gone");
    harness.run_steps(2);
    assert!(has_row(&harness, "hit_heavy.cue.ron *"));
    row(&harness, "player.moves.ron").click();
    harness.run_steps(3);
    assert_eq!(name_of(&harness), "player.moves.ron");
    assert!(has_row(&harness, "hit_heavy.cue.ron *"));

    // A file that changed under edits says so in its row, and a notice says what, since it isn't the one shown.
    fs::write(dir.0.join("a/hit_heavy.cue.ron"), CUE.replacen("-3.0", "-4.0", 1)).expect("someone's save");
    harness.run_steps(4);
    assert!(has_row(&harness, "hit_heavy.cue.ron !"));
    let notice = harness.state().notice().expect("a notice");
    assert!(notice.text.contains("hit_heavy.cue.ron"), "{notice:?}");

    // Finding narrows it, by any part of the path.
    let find = beside(&harness, "Find", Role::TextInput);
    find.click();
    harness.run_steps(1);
    beside(&harness, "Find", Role::TextInput).type_text("cue");
    harness.run_steps(2);
    assert!(!has_row(&harness, "player.moves.ron"));
    assert!(has_row(&harness, "hit_heavy.cue.ron !"));
}

/// The timeline's edits are its document's, whichever the top bar was on: it comes back, so what's saved or undone
/// next is what was just changed.
#[test]
fn an_edit_on_the_timeline_goes_to_its_document() {
    let dir = TestDir::new("back");
    let path = dir.write("player.moves.ron", PLAYER);
    dir.write("hit_heavy.cue.ron", CUE);
    let mut harness = harness(&path);
    harness.run_steps(2);
    row(&harness, "hit_heavy.cue.ron").click();
    harness.run_steps(3);
    assert_eq!(name_of(&harness), "hit_heavy.cue.ron");

    let end = harness.get_by_label("hit volumes light_1_hit 7..11 end").rect().center();
    harness.hover_at(end);
    harness.run_steps(2);
    harness.drag_at(end);
    harness.run_steps(1);
    harness.hover_at(end + egui::vec2(40.0, 0.0));
    harness.run_steps(1);
    harness.drop_at(end + egui::vec2(40.0, 0.0));
    harness.run_steps(3);
    assert_eq!(name_of(&harness), "player.moves.ron");
    assert_ne!(text_of(&harness), PLAYER);
    undo(&mut harness);
    assert_eq!(text_of(&harness), PLAYER);
}

/// The list comes from a look, and a look comes from the timer, not from each frame or each open: a new file
/// shows within five seconds and a look doesn't happen during a drag.
#[test]
fn the_browser_looks_on_a_timer_and_not_during_a_gesture() {
    let dir = TestDir::new("timer");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(2);
    let scans = |harness: &Harness<'static, Editor>| harness.state().documents().stats().scans;
    let before = scans(&harness);
    // A second and a half, all drawn.
    harness.run_steps(3);
    assert_eq!(scans(&harness), before, "no look a frame");

    dir.write("later.cue.ron", CUE);
    assert!(!has_row(&harness, "later.cue.ron"));
    harness.run_steps(12);
    assert!(scans(&harness) > before);
    assert!(has_row(&harness, "later.cue.ron"));

    // A new ID takes one look, to be sure of it, and going to what's open takes none.
    let before = scans(&harness);
    harness.state_mut().open(&dir.0.join("later.cue.ron"));
    assert_eq!(name_of(&harness), "later.cue.ron");
    assert_eq!(scans(&harness), before + 1);
    let before = scans(&harness);
    harness.state_mut().open(&path);
    harness.state_mut().open(&dir.0.join("later.cue.ron"));
    assert_eq!(scans(&harness), before, "both are known");

    // A window's end held down: a drag's frames stay even.
    row(&harness, "player.moves.ron").click();
    harness.run_steps(2);
    let end = harness.get_by_label("hit volumes light_1_hit 7..11 end").rect().center();
    harness.hover_at(end);
    harness.run_steps(2);
    harness.drag_at(end);
    harness.run_steps(1);
    harness.hover_at(end + egui::vec2(30.0, 0.0));
    harness.run_steps(1);
    assert!(harness.state().current().expect("a file").in_gesture());
    let before = scans(&harness);
    dir.write("during.cue.ron", CUE);
    harness.run_steps(14);
    assert_eq!(scans(&harness), before);
    assert!(!has_row(&harness, "during.cue.ron"));
    harness.drop_at(end);
    harness.run_steps(6);
    assert!(has_row(&harness, "during.cue.ron"), "the look comes once it's over");
}

/// Copying a document with its sidecar makes two claims of one ID. The open one stays open and editable, and a
/// notice says what to do, once.
#[test]
fn a_copy_made_with_its_sidecar_says_so_once() {
    let dir = TestDir::new("copy");
    let path = dir.write("player.moves.ron", PLAYER);
    let mut harness = harness(&path);
    harness.run_steps(2);
    let sidecar = fs::read_to_string(dir.0.join("player.moves.ron.meta")).expect("a sidecar");
    dir.write("copy.moves.ron", PLAYER);
    dir.write("copy.moves.ron.meta", &sidecar);
    harness.run_steps(12);
    let notice = harness.state().notice().expect("a notice").clone();
    assert!(notice.trouble);
    assert!(notice.text.contains("hold the same ID"), "{notice:?}");
    assert_eq!(name_of(&harness), "player.moves.ron");

    let edit = harness.state_mut().moves_document_mut().expect("moves").set_text(
        "edit",
        &"moves[0].startup".parse().expect("a path"),
        "8",
    );
    edit.expect("still editable");
}

/// A cue is nothing the timeline knows. Every kind of value of it is edited from its reflection alone: a number,
/// a word that's refused and one that isn't, an `Option`, a variant and a list's entry, and each is a step of its
/// document's undo.
#[test]
fn properties_edit_a_document_the_editor_knows_nothing_of() {
    let dir = TestDir::new("properties");
    let path = dir.write("hit_heavy.cue.ron", CUE);
    let mut harness = harness(&path);
    harness.run_steps(2);
    harness.get_by_label("Properties").click();
    harness.run_steps(3);

    // A number, by its widget, at the file's own spot and nowhere else.
    expand(&mut harness, "volume_db");
    let number = harness
        .get_all(By::new().role(Role::SpinButton))
        .find(|number| number.accesskit_node().numeric_value() == Some(-3.0))
        .expect("the volume's lower end");
    number.focus();
    harness.run_steps(3);
    harness
        .get_all(By::new().role(Role::SpinButton))
        .find(|number| number.is_focused())
        .expect("a number has the keyboard")
        .type_text("2");
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert!(text_of(&harness).contains("volume_db: (2.00, -1.0)"), "{}", text_of(&harness));
    assert_eq!(harness.state().current().expect("a file").undo_label(), Some("edit 0"));
    undo(&mut harness);
    assert_eq!(text_of(&harness), CUE);
    expand(&mut harness, "volume_db");

    // An `Option`, off and back by undo, keeping what was in it. Off is its default, which the file leaves out.
    beside(&harness, "attenuation", Role::CheckBox).click();
    harness.run_steps(3);
    assert!(!text_of(&harness).contains("attenuation"), "{}", text_of(&harness));
    undo(&mut harness);
    assert_eq!(text_of(&harness), CUE);

    // A variant.
    harness.get(By::new().role(Role::ComboBox).value("Sfx")).click();
    harness.run_steps(2);
    harness.get_by_label("Ambience").click();
    harness.run_steps(3);
    assert!(text_of(&harness).contains("bus: Ambience,"), "{}", text_of(&harness));
    undo(&mut harness);
    assert_eq!(text_of(&harness), CUE);

    // A list's entry goes, and the comment that was the file's stays where it is on the others.
    expand(&mut harness, "sounds");
    harness.get_all(By::new().role(Role::Button).label("x")).next().expect("an entry").click();
    harness.run_steps(3);
    let parsed = text_of(&harness);
    assert!(!parsed.contains("777260c4"), "{parsed}");
    assert!(parsed.contains("impactPunch_heavy_001"), "{parsed}");
    undo(&mut harness);
    assert_eq!(text_of(&harness), CUE);

    // A list that can't make a new entry says so: an ID has no default.
    harness.get(By::new().role(Role::Button).label("+")).click();
    harness.run_steps(3);
    assert!(harness.state().notice().is_some_and(|notice| notice.trouble), "{:?}", harness.state().notice());
    assert_eq!(text_of(&harness), CUE);

    // What the parser won't have stays where it's typed, stops the rest and can be dropped.
    let first_sound = "\"777260c4-e322-4f49-9a56-69565c28b7a0\"";
    let field = harness.get(By::new().role(Role::TextInput).value(first_sound));
    field.click();
    harness.run_steps(2);
    harness.get(By::new().role(Role::TextInput).value(first_sound)).type_text("x");
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert_eq!(text_of(&harness), CUE);
    assert!(harness.query_by_label_contains("can't go in 0").is_some());
    harness.get_by_label("Drop it").click();
    harness.run_steps(3);
    assert_eq!(text_of(&harness), CUE);
    assert!(!harness.state().is_dirty());

    // One it will have goes in as typed, with what it writes the bytes it was given.
    let other = "\"11111111-2222-4333-8444-555555555555\"";
    let field = harness.get(By::new().role(Role::TextInput).value(first_sound));
    field.click();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.run_steps(1);
    harness.get(By::new().role(Role::TextInput).value(first_sound)).type_text(other);
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert_eq!(text_of(&harness), CUE.replacen(first_sound, other, 1));
    harness.state_mut().undo();
    harness.run_steps(2);
    assert_eq!(text_of(&harness), CUE);
}

/// What the panes look like, with no window: for a person, or an agent, to read.
#[test]
fn a_capture_of_the_panes_reads() {
    let dir = TestDir::new("capture");
    let path = dir.write("hit_heavy.cue.ron", CUE);
    dir.write("player.moves.ron", PLAYER);
    let mut editor = Editor::new(Options {
        file: Some(dir.0.join("player.moves.ron")),
        content: Some(dir.0.join("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
    });
    editor.open(&path);
    editor.shell_mut().open(Pane::Properties);
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
    let folder = std::env::var_os("SAPPHIRE_EDITOR_CAPTURES")
        .map_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")), PathBuf::from);
    fs::create_dir_all(&folder).expect("a folder for captures");
    last.expect("a capture").write_png(&folder.join("panes.png")).expect("written");
}

/// A new entry comes from the type, on a list that can make one, and the timeline sees it.
#[test]
fn properties_add_an_entry_from_its_type() {
    let dir = TestDir::new("add");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    harness.run_steps(2);
    harness.get_by_label("Properties").click();
    harness.run_steps(3);
    let moves = MoveSetDocument::parse(SLICE).expect("parses").moves.len();
    beside(&harness, "moves", Role::Button).click();
    harness.run_steps(3);
    assert_eq!(harness.state().notice(), None);
    let added = MoveSetDocument::parse(&text_of(&harness)).expect("parses");
    assert_eq!(added.moves.len(), moves + 1);
    undo(&mut harness);
    assert_eq!(text_of(&harness), SLICE);
}

/// Past its cap a document lets its oldest steps go. Undo at the floor stays clickable, to say why it won't, and
/// leaves the text and the marks alone.
#[test]
fn undo_past_the_cap_says_so() {
    let dir = TestDir::new("cap");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    harness.run_steps(2);
    let id = harness.state().current().expect("a file").id();
    harness.state_mut().documents_mut().any_mut(id).expect("open").set_history_limit(600);
    let entry = edits::EntryRef { mv: 0, track: edits::Track::Cues, index: 0 };
    for at in 1..=9 {
        let document = harness.state_mut().moves_document_mut().expect("moves");
        document.together("move a cue", |document| edits::set_instant(document, entry, at)).expect("an edit");
    }
    harness.run_steps(2);
    let forgotten = harness.state().documents().any(id).expect("open").forgotten();
    assert!(forgotten > 0, "the cap is low enough to test");
    while harness.state().current().expect("a file").undo_label().is_some() {
        harness.state_mut().undo();
    }
    harness.run_steps(2);
    let floor = text_of(&harness);
    assert_ne!(floor, SLICE, "what's older is gone");
    assert_eq!(harness.state().notice(), None);

    harness.get_by_label("Undo").click();
    harness.run_steps(3);
    let notice = harness.state().notice().expect("it says why");
    assert!(notice.text.contains("nothing older to undo"), "{notice:?}");
    assert_eq!(text_of(&harness), floor, "and does nothing else");
    assert!(harness.state().is_dirty(), "and it's still not what the file has");

    // A save still saves, and redo still goes forward.
    harness.state_mut().redo();
    harness.run_steps(2);
    assert_ne!(text_of(&harness), floor);
}

/// Quit asks about every document and saves them all; Close asks about one.
#[test]
fn several_documents_with_edits_are_all_asked_about_and_saved() {
    let dir = TestDir::new("several");
    let path = dir.write("slice.moves.ron", SLICE);
    let cue = dir.write("hit_heavy.cue.ron", CUE);
    let mut harness = harness(&path);
    harness.run_steps(2);
    let entry = edits::EntryRef { mv: 0, track: edits::Track::Cues, index: 0 };
    let edit = harness.state_mut().moves_document_mut().expect("moves");
    edit.together("move a cue", |document| edits::set_instant(document, entry, 3)).expect("an edit");
    let moved = edit.text().to_owned();
    row(&harness, "hit_heavy.cue.ron").click();
    harness.run_steps(2);
    let id = harness.state().current().expect("a file").id();
    let path_of_sound = "sounds[0]".parse().expect("a path");
    let less = {
        let document = harness.state_mut().documents_mut().any_mut(id).expect("open");
        document.remove_entry("x", &path_of_sound).expect("gone");
        document.text().to_owned()
    };
    harness.run_steps(2);

    // Close: for this document alone, and each answer does what it says.
    harness.get_by_label("Close file").click();
    harness.run_steps(2);
    harness.get_by_label("Keep editing").click();
    harness.run_steps(2);
    assert_eq!(name_of(&harness), "hit_heavy.cue.ron");
    harness.get_by_label("Close file").click();
    harness.run_steps(2);
    harness.get_by_label("Close without saving").click();
    harness.run_steps(2);
    assert_eq!(name_of(&harness), "slice.moves.ron", "the moves are left");
    assert_eq!(fs::read_to_string(&cue).expect("the file"), CUE, "not saved");
    row(&harness, "hit_heavy.cue.ron").click();
    harness.run_steps(2);
    let id = harness.state().current().expect("a file").id();
    assert_eq!(text_of(&harness), CUE, "opened again from the file");
    harness
        .state_mut()
        .documents_mut()
        .any_mut(id)
        .expect("open")
        .remove_entry("x", &path_of_sound)
        .expect("gone");
    harness.run_steps(2);

    harness.state_mut().request_quit();
    assert!(!harness.state().may_quit());
    harness.run_steps(2);
    harness.get_by_label("Save and quit").click();
    harness.run_steps(2);
    assert!(harness.state().may_quit());
    assert_eq!(fs::read_to_string(&path).expect("the file"), moved);
    assert_eq!(fs::read_to_string(&cue).expect("the file"), less);
}

/// A quit's save that can't finish for one document doesn't quit, and saves the rest.
#[test]
fn a_quit_that_cant_save_one_saves_the_rest_and_stays() {
    let dir = TestDir::new("several_fail");
    let path = dir.write("slice.moves.ron", SLICE);
    let cue = dir.write("hit_heavy.cue.ron", CUE);
    let mut harness = harness(&path);
    harness.run_steps(2);
    let entry = edits::EntryRef { mv: 0, track: edits::Track::Cues, index: 0 };
    let edit = harness.state_mut().moves_document_mut().expect("moves");
    edit.together("move a cue", |document| edits::set_instant(document, entry, 3)).expect("an edit");
    let moved = edit.text().to_owned();
    row(&harness, "hit_heavy.cue.ron").click();
    harness.run_steps(2);
    let id = harness.state().current().expect("a file").id();
    let document = harness.state_mut().documents_mut().any_mut(id).expect("open");
    document.remove_entry("x", &"sounds[0]".parse().expect("a path")).expect("gone");
    fs::write(&cue, "someone's").expect("a change on disk");

    harness.state_mut().request_quit();
    harness.run_steps(2);
    harness.get_by_label("Save and quit").click();
    harness.run_steps(2);
    assert!(!harness.state().may_quit());
    assert!(harness.state().notice().is_some_and(|notice| notice.trouble), "{:?}", harness.state().notice());
    assert_eq!(fs::read_to_string(&path).expect("the file"), moved);
    assert_eq!(fs::read_to_string(&cue).expect("the file"), "someone's");
}

/// A third answer to a change on disk: each edit that still fits goes back on what the file says now.
#[test]
fn a_conflict_can_put_edits_back_on_the_files_text() {
    let dir = TestDir::new("rebase");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    harness.run_steps(2);
    let entry = edits::EntryRef { mv: 0, track: edits::Track::Cues, index: 0 };
    let edit = harness.state_mut().moves_document_mut().expect("moves");
    edit.together("move a cue", |document| edits::set_instant(document, entry, 3)).expect("an edit");
    let by_hand = SLICE.replacen("damage: 6,", "damage: 9,", 1);
    assert_ne!(by_hand, SLICE);
    fs::write(&path, &by_hand).expect("someone's save");
    harness.run_steps(4);
    assert!(harness.state().current().expect("a file").conflict().is_some());
    harness.get_by_label("Mine on top of the file's").click();
    harness.run_steps(3);
    let text = text_of(&harness);
    assert!(text.contains("damage: 9,"), "{text}");
    let document = MoveSetDocument::parse(&text).expect("parses");
    assert_eq!(document.moves[0].cues[0].at, 3);
    assert!(harness.state().current().expect("a file").conflict().is_none());
    let notice = harness.state().notice().expect("it says");
    assert!(notice.text.contains("1 edits went back"), "{notice:?}");
}

static BRIGHT: Cvar<f32> = Cvar::with_range("test.bright", 1.0, 0.0, 4.0, "How bright.");
static FOG: Cvar<bool> = Cvar::new("test.fog", true, "Whether there's fog.");

/// A device of the test's own: real live link and a real socket, answering as an app does from a real registry.
struct CvarDevice {
    address: SocketAddr,
    refusal: Arc<Mutex<Option<String>>>,
    listed: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CvarDevice {
    fn start() -> Self {
        let free = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = free.local_addr().expect("its address");
        drop(free);
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
        let mut cvars = CvarRegistry::new();
        cvars.register(&BRIGHT).expect("registered");
        cvars.register(&FOG).expect("registered");
        let refusal = Arc::new(Mutex::new(None::<String>));
        let listed = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (refusal, listed, stop) = (Arc::clone(&refusal), Arc::clone(&listed), Arc::clone(&stop));
            move || {
                while !stop.load(Ordering::Relaxed) {
                    match link.poll() {
                        Some(Push::Status { reply }) => {
                            reply.reply(ReplyBody::Status(DeviceStatus {
                                platform: "here".to_owned(),
                                cook: "desktop".to_owned(),
                                ..DeviceStatus::default()
                            }));
                        }
                        Some(Push::Cvars { reply }) => {
                            listed.fetch_add(1, Ordering::Relaxed);
                            reply.reply(list_cvars(&cvars));
                        }
                        Some(Push::Cvar { name, value, reply }) => {
                            if let Some(why) = refusal.lock().expect("the refusal").clone() {
                                reply.refuse(why);
                            } else {
                                reply.reply(set_cvar(&cvars, &name, &value));
                            }
                        }
                        Some(_) => {}
                        None => std::thread::sleep(Duration::from_millis(2)),
                    }
                }
            }
        });
        Self { address, refusal, listed, stop, thread: Some(thread) }
    }
}

impl Drop for CvarDevice {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn until(
    harness: &mut Harness<'static, Editor>,
    what: &str,
    mut done: impl FnMut(&Harness<'static, Editor>) -> bool,
) {
    let started = Stopwatch::start();
    while !done(harness) {
        assert!(started.elapsed() < Duration::from_secs(30), "never saw {what}");
        harness.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(1);
}

fn type_into_the_number(harness: &mut Harness<'static, Editor>, value: f64, typed: &str) {
    harness
        .get_all(By::new().role(Role::SpinButton))
        .find(|number| number.accesskit_node().numeric_value() == Some(value))
        .expect("the number")
        .focus();
    harness.run_steps(3);
    harness
        .get_all(By::new().role(Role::SpinButton))
        .find(|number| number.is_focused())
        .expect("a number has the keyboard")
        .type_text(typed);
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
}

/// The device says what it has and what each takes, and the panel sets it: real values in a real registry. It
/// asks only while it's being looked at. A refusal shows, and nothing's left of the last one when one's taken.
#[test]
fn cvars_come_from_the_device_and_set_it() {
    BRIGHT.reset();
    FOG.reset();
    let device = CvarDevice::start();
    let dir = TestDir::new("cvars");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = on_link(&path, device.address, 0.05);
    until(&mut harness, "the device", |harness| harness.state().link().device().is_some());
    std::thread::sleep(Duration::from_millis(1200));
    harness.run_steps(3);
    assert_eq!(device.listed.load(Ordering::Relaxed), 0, "nobody's looking");

    harness.get_by_label("Cvars").click();
    harness.run_steps(1);
    until(&mut harness, "the list", |harness| harness.query_by_label("test.bright").is_some());
    assert!(device.listed.load(Ordering::Relaxed) >= 1);

    type_into_the_number(&mut harness, 1.0, "2");
    until(&mut harness, "the number set", |_| (BRIGHT.get() - 2.0).abs() < 1e-6);
    beside(&harness, "test.fog", Role::CheckBox).click();
    harness.run_steps(1);
    until(&mut harness, "the switch set", |_| !FOG.get());

    // Told back to what it was by its row's button.
    beside(&harness, "test.fog", Role::Button).click();
    harness.run_steps(1);
    until(&mut harness, "the switch reset", |_| FOG.get());

    *device.refusal.lock().expect("the refusal") = Some("not this one".to_owned());
    type_into_the_number(&mut harness, 2.0, "3");
    until(&mut harness, "the refusal", |harness| {
        harness.query_by_label("test.bright: not this one").is_some()
    });
    assert!((BRIGHT.get() - 2.0).abs() < 1e-6, "the device kept what it had");
    *device.refusal.lock().expect("the refusal") = None;
    until(&mut harness, "what the device says, back", |harness| {
        harness
            .get_all(By::new().role(Role::SpinButton))
            .any(|number| number.accesskit_node().numeric_value() == Some(2.0))
    });

    // Finding narrows it.
    beside(&harness, "Find", Role::TextInput).click();
    harness.run_steps(1);
    harness
        .get_all(By::new().role(Role::TextInput))
        .find(|field| field.is_focused())
        .expect("the field")
        .type_text("fog");
    harness.run_steps(2);
    assert!(harness.query_by_label("test.bright").is_none());
    assert!(harness.query_by_label("test.fog").is_some());

    // Another tab, and it stops asking.
    harness.get_by_label("Inspector").click();
    harness.run_steps(3);
    let asked = device.listed.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(1200));
    harness.run_steps(3);
    assert!(device.listed.load(Ordering::Relaxed) <= asked + 1, "one may have been on its way");
    BRIGHT.reset();
    FOG.reset();
}

#[test]
fn cvars_with_no_device_say_so() {
    let dir = TestDir::new("cvars_none");
    let path = dir.write("slice.moves.ron", SLICE);
    let mut harness = harness(&path);
    harness.run_steps(2);
    harness.get_by_label("Cvars").click();
    harness.run_steps(3);
    assert!(harness.query_by_label("test.bright").is_none());
    assert_eq!(
        harness.get_all_by_label("Find").count(),
        1,
        "the browser's own: there's nothing to find here"
    );
}

// The zone and sequencer panes (M3 E5), with no GPU: the viewport's half is `tests/viewport_stage.rs`.

const PROVING_GROUND: &str = include_str!("../../../content/zones/proving_ground/proving_ground.zone.ron");
const SQUARE_LOOK: &str = include_str!("../../../content/sequences/square_look.timeline.ron");

/// A zone opens its pane, which lists its placements where the file has them, and a row selects one.
#[test]
fn the_zone_pane_lists_placements_and_a_row_selects_one() {
    let dir = TestDir::new("zone");
    let path = dir.write("z/proving_ground.zone.ron", PROVING_GROUND);
    let mut harness = harness(&path);
    harness.run_steps(2);
    assert_eq!(harness.state().selected_placement(), None);
    let rows = harness.query_all(By::new().label_contains("(9.00, 0.00, 10.00)")).count();
    assert_eq!(rows, 1, "the cart, where the file has it");
    harness.get(By::new().label_contains("(9.00, 0.00, 10.00)")).click();
    harness.run_steps(2);
    assert_eq!(harness.state().selected_placement(), Some(2));
    assert_eq!(text_of(&harness), PROVING_GROUND, "selecting edits nothing");
    assert!(harness.query(By::new().label_contains("(backdrop)")).is_some(), "the ground says what it is");
}

/// A timeline opens the sequencer, whose playhead Play moves a frame of a game's at a time on the pass's clock,
/// stopping at the end, and a press on the tracks puts where it was pressed.
#[test]
fn the_sequencer_plays_and_scrubs_its_playhead() {
    let dir = TestDir::new("sequencer");
    let path = dir.write("s/square_look.timeline.ron", SQUARE_LOOK);
    let mut harness = harness(&path);
    harness.run_steps(2);
    assert_eq!(harness.state().playhead(), Some(0));
    let _ = harness.get(By::new().label_contains("frame 0 of 240"));

    harness.get_by_label("Play").click();
    harness.run_steps(3);
    let playhead = harness.state().playhead().expect("a timeline");
    assert!((60..=120).contains(&playhead), "half a second a step: {playhead}");
    harness.run_steps(12);
    assert_eq!(harness.state().playhead(), Some(240), "it stops at the end");
    let _ = harness.get_by_label("Play");

    let tracks = harness.get_by_label("Tracks").rect();
    harness.state_mut().scrub_to(0);
    harness.run_steps(1);
    let names = 120.0;
    let middle = egui::pos2(tracks.min.x + names + (tracks.width() - names) * 0.5, tracks.center().y);
    harness.drag_at(middle);
    harness.run_steps(1);
    harness.drop_at(middle);
    harness.run_steps(1);
    assert_eq!(harness.state().playhead(), Some(120), "the middle of four seconds");
    assert_eq!(text_of(&harness), SQUARE_LOOK, "scrubbing edits nothing");
}

const CHARACTER_SCREEN: &str = include_str!("../../../content/ui/character.ui.ron");
const CHARACTER_SCREEN_META: &str = include_str!("../../../content/ui/character.ui.ron.meta");
const CHARACTER_MODEL: &str = include_str!("../../../content/ui/character.model.ron");

/// A screen opens its pane, which lists what its model file's paths read, starting at the game's sample model for
/// one of the game's own, and picks the aspect and language the viewport previews it at. None of it edits the
/// document.
#[test]
fn the_screen_pane_lists_its_paths_and_picks_an_aspect() {
    let dir = TestDir::new("screen");
    let path = dir.write("u/character.ui.ron", CHARACTER_SCREEN);
    dir.write("u/character.ui.ron.meta", CHARACTER_SCREEN_META);
    dir.write("u/character.model.ron", CHARACTER_MODEL);
    let mut harness = harness(&path);
    harness.run_steps(2);
    for field in ["attack", "crit", "defense", "detail", "hp", "level", "max_hp", "name"] {
        let _ = harness.get_by_label(field);
    }
    let shown = |harness: &mut Harness<'static, Editor>| {
        let frame = harness.state_mut().viewport_frame();
        frame.screen.map(|screen| (screen.aspect, screen.language.to_owned(), screen.values.to_vec()))
    };
    let (aspect, language, values) = shown(&mut harness).expect("a screen");
    assert_eq!((aspect, language.as_str(), values.len()), (Aspect::Wide, "en", 8));
    assert!(values.contains(&("level".to_owned(), StandIn::Number(12.0))), "the sample's: {values:?}");
    harness.get_by_label("4:3").click();
    harness.get_by_label("ja").click();
    harness.run_steps(2);
    let (aspect, language, _) = shown(&mut harness).expect("a screen");
    assert_eq!((aspect, language.as_str()), (Aspect::Standard, "ja"));
    assert!(harness.state_mut().set_stand_in("hp", StandIn::Number(6240.0)));
    assert!(!harness.state_mut().set_stand_in("no_such_path", StandIn::Bool(true)));
    assert_eq!(text_of(&harness), CHARACTER_SCREEN, "previewing edits nothing");
}

const HUD_MODEL: &str = include_str!("../../../content/ui/hud.model.ron");

/// The pane follows the model file the screen names: an edit naming another lists that one's paths, keeping what
/// was typed for a path both read, and undo brings the first back. A screen that doesn't read yet holds no model
/// until it does.
#[test]
fn the_screen_pane_follows_the_model_its_document_names() {
    let dir = TestDir::new("screen_model");
    let path = dir.write("u/character.ui.ron", "(not a screen yet");
    dir.write("u/character.ui.ron.meta", CHARACTER_SCREEN_META);
    dir.write("u/character.model.ron", CHARACTER_MODEL);
    dir.write("u/hud.model.ron", HUD_MODEL);
    let mut harness = harness(&path);
    harness.run_steps(2);
    assert!(harness.state_mut().viewport_frame().screen.is_none(), "nothing to show yet");
    fs::write(&path, CHARACTER_SCREEN).expect("the screen, saved");
    harness.state_mut().documents_mut().refresh();
    harness.run_steps(2);
    let values = |harness: &mut Harness<'static, Editor>| {
        let frame = harness.state_mut().viewport_frame();
        frame.screen.expect("a screen").values.to_vec()
    };
    let reads = |values: &[(String, StandIn)], path: &str| values.iter().any(|(known, _)| known == path);
    assert!(reads(&values(&mut harness), "crit"), "the character's model, once it reads");
    assert!(harness.state_mut().set_stand_in("level", StandIn::Number(77.0)));

    let id = harness.state().current().expect("open").id();
    let document = harness.state_mut().documents_mut().any_mut(id).expect("open");
    let model = "model".parse().expect("a path");
    document.set_ron("name another model", &model, "Some(\"hud.model.ron\")").expect("an edit");
    harness.run_steps(2);
    let hud = values(&mut harness);
    assert!(reads(&hud, "touch") && !reads(&hud, "crit"), "the hud's: {hud:?}");
    assert!(hud.contains(&("level".to_owned(), StandIn::Number(77.0))), "what was typed: {hud:?}");

    harness.state_mut().undo();
    harness.run_steps(2);
    assert!(reads(&values(&mut harness), "crit"), "undo names the character's again");
    assert_eq!(text_of(&harness), CHARACTER_SCREEN);
}

/// The pane notices its model file changing on disk (M3 Z5c): an outside edit to it reads at the next poll,
/// keeping what was typed, even one that keeps the file's length. A file gone just now keeps the model held; one
/// that doesn't parse holds the preview back until it does again. None of it edits the screen.
#[test]
fn the_screen_pane_reads_its_model_file_again_when_it_changes_on_disk() {
    let dir = TestDir::new("screen_model_disk");
    let path = dir.write("u/character.ui.ron", CHARACTER_SCREEN);
    dir.write("u/character.ui.ron.meta", CHARACTER_SCREEN_META);
    let model = dir.write("u/character.model.ron", CHARACTER_MODEL);
    let mut harness = harness(&path);
    harness.run_steps(2);
    let shown = |harness: &mut Harness<'static, Editor>| {
        let frame = harness.state_mut().viewport_frame();
        frame.screen.map(|screen| screen.values.to_vec())
    };
    let reads = |values: &[(String, StandIn)], path: &str| values.iter().any(|(known, _)| known == path);
    let mut now = 10.0;
    let mut poll = |harness: &mut Harness<'static, Editor>| {
        now += 10.0;
        harness.state_mut().poll(now);
        harness.run_steps(1);
    };
    assert!(reads(&shown(&mut harness).expect("a screen"), "crit"));
    assert!(harness.state_mut().set_stand_in("level", StandIn::Number(77.0)));

    // The same length: only the bytes tell.
    let renamed = CHARACTER_MODEL.replace("\"crit\"", "\"luck\"");
    assert_eq!(renamed.len(), CHARACTER_MODEL.len());
    fs::write(&model, &renamed).expect("an outside edit");
    poll(&mut harness);
    let values = shown(&mut harness).expect("a screen");
    assert!(reads(&values, "luck") && !reads(&values, "crit"), "the edit: {values:?}");
    assert!(values.contains(&("level".to_owned(), StandIn::Number(77.0))), "what was typed: {values:?}");

    fs::remove_file(&model).expect("gone just now");
    poll(&mut harness);
    assert!(reads(&shown(&mut harness).expect("still a screen"), "luck"), "a missing file waits");

    fs::write(&model, "(not a model").expect("a broken edit");
    poll(&mut harness);
    assert!(shown(&mut harness).is_none(), "a model that doesn't parse holds the preview back");

    fs::write(&model, CHARACTER_MODEL).expect("put back");
    poll(&mut harness);
    let values = shown(&mut harness).expect("a screen again");
    assert!(reads(&values, "crit"), "{values:?}");
    assert!(values.contains(&("level".to_owned(), StandIn::Number(77.0))), "typed, all along: {values:?}");
    assert_eq!(text_of(&harness), CHARACTER_SCREEN, "following the model edits nothing");
}
