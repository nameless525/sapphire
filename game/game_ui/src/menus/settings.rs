//! What the player has chosen (M3 U7, M4 A1): [`PlayerSettings`], which the app keeps in `settings.ron`
//! (`sapphire_save::SettingsFile`), and what a press of a setting's row steps it to.
//!
//! Version 2 (M4 A1) adds the accessibility settings spec §6.8 and m4-plan §3 list. Each is stored, shown,
//! saved, migrated and stepped here; what each changes is A2's (the screen, the sound, the subtitles), A4's
//! (hold or toggle) and the run's (auto-target, and A6's timing assist, which a step reads, so the run records
//! them). Every other setting is presentation and never reaches a checksum. A6's assist joined version 4
//! with no bump: a file without it, of any version, reads it as off, as serde's defaults do.
//!
//! Version 3 (M4 T3) adds the graphics page's: the quality tier (automatic, or one the player holds), the frame
//! cap, dynamic resolution and the battery saver, which the app gives its stage's quality and its platform's
//! pacing. A phone's cap is 30 or 60: what a desktop's file says past that reads as 60 there.
//!
//! Version 4 (M4 A4) adds the bindings the player remapped, by name, over the game's document
//! ([`PlayerSettings::bindings_over`]): only what differs from it, so a game's change to a default reaches
//! every player who never moved that binding.
//!
//! A value that parses but makes no sense, a volume of 900, is read as the nearest that does. A name this
//! build doesn't have fails its field alone, which the settings file's repair gives its default.

use std::num::NonZeroU32;

use bevy_reflect::TypePath;
use sapphire_controls::Remapped;
use sapphire_core::target::FormFactor;
use sapphire_gameplay::assist::TimingAssist;
use sapphire_gameplay::targeting::AutoTarget;
use sapphire_input::InputBindings;
use sapphire_loc::LocaleSettings;
use sapphire_look::EffectScale;
use sapphire_reflect::{Decoder, VersionError, Versioned, decode_versioned};
use sapphire_render::quality::TierId;
use serde::{Deserialize, Serialize};

/// The languages the game ships, in the order the setting steps through them.
pub const LANGUAGES: [&str; 2] = ["en", "ja"];
/// The interface sizes, by the name a table has for each, in order, with what each scales points by.
pub const SIZES: [(&str, f32); 3] = [("small", 0.85), ("normal", 1.0), ("large", 1.2)];
/// How far a press of a volume moves it, in percent. It comes round past 100.
const VOLUME_STEP: u8 = 10;
/// How far a press of shake or camera motion moves it, in percent. It comes round past 100.
const MOTION_STEP: u8 = 25;

/// Says what's chosen by name, which its row's key ends in, and steps to the next of its kind.
macro_rules! choices {
    ($(#[$meta:meta])* $name:ident { $($(#[$variant_meta:meta])* $variant:ident = $key:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            /// Every one, in the order a press steps through them.
            pub const ALL: &[Self] = &[$(Self::$variant,)+];

            /// The end of its key: what the tables name it by.
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $key,)+
                }
            }

            /// The one a press steps to, round past the last.
            #[must_use]
            pub fn next(self) -> Self {
                next_of(Self::ALL, self)
            }
        }
    };
}

choices! {
    /// How large text is drawn, beyond the interface's size (A2 scales the styles by it).
    #[derive(Default)]
    TextSize { #[default] Normal = "normal", Large = "large", Larger = "larger" }
}

choices! {
    /// Whether the screen's flashes are as authored or reduced: WCAG 2.3.1's limits under reduced (A3).
    #[derive(Default)]
    Flashes { #[default] Normal = "normal", Reduced = "reduced" }
}

choices! {
    /// How large a subtitle's line is drawn.
    #[derive(Default)]
    SubtitleSize { Small = "small", #[default] Normal = "normal", Large = "large", Larger = "larger" }
}

choices! {
    /// The band behind a subtitle: none, or lighter, as authored, or solid.
    #[derive(Default)]
    SubtitleBackground { Off = "off", Light = "light", #[default] Normal = "normal", Solid = "solid" }
}

impl TextSize {
    /// What every screen's text and the HUD's digits are drawn times their size by, over the interface's
    /// size: the larger two are what Steam's and Xbox's guidelines ask a player be able to choose.
    #[must_use]
    pub const fn scale(self) -> f32 {
        match self {
            Self::Normal => 1.0,
            Self::Large => 1.25,
            Self::Larger => 1.5,
        }
    }
}

impl Flashes {
    /// The most the screen effects may move a pixel's relative luminance (M4 A3): as authored, or held under
    /// WCAG 2.3.1's general flash threshold ([`EffectScale::SAFE_FLASH_SWING`]).
    #[must_use]
    pub const fn swing(self) -> f32 {
        match self {
            Self::Normal => 1.0,
            Self::Reduced => EffectScale::SAFE_FLASH_SWING,
        }
    }
}

impl SubtitleSize {
    /// What a subtitle's words are drawn times their size by, with the player's text size on top.
    #[must_use]
    pub const fn scale(self) -> f32 {
        match self {
            Self::Small => 0.8,
            Self::Normal => 1.0,
            Self::Large => 1.25,
            Self::Larger => 1.5,
        }
    }
}

impl SubtitleBackground {
    /// How much of the band the HUD document authors behind a subtitle shows, 0 to 1, and whether an
    /// opaque one stands under it: none for off, half for light, all of it as authored, and solid.
    #[must_use]
    pub const fn band(self) -> (f32, bool) {
        match self {
            Self::Off => (0.0, false),
            Self::Light => (0.5, false),
            Self::Normal => (1.0, false),
            Self::Solid => (1.0, true),
        }
    }
}

choices! {
    /// Whether a held action is held down, or pressed once to hold and again to let go (A4's `Toggle`).
    #[derive(Default)]
    HoldMode { #[default] Hold = "hold", Toggle = "toggle" }
}

choices! {
    /// An action a move can read held (a `Held` cancel window), whose button the player may toggle: the
    /// verbs these games hold, an attack's charge, a skill's, a sprint off the dodge and a lock. Ultimate,
    /// swap and jump are presses in them.
    HeldAction { Light = "light", Heavy = "heavy", Dodge = "dodge", Skill = "skill", LockOn = "lock-on" }
}

choices! {
    /// The quality tier the game draws at (M4 T3): the device's, which the thermal ladder may lower in a long
    /// session and raise again, or the high or low tier of the device's class, held whatever the heat (the
    /// ladder then steps down only within it).
    #[derive(Default)]
    QualityChoice { #[default] Automatic = "automatic", High = "high", Low = "low" }
}

impl QualityChoice {
    /// The tier it holds on a device of `class`; `None` for automatic.
    #[must_use]
    pub const fn tier(self, class: FormFactor) -> Option<TierId> {
        match (self, class) {
            (Self::Automatic, _) => None,
            (Self::High, FormFactor::Desktop) => Some(TierId::DesktopHigh),
            (Self::Low, FormFactor::Desktop) => Some(TierId::DesktopLow),
            (Self::High, FormFactor::Mobile) => Some(TierId::MobileHigh),
            (Self::Low, FormFactor::Mobile) => Some(TierId::MobileLow),
        }
    }
}

choices! {
    /// The most frames a second the game draws (M4 T3, spec §6.2): 30, 60, 120 or as many as the display shows
    /// on a desktop, 30 or 60 on a phone. Under the tier's own rate and the thermal ladder's cap.
    #[derive(Default)]
    FrameCap { Thirty = "30", Sixty = "60", OneTwenty = "120", #[default] Unlimited = "unlimited" }
}

impl FrameCap {
    /// The caps a phone offers, in the order a press steps through them.
    pub const PHONE: &[Self] = &[Self::Thirty, Self::Sixty];

    /// What it is on a device of `class`: a phone's 120 and unlimited are its 60, the most it offers.
    #[must_use]
    pub fn on(self, class: FormFactor) -> Self {
        if class == FormFactor::Mobile && !Self::PHONE.contains(&self) { Self::Sixty } else { self }
    }

    /// The one a press steps to on a device of `class`.
    #[must_use]
    pub fn next_on(self, class: FormFactor) -> Self {
        match class {
            FormFactor::Desktop => self.next(),
            FormFactor::Mobile => next_of(Self::PHONE, self.on(class)),
        }
    }

    /// Frames a second at most; `None` for as many as the display shows.
    #[must_use]
    pub const fn rate(self) -> Option<NonZeroU32> {
        match self {
            Self::Thirty => NonZeroU32::new(30),
            Self::Sixty => NonZeroU32::new(60),
            Self::OneTwenty => NonZeroU32::new(120),
            Self::Unlimited => None,
        }
    }
}

impl HeldAction {
    /// Its action's name in the combat bindings (`sapphire_controls`' `combat_bindings.ron`).
    #[must_use]
    pub const fn action(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Heavy => "heavy",
            Self::Dodge => "dodge",
            Self::Skill => "skill",
            Self::LockOn => "lock_on",
        }
    }
}

/// The one after `current` in `all`, round past the last. One `all` doesn't hold starts it over.
fn next_of<T: Copy + PartialEq>(all: &[T], current: T) -> T {
    let at = all.iter().position(|one| *one == current).map_or(0, |at| at + 1);
    all[at % all.len()]
}

/// A percent stepped by `step`, coming round past 100 to 0.
fn step_percent(percent: u8, step: u8) -> u8 {
    if percent >= 100 { 0 } else { percent.saturating_add(step).min(100) }
}

/// What a cutscene's and a fight's words show as (A2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Subtitles {
    /// Whether they show at all.
    pub shown: bool,
    pub size: SubtitleSize,
    pub background: SubtitleBackground,
    /// Whether a line names who says it.
    pub speakers: bool,
}

impl Default for Subtitles {
    fn default() -> Self {
        Self {
            shown: true,
            size: SubtitleSize::default(),
            background: SubtitleBackground::default(),
            speakers: true,
        }
    }
}

/// Hold or toggle, for each action that can be held ([`HeldAction`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Holds {
    pub light: HoldMode,
    pub heavy: HoldMode,
    pub dodge: HoldMode,
    pub skill: HoldMode,
    pub lock_on: HoldMode,
}

impl Holds {
    /// What `action` is set to.
    #[must_use]
    pub const fn mode(&self, action: HeldAction) -> HoldMode {
        match action {
            HeldAction::Light => self.light,
            HeldAction::Heavy => self.heavy,
            HeldAction::Dodge => self.dodge,
            HeldAction::Skill => self.skill,
            HeldAction::LockOn => self.lock_on,
        }
    }

    /// Its mode, to change.
    pub fn mode_mut(&mut self, action: HeldAction) -> &mut HoldMode {
        match action {
            HeldAction::Light => &mut self.light,
            HeldAction::Heavy => &mut self.heavy,
            HeldAction::Dodge => &mut self.dodge,
            HeldAction::Skill => &mut self.skill,
            HeldAction::LockOn => &mut self.lock_on,
        }
    }
}

impl Holds {
    /// The actions set to toggle, by their names in the combat bindings.
    #[must_use]
    pub fn toggled(&self) -> Vec<&'static str> {
        HeldAction::ALL
            .iter()
            .filter(|action| self.mode(**action) == HoldMode::Toggle)
            .map(|action| action.action())
            .collect()
    }
}

/// What the player has chosen, which the app keeps between runs. Version 4 (M4 A4, and A6's timing assist); version 3 (M4 T3)
/// had all but the remapped bindings and the assist, version 2 (M4 A1) all but the graphics page's too, and version 1 (M3 U7) the
/// language, the size, one volume and the rumble. Each upgrades with everything it lacks at its default.
#[derive(Clone, Debug, PartialEq, TypePath, Serialize, Deserialize)]
#[type_path = "sapphire::game_ui"]
#[serde(default)]
pub struct PlayerSettings {
    /// Of [`LANGUAGES`].
    pub language: String,
    /// The interface's size, by name, of [`SIZES`].
    pub size: String,
    pub text_size: TextSize,
    /// How much of a hit's shake the camera and a pad's rumble play, in percent.
    pub shake: u8,
    pub flashes: Flashes,
    /// How much of a hit's camera punches and zooms play, in percent.
    pub camera_motion: u8,
    /// Every sound's volume, in percent.
    pub sound: u8,
    /// The music bus's, in percent, under [`Self::sound`].
    pub music: u8,
    /// The effects bus's, in percent, under [`Self::sound`].
    pub effects: u8,
    /// The voices' bus's, in percent, under [`Self::sound`].
    pub voice: u8,
    /// Whether every sound plays in one channel.
    pub mono: bool,
    /// Whether a pad shakes.
    pub rumble: bool,
    pub subtitles: Subtitles,
    /// How hard auto-target pulls on touch, which a step reads, so a run records it.
    pub auto_target: AutoTarget,
    pub holds: Holds,
    /// The quality tier: automatic, or one the player holds.
    pub quality: QualityChoice,
    pub frame_cap: FrameCap,
    /// Whether dynamic resolution lowers the render scale to keep the frame's time. Off, as before M4 T3.
    pub dynamic_resolution: bool,
    /// Draws at 30 frames a second, a step down the thermal ladder, to last longer on a battery.
    pub battery_saver: bool,
    /// The bindings the player remapped, each by its name in the game's document and only where it differs
    /// from the document (`sapphire_controls::set`).
    pub remapped: Vec<Remapped>,
    /// Whether the photosensitivity notice has been read (M4 A3): until it has, a launch with a player at it
    /// shows it over the title. The notice marks it ([`super::MenuCommand::FlashNoticeRead`]), and no step
    /// takes it back.
    pub flash_notice_read: bool,
    /// How far the perfect-dodge and counter windows widen (M4 A6), which a step reads, so a run records it.
    pub timing_assist: TimingAssist,
}

impl Versioned for PlayerSettings {
    /// 1: M3 U7. 2: M4 A1's accessibility settings. 3: M4 T3's graphics page. 4: M4 A4's remapped bindings.
    const VERSION: u32 = 4;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        decode_versioned::<PlayerSettingsV3, D>(version, data).map(|old| Self {
            language: old.language,
            size: old.size,
            text_size: old.text_size,
            shake: old.shake,
            flashes: old.flashes,
            camera_motion: old.camera_motion,
            sound: old.sound,
            music: old.music,
            effects: old.effects,
            voice: old.voice,
            mono: old.mono,
            rumble: old.rumble,
            subtitles: old.subtitles,
            auto_target: old.auto_target,
            holds: old.holds,
            quality: old.quality,
            frame_cap: old.frame_cap,
            dynamic_resolution: old.dynamic_resolution,
            battery_saver: old.battery_saver,
            remapped: Vec::new(),
            timing_assist: TimingAssist::Off,
        })
    }
}

/// [`PlayerSettings`] version 3 (M4 T3), frozen: what a player's file from before M4 A4 holds.
#[derive(Deserialize)]
#[serde(default)]
struct PlayerSettingsV3 {
    language: String,
    size: String,
    text_size: TextSize,
    shake: u8,
    flashes: Flashes,
    camera_motion: u8,
    sound: u8,
    music: u8,
    effects: u8,
    voice: u8,
    mono: bool,
    rumble: bool,
    subtitles: Subtitles,
    auto_target: AutoTarget,
    holds: Holds,
    quality: QualityChoice,
    frame_cap: FrameCap,
    dynamic_resolution: bool,
    battery_saver: bool,
}

impl Default for PlayerSettingsV3 {
    fn default() -> Self {
        let now = PlayerSettings::default();
        Self {
            language: now.language,
            size: now.size,
            text_size: now.text_size,
            shake: now.shake,
            flashes: now.flashes,
            camera_motion: now.camera_motion,
            sound: now.sound,
            music: now.music,
            effects: now.effects,
            voice: now.voice,
            mono: now.mono,
            rumble: now.rumble,
            subtitles: now.subtitles,
            auto_target: now.auto_target,
            holds: now.holds,
            quality: now.quality,
            frame_cap: now.frame_cap,
            dynamic_resolution: now.dynamic_resolution,
            battery_saver: now.battery_saver,
        }
    }
}

impl Versioned for PlayerSettingsV3 {
    const VERSION: u32 = 3;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        decode_versioned::<PlayerSettingsV2, D>(version, data).map(|old| Self {
            language: old.language,
            size: old.size,
            text_size: old.text_size,
            shake: old.shake,
            flashes: old.flashes,
            camera_motion: old.camera_motion,
            sound: old.sound,
            music: old.music,
            effects: old.effects,
            voice: old.voice,
            mono: old.mono,
            rumble: old.rumble,
            subtitles: old.subtitles,
            auto_target: old.auto_target,
            holds: old.holds,
            ..Self::default()
        })
    }
}

/// [`PlayerSettings`] version 2 (M4 A1), frozen: what a player's file from before M4 T3 holds.
#[derive(Deserialize)]
#[serde(default)]
struct PlayerSettingsV2 {
    language: String,
    size: String,
    text_size: TextSize,
    shake: u8,
    flashes: Flashes,
    camera_motion: u8,
    sound: u8,
    music: u8,
    effects: u8,
    voice: u8,
    mono: bool,
    rumble: bool,
    subtitles: Subtitles,
    auto_target: AutoTarget,
    holds: Holds,
}

impl Versioned for PlayerSettingsV2 {
    const VERSION: u32 = 2;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        decode_versioned::<PlayerSettingsV1, D>(version, data).map(|old| Self {
            language: old.language,
            size: old.size,
            sound: old.sound,
            rumble: old.rumble,
            ..Self::default()
        })
    }
}

impl Default for PlayerSettingsV2 {
    fn default() -> Self {
        let now = PlayerSettings::default();
        Self {
            language: now.language,
            size: now.size,
            text_size: now.text_size,
            shake: now.shake,
            flashes: now.flashes,
            camera_motion: now.camera_motion,
            sound: now.sound,
            music: now.music,
            effects: now.effects,
            voice: now.voice,
            mono: now.mono,
            rumble: now.rumble,
            subtitles: now.subtitles,
            auto_target: now.auto_target,
            holds: now.holds,
        }
    }
}

/// [`PlayerSettings`] version 1 (M3 U7), frozen: what a player's file from before M4 holds.
#[derive(Deserialize)]
#[serde(default)]
struct PlayerSettingsV1 {
    language: String,
    size: String,
    sound: u8,
    rumble: bool,
}

impl Versioned for PlayerSettingsV1 {
    const VERSION: u32 = 1;
}

impl Default for PlayerSettingsV1 {
    fn default() -> Self {
        let now = PlayerSettings::default();
        Self { language: now.language, size: now.size, sound: now.sound, rumble: now.rumble }
    }
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self {
            language: "en".to_owned(),
            size: "normal".to_owned(),
            text_size: TextSize::default(),
            shake: 100,
            flashes: Flashes::default(),
            camera_motion: 100,
            sound: 100,
            music: 100,
            effects: 100,
            voice: 100,
            mono: false,
            rumble: true,
            subtitles: Subtitles::default(),
            auto_target: AutoTarget::default(),
            holds: Holds::default(),
            quality: QualityChoice::default(),
            frame_cap: FrameCap::default(),
            dynamic_resolution: false,
            battery_saver: false,
            remapped: Vec::new(),
            flash_notice_read: false,
            timing_assist: TimingAssist::default(),
        }
    }
}

/// A setting a press steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Setting {
    Language,
    Size,
    TextSize,
    Shake,
    Flashes,
    CameraMotion,
    Sound,
    Music,
    Effects,
    Voice,
    Mono,
    Rumble,
    Subtitles,
    SubtitleSize,
    SubtitleBackground,
    Speakers,
    AutoTarget,
    Hold(HeldAction),
    Quality,
    /// A desktop's steps; a phone's menu steps [`FrameCap::next_on`] its own.
    FrameCap,
    DynamicResolution,
    BatterySaver,
    TimingAssist,
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

    /// `base`, a game's bindings document, with the player's over it: what they remapped, and the held actions
    /// they toggle. What the document can't take is dropped with a log line (`sapphire_controls::layered`).
    #[must_use]
    pub fn bindings_over(&self, base: &InputBindings) -> InputBindings {
        sapphire_controls::layered(base, &self.remapped, &self.holds.toggled())
    }

    /// A percent as a share, 0 to 1, read as 100 past it.
    #[must_use]
    pub fn share(percent: u8) -> f32 {
        f32::from(percent.min(100)) / 100.0
    }

    /// Every sound's volume as a share.
    #[must_use]
    pub fn volume(&self) -> f32 {
        Self::share(self.sound)
    }

    /// How much of combat's screen effects shows (M4 A2, A3): the flashes setting's swing, which post holds
    /// every effect to and which limits full-screen flashes to three a second, and camera motion's share of
    /// the zoom blur and colour punch.
    #[must_use]
    pub fn effect_scale(&self) -> EffectScale {
        EffectScale { flash_swing: self.flashes.swing(), motion: Self::share(self.camera_motion) }
    }

    /// How much of each shake and rumble plays, 0 to 1.
    #[must_use]
    pub fn shake_share(&self) -> f32 {
        Self::share(self.shake)
    }

    /// The languages the game reads and speaks in. Voices follow the chosen language: the engine keeps the
    /// two apart (M3 A1), but the settings screen has one control for both until a game wants a second.
    #[must_use]
    pub fn locale(&self) -> LocaleSettings {
        let language = self.known_language().to_owned();
        LocaleSettings { text: language.clone(), voice: language }
    }

    /// Steps `setting` on to its next value, round past its last.
    pub fn step(&mut self, setting: Setting) {
        match setting {
            Setting::Language => {
                let at = LANGUAGES.iter().position(|tag| *tag == self.known_language()).unwrap_or(0);
                self.language = LANGUAGES[(at + 1) % LANGUAGES.len()].to_owned();
            }
            Setting::Size => {
                let at = SIZES.iter().position(|(name, _)| *name == self.size).unwrap_or(1);
                self.size = SIZES[(at + 1) % SIZES.len()].0.to_owned();
            }
            Setting::TextSize => self.text_size = self.text_size.next(),
            Setting::Shake => self.shake = step_percent(self.shake, MOTION_STEP),
            Setting::Flashes => self.flashes = self.flashes.next(),
            Setting::CameraMotion => self.camera_motion = step_percent(self.camera_motion, MOTION_STEP),
            Setting::Sound => self.sound = step_percent(self.sound, VOLUME_STEP),
            Setting::Music => self.music = step_percent(self.music, VOLUME_STEP),
            Setting::Effects => self.effects = step_percent(self.effects, VOLUME_STEP),
            Setting::Voice => self.voice = step_percent(self.voice, VOLUME_STEP),
            Setting::Mono => self.mono = !self.mono,
            Setting::Rumble => self.rumble = !self.rumble,
            Setting::Subtitles => self.subtitles.shown = !self.subtitles.shown,
            Setting::SubtitleSize => self.subtitles.size = self.subtitles.size.next(),
            Setting::SubtitleBackground => self.subtitles.background = self.subtitles.background.next(),
            Setting::Speakers => self.subtitles.speakers = !self.subtitles.speakers,
            Setting::AutoTarget => self.auto_target = next_of(&AutoTarget::ALL, self.auto_target),
            Setting::Hold(action) => {
                let mode = self.holds.mode_mut(action);
                *mode = mode.next();
            }
            Setting::Quality => self.quality = self.quality.next(),
            Setting::FrameCap => self.frame_cap = self.frame_cap.next(),
            Setting::DynamicResolution => self.dynamic_resolution = !self.dynamic_resolution,
            Setting::BatterySaver => self.battery_saver = !self.battery_saver,
            Setting::TimingAssist => self.timing_assist = next_of(&TimingAssist::ALL, self.timing_assist),
        }
    }

    /// [`Self::step`] on a device of `class`: a phone's frame cap steps between its own two.
    pub fn step_on(&mut self, setting: Setting, class: FormFactor) {
        match setting {
            Setting::FrameCap => self.frame_cap = self.frame_cap.next_on(class),
            _ => self.step(setting),
        }
    }
}

/// The end of auto-target's key at `strength`.
pub(super) const fn auto_target_name(strength: AutoTarget) -> &'static str {
    match strength {
        AutoTarget::Weak => "weak",
        AutoTarget::Normal => "normal",
        AutoTarget::Strong => "strong",
    }
}

/// The end of the timing assist's key at `step`.
pub(super) const fn timing_assist_name(step: TimingAssist) -> &'static str {
    match step {
        TimingAssist::Off => "off",
        TimingAssist::Wide => "wide",
        TimingAssist::Wider => "wider",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults draw what the game drew before the settings acted (M4 A2): every effect, shake and
    /// rumble at full, every text at its own size, and the subtitles' band as authored.
    #[test]
    fn the_defaults_change_nothing_and_each_setting_scales_what_it_names() {
        let settings = PlayerSettings::default();
        assert_eq!(settings.effect_scale(), EffectScale::FULL);
        let sizes = (settings.shake_share(), settings.text_size.scale(), settings.subtitles.size.scale());
        assert_eq!(sizes, (1.0, 1.0, 1.0));
        assert_eq!(settings.subtitles.background.band(), (1.0, false));
        let calmer = PlayerSettings {
            flashes: Flashes::Reduced,
            camera_motion: 0,
            shake: 50,
            ..PlayerSettings::default()
        };
        assert_eq!(
            calmer.effect_scale(),
            EffectScale { flash_swing: EffectScale::SAFE_FLASH_SWING, motion: 0.0 }
        );
        assert_eq!(calmer.shake_share(), 0.5);
    }

    /// Every setting a press steps comes round to where it started, through every value it has, and moves
    /// nothing else.
    #[test]
    fn each_setting_comes_round_and_moves_only_itself() {
        let mut every = vec![
            Setting::Language,
            Setting::Size,
            Setting::TextSize,
            Setting::Shake,
            Setting::Flashes,
            Setting::CameraMotion,
            Setting::Sound,
            Setting::Music,
            Setting::Effects,
            Setting::Voice,
            Setting::Mono,
            Setting::Rumble,
            Setting::Subtitles,
            Setting::SubtitleSize,
            Setting::SubtitleBackground,
            Setting::Speakers,
            Setting::AutoTarget,
            Setting::Quality,
            Setting::FrameCap,
            Setting::DynamicResolution,
            Setting::BatterySaver,
            Setting::TimingAssist,
        ];
        every.extend(HeldAction::ALL.iter().map(|action| Setting::Hold(*action)));
        let start = PlayerSettings::default();
        for setting in every {
            let mut settings = start.clone();
            let mut seen = Vec::new();
            for _ in 0..12 {
                settings.step(setting);
                if settings == start {
                    break;
                }
                assert!(!seen.contains(&settings), "{setting:?} came round short of its start");
                seen.push(settings.clone());
            }
            assert_eq!(settings, start, "{setting:?} comes round");
            assert!(!seen.is_empty(), "{setting:?} has somewhere to go");
            let mut again = start.clone();
            again.step(setting);
            let changed = [
                again.language != start.language,
                again.size != start.size,
                again.text_size != start.text_size,
                again.shake != start.shake,
                again.flashes != start.flashes,
                again.camera_motion != start.camera_motion,
                again.sound != start.sound,
                again.music != start.music,
                again.effects != start.effects,
                again.voice != start.voice,
                again.mono != start.mono,
                again.rumble != start.rumble,
                again.subtitles != start.subtitles,
                again.auto_target != start.auto_target,
                again.holds != start.holds,
                again.quality != start.quality,
                again.frame_cap != start.frame_cap,
                again.dynamic_resolution != start.dynamic_resolution,
                again.battery_saver != start.battery_saver,
                again.flash_notice_read != start.flash_notice_read,
                again.timing_assist != start.timing_assist,
            ];
            assert_eq!(changed.iter().filter(|changed| **changed).count(), 1, "{setting:?}");
        }
    }

    #[test]
    fn a_percent_steps_to_100_and_comes_round() {
        assert_eq!(step_percent(90, VOLUME_STEP), 100);
        assert_eq!(step_percent(95, VOLUME_STEP), 100, "a hand-edited value lands on the top");
        assert_eq!(step_percent(100, VOLUME_STEP), 0);
        assert_eq!(step_percent(200, MOTION_STEP), 0, "past the top reads as the top");
        assert_eq!(step_percent(0, MOTION_STEP), 25);
        assert_eq!(PlayerSettings::share(250), 1.0);
    }

    /// M4 T3: a desktop steps through every cap, a phone between 30 and 60, and what a desktop's file says
    /// past 60 is a phone's 60; each choice holds its class's tier.
    #[test]
    fn a_phone_caps_at_30_or_60_and_a_choice_holds_its_class_s_tier() {
        let mut settings = PlayerSettings::default();
        assert_eq!(settings.frame_cap.rate(), None, "a desktop paces to its display, as before");
        let mut seen = Vec::new();
        for _ in 0..4 {
            settings.step_on(Setting::FrameCap, FormFactor::Desktop);
            seen.push(settings.frame_cap.rate().map(NonZeroU32::get));
        }
        assert_eq!(seen, [Some(30), Some(60), Some(120), None]);
        assert_eq!(FrameCap::Unlimited.on(FormFactor::Mobile), FrameCap::Sixty);
        assert_eq!(FrameCap::OneTwenty.on(FormFactor::Mobile), FrameCap::Sixty);
        assert_eq!(FrameCap::Thirty.on(FormFactor::Mobile), FrameCap::Thirty);
        let mut phone = PlayerSettings::default();
        let mut caps = Vec::new();
        for _ in 0..3 {
            phone.step_on(Setting::FrameCap, FormFactor::Mobile);
            caps.push(phone.frame_cap);
        }
        assert_eq!(caps, [FrameCap::Thirty, FrameCap::Sixty, FrameCap::Thirty]);
        assert_eq!(QualityChoice::Automatic.tier(FormFactor::Desktop), None);
        assert_eq!(QualityChoice::High.tier(FormFactor::Mobile), Some(TierId::MobileHigh));
        assert_eq!(QualityChoice::Low.tier(FormFactor::Desktop), Some(TierId::DesktopLow));
    }

    /// A4 maps each held action to its binding by name: each is an action the combat bindings have.
    #[test]
    fn each_held_action_names_a_combat_action() {
        let bindings = include_str!("../../../../crates/sapphire_controls/combat_bindings.ron");
        for action in HeldAction::ALL {
            let named = format!("(name: \"{}\", kind: Button)", action.action());
            assert!(bindings.contains(&named), "{action:?}");
        }
    }
}
