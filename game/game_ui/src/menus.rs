//! The game's menus (M3 U7): the title, the pause, the settings, the character screen, the dialogue panel
//! and the HUD's words, each a document in `content/ui/` reading a model here, on one [`ScreenStack`].
//!
//! [`GameMenus`] reads a menu's controls ([`MenuControls`]), hands them to the top screen and turns what
//! happened there into what the screen means: another screen, a setting moved, or a [`MenuCommand`] for
//! whoever holds it. What a node does goes by its name, in [`action_of`], and a test holds every document's
//! focusable nodes to it. Screens stay on the stack until their machine has taken them out.
//!
//! Every press a menu takes is one a fight mustn't: [`GameMenus::is_modal`] says when that's so, and its
//! holder keeps those presses from the fight's bindings. The HUD's document isn't modal, and takes only what
//! lands on its pause.

use std::fmt::Write as _;

use bevy_reflect::{PartialReflect, Reflect};
use sapphire_assets::AssetId;
use sapphire_assets::ui::UiDocument;
use sapphire_controls::{MenuControls, MenuError};
use sapphire_loc::LocaleSettings;
use sapphire_look::{UiDrawList, UiPageState};
use sapphire_math::Vec2;
use sapphire_platform::{Event, Timestamp, Window};
use sapphire_reflect::Versioned;
use sapphire_stage::content::ScreenAssets;
use sapphire_ui::{Atlas, UiEvent, UiHit, UiInput, UiMetrics, UiTree};
use serde::{Deserialize, Serialize};

use crate::screen::{ScreenStack, StackSetup};

/// The languages the game ships, in the order the setting steps through them.
pub const LANGUAGES: [&str; 2] = ["en", "ja"];
/// The interface sizes, by the name a table has for each, in order, with what each scales points by.
pub const SIZES: [(&str, f32); 3] = [("small", 0.85), ("normal", 1.0), ("large", 1.2)];
/// How far a press of the volume setting moves it, in percent. It comes round past 100.
const SOUND_STEP: u8 = 10;

/// One of the game's screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuScreen {
    Title,
    Pause,
    Settings,
    Character,
    Dialogue,
    Hud,
}

impl MenuScreen {
    /// Every screen, in the order [`Self::ids`] has them.
    pub const ALL: [Self; 6] =
        [Self::Title, Self::Pause, Self::Settings, Self::Character, Self::Dialogue, Self::Hud];

    /// The documents, for `Content::load_screens`.
    #[must_use]
    pub const fn ids() -> [AssetId; 6] {
        [
            AssetId::from_text("a733bc8e-d4f9-4e85-a8d5-7c7d30b5e30d"),
            AssetId::from_text("2be53fec-1a42-4f0b-a6b4-f2781357c0fb"),
            AssetId::from_text("cf52ef55-199e-426f-adea-52940c953a9f"),
            AssetId::from_text("014b8262-3dda-4d77-a391-9ee209740962"),
            AssetId::from_text("1c4f98ba-d58a-46d3-b8df-8be091d9e5dc"),
            AssetId::from_text("a86b3aa5-faf3-4fb2-ab2c-27e9b0dae713"),
        ]
    }

    /// Its document.
    #[must_use]
    pub const fn id(self) -> AssetId {
        Self::ids()[self.index()]
    }

    /// Its document's source, from the repository's root: what live tuning watches.
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::Title => "content/ui/title.ui.ron",
            Self::Pause => "content/ui/pause.ui.ron",
            Self::Settings => "content/ui/settings.ui.ron",
            Self::Character => "content/ui/character.ui.ron",
            Self::Dialogue => "content/ui/dialogue.ui.ron",
            Self::Hud => "content/ui/hud.ui.ron",
        }
    }

    /// Its document's file name, without what follows.
    #[must_use]
    pub const fn stem(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Pause => "pause",
            Self::Settings => "settings",
            Self::Character => "character",
            Self::Dialogue => "dialogue",
            Self::Hud => "hud",
        }
    }

    /// The screen a name means, as `--ui-game` says it.
    #[must_use]
    pub fn from_stem(stem: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|screen| screen.stem() == stem)
    }

    /// Its model as it stands with nothing yet said, which is also what a golden shows.
    #[must_use]
    pub fn sample_model(self) -> Box<dyn PartialReflect> {
        match self {
            Self::Title => Box::new(TitleModel { can_leave: true, can_continue: true }),
            Self::Pause => Box::new(PauseModel { can_leave: true }),
            Self::Settings => Box::new(SettingsModel::of(&PlayerSettings::default())),
            Self::Character => Box::new(CharacterModel::default()),
            Self::Dialogue => Box::new(DialogueModel::default()),
            Self::Hud => {
                Box::new(HudModel { touch: true, has_banner: true, can_skip: true, ..HudModel::default() })
            }
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// What the title's paths read.
#[derive(Clone, Debug, Default, Reflect)]
pub struct TitleModel {
    /// Whether this platform has a way out to offer: iOS doesn't.
    pub can_leave: bool,
    /// Whether there's a run to go on with: its row comes first, and with none, focus starts on a new game.
    pub can_continue: bool,
}

/// What the pause's paths read.
#[derive(Clone, Debug, Default, Reflect)]
pub struct PauseModel {
    pub can_leave: bool,
}

/// What the settings' paths read.
#[derive(Clone, Debug, Default, Reflect)]
pub struct SettingsModel {
    /// The key of the language's name.
    pub language: String,
    /// The key of the size's name.
    pub size: String,
    /// In percent.
    pub sound: i32,
    /// The key of what it's set to.
    pub rumble: String,
}

impl SettingsModel {
    fn of(settings: &PlayerSettings) -> Self {
        Self {
            language: format!("settings-language-{}", settings.known_language()),
            size: format!("settings-size-{}", settings.size),
            sound: i32::from(settings.sound),
            rumble: format!("settings-rumble-{}", if settings.rumble { "on" } else { "off" }),
        }
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
            objective: "hud-objective-arena".to_owned(),
            touch: false,
            banner: "hud-banner-checkpoint".to_owned(),
            has_banner: false,
            can_skip: false,
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
    pub settings: PlayerSettings,
    /// Whether this platform has a way out to offer.
    pub can_leave: bool,
    /// Whether under pseudo-localization.
    pub pseudo: bool,
}

/// What the menus ask of whoever holds them. A game's flow takes most of them as its own commands
/// (`game_flow::flow_command`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// What a node with focus does when chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Command(MenuCommand),
    /// Another screen on top.
    Open(MenuScreen),
    /// Steps a setting.
    Step(Setting),
    /// The document's machine does all there is, by what the node fires.
    Nothing,
}

/// A setting a press steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Language,
    Size,
    Sound,
    Rumble,
}

/// What choosing the node `name` on `screen` does: `None` for a node nothing's been written for, which a test
/// keeps any document from having.
#[must_use]
pub fn action_of(screen: MenuScreen, name: &str) -> Option<MenuAction> {
    use MenuAction::{Command, Nothing, Open, Step};
    Some(match (screen, name) {
        (MenuScreen::Title, "continue") => Command(MenuCommand::Continue),
        (MenuScreen::Title, "new") => Command(MenuCommand::NewGame),
        (MenuScreen::Pause, "to-title") => Command(MenuCommand::ToTitle),
        (MenuScreen::Title | MenuScreen::Pause, "settings") => Open(MenuScreen::Settings),
        (MenuScreen::Title | MenuScreen::Pause, "leave") => Command(MenuCommand::Leave),
        (MenuScreen::Pause, "resume") => Command(MenuCommand::Resume),
        (MenuScreen::Pause, "character") => Open(MenuScreen::Character),
        (MenuScreen::Settings, "language") => Step(Setting::Language),
        (MenuScreen::Settings, "size") => Step(Setting::Size),
        (MenuScreen::Settings, "sound") => Step(Setting::Sound),
        (MenuScreen::Settings, "rumble") => Step(Setting::Rumble),
        (MenuScreen::Settings | MenuScreen::Character, "back") => Nothing,
        (MenuScreen::Character, "skill-attack" | "skill-dodge" | "skill-art") => Nothing,
        (MenuScreen::Dialogue, "next") => Command(MenuCommand::DialogueNext),
        (MenuScreen::Dialogue, "first") => Command(MenuCommand::DialogueChose(0)),
        (MenuScreen::Dialogue, "second") => Command(MenuCommand::DialogueChose(1)),
        (MenuScreen::Dialogue, "third") => Command(MenuCommand::DialogueChose(2)),
        (MenuScreen::Dialogue, "fourth") => Command(MenuCommand::DialogueChose(3)),
        (MenuScreen::Hud, "pause") => Command(MenuCommand::Pause),
        (MenuScreen::Hud, "skip") => Command(MenuCommand::Skip),
        _ => return None,
    })
}

/// See this module.
#[derive(Debug)]
pub struct GameMenus {
    stack: ScreenStack,
    /// Which screen each of the stack's is.
    shown: Vec<MenuScreen>,
    /// In [`MenuScreen::ALL`]'s order.
    documents: Vec<UiDocument>,
    controls: MenuControls,
    settings: PlayerSettings,
    can_leave: bool,
    commands: Vec<MenuCommand>,
    events: Vec<UiEvent>,
    /// The name of a focus timeline, kept to be written into.
    timeline: String,
    now: Timestamp,
}

impl GameMenus {
    /// Menus over `assets`, which [`MenuScreen::ids`] asked for, sharing `atlas` with the HUD. Nothing shows
    /// until something [`Self::open`]s.
    pub fn new(assets: ScreenAssets, atlas: Atlas, setup: MenuSetup) -> Result<Self, String> {
        let MenuSetup { settings, can_leave, pseudo } = setup;
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
            shown: Vec::with_capacity(MenuScreen::ALL.len()),
            documents: assets.documents,
            controls,
            settings,
            can_leave,
            commands: Vec::with_capacity(8),
            events: Vec::with_capacity(8),
            timeline: String::with_capacity(32),
            now: Timestamp::default(),
        })
    }

    /// Puts `screen` on top, opening from the frame's time.
    pub fn open(&mut self, screen: MenuScreen) {
        let model = self.model_of(screen);
        self.place(screen, model, self.shown.len());
    }

    /// Puts `screen` under whatever's showing, for what's behind them when they go: the HUD under a title.
    pub fn open_under(&mut self, screen: MenuScreen) {
        let model = self.model_of(screen);
        self.place(screen, model, 0);
    }

    fn model_of(&self, screen: MenuScreen) -> Box<dyn PartialReflect> {
        match screen {
            MenuScreen::Title => Box::new(TitleModel { can_leave: self.can_leave, can_continue: false }),
            MenuScreen::Pause => Box::new(PauseModel { can_leave: self.can_leave }),
            MenuScreen::Settings => Box::new(SettingsModel::of(&self.settings)),
            MenuScreen::Hud => Box::new(HudModel { touch: true, ..HudModel::default() }),
            other => other.sample_model(),
        }
    }

    fn place(&mut self, screen: MenuScreen, model: Box<dyn PartialReflect>, index: usize) {
        let document = &self.documents[screen.index()];
        // A document that's been cooked has passed everything a tree refuses.
        let placed = if index == self.shown.len() {
            self.stack.push(document, model, self.now, Vec::new())
        } else {
            self.stack.push_under(document, model, self.now)
        };
        match placed {
            Ok(()) => self.shown.insert(index, screen),
            Err(error) => tracing::error!(target: crate::LOG_TARGET, %error, ?screen, "a menu can't open"),
        }
    }

    /// Takes `screen` out through its machine, if it's showing.
    pub fn close(&mut self, screen: MenuScreen) {
        let Some(index) = self.shown.iter().position(|shown| *shown == screen) else { return };
        // One that's never been drawn has nothing to leave through, and would open at its first draw; one
        // with no machine, the HUD's, has none at all.
        if self.stack.has_opened(index) && self.stack.tree(index).state().is_some() {
            self.stack.tree_mut(index).fire("close", self.now);
        } else {
            self.stack.remove(index);
            self.shown.remove(index);
        }
    }

    /// Takes every screen but the HUD out.
    pub fn close_all(&mut self) {
        for index in 0..self.shown.len() {
            if self.shown[index] != MenuScreen::Hud {
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
        self.shown.iter().position(|shown| *shown == screen).is_some_and(|index| self.stays(index))
    }

    /// Whether the screen at `index` isn't on its way out, or out already and waiting to be taken away.
    fn stays(&self, index: usize) -> bool {
        self.stack.tree(index).state() != Some("leaving") && !self.stack.has_left(index)
    }

    /// Whether a screen that takes presses is up: while one is, a fight's bindings mustn't see them.
    #[must_use]
    pub fn is_modal(&self) -> bool {
        self.shown.iter().enumerate().any(|(index, screen)| *screen != MenuScreen::Hud && self.stays(index))
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

    /// The tree of `screen`, if it's showing.
    #[must_use]
    pub fn tree(&self, screen: MenuScreen) -> Option<&UiTree> {
        self.shown.iter().position(|shown| *shown == screen).map(|index| self.stack.tree(index))
    }

    /// The model of `screen`, if it's showing and it's a `T`.
    pub fn model_mut<T: PartialReflect>(&mut self, screen: MenuScreen) -> Option<&mut T> {
        let index = self.shown.iter().position(|shown| *shown == screen)?;
        self.stack.model_mut::<T>(index)
    }

    /// A platform event, for the next [`Self::update`].
    pub fn handle_event(&mut self, event: &Event) {
        self.controls.handle_event(event);
    }

    /// Whether a landing at `share` of the window is on something of the HUD's document that takes it: a touch
    /// the fight's zones must then leave alone.
    #[must_use]
    pub fn hud_takes(&self, share: Vec2) -> bool {
        self.tree(MenuScreen::Hud)
            .is_some_and(|tree| matches!(tree.hit(share * tree.size()), UiHit::Focusable(_)))
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

    fn read_events(&mut self) {
        let Some(top) = self.shown.len().checked_sub(1) else { return };
        let screen = self.shown[top];
        let mut events = std::mem::take(&mut self.events);
        self.stack.drain_events(&mut events);
        for event in events.drain(..) {
            match event {
                UiEvent::Activated(node) => {
                    let action = action_of(screen, self.stack.tree(top).name(node));
                    self.act(action);
                }
                UiEvent::Back => match screen {
                    MenuScreen::Title => self.commands.push(MenuCommand::Leave),
                    MenuScreen::Pause => self.commands.push(MenuCommand::Resume),
                    _ => {}
                },
                UiEvent::FocusMoved { to: Some(node), .. } => self.focus_moved(top, screen, node),
                UiEvent::FocusMoved { to: None, .. } => {}
            }
        }
        self.events = events;
    }

    fn act(&mut self, action: Option<MenuAction>) {
        match action {
            Some(MenuAction::Command(command)) => self.commands.push(command),
            Some(MenuAction::Open(screen)) => self.open(screen),
            Some(MenuAction::Step(setting)) => self.step(setting),
            Some(MenuAction::Nothing) | None => {}
        }
    }

    /// What focus landing on a node does: its kick, and on the character screen what the line under says.
    fn focus_moved(&mut self, top: usize, screen: MenuScreen, node: sapphire_ui::UiNodeId) {
        let tree = self.stack.tree(top);
        self.timeline.clear();
        let name = tree.name(node);
        let _ = write!(self.timeline, "focus-{name}");
        if screen == MenuScreen::Character
            && let Some(skill) = name.strip_prefix("skill-")
        {
            let detail = format!("character-detail-{skill}");
            if let Some(model) = self.stack.model_mut::<CharacterModel>(top) {
                model.detail = detail;
            }
        }
        self.stack.tree_mut(top).play(&self.timeline, self.now);
    }

    /// Steps a setting on, shows it, and says it's time to keep them.
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
        let shown = SettingsModel::of(&self.settings);
        if let Some(model) = self.model_mut::<SettingsModel>(MenuScreen::Settings) {
            *model = shown;
        }
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
