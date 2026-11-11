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
//!
//! **Remapping (M4 A4):** a row that remaps a binding ([`RowAction::Remap`]) arms the menus' own input's
//! capture for that binding's device and opens [`MenuScreen::REMAP_LISTEN`]; the device's next press is the
//! binding's, unless it's Start or Escape, which end it, or another binding's. A press another remappable
//! binding has opens [`MenuScreen::REMAP_TAKEN`], to swap the two or keep them; one a binding no player
//! remaps has (the pause's, the camera's) is shown as kept for it, and the capture waits on. What the player
//! remapped goes into [`PlayerSettings::remapped`], and the glyphs every menu shows follow at once
//! ([`RemapModel`]), in the labels of the pad last used.

mod document;
mod settings;

use std::fmt::Write as _;

use bevy_reflect::{PartialReflect, Reflect, TypeInfo, Typed};
use sapphire_assets::ui::UiDocument;
use sapphire_controls::{MenuControls, MenuError, RemapDevice, Taken};
use sapphire_core::target::FormFactor;
use sapphire_input::{DeviceKind, GamepadFamily, InputBindings, Source};
use sapphire_look::{UiDrawList, UiPageState};
use sapphire_math::Vec2;
use sapphire_platform::{Event, Timestamp, Window};
use sapphire_stage::content::ScreenAssets;
use sapphire_ui::{Atlas, HudValues, UiEvent, UiHit, UiInput, UiMetrics, UiTree};
use serde::{Deserialize, Serialize};

pub use self::document::{
    MenuDocument, MenuScreen, REMAP_SCREENS, RowAction, ScreenEntry, ScreenModel, ScreenRow,
};
pub use self::settings::{
    Flashes, FrameCap, HeldAction, HoldMode, Holds, LANGUAGES, PlayerSettings, QualityChoice,
    REDUCED_FLASHES, SIZES, Setting, SubtitleBackground, SubtitleSize, Subtitles, TextSize,
};
use crate::screen::{ScreenStack, StackSetup};

/// The key of the window's title, the one string the game shows outside a screen (M3 Z5b): the app names
/// its window by it, in the menus' language ([`GameMenus::window_title`]).
pub const WINDOW_TITLE: &str = "window-title";

/// The screens [`GameMenus`]' holders open themselves, by name: a game's [`MenuDocument`] has to have them.
pub const OPENED_BY_CODE: [&str; 4] = ["title", "pause", "hud", "dialogue"];

impl ScreenModel {
    /// Its model as a golden shows it: as it stands with nothing yet said, but for the HUD, which shows every
    /// word it has.
    #[must_use]
    pub fn sample(self) -> Box<dyn PartialReflect> {
        match self {
            Self::Menu => Box::new(MenuModel::of(&PlayerSettings::default(), true, false, true)),
            Self::Character => Box::new(CharacterModel::default()),
            Self::Dialogue => Box::new(DialogueModel::default()),
            // Every word the HUD can say, the document's own: a foe's name and an objective too.
            Self::Hud => Box::new(HudModel {
                touch: true,
                has_enemy: true,
                has_objective: true,
                has_banner: true,
                can_skip: true,
                ..HudModel::default()
            }),
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

/// What a binding shows for each remappable action of one device (M4 A4): the glyph of what it reads, by its
/// binding's name before the device's suffix (`light` is `light.gamepad`'s on the pad), or nothing for one
/// the bindings don't have. A game that lets a player remap another binding adds its field here.
#[derive(Clone, Debug, Default, Reflect)]
pub struct BindingGlyphs {
    pub move_forward: String,
    pub move_back: String,
    pub move_left: String,
    pub move_right: String,
    pub light: String,
    pub heavy: String,
    pub dodge: String,
    pub skill: String,
    pub ultimate: String,
    pub swap: String,
    pub lock_on: String,
    pub jump: String,
}

impl BindingGlyphs {
    /// Each field's glyph, from what its binding of `device` reads in `bindings`, as `family` labels a pad.
    fn show(&mut self, bindings: &InputBindings, device: RemapDevice, family: GamepadFamily) {
        use bevy_reflect::structs::Struct as _;

        let mut name = String::new();
        for index in 0..self.field_len() {
            let Some(stem) = self.name_at(index) else { continue };
            name.clear();
            name.push_str(stem);
            name.push_str(device.suffix());
            let glyph = sapphire_controls::source_of(bindings, &name)
                .map(|source| sapphire_controls::glyph(source, family))
                .unwrap_or_default();
            if let Some(held) = self.field_at_mut(index).and_then(|field| field.try_downcast_mut::<String>())
                && *held != glyph
            {
                held.clear();
                held.push_str(&glyph);
            }
        }
    }
}

/// What the remap screens read (M4 A4): each binding's glyph, and the remap in progress. Until a remap
/// starts it holds a sample, light attack's press on the pad taken by heavy attack, which is what a golden
/// of the screens shows.
#[derive(Clone, Debug, Reflect)]
pub struct RemapModel {
    pub gamepad: BindingGlyphs,
    pub keyboard: BindingGlyphs,
    /// The key of the binding being remapped, under `remap-action-`.
    pub binding: String,
    /// The key of what to press, `remap-press-gamepad` or `remap-press-keyboard`.
    pub press: String,
    /// The glyph of the press that ends the remap: Start, or Escape.
    pub cancel: String,
    /// The glyph of the press the player made, which another binding has.
    pub pressed: String,
    /// The key of the action it's taken by, under `remap-action-`.
    pub taken_by: String,
    /// The press is a binding's no player remaps, so it's kept, and the remap waits on.
    pub kept: bool,
}

impl Default for RemapModel {
    fn default() -> Self {
        Self {
            gamepad: BindingGlyphs::default(),
            keyboard: BindingGlyphs::default(),
            binding: "remap-action-light".to_owned(),
            press: "remap-press-gamepad".to_owned(),
            cancel: sapphire_controls::glyph(&RemapDevice::Gamepad.cancel(), GamepadFamily::Xbox)
                .into_owned(),
            pressed: "B".to_owned(),
            taken_by: "remap-action-heavy".to_owned(),
            kept: false,
        }
    }
}

/// The key under `remap-action-` that names a binding or an action: its name before the device's suffix,
/// with words joined by hyphens, as keys are.
fn action_key(name: &str) -> String {
    let stem =
        RemapDevice::of_binding(name).map_or(name, |device| &name[..name.len() - device.suffix().len()]);
    format!("remap-action-{}", stem.replace('_', "-"))
}

/// What any menu's paths can read (M3 U9): whether there's a way out and a run to go on with, and the
/// player's settings (M4 A1 adds the settings' pages' own, A4 the bindings' glyphs), each a key of what it's
/// set to or a percent. The title, the pause and the settings read it, and so can any screen content adds.
#[derive(Clone, Debug, Default, Reflect)]
pub struct MenuModel {
    /// Whether this platform has a way out to offer: iOS doesn't.
    pub can_leave: bool,
    /// Whether the pause offers a mark: only a playtest build keeps one (M4 F1).
    pub can_mark: bool,
    /// Whether there's a run to go on with: the title's row comes first, and with none, focus starts on a new
    /// game.
    pub can_continue: bool,
    /// The key of the language's name.
    pub language: String,
    /// The key of the interface's size's name.
    pub size: String,
    pub text_size: String,
    /// In percent.
    pub shake: i32,
    pub flashes: String,
    /// In percent.
    pub camera_motion: i32,
    /// Every sound's volume, in percent.
    pub sound: i32,
    /// In percent.
    pub music: i32,
    /// In percent.
    pub effects: i32,
    /// In percent.
    pub voice: i32,
    pub mono: String,
    pub rumble: String,
    /// Whether subtitles show.
    pub subtitles: String,
    pub subtitle_size: String,
    pub subtitle_background: String,
    /// Whether a subtitle names its speaker.
    pub speakers: String,
    pub auto_target: String,
    pub hold_light: String,
    pub hold_heavy: String,
    pub hold_dodge: String,
    pub hold_skill: String,
    pub hold_lock_on: String,
    /// The graphics page's (M4 T3): the quality tier, the frame cap as this device has it, dynamic resolution
    /// and the battery saver.
    pub quality: String,
    pub frame_cap: String,
    pub dynamic_resolution: String,
    pub battery_saver: String,
    pub remap: RemapModel,
}

impl MenuModel {
    /// What a menu shows of `settings`, with a way out if `can_leave`, the pause's mark if `can_mark` (a playtest
    /// build's, M4 F1), a run to go on with if `can_continue`, and the glyphs of the compiled-in bindings with the
    /// player's remaps over them, on an Xbox pad.
    #[must_use]
    pub fn of(settings: &PlayerSettings, can_leave: bool, can_mark: bool, can_continue: bool) -> Self {
        let mut model = Self { can_leave, can_mark, can_continue, ..Self::default() };
        model.show_settings(settings);
        // The compiled-in document is in the build, so it reads; a failure would show empty glyphs.
        if let Ok(base) = sapphire_controls::default_bindings() {
            let bindings = sapphire_controls::layered(&base, &settings.remapped, &[]);
            model.show_bindings(&bindings, GamepadFamily::Xbox);
        }
        model
    }

    /// The glyph of each remappable binding in `bindings`, a pad's as `family` labels them.
    pub fn show_bindings(&mut self, bindings: &InputBindings, family: GamepadFamily) {
        self.remap.gamepad.show(bindings, RemapDevice::Gamepad, family);
        self.remap.keyboard.show(bindings, RemapDevice::Keyboard, family);
    }

    fn show_settings(&mut self, settings: &PlayerSettings) {
        let switch = |on: bool| format!("settings-switch-{}", if on { "on" } else { "off" });
        let held = |action| format!("controls-held-{}", settings.holds.mode(action).name());
        self.language = format!("settings-language-{}", settings.known_language());
        self.size = format!("display-size-{}", settings.size);
        self.text_size = format!("display-text-{}", settings.text_size.name());
        self.shake = i32::from(settings.shake);
        self.flashes = format!("display-flashes-{}", settings.flashes.name());
        self.camera_motion = i32::from(settings.camera_motion);
        self.sound = i32::from(settings.sound);
        self.music = i32::from(settings.music);
        self.effects = i32::from(settings.effects);
        self.voice = i32::from(settings.voice);
        self.mono = switch(settings.mono);
        self.rumble = switch(settings.rumble);
        self.subtitles = switch(settings.subtitles.shown);
        self.subtitle_size = format!("subtitles-size-{}", settings.subtitles.size.name());
        self.subtitle_background = format!("subtitles-background-{}", settings.subtitles.background.name());
        self.speakers = switch(settings.subtitles.speakers);
        self.auto_target =
            format!("controls-auto-target-{}", settings::auto_target_name(settings.auto_target));
        self.hold_light = held(HeldAction::Light);
        self.hold_heavy = held(HeldAction::Heavy);
        self.hold_dodge = held(HeldAction::Dodge);
        self.hold_skill = held(HeldAction::Skill);
        self.hold_lock_on = held(HeldAction::LockOn);
        self.quality = format!("graphics-quality-{}", settings.quality.name());
        self.frame_cap = format!("graphics-cap-{}", settings.frame_cap.on(FormFactor::CURRENT).name());
        self.dynamic_resolution = switch(settings.dynamic_resolution);
        self.battery_saver = switch(settings.battery_saver);
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
    /// The key of the foe's name, `enemy-<kind>`, as its kind of enemy names it ([`Self::follow_fight`]).
    /// Empty reads the document's own.
    pub enemy: String,
    /// Whether the fight has a foe for the HUD to name: none until [`Self::follow_fight`] says so.
    pub has_enemy: bool,
    /// The foe has a name and isn't a boss, so its name is `enemy`.
    pub names_enemy: bool,
    /// The key of a boss's name, `boss-<kind>`, as its kind of enemy names it: its bar's name in place of
    /// `enemy`. Empty reads the document's own.
    pub boss: String,
    pub has_boss: bool,
    /// The objective's key, under `hud-objective-`. Empty reads the document's own.
    pub objective: String,
    /// Whether the run gives an objective: none unless whoever stands the run says so.
    pub has_objective: bool,
    /// Whether it's played by touch, which has no key to pause with.
    pub touch: bool,
    /// The banner's key, under `hud-banner-`: a checkpoint reached, a victory, a defeat.
    pub banner: String,
    pub has_banner: bool,
    /// Whether what's playing can be skipped: a cutscene.
    pub can_skip: bool,
    /// The key of the line a cutscene says now, under `subtitle-`, as its timeline names it. Empty reads the
    /// document's own.
    pub subtitle: String,
    pub has_subtitle: bool,
    /// The key of who says it, under `speaker-`.
    pub speaker: String,
    pub has_speaker: bool,
    /// How much of the fight's words show, 0 to 1: its names and objective, which a cutscene hides.
    pub fight_words: f32,
    /// What a subtitle's words and its speaker's name are drawn times their size by (M4 A2).
    pub subtitle_scale: f32,
    /// How much of the band behind a subtitle shows, 0 to 1.
    pub subtitle_band: f32,
    /// Whether an opaque band stands under it.
    pub subtitle_solid: bool,
}

impl Default for HudModel {
    fn default() -> Self {
        Self {
            player: "character-name-wanderer".to_owned(),
            level: 12,
            enemy: String::new(),
            has_enemy: false,
            names_enemy: true,
            boss: String::new(),
            has_boss: false,
            objective: String::new(),
            has_objective: false,
            touch: false,
            banner: "hud-banner-checkpoint".to_owned(),
            has_banner: false,
            can_skip: false,
            subtitle: String::new(),
            has_subtitle: false,
            speaker: String::new(),
            has_speaker: false,
            fight_words: 1.0,
            subtitle_scale: 1.0,
            subtitle_band: 1.0,
            subtitle_solid: false,
        }
    }
}

/// Who a fight's voice line said by the player's side is captioned as (M4 A2): the player's character, by
/// the speaker key the cutscenes name them by.
pub const PLAYER_SPEAKER: &str = "speaker-wanderer";

impl HudModel {
    /// Follows the fight's bars: whether there's a foe, and its name, `foe_name` (the key `game_combat`'s
    /// `FightShow::foe_name` gives, as the foe's kind of enemy names it), which is `boss` while its bar is a
    /// boss's and `enemy` otherwise (M4 X2). A foe with no name shows none, rather than another's. Allocates
    /// only for a name longer than any before.
    pub fn follow_fight(&mut self, values: Option<&HudValues>, foe_name: Option<&str>) {
        self.has_enemy = values.is_some_and(|values| values.enemy.is_some());
        let name = foe_name.filter(|_| self.has_enemy);
        self.has_boss = name.is_some() && values.is_some_and(|values| values.boss.is_some());
        self.names_enemy = name.is_some() && !self.has_boss;
        let held = if self.has_boss { &mut self.boss } else { &mut self.enemy };
        if let Some(name) = name
            && held != name
        {
            name.clone_into(held);
        }
    }

    /// Follows the player's choices for subtitles (M4 A2): their size, and the band behind them. Whether
    /// they show, and their speakers, is [`Self::follow_subtitle`]'s caller's to filter.
    pub fn follow_subtitle_look(&mut self, subtitles: &Subtitles) {
        self.subtitle_scale = subtitles.size.scale();
        (self.subtitle_band, self.subtitle_solid) = subtitles.background.band();
    }

    /// Follows what a cutscene says at its playhead: `line`'s key and, if it names one, its speaker's (the
    /// keys `sapphire_sequencer`'s `Subtitle` holds), or nothing. Allocates only for a key longer than any
    /// before.
    pub fn follow_subtitle(&mut self, line: Option<&str>, speaker: Option<&str>) {
        let speaker = speaker.filter(|_| line.is_some());
        self.has_subtitle = line.is_some();
        self.has_speaker = speaker.is_some();
        for (held, key) in [(&mut self.subtitle, line), (&mut self.speaker, speaker)] {
            if let Some(key) = key
                && held.as_str() != key
            {
                key.clone_into(held);
            }
        }
    }
}

/// How a [`GameMenus`] starts.
#[derive(Clone, Debug)]
pub struct MenuSetup {
    /// The game's screens.
    pub screens: MenuDocument,
    pub settings: PlayerSettings,
    /// The game's bindings document, which the player's remaps lie over (M4 A4): the compiled-in one, or the
    /// game's own.
    pub bindings: InputBindings,
    /// Whether this platform has a way out to offer.
    pub can_leave: bool,
    /// Whether the pause offers a mark, which only a playtest build keeps (M4 F1).
    pub can_mark: bool,
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
    /// The pause's mark (M4 F1): a playtest keeps the run's tick, to replay from, and the run goes on.
    Mark,
}

/// What a row does, with the screen it opens found: what [`GameMenus`] acts on with nothing to allocate but a
/// remap's binding.
#[derive(Clone, Debug)]
enum Chosen {
    Command(MenuCommand),
    /// The screen at this place in the document.
    Open(usize),
    /// The screen at this place in the document, in place of the top one.
    Switch(usize),
    Step(Setting),
    Remap(String),
    Swap,
    Keep,
    Reset(RemapDevice),
    Nothing,
}

/// A remap on its way (M4 A4).
#[derive(Clone, Debug, PartialEq)]
enum Remapping {
    /// Waiting for `device`'s next press for `binding`, on [`MenuScreen::REMAP_LISTEN`].
    Listening { binding: String, device: RemapDevice },
    /// The press, `source`, is `other`'s: on [`MenuScreen::REMAP_TAKEN`], a swap gives `other` what `binding`
    /// read.
    Taken { binding: String, source: Source, other: String },
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
    /// The game's bindings document, and the player's remaps over it: what glyphs and conflicts read.
    base: InputBindings,
    bindings: InputBindings,
    /// Whose labels a pad's glyphs show: the pad last used's, an Xbox pad's until one is.
    family: GamepadFamily,
    remap: Option<Remapping>,
    /// What every menu shows of the bindings and the remap, kept here for the screens that open later.
    remap_model: RemapModel,
    can_leave: bool,
    can_mark: bool,
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
        let MenuSetup { screens, settings, bindings: base, can_leave, can_mark, pseudo } = setup;
        screens.require(&OPENED_BY_CODE)?;
        if assets.documents.len() != screens.screens.len() {
            return Err(format!(
                "{} screens and {} documents: the documents are the screens' `ids()`",
                screens.screens.len(),
                assets.documents.len()
            ));
        }
        let mut stack = ScreenStack::new(
            assets.tables,
            assets.fonts,
            atlas,
            StackSetup { locale: settings.known_language().to_owned(), pseudo, scale: settings.scale() },
        )?;
        stack.set_text_scale(settings.text_size.scale());
        let mut controls = MenuControls::with_defaults().map_err(|error: MenuError| error.to_string())?;
        controls.set_player_scale(settings.scale());
        let bindings = sapphire_controls::layered(&base, &settings.remapped, &[]);
        let mut shown = MenuModel::default();
        shown.show_bindings(&bindings, GamepadFamily::Xbox);
        Ok(Self {
            stack,
            shown: Vec::with_capacity(screens.screens.len()),
            screens,
            documents: assets.documents,
            controls,
            settings,
            base,
            bindings,
            family: GamepadFamily::Xbox,
            remap: None,
            remap_model: shown.remap,
            can_leave,
            can_mark,
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
            ScreenModel::Menu => {
                let (can_leave, can_mark, can_continue) = (self.can_leave, self.can_mark, self.can_continue);
                let mut model = MenuModel {
                    can_leave,
                    can_mark,
                    can_continue,
                    remap: self.remap_model.clone(),
                    ..MenuModel::default()
                };
                model.show_settings(&self.settings);
                Box::new(model)
            }
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

    /// Draws, where there's no window, as a display of `scale` pixels per point would
    /// ([`ScreenStack::set_display_scale`]).
    pub fn set_display_scale(&mut self, scale: f32) {
        self.stack.set_display_scale(scale);
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
        self.follow_family();
        self.follow_capture();
        self.read_events();
    }

    /// Whether a remap is waiting for a press.
    #[must_use]
    pub fn is_remapping(&self) -> bool {
        self.remap.is_some()
    }

    /// The glyphs follow the labels of the pad last used.
    fn follow_family(&mut self) {
        if let Some(DeviceKind::Gamepad(family)) = self.controls.input().last_device()
            && family != self.family
        {
            self.family = family;
            self.show_bindings();
        }
    }

    /// Every menu shows the remap as it stands.
    fn show_remap(&mut self) {
        let shown = std::mem::take(&mut self.remap_model);
        self.visit_models(|model: &mut MenuModel| model.remap.clone_from(&shown));
        self.remap_model = shown;
    }

    /// Takes the press a remap waits for, if it came this update.
    fn follow_capture(&mut self) {
        let Some(Remapping::Listening { binding, device }) = &self.remap else { return };
        let (binding, device) = (binding.clone(), *device);
        let Some(source) = self.controls.input_mut().take_captured() else { return };
        if source == device.cancel() {
            self.keep();
            return;
        }
        if !device.accepts(&source) {
            // A stick: not a button, so the remap waits on.
            self.controls.input_mut().capture_next(device.capture());
            return;
        }
        self.remap_model.pressed = sapphire_controls::glyph(&source, self.family).into_owned();
        match sapphire_controls::taken(&self.bindings, &binding, &source) {
            None => {
                sapphire_controls::set(&mut self.settings.remapped, &self.base, &binding, source);
                self.remap = None;
                self.close(MenuScreen::REMAP_LISTEN);
                self.bindings_changed();
            }
            Some(Taken::Kept { action }) => {
                self.remap_model.taken_by = action_key(action);
                self.remap_model.kept = true;
                self.show_remap();
                self.controls.input_mut().capture_next(device.capture());
            }
            Some(Taken::Swappable { binding: other, .. }) => {
                self.remap_model.taken_by = action_key(other);
                self.remap_model.kept = false;
                self.remap = Some(Remapping::Taken { binding, source, other: other.to_owned() });
                self.show_remap();
                self.close(MenuScreen::REMAP_LISTEN);
                self.open(MenuScreen::REMAP_TAKEN);
            }
        }
    }

    /// Starts remapping `binding`: its device's next press is what it reads.
    fn start_remap(&mut self, binding: String) {
        if self.remap.is_some() {
            return;
        }
        let Some(device) = RemapDevice::of_binding(&binding) else { return };
        if sapphire_controls::source_of(&self.bindings, &binding).is_none() {
            tracing::error!(target: crate::LOG_TARGET, %binding, "the bindings have no binding by that name to remap");
            return;
        }
        let shown = &mut self.remap_model;
        shown.binding = action_key(&binding);
        shown.press = format!("remap-press-{}", device.name());
        shown.cancel = sapphire_controls::glyph(&device.cancel(), self.family).into_owned();
        shown.kept = false;
        self.show_remap();
        self.controls.input_mut().capture_next(device.capture());
        self.remap = Some(Remapping::Listening { binding, device });
        self.open(MenuScreen::REMAP_LISTEN);
    }

    /// The remap that found its press taken goes ahead, and the other binding takes what this one read.
    fn swap(&mut self) {
        let Some(Remapping::Taken { binding, source, other }) = self.remap.take() else { return };
        let was = sapphire_controls::source_of(&self.bindings, &binding).cloned();
        let remapped = &mut self.settings.remapped;
        sapphire_controls::set(remapped, &self.base, &binding, source);
        if let Some(was) = was {
            sapphire_controls::set(remapped, &self.base, &other, was);
        }
        self.close(MenuScreen::REMAP_TAKEN);
        self.bindings_changed();
    }

    /// The remap ends, with nothing changed.
    fn keep(&mut self) {
        self.remap = None;
        self.controls.input_mut().cancel_capture();
        self.close(MenuScreen::REMAP_LISTEN);
        self.close(MenuScreen::REMAP_TAKEN);
    }

    /// The player's bindings moved: every menu's glyphs follow, and it's time to keep them.
    fn bindings_changed(&mut self) {
        self.bindings = sapphire_controls::layered(&self.base, &self.settings.remapped, &[]);
        self.show_bindings();
        self.commands.push(MenuCommand::SettingsChanged);
    }

    fn show_bindings(&mut self) {
        let mut shown = MenuModel { remap: std::mem::take(&mut self.remap_model), ..MenuModel::default() };
        shown.show_bindings(&self.bindings, self.family);
        self.remap_model = shown.remap;
        self.show_remap();
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
            Some(RowAction::Switch(name)) => self
                .screens
                .screens
                .iter()
                .position(|screen| screen.name == *name)
                .map_or(Chosen::Nothing, Chosen::Switch),
            Some(RowAction::Step(setting)) => Chosen::Step(*setting),
            Some(RowAction::Remap(binding)) => Chosen::Remap(binding.clone()),
            Some(RowAction::Swap) => Chosen::Swap,
            Some(RowAction::Keep) => Chosen::Keep,
            Some(RowAction::Reset(device)) => Chosen::Reset(*device),
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
            Chosen::Switch(place) => {
                if let Some(top) = self.shown.len().checked_sub(1) {
                    self.stack.tree_mut(top).fire("close", self.now);
                }
                self.place(place, self.shown.len());
            }
            Chosen::Step(setting) => self.step(setting),
            Chosen::Remap(binding) => self.start_remap(binding),
            Chosen::Swap => self.swap(),
            Chosen::Keep => self.keep(),
            Chosen::Reset(device) => {
                sapphire_controls::reset(&mut self.settings.remapped, device);
                self.bindings_changed();
            }
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
        settings.step_on(setting, FormFactor::CURRENT);
        let language = settings.known_language().to_owned();
        let scale = settings.scale();
        if let Err(error) = self.stack.set_locale(&language) {
            tracing::error!(target: crate::LOG_TARGET, %error, "the language can't change");
        }
        self.stack.set_scale(scale);
        self.stack.set_text_scale(settings.text_size.scale());
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
