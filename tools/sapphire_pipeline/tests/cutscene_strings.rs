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
