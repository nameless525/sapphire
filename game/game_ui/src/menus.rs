//! The game's menus (M3 U7): the title, the pause, the settings, the character screen, the dialogue panel
//! and the HUD's words, each a document in `content/ui/`, on one [`ScreenStack`].
//!
//! Which screens there are is content's (M3 U9): the game's [`MenuDocument`] (`content/ui/menus.ron`) lists
//! each screen's document, the model it reads, whether it's modal, what back does on it, and what each of its
//! rows does: a [`MenuCommand`] for whoever holds the menus, another screen, a setting stepped, or nothing
//! beyond what its machine does. A test holds every document's focusable nodes to its rows. A screen that
//! shows only the menus' own values ([`MenuModel`]) is documents alone. Rust models stay only for live game
//! values: a fighter's stats ([`CharacterModel`]), a conversation ([`DialogueModel`]) and the fight's HUD
//! ([`HudModel`]).
//!
//! [`GameMenus`] reads a menu's controls ([`MenuControls`]), hands them to the top screen and turns what
//! happened there into what its row says. Screens stay on the stack until their machine has taken them out.
//!
//! Every press a menu takes is one a fight mustn't: [`GameMenus::is_modal`] says when that's so, and its
//! holder keeps those presses from the fight's bindings. The HUD's document isn't modal, and takes only what
//! lands on its pause.

mod document;

use std::fmt::Write as _;

use bevy_reflect::{PartialReflect, Reflect, TypeInfo, Typed};
use sapphire_assets::ui::UiDocument;
use sapphire_controls::{MenuControls, MenuError};
use sapphire_loc::LocaleSettings;
use sapphire_look::{UiDrawList, UiPageState};
use sapphire_math::Vec2;
use sapphire_platform::{Event, Timestamp, Window};
use sapphire_reflect::Versioned;
use sapphire_stage::content::ScreenAssets;
use sapphire_ui::{Atlas, HudValues, UiEvent, UiHit, UiInput, UiMetrics, UiTree};
use serde::{Deserialize, Serialize};

pub use self::document::{MenuDocument, MenuScreen, RowAction, ScreenEntry, ScreenModel, ScreenRow};
use crate::screen::{ScreenStack, StackSetup};

/// The languages the game ships, in the order the setting steps through them.
pub const LANGUAGES: [&str; 2] = ["en", "ja"];
/// The interface sizes, by the name a table has for each, in order, with what each scales points by.
pub const SIZES: [(&str, f32); 3] = [("small", 0.85), ("normal", 1.0), ("large", 1.2)];
/// How far a press of the volume setting moves it, in percent. It comes round past 100.
const SOUND_STEP: u8 = 10;
/// The key of the window's title, the one string the game shows outside a screen (M3 Z5b): the app names
/// its window by it, in the menus' language ([`GameMenus::window_title`]).
pub const WINDOW_TITLE: &str = "window-title";

/// The screens [`GameMenus`]' holders open themselves, by name: a game's [`MenuDocument`] has to have them.
pub const OPENED_BY_CODE: [&str; 4] = ["title", "pause", "hud", "dialogue"];

impl ScreenModel {
    /// Its model as it stands with nothing yet said, which is also what a golden shows.
    #[must_use]
    pub fn sample(self) -> Box<dyn PartialReflect> {
        match self {
            Self::Menu => Box::new(MenuModel::of(&PlayerSettings::default(), true, true)),
            Self::Character => Box::new(CharacterModel::default()),
            Self::Dialogue => Box::new(DialogueModel::default()),
            Self::Hud => {
                Box::new(HudModel { touch: true, has_banner: true, can_skip: true, ..HudModel::default() })
            }
        }
    }

    /// Its type, which its model document (`content/ui/<`[`Self::file`]`>`) is made from.
    #[must_use]
    pub fn type_info(self) -> &'static TypeInfo {
        match self {
            Self::Menu => MenuModel::type_info(),
            Self::Character => CharacterModel::type_info(),
            Self::Dialogue => DialogueModel::type_info(),
            Self::Hud => HudModel::type_info(),
        }
    }
}

/// What any menu's paths can read (M3 U9): whether there's a way out and a run to go on with, and the
/// player's settings. The title, the pause and the settings read it, and so can any screen content adds.
#[derive(Clone, Debug, Default, Reflect)]
pub struct MenuModel {
    /// Whether this platform has a way out to offer: iOS doesn't.
    pub can_leave: bool,
    /// Whether there's a run to go on with: the title's row comes first, and with none, focus starts on a new
    /// game.
    pub can_continue: bool,
    /// The key of the language's name.
    pub language: String,
    /// The key of the size's name.
    pub size: String,
    /// In percent.
    pub sound: i32,
    /// The key of what it's set to.
    pub rumble: String,
}

impl MenuModel {
    fn of(settings: &PlayerSettings, can_leave: bool, can_continue: bool) -> Self {
        let mut model = Self { can_leave, can_continue, ..Self::default() };
        model.show_settings(settings);
        model
    }

    fn show_settings(&mut self, settings: &PlayerSettings) {
        self.language = format!("settings-language-{}", settings.known_language());
        self.size = format!("settings-size-{}", settings.size);
        self.sound = i32::from(settings.sound);
        self.rumble = format!("settings-rumble-{}", if settings.rumble { "on" } else { "off" });
    }
}

/// What the character screen's paths read.
#[derive(Clone, Debug, Reflect)]
pub struct CharacterModel {
    /// The key of their name.
    pub name: String,
    pub level: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub attack: i32,
    pub defense: i32,
    /// In percent.
    pub crit: i32,
    /// The key of what the skill with focus does.
    pub detail: String,
}

impl Default for CharacterModel {
    fn default() -> Self {
        Self {
            name: "character-name-wanderer".to_owned(),
            level: 12,
            hp: 6480,
            max_hp: 6480,
            attack: 412,
            defense: 268,
            crit: 8,
            detail: "character-detail-attack".to_owned(),
        }
    }
}

/// One answer on the dialogue panel.
#[derive(Clone, Debug, Default, Reflect)]
pub struct DialogueChoice {
    pub shown: bool,
    /// Its key.
    pub line: String,
}

/// What the dialogue panel's paths read. Each string is a key under `dialogue-`. With any answer shown there's
/// no going on without one, so `next` stays off. A conversation fills it from its view ([`Self::show`], M3
/// Q2b); with none it holds U7's sample.
#[derive(Clone, Debug, Reflect)]
pub struct DialogueModel {
    pub speaker: String,
    pub line: String,
    pub next: bool,
    pub first: DialogueChoice,
    pub second: DialogueChoice,
    pub third: DialogueChoice,
    pub fourth: DialogueChoice,
}

impl Default for DialogueModel {
    fn default() -> Self {
        let choice = |line: &str| DialogueChoice { shown: true, line: line.to_owned() };
        Self {
            speaker: "dialogue-sample-keeper".to_owned(),
            line: "dialogue-sample-line".to_owned(),
            next: false,
            first: choice("dialogue-sample-stay"),
            second: choice("dialogue-sample-ask"),
            third: choice("dialogue-sample-go"),
            fourth: choice("dialogue-sample-later"),
        }
    }
}

/// What the HUD's document's paths read.
#[derive(Clone, Debug, Reflect)]
pub struct HudModel {
    /// The key of the player's name.
    pub player: String,
    pub level: i32,
    /// The key of the enemy's.
    pub enemy: String,
    pub has_enemy: bool,
    /// The foe isn't a boss, so its name is `enemy`.
    pub names_enemy: bool,
    /// The key of a boss's name, `boss-<kind>`, as its kind of enemy names it: its bar's name in place of
    /// `enemy`. Empty reads the document's own.
    pub boss: String,
    pub has_boss: bool,
    /// The objective's key.
    pub objective: String,
    /// Whether it's played by touch, which has no key to pause with.
    pub touch: bool,
    /// The banner's key, under `hud-banner-`: a checkpoint reached, a victory, a defeat.
    pub banner: String,
    pub has_banner: bool,
    /// Whether what's playing can be skipped: a cutscene.
    pub can_skip: bool,
}

impl Default for HudModel {
    fn default() -> Self {
        Self {
            player: "character-name-wanderer".to_owned(),
            level: 12,
            enemy: "character-name-skeleton".to_owned(),
            has_enemy: true,
            names_enemy: true,
            boss: String::new(),
            has_boss: false,
            objective: "hud-objective-arena".to_owned(),
            touch: false,
            banner: "hud-banner-checkpoint".to_owned(),
            has_banner: false,
            can_skip: false,
        }
    }
}

impl HudModel {
    /// Follows the fight's bars: whether there's a foe, and its name, a boss's by `boss_name` (the key
    /// `game_combat`'s `FightShow::boss_name` gives) while its bar is a boss's. Allocates only for a boss's
    /// name longer than any before.
    pub fn follow_fight(&mut self, values: Option<&HudValues>, boss_name: Option<&str>) {
        self.has_enemy = values.is_some_and(|values| values.enemy.is_some());
        let boss = boss_name.filter(|_| values.is_some_and(|values| values.boss.is_some()));
        self.has_boss = boss.is_some();
        self.names_enemy = !self.has_boss;
        if let Some(boss) = boss
            && self.boss != boss
        {
            boss.clone_into(&mut self.boss);
        }
    }
}

/// What the player has chosen, which the app keeps between runs.
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[type_path = "sapphire::game_ui"]
#[serde(default)]
pub struct PlayerSettings {
    /// Of [`LANGUAGES`].
    pub language: String,
    /// By name, of [`SIZES`].
    pub size: String,
    /// In percent.
    pub sound: u8,
    /// Whether a pad shakes.
    pub rumble: bool,
}

impl Versioned for PlayerSettings {
    const VERSION: u32 = 1;
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self { language: "en".to_owned(), size: "normal".to_owned(), sound: 100, rumble: true }
    }
}

impl PlayerSettings {
    /// What the chosen size scales points by. A size no build has means what the setting starts at.
    #[must_use]
    pub fn scale(&self) -> f32 {
        SIZES.iter().find(|(name, _)| *name == self.size).map_or(1.0, |(_, scale)| *scale)
    }

    /// The chosen language, if this build has it.
    #[must_use]
    pub fn known_language(&self) -> &str {
        if LANGUAGES.contains(&self.language.as_str()) { &self.language } else { LANGUAGES[0] }
    }

    /// The volume as a share.
    #[must_use]
    pub fn volume(&self) -> f32 {
        f32::from(self.sound.min(100)) / 100.0
    }

    /// The languages the game reads and speaks in. Voices follow the chosen language: the engine keeps the
    /// two apart (M3 A1), but the settings screen has one control for both until a game wants a second.
    #[must_use]
    pub fn locale(&self) -> LocaleSettings {
        let language = self.known_language().to_owned();
        LocaleSettings { text: language.clone(), voice: language }
    }
}

/// How a [`GameMenus`] starts.
#[derive(Clone, Debug)]
pub struct MenuSetup {
    /// The game's screens.
    pub screens: MenuDocument,
    pub settings: PlayerSettings,
    /// Whether this platform has a way out to offer.
    pub can_leave: bool,
    /// Whether under pseudo-localization.
    pub pseudo: bool,
}

/// What the menus ask of whoever holds them. A game's flow takes most of them as its own commands
/// (`game_flow::flow_command`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuCommand {
    /// The title's new game.
    NewGame,
    /// The title's continue: the run there is to go on with.
    Continue,
    /// The pause's way back to the title.
    ToTitle,
    /// The HUD's skip, of a cutscene.
    Skip,
    /// The pause is over.
    Resume,
    /// The HUD's pause was pressed.
    Pause,
    /// The player left, from the title or the pause.
    Leave,
    /// A setting moved, and it's time to keep them ([`GameMenus::settings`]).
    SettingsChanged,
    /// The dialogue panel's next.
    DialogueNext,
    /// The dialogue panel's answer, from 0.
    DialogueChose(u8),
}

/// A setting a press steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Setting {
    Language,
    Size,
    Sound,
    Rumble,
}

/// What a row does, with the screen it opens found: what [`GameMenus`] acts on with nothing to allocate.
#[derive(Clone, Copy, Debug)]
enum Chosen {
    Command(MenuCommand),
    /// The screen at this place in the document.
    Open(usize),
    Step(Setting),
    Nothing,
}

/// See this module.
#[derive(Debug)]
pub struct GameMenus {
    stack: ScreenStack,
    /// Where in the document each of the stack's screens is.
    shown: Vec<usize>,
    /// The game's screens.
    screens: MenuDocument,
    /// In the document's order.
    documents: Vec<UiDocument>,
    controls: MenuControls,
    settings: PlayerSettings,
    can_leave: bool,
    can_continue: bool,
    commands: Vec<MenuCommand>,
    events: Vec<UiEvent>,
    /// The name of a focus timeline, kept to be written into.
    timeline: String,
    now: Timestamp,
}

impl GameMenus {
    /// Menus over `assets`, which `setup`'s screens' [`MenuDocument::ids`] asked for, sharing `atlas` with the
    /// HUD. Nothing shows until something [`Self::open`]s. The screens have to have those that are
    /// [`OPENED_BY_CODE`].
    pub fn new(assets: ScreenAssets, atlas: Atlas, setup: MenuSetup) -> Result<Self, String> {
        let MenuSetup { screens, settings, can_leave, pseudo } = setup;
        screens.require(&OPENED_BY_CODE)?;
        if assets.documents.len() != screens.screens.len() {
            return Err(format!(
                "{} screens and {} documents: the documents are the screens' `ids()`",
                screens.screens.len(),
                assets.documents.len()
            ));
        }
        let stack = ScreenStack::new(
            assets.tables,
            assets.fonts,
            atlas,
            StackSetup { locale: settings.known_language().to_owned(), pseudo, scale: settings.scale() },
        )?;
        let mut controls = MenuControls::with_defaults().map_err(|error: MenuError| error.to_string())?;
        controls.set_player_scale(settings.scale());
        Ok(Self {
            stack,
            shown: Vec::with_capacity(screens.screens.len()),
            screens,
            documents: assets.documents,
            controls,
            settings,
            can_leave,
            can_continue: false,
            commands: Vec::with_capacity(8),
            events: Vec::with_capacity(8),
            timeline: String::with_capacity(32),
            now: Timestamp::default(),
        })
    }

    /// The game's screens.
    #[must_use]
    pub fn screens(&self) -> &MenuDocument {
        &self.screens
    }

    /// Puts `screen` on top, opening from the frame's time.
    pub fn open(&mut self, screen: MenuScreen) {
        match self.screens.place(screen) {
            Some(place) => self.place(place, self.shown.len()),
            None => tracing::error!(target: crate::LOG_TARGET, ?screen, "no such screen to open"),
        }
    }

    /// Puts `screen` under whatever's showing, for what's behind them when they go: the HUD under a title.
    pub fn open_under(&mut self, screen: MenuScreen) {
        match self.screens.place(screen) {
            Some(place) => self.place(place, 0),
            None => tracing::error!(target: crate::LOG_TARGET, ?screen, "no such screen to open"),
        }
    }

    fn model_of(&self, kind: ScreenModel) -> Box<dyn PartialReflect> {
        match kind {
            ScreenModel::Menu => Box::new(MenuModel::of(&self.settings, self.can_leave, self.can_continue)),
            ScreenModel::Hud => Box::new(HudModel { touch: true, ..HudModel::default() }),
            other => other.sample(),
        }
    }

    fn place(&mut self, place: usize, index: usize) {
        let model = self.model_of(self.screens.screens[place].model);
        let document = &self.documents[place];
        // A document that's been cooked has passed everything a tree refuses.
        let placed = if index == self.shown.len() {
            self.stack.push(document, model, self.now, Vec::new())
        } else {
            self.stack.push_under(document, model, self.now)
        };
        match placed {
            Ok(()) => self.shown.insert(index, place),
            Err(error) => {
                let screen = &self.screens.screens[place].name;
                tracing::error!(target: crate::LOG_TARGET, %error, %screen, "a menu can't open");
            }
        }
    }

    /// Where on the stack `screen` is, if it's showing.
    fn index_of(&self, screen: MenuScreen) -> Option<usize> {
        let place = self.screens.place(screen)?;
        self.shown.iter().position(|shown| *shown == place)
    }

    fn entry_at(&self, index: usize) -> &ScreenEntry {
        &self.screens.screens[self.shown[index]]
    }

    /// Takes `screen` out through its machine, if it's showing.
    pub fn close(&mut self, screen: MenuScreen) {
        let Some(index) = self.index_of(screen) else { return };
        // One that's never been drawn has nothing to leave through, and would open at its first draw; one
        // with no machine, the HUD's, has none at all.
        if self.stack.has_opened(index) && self.stack.tree(index).state().is_some() {
            self.stack.tree_mut(index).fire("close", self.now);
        } else {
            self.stack.remove(index);
            self.shown.remove(index);
        }
    }

    /// Takes every modal screen out: all but the HUD's words.
    pub fn close_all(&mut self) {
        for index in 0..self.shown.len() {
            if self.entry_at(index).modal {
                self.stack.tree_mut(index).fire("close", self.now);
            }
        }
    }

    /// Whether any of the game's screens holds a 3D inset (M3 U6), opened or not: what declares the inset's
    /// pipeline before the character screen first opens, so it never builds on first use.
    #[must_use]
    pub fn holds_insets(&self) -> bool {
        self.documents.iter().any(|document| document.nodes.iter().any(|node| node.inset.is_some()))
    }

    /// Hands `visit` each inset the last [`Self::draw`] showed, as a `SceneShow` does.
    pub fn inset_shots(&self, visit: &mut dyn FnMut(sapphire_stage::InsetShot<'_>)) {
        self.stack.inset_shots(visit);
    }

    /// Turns the insets on show as a drag `across` that share of their width would: a stick's turn.
    pub fn turn_insets(&mut self, across: f32) {
        self.stack.turn_insets(across);
    }

    /// Whether `screen` is showing and isn't on its way out.
    #[must_use]
    pub fn is_open(&self, screen: MenuScreen) -> bool {
        self.index_of(screen).is_some_and(|index| self.stays(index))
    }

    /// Whether a screen reading `kind` of model is showing and isn't on its way out: the character screen,
    /// or any other a fighter shows on.
    #[must_use]
    pub fn shows(&self, kind: ScreenModel) -> bool {
        (0..self.shown.len()).any(|index| self.entry_at(index).model == kind && self.stays(index))
    }

    /// Whether the screen at `index` isn't on its way out, or out already and waiting to be taken away.
    fn stays(&self, index: usize) -> bool {
        self.stack.tree(index).state() != Some("leaving") && !self.stack.has_left(index)
    }

    /// Whether a screen that takes presses is up: while one is, a fight's bindings mustn't see them.
    #[must_use]
    pub fn is_modal(&self) -> bool {
        (0..self.shown.len()).any(|index| self.entry_at(index).modal && self.stays(index))
    }

    /// The controls, for a test's raw input and for rebinding.
    pub fn controls_mut(&mut self) -> &mut MenuControls {
        &mut self.controls
    }

    /// What the player has chosen.
    #[must_use]
    pub fn settings(&self) -> &PlayerSettings {
        &self.settings
    }

    /// What the window is called in the language the menus show, which follows the language setting.
    #[must_use]
    pub fn window_title(&self) -> &str {
        self.stack.text(WINDOW_TITLE)
    }

    /// Whether there's a run to go on with, which every menu's [`MenuModel::can_continue`] shows from now
    /// on. It allocates nothing, so a frame can say it.
    pub fn set_can_continue(&mut self, can_continue: bool) {
        self.can_continue = can_continue;
        self.visit_models(|model: &mut MenuModel| model.can_continue = can_continue);
    }

    /// The tree of `screen`, if it's showing.
    #[must_use]
    pub fn tree(&self, screen: MenuScreen) -> Option<&UiTree> {
        self.index_of(screen).map(|index| self.stack.tree(index))
    }

    /// The model of `screen`, if it's showing and it's a `T`.
    pub fn model_mut<T: PartialReflect>(&mut self, screen: MenuScreen) -> Option<&mut T> {
        let index = self.index_of(screen)?;
        self.stack.model_mut::<T>(index)
    }

    /// Hands `visit` the model of every screen showing that's a `T`: what fills a kind of model on whichever
    /// screens read it.
    pub fn visit_models<T: PartialReflect>(&mut self, mut visit: impl FnMut(&mut T)) {
        for index in 0..self.shown.len() {
            if let Some(model) = self.stack.model_mut::<T>(index) {
                visit(model);
            }
        }
    }

    /// A platform event, for the next [`Self::update`].
    pub fn handle_event(&mut self, event: &Event) {
        self.controls.handle_event(event);
    }

    /// Whether a landing at `share` of the window is on something of a screen that isn't modal (the HUD's
    /// words) that takes it: a touch the fight's zones must then leave alone.
    #[must_use]
    pub fn hud_takes(&self, share: Vec2) -> bool {
        (0..self.shown.len()).filter(|&index| !self.entry_at(index).modal).any(|index| {
            let tree = self.stack.tree(index);
            matches!(tree.hit(share * tree.size()), UiHit::Focusable(_))
        })
    }

    /// Follows `window`: what a finger's place means, and how large a point is.
    pub fn follow_window(&mut self, window: &Window) {
        let input = self.controls.input_mut();
        let size = window.size();
        input.set_window_size(size.width as f32, size.height as f32);
        input.set_points_per_coordinate(window.points_per_coordinate());
        self.stack.set_metrics(UiMetrics::of_window(window, self.settings.scale()));
    }

    /// The frame's input as of `now`, and what it did. The asks it leaves are in [`Self::take_commands`].
    pub fn update(&mut self, now: Timestamp) {
        profiling::scope!("GameMenus::update");
        self.now = now;
        self.stack.set_now(now);
        for index in (0..self.shown.len()).rev() {
            if self.stack.has_left(index) {
                self.stack.remove(index);
                self.shown.remove(index);
            }
        }
        let modal = self.is_modal();
        let input = self.controls.update(now);
        if modal {
            self.stack.input(&input);
        } else {
            self.stack.input(&UiInput { pointers: input.pointers, ..UiInput::default() });
        }
        self.read_events();
    }

    /// What `action` does, with the screen it opens found.
    fn chosen(&self, action: Option<&RowAction>) -> Chosen {
        match action {
            Some(RowAction::Command(command)) => Chosen::Command(*command),
            // The document's check has held every screen a row opens to one of its own.
            Some(RowAction::Open(name)) => self
                .screens
                .screens
                .iter()
                .position(|screen| screen.name == *name)
                .map_or(Chosen::Nothing, Chosen::Open),
            Some(RowAction::Step(setting)) => Chosen::Step(*setting),
            Some(RowAction::Nothing) | None => Chosen::Nothing,
        }
    }

    fn read_events(&mut self) {
        let Some(top) = self.shown.len().checked_sub(1) else { return };
        let mut events = std::mem::take(&mut self.events);
        self.stack.drain_events(&mut events);
        for event in events.drain(..) {
            let entry = self.entry_at(top);
            match event {
                UiEvent::Activated(node) => {
                    let chosen = self.chosen(entry.action(self.stack.tree(top).name(node)));
                    self.act(chosen);
                }
                UiEvent::Back => {
                    let chosen = self.chosen(entry.back.as_ref());
                    self.act(chosen);
                }
                UiEvent::FocusMoved { to: Some(node), .. } => self.focus_moved(top, node),
                UiEvent::FocusMoved { to: None, .. } => {}
            }
        }
        self.events = events;
    }

    fn act(&mut self, chosen: Chosen) {
        match chosen {
            Chosen::Command(command) => self.commands.push(command),
            Chosen::Open(place) => self.place(place, self.shown.len()),
            Chosen::Step(setting) => self.step(setting),
            Chosen::Nothing => {}
        }
    }

    /// What focus landing on a node does: its kick, and on a character's screen what the line under says.
    fn focus_moved(&mut self, top: usize, node: sapphire_ui::UiNodeId) {
        let character = self.entry_at(top).model == ScreenModel::Character;
        let tree = self.stack.tree(top);
        self.timeline.clear();
        let name = tree.name(node);
        let _ = write!(self.timeline, "focus-{name}");
        if character && let Some(skill) = name.strip_prefix("skill-") {
            let detail = format!("character-detail-{skill}");
            if let Some(model) = self.stack.model_mut::<CharacterModel>(top) {
                model.detail = detail;
            }
        }
        self.stack.tree_mut(top).play(&self.timeline, self.now);
    }

    /// Steps a setting on, shows it on every menu, and says it's time to keep them.
    fn step(&mut self, setting: Setting) {
        let settings = &mut self.settings;
        match setting {
            Setting::Language => {
                let at = LANGUAGES.iter().position(|tag| *tag == settings.known_language()).unwrap_or(0);
                settings.language = LANGUAGES[(at + 1) % LANGUAGES.len()].to_owned();
            }
            Setting::Size => {
                let at = SIZES.iter().position(|(name, _)| *name == settings.size).unwrap_or(1);
                settings.size = SIZES[(at + 1) % SIZES.len()].0.to_owned();
            }
            Setting::Sound => {
                settings.sound =
                    if settings.sound >= 100 { 0 } else { (settings.sound + SOUND_STEP).min(100) };
            }
            Setting::Rumble => settings.rumble = !settings.rumble,
        }
        let language = settings.known_language().to_owned();
        let scale = settings.scale();
        if let Err(error) = self.stack.set_locale(&language) {
            tracing::error!(target: crate::LOG_TARGET, %error, "the language can't change");
        }
        self.stack.set_scale(scale);
        self.controls.set_player_scale(scale);
        let settings = self.settings.clone();
        self.visit_models(|model: &mut MenuModel| model.show_settings(&settings));
        self.commands.push(MenuCommand::SettingsChanged);
    }

    /// What the menus have asked since this was last called.
    pub fn take_commands(&mut self) -> std::vec::Drain<'_, MenuCommand> {
        self.commands.drain(..)
    }

    /// What the renderer's glyph pages hold, for the next [`Self::draw`].
    pub fn follow_pages(&mut self, pages: UiPageState) {
        self.stack.follow_pages(pages);
    }

    /// Writes what's showing into `list`, for a target of `size` pixels.
    pub fn draw(&mut self, size: (u32, u32), list: &mut UiDrawList) {
        self.stack.draw(size, list);
    }
}
