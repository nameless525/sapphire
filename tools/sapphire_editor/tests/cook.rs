//! Cooking from the editor (M6 T5) with the real pipeline: a save cooks, a refused material shows its refusal on
//! its field, and the editor's cook writes the bytes `cargo xtask content` does.
//!
//! They run the pipeline `cargo xtask content` builds (`target/tools/sapphire_pipeline`), which must be this tree's,
//! so they're ignored: `cargo xtask content --cook editor`, then
//! `cargo test -p sapphire_editor --test cook -- --ignored --test-threads=1`. The editor's own counts and timing
//! of cooks are `sapphire_editor::cook`'s unit tests, with no process.

use std::fs;
use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use sapphire_editor::commands::{SCRIPT_SUFFIX, Script};
use sapphire_editor::cook::{CookPlan, Cooker, Pipeline};
use sapphire_editor::shell::Pane;
use sapphire_editor::{Editor, Options};

const SWATCH: &str = include_str!("../../../content/samples/swatch_bands.material.ron");
const SWATCH_META: &str = include_str!("../../../content/samples/swatch_bands.material.ron.meta");

/// An ID no source has.
const NOWHERE: &str = "5e1f0c2a-6b7d-4f1e-9a3c-2d8e4b6f0a17";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).expect("the workspace").to_owned()
}

/// The pipeline `cargo xtask content` builds.
fn pipeline() -> PathBuf {
    root().join(format!("target/tools/sapphire_pipeline{}", std::env::consts::EXE_SUFFIX))
}

/// A folder of the test's own, laid out as a repository is: `content/` and `target/content/editor/`.
struct Tree(PathBuf);

impl Tree {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire_editor_cook_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("content")).expect("its content");
        fs::create_dir_all(dir.join("target/content/editor")).expect("its cook");
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().expect("a parent")).expect("its folder");
        fs::write(&path, text).expect("written");
        path
    }

    fn packs(&self) -> PathBuf {
        self.0.join("target/content/editor")
    }

    /// Runs `commands` as a script file beside the content, as `--run` does, and what they asked to cook.
    fn run(&self, editor: &mut Editor, commands: &str) {
        let path =
            self.write(&format!("steps{SCRIPT_SUFFIX}"), &format!("(version: 1, commands: [{commands}])"));
        let script = Script::read(&path).unwrap_or_else(|error| panic!("{error}"));
        editor.run_script(&script).unwrap_or_else(|error| panic!("{error}"));
        editor.finish_cooks();
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A material bound to a texture no source has saves, its cook is refused, and the editor names the document and
/// the field: on the top bar, at the top of Properties and on the field's own row. Taking the binding back cooks
/// clean, and the refusal goes.
#[test]
#[ignore = "needs this tree's pipeline: cargo xtask content"]
fn a_broken_material_shows_its_refusal_by_field() {
    let tree = Tree::new("material");
    let material = tree.write("content/broken/swatch.material.ron", SWATCH);
    tree.write("content/broken/swatch.material.ron.meta", SWATCH_META);
    let mut editor = Editor::new(Options {
        file: Some(material.clone()),
        content: Some(tree.packs()),
        link: Some("127.0.0.1:9".parse().expect("an address")),
        pipeline: Some(pipeline()),
        game: None,
    });
    tree.run(
        &mut editor,
        &format!(r#"SetProperty(path: "base_color_map", value: "Some(\"{NOWHERE}\")"), Save"#),
    );
    let refusal = editor.cook_refusal().expect("the cook refused it").clone();
    assert_eq!(refusal.field.as_deref(), Some("base_color_map"), "{refusal:?}");
    let source = refusal.source.as_deref().expect("a source").canonicalize().expect("there");
    assert_eq!(source, material.canonicalize().expect("there"));
    assert!(refusal.message.contains("texture slot `base_color`"), "{}", refusal.message);
    let stats = editor.cook_stats().expect("a cook ran");
    assert_eq!((stats.cooks, stats.cooks_failed), (1, 1));
    assert_eq!(editor.cooks_landed(), 0);

    editor.shell_mut().open(Pane::Properties);
    let mut harness = Harness::builder()
        .with_size((1400.0, 900.0))
        .with_accessibility_check(false)
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), editor);
    harness.run_steps(4);
    assert!(harness.query_by_label_contains("cook refused swatch.material.ron at base_color_map").is_some());
    assert!(harness.query_by_label_contains("The cook refused this at base_color_map").is_some());
    assert!(harness.query_by_label_contains("(refused)").is_some(), "the field's own row");
    let mut editor = harness.into_state();

    tree.run(&mut editor, r#"SetProperty(path: "base_color_map", value: "None"), Save"#);
    assert_eq!(editor.cook_refusal(), None, "{:?}", editor.cook_refusal());
    let stats = editor.cook_stats().expect("cooks ran");
    assert_eq!((stats.cooks, stats.cooks_failed), (2, 1));
    assert_eq!(editor.cooks_landed(), 1);
    assert!(tree.packs().join("shared.spack").is_file(), "the clean cook packed");
}

/// M6 T16a: the sequencer's keys through the real cook. A face key added by its preset name cooks clean; a subtitle
/// added with a Fluent key the string tables don't hold is refused by the cook on its field, and one they hold cooks
/// clean again.
#[test]
#[ignore = "needs this tree's pipeline: cargo xtask content"]
fn the_sequencers_keys_cook_by_preset_and_by_the_tables_keys() {
    let tree = Tree::new("sequencer_keys");
    let content = root().join("content");
    let copy = |from: &str, to: &str| {
        tree.write(to, &fs::read_to_string(content.join(from)).expect("a shipped source"));
    };
    for file in ["court_gate.timeline.ron", "court_gate.timeline.ron.meta"] {
        copy(&format!("zones/lantern_ward/{file}"), &format!("content/gate/{file}"));
    }
    for file in ["en.locale.ron", "en.locale.ron.meta", "en/cutscenes.ftl", "en/cutscenes.ftl.meta"] {
        copy(&format!("loc/{file}"), &format!("content/loc/{file}"));
    }
    let gate = tree.0.join("content/gate/court_gate.timeline.ron");
    let mut editor = Editor::new(Options {
        file: Some(gate.clone()),
        content: Some(tree.packs()),
        link: Some("127.0.0.1:9".parse().expect("an address")),
        pipeline: Some(pipeline()),
        game: None,
    });
    tree.run(&mut editor, r#"Scrub(60), AddKey(lane: Faces("hero"), name: Some("surprised"), at: 60), Save"#);
    assert_eq!(editor.cook_refusal(), None, "{:?}", editor.cook_refusal());
    assert_eq!(editor.cooks_landed(), 1, "the face cooked clean");

    tree.run(
        &mut editor,
        r#"Scrub(500), AddKey(lane: Subtitles, name: Some("subtitle-court-gate-unwritten"), at: 500), Save"#,
    );
    let refusal = editor.cook_refusal().expect("the cook refused it").clone();
    assert_eq!(refusal.field.as_deref(), Some("subtitles[3].key"), "{refusal:?}");
    let source = refusal.source.as_deref().expect("a source").canonicalize().expect("there");
    assert_eq!(source, gate.canonicalize().expect("there"));
    assert!(refusal.message.contains("locale `en`"), "{}", refusal.message);

    tree.run(
        &mut editor,
        r#"SetProperty(path: "subtitles[3].key", value: "\"subtitle-court-gate-answer\""), Save"#,
    );
    assert_eq!(editor.cook_refusal(), None, "{:?}", editor.cook_refusal());
    assert_eq!(editor.cooks_landed(), 2);
}

/// Every file the editor's cook of `content/` writes is the bytes `cargo xtask content --cook editor` wrote: the same
/// pipeline with the same arguments, into packs of its own.
#[test]
#[ignore = "needs this tree's pipeline and its editor cook: cargo xtask content --cook editor"]
#[allow(clippy::print_stdout, reason = "the count is for a person reading --nocapture")]
fn a_cook_from_the_editor_is_cargo_xtask_contents() {
    let shipped = root().join("target/content/editor");
    let mut plan = CookPlan::for_cook(&shipped, None).expect("the repository's editor cook");
    let tree = Tree::new("same_bytes");
    plan.packs = tree.packs();
    plan.loose = tree.0.join("target/content/loose/editor");
    let mut cooker = Cooker::new(Box::new(Pipeline::new(plan)));
    cooker.ask(0.0);
    let cooked = cooker.settle().expect("a cook ran");
    assert!(cooked.is_ok(), "{cooked:?}");
    let ours = files(&tree.packs());
    let theirs = files(&shipped);
    assert_eq!(
        ours.iter().map(|(path, _)| path).collect::<Vec<_>>(),
        theirs.iter().map(|(path, _)| path).collect::<Vec<_>>(),
        "the same files"
    );
    for ((path, ours), (_, theirs)) in ours.iter().zip(&theirs) {
        assert!(ours == theirs, "{} differs", path.display());
    }
    println!("{} files, the same bytes", ours.len());
}

/// Every file under `dir`, by its path there, with its bytes.
fn files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut folders = vec![dir.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).expect("a folder") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                folders.push(path);
            } else {
                let bytes = fs::read(&path).expect("a file");
                found.push((path.strip_prefix(dir).expect("under it").to_owned(), bytes));
            }
        }
    }
    found.sort();
    found
}
