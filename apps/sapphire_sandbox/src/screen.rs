//! The sandbox's test screen (`--ui-screen`, M3 U4): `content/ui/test_screen.ui.ron` in the language asked
//! for, with fixed values to bind, which is what the tree's goldens show and what hot reload and live link
//! reach. `game_ui::screen` shows it. It isn't a game's.

use bevy_reflect::Reflect;
use game_ui::screen::{ScreenLayer, ScreenSetup};
use sapphire_assets::AssetId;
use sapphire_stage::content::ScreenAssets;

/// `content/ui/test_screen.ui.ron`.
pub const TEST_SCREEN: AssetId = AssetId::from_text("0606f00f-d999-4659-9953-965ffdcfd348");

/// What the test screen's paths read. `content/ui/test_screen.model.ron` says the same for a cook, and a
/// test holds the two together.
#[derive(Clone, Debug, Reflect)]
pub struct TestScreenModel {
    pub level: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub coins: i64,
    /// A share of the bar.
    pub health: f32,
    pub show_badge: bool,
}

impl Default for TestScreenModel {
    fn default() -> Self {
        Self { level: 90, hp: 6240, max_hp: 9600, coins: 12_480, health: 0.65, show_badge: true }
    }
}

/// How `--ui-screen` shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenOptions {
    /// `--ui-locale`.
    pub locale: String,
    /// `--ui-pseudo`.
    pub pseudo: bool,
    /// `--ui-scale`: pixels per point.
    pub scale: f32,
}

impl Default for ScreenOptions {
    fn default() -> Self {
        Self { locale: "en".to_owned(), pseudo: false, scale: 1.0 }
    }
}

impl ScreenOptions {
    /// The document it shows.
    #[must_use]
    pub const fn document(&self) -> AssetId {
        TEST_SCREEN
    }

    /// `assets` as these flags ask, on the test screen's model.
    pub fn layer(&self, assets: ScreenAssets) -> Result<ScreenLayer, String> {
        let setup = ScreenSetup {
            locale: self.locale.clone(),
            pseudo: self.pseudo,
            scale: self.scale,
            model: Box::new(TestScreenModel::default()),
        };
        ScreenLayer::new(assets, setup).map_err(|error| format!("--ui-screen: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use bevy_reflect::Typed;
    use sapphire_assets::ui::{UiDocument, UiModel};
    use sapphire_loc::StringTable;
    use sapphire_look::UiDrawList;
    use sapphire_stage::content::{FONT_CHAINS, LOCALES};

    use super::*;

    const DOCUMENT: &str = include_str!("../../../content/ui/test_screen.ui.ron");
    const MODEL: &str = include_str!("../../../content/ui/test_screen.model.ron");

    #[test]
    fn the_ids_are_the_sidecars() {
        for (id, sidecar) in [
            (TEST_SCREEN, include_str!("../../../content/ui/test_screen.ui.ron.meta")),
            (FONT_CHAINS, include_str!("../../../content/fonts/ui.fontchains.ron.meta")),
            (LOCALES[0], include_str!("../../../content/loc/en.locale.ron.meta")),
            (LOCALES[1], include_str!("../../../content/loc/ja.locale.ron.meta")),
        ] {
            assert!(sidecar.contains(&id.to_string()), "{id} isn't in {sidecar}");
        }
    }

    #[test]
    fn the_model_document_is_what_the_type_has() {
        assert_eq!(UiModel::from_document(MODEL).unwrap(), UiModel::of_type(TestScreenModel::type_info()));
    }

    /// What the document names.
    const KEYS: [&str; 9] = [
        "test-screen-body",
        "test-screen-coins",
        "test-screen-controls",
        "test-screen-display",
        "test-screen-health",
        "test-screen-language",
        "test-screen-sound",
        "test-screen-subtitle",
        "test-screen-title",
    ];

    fn table(locale: &str, fallback: &[&str], fluent: &[(&str, &str)]) -> StringTable {
        let mut table = StringTable {
            locale: locale.to_owned(),
            fallback: fallback.iter().map(|tag| (*tag).to_owned()).collect(),
            fonts: ["body", "display"]
                .map(|role| sapphire_strings::FontChain {
                    role: role.to_owned(),
                    families: vec!["M PLUS 1".to_owned()],
                })
                .to_vec(),
            ..StringTable::default()
        };
        let mut sorted = fluent.to_vec();
        sorted.sort_by_key(|(key, _)| *key);
        for (key, text) in sorted {
            let mut span = |text: &str| {
                let start = table.pool.len() as u32;
                table.pool.push_str(text);
                sapphire_strings::Span { start, len: text.len() as u32 }
            };
            let key = span(key);
            let pattern = sapphire_strings::Pattern::Plain(span(text));
            table.entries.push(sapphire_strings::Entry { key, pattern });
        }
        table
    }

    fn layer() -> ScreenLayer {
        let assets = ScreenAssets {
            document: UiDocument::from_document(DOCUMENT).unwrap(),
            tables: vec![table("en", &[], &KEYS.map(|key| (key, "Text")))],
            fonts: Vec::new(),
        };
        ScreenOptions::default().layer(assets).unwrap()
    }

    fn nodes(layer: &ScreenLayer) -> Vec<String> {
        layer.tree().strings().map(|(node, _)| node.to_owned()).collect()
    }

    #[test]
    fn a_document_replaces_the_tree_and_a_bad_one_keeps_it() {
        let mut layer = layer();
        let mut list = UiDrawList::default();
        layer.draw((640, 360), &mut list);
        let before = nodes(&layer);
        assert!(before.contains(&"title".to_owned()), "{before:?}");
        assert!(layer.tree().problems().is_empty(), "{:#?}", layer.tree().problems());

        // A node renamed: what's on screen follows.
        layer
            .replace(&DOCUMENT.replace(
                "name: \"title\",\n                parent: \"header\"",
                "name: \"heading\",\n                parent: \"header\"",
            ))
            .unwrap();
        layer.draw((640, 360), &mut list);
        assert!(nodes(&layer).contains(&"heading".to_owned()));

        // Doesn't parse, doesn't check, doesn't resolve, and each says where.
        let renamed = nodes(&layer);
        for (bad, says) in [
            ("(type: \"sapphire::assets::UiDocument\", version: 2, data: (nodes: [(nam: \"x\")]))", "line"),
            ("(type: \"sapphire::assets::UiDocument\", version: 2, data: (nodes: []))", "nodes"),
            (&*DOCUMENT.replace("path: \"health\"", "path: \"mana\""), "mana"),
        ] {
            let error = layer.replace(bad).unwrap_err();
            assert!(error.contains(says), "{error}");
            layer.draw((640, 360), &mut list);
            assert_eq!(nodes(&layer), renamed, "the good tree stays");
        }
    }

    #[test]
    fn an_unknown_language_says_which() {
        let assets = ScreenAssets {
            document: UiDocument::from_document(DOCUMENT).unwrap(),
            tables: vec![table("en", &[], &[])],
            fonts: Vec::new(),
        };
        let options = ScreenOptions { locale: "xx".to_owned(), ..ScreenOptions::default() };
        let refused = options.layer(assets).unwrap_err();
        assert!(refused.contains("--ui-screen") && refused.contains("xx"), "{refused}");
    }
}
