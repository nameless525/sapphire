//! The game's screens in the sandbox (`--ui-game <stem>`, M3 U7): one of `content/ui/<stem>.ui.ron` on its
//! sample model, in the language asked for, which is what the `game-*` goldens show and what hot reload and
//! live link reach. It opens at the clock's start, is read at `--ui-time`, after the steps of `--ui-walk`, and
//! `--ui-plain` stands a flat color behind it. `game_ui::screen` shows it, and no window's input reaches it: a
//! game's own menus run in `game_app`.

use game_ui::menus::MenuScreen;
use game_ui::screen::{ScreenStack, StackSetup};
use sapphire_platform::Timestamp;
use sapphire_stage::content::ScreenAssets;
use sapphire_ui::{Atlas, UiStep};

/// How `--ui-game` shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenOptions {
    /// The screen.
    pub screen: MenuScreen,
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

impl ScreenOptions {
    /// `screen` as it settles, in English.
    #[must_use]
    pub fn of(screen: MenuScreen) -> Self {
        Self {
            screen,
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

    /// Whether it shows an inset, which needs a showcase of the character standing for inset 0: the character
    /// screen's (M3 U6).
    #[must_use]
    pub const fn shows_inset(&self) -> bool {
        matches!(self.screen, MenuScreen::Character)
    }

    /// `assets` over `atlas` as these flags ask, on the screen's sample model, opened at the clock's start.
    /// `assets` holds the screen's document, which `Content::load_screens` gives for `screen.id()`.
    pub fn stack(&self, assets: ScreenAssets, atlas: Atlas) -> Result<ScreenStack, String> {
        if let Some(step) =
            self.walk.iter().find(|step| *step != "confirm" && UiStep::from_name(step).is_none())
        {
            return Err(format!("--ui-walk: `{step}` isn't up, down, left, right or confirm"));
        }
        let setup = StackSetup { locale: self.locale.clone(), pseudo: self.pseudo, scale: self.scale };
        let shown = |error| format!("--ui-game: {error}");
        let mut stack = ScreenStack::new(assets.tables, assets.fonts, atlas, setup).map_err(shown)?;
        let start = Timestamp::from_nanos(0);
        let model = self.screen.sample_model();
        stack.push(&assets.documents[0], model, start, self.walk.clone()).map_err(shown)?;
        if self.plain {
            stack.push_plain_under(start).map_err(shown)?;
        }
        stack.set_now(Timestamp::from_nanos((f64::from(self.time) * 1e9) as u64));
        stack.turn_insets(self.turn);
        Ok(stack)
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::ui::UiDocument;
    use sapphire_loc::StringTable;
    use sapphire_look::UiDrawList;
    use sapphire_stage::content::{FONT_CHAINS, LOCALES};

    use super::*;

    const SETTINGS: &str = include_str!("../../../content/ui/settings.ui.ron");
    const PAUSE: &str = include_str!("../../../content/ui/pause.ui.ron");

    #[test]
    fn the_ids_are_the_sidecars() {
        for (id, sidecar) in [
            (FONT_CHAINS, include_str!("../../../content/fonts/ui.fontchains.ron.meta")),
            (LOCALES[0], include_str!("../../../content/loc/en.locale.ron.meta")),
            (LOCALES[1], include_str!("../../../content/loc/ja.locale.ron.meta")),
        ] {
            assert!(sidecar.contains(&id.to_string()), "{id} isn't in {sidecar}");
        }
    }

    /// A table with every key `document` names, each saying `text`.
    fn table(document: &UiDocument, text: &str) -> StringTable {
        let mut keys: Vec<String> = document.texts().map(|(_, text, _)| text.key.clone()).collect();
        keys.sort();
        keys.dedup();
        let mut table = StringTable {
            locale: "en".to_owned(),
            fonts: ["body", "heading", "display"]
                .map(|role| sapphire_strings::FontChain {
                    role: role.to_owned(),
                    families: vec!["M PLUS 1".to_owned()],
                })
                .to_vec(),
            ..StringTable::default()
        };
        for key in keys {
            let mut span = |text: &str| {
                let start = table.pool.len() as u32;
                table.pool.push_str(text);
                sapphire_strings::Span { start, len: text.len() as u32 }
            };
            let key = span(&key);
            let pattern = sapphire_strings::Pattern::Plain(span(text));
            table.entries.push(sapphire_strings::Entry { key, pattern });
        }
        table
    }

    fn assets(text: &str) -> ScreenAssets {
        let document = UiDocument::from_document(text).unwrap();
        let table = table(&document, "Text");
        ScreenAssets { documents: vec![document], tables: vec![table], fonts: Vec::new() }
    }

    /// The settings at their fixed points: where a walk ends up, at every aspect a screen ships at.
    #[test]
    fn a_screen_opens_walks_and_reaches_all_it_holds() {
        for size in [(1024, 768), (1280, 720), (2560, 1080)] {
            let options = ScreenOptions {
                time: 1.0,
                walk: ["down"; 3].map(str::to_owned).to_vec(),
                ..ScreenOptions::of(MenuScreen::Settings)
            };
            let mut stack = options.stack(assets(SETTINGS), Atlas::paint()).unwrap();
            stack.draw(size, &mut UiDrawList::default());
            let top = stack.top().unwrap();
            assert!(top.problems().is_empty(), "{:#?}", top.problems());
            assert_eq!(top.state(), Some("open"));
            assert_eq!(top.focus_name(), Some("rumble"));
            assert_eq!(top.unreachable(), Vec::<&str>::new());
        }
    }

    /// The backdrop is a screen under the one asked for, which still has the input and the walk.
    #[test]
    fn a_plain_backdrop_goes_under_the_screen() {
        let options = ScreenOptions { plain: true, ..ScreenOptions::of(MenuScreen::Pause) };
        let mut stack = options.stack(assets(PAUSE), Atlas::paint()).unwrap();
        stack.draw((640, 360), &mut UiDrawList::default());
        assert_eq!(stack.len(), 2);
        assert!(stack.tree(0).problems().is_empty(), "{:#?}", stack.tree(0).problems());
        assert!(stack.top().unwrap().find("resume").is_some(), "the screen's on top");
    }

    #[test]
    fn a_walk_is_checked_when_it_is_asked_for() {
        let options =
            ScreenOptions { walk: vec!["sideways".to_owned()], ..ScreenOptions::of(MenuScreen::Pause) };
        let refused = options.stack(assets(PAUSE), Atlas::paint()).map(drop).unwrap_err();
        assert!(refused.contains("sideways"), "{refused}");
    }

    fn nodes(stack: &ScreenStack) -> Vec<String> {
        stack.top().unwrap().strings().map(|(node, _)| node.to_owned()).collect()
    }

    #[test]
    fn a_document_replaces_the_tree_and_a_bad_one_keeps_it() {
        let mut stack = ScreenOptions::of(MenuScreen::Pause).stack(assets(PAUSE), Atlas::paint()).unwrap();
        let mut list = UiDrawList::default();
        stack.draw((640, 360), &mut list);
        let before = nodes(&stack);
        assert!(before.contains(&"title".to_owned()), "{before:?}");

        // A node renamed: what's on screen follows.
        let renamed = PAUSE.replacen(
            "name: \"title\",\n                parent: \"panel\"",
            "name: \"heading\",\n                parent: \"panel\"",
            1,
        );
        assert_ne!(renamed, PAUSE, "the node is still there to rename");
        stack.replace(&renamed).unwrap();
        stack.draw((640, 360), &mut list);
        let after = nodes(&stack);
        assert!(after.contains(&"heading".to_owned()), "{after:?}");

        // Doesn't parse, doesn't check, doesn't resolve, and each says where.
        for (bad, says) in [
            ("(type: \"sapphire::assets::UiDocument\", version: 2, data: (nodes: [(nam: \"x\")]))", "line"),
            ("(type: \"sapphire::assets::UiDocument\", version: 2, data: (nodes: []))", "nodes"),
            (&*PAUSE.replacen("path: \"can_leave\"", "path: \"mana\"", 1), "mana"),
        ] {
            let error = stack.replace(bad).unwrap_err();
            assert!(error.contains(says), "{error}");
            stack.draw((640, 360), &mut list);
            assert_eq!(nodes(&stack), after, "the good tree stays");
        }
    }

    #[test]
    fn an_unknown_language_says_which() {
        let options = ScreenOptions { locale: "xx".to_owned(), ..ScreenOptions::of(MenuScreen::Pause) };
        let refused = options.stack(assets(PAUSE), Atlas::paint()).map(drop).unwrap_err();
        assert!(refused.contains("--ui-game") && refused.contains("xx"), "{refused}");
    }
}
