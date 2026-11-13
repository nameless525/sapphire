//! Every committed timeline's simulation digest, pinned: what its replays and `tables_hash` read. A
//! change to the presentation half (M5 C5's faces and looks, a shot's focus) must leave each of these
//! exactly as it was; a change that moves one moves replays, and belongs in a change that says so.

use sapphire_timeline::TimelineDocument;

/// Each committed document, and its digest as it cooked before timeline version 2. None of the shipped
/// three changed, and the cinematic's is the one `cooked.rs` pins too; the cutscene gained faces and looks.
const PINNED: &[(&str, &str, u64)] = &[
    (
        "ultimate",
        include_str!("../../../content/combat/player/ultimate.timeline.ron"),
        5_164_331_536_619_791_185,
    ),
    (
        "square_look",
        include_str!("../../../content/sequences/square_look.timeline.ron"),
        14_166_511_957_577_999_600,
    ),
    (
        "court_gate",
        include_str!("../../../content/zones/lantern_ward/court_gate.timeline.ron"),
        13_393_692_722_777_780_803,
    ),
    ("cinematic", include_str!("../test_content/cinematic.timeline.ron"), 2_869_729_819_227_249_548),
    ("cutscene", include_str!("../test_content/cutscene.timeline.ron"), 14_018_787_872_005_434_949),
];

#[test]
fn every_committed_timelines_digest_is_pinned() {
    let found: Vec<(&str, u64)> = PINNED
        .iter()
        .map(|&(name, text, _)| {
            let document = TimelineDocument::from_ron(text).unwrap_or_else(|error| panic!("{name}: {error}"));
            (name, document.cook().unwrap_or_else(|error| panic!("{name}: {error}")).simulation.digest())
        })
        .collect();
    let pinned: Vec<(&str, u64)> = PINNED.iter().map(|&(name, _, digest)| (name, digest)).collect();
    assert_eq!(found, pinned, "a timeline's simulation digest moved: every replay that plays it moves too");
}
