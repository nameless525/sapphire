//! The docked panes under `egui_kittest`, with no GPU and no window: they split, close, drag and last as RON,
//! with real pointer events on the tabs. The first half has nothing behind the panes; the second is the editor's.

use std::fs;
use std::path::PathBuf;

use egui::{Pos2, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_tiles::{Container, LinearDir, Tile};
use sapphire_editor::shell::{Pane, PaneUi, Shell};
use sapphire_editor::{Editor, Options};

/// Draws each pane as its name, and remembers where.
#[derive(Default)]
struct Recorded {
    drawn: Vec<(Pane, Rect)>,
}

impl PaneUi for Recorded {
    fn pane_ui(&mut self, ui: &mut egui::Ui, pane: Pane) {
        self.drawn.push((pane, ui.max_rect()));
        ui.label(format!("inside {}", pane.title()));
    }
}

struct State {
    shell: Shell,
    panes: Recorded,
}

impl State {
    fn rect_of(&self, pane: Pane) -> Rect {
        self.panes.drawn.iter().find(|(drawn, _)| *drawn == pane).expect("drawn").1
    }
}

fn harness() -> Harness<'static, State> {
    Harness::builder().with_size(egui::vec2(1200.0, 700.0)).build_ui_state(
        |ui, state: &mut State| {
            state.panes.drawn.clear();
            state.shell.ui(ui, &mut state.panes);
        },
        State { shell: Shell::new(), panes: Recorded::default() },
    )
}

fn titles(shell: &Shell) -> Vec<&'static str> {
    let mut titles: Vec<_> = shell
        .tree
        .tiles
        .tiles()
        .filter_map(|tile| match tile {
            Tile::Pane(pane) => Some(pane.title()),
            Tile::Container(_) => None,
        })
        .collect();
    titles.sort_unstable();
    titles
}

fn press_and_let_go(harness: &mut Harness<'static, State>, at: Pos2) {
    harness.hover_at(at);
    harness.run_steps(2);
    harness.drag_at(at);
    harness.run_steps(1);
    harness.drop_at(at);
    harness.run_steps(4);
}

#[test]
fn every_pane_is_open_with_a_tab_and_those_showing_draw() {
    let mut harness = harness();
    harness.run_steps(3);
    assert_eq!(titles(&harness.state().shell).len(), Pane::ALL.len());
    for pane in Pane::ALL {
        assert!(harness.query_by_label(pane.title()).is_some(), "{pane:?} has a tab");
        assert!(harness.state().shell.is_open(pane), "{pane:?}");
    }
    let preview = harness.state().rect_of(Pane::Preview);
    assert!(preview.width() > 500.0 && preview.height() > 250.0, "{preview:?}");
    let timeline = harness.state().rect_of(Pane::Timeline);
    assert!(timeline.top() >= preview.bottom(), "the preview over the timeline: {preview:?} {timeline:?}");
    assert!(harness.state().rect_of(Pane::Browser).right() <= preview.left());
    assert!(harness.state().rect_of(Pane::Inspector).left() >= preview.right());
    // A tab away isn't drawn, so what asks a device for its cvars asks only when it's looked at.
    assert!(!harness.state().panes.drawn.iter().any(|(pane, _)| *pane == Pane::Cvars));
}

#[test]
fn a_tab_switches_what_shows() {
    let mut harness = harness();
    harness.run_steps(3);
    assert!(harness.query_by_label("inside Properties").is_none());
    harness.get_by_label("Properties").click();
    harness.run_steps(3);
    assert!(harness.query_by_label("inside Properties").is_some());
    assert!(harness.query_by_label("inside Inspector").is_none());
}

#[test]
fn a_pane_splits_by_call_and_the_tree_draws_it() {
    let mut harness = harness();
    harness.run_steps(2);
    let before = harness.state().rect_of(Pane::Preview);
    let shell = &mut harness.state_mut().shell;
    let preview = shell.find(Pane::Preview).expect("open");
    shell.split(preview, Pane::Device, LinearDir::Vertical);
    harness.run_steps(3);
    assert_eq!(titles(&harness.state().shell).len(), Pane::ALL.len(), "it moved");
    let after = harness.state().rect_of(Pane::Preview);
    assert!(after.height() < before.height() * 0.7, "{before:?} against {after:?}");
    assert!((after.width() - before.width()).abs() < 2.0, "{before:?} against {after:?}");
    assert!(harness.state().rect_of(Pane::Device).top() >= after.bottom());
}

/// Every tab's close button is a node called the same, with nothing to say whose. It's 12 points across, 8 from
/// the tab's right end (`Behavior::close_button_outer_size` and `tab_title_spacing`), so a test clicks there.
#[test]
fn a_pane_closes_by_its_tabs_close_button_and_comes_back() {
    let mut harness = harness();
    harness.run_steps(3);
    let tab = harness.get_by_label("Device").rect();
    press_and_let_go(&mut harness, Pos2::new(tab.right() - 14.0, tab.center().y));
    assert!(harness.query_by_label("Device").is_none(), "the tab at {tab:?}");
    assert!(!harness.state().shell.is_open(Pane::Device));
    assert_eq!(titles(&harness.state().shell).len(), Pane::ALL.len() - 1);

    harness.state_mut().shell.open(Pane::Device);
    harness.run_steps(3);
    assert!(harness.query_by_label("inside Device").is_some());

    // Opening what's open shows it: it's a tab away, and opens nothing new.
    harness.state_mut().shell.open(Pane::Cvars);
    harness.run_steps(3);
    assert!(harness.query_by_label("inside Cvars").is_some());
    assert_eq!(titles(&harness.state().shell).len(), Pane::ALL.len());
}

/// Every pane closed is no trouble, and the first back has the room.
#[test]
fn closing_everything_leaves_a_way_back() {
    let mut harness = harness();
    harness.run_steps(2);
    for pane in Pane::ALL {
        harness.state_mut().shell.close(pane);
        harness.run_steps(2);
    }
    assert!(titles(&harness.state().shell).is_empty());
    harness.state_mut().shell.open(Pane::Timeline);
    harness.run_steps(3);
    assert!(harness.query_by_label("inside Timeline").is_some());
    assert!(harness.state().rect_of(Pane::Timeline).width() > 1000.0);
}

#[test]
fn a_tab_dragged_to_a_panes_edge_splits_it() {
    let mut harness = harness();
    harness.run_steps(3);
    let tab = harness.get_by_label("Cvars").rect();
    let from = tab.center();
    let preview = harness.state().rect_of(Pane::Preview);
    let to = Pos2::new(preview.center().x, preview.bottom() - 10.0);
    harness.hover_at(from);
    harness.run_steps(1);
    harness.drag_at(from);
    harness.run_steps(2);
    for step in 1..=10 {
        harness.hover_at(from.lerp(to, step as f32 / 10.0));
        harness.run_steps(1);
    }
    harness.drop_at(to);
    harness.run_steps(4);
    let after = harness.state().rect_of(Pane::Preview);
    assert!(after.height() < preview.height() * 0.8, "{preview:?} against {after:?}");
    let shell = &harness.state().shell;
    let cvars = shell.find(Pane::Cvars).expect("still there");
    let holder = shell.tree.tiles.parent_of(cvars).expect("held");
    let grandparent = shell.tree.tiles.parent_of(holder).expect("a tab in a split");
    assert!(
        matches!(shell.tree.tiles.get_container(grandparent), Some(Container::Linear(linear)) if linear.dir == LinearDir::Vertical),
        "{:#?}",
        shell.tree
    );
}

#[test]
fn a_layout_goes_to_ron_and_back_the_same() {
    let mut harness = harness();
    harness.run_steps(2);
    let shell = &mut harness.state_mut().shell;
    let preview = shell.find(Pane::Preview).expect("open");
    shell.split(preview, Pane::Device, LinearDir::Vertical);
    shell.close(Pane::Cvars);
    harness.run_steps(3);
    let saved = harness.state().shell.save_layout().expect("saves");
    let tree = harness.state().shell.tree.clone();
    let rect = harness.state().rect_of(Pane::Preview);

    let mut restored = self::harness();
    restored.run_steps(2);
    restored.state_mut().shell.restore_layout(&saved).expect("restores");
    restored.run_steps(3);
    assert_eq!(restored.state().shell.tree, tree, "the same tree");
    assert_eq!(restored.state().rect_of(Pane::Preview), rect, "drawn the same");
    assert!(restored.query_by_label("Cvars").is_none());
    // The same tree, though not always the same bytes: `Tiles` keeps a hash map, so its order varies. A layout
    // isn't a document, so this doesn't matter unless one gets committed.
    let again = restored.state().shell.save_layout().expect("saves");
    assert_eq!(ron::from_str::<egui_tiles::Tree<Pane>>(&again).expect("reads"), tree);
    assert!(saved.len() < 8192, "{}", saved.len());
}

#[test]
fn a_layout_that_doesnt_read_leaves_the_one_there() {
    let mut harness = harness();
    harness.run_steps(2);
    let tree = harness.state().shell.tree.clone();
    assert!(harness.state_mut().shell.restore_layout("(not a layout").is_err());
    assert_eq!(harness.state().shell.tree, tree);
}

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire_editor_docking_{name}_{}", std::process::id()));
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

fn editor_harness(dir: &TestDir) -> Harness<'static, Editor> {
    let mut editor = Editor::new(Options {
        file: None,
        content: Some(dir.0.join("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
    });
    editor.keep_layout_at(dir.0.join("layout.ron"));
    Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .with_step_dt(0.5)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), editor)
}

/// The editor keeps what its panes were left as, and the Panes menu brings back what was closed.
#[test]
fn the_editors_layout_lasts_past_the_editor() {
    let dir = TestDir::new("lasts");
    let file = dir.0.join("layout.ron");
    let mut harness = editor_harness(&dir);
    harness.run_steps(4);
    assert!(!file.exists(), "nothing's changed, so nothing's written");

    let tab = harness.get_by_label("Device").rect();
    let close = Pos2::new(tab.right() - 14.0, tab.center().y);
    harness.hover_at(close);
    harness.run_steps(2);
    harness.drag_at(close);
    harness.run_steps(1);
    harness.drop_at(close);
    harness.run_steps(6);
    assert!(!harness.state().shell().is_open(Pane::Device));
    let saved = fs::read_to_string(&file).expect("kept");
    assert_eq!(fs::read_dir(&dir.0).expect("lists").count(), 1, "nothing beside it");
    drop(harness);

    let mut harness = editor_harness(&dir);
    harness.run_steps(3);
    assert!(!harness.state().shell().is_open(Pane::Device), "from {saved}");
    assert!(harness.query_by_label("Device").is_none());

    harness.get_by_label("Panes").click();
    harness.run_steps(2);
    harness.get_by_label("Device").click();
    harness.run_steps(3);
    assert!(harness.state().shell().is_open(Pane::Device));
}

/// A bad file is a notice and the usual layout, and it stays until the layout changes.
#[test]
fn a_kept_layout_that_doesnt_read_says_so_and_starts_from_the_usual() {
    let dir = TestDir::new("bad");
    let file = dir.0.join("layout.ron");
    fs::write(&file, "(not a layout").expect("written");
    let mut harness = editor_harness(&dir);
    harness.run_steps(4);
    assert!(harness.state().notice().is_some_and(|notice| notice.text.contains("layout.ron")));
    assert_eq!(titles(harness.state().shell()).len(), Pane::ALL.len());
    assert_eq!(fs::read_to_string(&file).expect("still there"), "(not a layout");
}
