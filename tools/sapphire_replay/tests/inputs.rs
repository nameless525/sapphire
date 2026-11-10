//! The committed replays against the scripted input documents (M4 R2): every replay a script recorded is what
//! one of `content/inputs/`'s documents gives, tick for tick on the cast its scenario sets up, with the same
//! commands. So a document can't drift from the file it made, and a change to how documents are read that would
//! change what a recording steps fails here, before any checksum could say so.

use std::fs;
use std::path::Path;

use sapphire_gameplay::TickActions;
use sapphire_gameplay::replay::{Player, Replay, ReplayCommand, SCRIPTS, Script};
use sapphire_script::scenarios::ALL;

/// The replays no script made: a run of the game's flow, which records its beats' commands itself.
const NOT_SCRIPTED: &[&str] = &["ward-whole-walk"];

/// Whether `script` gives `replay`'s actions on `cast` and the commands it holds. A skip runs the steps the
/// timeline holds on neutral input (`Recording::skip_timeline`), so from a skip's tick the script's input
/// waits for those, as many as the simulation says.
fn made(
    script: &Script,
    replay: &Replay,
    actions: &[TickActions],
    cast: &[sapphire_gameplay::ActorId],
) -> bool {
    let commands: Vec<_> =
        script.commands.iter().copied().filter(|&(tick, _)| tick < replay.ticks()).collect();
    if commands != replay.commands() {
        return false;
    }
    let mut held = false;
    for (recorded, tick) in actions.iter().zip(0..) {
        held |= commands.iter().any(|&(at, command)| {
            u64::from(at) == tick && matches!(command, ReplayCommand::SkipTimeline { .. })
        });
        held &= *recorded == TickActions::NEUTRAL;
        if !held && script.actions(tick, cast) != *recorded {
            return false;
        }
    }
    true
}

#[test]
fn every_scripted_replay_is_its_documents_input() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays");
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .expect("tests/replays")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sreplay"))
        .collect();
    paths.sort();
    assert!(paths.len() > 30, "the suite is there: {}", paths.len());
    let mut unmade = Vec::new();
    for path in &paths {
        let name = path.file_stem().and_then(|stem| stem.to_str()).expect("a name").to_owned();
        let bytes = fs::read(path).expect("it reads");
        let replay = Replay::decode(&bytes).expect("it decodes");
        let player = Player::new(Replay::decode(&bytes).expect("it decodes"), &ALL).expect("it starts");
        let actions: Vec<TickActions> = replay.actions().collect();
        if !SCRIPTS.iter().any(|script| made(script, &replay, &actions, player.cast())) {
            unmade.push(name);
        }
    }
    assert_eq!(unmade, NOT_SCRIPTED, "replays no document makes");
}
