//! The sandbox's test screens (`--ui-screen`, M3 U4): `content/ui/test_screen.ui.ron` in the language asked
//! for, with fixed values to bind, which is what the tree's goldens show and what hot reload and live link
//! reach. `--ui-menu` shows `content/ui/test_menu.ui.ron` instead (M3 U5), opened at the clock's start and
//! read at `--ui-time`, after the steps of `--ui-walk`. `--ui-inset` shows `content/ui/test_inset.ui.ron`
//! (M3 U6): the character in a 3D inset, a showcase of it the sandbox stands for inset 0, turned by
//! `--ui-turn`. `--ui-game <stem>` shows one of the game's screens on its sample model (M3 U7), which is what
//! their goldens are of. `game_ui::screen` shows them all, and no window's input reaches any of them: a
//! game's own menus run in `game_app`.

use bevy_reflect::{PartialReflect, Reflect};
use game_ui::menus::MenuScreen;
use game_ui::screen::{ScreenStack, StackSetup};
use sapphire_assets::AssetId;
use sapphire_assets::ui::UiDocument;
use sapphire_platform::Timestamp;
use sapphire_stage::content::ScreenAssets;
use sapphire_ui::{Atlas, UiStep};

/// `content/ui/test_screen.ui.ron`.
pub const TEST_SCREEN: AssetId = AssetId::from_text("0606f00f-d999-4659-9953-965ffdcfd348");
/// `content/ui/test_menu.ui.ron`.
pub const TEST_MENU: AssetId = AssetId::from_text("2eca009b-073e-4cab-a436-1894f36479d0");

/// `content/ui/test_inset.ui.ron`, which reads [`TestScreenModel`].
pub const TEST_INSET: AssetId = AssetId::from_text("c4c93b69-e921-4489-b4c6-7ceb62b89ce8");

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

/// What the test menu's path reads. `content/ui/test_menu.model.ron` says the same for a cook, and a test
/// holds the two together.
#[derive(Clone, Debug, Reflect)]
pub struct TestMenuModel {
    /// The key of the line under the list.
    pub line: String,
}

impl Default for TestMenuModel {
    fn default() -> Self {
        Self { line: "test-menu-line-first".to_owned() }
    }
}

/// `--ui-plain`: what stands behind a screen in place of the scene, so its golden moves for what it shows and
/// nothing else. Not cooked: only the sandbox shows it, and it says nothing.
const PLAIN_BACKDROP: &str = r#"(
    type: "sapphire::assets::UiDocument",
    version: 2,
    data: (nodes: [(name: "root", look: (fill: Color((74, 88, 112, 255))))]),
)"#;

/// What the backdrop's paths read: none.
#[derive(Clone, Debug, Default, Reflect)]
struct NoModel;

/// Which screen it shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShownScreen {
    /// `--ui-screen`.
    #[default]
    TestScreen,
    /// `--ui-menu`.
    TestMenu,
    /// `--ui-inset`.
    TestInset,
    /// `--ui-game <stem>`.
    Game(MenuScreen),
}

/// How `--ui-screen` shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenOptions {
    /// The screen.
    pub shown: ShownScreen,
    /// `--ui-locale`.
    pub locale: String,
    /// `--ui-pseudo`.
    pub pseudo: bool,
    /// `--ui-scale`: pixels per point.
    pub scale: f32,
    /// `--ui-plain`: a flat color behind it.
    pub plain: bool,
    /// `--ui-time`: the seconds on the screen's clock, always this: a capture of a time is that time.
    pub time: f32,
    /// `--ui-walk`: steps of focus taken when it opens, as `UiTree::walk` names them.
    pub walk: Vec<String>,
    /// `--ui-turn`: its insets turned as a drag across that share of their width turns them.
    pub turn: f32,
    /// `--ui-spin`: as `turn`, again each frame a bench runs, so its insets draw every frame.
    pub spin: f32,
}

/// Where the screen's clock stands with no `--ui-time`: well past what any screen does as it opens.
const SETTLED: f32 = 60.0;

impl Default for ScreenOptions {
    fn default() -> Self {
        Self {
            shown: ShownScreen::default(),
            locale: "en".to_owned(),
            pseudo: false,
            scale: 1.0,
            plain: false,
            time: SETTLED,
            walk: Vec::new(),
            turn: 0.0,
            spin: 0.0,
        }
    }
}

impl ScreenOptions {
    /// Whether it shows an inset, which needs a showcase of the character standing for inset 0.
    #[must_use]
    pub const fn shows_inset(&self) -> bool {
        matches!(self.shown, ShownScreen::TestInset | ShownScreen::Game(MenuScreen::Character))
    }

    /// The document it shows.
    #[must_use]
    pub const fn document(&self) -> AssetId {
        match self.shown {
            ShownScreen::TestScreen => TEST_SCREEN,
            ShownScreen::TestMenu => TEST_MENU,
            ShownScreen::TestInset => TEST_INSET,
            ShownScreen::Game(screen) => screen.id(),
        }
    }

    /// `assets` over `atlas` as these flags ask, on the model of the screen they show, opened at the clock's
    /// start.
    pub fn stack(&self, assets: ScreenAssets, atlas: Atlas) -> Result<ScreenStack, String> {
        if let Some(step) =
            self.walk.iter().find(|step| *step != "confirm" && UiStep::from_name(step).is_none())
        {
            return Err(format!("--ui-walk: `{step}` isn't up, down, left, right or confirm"));
        }
        let model: Box<dyn PartialReflect> = match self.shown {
            ShownScreen::TestScreen | ShownScreen::TestInset => Box::new(TestScreenModel::default()),
            ShownScreen::TestMenu => Box::new(TestMenuModel::default()),
            ShownScreen::Game(screen) => screen.sample_model(),
        };
        let setup = StackSetup { locale: self.locale.clone(), pseudo: self.pseudo, scale: self.scale };
        let shown = |error| format!("--ui-screen: {error}");
        let mut stack = ScreenStack::new(assets.tables, assets.fonts, atlas, setup).map_err(shown)?;
        let start = Timestamp::from_nanos(0);
        stack.push(&assets.documents[0], model, start, self.walk.clone()).map_err(shown)?;
        if self.plain {
            let backdrop =
                UiDocument::from_document(PLAIN_BACKDROP).map_err(|error| shown(error.to_string()))?;
            stack.push_under(&backdrop, Box::new(NoModel), start).map_err(shown)?;
        }
        stack.set_now(Timestamp::from_nanos((f64::from(self.time) * 1e9) as u64));
        stack.turn_insets(self.turn);
        Ok(stack)
    }
}

#[cfg(test)]
mod tests {
    use bevy_reflect::Typed;
    use sapphire_assets::ui::UiModel;
    use sapphire_loc::StringTable;
    use sapphire_look::UiDrawList;
    use sapphire_stage::content::{FONT_CHAINS, LOCALES};

    use super::*;

    const DOCUMENT: &str = include_str!("../../../content/ui/test_screen.ui.ron");
    const MODEL: &str = include_str!("../../../content/ui/test_screen.model.ron");
    const MENU: &str = include_str!("../../../content/ui/test_menu.ui.ron");
    const MENU_MODEL: &str = include_str!("../../../content/ui/test_menu.model.ron");

    #[test]
    fn the_ids_are_the_sidecars() {
        for (id, sidecar) in [
            (TEST_SCREEN, include_str!("../../../content/ui/test_screen.ui.ron.meta")),
            (TEST_MENU, include_str!("../../../content/ui/test_menu.ui.ron.meta")),
            (TEST_INSET, include_str!("../../../content/ui/test_inset.ui.ron.meta")),
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
        assert_eq!(UiModel::from_document(MENU_MODEL).unwrap(), UiModel::of_type(TestMenuModel::type_info()));
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

    fn stack() -> ScreenStack {
        let assets = ScreenAssets {
            documents: vec![UiDocument::from_document(DOCUMENT).unwrap()],
            tables: vec![table("en", &[], &KEYS.map(|key| (key, "Text")))],
            fonts: Vec::new(),
        };
        ScreenOptions::default().stack(assets, Atlas::paint()).unwrap()
    }

    /// The test menu at its fixed points: where a walk ends up, at every aspect a screen ships at.
    #[test]
    fn the_menu_opens_walks_and_reaches_all_it_holds() {
        let document = UiDocument::from_document(MENU).unwrap();
        let keys: Vec<(String, &str)> = document
            .texts()
            .map(|(_, text, _)| (text.key.clone(), "Text"))
            .chain([("test-menu-line-first".to_owned(), "A line")])
            .collect();
        let keys: Vec<(&str, &str)> = keys.iter().map(|(key, text)| (key.as_str(), *text)).collect();
        for size in [(1024, 768), (1280, 720), (2560, 1080)] {
            let options = ScreenOptions {
                shown: ShownScreen::TestMenu,
                time: 1.0,
                walk: ["down"; 5].map(str::to_owned).to_vec(),
                ..ScreenOptions::default()
            };
            let assets = ScreenAssets {
                documents: vec![document.clone()],
                tables: vec![table("en", &[], &keys)],
                fonts: Vec::new(),
            };
            let mut stack = options.stack(assets, Atlas::paint()).unwrap();
            let mut list = UiDrawList::default();
            stack.draw(size, &mut list);
            assert!(stack.top().unwrap().problems().is_empty(), "{:#?}", stack.top().unwrap().problems());
            assert_eq!(stack.top().unwrap().state(), Some("open"));
            assert_eq!(stack.top().unwrap().focus_name(), Some("row-language"));
            assert_eq!(stack.top().unwrap().unreachable(), Vec::<&str>::new());
            let list_node = stack.top().unwrap().find("list").unwrap();
            assert!(
                stack.top().unwrap().scroll_offset(list_node).unwrap() > 0.0,
                "the walk went off its end"
            );
        }
    }

    /// The backdrop is a screen under the one asked for, which still has the input and the walk.
    #[test]
    fn a_plain_backdrop_goes_under_the_screen() {
        let options = ScreenOptions { plain: true, ..ScreenOptions::default() };
        let assets = ScreenAssets {
            documents: vec![UiDocument::from_document(DOCUMENT).unwrap()],
            tables: vec![table("en", &[], &KEYS.map(|key| (key, "Text")))],
            fonts: Vec::new(),
        };
        let mut stack = options.stack(assets, Atlas::paint()).unwrap();
        stack.draw((640, 360), &mut UiDrawList::default());
        assert_eq!(stack.len(), 2);
        assert!(stack.tree(0).problems().is_empty(), "{:#?}", stack.tree(0).problems());
        assert!(stack.top().unwrap().find("title").is_some(), "the screen's on top");
    }

    #[test]
    fn a_walk_is_checked_when_it_is_asked_for() {
        let options = ScreenOptions { walk: vec!["sideways".to_owned()], ..ScreenOptions::default() };
        let assets = ScreenAssets {
            documents: vec![UiDocument::from_document(DOCUMENT).unwrap()],
            tables: vec![table("en", &[], &[])],
            fonts: Vec::new(),
        };
        let refused = options.stack(assets, Atlas::paint()).map(drop).unwrap_err();
        assert!(refused.contains("sideways"), "{refused}");
    }

    fn nodes(stack: &ScreenStack) -> Vec<String> {
        stack.top().unwrap().strings().map(|(node, _)| node.to_owned()).collect()
    }

    #[test]
    fn a_document_replaces_the_tree_and_a_bad_one_keeps_it() {
        let mut stack = stack();
        let mut list = UiDrawList::default();
        stack.draw((640, 360), &mut list);
        let before = nodes(&stack);
        assert!(before.contains(&"title".to_owned()), "{before:?}");
        assert!(stack.top().unwrap().problems().is_empty(), "{:#?}", stack.top().unwrap().problems());

        // A node renamed: what's on screen follows.
        stack
            .replace(&DOCUMENT.replace(
                "name: \"title\",\n                parent: \"header\"",
                "name: \"heading\",\n                parent: \"header\"",
            ))
            .unwrap();
        stack.draw((640, 360), &mut list);
        assert!(nodes(&stack).contains(&"heading".to_owned()));

        // Doesn't parse, doesn't check, doesn't resolve, and each says where.
        let renamed = nodes(&stack);
        for (bad, says) in [
            ("(type: \"sapphire::assets::UiDocument\", version: 2, data: (nodes: [(nam: \"x\")]))", "line"),
            ("(type: \"sapphire::assets::UiDocument\", version: 2, data: (nodes: []))", "nodes"),
            (&*DOCUMENT.replace("path: \"health\"", "path: \"mana\""), "mana"),
        ] {
            let error = stack.replace(bad).unwrap_err();
            assert!(error.contains(says), "{error}");
            stack.draw((640, 360), &mut list);
            assert_eq!(nodes(&stack), renamed, "the good tree stays");
        }
    }

    #[test]
    fn an_unknown_language_says_which() {
        let assets = ScreenAssets {
            documents: vec![UiDocument::from_document(DOCUMENT).unwrap()],
            tables: vec![table("en", &[], &[])],
            fonts: Vec::new(),
        };
        let options = ScreenOptions { locale: "xx".to_owned(), ..ScreenOptions::default() };
        let refused = options.stack(assets, Atlas::paint()).map(drop).unwrap_err();
        assert!(refused.contains("--ui-screen") && refused.contains("xx"), "{refused}");
    }
}
