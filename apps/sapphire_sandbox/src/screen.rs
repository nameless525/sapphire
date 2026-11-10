//! The game's screens in the sandbox (`--ui-game <name>`, M3 U7): one of the game's screens, as its menus
//! document names them (M3 U9), `content/ui/<name>.ui.ron` on its kind's sample model, in the language asked for, which is what the `game-*` goldens show and what hot reload and
//! live link reach. It opens at the clock's start, is read at `--ui-time`, after the steps of `--ui-walk`, and
//! `--ui-plain` stands a flat color behind it. `game_ui::screen` shows it, and no window's input reaches it: a
//! game's own menus run in `game_app`. `--ui-dialogue <nexts>` puts the dialogue panel on a real conversation
//! in place of its sample: the Lantern Ward guide's, cooked, that many lines in (M3 Q2b).

use bevy_reflect::PartialReflect;
use game_ui::menus::{DialogueModel, MenuDocument, ScreenModel};
use game_ui::screen::{ScreenStack, StackSetup};
use sapphire_assets::AssetId;
use sapphire_dialogue::{ConditionId, Dialogue, DialogueWalk, DialogueWorld, EffectId};
use sapphire_platform::{AppInitError, Timestamp};
use sapphire_render::Renderer;
use sapphire_rhi::GpuContext;
use sapphire_stage::content::{Content, ScreenAssets};
use sapphire_ui::{Atlas, UiStep};

/// The Lantern Ward guide's dialogue, `content/dialogue/ward_guide.dialogue.ron`, which `--ui-dialogue`
/// walks.
pub const WARD_GUIDE: AssetId = AssetId::from_text("7b3a0339-b2a1-4ac8-9cfa-458d4b0134a7");

/// How `--ui-game` shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenOptions {
    /// The screen, by its name in the game's menus document: its document's file name without `.ui.ron`.
    pub screen: String,
    /// Its document's source, from the repository's root: what live tuning watches.
    pub source: String,
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
    /// `--ui-dialogue`: the panel on [`WARD_GUIDE`]'s conversation, this many lines past its first, in a
    /// world where none of its conditions holds and nothing it sets lands.
    pub dialogue: Option<u32>,
}

/// Where the screen's clock stands with no `--ui-time`: well past what any screen does as it opens.
const SETTLED: f32 = 60.0;

impl ScreenOptions {
    /// The screen named `screen` as it settles, in English.
    #[must_use]
    pub fn of(screen: &str) -> Self {
        Self {
            screen: screen.to_owned(),
            source: format!("content/ui/{screen}.ui.ron"),
            locale: "en".to_owned(),
            pseudo: false,
            scale: 1.0,
            plain: false,
            time: SETTLED,
            walk: Vec::new(),
            turn: 0.0,
            spin: 0.0,
            dialogue: None,
        }
    }

    /// Its document and the model it reads, from the game's `screens`, which have to have it. `--ui-dialogue`
    /// fills a dialogue panel, so it needs a screen that reads one.
    pub fn find(&self, screens: &MenuDocument) -> Result<(AssetId, ScreenModel), String> {
        let screen = screens.screens.iter().find(|screen| screen.name == self.screen).ok_or_else(|| {
            format!("--ui-game: `{}` isn't one of the game's screens: {}", self.screen, screens.names())
        })?;
        if self.dialogue.is_some() && screen.model != ScreenModel::Dialogue {
            return Err(format!("--ui-dialogue fills a dialogue panel, and `{}` isn't one", self.screen));
        }
        Ok((screen.document, screen.model))
    }

    /// Loads it from `content` over `atlas`, as the game's `screens` name it, and says whether it shows an
    /// inset (M3 U6), which needs a showcase of the character standing for inset 0.
    pub fn load(
        &self,
        screens: &MenuDocument,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
        atlas: Atlas,
    ) -> Result<(ScreenStack, bool), AppInitError> {
        let (id, model) = self.find(screens).map_err(AppInitError::msg)?;
        let assets = content.load_screens(&[id], renderer, gpu)?;
        let inset =
            assets.documents.iter().any(|document| document.nodes.iter().any(|node| node.inset.is_some()));
        let dialogue = self.walks().map(|id| content.load_dialogue(id, renderer, gpu)).transpose()?;
        let stack = self.stack(assets, atlas, dialogue.as_deref(), model).map_err(AppInitError::msg)?;
        Ok((stack, inset))
    }

    /// The cooked dialogue it walks, if any, which `Content::load_dialogue` gives.
    #[must_use]
    pub fn walks(&self) -> Option<AssetId> {
        self.dialogue.map(|_| WARD_GUIDE)
    }

    /// `assets` over `atlas` as these flags ask, on `model`'s sample, or `dialogue`'s line, opened at the
    /// clock's start. `assets` holds the screen's document, which `Content::load_screens` gives for its ID
    /// ([`Self::find`]); `dialogue` is what [`Self::walks`] names.
    pub fn stack(
        &self,
        assets: ScreenAssets,
        atlas: Atlas,
        dialogue: Option<&Dialogue>,
        model: ScreenModel,
    ) -> Result<ScreenStack, String> {
        if let Some(step) =
            self.walk.iter().find(|step| *step != "confirm" && UiStep::from_name(step).is_none())
        {
            return Err(format!("--ui-walk: `{step}` isn't up, down, left, right or confirm"));
        }
        let setup = StackSetup { locale: self.locale.clone(), pseudo: self.pseudo, scale: self.scale };
        let shown = |error| format!("--ui-game: {error}");
        let mut stack = ScreenStack::new(assets.tables, assets.fonts, atlas, setup).map_err(shown)?;
        let start = Timestamp::from_nanos(0);
        let model = match (self.dialogue, dialogue) {
            (Some(lines), Some(dialogue)) => conversation(dialogue, lines)?,
            (Some(_), None) => return Err("--ui-dialogue: the guide's dialogue wasn't loaded".to_owned()),
            (None, _) => model.sample(),
        };
        stack.push(&assets.documents[0], model, start, self.walk.clone()).map_err(shown)?;
        if self.plain {
            stack.push_plain_under(start).map_err(shown)?;
        }
        stack.set_now(Timestamp::from_nanos((f64::from(self.time) * 1e9) as u64));
        stack.turn_insets(self.turn);
        Ok(stack)
    }
}

/// A world where nothing holds and nothing lands: a conversation's first lines, as a new game hears them.
struct Untouched;

impl DialogueWorld for Untouched {
    fn ask(&mut self, _: ConditionId) -> bool {
        false
    }

    fn apply(&mut self, _: EffectId) {}
}

/// The panel's model `lines` lines into `dialogue`, going on by "next" alone.
fn conversation(dialogue: &Dialogue, lines: u32) -> Result<Box<dyn PartialReflect>, String> {
    let mut walk = DialogueWalk::begin(dialogue, &mut Untouched);
    for line in 0..lines {
        if !walk.next(dialogue, &mut Untouched) {
            return Err(format!("--ui-dialogue: line {line} wants an answer, or the conversation's over"));
        }
    }
    let view = walk.view(dialogue).ok_or_else(|| format!("--ui-dialogue: it's over after {lines} lines"))?;
    let mut model = DialogueModel::default();
    model.show(&view);
    Ok(Box::new(model))
}

#[cfg(test)]
mod tests {
    use sapphire_assets::ui::UiDocument;
    use sapphire_loc::StringTable;
    use sapphire_look::UiDrawList;
    use sapphire_stage::content::{FONT_CHAINS, LOCALES};

    use super::*;

    const SETTINGS: &str = include_str!("../../../content/ui/settings.ui.ron");
    const MENU: ScreenModel = ScreenModel::Menu;
    const PAUSE: &str = include_str!("../../../content/ui/pause.ui.ron");

    #[test]
    fn the_ids_are_the_sidecars() {
        for (id, sidecar) in [
            (FONT_CHAINS, include_str!("../../../content/fonts/ui.fontchains.ron.meta")),
            (LOCALES[0], include_str!("../../../content/loc/en.locale.ron.meta")),
            (LOCALES[1], include_str!("../../../content/loc/ja.locale.ron.meta")),
            (WARD_GUIDE, include_str!("../../../content/dialogue/ward_guide.dialogue.ron.meta")),
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
                ..ScreenOptions::of("settings")
            };
            let mut stack = options.stack(assets(SETTINGS), Atlas::paint(), None, MENU).unwrap();
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
        let options = ScreenOptions { plain: true, ..ScreenOptions::of("pause") };
        let mut stack = options.stack(assets(PAUSE), Atlas::paint(), None, MENU).unwrap();
        stack.draw((640, 360), &mut UiDrawList::default());
        assert_eq!(stack.len(), 2);
        assert!(stack.tree(0).problems().is_empty(), "{:#?}", stack.tree(0).problems());
        assert!(stack.top().unwrap().find("resume").is_some(), "the screen's on top");
    }

    #[test]
    fn a_walk_is_checked_when_it_is_asked_for() {
        let options = ScreenOptions { walk: vec!["sideways".to_owned()], ..ScreenOptions::of("pause") };
        let refused = options.stack(assets(PAUSE), Atlas::paint(), None, MENU).map(drop).unwrap_err();
        assert!(refused.contains("sideways"), "{refused}");
    }

    /// `--ui-dialogue 1`: the guide's question, its hidden answer hidden, with no going on without one.
    #[test]
    fn a_conversation_fills_the_panel_in_place_of_its_sample() {
        use sapphire_dialogue::{DialogueDocument, FlagRegistry};
        let registry = FlagRegistry::from_ron(include_str!("../../../content/flags.ron")).unwrap();
        let guide =
            DialogueDocument::from_ron(include_str!("../../../content/dialogue/ward_guide.dialogue.ron"))
                .and_then(|document| document.cook(&registry, |_| Ok(())))
                .unwrap();
        let shown = |lines| {
            let model = conversation(&guide, lines)?;
            let model = model.try_downcast_ref::<DialogueModel>().expect("the panel's model").clone();
            Ok::<_, String>(model)
        };
        let greeting = shown(0).unwrap();
        assert_eq!(
            (greeting.line.as_str(), greeting.next, greeting.first.shown),
            ("dialogue-guide-greet", true, false)
        );
        let asked = shown(1).unwrap();
        assert_eq!(asked.line, "dialogue-guide-ask");
        assert!(!asked.next);
        let answers = [&asked.first, &asked.second, &asked.third, &asked.fourth].map(|choice| choice.shown);
        assert_eq!(answers, [true, true, false, false], "asking again needs the ward lit");
        assert!(shown(2).unwrap_err().contains("wants an answer"));

        let options = ScreenOptions { dialogue: Some(1), ..ScreenOptions::of("dialogue") };
        assert_eq!(options.walks(), Some(WARD_GUIDE));
        let refused = options.stack(assets(PAUSE), Atlas::paint(), None, ScreenModel::Dialogue).map(drop);
        assert!(refused.unwrap_err().contains("loaded"));
    }

    fn nodes(stack: &ScreenStack) -> Vec<String> {
        stack.top().unwrap().strings().map(|(node, _)| node.to_owned()).collect()
    }

    #[test]
    fn a_document_replaces_the_tree_and_a_bad_one_keeps_it() {
        let mut stack = ScreenOptions::of("pause").stack(assets(PAUSE), Atlas::paint(), None, MENU).unwrap();
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
        let options = ScreenOptions { locale: "xx".to_owned(), ..ScreenOptions::of("pause") };
        let refused = options.stack(assets(PAUSE), Atlas::paint(), None, MENU).map(drop).unwrap_err();
        assert!(refused.contains("--ui-game") && refused.contains("xx"), "{refused}");
    }

    /// A screen is found by its name in the game's menus document (M3 U9), and one it doesn't have is
    /// refused with those it has; `--ui-dialogue` needs a screen reading a dialogue.
    #[test]
    fn a_screen_is_one_of_the_games_by_name() {
        let screens = MenuDocument::from_ron(include_str!("../../../content/ui/menus.ron")).unwrap();
        let (id, model) = ScreenOptions::of("character").find(&screens).unwrap();
        assert!(include_str!("../../../content/ui/character.ui.ron.meta").contains(&id.to_string()));
        assert_eq!(model, ScreenModel::Character);
        let refused = ScreenOptions::of("shop").find(&screens).unwrap_err();
        assert!(refused.contains("shop") && refused.contains("settings"), "{refused}");
        let options = ScreenOptions { dialogue: Some(0), ..ScreenOptions::of("pause") };
        assert!(options.find(&screens).unwrap_err().contains("--ui-dialogue"));
        let options = ScreenOptions { dialogue: Some(0), ..ScreenOptions::of("dialogue") };
        assert_eq!(options.find(&screens).unwrap().1, ScreenModel::Dialogue);
    }
}
