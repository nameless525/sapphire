//! The game's flow as the content ships it (M3 G1), with no GPU: the Lantern Ward's flow stands a game against
//! this build's scenarios and the flag registry, its scenario loads the court's gate and the guide's
//! conversation its beats name, its music names the ward's score's phases, and the save the
//! `game-ward-checkpoint` golden continues reads as a save of the same zone.

use std::path::{Path, PathBuf};

use game_combat::combat::SCENARIOS;
use game_flow::document::{FlowDocument, Then, When};
use game_flow::game::{FlowContent, Game, GameSetup};
use game_flow::progress::Progress;
use sapphire_assets::score::Score;
use sapphire_core::Name;
use sapphire_dialogue::FlagRegistry;
use sapphire_gameplay::replay::find_scenario;
use sapphire_gameplay::{DialogueId, TimelineId};
use sapphire_serialize::AssetId;

/// content/zones/lantern_ward/court_gate.timeline.ron
const COURT_GATE: AssetId = AssetId::from_text("7c42b73f-d94d-4eb2-b064-2637819640a8");
/// content/dialogue/ward_guide.dialogue.ron
const WARD_GUIDE: AssetId = AssetId::from_text("7b3a0339-b2a1-4ac8-9cfa-458d4b0134a7");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn text(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn flow() -> FlowDocument {
    FlowDocument::from_ron(&text("content/game.flow.ron")).expect("the game's flow reads")
}

#[test]
fn the_ward_s_flow_stands_a_game_on_what_its_scenario_loads() {
    let registry = FlagRegistry::from_ron(&text("content/flags.ron")).expect("the flags read");
    let registered = registry.flags.iter().map(|flag| Name::new(flag)).collect();
    let document = flow();
    assert_eq!(document.scenario, "ward-run");
    let content = FlowContent { document: document.clone(), registered };
    Game::new(content, GameSetup::default()).expect("the game has what its flow names");

    // The run's scenario loads the gate before anything else, and the guide's conversation as the only one.
    let scenario = find_scenario(&SCENARIOS, &document.scenario).expect("this build holds the ward's run");
    assert_eq!(scenario.timelines().expect("loads"), [(COURT_GATE, TimelineId::from_raw(0))]);
    assert_eq!(scenario.dialogues().expect("loads"), [(WARD_GUIDE, DialogueId::from_raw(0))]);
    // The beats play the gate when the court starts, the guide when it ends, and only before the guide's met.
    assert!(document.beats.iter().any(|beat| beat.then == Then::Timeline(COURT_GATE)));
    assert!(document.beats.iter().any(|beat| {
        beat.when == When::Ended(COURT_GATE)
            && beat.then == Then::Dialogue(WARD_GUIDE)
            && beat.unless.as_deref() == Some("met_guide")
    }));
    assert!(document.beats.iter().any(|beat| beat.then == Then::Victory));
}

#[test]
fn a_flow_naming_what_its_scenario_doesnt_load_is_refused() {
    let mut document = flow();
    let unknown = AssetId::from_text("00000000-0000-4000-8000-000000000001");
    document.beats[0].then = Then::Dialogue(unknown);
    let registry = FlagRegistry::from_ron(&text("content/flags.ron")).expect("the flags read");
    let registered = registry.flags.iter().map(|flag| Name::new(flag)).collect();
    let refused = Game::new(FlowContent { document, registered }, GameSetup::default()).err();
    assert!(refused.is_some_and(|error| error.contains("loads no dialogue")));
    // A beat's `unless` names a registered flag.
    let refused =
        Game::new(FlowContent { document: flow(), registered: Vec::new() }, GameSetup::default()).err();
    assert!(refused.is_some_and(|error| error.contains("met_guide")));
}

/// A game's run meets every fighter in its zone, so its scenario's pool holds them all (M3 G1c): the market's
/// room alone, which holds only what its own fight brings, can't be a game's run of the whole ward.
#[test]
fn a_flow_whose_scenario_cant_hold_everyone_it_meets_is_refused() {
    let mut document = flow();
    document.scenario = "ward-market".to_owned();
    document.beats.retain(|beat| beat.then == Then::Victory);
    let registry = FlagRegistry::from_ron(&text("content/flags.ron")).expect("the flags read");
    let registered = registry.flags.iter().map(|flag| Name::new(flag)).collect();
    let refused = Game::new(FlowContent { document, registered }, GameSetup::default()).err();
    assert!(
        refused.as_deref().is_some_and(|error| error.contains("the last would never come")),
        "{refused:?}"
    );
}

#[test]
fn the_flow_s_music_is_the_ward_s_score_s_phases() {
    let score =
        Score::from_document(&text("content/audio/generated/music/lantern.score.ron")).expect("reads");
    let music = flow().music;
    for phase in [&music.title, &music.victory, &music.defeat] {
        assert!(score.phase(phase).is_some(), "the score has no phase `{phase}`");
    }
}

#[test]
fn the_checkpoint_golden_s_save_is_at_the_court_of_the_flow_s_zone() {
    let progress: Progress =
        sapphire_serialize::from_ron(&text("tests/saves/ward-court.progress.ron")).expect("the save reads");
    assert_eq!(progress.zone, flow().zone);
    assert_eq!(progress.start.checkpoint.as_deref(), Some("court"));
    let mut cleared = progress.start.cleared.clone();
    cleared.sort_unstable();
    assert_eq!(cleared, progress.start.cleared, "ascending, as a save keeps them");
}
