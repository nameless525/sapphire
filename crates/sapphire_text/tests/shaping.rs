//! What shaping has to do for Japanese and for the three fonts, with the real fonts: no system fonts,
//! kinsoku, mixed lines, one baseline, and weight along the variable axis.
//!
//! `cargo test -p sapphire_text --test shaping`.

mod common;

use common::{
    FAMILY_BODY, FAMILY_DISPLAY_JA, FAMILY_DISPLAY_LATIN, MPLUS1, OXANIUM, read_font, style, system,
};
use sapphire_text::{TextSystem, family_list};

/// Characters a line must not start with (JIS X 4051's line-start prohibitions, the common ones).
const NO_LINE_START: &str = "。、．，）」』】〉》！？：；・ーぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮ々ゝゞ";
/// Characters a line must not end with.
const NO_LINE_END: &str = "（「『【〈《";

/// A paragraph with a prohibited character every few cells, so most widths try to break at one.
const PARAGRAPH: &str = "嵐が過ぎた朝、港の鐘がふたつ鳴った。「まだ船は戻らないのか」と老人は海を見つめたまま尋ねた。\
    刀を鞘に収め、静かに首を振る。「約束は守ります。夜明けまでに北の門を開けて、街の人たちを逃がしてください。」\
    ショップで買ったキャップをかぶって、ちょっとだけ外へ出よう！本当に？もちろん。";

#[test]
fn kinsoku_holds_at_every_width() {
    let dir = common::fonts_dir();
    let mut text = system(&dir);
    let body = style(FAMILY_BODY, 14.0, 400.0, "ja");
    let mut breaks = 0;
    let mut offsets = common::Digest::default();
    for width in (70..=420).step_by(7) {
        let shaped = text.shape(PARAGRAPH, &body, 1.0, Some(width as f32));
        assert!(shaped.lines.len() > 1 || width > 400, "{width} points should wrap");
        for pair in shaped.lines.windows(2) {
            let (before, after) = (
                &PARAGRAPH[pair[0].clone().start as usize..pair[0].end as usize],
                &PARAGRAPH[pair[1].start as usize..pair[1].end as usize],
            );
            let first = after.chars().next().expect("a line has text");
            let last = before.chars().next_back().expect("a line has text");
            breaks += 1;
            assert!(!NO_LINE_START.contains(first), "{width} points: a line starts with {first}: {after}");
            assert!(!NO_LINE_END.contains(last), "{width} points: a line ends with {last}: {before}");
            offsets.u32(pair[1].start);
        }
    }
    // Build with `--features parley/complex-scripts` and compare: the digest says if any break moved.
    eprintln!("kinsoku: {breaks} breaks over 51 widths held every rule, offsets {:016x}", offsets.0);
    assert!(breaks > 200, "only {breaks} breaks: the test isn't testing");
}

/// Where the lines of a wrapped paragraph actually fall, cell by cell: at 14 pixels in a box of 140, a
/// line of full-width characters holds ten. These are the rules of JIS X 4051 as a reader sees them,
/// so a segmenter that broke only at punctuation, or anywhere at all, fails here where the test above
/// passes. Each line was read by hand when this was written.
///
/// None of it needs a word dictionary. In a debug build ICU4X prints `ICU4X data error: No segmentation
/// model for complex script: Chinese/Japanese` when shaping Japanese, which is parley's word segmenter
/// finding no `cjdict`. Word boundaries only move an editing cursor. Line breaks come from the line
/// segmenter's rules, and building with `parley/complex-scripts` leaves the offsets above identical.
#[test]
fn a_wrapped_japanese_paragraph_breaks_where_a_reader_expects() {
    let cases: &[(&str, &str, &[&str])] = &[
        ("any two kana may part", "あいうえおかきくけこさしすせそ", &["あいうえおかきくけこ", "さしすせそ"]),
        ("any two Han may part", "攻撃力防御力会心率会心攻撃力", &["攻撃力防御力会心率会", "心攻撃力"]),
        ("a closing mark that fits stays", "あいうえおかきくけ。こさし", &["あいうえおかきくけ。", "こさし"]),
        (
            "one that doesn't brings a friend",
            "あいうえおかきくけこ。さし",
            &["あいうえおかきくけ", "こ。さし"],
        ),
        (
            "an opening bracket never ends a line",
            "あいうえおかきくけ「こさし",
            &["あいうえおかきくけ", "「こさし"],
        ),
        ("a small kana never starts one", "あいうえおかきくけこっさし", &["あいうえおかきくけ", "こっさし"]),
        ("nor a long vowel mark", "アイウエオカキクケコーサシ", &["アイウエオカキクケ", "コーサシ"]),
        ("nor an iteration mark", "あいうえおかきくけ人々さし", &["あいうえおかきくけ", "人々さし"]),
        ("nor a middle dot", "あいうえおかきくけこ・さし", &["あいうえおかきくけ", "こ・さし"]),
        ("nor a wave dash", "あいうえおかきくけこ〜さし", &["あいうえおかきくけ", "こ〜さし"]),
        ("nor a run of marks", "あいうえおかきくけこ！？さし", &["あいうえおかきくけ", "こ！？さし"]),
        ("a bracket then a stop", "あいうえおかきくけこ」。さし", &["あいうえおかきくけ", "こ」。さし"]),
        ("an ellipsis doesn't part", "あいうえおかきくけ……さし", &["あいうえおかきく", "け……さし"]),
        ("a dash doesn't part", "あいうえおかきくけ――さし", &["あいうえおかきくけ", "――さし"]),
        ("a number stays whole", "あいうえおかきく12,480コイン", &["あいうえおかきく", "12,480コイン"]),
        ("a percentage too", "あいうえおかきく100%しすせそ", &["あいうえおかきく", "100%しすせそ"]),
        ("a Latin word too", "あいうえおかきくLevelさし", &["あいうえおかきく", "Levelさし"]),
        ("Latin parts at its space", "あいうえおかきくHP 6,240さし", &["あいうえおかきくHP ", "6,240さし"]),
        ("a newline is a line", "あいうえお\nかきく", &["あいうえお\n", "かきく"]),
    ];
    let dir = common::fonts_dir();
    let mut text = system(&dir);
    let body = style(FAMILY_BODY, 14.0, 400.0, "ja");
    for (what, source, expected) in cases {
        let shaped = text.shape(source, &body, 1.0, Some(140.0));
        let lines: Vec<&str> =
            shaped.lines.iter().map(|line| &source[line.start as usize..line.end as usize]).collect();
        assert_eq!(&lines, expected, "{what}");
    }
}

#[test]
fn a_line_breaks_between_han_and_kana_and_not_inside_latin_words() {
    let dir = common::fonts_dir();
    let mut text = system(&dir);
    let body = style(FAMILY_BODY, 14.0, 400.0, "ja");
    let mixed = "Sapphire Engine で作ったゲームの設定画面を Wuthering Waves のように表示します。Level 90 の Boss を倒そう";
    for width in [80.0, 120.0, 160.0, 200.0, 260.0] {
        let shaped = text.shape(mixed, &body, 1.0, Some(width));
        for line in &shaped.lines {
            let line = &mixed[line.start as usize..line.end as usize];
            eprintln!("{width:>5}: {line}");
        }
        for pair in shaped.lines.windows(2) {
            let end = mixed[..pair[0].end as usize].chars().next_back().expect("text");
            let start = mixed[pair[1].start as usize..].chars().next().expect("text");
            assert!(
                !(end.is_ascii_alphanumeric() && start.is_ascii_alphanumeric()),
                "{width}: a break inside a Latin word: {end}|{start}",
            );
            assert!(!NO_LINE_START.contains(start), "{width}: a line starts with {start}");
        }
    }
    // Japanese with no spaces still breaks, in a line narrower than the whole.
    let shaped = text.shape("攻撃力防御力会心率会心攻撃力防御力", &body, 1.0, Some(70.0));
    assert!(shaped.lines.len() >= 3, "Han breaks anywhere: {} lines", shaped.lines.len());
}

/// What the three faces give on one line, for the eye and for layout: the baseline is one number
/// wherever a run comes from.
#[test]
fn three_faces_sit_on_one_baseline() {
    let dir = common::fonts_dir();
    let mut text = system(&dir);
    for (family, name) in [
        (FAMILY_BODY, "M PLUS 1"),
        (FAMILY_DISPLAY_LATIN, "Oxanium, M PLUS 1"),
        (FAMILY_DISPLAY_JA, "Dela Gothic One, M PLUS 1"),
    ] {
        let shaped = text.shape("Level 90 必殺技ゲージ", &style(family, 24.0, 400.0, "ja"), 1.0, None);
        let baseline = shaped.baselines[0];
        let fonts: Vec<u64> = shaped.runs.iter().map(|run| run.font.data.id()).collect();
        eprintln!(
            "{name}: height {:.2}, baseline {baseline:.2}, {} runs from fonts {fonts:?}",
            shaped.height,
            shaped.runs.len(),
        );
        for glyph in &shaped.glyphs {
            assert_eq!(glyph.y, baseline, "{name}: every run on the line's baseline");
        }
    }
    for (name, file) in [("M PLUS 1", MPLUS1), ("Oxanium", OXANIUM), ("Dela Gothic One", common::DELA)] {
        let font = read_font(&dir, file);
        let font = swash::FontRef::from_index(&font, 0).expect("a font");
        let metrics = font.metrics(&[]);
        let em = f32::from(metrics.units_per_em);
        eprintln!(
            "{name}: ascent {:.3} descent {:.3} leading {:.3} cap {:.3} x {:.3} em",
            metrics.ascent / em,
            metrics.descent / em,
            metrics.leading / em,
            metrics.cap_height / em,
            metrics.x_height / em,
        );
    }
    // A Latin display face needs M PLUS 1 for the Japanese, so a line that mixes them has both runs.
    let mixed = text.shape("Level 90 必殺技", &style(FAMILY_DISPLAY_LATIN, 24.0, 600.0, "ja"), 1.0, None);
    let fonts = mixed.runs.iter().map(|run| run.font.data.id()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(fonts.len(), 2, "Oxanium for Latin, M PLUS 1 for Japanese");
}

#[test]
fn weight_moves_the_variable_axis() {
    let dir = common::fonts_dir();
    let mut text = TextSystem::new();
    text.register_font(read_font(&dir, MPLUS1));
    text.register_font(read_font(&dir, OXANIUM));
    for (family, sample) in [(FAMILY_BODY, "必殺技 Attack"), ("Oxanium", "Attack 1,284")] {
        let mut last_coord = i16::MIN;
        let mut last_width = 0.0;
        for weight in [200.0, 400.0, 600.0, 800.0] {
            let shaped = text.shape(sample, &style(family, 24.0, weight, "ja"), 1.0, None);
            // A font's default instance has no coordinates: Oxanium's is its lightest.
            let coord = shaped.runs[0].coords.first().copied().unwrap_or(0);
            eprintln!("{family} {weight}: wght coord {coord}, width {:.2}", shaped.width);
            assert!(coord > last_coord, "{family}: {weight} must move wght");
            assert!(shaped.width >= last_width, "{family}: {weight} must not get narrower");
            (last_coord, last_width) = (coord, shaped.width);
        }
    }
}

/// With nothing registered, or nothing covering a script, there is no font to find: on any OS, a
/// system font would have shaped these.
#[test]
fn nothing_comes_from_the_system() {
    let mut empty = TextSystem::new();
    assert!(empty.family_names().is_empty(), "no families until one's registered");
    let shaped = empty.shape("Hello 日本語", &style("sans-serif", 14.0, 400.0, "ja"), 1.0, None);
    assert!(shaped.glyphs.is_empty(), "no font, no glyphs: {:?}", shaped.glyphs);

    let dir = common::fonts_dir();
    let mut text = system(&dir);
    let mut names = text.family_names();
    names.sort();
    eprintln!("families: {names:?}");
    assert_eq!(names, ["Dela Gothic One", "M PLUS 1", "Oxanium"]);
    // Every desktop and phone OS has fonts for Hangul, Arabic and Devanagari. M PLUS 1 has none, and
    // nothing else may answer, so they all come out as its missing glyph, however the style asks.
    for family in [FAMILY_BODY, "sans-serif", "Helvetica, Arial, 'Noto Sans', Roboto, 'Segoe UI'"] {
        for sample in ["한국어", "العربية", "हिन्दी"] {
            let shaped = text.shape(sample, &style(family, 14.0, 400.0, "ja"), 1.0, None);
            assert!(
                shaped.glyphs.iter().all(|glyph| glyph.id == 0),
                "{family} {sample}: {:?}",
                shaped.glyphs,
            );
        }
    }
}

/// A script's fallbacks are the style's own list: nothing past it answers, and the list answers for
/// every script, including those `fontique`'s fallback table doesn't track (hiragana and katakana
/// with a locale, where setting a family reports false).
#[test]
fn a_chain_is_a_list_of_families() {
    let dir = common::fonts_dir();
    let mut text = TextSystem::new();
    text.register_font(read_font(&dir, OXANIUM));
    text.register_font(read_font(&dir, MPLUS1));
    let samples = ["設定", "かな", "カナ", "設定かなカナ", "Level 90 必殺技"];
    let alone = style("Oxanium", 20.0, 400.0, "ja");
    for sample in &samples[..4] {
        let shaped = text.shape(sample, &alone, 1.0, None);
        assert!(shaped.glyphs.iter().all(|glyph| glyph.id == 0), "{sample}: no list, no glyphs");
    }
    let families = family_list(["Oxanium", "M PLUS 1"]);
    assert_eq!(families, "'Oxanium', 'M PLUS 1'");
    let listed = sapphire_text::TextStyle { families: &families, ..alone };
    for sample in samples {
        let shaped = text.shape(sample, &listed, 1.0, None);
        assert!(shaped.glyphs.iter().all(|glyph| glyph.id != 0), "{sample}: {:?}", shaped.glyphs);
    }
    // A style borrowed from a string that's gone finds what it made while it was here.
    let again = family_list(["Oxanium", "M PLUS 1"]);
    let before = text.take_stats();
    assert_eq!(before.hits, 0);
    text.shape(samples[0], &sapphire_text::TextStyle { families: &again, ..alone }, 1.0, None);
    assert_eq!(text.take_stats().hits, 1, "the cache keeps its own copy of what a style says");
}

#[test]
fn punctuation_in_an_imported_family_selects_that_exact_font_before_its_fallback() {
    let dir = common::fonts_dir();
    let mut renamed = (*read_font(&dir, OXANIUM)).clone();
    // Keep every name record's length/offset unchanged. A valid font can name its family with
    // punctuation; all matching Unicode name records use seven UTF-16 code units here.
    let family = "A'B\",\\C";
    let old: Vec<_> = "Oxanium".encode_utf16().flat_map(u16::to_be_bytes).collect();
    let new: Vec<_> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
    assert_eq!(old.len(), new.len());
    let tables = u16::from_be_bytes(renamed[4..6].try_into().unwrap()) as usize;
    let name = (0..tables)
        .map(|index| &renamed[12 + index * 16..12 + (index + 1) * 16])
        .find(|record| &record[..4] == b"name")
        .unwrap();
    let start = u32::from_be_bytes(name[8..12].try_into().unwrap()) as usize;
    let len = u32::from_be_bytes(name[12..16].try_into().unwrap()) as usize;
    let names = &mut renamed[start..start + len];
    let mut replaced = 0;
    for index in 0..=names.len() - old.len() {
        if names[index..index + old.len()] == old {
            names[index..index + old.len()].copy_from_slice(&new);
            replaced += 1;
        }
    }
    assert!(replaced > 0);
    let renamed = std::sync::Arc::new(renamed);
    let mut text = TextSystem::new();
    text.register_font(read_font(&dir, MPLUS1));
    text.register_font(std::sync::Arc::clone(&renamed));
    assert!(text.family_names().iter().any(|name| name == family));
    let families = family_list([family, "M PLUS 1"]);
    let style = sapphire_text::TextStyle { families: &families, ..style(FAMILY_BODY, 24.0, 400.0, "en") };
    let shaped = text.shape("Attack", &style, 1.0, None);
    assert!(!shaped.runs.is_empty());
    assert!(shaped.runs.iter().all(|run| run.font.data.as_ref() == renamed.as_slice()));
}
