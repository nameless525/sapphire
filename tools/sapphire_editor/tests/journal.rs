//! The journal against a kill (M6 T3): an editor in a child process, with no window, edits three documents,
//! saves none of them, and is killed in the middle of a drag. A new editor offers every text it hadn't saved,
//! unchanged, and the one whose file someone saved meanwhile as a conflict. No source is written by any of it.
//!
//! The child is this test binary run again on [`child_edits_then_waits_to_be_killed`] alone, which does nothing
//! unless the variable [`CHILD`] names its folder.
//!
//! The rest hold what a window's frames write (once a frame at most, a typed value on the frame it lands) and
//! what an entry is taken back as (its own text's kind, wherever its file went).

use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;
use std::{env, fs};

use egui::{Key, Modifiers, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{By, Queryable};
use sapphire_assets::cue::Cue;
use sapphire_document::{Journal, JournalEntry};
use sapphire_editor::commands::{Command, Script};
use sapphire_editor::{Editor, Options};
use sapphire_time::Stopwatch;

const PLAYER: &str = include_str!("../../../content/combat/player/player.moves.ron");
const WALL: &str = include_str!("../../../content/samples/environment/wall.material.ron");
const MIST: &str = include_str!("../../../content/samples/environment/mist.material.ron");
const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");
const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
const CUE: &str = include_str!("../../../content/audio/hit_heavy.cue.ron");

/// Names the child's folder, and says it's the child.
const CHILD: &str = "SAPPHIRE_EDITOR_JOURNAL_CHILD";
/// The documents, by their names in the folder.
const NAMES: [&str; 3] = ["warrior.moves.ron", "hit_heavy.cue.ron", "slice.moves.ron"];
/// What the child does: a drag ended on the warrior, a number set on the cue, and a drag held on the slice.
const EDITS: &str = r#"(version: 1, commands: [
    Begin("drag a window"), SetWindow(entry: (mv: 0, track: Hitboxes, index: 0), from: 4, to: 9), End,
    Open("hit_heavy.cue.ron"), SetProperty(path: "volume_db[0]", value: "-6.0"),
    Open("slice.moves.ron"), Begin("drag a window"),
    SetWindow(entry: (mv: 0, track: Hitboxes, index: 0), from: 5, to: 9),
    SetWindow(entry: (mv: 0, track: Hitboxes, index: 0), from: 5, to: 10),
])"#;
/// How long the child waits to be killed before it gives up on its parent.
const CHILD_WAITS: Duration = Duration::from_secs(120);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            env::temp_dir().join(format!("sapphire_editor_journal_{name}_{}_{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An editor as `--run` starts one, on `file` if there's one, keeping its journal in the folder's `journal`.
fn editor(dir: &Path, file: Option<PathBuf>) -> Editor {
    let mut editor = Editor::new(Options {
        file,
        content: Some(dir.join("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
        pipeline: None,
        game: None,
    });
    editor.keep_journal_at(&dir.join("journal"));
    assert!(editor.journal().is_some(), "{:?}", editor.notice());
    editor
}

/// The child: runs [`EDITS`], writes what each document holds to `expected/`, then `ready`, and waits.
#[test]
fn child_edits_then_waits_to_be_killed() {
    let Some(dir) = env::var_os(CHILD).map(PathBuf::from) else { return };
    let mut editor = editor(&dir, Some(dir.join(NAMES[0])));
    let script_path = dir.join("edits.commands.ron");
    fs::write(&script_path, EDITS).expect("the script");
    let script = Script::read(&script_path).expect("reads");
    editor.run_script(&script).expect("runs");
    assert!(editor.current().expect("the slice").in_gesture(), "killed mid-gesture");
    let expected = dir.join("expected");
    fs::create_dir_all(&expected).expect("a folder");
    for id in editor.documents().ids() {
        let documents = editor.documents();
        let name = documents.path(id).and_then(Path::file_name).expect("a name");
        fs::write(expected.join(name), documents.any(id).expect("open").text()).expect("written");
    }
    fs::write(dir.join("ready"), b"").expect("ready");
    let waiting = Stopwatch::start();
    while waiting.elapsed() < CHILD_WAITS {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::process::exit(3);
}

/// The parent: kills the child mid-gesture, then takes back what it kept.
#[test]
fn an_editor_killed_mid_gesture_loses_no_unsaved_text() {
    let dir = TestDir::new("killed");
    let originals = [WARRIOR, CUE, SLICE];
    for (name, text) in NAMES.iter().zip(originals) {
        fs::write(dir.0.join(name), text).expect("a document");
    }
    let mut child = Process::new(env::current_exe().expect("this test"))
        .args(["child_edits_then_waits_to_be_killed", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, &dir.0)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the child starts");
    let waiting = Stopwatch::start();
    while !dir.0.join("ready").exists() {
        if let Some(status) = child.try_wait().expect("the child") {
            let mut said = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut stderr, &mut said);
            }
            panic!("the child stopped first, {status}: {said}");
        }
        assert!(waiting.elapsed() < CHILD_WAITS, "the child never got to its gesture");
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().expect("killed");
    let status = child.wait().expect("gone");
    assert!(!status.success(), "it was killed: {status}");

    let read = |name: &str| fs::read_to_string(dir.0.join(name)).expect("a file");
    for (name, text) in NAMES.iter().zip(originals) {
        assert_eq!(read(name), text, "{name}: no source written");
    }
    // Someone else saves the cue meanwhile, changing another of its numbers.
    let theirs = CUE.replacen("pitch: (0.78, 0.88)", "pitch: (0.7, 0.9)", 1);
    assert_ne!(theirs, CUE);
    fs::write(dir.0.join(NAMES[1]), &theirs).expect("someone else's save");

    let mut editor = editor(&dir.0, None);
    let offers = editor.journal().expect("kept").offers().len();
    assert_eq!(offers, 3, "one for each unsaved document");
    for name in NAMES {
        let path = dir.0.join(name);
        editor.run(Command::Recover(path.clone())).unwrap_or_else(|why| panic!("{name}: {why}"));
        let current = editor.current().expect("taken back");
        assert_eq!(current.path(), path);
        assert_eq!(current.text(), read(&format!("expected/{name}")), "{name}: unchanged");
        assert!(current.is_dirty(), "{name}: unsaved");
        assert_eq!(current.conflict().is_some(), name == NAMES[1], "{name}: a conflict only if it moved on");
    }
    assert!(editor.journal().expect("kept").offers().is_empty());
    for (name, text) in NAMES.iter().zip([WARRIOR, &theirs, SLICE]) {
        assert_eq!(read(name), text, "{name}: still no source written");
    }

    // The cue's edit goes back on top of theirs, and the file waits for a save.
    editor.run(Command::Open(dir.0.join(NAMES[1]))).expect("the cue");
    editor.run(Command::MineOnTop).expect("rebased");
    let rebased = editor.current().expect("the cue").text().to_owned();
    let cue: Cue = sapphire_serialize::from_ron(&rebased).expect("reads");
    assert_eq!((cue.volume_db, cue.pitch), ([-6.0, -1.0], [0.7, 0.9]), "the edit, and theirs");
    assert_eq!(read(NAMES[1]), theirs);
    editor.run(Command::SaveAll).expect("saved");
    assert_eq!(read(NAMES[1]), rebased, "a save writes it, and only a save");
    editor.close_journal();
    assert_eq!(fs::read_dir(dir.0.join("journal")).expect("the journal").count(), 0, "nothing left");
}

fn harness(editor: Editor) -> Harness<'static, Editor> {
    Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), editor)
}

/// The entry this session holds of the one document with unsaved text.
fn the_entry(journal: &Journal) -> JournalEntry {
    let mut entries: Vec<_> = fs::read_dir(journal.session())
        .expect("its folder")
        .filter_map(|file| file.ok().map(|file| file.path()))
        .filter(|file| file.to_string_lossy().ends_with(".journal.ron"))
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    JournalEntry::parse(&fs::read_to_string(entries.remove(0)).expect("an entry")).expect("it reads")
}

/// A drag held for 120 frames of a window writes its document's entry once a frame at most, not once a command
/// (the drag runs two each frame it moves), and what a kill would leave is the text as it stands.
#[test]
fn a_drag_writes_its_entry_once_a_frame_at_most() {
    let dir = TestDir::new("drag");
    let path = dir.0.join("player.moves.ron");
    fs::write(&path, PLAYER).expect("a document");
    let mut editor = editor(&dir.0, Some(path));
    // Kept only to count the commands the frames ran.
    editor.record_to(dir.0.join("count.commands.ron"));
    let mut harness = harness(editor);
    harness.run_steps(3);
    let commands =
        |harness: &Harness<'_, Editor>| harness.state().recorded().expect("recording").commands.len();
    let writes = |harness: &Harness<'_, Editor>| harness.state().journal().expect("kept").stats().writes;
    let (ran, wrote) = (commands(&harness), writes(&harness));

    let start = harness.get_by_label("hit volumes light_1_hit 7..11 start").rect().center();
    let end = harness.get_by_label("hit volumes light_1_hit 7..11 end").rect().center();
    let per_tick = (end.x - start.x + 2.0) / 4.0;
    harness.hover_at(end);
    harness.run_steps(2);
    harness.drag_at(end);
    harness.step();
    let mut frames = 3;
    for frame in 0..120 {
        let ticks = if frame % 2 == 0 { 1.0 } else { 2.0 };
        harness.hover_at(end + egui::vec2(ticks * per_tick, 0.0));
        harness.step();
        frames += 1;
    }
    assert!(harness.state().current().expect("the moves").in_gesture(), "held");
    let (ran, wrote) = ((commands(&harness) - ran) as u64, writes(&harness) - wrote);
    assert!(ran >= 200, "the drag ran a command or two a frame: {ran}");
    assert!(wrote <= frames, "once a frame at most: {wrote} writes over {frames} frames");
    assert!(wrote * 3 <= ran * 2, "fewer writes than commands: {wrote} for {ran}");
    let entry = the_entry(harness.state().journal().expect("kept"));
    assert_eq!(entry.text, harness.state().current().expect("the moves").text(), "the drag as it stands");
    harness.drop_at(end + egui::vec2(2.0 * per_tick, 0.0));
    harness.run_steps(3);
}

/// A value typed in that lands when its field lets go is in the journal at the end of the frame it lands on,
/// not at the next look at the files, a second on.
#[test]
fn a_typed_value_is_journaled_on_the_frame_it_lands() {
    let dir = TestDir::new("typed");
    let path = dir.0.join("player.moves.ron");
    fs::write(&path, PLAYER).expect("a document");
    let mut harness = harness(editor(&dir.0, Some(path)));
    harness.run_steps(3);
    harness.get_by_label("cues swing at 7").click();
    harness.run_steps(3);
    harness.get(By::new().role(Role::TextInput).value("\"swing\"")).focus();
    harness.run_steps(1);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.run_steps(1);
    harness
        .get_all(By::new().role(Role::TextInput))
        .find(|field| field.is_focused())
        .expect("a field has the keyboard")
        .type_text("\"swung\"");
    harness.run_steps(2);
    assert!(!harness.state().current().expect("the moves").text().contains("\"swung\""), "still being typed");
    harness.key_press(Key::Enter);
    let mut frames = 0;
    while !harness.state().current().expect("the moves").text().contains("\"swung\"") {
        assert!(frames < 4, "it lands when the field lets go");
        harness.step();
        frames += 1;
    }
    let entry = the_entry(harness.state().journal().expect("kept"));
    assert!(entry.text.contains("(name: \"swung\", at: 7)"), "kept on the frame it landed");
}

/// An entry is taken back as its own text's kind: a material whose file moved, or went, since the editor that
/// kept it stopped comes back as a material that reads, and its save writes a material's sidecar. One of no kind
/// is refused by its path and stays offered.
#[test]
fn an_entry_comes_back_as_its_own_texts_kind_wherever_its_file_went() {
    let unsaved = WALL.replacen("roughness: 0.9,", "roughness: 0.5,", 1);
    assert_ne!(unsaved, WALL);
    for moved in [true, false] {
        let dir = TestDir::new(if moved { "material_moved" } else { "material_gone" });
        let path = dir.0.join("wall.material.ron");
        fs::write(&path, WALL).expect("a material");
        let mut first = editor(&dir.0, Some(path.clone()));
        first
            .run(Command::SetProperty { path: "roughness".to_owned(), value: "0.5".to_owned() })
            .expect("set");
        assert_eq!(first.current().expect("the material").text(), unsaved);
        // Stopped as a crash stops it: the journal's session is left.
        drop(first);

        let sidecar = dir.0.join("wall.material.ron.meta");
        let now = if moved {
            fs::create_dir_all(dir.0.join("elsewhere")).expect("a folder");
            let now = dir.0.join("elsewhere").join("wall.material.ron");
            fs::rename(&path, &now).expect("moved");
            fs::rename(&sidecar, now.with_extension("ron.meta")).expect("with its sidecar");
            now
        } else {
            fs::remove_file(&path).expect("gone");
            fs::remove_file(&sidecar).expect("its sidecar too");
            path.clone()
        };
        let mut editor = editor(&dir.0, None);
        editor.run(Command::Recover(path.clone())).unwrap_or_else(|why| panic!("moved {moved}: {why}"));
        let current = editor.current().expect("taken back");
        assert_eq!(current.path(), now);
        assert_eq!(current.text(), unsaved);
        // As moves, which it was taken back as before, it wouldn't read.
        assert_eq!(current.document().failure_message(), None, "it reads, as the material it is");
        editor.run(Command::Save).expect("saved");
        assert_eq!(fs::read_to_string(&now).expect("written"), unsaved);
        let meta = fs::read_to_string(now.with_extension("ron.meta")).expect("its sidecar");
        assert!(meta.contains("importer: Material(())"), "a material's sidecar: {meta}");
        editor.close_journal();
    }

    // A fog card's material is no kind this editor edits: an entry of it is refused, and stays offered.
    let dir = TestDir::new("no_kind");
    let path = dir.0.join("mist.material.ron");
    fs::write(&path, MIST).expect("a material of no kind");
    let ended = dir.0.join("journal").join("ended-0");
    fs::create_dir_all(&ended).expect("an ended session");
    fs::write(ended.join("session.lock"), b"").expect("its lock, let go");
    let id: sapphire_assets::AssetId = "a4cfe9cc-8e05-4940-92ad-922472ccb36a".parse().expect("an ID");
    let entry = JournalEntry::new(id, path.clone(), MIST.as_bytes(), MIST.replacen("0.6)", "0.5)", 1), None);
    fs::write(ended.join(format!("{id}.journal.ron")), entry.to_text()).expect("its entry");
    let mut editor = editor(&dir.0, None);
    assert_eq!(editor.journal().expect("kept").offers().len(), 1);
    let why = editor.run(Command::Recover(path.clone())).expect_err("refused");
    assert!(why.contains("mist.material.ron") && why.contains("no kind"), "{why}");
    assert_eq!(editor.journal().expect("kept").offers().len(), 1, "left for a person");
    assert!(editor.current().is_none(), "nothing opened");
    assert!(!dir.0.join("mist.material.ron.meta").exists(), "no sidecar written");
}

/// The entry the frame tests edit: the warrior's first hit volume.
const HIT: &str = "(mv: 0, track: Hitboxes, index: 0)";

/// Runs `commands` (a script's, without its envelope) outside a frame.
fn run(editor: &mut Editor, commands: &str) {
    let script = Script::parse(&format!("(version: 1, commands: [{commands}])")).expect("a script");
    editor.run_script(&script).expect("it ran");
}

/// A frame that changes a document's text twice (Cmd+Z, then Alt+Right, each a command) writes its entry once:
/// at the frame's end, not at each command's, which wrote it twice a frame.
#[test]
fn a_frame_of_two_edits_writes_its_entry_once() {
    let dir = TestDir::new("two_edits");
    let path = dir.0.join("warrior.moves.ron");
    fs::write(&path, WARRIOR).expect("a document");
    let mut editor = editor(&dir.0, Some(path));
    run(&mut editor, &format!("Select(Some({HIT})), Nudge(1), Nudge(1)"));
    // Kept only to count the commands the frames ran.
    editor.record_to(dir.0.join("count.commands.ron"));
    // A pass of the editor's interface is a frame: egui can run a step's more than once.
    let passes = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&passes);
    let mut harness =
        Harness::builder().with_size((1400.0, 900.0)).with_accessibility_check(false).build_ui_state(
            move |ui, editor: &mut Editor| {
                counted.fetch_add(1, Ordering::Relaxed);
                editor.ui(ui);
            },
            editor,
        );
    harness.run_steps(3);
    let writes = |harness: &Harness<'_, Editor>| harness.state().journal().expect("kept").stats().writes;
    let commands =
        |harness: &Harness<'_, Editor>| harness.state().recorded().expect("recording").commands.len() as u64;
    let before = harness.state().current().expect("the moves").text().to_owned();
    let (wrote, ran, frames) = (writes(&harness), commands(&harness), passes.load(Ordering::Relaxed));
    // Both presses in one frame's input: kittest's own `key_press` gives each event a frame of its own.
    let press = |modifiers, key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    for _ in 0..40 {
        let events = &mut harness.input_mut().events;
        events.push(press(Modifiers::COMMAND, Key::Z));
        events.push(press(Modifiers::ALT, Key::ArrowRight));
        harness.step();
    }
    let (wrote, ran) = (writes(&harness) - wrote, commands(&harness) - ran);
    let frames = passes.load(Ordering::Relaxed) - frames;
    assert_eq!(
        harness.state().current().expect("the moves").text(),
        before,
        "each frame took back and nudged"
    );
    assert!(ran >= 80, "two edits a step at least: {ran}");
    assert!(wrote >= 20, "each frame's change is written: {wrote} over {frames} frames");
    assert!(wrote <= frames, "once a frame at most: {wrote} writes over {frames} frames");
    assert!(wrote * 2 <= ran, "not once an edit: {wrote} writes for {ran} edits");
    assert_eq!(the_entry(harness.state().journal().expect("kept")).text, before, "what a kill would leave");
}

/// A journal whose folder went fails each write, and is tried again once a second, not each frame, warning once;
/// once the folder's back, the next try writes what's unsaved.
#[test]
fn a_failing_journal_is_tried_once_a_second_until_it_writes() {
    let dir = TestDir::new("failing");
    let path = dir.0.join("warrior.moves.ron");
    fs::write(&path, WARRIOR).expect("a document");
    let mut editor = editor(&dir.0, Some(path));
    run(&mut editor, &format!("Select(Some({HIT})), Nudge(1)"));
    let session = editor.journal().expect("kept").session().to_owned();
    fs::remove_dir_all(&session).expect("the session's folder goes");
    run(&mut editor, "Nudge(1)");
    assert!(editor.journal_failing(), "{:?}", editor.notice());
    let mut harness = harness(editor);
    let failed = |harness: &Harness<'_, Editor>| harness.state().journal().expect("kept").stats().failed;
    let before = failed(&harness);
    // 40 frames of kittest's quarter of a second: ten seconds, so about ten tries, not forty.
    for _ in 0..40 {
        harness.step();
    }
    let tried = failed(&harness) - before;
    assert!((5..=12).contains(&tried), "once a second: {tried} tries over 40 frames of ten seconds");
    assert!(harness.state().journal_failing());
    fs::create_dir_all(&session).expect("the folder's back");
    harness.run_steps(8);
    assert!(!harness.state().journal_failing(), "{:?}", harness.state().notice());
    let text = harness.state().current().expect("the moves").text().to_owned();
    assert_eq!(the_entry(harness.state().journal().expect("kept")).text, text, "written once it could be");
}

/// A recording whose folder went is tried again once a second until it writes, not each frame, and not given
/// up on: the first failed write doesn't count it written.
#[test]
fn a_failing_recording_is_tried_once_a_second_until_it_writes() {
    let dir = TestDir::new("recording");
    let path = dir.0.join("warrior.moves.ron");
    fs::write(&path, WARRIOR).expect("a document");
    let mut editor = editor(&dir.0, Some(path));
    let folder = dir.0.join("recorded");
    fs::create_dir_all(&folder).expect("a folder");
    let file = folder.join("session.commands.ron");
    editor.record_to(file.clone());
    run(&mut editor, &format!("Select(Some({HIT}))"));
    fs::remove_dir_all(&folder).expect("the recording's folder goes");
    run(&mut editor, "Nudge(1)");
    let mut harness = harness(editor);
    let writes = |harness: &Harness<'_, Editor>| harness.state().recording_writes().expect("recording");
    let before = writes(&harness);
    for _ in 0..40 {
        harness.step();
    }
    let tried = writes(&harness) - before;
    assert!((5..=12).contains(&tried), "once a second: {tried} tries over 40 frames of ten seconds");
    fs::create_dir_all(&folder).expect("the folder's back");
    harness.run_steps(8);
    let kept =
        Script::parse(&fs::read_to_string(&file).expect("written once it could be")).expect("it reads");
    assert_eq!(kept.commands.last(), Some(&Command::Nudge(1)), "{kept:?}");
}
