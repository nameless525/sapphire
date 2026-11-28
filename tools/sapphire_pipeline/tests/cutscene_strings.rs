//! M3 Q3: the Lantern Ward's gate cutscene shows its subtitles in the chosen language. Its English and
//! Japanese tables are cooked here from the content tree as a cook would, the timeline plays, and at each
//! line the playhead's subtitle and speaker resolve through a localizer, in English, then after a live switch
//! in Japanese. Drawing them is the game interface's (U7); what it draws is what this resolves.

use std::path::PathBuf;
use std::sync::Arc;

use sapphire_loc::{LocaleSettings, Localizer};
use sapphire_pipeline::loc::{LocaleDocument, compile};
use sapphire_sequencer::{Origin, TimelinePlayer};
use sapphire_strings::StringTable;
use sapphire_timeline::TimelineDocument;

const GATE: &str = include_str!("../../../content/zones/lantern_ward/court_gate.timeline.ron");

/// Who says a line and what they say, in one language.
type Said = (&'static str, &'static str);

/// What each line says, at a moment it's up, in English and Japanese.
const LINES: [(f32, Said, Said); 3] = [
    (
        1.5,
        ("Wanderer", "The temple court. It was quiet a moment ago."),
        ("旅人", "神殿の中庭……さっきまで静かだったのに。"),
    ),
    (
        4.0,
        ("Warden of the Gate", "None pass the lantern gate."),
        ("門の守り手", "灯籠の門を越える者はいない。"),
    ),
    (6.0, ("Wanderer", "Then I'll go through you."), ("旅人", "なら、あなたを越えて行く。")),
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `tag`'s table from its document and its cutscenes' strings: the cook's lowering of the same files.
fn table(tag: &str) -> StringTable {
    let loc = root().join("content/loc");
    let document: LocaleDocument = sapphire_serialize::from_ron(
        &std::fs::read_to_string(loc.join(format!("{tag}.locale.ron"))).unwrap(),
    )
    .unwrap();
    let path = loc.join(tag).join("cutscenes.ftl");
    let text = std::fs::read_to_string(&path).unwrap();
    compile(tag, &document, &[(path, text)]).unwrap()
}

/// M6 T16a: the cook's check holds a subtitle's key and its speaker's to the tables: the gate's resolve in both
/// languages, and a key or speaker no table holds is refused by its field and the locale that lacks it.
#[test]
fn a_subtitle_names_a_string_the_tables_hold() {
    let tables = [table("en"), table("ja")];
    let cooked = |text: &str| {
        let path = PathBuf::from("court_gate.timeline.ron");
        vec![(path, TimelineDocument::from_ron(text).unwrap().cook().unwrap())]
    };
    sapphire_pipeline::timeline::check_keys(&cooked(GATE), &tables).expect("the gate's lines resolve");
    let refused = |text: String| {
        let (path, missing) = sapphire_pipeline::timeline::check_keys(&cooked(&text), &tables).unwrap_err();
        assert_eq!(path, PathBuf::from("court_gate.timeline.ron"));
        (missing.field, missing.key, missing.locale)
    };
    let unknown = GATE.replacen("\"subtitle-court-gate-warning\"", "\"subtitle-court-gate-unwritten\"", 1);
    assert_eq!(
        refused(unknown),
        ("subtitles[1].key".to_owned(), "subtitle-court-gate-unwritten".to_owned(), "en".to_owned())
    );
    let nobody = GATE.replacen("Some(\"speaker-warden\")", "Some(\"speaker-nobody\")", 1);
    assert_eq!(refused(nobody).0, "subtitles[1].speaker");
}

#[test]
fn the_gates_subtitles_follow_the_chosen_language() {
    let mut loc = Localizer::new(LocaleSettings::default());
    loc.install(table("en")).unwrap();
    loc.install(table("ja")).unwrap();
    loc.set_text_locale("en").unwrap();
    let timeline = Arc::new(TimelineDocument::from_ron(GATE).unwrap().cook().unwrap());
    let mut player = TimelinePlayer::new(timeline, Origin::default());
    let mut played = 0.0;
    for (at, english, japanese) in LINES {
        player.advance(at - played, None);
        played = at;
        let subtitle = player.subtitle().expect("a line is up");
        let speaker = subtitle.speaker.expect("every line says who");
        loc.set_text_locale("en").unwrap();
        assert_eq!((loc.text(speaker), loc.text(subtitle.key)), english);
        let generation = loc.generation();
        loc.set_text_locale("ja").unwrap();
        assert_ne!(loc.generation(), generation, "a switch tells whatever shows text to resolve again");
        assert_eq!((loc.text(speaker), loc.text(subtitle.key)), japanese);
    }
    // Every key resolved: none was a key shown as itself.
    assert_eq!(loc.stats().unknown_keys, 0);
    player.advance(1.5, None);
    assert!(player.subtitle().is_none(), "nothing is said between lines");
}
