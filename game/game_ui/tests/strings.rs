//! Every string the game draws (M3 C3): the game's documents, its cutscenes' subtitles, its dialogues'
//! lines (M3 Q2), its bosses' names and its window's title (M3 Z5b) name keys, and only the string tables say what they show, in English
//! and Japanese. Nothing a document, a cutscene or a boss names is missing from either language, the two
//! have the same keys taking the same variables, no key is one nothing can show, the Japanese says it's
//! machine-quality, and the fonts' character set holds every character the tables use. How a string is
//! added is in `content/loc/en/game_ui.ftl`'s header. `menus.rs` walks every screen in both languages and
//! under pseudo-localization.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use game_ui::menus::{MenuDocument, WINDOW_TITLE};
use sapphire_assets::ui::UiDocument;
use sapphire_dialogue::DialogueDocument;
use sapphire_pipeline::font::Characters;
use sapphire_strings::{Expression, Part, Pattern, StringTable};
use sapphire_timeline::TimelineDocument;
use support::{boss_names, content, document_in, menus, table, testdata, with_help};

const LOCALES: [&str; 2] = ["en", "ja"];

/// The game's documents beside its menus, which show no text: the zone trip's wipe (M3 W4).
const TEXTLESS: [&str; 1] = ["zone_wipe"];

fn keys(table: &StringTable) -> BTreeSet<&str> {
    table.entries.iter().map(|entry| table.str(entry.key)).collect()
}

/// What every one of the game's screens names: its keys, and the prefixes its `key_from`s choose whole keys
/// under. The window's title is the one key the app names rather than a screen (M3 Z5b).
fn named() -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut keys, prefixes) = named_in(&menus(), &[content().join("ui")]);
    keys.insert(WINDOW_TITLE.to_owned());
    (keys, prefixes)
}

/// What every screen of `menus` names, each document from the first of `dirs` that has it: which screens
/// there are is the menus document's, so a screen added there is held here with no change to this file.
fn named_in(menus: &MenuDocument, dirs: &[PathBuf]) -> (BTreeSet<String>, BTreeSet<String>) {
    let documents = menus.screens.iter().map(|screen| (screen.name.clone(), document_in(&screen.name, dirs)));
    named_by(documents)
}

fn named_by(documents: impl Iterator<Item = (String, UiDocument)>) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut keys, mut prefixes) = (BTreeSet::new(), BTreeSet::new());
    for (screen, document) in documents {
        for (node, text, _) in document.texts() {
            assert!(
                !text.key.is_empty() || text.key_from.is_some(),
                "{screen}: `{}` shows text with no key",
                node.name
            );
            if !text.key.is_empty() {
                keys.insert(text.key.clone());
            }
            if let Some(from) = &text.key_from {
                prefixes.insert(from.prefix.clone());
            }
        }
    }
    (keys, prefixes)
}

fn add_spoken(dir: &Path, into: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            add_spoken(&path, into);
        } else if path.to_str().is_some_and(|name| name.ends_with(".timeline.ron")) {
            let timeline = TimelineDocument::from_ron(&fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            for line in timeline.subtitles {
                into.extend(line.speaker);
                into.insert(line.key);
            }
        }
    }
}

/// What every timeline's subtitles say, and who says it: keys a cutscene shows. Then what every dialogue's
/// lines and choices say, and who says them: keys a dialogue's panel shows (M3 Q2). Then every boss's name:
/// keys its boss bar shows.
fn spoken() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    add_spoken(&content(), &mut keys);
    add_said(&content(), &mut keys);
    keys.extend(boss_names());
    keys
}

fn add_said(dir: &Path, into: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            add_said(&path, into);
        } else if path.to_str().is_some_and(|name| name.ends_with(".dialogue.ron")) {
            let dialogue = DialogueDocument::from_ron(&fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            for node in dialogue.nodes {
                if let Some(say) = node.say {
                    into.insert(say.speaker);
                    into.insert(say.line);
                }
                into.extend(node.choices.into_iter().map(|choice| choice.line));
            }
        }
    }
}

fn add_variables(table: &StringTable, expression: Expression, into: &mut BTreeSet<String>) {
    if let Expression::Variable(name) | Expression::NumberVariable { name, .. } = expression {
        into.insert(table.str(name).to_owned());
    }
}

fn variables_of(table: &StringTable, pattern: Pattern, into: &mut BTreeSet<String>) {
    let Pattern::Parts(run) = pattern else { return };
    for &part in table.parts_of(run) {
        match part {
            Part::Text(_) => {}
            Part::Place(expression) => add_variables(table, expression, into),
            Part::Select(index) => {
                let select = table.selects[index as usize];
                add_variables(table, select.selector, into);
                for variant in table.variants_of(select.variants) {
                    variables_of(table, variant.pattern, into);
                }
            }
        }
    }
}

fn variables(table: &StringTable, key: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    variables_of(table, table.entry(key).expect("the key").pattern, &mut found);
    found
}

/// The game's documents are the screens its menus document lists, and those that show no text: a document
/// beside them would show strings no test reads.
#[test]
fn the_game_s_documents_are_its_screens() {
    let mut found: Vec<String> = fs::read_dir(content().join("ui"))
        .unwrap()
        .filter_map(|entry| entry.unwrap().file_name().to_str()?.strip_suffix(".ui.ron").map(str::to_owned))
        .collect();
    found.sort();
    let mut known: Vec<String> = menus().screens.into_iter().map(|screen| screen.name).collect();
    known.extend(TEXTLESS.map(str::to_owned));
    known.sort();
    assert_eq!(found, known);
    for stem in TEXTLESS {
        let source = fs::read_to_string(content().join(format!("ui/{stem}.ui.ron"))).unwrap();
        let document = UiDocument::from_document(&source).unwrap_or_else(|error| panic!("{stem}: {error}"));
        assert_eq!(document.texts().count(), 0, "{stem} shows text, so it's a screen to test as one");
    }
}

#[test]
fn every_key_a_screen_or_a_cutscene_names_is_in_both_languages() {
    let (mut named, prefixes) = named();
    let spoken = spoken();
    assert!(!spoken.is_empty(), "no cutscene says anything");
    named.extend(spoken);
    for locale in LOCALES {
        let table = table(locale);
        let missing: Vec<_> = named.iter().filter(|key| table.entry(key).is_none()).collect();
        assert!(missing.is_empty(), "{locale} lacks {missing:?}");
        for prefix in &prefixes {
            assert!(
                keys(&table).iter().any(|key| key.starts_with(prefix.as_str())),
                "{locale}: nothing under {prefix}"
            );
        }
    }
}

/// A screen added by documents alone (M3 U9, `menus.rs`'s `a_new_screen_is_documents`) has its keys held as
/// the game's own are, with nothing here naming it: the fixture's are all in both languages, and the same
/// fixture with a key no table has is caught by name.
#[test]
fn a_new_screens_keys_are_held_with_no_change_here() {
    let dirs = [testdata(), content().join("ui")];
    let (keys, _) = named_in(&with_help(), &dirs);
    let (game, _) = named();
    let help = document_in("help", &dirs);
    assert!(
        help.texts().all(|(_, text, _)| text.key.is_empty() || keys.contains(&text.key)),
        "its keys are read"
    );
    for locale in LOCALES {
        let table = table(locale);
        let missing: Vec<_> = keys.iter().filter(|key| table.entry(key).is_none()).collect();
        assert!(missing.is_empty(), "{locale} lacks {missing:?}");
    }

    let source = fs::read_to_string(testdata().join("help.ui.ron")).unwrap();
    let renamed = source.replacen(
        "key: \"settings-language\", style: \"title\"",
        "key: \"help-title\", style: \"title\"",
        1,
    );
    assert_ne!(renamed, source, "the fixture's title as the test knows it");
    let new = UiDocument::from_document(&renamed).unwrap();
    let (keys, _) = named_by(std::iter::once(("help".to_owned(), new)));
    let unknown: Vec<_> =
        keys.iter().filter(|key| !game.contains(*key) && table("en").entry(key).is_none()).collect();
    assert_eq!(unknown, ["help-title"]);
}

/// A boss's name is in every language, so no boss bar names a key (M3 Z3).
#[test]
fn every_boss_s_name_is_in_every_language() {
    let bosses = boss_names();
    // No boss by name: they're content, which `sapphire_gameplay`'s `bosses_are_content` holds code to.
    assert!(!bosses.is_empty(), "some kind of enemy is a boss");
    for locale in LOCALES {
        let table = table(locale);
        let missing: Vec<_> = bosses.iter().filter(|key| table.entry(key).is_none()).collect();
        assert!(missing.is_empty(), "{locale} lacks the bosses' names {missing:?}");
    }
}

#[test]
fn both_languages_have_the_same_keys_taking_the_same_variables() {
    let (en, ja) = (table("en"), table("ja"));
    let only_en: Vec<_> = keys(&en).difference(&keys(&ja)).copied().collect();
    let only_ja: Vec<_> = keys(&ja).difference(&keys(&en)).copied().collect();
    assert!(only_en.is_empty() && only_ja.is_empty(), "only in en: {only_en:?}; only in ja: {only_ja:?}");
    for key in keys(&en) {
        assert_eq!(variables(&en, key), variables(&ja, key), "`{key}` takes different variables");
    }
}

#[test]
fn every_key_is_one_a_screen_or_a_cutscene_can_show() {
    let (mut named, prefixes) = named();
    named.extend(spoken());
    for locale in LOCALES {
        let table = table(locale);
        let dead: Vec<_> = keys(&table)
            .into_iter()
            .filter(|key| {
                !named.contains(*key) && !prefixes.iter().any(|prefix| key.starts_with(prefix.as_str()))
            })
            .collect();
        assert!(dead.is_empty(), "{locale}: no screen or cutscene shows {dead:?}");
    }
}

/// Owed a Japanese reader's pass, and saying so where the next writer sees it.
#[test]
fn the_japanese_says_it_is_machine_quality() {
    for entry in fs::read_dir(content().join("loc/ja")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "ftl") {
            let text = fs::read_to_string(&path).unwrap();
            let marked = text
                .lines()
                .take_while(|line| line.starts_with('#'))
                .any(|line| line.starts_with("###") && line.contains("Machine-quality Japanese"));
            assert!(marked, "{} doesn't say it's machine-quality in its resource comment", path.display());
        }
    }
}

#[test]
fn the_character_set_holds_every_character_the_tables_use() {
    let set = Characters::from_document(&fs::read_to_string(content().join("fonts/ui.charset.ron")).unwrap())
        .unwrap();
    for locale in LOCALES {
        let missing: String = table(locale)
            .characters()
            .into_iter()
            .filter(|&c| !c.is_whitespace() && !set.contains(c))
            .collect();
        assert!(missing.is_empty(), "content/fonts/ui.charset.ron lacks {locale}'s {missing}");
    }
}
