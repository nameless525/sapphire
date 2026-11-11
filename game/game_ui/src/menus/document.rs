//! The game's screens as content says (M3 U9): which there are, the document each shows, the model it reads,
//! and what each of its rows does. A screen that shows the menus' own values and takes the actions below is
//! documents alone: its `.ui.ron`, its sidecar and a row here.

use bevy_reflect::TypePath;
use game_combat::documents::error_chain;
use sapphire_assets::AssetId;
use sapphire_assets::documents::{GameDocuments, MANIFEST_FILE, SHIPPED_DIR, shipped_path};
use sapphire_controls::RemapDevice;
use sapphire_core::Name;
use sapphire_platform::ContentFs;
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use super::{MenuCommand, Setting};

/// One of the game's screens, by the name its row in the [`MenuDocument`] gives it, which is its document's
/// file name without what follows. The code names only the screens it opens itself; the rest are content's.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MenuScreen(Name);

impl MenuScreen {
    /// The title, which the game opens first.
    pub const TITLE: Self = Self::named("title");
    /// The pause, which a key, a pad's button or the HUD's opens.
    pub const PAUSE: Self = Self::named("pause");
    /// The HUD's words, under the fight.
    pub const HUD: Self = Self::named("hud");
    /// The panel a conversation opens.
    pub const DIALOGUE: Self = Self::named("dialogue");
    /// What a remap shows while it waits for the press (M4 A4), over the screen whose row began it.
    pub const REMAP_LISTEN: Self = Self::named("remap_listen");
    /// What a remap shows when the press is another binding's: swap the two, or keep them as they were.
    pub const REMAP_TAKEN: Self = Self::named("remap_taken");

    /// The screen named `name`.
    #[must_use]
    pub const fn named(name: &str) -> Self {
        Self(Name::new(name))
    }
}

impl std::fmt::Debug for MenuScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MenuScreen({:?})", self.0)
    }
}

/// The model a screen's document reads: the one Rust type for each kind of value a screen can show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenModel {
    /// What any menu may show: whether there's a way out and a run to continue, and the player's settings
    /// ([`MenuModel`](super::MenuModel), `menu.model.ron`). A screen that shows nothing else, or nothing at
    /// all, reads this.
    #[default]
    Menu,
    /// A fighter's stats and the skill with focus ([`CharacterModel`](super::CharacterModel)): live values
    /// the game sets.
    Character,
    /// A conversation's speaker, line and answers ([`DialogueModel`](super::DialogueModel)), from the
    /// dialogue runner.
    Dialogue,
    /// The fight's names, banner, skip and touch ([`HudModel`](super::HudModel)), from the fight and the
    /// flow.
    Hud,
}

impl ScreenModel {
    /// The kind whose model document is `file`, as a screen's document names it.
    #[must_use]
    pub fn of_file(file: &str) -> Option<Self> {
        [Self::Menu, Self::Character, Self::Dialogue, Self::Hud].into_iter().find(|kind| kind.file() == file)
    }

    /// Its model document's file name, beside the screens' documents.
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Self::Menu => "menu.model.ron",
            Self::Character => "character.model.ron",
            Self::Dialogue => "dialogue.model.ron",
            Self::Hud => "hud.model.ron",
        }
    }
}

/// What choosing a row does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowAction {
    /// Asks whoever holds the menus: a game's flow takes most of these as its own commands.
    Command(MenuCommand),
    /// Puts the screen of this name on top.
    Open(String),
    /// Steps a setting on.
    Step(Setting),
    /// The document's machine does all there is, by what the row fires: a way back, say.
    Nothing,
    /// Puts the screen of this name in this one's place: a tab.
    Switch(String),
    /// Remaps the binding of this name in the game's bindings document (M4 A4), which ends in its device's
    /// suffix (`light.gamepad`): the device's next press is what it reads, through [`REMAP_SCREENS`].
    Remap(String),
    /// The remap whose press another binding has goes ahead, and that one takes what this one read.
    Swap,
    /// The remap ends, with the bindings as they were.
    Keep,
    /// Every binding of the device goes back to the document.
    Reset(RemapDevice),
}

/// The screens a remap opens itself (M4 A4): a game whose rows remap anything has to have them.
pub const REMAP_SCREENS: [&str; 2] = ["remap_listen", "remap_taken"];

/// What a row of a screen does: the focusable node `node`, chosen, does `does`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenRow {
    pub node: String,
    pub does: RowAction,
}

/// One screen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenEntry {
    /// Its name: its document's file name without `.ui.ron`, which `--ui-game` takes.
    pub name: String,
    /// Its `.ui.ron`, by its sidecar's ID.
    pub document: AssetId,
    /// The model its document reads.
    #[serde(default)]
    pub model: ScreenModel,
    /// Whether it takes the presses a fight would, while it's up. Only the HUD's words don't, and take only
    /// what lands on them.
    #[serde(default = "modal")]
    pub modal: bool,
    /// What the menus' back does on it, beyond what its machine does: a title's leaves, a pause's resumes.
    #[serde(default)]
    pub back: Option<RowAction>,
    /// What each of its focusable nodes does. A test holds every focusable node of its document to a row.
    #[serde(default)]
    pub rows: Vec<ScreenRow>,
}

const fn modal() -> bool {
    true
}

impl ScreenEntry {
    /// What choosing its node `node` does: `None` for a node it names no row for.
    #[must_use]
    pub fn action(&self, node: &str) -> Option<&RowAction> {
        self.rows.iter().find(|row| row.node == node).map(|row| &row.does)
    }
}

/// The game's screens: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::game"]
#[serde(deny_unknown_fields)]
pub struct MenuDocument {
    pub screens: Vec<ScreenEntry>,
    /// Each screen's name, hashed, in `screens`' order: not in the document.
    #[serde(skip)]
    names: Vec<MenuScreen>,
}

impl Versioned for MenuDocument {
    /// 1: M3 U9.
    const VERSION: u32 = 1;
}

impl MenuDocument {
    /// Reads and checks a document.
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let mut document: Self = sapphire_serialize::from_ron(text).map_err(|error| error.to_string())?;
        document.names =
            document.screens.iter().map(|screen| MenuScreen(Name::intern(&screen.name))).collect();
        document.check()?;
        Ok(document)
    }

    /// The document the manifest a cook writes names as the game's `menus`, from `content`.
    pub fn read(content: &ContentFs) -> Result<Self, String> {
        Self::read_with(&|path| content.read(path).map_err(|error| error_chain(&error)))
    }

    /// [`Self::read`], with `read` giving a file's bytes from the content's root. A game that names none has
    /// no menus to show, which is an error: the game opens its title first.
    pub fn read_with(read: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Self, String> {
        let manifest_path = format!("{SHIPPED_DIR}/{MANIFEST_FILE}");
        let text = |path: &str| {
            read(path).and_then(|bytes| String::from_utf8(bytes).map_err(|error| error.to_string()))
        };
        let manifest = GameDocuments::from_ron(&text(&manifest_path)?)
            .map_err(|error| format!("{manifest_path}: {error}"))?;
        let menus = manifest.menus.ok_or_else(|| format!("{manifest_path} names no `menus`"))?;
        let path = shipped_path(&menus);
        Self::from_ron(&text(&path)?).map_err(|error| format!("the game's screens, {path}: {error}"))
    }

    /// What it can say by itself: names that name something, each once; each row once a screen; and each
    /// screen a row opens is one of its own.
    pub fn check(&self) -> Result<(), String> {
        for (index, screen) in self.screens.iter().enumerate() {
            if screen.name.is_empty() {
                return Err(format!("screens[{index}] has no name"));
            }
            if self.screens[..index].iter().any(|other| other.name == screen.name) {
                return Err(format!("two screens are named `{}`", screen.name));
            }
        }
        for screen in &self.screens {
            for (at, row) in screen.rows.iter().enumerate() {
                if screen.rows[..at].iter().any(|other| other.node == row.node) {
                    return Err(format!("`{}` says twice what `{}` does", screen.name, row.node));
                }
            }
            for action in screen.rows.iter().map(|row| &row.does).chain(&screen.back) {
                if let RowAction::Open(opened) | RowAction::Switch(opened) = action
                    && !self.screens.iter().any(|other| other.name == *opened)
                {
                    return Err(format!(
                        "`{}` opens `{opened}`, which isn't one of the screens",
                        screen.name
                    ));
                }
                if let RowAction::Remap(binding) = action {
                    if RemapDevice::of_binding(binding).is_none() {
                        return Err(format!(
                            "`{}` remaps `{binding}`, whose name ends in no device's suffix",
                            screen.name
                        ));
                    }
                    self.require(&REMAP_SCREENS)?;
                }
            }
        }
        Ok(())
    }

    /// An error naming the first of `needed` it has no screen by: the screens a holder opens itself.
    pub fn require(&self, needed: &[&str]) -> Result<(), String> {
        match needed.iter().find(|name| self.find(name).is_none()) {
            Some(name) => Err(format!("the game's screens have no `{name}`, which the game opens")),
            None => Ok(()),
        }
    }

    /// Every screen, in its order.
    pub fn all(&self) -> impl Iterator<Item = MenuScreen> + '_ {
        self.names.iter().copied()
    }

    /// Each screen's document, in its order: what `Content::load_screens` takes.
    #[must_use]
    pub fn ids(&self) -> Vec<AssetId> {
        self.screens.iter().map(|screen| screen.document).collect()
    }

    /// Where `screen` is in the list.
    #[must_use]
    pub fn place(&self, screen: MenuScreen) -> Option<usize> {
        self.names.iter().position(|named| *named == screen)
    }

    /// `screen`'s row, if it's one of these.
    #[must_use]
    pub fn entry(&self, screen: MenuScreen) -> Option<&ScreenEntry> {
        self.place(screen).map(|place| &self.screens[place])
    }

    /// The screen named `name`, if it's one of these.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<MenuScreen> {
        self.screens.iter().position(|screen| screen.name == name).map(|place| self.names[place])
    }

    /// The screen whose document is `id`, if it's one of these.
    #[must_use]
    pub fn of_document(&self, id: AssetId) -> Option<MenuScreen> {
        self.screens.iter().position(|screen| screen.document == id).map(|place| self.names[place])
    }

    /// Every screen's name, for a message.
    #[must_use]
    pub fn names(&self) -> String {
        self.screens.iter().map(|screen| screen.name.as_str()).collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(screens: &str) -> String {
        format!("(type: \"sapphire::game::MenuDocument\", version: 1, data: (screens: [{screens}]))")
    }

    const TWO: &str = r#"
        (name: "title", document: "a733bc8e-d4f9-4e85-a8d5-7c7d30b5e30d", back: Some(Command(Leave)),
         rows: [(node: "help", does: Open("help")), (node: "new", does: Command(NewGame))]),
        (name: "help", document: "2be53fec-1a42-4f0b-a6b4-f2781357c0fb", rows: [(node: "back", does: Nothing)]),
    "#;

    #[test]
    fn a_document_names_its_screens_and_what_their_rows_do() {
        let menus = MenuDocument::from_ron(&document(TWO)).expect("reads");
        assert_eq!(menus.all().collect::<Vec<_>>(), [MenuScreen::TITLE, MenuScreen::named("help")]);
        assert_eq!(menus.find("help"), Some(MenuScreen::named("help")));
        assert_eq!(menus.of_document(menus.screens[1].document), Some(MenuScreen::named("help")));
        assert_eq!(menus.place(MenuScreen::TITLE), Some(0));
        assert_eq!(menus.place(MenuScreen::PAUSE), None);
        let title = menus.entry(MenuScreen::TITLE).expect("the title");
        assert_eq!(title.action("help"), Some(&RowAction::Open("help".to_owned())));
        assert_eq!(title.action("gone"), None);
        assert_eq!((title.model, title.modal), (ScreenModel::Menu, true), "what a row leaves out");
        assert_eq!(menus.ids().len(), 2);
        assert!(menus.require(&["title"]).is_ok());
        let missing = menus.require(&["title", "pause"]).expect_err("no pause");
        assert!(missing.contains("`pause`"), "{missing}");
    }

    #[test]
    fn what_a_document_says_wrong_by_itself_is_refused_by_name() {
        let cases = [
            (TWO.replacen("Open(\"help\")", "Open(\"credits\")", 1), "`credits`"),
            (TWO.replacen("name: \"help\"", "name: \"title\"", 1), "two screens"),
            (TWO.replacen("name: \"help\"", "name: \"\"", 1), "no name"),
            (TWO.replacen("(node: \"new\"", "(node: \"help\"", 1), "twice"),
            (TWO.replacen("rows:", "rowz:", 1), "rowz"),
            (TWO.replacen("Command(NewGame)", "Remap(\"light.gamepad\")", 1), "`remap_listen`"),
            (TWO.replacen("Command(NewGame)", "Remap(\"light\")", 1), "suffix"),
            (TWO.replacen("Open(\"help\")", "Switch(\"credits\")", 1), "`credits`"),
        ];
        for (text, said) in cases {
            let refused = MenuDocument::from_ron(&document(&text)).expect_err(said);
            assert!(refused.contains(said), "{said}: {refused}");
        }
    }

    /// The cooker can't link this type, so the manifest holds its name as text, and this holds it to the type.
    #[test]
    fn the_manifest_names_this_type() {
        let manifest = GameDocuments { menus: Some("ui/menus.ron".to_owned()), ..GameDocuments::default() };
        let named: Vec<_> = manifest.named().map(|named| (named.field, named.type_name)).collect();
        assert_eq!(named, [("menus", sapphire_reflect::stable_name::<MenuDocument>())]);
    }
}
