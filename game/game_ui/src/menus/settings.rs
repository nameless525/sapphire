//! What the player has chosen (M3 U7, M4 A1): [`PlayerSettings`], which the app keeps in `settings.ron`
//! (`sapphire_save::SettingsFile`), and what a press of a setting's row steps it to.
//!
//! Version 2 (M4 A1) adds the accessibility settings spec §6.8 and m4-plan §3 list. Each is stored, shown,
//! saved, migrated and stepped here; what each changes is A2's (the screen, the sound, the subtitles), A4's
//! (hold or toggle) and the run's (auto-target, which a step reads, so the run records it). Every other
//! setting is presentation and never reaches a checksum.
//!
//! A value that parses but makes no sense, a volume of 900, is read as the nearest that does. A name this
//! build doesn't have fails its field alone, which the settings file's repair gives its default.

use bevy_reflect::TypePath;
use sapphire_gameplay::targeting::AutoTarget;
use sapphire_loc::LocaleSettings;
use sapphire_reflect::{Decoder, VersionError, Versioned, decode_versioned};
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

/// What the player has chosen, which the app keeps between runs. Version 2 (M4 A1); version 1 (M3 U7) had the
/// language, the size, one volume and the rumble, and upgrades with everything else at its default.
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
    /// How hard auto-target pulls on touch: the one setting a step reads, so a run records it.
    pub auto_target: AutoTarget,
    pub holds: Holds,
}

impl Versioned for PlayerSettings {
    /// 1: M3 U7. 2: M4 A1's accessibility settings.
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

#[cfg(test)]
mod tests {
    use super::*;

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
