//! The settings file's versions (M4 A1): what an older build wrote reads into this one, and what this one
//! writes reads back. Version 1's bytes are frozen here as M3's build wrote them, so a change to how they
//! upgrade shows as a failure rather than as a player's lost settings.

use game_ui::menus::{HeldAction, HoldMode, PlayerSettings};
use sapphire_gameplay::targeting::AutoTarget;
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

#[test]
fn this_build_writes_version_2_and_reads_it_back() {
    assert_eq!(PlayerSettings::VERSION, 2);
    let mut settings = PlayerSettings { language: "ja".to_owned(), ..PlayerSettings::default() };
    settings.mono = true;
    settings.auto_target = AutoTarget::Strong;
    *settings.holds.mode_mut(HeldAction::Dodge) = HoldMode::Toggle;
    let text = to_ron(&settings).expect("it writes");
    assert!(text.contains("version: 2,"), "{text}");
    assert_eq!(from_ron::<PlayerSettings>(&text).expect("it reads"), settings);
    let upgraded: PlayerSettings = from_ron(VERSION_1).expect("version 1 reads");
    let rewritten = to_ron(&upgraded).expect("it writes");
    assert_eq!(from_ron::<PlayerSettings>(&rewritten).expect("it reads"), upgraded, "an upgrade sticks");
}
