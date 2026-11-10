//! The game's menus as their sources have them, for what tests them with no cooked content.

#![allow(dead_code, reason = "each test binary uses what it needs")]

pub mod walk;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use game_ui::menus::MenuScreen;
use sapphire_assets::font::Font;
use sapphire_assets::ui::UiDocument;
use sapphire_loc::StringTable;
use sapphire_moves::EnemyDocument;
use sapphire_pipeline::loc::{self, FluentFile};
use sapphire_stage::content::ScreenAssets;

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

fn add_boss_names(dir: &Path, into: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            add_boss_names(&path, into);
        } else if path.to_str().is_some_and(|name| name.ends_with(".enemy.ron")) {
            let kind = EnemyDocument::parse(&std::fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            into.extend(kind.boss.map(|boss| boss.name));
        }
    }
}

/// The key of every boss's name that a kind of enemy under `content/` holds (M3 B3): what its bar shows.
/// No boss by name: they're content, which `sapphire_gameplay`'s `bosses_are_content` holds code to.
pub fn boss_names() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    add_boss_names(&content(), &mut keys);
    keys
}

pub fn document(screen: MenuScreen) -> UiDocument {
    let path = content().join(format!("ui/{}.ui.ron", screen.stem()));
    UiDocument::from_document(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The screens' assets, each font whole and uncut, with no display face's field: every glyph draws from
/// the pages.
pub fn assets() -> ScreenAssets {
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
    ScreenAssets {
        documents: MenuScreen::ALL.map(document).to_vec(),
        tables: vec![table("en"), table("ja")],
        fonts,
    }
}
