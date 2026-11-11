//! The player's remapped bindings on disk (M4 A4): they survive a restart, laid over the game's document, and
//! a broken one costs the player that binding, with a log line, never the game or the rest of their settings.

use std::path::PathBuf;

use game_ui::menus::PlayerSettings;
use sapphire_controls::{Remapped, default_bindings};
use sapphire_input::{InputBindings, InputMap, Key, PadButton, Source};
use sapphire_platform::durable_replace;
use sapphire_save::{SettingsFile, SettingsOutcome};

/// A directory of its own, gone with the test.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("game-flow-bindings-{name}-{}", std::process::id()));
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

/// What `binding` reads in `bindings`, which must compile.
fn source_of(bindings: &InputBindings, binding: &str) -> Source {
    InputMap::new(bindings.clone()).expect("they compile");
    sapphire_controls::source_of(bindings, binding).expect(binding).clone()
}

#[test]
fn remapped_bindings_survive_a_restart_over_the_games_document() {
    let dir = Dir::new("restart");
    let file = dir.file();
    let remapped = vec![Remapped { binding: "dodge.keyboard".to_owned(), source: Source::Key(Key::Space) }];
    let settings = PlayerSettings { remapped, ..PlayerSettings::default() };
    file.write(&file.snapshot(&settings).expect("a snapshot")).expect("written");

    let loaded = file.load();
    assert!(matches!(loaded.outcome, SettingsOutcome::Read), "{:?}", loaded.outcome);
    assert_eq!(loaded.value, settings);
    let map = loaded.value.bindings_over(&default_bindings().unwrap());
    assert_eq!(source_of(&map, "dodge.keyboard"), Source::Key(Key::Space));
    assert_eq!(
        source_of(&map, "dodge.gamepad"),
        Source::Pad(PadButton::South),
        "the rest as the game has it"
    );
}

#[test]
fn a_broken_saved_binding_is_dropped_and_the_rest_stay() {
    // One the build can't read at all: the bindings go back to the game's, and the rest of the file stays.
    let dir = Dir::new("broken");
    let file = dir.file();
    let remapped =
        vec![Remapped { binding: "light.gamepad".to_owned(), source: Source::Pad(PadButton::North) }];
    let settings = PlayerSettings { language: "ja".to_owned(), remapped, ..PlayerSettings::default() };
    file.write(&file.snapshot(&settings).expect("a snapshot")).expect("written");
    let text = std::fs::read_to_string(file.path()).expect("read");
    assert!(text.contains("Pad(North)"), "{text}");
    std::fs::write(file.path(), text.replace("Pad(North)", "Pad(Thirteenth)")).expect("edited");
    let loaded = file.load();
    let SettingsOutcome::Repaired { dropped, .. } = &loaded.outcome else { panic!("{:?}", loaded.outcome) };
    assert_eq!(dropped, &["remapped".to_owned()]);
    assert_eq!(loaded.value.language, "ja", "the rest of the settings stay");
    assert_eq!(loaded.value.remapped, []);

    // One that reads but names what the game's document hasn't, or a press its device can't make: that
    // binding alone is dropped, and the rest still apply.
    let base = default_bindings().unwrap();
    let saved = PlayerSettings {
        remapped: vec![
            Remapped { binding: "fly.gamepad".to_owned(), source: Source::Pad(PadButton::North) },
            Remapped { binding: "light.keyboard".to_owned(), source: Source::Pad(PadButton::North) },
            Remapped { binding: "jump.keyboard".to_owned(), source: Source::Key(Key::F) },
        ],
        ..PlayerSettings::default()
    };
    let map = saved.bindings_over(&base);
    assert_eq!(source_of(&map, "light.keyboard"), Source::Key(Key::J));
    assert_eq!(source_of(&map, "jump.keyboard"), Source::Key(Key::F));
}
