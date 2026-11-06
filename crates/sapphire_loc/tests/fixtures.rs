//! The evaluator over the fixture's cooked tables, in English and Japanese: everything it runs, through
//! the bytes a pack would hold. `sapphire_pipeline` holds those bytes to their `.ftl` sources, and they're
//! compiled in, so this binary also runs as it is on an iOS Simulator and an Android emulator.

use sapphire_loc::{Arg, LocaleSettings, Localizer, StringTable, Value, is_framed};
use sapphire_strings::{
    Entry, Expression, NUMBER_CHARACTERS, NumberFormat, Part, Pattern, Run, Span, TermArg, pseudo_characters,
};

const EN: &[u8] = include_bytes!("../testdata/cooked/en.sloc");
const JA: &[u8] = include_bytes!("../testdata/cooked/ja.sloc");

fn localizer(text: &str) -> Localizer {
    let mut loc = Localizer::new(LocaleSettings { text: text.to_owned(), voice: "en".to_owned() });
    loc.install(StringTable::read(EN).unwrap()).unwrap();
    loc.install(StringTable::read(JA).unwrap()).unwrap();
    loc
}

fn format(loc: &Localizer, key: &str, args: &[Arg<'_>]) -> String {
    let mut out = String::new();
    loc.format(key, args, &mut out);
    out
}

#[test]
fn plain_messages_and_attributes() {
    let en = localizer("en");
    assert_eq!(en.text("title"), "Main Menu");
    assert_eq!(en.text("save.tooltip"), "Save your progress");
    assert_eq!(en.text("only-attributes.label"), "Only an attribute");
    let ja = localizer("ja");
    assert_eq!(ja.text("title"), "メインメニュー");
    assert_eq!(ja.text("save.tooltip"), "進行状況をセーブします");
    assert_eq!(ja.stats().unknown_keys, 0);
}

#[test]
fn variables_and_references() {
    let name = [("name", Value::Text("Aoi"))];
    let en = localizer("en");
    assert_eq!(format(&en, "welcome", &name), "Welcome to Sapphire, Aoi!");
    assert_eq!(format(&en, "welcome-back", &name), "Welcome to Sapphire, Aoi! Good luck.");
    assert_eq!(format(&en, "tooltip-of-save", &[]), "Press Ctrl+S to save.");
    let ja = localizer("ja");
    assert_eq!(format(&ja, "welcome", &name), "サファイアへようこそ、Aoiさん！");
    assert_eq!(format(&ja, "welcome-back", &name), "サファイアへようこそ、Aoiさん！ 幸運を。");
}

#[test]
fn a_term_reads_only_what_its_given() {
    let en = localizer("en");
    assert_eq!(format(&en, "inside", &[]), "You are in the inner realm.");
    // No arguments: its default, whatever the caller passed.
    assert_eq!(format(&en, "realm-name", &[("case", Value::Text("inner"))]), "Realm");
    assert_eq!(en.stats().missing_args, 1, "the select's `$case`, unpassed");
    assert_eq!(format(&en, "sword-taken", &[]), "It is yours");
    assert_eq!(format(&localizer("ja"), "inside", &[]), "内なる領域にいます。");
}

/// English has two plural categories and Japanese one, and a [one] variant in Japanese is never chosen
/// by a count of 1.
#[test]
fn plurals_by_locale() {
    let (en, ja) = (localizer("en"), localizer("ja"));
    let count = |n: i64| [("count", Value::Integer(n))];
    assert_eq!(format(&en, "enemies-left", &count(1)), "One enemy left");
    assert_eq!(format(&en, "enemies-left", &count(2)), "2 enemies left");
    assert_eq!(format(&en, "enemies-left", &count(0)), "0 enemies left");
    assert_eq!(format(&en, "enemies-left", &count(1234)), "1,234 enemies left");
    assert_eq!(format(&ja, "enemies-left", &count(1)), "残りの敵：1体");
    assert_eq!(format(&ja, "enemies-left", &count(2)), "残りの敵：2体");
    assert_eq!(format(&ja, "enemies-left", &count(12345)), "残りの敵：12,345体");
    let float = [("count", Value::Float(1.5))];
    assert_eq!(format(&en, "enemies-left", &float), "1.5 enemies left");
}

#[test]
fn exact_numbers_before_categories() {
    let (en, ja) = (localizer("en"), localizer("ja"));
    let lives = |n: i64| [("lives", Value::Integer(n))];
    assert_eq!(format(&en, "lives", &lives(0)), "No lives left");
    assert_eq!(format(&en, "lives", &lives(1)), "Last life");
    assert_eq!(format(&en, "lives", &lives(3)), "3 lives");
    assert_eq!(format(&ja, "lives", &lives(1)), "残機あと一つ");
    assert_eq!(format(&ja, "lives", &lives(3)), "残機：3");
}

#[test]
fn numbers_are_grouped_and_rounded_per_format() {
    let (en, ja) = (localizer("en"), localizer("ja"));
    let points = |n: i64| [("points", Value::Integer(n))];
    assert_eq!(format(&en, "score", &points(1_234_567)), "Score: 1,234,567");
    assert_eq!(format(&en, "score", &points(-1_234)), "Score: -1,234");
    assert_eq!(format(&ja, "score", &points(1_234_567)), "スコア：1,234,567");
    assert_eq!(format(&en, "score", &[("points", Value::Float(1234.56789))]), "Score: 1,234.568");
    assert_eq!(format(&en, "score", &[("points", Value::Float(2.5))]), "Score: 2.5");
    assert_eq!(format(&en, "score", &[("points", Value::Float(3.0))]), "Score: 3");
    assert_eq!(format(&en, "damage", &[("amount", Value::Float(12.345))]), "12.3 damage");
    assert_eq!(format(&en, "damage", &[("amount", Value::Integer(12))]), "12.0 damage");
    assert_eq!(format(&ja, "damage", &[("amount", Value::Integer(1200))]), "1,200.0ダメージ");
    assert_eq!(format(&en, "seed", &[("seed", Value::Integer(1_234_567))]), "Seed 1234567");
    assert_eq!(format(&en, "coins", &[]), "Exactly 1,000 coins");
    assert_eq!(format(&ja, "coins", &[]), "ちょうど1,000コイン");
    assert_eq!(format(&en, "price", &[("price", Value::Float(4.5))]), "4.50 each");
}

/// What a font subset is told numbers are written in is what they're written in.
#[test]
fn numbers_stay_within_what_a_subset_keeps() {
    let (en, ja) = (localizer("en"), localizer("ja"));
    let digits = "0123456789";
    for loc in [&en, &ja] {
        for value in [0i64, 9, 1234, -1_234_567_890_123] {
            let text = format(loc, "seed", &[("seed", value.into())]);
            let number = text.trim_start_matches("Seed ").trim_start_matches("シード ");
            assert!(number.chars().all(|c| NUMBER_CHARACTERS.contains(c)), "{text}");
            let text = format(loc, "damage", &[("amount", value.into())]);
            let number: String = text.chars().take_while(|c| c.is_ascii() && *c != ' ').collect();
            assert!(number.chars().all(|c| NUMBER_CHARACTERS.contains(c)), "{text}");
            assert!(number.chars().any(|c| digits.contains(c)), "{text}");
        }
    }
}

/// What the font cook (M3 U3) subsets to: a table's text in its own script, numbers included.
#[test]
fn a_table_says_what_it_can_show() {
    let ja = StringTable::read(JA).unwrap().characters();
    for shown in ['メ', 'ー', '領', '：', '、', '！', 'C', '+', '0', '9', ','] {
        assert!(ja.contains(&shown), "{shown}");
    }
    // Keys and variables aren't text.
    assert!(!ja.contains(&'$') && !ja.contains(&'q'));
    let en = StringTable::read(EN).unwrap().characters();
    assert!(en.contains(&'M') && !en.contains(&'メ'));
}

#[test]
fn a_selection_by_text_and_a_literal() {
    let en = localizer("en");
    assert_eq!(format(&en, "stance", &[("stance", Value::Text("low"))]), "Low guard");
    assert_eq!(format(&en, "stance", &[("stance", Value::Text("???"))]), "No guard");
    assert_eq!(format(&en, "braces", &[]), "Literal { and } and あ");
    assert_eq!(en.text("braces"), "Literal { and } and あ", "literals fold into one span");
}

#[test]
fn a_missing_key_falls_back_with_its_own_rules() {
    let ja = localizer("ja");
    assert_eq!(ja.text("only-english"), "This has no Japanese yet");
    // In English, so by English's plurals, where Japanese has none.
    assert_eq!(format(&ja, "only-english-count", &[("count", 1.into())]), "1 line, in English");
    assert_eq!(format(&ja, "only-english-count", &[("count", 2.into())]), "2 lines, in English");
    // And English's terms, rather than half a sentence of each.
    assert_eq!(format(&ja, "only-english-brand", &[]), "Sapphire stays English here");
    assert_eq!(ja.stats().unknown_keys, 0);
}

#[test]
fn what_cant_be_answered_shows_and_counts_and_never_panics() {
    let en = localizer("en");
    assert_eq!(en.text("no-such-key"), "no-such-key");
    assert_eq!(format(&en, "no-such-key", &[]), "no-such-key");
    assert_eq!(en.stats().unknown_keys, 2);
    assert_eq!(en.text("welcome"), "welcome", "it has placeables");
    assert_eq!(en.stats().not_plain, 1);
    assert_eq!(format(&en, "welcome", &[]), "Welcome to Sapphire, {$name}!");
    assert_eq!(en.stats().missing_args, 1);
    assert_eq!(format(&en, "enemies-left", &[]), "{$count} enemies left");
    assert_eq!(en.stats().missing_args, 3, "the selector's and the placeable's");
    assert_eq!(format(&en, "score", &[("points", Value::Float(f64::NAN))]), "Score: NaN");
    assert_eq!(format(&en, "score", &[("points", Value::Float(f64::INFINITY))]), "Score: NaN");
    assert_eq!(format(&en, "score", &[("points", Value::Float(1e300))]), "Score: NaN");
    assert_eq!(en.stats().bad_numbers, 3);
    // A name where a number was wanted shows the name.
    assert_eq!(format(&en, "damage", &[("amount", Value::Text("lots"))]), "lots damage");
    assert_eq!(format(&en, "enemies-left", &[("count", Value::Text("many"))]), "many enemies left");

    let empty = Localizer::new(LocaleSettings::default());
    assert_eq!(empty.text("title"), "title");
    assert_eq!(format(&empty, "title", &[]), "title");
}

#[test]
fn resolve_takes_whichever_a_message_needs() {
    let en = localizer("en");
    let mut scratch = String::from("left alone");
    assert_eq!(en.resolve("title", &[], &mut scratch), "Main Menu");
    assert_eq!(scratch, "left alone");
    let name = [("name", Value::Text("Aoi"))];
    assert_eq!(en.resolve("welcome", &name, &mut scratch), "Welcome to Sapphire, Aoi!");
    assert_eq!(en.resolve("welcome", &name, &mut scratch), "Welcome to Sapphire, Aoi!", "overwritten");
    assert_eq!(en.resolve("no-such-key", &[], &mut scratch), "no-such-key");
}

#[test]
fn the_language_switches_live_and_the_voice_stays_apart() {
    let mut loc = localizer("en");
    let before = loc.generation();
    assert_eq!(loc.text("title"), "Main Menu");
    loc.set_text_locale("ja").unwrap();
    assert_eq!(loc.text("title"), "メインメニュー");
    assert!(loc.generation() > before, "what shows text can tell");
    assert_eq!(loc.voice_locale(), "en", "text moved alone");
    assert_eq!(loc.font_chain("display").unwrap(), ["Dela Gothic One", "Oxanium", "M PLUS 1"]);

    let generation = loc.generation();
    loc.set_voice_locale("ja");
    assert_eq!(loc.settings(), &LocaleSettings { text: "ja".to_owned(), voice: "ja".to_owned() });
    assert_eq!(loc.generation(), generation, "voice moved alone");
    assert_eq!(loc.text("title"), "メインメニュー");

    loc.set_text_locale("en").unwrap();
    assert_eq!(loc.text("title"), "Main Menu");
    assert_eq!(loc.font_chain("display").unwrap(), ["Oxanium", "M PLUS 1"]);
    assert_eq!(loc.voice_locale(), "ja");

    let mut locales: Vec<&str> = loc.locales().collect();
    locales.sort_unstable();
    assert_eq!(locales, ["en", "ja"]);
    assert_eq!(loc.stats().switches, 2);
    assert!(loc.stats().table_bytes > 0);

    // What can't be switched to changes nothing.
    assert!(loc.set_text_locale("fr").is_err());
    assert_eq!(loc.text_locale(), "en");
    assert_eq!(loc.text("title"), "Main Menu");
    let bad = LocaleSettings { text: "fr".to_owned(), voice: "fr".to_owned() };
    assert!(loc.apply(&bad).is_err());
    assert_eq!(loc.voice_locale(), "ja");
    loc.apply(&LocaleSettings { text: "ja".to_owned(), voice: "en".to_owned() }).unwrap();
    assert_eq!((loc.text_locale(), loc.voice_locale()), ("ja", "en"));
}

#[test]
fn a_chain_waits_for_its_tables_and_a_table_can_be_replaced() {
    let mut loc = Localizer::new(LocaleSettings { text: "ja".to_owned(), voice: "ja".to_owned() });
    loc.install(StringTable::read(JA).unwrap()).unwrap();
    assert_eq!(loc.text("title"), "title", "its fallback isn't here yet");
    assert!(loc.set_text_locale("ja").is_err());
    loc.install(StringTable::read(EN).unwrap()).unwrap();
    assert_eq!(loc.text("title"), "メインメニュー");
    let generation = loc.generation();
    let mut changed = StringTable::read(JA).unwrap();
    changed.fonts.clear();
    loc.install(changed).unwrap();
    assert_eq!(loc.locales().count(), 2, "replaced, as hot reload does");
    assert!(loc.generation() > generation);
    assert_eq!(loc.font_chain("body").unwrap(), ["M PLUS 1"], "from down the chain");

    let mut bad = StringTable::read(EN).unwrap();
    bad.locale = "not a tag".to_owned();
    assert!(loc.install(bad).is_err());
    assert_eq!(loc.locales().count(), 2);
}

#[test]
fn settings_survive_a_settings_file() {
    let settings = LocaleSettings { text: "ja".to_owned(), voice: "en".to_owned() };
    let written = ron::to_string(&settings).unwrap();
    assert_eq!(ron::from_str::<LocaleSettings>(&written).unwrap(), settings);
    assert!(ron::from_str::<LocaleSettings>(r#"(text: "ja", voice: "en", subtitles: "ja")"#).is_err());
    assert_eq!(LocaleSettings::default(), LocaleSettings { text: "en".to_owned(), voice: "en".to_owned() });
}

/// Exit test 4's rule in small: with the mode on, everything through the tables is framed, in either
/// language, and anything else isn't, which is how a hard-coded string is found.
#[test]
fn pseudo_localization_finds_what_isnt_in_the_tables() {
    let mut loc = localizer("en");
    assert!(!is_framed(loc.text("title")));
    let before = loc.generation();
    loc.set_pseudo(true);
    assert!(loc.generation() > before);
    assert_eq!(loc.text("save"), "[!! Šååvéé !!]");
    assert_eq!(format(&loc, "enemies-left", &[("count", 1.into())]), "[!! ÖÖñéé ééñéémý ļééfţ~ !!]");
    let many = format(&loc, "enemies-left", &[("count", 1234.into())]);
    assert!(many.contains("1,234"), "numbers stay as they read: {many}");
    assert_eq!(loc.text("only-attributes.label"), "[!! ÖÖñļý ååñ ååţţŕîîbûûţéé~ !!]");

    let args: [Arg<'_>; 8] = [
        ("name", "Aoi".into()),
        ("count", 2.into()),
        ("lives", 2.into()),
        ("points", 2.into()),
        ("amount", 2.into()),
        ("seed", 2.into()),
        ("price", 2.into()),
        ("stance", "low".into()),
    ];
    let table = StringTable::read(EN).unwrap();
    for locale in ["en", "ja"] {
        loc.set_text_locale(locale).unwrap();
        for entry in &table.entries {
            let key = table.str(entry.key);
            if key.starts_with('-') {
                continue;
            }
            let shown = format(&loc, key, &args);
            assert!(is_framed(&shown), "{locale} {key}: {shown}");
        }
    }

    // What a screen shows that didn't come through a message.
    for hard_coded in ["Main Menu", "メインメニュー", "OK", ""] {
        assert!(!is_framed(hard_coded), "{hard_coded}");
    }
    assert!(!is_framed(loc.text("no-such-key")), "and an unknown key stands out the same way");

    // A clipped message has lost its close.
    let shown = loc.text("title").to_owned();
    assert!(!is_framed(&shown[..shown.len() - 4]));

    // The mode's characters are ones a subset can be told to keep.
    let characters = pseudo_characters();
    loc.set_text_locale("en").unwrap();
    for letter in loc.text("title").chars().filter(|letter| *letter != ' ') {
        assert!(characters.contains(&letter), "{letter}");
    }

    loc.set_pseudo(false);
    assert_eq!(loc.text("title"), "Main Menu");
    assert_eq!(loc.stats().unknown_keys, 1);
}

/// A table no cook made: its references loop, or fan out without end. They're stopped, shown as far
/// as they got and counted, and nothing else is hurt.
#[test]
fn hostile_references_stop() {
    let mut pool = String::from("loopfantext");
    let loop_key = Span { start: 0, len: 4 };
    let fan_key = Span { start: 4, len: 3 };
    let text = Span { start: 7, len: 4 };
    let args = Run::default();
    pool.push_str("-term");
    let term_key = Span { start: 11, len: 5 };
    let table = StringTable {
        locale: "en".to_owned(),
        fallback: vec![],
        fonts: vec![],
        pool,
        entries: vec![
            Entry { key: term_key, pattern: Pattern::Parts(Run { start: 0, len: 1 }) },
            Entry { key: fan_key, pattern: Pattern::Parts(Run { start: 1, len: 3 }) },
            Entry { key: loop_key, pattern: Pattern::Parts(Run { start: 4, len: 2 }) },
        ],
        parts: vec![
            Part::Place(Expression::Term { key: term_key, args }),
            Part::Text(text),
            Part::Place(Expression::Message(fan_key)),
            Part::Place(Expression::Message(fan_key)),
            Part::Text(text),
            Part::Place(Expression::Message(loop_key)),
        ],
        selects: vec![],
        variants: vec![],
        args: Vec::<TermArg>::new(),
    };
    let cooked = table.to_cooked().unwrap();
    let mut loc = Localizer::new(LocaleSettings::default());
    loc.install(StringTable::read(&cooked).unwrap()).unwrap();

    let looped = format(&loc, "loop", &[]);
    assert_eq!(looped, "text".repeat(sapphire_strings::MAX_DEPTH as usize));
    assert_eq!(loc.stats().overruns, 1);

    let fanned = format(&loc, "fan", &[]);
    assert!(fanned.starts_with("text") && fanned.len() < sapphire_strings::MAX_STEPS as usize * 4 + 1);
    assert_eq!(loc.stats().overruns, 2);

    let _ = format(&loc, "-term", &[]);
    assert_eq!(loc.stats().overruns, 3);

    // A number format can't be spoiled past the reader, so the largest it lets through is what's left.
    let huge = NumberFormat { min_fraction: 20, max_fraction: 20, grouping: true };
    let mut table = StringTable::read(EN).unwrap();
    table.parts[0] = Part::Place(Expression::Number { value: 1e29, format: huge });
    let spoiled = StringTable::read(&table.to_cooked().unwrap()).unwrap();
    let mut loc = Localizer::new(LocaleSettings::default());
    loc.install(spoiled).unwrap();
    for entry in &table.entries {
        let _ = format(&loc, table.str(entry.key), &[]);
    }
}

#[test]
fn replacing_a_fallback_chain_matches_a_fresh_install_even_while_incomplete() {
    for pseudo in [false, true] {
        let mut hot = localizer("ja");
        hot.set_pseudo(pseudo);
        let mut changed = StringTable::read(JA).unwrap();
        changed.fallback = vec!["fr".to_owned()];
        changed.fonts.clear();
        let generation = hot.generation();
        hot.install(changed.clone()).unwrap();
        assert_eq!(hot.generation(), generation + 1);
        let mut fresh = Localizer::new(hot.settings().clone());
        fresh.set_pseudo(pseudo);
        fresh.install(StringTable::read(EN).unwrap()).unwrap();
        fresh.install(changed).unwrap();
        for key in ["title", "only-english"] {
            assert_eq!(hot.text(key), fresh.text(key));
            assert_eq!(hot.text(key), key, "an incomplete chain answers no keys");
        }
        assert_eq!(hot.font_chain("body"), None);
        // An inactive table doesn't revive an obsolete fallback. Arrival of the declared one does.
        hot.install(StringTable::read(EN).unwrap()).unwrap();
        assert_eq!(hot.text("title"), "title");
        let mut french = StringTable::read(EN).unwrap();
        french.locale = "fr".to_owned();
        hot.install(french.clone()).unwrap();
        fresh.install(french).unwrap();
        assert_eq!(hot.text("title"), fresh.text("title"));
        assert_eq!(hot.text("only-english"), fresh.text("only-english"));
        assert_eq!(hot.font_chain("body"), fresh.font_chain("body"));
        let mut without_fallback = StringTable::read(JA).unwrap();
        without_fallback.fallback.clear();
        hot.install(without_fallback).unwrap();
        assert_eq!(hot.text("only-english"), "only-english");
        assert_ne!(hot.text("title"), "title");
    }
}

#[test]
fn pseudo_selects_by_original_terms_while_showing_the_same_terms_as_pseudo_text() {
    use sapphire_strings::{FRAME_CLOSE, FRAME_OPEN, pseudolocalize};

    for locale in ["en", "ja"] {
        let mut loc = localizer(locale);
        let ordinary = format(&loc, "sword-taken", &[]);
        loc.set_pseudo(true);
        assert_eq!(
            format(&loc, "sword-taken", &[]),
            format!("{FRAME_OPEN}{}{FRAME_CLOSE}", pseudolocalize(&ordinary)),
            "pseudo mode must exercise the same branch"
        );
        assert_eq!(loc.text("-sword.gender"), pseudolocalize("neuter"));
        // Argument selectors still read semantic caller/term arguments.
        assert!(format(&loc, "inside", &[]).contains(&pseudolocalize(if locale == "en" {
            "the inner realm"
        } else {
            "内なる領域"
        })));
        loc.set_pseudo(false);
        assert_eq!(format(&loc, "sword-taken", &[]), ordinary);
    }
    // The term and select can both come from a fallback, even when the key is a nested reference.
    let mut english = StringTable::read(EN).unwrap();
    let key = Span { start: english.pool.len() as u32, len: 6 };
    english.pool.push_str("nested");
    let reference = english.entry("sword-taken").unwrap().key;
    let start = english.parts.len() as u32;
    english.parts.push(Part::Place(Expression::Message(reference)));
    english.entries.push(Entry { key, pattern: Pattern::Parts(Run { start, len: 1 }) });
    let pool = &english.pool;
    english
        .entries
        .sort_by_key(|entry| &pool[entry.key.start as usize..(entry.key.start + entry.key.len) as usize]);
    let mut loc = localizer("ja");
    loc.install(english).unwrap();
    loc.set_pseudo(true);
    let correct = pseudolocalize("It is yours");
    assert!(format(&loc, "nested", &[]).contains(&correct));
}
