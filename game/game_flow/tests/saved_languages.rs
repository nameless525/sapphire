//! The player's languages on disk (M6 U13a): a settings file from before the Voice language row, whole or
//! damaged, keeps the voices in the text's language, as its one Language row had them.

use std::path::PathBuf;

use game_ui::menus::PlayerSettings;
use sapphire_platform::durable_replace;
use sapphire_save::{SettingsFile, SettingsOutcome};

/// A directory of its own, gone with the test.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("game-flow-languages-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        Self(dir)
    }

    fn file(&self) -> SettingsFile<PlayerSettings> {
        SettingsFile::new(&self.0, "settings", durable_replace)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A Japanese player's file as a build before the row wrote it: no `voice_language`, `sound` as `sound`.
fn before_the_row(sound: &str) -> String {
    format!(
        r#"(
    type: "sapphire::game_ui::PlayerSettings",
    version: 4,
    data: (
        language: "ja",
        sound: {sound},
        mono: true,
    ),
)
"#
    )
}

#[test]
fn a_file_from_before_the_row_speaks_its_language() {
    let dir = Dir::new("whole");
    let file = dir.file();
    std::fs::write(file.path(), before_the_row("70")).expect("written");
    let loaded = file.load();
    assert!(matches!(loaded.outcome, SettingsOutcome::Read), "{:?}", loaded.outcome);
    let locale = loaded.value.locale();
    assert_eq!((locale.text.as_str(), locale.voice.as_str()), ("ja", "ja"));
    assert_eq!(loaded.value.sound, 70);
}

/// The repair lays what it can read over the defaults, at this build's version, with no upgrade: a damaged file
/// from before the row keeps its voices as its text all the same.
#[test]
fn a_damaged_file_from_before_the_row_speaks_its_language() {
    let dir = Dir::new("damaged");
    let file = dir.file();
    std::fs::write(file.path(), before_the_row("loud")).expect("written");
    let loaded = file.load();
    let SettingsOutcome::Repaired { dropped, .. } = &loaded.outcome else { panic!("{:?}", loaded.outcome) };
    assert_eq!(dropped, &["sound".to_owned()]);
    let locale = loaded.value.locale();
    assert_eq!((locale.text.as_str(), locale.voice.as_str()), ("ja", "ja"), "the voices as the text");
    assert!(loaded.value.mono, "and the rest it held");
}
