//! The game's menus as their sources have them, for what tests them with no cooked content.

#![allow(dead_code, reason = "each test binary uses what it needs")]

pub mod walk;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use game_ui::menus::{MenuDocument, MenuScreen};
use sapphire_assets::font::Font;
use sapphire_assets::ui::UiDocument;
use sapphire_loc::StringTable;
use sapphire_moves::EnemyDocument;
use sapphire_pipeline::loc::{self, FluentFile};
use sapphire_stage::content::ScreenAssets;
use sapphire_timeline::TimelineDocument;

/// The title's version label in every test: a packaged build's, the longer form (0.1.0).
pub const VERSION: &str = "v0.1.0 (1234)";

pub fn content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

/// A locale's table, cooked from every source it has.
pub fn table(locale: &str) -> StringTable {
    let dir = content().join("loc").join(locale);
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if loc::is_ftl_source(&path) {
            let name = format!("loc/{locale}/{}", path.file_name().unwrap().to_str().unwrap());
            files.push(FluentFile { path, name });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let document = content().join(format!("loc/{locale}.locale.ron"));
    let bytes = loc::cook(&document, &std::fs::read(&document).unwrap(), &files).unwrap();
    StringTable::read(&bytes).unwrap()
}

fn add_kind_names(dir: &Path, into: &mut BTreeMap<String, bool>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            add_kind_names(&path, into);
        } else if path.to_str().is_some_and(|name| name.ends_with(".enemy.ron")) {
            let kind = EnemyDocument::parse(&std::fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let other = into.insert(kind.name.clone(), kind.boss);
            assert!(other.is_none(), "{}: another kind is named {}", path.display(), kind.name);
        }
    }
}

/// The key of every kind of enemy's name under `content/` (M4 X2), and whether it's a boss's, which its bar
/// shows (M3 B3): what the HUD calls its foe. No kind by name: they're content, and a boss's name is in no
/// `.rs` file (`sapphire_gameplay`'s `bosses_are_content`).
pub fn kind_names() -> BTreeMap<String, bool> {
    let mut keys = BTreeMap::new();
    add_kind_names(&content(), &mut keys);
    keys
}

/// The game's screens, as `content/ui/menus.ron` has them.
pub fn menus() -> MenuDocument {
    MenuDocument::from_ron(&std::fs::read_to_string(content().join("ui/menus.ron")).unwrap()).unwrap()
}

fn add_subtitles(dir: &Path, into: &mut Vec<(String, Option<String>)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            add_subtitles(&path, into);
        } else if path.to_str().is_some_and(|name| name.ends_with(".timeline.ron")) {
            let timeline = TimelineDocument::from_ron(&std::fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            into.extend(timeline.subtitles.into_iter().map(|line| (line.key, line.speaker)));
        }
    }
}

/// Every timeline's subtitles under `content/` (M3 Q3): each line's key and its speaker's, as the HUD shows
/// them.
pub fn subtitles() -> Vec<(String, Option<String>)> {
    let mut lines = Vec::new();
    add_subtitles(&content(), &mut lines);
    lines
}

/// The document of the screen named `name`: `<name>.ui.ron` in the first of `dirs` that has one.
pub fn document_in(name: &str, dirs: &[PathBuf]) -> UiDocument {
    let path = dirs
        .iter()
        .map(|dir| dir.join(format!("{name}.ui.ron")))
        .find(|path| path.exists())
        .unwrap_or_else(|| panic!("no {name}.ui.ron in {dirs:?}"));
    UiDocument::from_document(&std::fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The game's document of `screen`.
pub fn document(screen: MenuScreen) -> UiDocument {
    let menus = menus();
    document_in(&menus.entry(screen).expect("one of the game's screens").name, &[content().join("ui")])
}

/// What menus are made of: the screens, and their assets in the screens' order.
pub struct Screens {
    pub assets: ScreenAssets,
    pub menus: MenuDocument,
}

/// The game's screens and their assets, each font whole and uncut, with no display face's field: every glyph
/// draws from the pages.
pub fn assets() -> Screens {
    screens_of(menus(), &[content().join("ui")])
}

/// `menus` and their assets, each screen's document from the first of `dirs` that has it, as [`assets`]
/// has them.
pub fn screens_of(menus: MenuDocument, dirs: &[PathBuf]) -> Screens {
    let dir = content().join("third_party");
    let fonts = [
        ("M PLUS 1", "mplus1/MPLUS1[wght].ttf"),
        ("Oxanium", "oxanium/Oxanium[wght].ttf"),
        ("Dela Gothic One", "dela_gothic_one/DelaGothicOne-Regular.ttf"),
    ]
    .map(|(family, font)| Font {
        family: family.to_owned(),
        pack: String::new(),
        baseline_shift: 0.0,
        license: String::new(),
        file: std::fs::read(dir.join(font)).unwrap(),
        field: None,
    })
    .to_vec();
    let documents = menus.screens.iter().map(|screen| document_in(&screen.name, dirs)).collect();
    Screens { assets: ScreenAssets { documents, tables: vec![table("en"), table("ja")], fonts }, menus }
}

/// Where the tests' fixtures are: documents no game has.
pub fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata")
}

/// The ID `tests/testdata/help.ui.ron` would have: it's never cooked, so it has no sidecar.
pub const HELP: &str = "5f0c7f4e-9d3b-4a51-8e2a-6b1d0c9e7a43";

/// The game's screens with one more, `tests/testdata/help.ui.ron`, which the pause's settings row now opens:
/// what a game adding a screen writes, all of it documents.
pub fn with_help() -> MenuDocument {
    let text = std::fs::read_to_string(content().join("ui/menus.ron")).unwrap();
    let pause_rows = "(node: \"character\", does: Open(\"character\")),\n                    \
                      (node: \"settings\", does: Open(\"settings\")),";
    assert!(text.contains(pause_rows), "the pause's rows as the test knows them");
    let text = text.replacen(
        pause_rows,
        "(node: \"character\", does: Open(\"character\")),\n(node: \"settings\", does: Open(\"help\")),",
        1,
    );
    let end = text.rfind("        ],\n    ),\n)").expect("the end of the screens");
    let help = format!(
        "(name: \"help\", document: \"{HELP}\", back: Some(Command(Resume)), rows: [\
         (node: \"settings\", does: Open(\"settings\")), (node: \"back\", does: Nothing)]),\n"
    );
    MenuDocument::from_ron(&format!("{}{help}{}", &text[..end], &text[end..])).expect("reads")
}
