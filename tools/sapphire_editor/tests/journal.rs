//! The journal against a kill (M6 T3): an editor in a child process, with no window, edits three documents,
//! saves none of them, and is killed in the middle of a drag. A new editor offers every text it hadn't saved,
//! unchanged, and the one whose file someone saved meanwhile as a conflict. No source is written by any of it.
//!
//! The child is this test binary run again on [`child_edits_then_waits_to_be_killed`] alone, which does nothing
//! unless the variable [`CHILD`] names its folder.

use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use std::{env, fs};

use sapphire_assets::cue::Cue;
use sapphire_editor::commands::{Command, Script};
use sapphire_editor::{Editor, Options};
use sapphire_time::Stopwatch;

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
