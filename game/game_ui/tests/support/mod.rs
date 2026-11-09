//! The game's menus as their sources have them, for what tests them with no cooked content.

#![allow(dead_code, reason = "each test binary uses what it needs")]

use std::path::{Path, PathBuf};

use game_ui::menus::MenuScreen;
use sapphire_assets::font::Font;
use sapphire_assets::ui::UiDocument;
use sapphire_loc::StringTable;
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
