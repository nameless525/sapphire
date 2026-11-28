//! The settings file's versions (M4 A1): what an older build wrote reads into this one, and what this one
//! writes reads back. Version 1's bytes are frozen here as M3's build wrote them, version 2's as M4 A1's,
//! version 3's as M4 T3's and version 4's as M6 U10's, so a change to how they upgrade shows as a failure rather
//! than as a player's lost settings.

use game_ui::menus::{
    DynamicRange, FrameCap, HeldAction, HoldMode, PlayerSettings, QualityChoice, Setting, SubtitleSize,
    TextSize,
};
use sapphire_controls::Remapped;
use sapphire_gameplay::assist::TimingAssist;
use sapphire_gameplay::targeting::AutoTarget;
use sapphire_input::{Key, PadButton, Source};
use sapphire_reflect::Versioned;
use sapphire_serialize::{from_ron, to_ron};

/// M3's `settings.ron`, byte for byte: Japanese, the large interface, the volume at 40, rumble off.
const VERSION_1: &str = r#"(
    type: "sapphire::game_ui::PlayerSettings",
    version: 1,
    data: (
        language: "ja",
        size: "large",
        sound: 40,
        rumble: false,
    ),
)
"#;

/// M4 A1's `settings.ron`, as that build wrote it: every accessibility setting off its default.
const VERSION_2: &str = r#"(
    type: "sapphire::game_ui::PlayerSettings",
    version: 2,
    data: (
        language: "ja",
        size: "small",
        text_size: Larger,
        shake: 50,
        flashes: Reduced,
        camera_motion: 25,
        sound: 70,
        music: 60,
        effects: 50,
        voice: 40,
        mono: true,
        rumble: false,
        subtitles: (
            shown: false,
            size: Large,
            background: Solid,
            speakers: false,
        ),
        auto_target: Strong,
        holds: (
            light: Hold,
            heavy: Toggle,
            dodge: Hold,
            skill: Hold,
            lock_on: Toggle,
        ),
    ),
)
"#;

#[test]
fn version_2_keeps_what_it_held_and_the_graphics_page_starts_as_before() {
    let read: PlayerSettings = from_ron(VERSION_2).expect("version 2 reads");
    assert_eq!((read.language.as_str(), read.size.as_str()), ("ja", "small"));
    assert_eq!(read.locale().voice, "ja", "its one language row set the voices too");
    assert_eq!((read.text_size, read.shake, read.camera_motion), (TextSize::Larger, 50, 25));
    assert_eq!((read.sound, read.music, read.effects, read.voice), (70, 60, 50, 40));
    assert!(read.mono && !read.rumble && !read.subtitles.shown && !read.subtitles.speakers);
    assert_eq!(read.subtitles.size, SubtitleSize::Large);
    assert_eq!(read.auto_target, AutoTarget::Strong);
    assert_eq!(read.holds.mode(HeldAction::Heavy), HoldMode::Toggle);
    assert_eq!(read.holds.mode(HeldAction::LockOn), HoldMode::Toggle);
    // What M4 T3 adds starts where the game ran before it: the device's tier, paced to the display, no
    // dynamic resolution, no battery saver.
    assert_eq!((read.quality, read.frame_cap), (QualityChoice::Automatic, FrameCap::Unlimited));
    assert!(!read.dynamic_resolution && !read.battery_saver);
}

/// M4 T3's `settings.ron`, as that build wrote the fields it holds: the graphics page off its defaults.
const VERSION_3: &str = r#"(
    type: "sapphire::game_ui::PlayerSettings",
    version: 3,
    data: (
        language: "ja",
        sound: 70,
        holds: (
            light: Hold,
            heavy: Hold,
            dodge: Toggle,
            skill: Hold,
            lock_on: Hold,
        ),
        quality: Low,
        frame_cap: Thirty,
        dynamic_resolution: true,
        battery_saver: true,
    ),
)
"#;

#[test]
fn version_3_keeps_its_graphics_and_remaps_nothing() {
    let read: PlayerSettings = from_ron(VERSION_3).expect("version 3 reads");
    let mut expected = PlayerSettings {
        language: "ja".to_owned(),
        sound: 70,
        quality: QualityChoice::Low,
        frame_cap: FrameCap::Thirty,
        dynamic_resolution: true,
        battery_saver: true,
        ..PlayerSettings::default()
    };
    *expected.holds.mode_mut(HeldAction::Dodge) = HoldMode::Toggle;
    assert_eq!(read, expected);
    assert_eq!(read.remapped, [], "every binding as the game's document has it");
}

#[test]
fn version_1_keeps_what_it_held_and_takes_the_defaults_for_the_rest() {
    let read: PlayerSettings = from_ron(VERSION_1).expect("version 1 reads");
    let expected = PlayerSettings {
        language: "ja".to_owned(),
        size: "large".to_owned(),
        sound: 40,
        rumble: false,
        ..PlayerSettings::default()
    };
    assert_eq!(read, expected);
    assert_eq!(read.auto_target, AutoTarget::Normal, "what every run before it played on");
    assert!(read.subtitles.shown, "subtitles start on");
    for action in HeldAction::ALL {
        assert_eq!(read.holds.mode(*action), HoldMode::Hold, "{} is held, as before", action.name());
    }
}

/// M6 U10's `settings.ron`, as that build wrote it, before the Voice language row, every field off its default:
/// Japanese, whose one Language row also chose Japanese voices.
const VERSION_4: &str = r#"(
    type: "sapphire::game_ui::PlayerSettings",
    version: 4,
    data: (
        language: "ja",
        size: "large",
        text_size: Large,
        shake: 75,
        flashes: Reduced,
        camera_motion: 50,
        sound: 80,
        music: 70,
        effects: 60,
        voice: 50,
        mono: true,
        dynamic_range: Night,
        rumble: false,
        subtitles: (
            shown: true,
            size: Large,
            background: Solid,
            speakers: false,
        ),
        auto_target: Weak,
        holds: (
            light: Hold,
            heavy: Hold,
            dodge: Toggle,
            skill: Hold,
            lock_on: Hold,
        ),
        quality: Low,
        frame_cap: Thirty,
        dynamic_resolution: true,
        battery_saver: true,
        remapped: [
            (
                binding: "light.gamepad",
                source: Pad(LeftShoulder),
            ),
        ],
        flash_notice_read: true,
        timing_assist: Wide,
    ),
)
"#;

/// M6 U13a: a version 4 file from before the Voice language row keeps everything it held, and its voices speak
/// its language, as they did: a field it lacks is "as the text".
#[test]
fn version_4_keeps_what_it_held_and_speaks_its_language() {
    let read: PlayerSettings = from_ron(VERSION_4).expect("version 4 reads");
    let mut expected = PlayerSettings {
        language: "ja".to_owned(),
        size: "large".to_owned(),
        text_size: TextSize::Large,
        shake: 75,
        flashes: game_ui::menus::Flashes::Reduced,
        camera_motion: 50,
        sound: 80,
        music: 70,
        effects: 60,
        voice: 50,
        mono: true,
        dynamic_range: DynamicRange::Night,
        rumble: false,
        auto_target: AutoTarget::Weak,
        quality: QualityChoice::Low,
        frame_cap: FrameCap::Thirty,
        dynamic_resolution: true,
        battery_saver: true,
        remapped: vec![Remapped {
            binding: "light.gamepad".to_owned(),
            source: Source::Pad(PadButton::LeftShoulder),
        }],
        flash_notice_read: true,
        timing_assist: TimingAssist::Wide,
        ..PlayerSettings::default()
    };
    expected.subtitles.size = SubtitleSize::Large;
    expected.subtitles.background = game_ui::menus::SubtitleBackground::Solid;
    expected.subtitles.speakers = false;
    *expected.holds.mode_mut(HeldAction::Dodge) = HoldMode::Toggle;
    assert_eq!(read, expected);
    let locale = read.locale();
    assert_eq!((locale.text.as_str(), locale.voice.as_str()), ("ja", "ja"));
}

/// M6 U13a: the Voice language row steps the voices alone, and what it chose is saved and read back apart from
/// the text's language.
#[test]
fn the_voice_language_steps_and_saves_apart_from_the_text() {
    let mut settings = PlayerSettings::default();
    settings.step(Setting::VoiceLanguage);
    assert_eq!((settings.language.as_str(), settings.voice_language.as_str()), ("en", "ja"));
    let text = to_ron(&settings).expect("it writes");
    assert!(text.contains("voice_language: \"ja\""), "{text}");
    let read: PlayerSettings = from_ron(&text).expect("it reads");
    let locale = read.locale();
    assert_eq!((locale.text.as_str(), locale.voice.as_str()), ("en", "ja"), "English menus, Japanese voices");
    settings.step(Setting::VoiceLanguage);
    assert_eq!(settings.voice_language, "en", "round past the last");
    // A voice language this install doesn't hold speaks the base install's.
    let unheld = PlayerSettings { voice_language: "fr".to_owned(), ..PlayerSettings::default() };
    assert_eq!(unheld.locale().voice, "en");
}

#[test]
fn this_build_writes_version_4_and_reads_it_back() {
    assert_eq!(PlayerSettings::VERSION, 4);
    let mut settings = PlayerSettings { language: "ja".to_owned(), ..PlayerSettings::default() };
    settings.mono = true;
    settings.auto_target = AutoTarget::Strong;
    settings.timing_assist = TimingAssist::Wider;
    *settings.holds.mode_mut(HeldAction::Dodge) = HoldMode::Toggle;
    settings.quality = QualityChoice::Low;
    settings.frame_cap = FrameCap::Thirty;
    settings.battery_saver = true;
    settings.remapped = vec![
        Remapped { binding: "light.gamepad".to_owned(), source: Source::Pad(PadButton::LeftShoulder) },
        Remapped { binding: "dodge.keyboard".to_owned(), source: Source::Key(Key::Space) },
    ];
    settings.voice_language = "en".to_owned();
    let text = to_ron(&settings).expect("it writes");
    assert!(text.contains("version: 4,"), "{text}");
    assert_eq!(from_ron::<PlayerSettings>(&text).expect("it reads"), settings);
    for old in [VERSION_1, VERSION_2, VERSION_3, VERSION_4] {
        let upgraded: PlayerSettings = from_ron(old).expect("an older version reads");
        let rewritten = to_ron(&upgraded).expect("it writes");
        assert_eq!(from_ron::<PlayerSettings>(&rewritten).expect("it reads"), upgraded, "an upgrade sticks");
    }
}

/// The timing assist (M4 A6) and the dynamic range (M6 U10) joined version 4 with no bump, since a field a file
/// lacks reads at its default: a version 4 file from before them, as A4's build wrote one, reads with the assist
/// off and the range standard, which is what every run before them played on.
#[test]
fn a_version_4_file_from_before_the_timing_assist_reads_it_as_off() {
    let before = VERSION_4
        .replace("        timing_assist: Wide,\n", "")
        .replace("        dynamic_range: Night,\n", "");
    assert!(!before.contains("timing_assist") && !before.contains("dynamic_range"), "{before}");
    let read: PlayerSettings = from_ron(&before).expect("it reads");
    assert_eq!((read.timing_assist, read.dynamic_range), (TimingAssist::Off, DynamicRange::Standard));
    assert_eq!(
        (read.auto_target, read.locale().voice.as_str()),
        (AutoTarget::Weak, "ja"),
        "and keeps the rest"
    );
}
