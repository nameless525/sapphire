//! The editor's interface, driven as a person drives it with no window: `egui_kittest` finds its widgets
//! by what they're called and sends real pointer and key events, and everything goes through
//! [`Editor::ui`], as the window's frames do. Edits land in temporary copies of the real documents.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use sapphire_editor::capture::{self, Textures};
use sapphire_editor::{Editor, Options};
use sapphire_moves::MoveSetDocument;

const PLAYER: &str = include_str!("../../../content/combat/player/player.moves.ron");
const PLAYER_META: &str = include_str!("../../../content/combat/player/player.moves.ron.meta");
const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");

/// A directory of this test's own, gone when it ends.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sapphire_editor_ui_{name}_{}_{unique}", std::process::id()));
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
    let lit = image.pixels.chunks_exact(4).filter(|pixel| pixel[0] > 20).count();
    assert!(lit > 100_000, "{lit} lit pixels");
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
    assert_eq!(MoveSetDocument::parse(PLAYER).expect("parses").moves.len(), 6);
}
