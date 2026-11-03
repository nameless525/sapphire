use bevy_reflect::TypePath;
use sapphire_platform::durable_replace;
use sapphire_reflect::{Decoder, VersionError, decode_versioned};
use serde::Deserialize;

use super::*;
use crate::test_support::{Fault, Step, TestDir, died, faulty_replace};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
enum Window {
    #[default]
    Fullscreen,
    Sized(u32, u32),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Keys {
    jump: String,
    dodge: String,
}

#[derive(Debug, Clone, PartialEq, TypePath, Serialize, Deserialize)]
#[type_path = "sapphire::save_test"]
#[serde(default)]
struct Options {
    volume: f32,
    language: String,
    window: Window,
    keys: Keys,
    subtitles: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            volume: 1.0,
            language: "en".to_owned(),
            window: Window::Fullscreen,
            keys: Keys { jump: "Space".to_owned(), dodge: "Shift".to_owned() },
            subtitles: None,
        }
    }
}

impl Versioned for Options {
    const VERSION: u32 = 2;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        decode_versioned::<OptionsV1, D>(version, data)
            .map(|old| Options { volume: f32::from(old.loudness) / 100.0, ..Options::default() })
    }
}

#[derive(Deserialize)]
struct OptionsV1 {
    loudness: u8,
}

impl Versioned for OptionsV1 {
    const VERSION: u32 = 1;
}

/// A type that names every field it needs and refuses what it doesn't know.
#[derive(Debug, PartialEq, TypePath, Serialize, Deserialize)]
#[type_path = "sapphire::save_test"]
#[serde(deny_unknown_fields)]
struct Strict {
    a: u32,
    b: u32,
}

impl Default for Strict {
    fn default() -> Self {
        Self { a: 1, b: 2 }
    }
}

impl Versioned for Strict {
    const VERSION: u32 = 1;
}

fn played() -> Options {
    Options {
        volume: 0.25,
        language: "ja".to_owned(),
        window: Window::Sized(1280, 720),
        keys: Keys { jump: "J".to_owned(), dodge: "K".to_owned() },
        subtitles: Some("large".to_owned()),
    }
}

fn file(dir: &TestDir) -> SettingsFile<Options> {
    SettingsFile::new(dir.path(), "settings", durable_replace)
}

/// The player's file on disk, with `edit` made to its text.
fn edited(file: &SettingsFile<Options>, edit: impl FnOnce(&str) -> String) -> String {
    file.write(&file.snapshot(&played()).expect("snapshot")).expect("write");
    let text = edit(&fs::read_to_string(file.path()).expect("read"));
    fs::write(file.path(), &text).expect("edit");
    text
}

/// Loads a file that needs repair: the original must be kept aside, and the file left clean.
fn repaired(dir: &TestDir, file: &SettingsFile<Options>, original: &[u8]) -> (Options, Vec<String>) {
    let loaded = file.load();
    let SettingsOutcome::Repaired { kept_aside, dropped, error } = loaded.outcome else {
        panic!("{:?}", loaded.outcome)
    };
    assert!(!error.is_empty());
    let aside = kept_aside.expect("kept aside");
    assert!(aside.starts_with(dir.path()));
    assert_eq!(fs::read(&aside).expect("read the copy"), original);
    // Never stuck on it: the next launch reads the repaired file, with no second copy.
    let again = file.load();
    assert!(matches!(again.outcome, SettingsOutcome::Read), "{:?}", again.outcome);
    assert_eq!(again.value, loaded.value);
    (loaded.value, dropped)
}

#[test]
fn no_file_gives_the_defaults_and_writes_nothing() {
    let dir = TestDir::new("settings_missing");
    let loaded = file(&dir).load();
    assert!(matches!(loaded.outcome, SettingsOutcome::Missing));
    assert_eq!(loaded.value, Options::default());
    assert!(dir.listing().is_empty());
}

#[test]
fn settings_round_trip_as_readable_ron() {
    let dir = TestDir::new("settings_round");
    let file = file(&dir);
    file.write(&file.snapshot(&played()).expect("snapshot")).expect("write");
    let text = fs::read_to_string(file.path()).expect("read");
    assert!(text.contains("volume: 0.25"), "{text}");
    assert!(text.contains("type: \"sapphire::save_test::Options\""), "{text}");
    let loaded = file.load();
    assert!(matches!(loaded.outcome, SettingsOutcome::Read));
    assert_eq!(loaded.value, played());
    assert_eq!(dir.listing(), ["settings.lock", "settings.ron"]);
}

#[test]
fn a_field_this_version_doesnt_know_is_ignored_and_gone_at_the_next_write() {
    let dir = TestDir::new("settings_unknown");
    let file = file(&dir);
    edited(&file, |text| text.replace("volume:", "from_a_later_build: (1, 2),\n        volume:"));
    let loaded = file.load();
    assert!(matches!(loaded.outcome, SettingsOutcome::Read), "{:?}", loaded.outcome);
    assert_eq!(loaded.value, played());
    file.write(&file.snapshot(&loaded.value).expect("snapshot")).expect("write");
    assert!(!fs::read_to_string(file.path()).expect("read").contains("from_a_later_build"));
}

#[test]
fn a_field_this_file_lacks_takes_its_default() {
    let dir = TestDir::new("settings_lacks");
    let file = file(&dir);
    edited(&file, |text| text.replace("        volume: 0.25,\n", ""));
    let loaded = file.load();
    assert!(matches!(loaded.outcome, SettingsOutcome::Read), "{:?}", loaded.outcome);
    assert_eq!(loaded.value, Options { volume: 1.0, ..played() });
}

#[test]
fn an_older_version_upgrades_with_no_repair() {
    let dir = TestDir::new("settings_old");
    let file = file(&dir);
    let text = "(type: \"sapphire::save_test::Options\", version: 1, data: (loudness: 50))";
    fs::write(file.path(), text).expect("write");
    let loaded = file.load();
    assert!(matches!(loaded.outcome, SettingsOutcome::Read), "{:?}", loaded.outcome);
    assert_eq!(loaded.value.volume, 0.5);
}

#[test]
fn one_wrong_value_costs_only_that_field() {
    let dir = TestDir::new("settings_value");
    let file = file(&dir);
    let text = edited(&file, |text| text.replace("volume: 0.25", "volume: \"loud\""));
    let (value, dropped) = repaired(&dir, &file, text.as_bytes());
    assert_eq!(value, Options { volume: 1.0, ..played() });
    assert_eq!(dropped, ["volume"]);
}

#[test]
fn one_line_of_junk_costs_nothing() {
    let dir = TestDir::new("settings_junk");
    let file = file(&dir);
    let text = edited(&file, |text| text.replace("volume:", "%% oops\n        volume:"));
    let (value, dropped) = repaired(&dir, &file, text.as_bytes());
    assert_eq!(value, played());
    assert!(dropped.is_empty(), "{dropped:?}");
}

#[test]
fn a_missing_comma_costs_nothing() {
    let dir = TestDir::new("settings_comma");
    let file = file(&dir);
    let text = edited(&file, |text| text.replace("volume: 0.25,", "volume: 0.25"));
    let (value, dropped) = repaired(&dir, &file, text.as_bytes());
    assert_eq!(value, played());
    assert!(dropped.is_empty(), "{dropped:?}");
}

#[test]
fn a_bracket_left_open_costs_only_its_field() {
    let dir = TestDir::new("settings_open");
    let file = file(&dir);
    let text = edited(&file, |text| text.replace("Sized(1280, 720)", "Sized(1280, 720"));
    let (value, dropped) = repaired(&dir, &file, text.as_bytes());
    assert_eq!(value, Options { window: Window::Fullscreen, ..played() });
    assert_eq!(dropped, ["window"]);
}

#[test]
fn a_damaged_nested_struct_costs_only_its_own_field() {
    let dir = TestDir::new("settings_nested");
    let file = file(&dir);
    let text = edited(&file, |text| text.replace("jump: \"J\"", "jump: 12"));
    let (value, dropped) = repaired(&dir, &file, text.as_bytes());
    assert_eq!(value, Options { keys: Options::default().keys, ..played() });
    assert_eq!(dropped, ["keys"]);
}

#[test]
fn a_file_cut_short_keeps_what_came_before_the_cut() {
    let dir = TestDir::new("settings_cut");
    let file = file(&dir);
    let text = edited(&file, |text| {
        let cut = text.find("window").expect("the field");
        text[..cut].to_owned()
    });
    let (value, _) = repaired(&dir, &file, text.as_bytes());
    assert_eq!(value, Options { volume: 0.25, language: "ja".to_owned(), ..Options::default() });
}

#[test]
fn garbage_and_an_empty_file_give_the_defaults_and_arent_stuck() {
    for (name, bytes) in [("empty", &b""[..]), ("garbage", &[0xff, 0x00, 0xfe, 0x28, 0x0a, 0x29][..])] {
        let dir = TestDir::new(name);
        let file = file(&dir);
        fs::write(file.path(), bytes).expect("write");
        let (value, dropped) = repaired(&dir, &file, bytes);
        assert_eq!(value, Options::default());
        assert!(dropped.is_empty(), "{dropped:?}");
    }
}

#[test]
fn a_newer_version_keeps_what_it_can_and_is_kept_aside() {
    let dir = TestDir::new("settings_newer");
    let file = file(&dir);
    let text = edited(&file, |text| text.replace("version: 2", "version: 9"));
    let (value, _) = repaired(&dir, &file, text.as_bytes());
    assert_eq!(value, played());
}

#[test]
fn a_strict_type_loses_only_the_field_it_refuses_and_gets_what_it_lacks() {
    let dir = TestDir::new("settings_strict");
    let file = SettingsFile::<Strict>::new(dir.path(), "strict", durable_replace);
    let text = "(type: \"sapphire::save_test::Strict\", version: 1, data: (a: 5, extra: 1))";
    fs::write(file.path(), text).expect("write");
    let loaded = file.load();
    assert_eq!(loaded.value, Strict { a: 5, b: 2 });
    let SettingsOutcome::Repaired { dropped, .. } = loaded.outcome else { panic!("{:?}", loaded.outcome) };
    assert_eq!(dropped, ["extra"]);
}

#[test]
fn repairs_stack_up_only_as_far_as_the_limit() {
    let dir = TestDir::new("settings_limit");
    let file = file(&dir);
    for round in 0..crate::KEPT_ASIDE + 2 {
        fs::write(file.path(), format!("broken {round}")).expect("write");
        let _ = file.load();
    }
    let copies = dir.listing().iter().filter(|name| name.contains(".bad.")).count();
    assert_eq!(copies, crate::KEPT_ASIDE);
}

#[test]
fn a_repair_that_cant_write_still_gives_a_value_and_leaves_the_file() {
    let dir = TestDir::new("settings_nowrite");
    let file = SettingsFile::<Options>::new(dir.path(), "settings", |_: &Path, _: &[u8]| {
        Err(io::Error::other("the disk is full"))
    });
    fs::write(file.path(), "(data: (volume: 0.5, %%").expect("write");
    for _ in 0..2 {
        let loaded = file.load();
        assert_eq!(loaded.value.volume, 0.5);
        assert!(matches!(loaded.outcome, SettingsOutcome::Repaired { .. }));
        assert_eq!(fs::read_to_string(file.path()).expect("read"), "(data: (volume: 0.5, %%");
    }
}

#[test]
fn a_directory_in_the_files_place_is_unreadable_not_a_panic() {
    let dir = TestDir::new("settings_unreadable");
    let file = file(&dir);
    fs::create_dir(file.path()).expect("a directory");
    let loaded = file.load();
    assert!(matches!(loaded.outcome, SettingsOutcome::Unreadable(_)), "{:?}", loaded.outcome);
    assert_eq!(loaded.value, Options::default());
}

#[test]
fn a_write_stopped_at_any_step_leaves_a_whole_file() {
    for step in Step::ALL {
        for fault in [Fault::Fail, Fault::Die] {
            let context = format!("{step:?}, {fault:?}");
            let dir = TestDir::new("settings_steps");
            let file = file(&dir);
            file.write(&file.snapshot(&Options::default()).expect("snapshot")).expect("write");

            let faulty = SettingsFile::<Options>::new(dir.path(), "settings", faulty_replace(step, fault));
            let snapshot = faulty.snapshot(&played()).expect("snapshot");
            let mut result = None;
            assert_eq!(died(|| result = Some(faulty.write(&snapshot))), fault == Fault::Die, "{context}");
            assert!(result.is_none_or(|result| result.is_err()), "{context}");

            let loaded = file.load();
            assert!(matches!(loaded.outcome, SettingsOutcome::Read), "{context}: {:?}", loaded.outcome);
            let expected = if step.after_rename() { played() } else { Options::default() };
            assert_eq!(loaded.value, expected, "{context}");

            file.write(&file.snapshot(&played()).expect("snapshot")).expect(&context);
            assert_eq!(file.load().value, played(), "{context}");
        }
    }
}
