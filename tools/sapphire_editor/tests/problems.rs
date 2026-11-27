//! The problems pane (M6 T6): an open malformed document is one row on its field, and the row's command, run by a
//! script or by a click, goes to that field in Properties.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, Queryable};
use sapphire_document::{At, Origin};
use sapphire_editor::commands::{Command, SCRIPT_SUFFIX, Script};
use sapphire_editor::shell::Pane;
use sapphire_editor::{Editor, Options};

/// A copy of the document crate's malformed fixtures in a folder of the test's own, gone when it ends.
struct Fixtures(PathBuf);

impl Fixtures {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("sapphire_editor_problems_{name}_{}_{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sapphire_document/test_content/problems");
        for entry in fs::read_dir(from).expect("the fixtures") {
            let entry = entry.expect("a fixture");
            fs::copy(entry.path(), dir.join(entry.file_name())).expect("a copy");
        }
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn options(file: &Path) -> Options {
    Options {
        file: Some(file.to_owned()),
        content: Some(file.with_file_name("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
        pipeline: None,
        game: None,
    }
}

/// Runs `commands` from a script in the fixtures' folder, as `--run` does.
fn run(editor: &mut Editor, fixtures: &Fixtures, commands: &str) {
    let path = fixtures.0.join(format!("twin{SCRIPT_SUFFIX}"));
    fs::write(&path, format!("(version: 1, commands: [{commands}])")).expect("a script");
    let script = Script::read(&path).unwrap_or_else(|error| panic!("{error}"));
    editor.run_script(&script).unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn a_document_that_doesnt_read_is_one_row_on_its_field_and_its_row_goes_there() {
    let fixtures = Fixtures::new("parse");
    let zone = fixtures.path("flat_scale.zone.ron");
    let mut editor = Editor::new(options(&zone));
    let [problem] = editor.content_problems() else { panic!("one row: {:?}", editor.content_problems()) };
    assert_eq!(problem.origin, Origin::Parse);
    assert_eq!(problem.at, At::Field("placements[1].scale".to_owned()));
    let command = Editor::problem_command(problem);
    assert_eq!(command.to_text(), Command::parse(&command.to_text()).expect("it reads back").to_text());
    editor.run(command).expect("it goes there");
    let field = editor.selected_field().expect("a field");
    assert_eq!(
        field.path.to_string(),
        "placements[1].scale",
        "the text's own path: there's no value to follow"
    );
}

/// The script twin of the kittest below: the same command, written as a person writes it.
#[test]
fn a_rules_issue_goes_to_its_field_by_the_entrys_name() {
    let fixtures = Fixtures::new("check");
    let foliage = fixtures.path("unknown_kind.foliage.ron");
    let mut editor = Editor::new(options(&fixtures.path("loud.cue.ron")));
    assert_eq!(editor.content_problems().len(), 1, "the cue's own: {:?}", editor.content_problems());
    run(
        &mut editor,
        &fixtures,
        r#"SelectField((document: "unknown_kind.foliage.ron", path: "rules[1].kind"))"#,
    );
    let field = editor.selected_field().expect("a field");
    assert_eq!(field.path.to_string(), r#"rules["maples"].kind"#);
    let opened = editor.documents().path(field.document).expect("open").canonicalize().expect("a file");
    assert_eq!(opened, foliage.canonicalize().expect("a file"), "it opened the document");
    let rows: Vec<String> = editor.content_problems().iter().map(ToString::to_string).collect();
    assert_eq!(rows.len(), 2, "the cue's and the foliage's: {rows:?}");
    assert!(rows.iter().any(|row| row.starts_with("rules[1].kind: ")), "{rows:?}");

    // A path the document hasn't is refused, and leaves no field selected.
    let picked = sapphire_editor::commands::Picked { document: foliage, path: "rules[9].kind".to_owned() };
    let refused = editor.run(Command::SelectField(picked));
    assert!(refused.is_err());
    assert!(editor.selected_field().is_none());
}

#[test]
fn a_click_on_a_row_goes_to_its_field() {
    let fixtures = Fixtures::new("click");
    let foliage = fixtures.path("unknown_kind.foliage.ron");
    let mut harness = Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), Editor::new(options(&foliage)));
    harness.state_mut().run(Command::OpenPane(Pane::Problems)).expect("the pane");
    harness.run_steps(4);
    let text = harness.state().content_problems()[0].to_string();
    harness.get(By::new().role(Role::Button).label(&text)).click();
    harness.run_steps(4);
    let field = harness.state().selected_field().expect("a field");
    assert_eq!(field.path.to_string(), r#"rules["maples"].kind"#);
}
