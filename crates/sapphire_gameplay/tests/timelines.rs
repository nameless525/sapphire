//! M3's exit test 3, in the simulation: a skip lands on exactly the state watching reached, wherever it
//! came, and the fight that follows stays on the same checksums. `sapphire_sequencer`'s own test does the
//! same with a player in front of it, skipped at every frame.

use sapphire_gameplay::replay::{
    CINEMATIC, CUTSCENE, FORMAT_VERSION, PLAIN_FORMAT_VERSION, Player, Recording, Replay, ReplayError,
    SCENARIOS, Scenario, arena_fight, find_scenario, find_script, melee_sweep,
};
use sapphire_gameplay::{ActorId, Simulation, TickActions, TimelineCommand, TimelineId, TimelinePlayError};
use sapphire_timeline::{Beat, BeatKind, TimelineDocument, TimelineSimulation};

const CINEMATIC_DOCUMENT: &str = include_str!("../../sapphire_timeline/test_content/cinematic.timeline.ron");

/// When the tests below start a timeline, after enough of a fight that things are going on.
const PLAY_AT: u32 = 120;
/// How long the fight runs past the cinematic's end, for a difference to show if there were one.
const AFTER: u32 = 120;
/// How long `CINEMATIC_DOCUMENT` holds input.
const CINEMATIC_TICKS: u32 = 150;

fn timelines() -> &'static Scenario {
    find_scenario(SCENARIOS, "timelines").expect("listed")
}

fn recording() -> Recording {
    Recording::new(timelines(), timelines().config).expect("sets up")
}

fn run_to(recording: &mut Recording, ticks: u32) {
    let mut source = melee_sweep;
    while recording.recorded_ticks() < ticks {
        recording.step(&mut source).expect("short");
    }
}

/// The cinematic, watched to its end with buttons mashed all the way through.
fn watched() -> (Replay, Vec<u64>) {
    let mut recording = recording();
    run_to(&mut recording, PLAY_AT);
    recording.play_timeline(CINEMATIC).expect("loaded");
    run_to(&mut recording, PLAY_AT + CINEMATIC_TICKS + AFTER);
    let (replay, played) = recording.finish();
    (replay, trail_of(&played.trail))
}

fn trail_of(trail: &sapphire_gameplay::replay::Trail) -> Vec<u64> {
    (0..=trail.ticks()).filter_map(|tick| trail.at(tick)).collect()
}

/// **The crux.** Skipped after each of the steps it could be, the cinematic reaches every checksum
/// watching reached: at its end, and at every tick of the fight after it. And the fight under it is
/// really going, with time scale and events firing, so there's something to get wrong.
#[test]
fn a_skip_at_every_step_of_a_cinematic_reaches_the_checksums_of_watching_it() {
    let (_, watch) = watched();
    for skip_after in 0..=CINEMATIC_TICKS {
        let mut recording = recording();
        run_to(&mut recording, PLAY_AT);
        recording.play_timeline(CINEMATIC).expect("loaded");
        run_to(&mut recording, PLAY_AT + skip_after);
        recording.skip_timeline(skip_after).expect("short");
        assert!(recording.simulation().timeline().is_none(), "a skip leaves nothing playing");
        assert_eq!(recording.recorded_ticks(), PLAY_AT + CINEMATIC_TICKS.max(skip_after));
        run_to(&mut recording, PLAY_AT + CINEMATIC_TICKS + AFTER);
        let (replay, played) = recording.finish();
        assert_eq!(trail_of(&played.trail), watch, "skipped {skip_after} steps in");

        // Its own file plays back to it, through its bytes.
        let bytes = replay.encode();
        let mut player = Player::new(Replay::decode(&bytes).expect("reads"), SCENARIOS).expect("starts");
        player.run_to_end();
        assert_eq!(trail_of(player.trail()), watch, "skipped {skip_after} steps in, played back");
    }
}

/// The test above would pass if a timeline did nothing. So: the same cinematic with any one thing
/// different ends elsewhere, and running one at all moves the fight.
#[test]
fn what_a_cinematic_fires_really_reaches_the_checksum() {
    let (_, watch) = watched();

    // No cinematic at all.
    let mut plain = recording();
    run_to(&mut plain, PLAY_AT + CINEMATIC_TICKS + AFTER);
    assert_ne!(trail_of(&plain.finish().1.trail), watch);

    let run = |change: fn(&mut TimelineSimulation)| {
        let scenario = timelines();
        let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
        let mut timeline = cinematic();
        change(&mut timeline);
        let id = sim.add_timeline(timeline).expect("loads");
        run_sim(&mut sim, &cast, id, None)
    };
    let good = run(|_| {});
    let is_event = |beat: &Beat| matches!(beat.what, BeatKind::Event { .. });
    // A skip that dropped what it jumped over, or got it wrong, would look like one of these.
    let lost_event: fn(&mut TimelineSimulation) = |timeline| {
        let at = timeline.beats.iter().position(|beat| matches!(beat.what, BeatKind::Event { .. })).unwrap();
        timeline.beats.remove(at);
    };
    let lost_scale: fn(&mut TimelineSimulation) = |timeline| {
        timeline.beats.retain(|beat| matches!(beat.what, BeatKind::Event { .. }));
    };
    let late_event: fn(&mut TimelineSimulation) = |timeline| {
        let at = timeline.beats.iter().rposition(|beat| matches!(beat.what, BeatKind::Event { .. })).unwrap();
        timeline.beats[at].tick += 1;
    };
    assert!(cinematic().beats.iter().any(is_event));
    for (what, change) in [("an event", lost_event), ("a scale", lost_scale), ("a late event", late_event)] {
        assert_ne!(run(change), good, "{what}");
    }
}

fn cinematic() -> TimelineSimulation {
    let document = TimelineDocument::from_ron(CINEMATIC_DOCUMENT).expect("reads");
    document.cook().expect("cooks").simulation
}

/// Plays `id` from [`PLAY_AT`] on a plain simulation, skipping `skip_after` steps in if asked, and gives the
/// checksums of every tick.
fn run_sim(sim: &mut Simulation, cast: &[ActorId], id: TimelineId, skip_after: Option<u32>) -> Vec<u64> {
    let mut trail = Vec::new();
    let mut step = |sim: &mut Simulation, actions: &TickActions| {
        sim.step(actions);
        trail.push(sim.checksum());
    };
    let ticks = PLAY_AT + CINEMATIC_TICKS + AFTER;
    let mut tick = 0;
    while tick < ticks {
        if tick == PLAY_AT {
            sim.play_timeline(id).expect("loaded");
        }
        if Some(tick) == skip_after.map(|steps| PLAY_AT + steps) {
            for _ in 0..sim.timeline_steps_held() {
                step(sim, &TickActions::NEUTRAL);
                tick += 1;
            }
        }
        step(sim, &melee_sweep(u64::from(tick), cast));
        tick += 1;
    }
    trail
}

/// The same crux on the real fight: the warrior's brain goes on thinking and swinging under a cinematic,
/// and the real moves, with nobody's input reaching them.
#[test]
fn a_skip_over_a_real_fight_reaches_the_checksums_of_watching_it() {
    let run = |skip_after| {
        let scenario = find_scenario(SCENARIOS, "arena-fight").expect("listed");
        let mut sim = Simulation::new(scenario.config);
        let cast = arena_fight(&mut sim).expect("sets up");
        let id = sim.add_timeline(cinematic()).expect("loads");
        run_sim(&mut sim, &cast, id, skip_after)
    };
    let watch = run(None);
    for skip_after in (0..=CINEMATIC_TICKS).step_by(11) {
        assert_eq!(run(Some(skip_after)), watch, "skipped {skip_after} steps in");
    }
}

/// A cutscene takes no steps, so it's the same one command whenever anyone skipped: it's in the file, for
/// a replay shown beside a renderer, and nothing in the simulation reads it.
#[test]
fn a_cutscene_has_the_same_checksums_watched_or_skipped_and_fires_its_events_on_the_next_step() {
    let run = |skip| {
        let mut recording = recording();
        run_to(&mut recording, PLAY_AT);
        recording.play_timeline(CUTSCENE).expect("loaded");
        if let Some(at_ms) = skip {
            recording.skip_timeline(at_ms).expect("short");
        }
        assert_eq!(recording.recorded_ticks(), PLAY_AT, "it took no steps");
        run_to(&mut recording, PLAY_AT + 1);
        assert_eq!(recording.simulation().timeline_events().count(), 2);
        run_to(&mut recording, PLAY_AT + AFTER);
        trail_of(&recording.finish().1.trail)
    };
    let watch = run(None);
    for at_ms in [0, 1, 2500, 5999, 6000] {
        assert_eq!(run(Some(at_ms)), watch);
    }
    let mut plain = recording();
    run_to(&mut plain, PLAY_AT + AFTER);
    assert_ne!(trail_of(&plain.finish().1.trail), watch, "a cutscene's events are in the checksum");
}

/// The committed scripts' pairs: what `cargo xtask replay` holds to committed checksums, each pair to one
/// trail.
#[test]
fn the_scripts_pairs_reach_one_trail() {
    let trail = |name| {
        let script = find_script(name).expect("listed");
        let mut recording = recording();
        recording.run_script(script, script.ticks).expect("runs");
        let (replay, played) = recording.finish();
        played.allocation_gate().expect("no allocation");
        (replay.commands().len(), trail_of(&played.trail))
    };
    for (watched, skipped) in
        [("cinematic-watched", "cinematic-skipped"), ("cutscene-watched", "cutscene-skipped")]
    {
        let (watched, skipped) = (trail(watched), trail(skipped));
        assert_eq!((watched.0, skipped.0), (1, 2));
        assert_eq!(watched.1, skipped.1);
    }
}

#[test]
fn a_file_of_commands_is_in_its_own_version_and_round_trips() {
    let (replay, _) = watched();
    assert_eq!(replay.commands(), [(PLAY_AT, TimelineCommand::Play(CINEMATIC))]);
    let bytes = replay.encode();
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), FORMAT_VERSION);
    let decoded = Replay::decode(&bytes).expect("reads");
    assert_eq!(decoded, replay);
    assert_eq!(decoded.encode(), bytes, "one run, one encoding");
}

#[test]
fn a_file_without_commands_keeps_the_bytes_it_always_had() {
    let mut recording = recording();
    run_to(&mut recording, 10);
    let bytes = recording.finish().0.encode();
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), PLAIN_FORMAT_VERSION);
    assert!(Replay::decode(&bytes).expect("reads").commands().is_empty());
}

/// Offset of the commands' section in a `timelines` file, and how to reseal one.
fn commands_at() -> usize {
    8 + 4 + 4 + 8 + 4 + 8 + 8 + 4 + 1 + "timelines".len()
}

fn reseal(mut bytes: Vec<u8>) -> Vec<u8> {
    let end = bytes.len() - 8;
    let digest = xxhash_rust::xxh3::xxh3_64(&bytes[..end]);
    bytes[end..].copy_from_slice(&digest.to_le_bytes());
    bytes
}

fn command_file(ticks: u32) -> Vec<u8> {
    let mut recording = recording();
    recording.play_timeline(CINEMATIC).expect("loaded");
    run_to(&mut recording, ticks);
    recording.skip_timeline(5).expect("short");
    recording.finish().0.encode()
}

/// What's wrong with a file, how to make it so, and what's said of it.
type Crafted = (&'static str, fn(&mut Vec<u8>, usize), fn(&ReplayError) -> bool);

#[test]
fn crafted_commands_are_refused_by_name() {
    let good = command_file(200);
    let at = commands_at();
    assert_eq!(u32::from_le_bytes(good[at..at + 4].try_into().unwrap()), 2, "two commands");
    assert!(Replay::decode(&good).is_ok());

    let cases: &[Crafted] = &[
        (
            "no commands, in the version that has them",
            |bytes, at| {
                bytes[at..at + 4].fill(0);
            },
            |e| matches!(e, ReplayError::EmptyCommands),
        ),
        (
            "more than there's room for",
            |bytes, at| {
                bytes[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            },
            |e| matches!(e, ReplayError::Truncated { .. }),
        ),
        (
            "a kind nobody knows",
            |bytes, at| bytes[at + 4 + 4] = 9,
            |e| matches!(e, ReplayError::BadCommand { index: 0, kind: 9 }),
        ),
        (
            "out of order",
            |bytes, at| {
                let earlier = (at + 4, at + 4 + 9);
                bytes[earlier.0..earlier.0 + 4].copy_from_slice(&50_u32.to_le_bytes());
                bytes[earlier.1..earlier.1 + 4].copy_from_slice(&40_u32.to_le_bytes());
            },
            |e| matches!(e, ReplayError::CommandOutOfOrder { index: 1, tick: 40, .. }),
        ),
        (
            "past the file's end",
            |bytes, at| {
                let second = at + 4 + 9;
                bytes[second..second + 4].copy_from_slice(&100_000_u32.to_le_bytes());
            },
            |e| matches!(e, ReplayError::CommandOutOfOrder { index: 1, tick: 100_000, .. }),
        ),
    ];
    for &(what, tamper, is_right) in cases {
        let mut bytes = good.clone();
        tamper(&mut bytes, at);
        let error = Replay::decode(&reseal(bytes)).expect_err(what);
        assert!(is_right(&error), "{what}: {error}");
    }
}

/// A file playing a timeline this build hasn't got can't reach a checksum, and doesn't crash.
#[test]
fn a_crafted_play_of_nothing_is_ignored() {
    let good = command_file(200);
    let mut bytes = good.clone();
    let argument = commands_at() + 4 + 4 + 1;
    bytes[argument..argument + 4].copy_from_slice(&77_u32.to_le_bytes());
    let decoded = Replay::decode(&reseal(bytes)).expect("well formed");
    let mut player = Player::new(decoded, SCENARIOS).expect("starts");
    player.run_to_end();
    assert!(player.simulation().timeline().is_none());
}

#[test]
fn a_recording_refuses_what_a_simulation_does_and_records_nothing_for_it() {
    let mut recording = recording();
    let nothing = TimelineId::from_raw(9);
    assert_eq!(recording.play_timeline(nothing), Err(TimelinePlayError::Unknown(nothing)));
    recording.play_timeline(CINEMATIC).expect("loaded");
    assert_eq!(recording.play_timeline(CUTSCENE), Err(TimelinePlayError::Busy(CINEMATIC)));
    run_to(&mut recording, 1);
    assert_eq!(recording.finish().0.commands(), [(0, TimelineCommand::Play(CINEMATIC))]);
}

/// A changed cinematic is changed tables, by name, like a changed move: never a broken simulation.
#[test]
fn a_replay_refuses_a_timeline_that_changed() {
    let (replay, _) = watched();
    let mut stale = replay;
    stale.set_tables_hash(stale.tables_hash() ^ 1);
    let error = Player::new(stale, SCENARIOS).err().expect("refused");
    assert!(matches!(error, sapphire_gameplay::replay::PlayError::MovesChanged { .. }), "{error}");
}
