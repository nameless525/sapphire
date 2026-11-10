//! A run that starts from a save's place in its zone (M3 G1): its file holds the start, in format 3, and
//! plays back from there; a run that starts fresh keeps the bytes it always had.

use sapphire_gameplay::ZoneStart;
use sapphire_gameplay::replay::{
    COMMANDS_FORMAT_VERSION, FORMAT_VERSION, LiveMoves, PLAIN_FORMAT_VERSION, PlayError, Player, Recording,
    Replay, ReplayError, RunStart, SCENARIOS, Scenario, find_scenario, find_script,
};

fn scenario(name: &str) -> &'static Scenario {
    find_scenario(&SCENARIOS, name).expect("listed")
}

fn version(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[8..12].try_into().expect("four bytes"))
}

/// The cart yard, continued at the platform with its one encounter cleared.
fn saved() -> ZoneStart {
    let (sim, _) = scenario("cart-yard").start(scenario("cart-yard").config).expect("sets up");
    let cleared = vec![sim.zone().expect("a zone").encounters()[0].id()];
    ZoneStart { checkpoint: Some("platform".to_owned()), cleared }
}

fn record(start: &RunStart, ticks: u32) -> (Replay, u64) {
    let cart_yard = scenario("cart-yard");
    let mut recording =
        Recording::starting_from(cart_yard, cart_yard.config, &LiveMoves::default(), start, |_, _, _| Ok(()))
            .expect("starts");
    let mut source = find_script("from-the-platform").expect("listed");
    for _ in 0..ticks {
        recording.step(&mut source).expect("records");
    }
    let end = recording.simulation().checksum();
    (recording.finish().0, end)
}

#[test]
fn a_run_from_a_save_starts_there_and_its_file_says_so() {
    let start = RunStart { zone: Some(saved()), flags: Vec::new() };
    let cart_yard = scenario("cart-yard");
    let recording = Recording::starting_from(
        cart_yard,
        cart_yard.config,
        &LiveMoves::default(),
        &start,
        |_, _, _| Ok(()),
    )
    .expect("starts");
    let sim = recording.simulation();
    let zone = sim.zone().expect("a zone");
    assert_eq!(
        sim.zone_start().as_ref(),
        start.zone.as_ref(),
        "a save written at once is the one it came from"
    );
    let platform = zone.checkpoint("platform").expect("the ground's");
    let player = sim.actor(recording.cast()[0]).expect("standing");
    assert_eq!(player.position, zone.checkpoint_place(platform).expect("a place").position);

    let (replay, end) = record(&start, 120);
    let bytes = replay.encode();
    assert_eq!(version(&bytes), FORMAT_VERSION);
    let decoded = Replay::decode(&bytes).expect("reads");
    assert_eq!(decoded.zone_start(), start.zone.as_ref());
    assert_eq!(decoded.encode(), bytes, "one encoding");
    let mut player = Player::new(decoded, &SCENARIOS).expect("starts where it was recorded");
    player.run_to_end();
    assert_eq!(player.simulation().checksum(), end, "and plays back to where the run got");
}

#[test]
fn a_fresh_run_keeps_the_bytes_it_had() {
    let (replay, _) = record(&RunStart::default(), 30);
    assert_eq!(replay.zone_start(), None);
    assert_eq!(version(&replay.encode()), PLAIN_FORMAT_VERSION);
    assert_eq!(COMMANDS_FORMAT_VERSION, 2, "committed files are in it");
}

#[test]
fn a_start_a_file_cant_hold_or_a_scenario_cant_take_is_refused() {
    let nameless = RunStart {
        zone: Some(ZoneStart { checkpoint: Some(String::new()), cleared: Vec::new() }),
        flags: Vec::new(),
    };
    let cart_yard = scenario("cart-yard");
    let refused =
        Recording::starting_from(cart_yard, cart_yard.config, &LiveMoves::default(), &nameless, |_, _, _| {
            Ok(())
        });
    assert!(matches!(refused, Err(PlayError::File(ReplayError::BadStart { .. }))));

    let elsewhere = RunStart {
        zone: Some(ZoneStart { checkpoint: Some("nowhere".to_owned()), cleared: Vec::new() }),
        flags: Vec::new(),
    };
    let refused = Recording::starting_from(
        cart_yard,
        cart_yard.config,
        &LiveMoves::default(),
        &elsewhere,
        |_, _, _| Ok(()),
    );
    assert!(matches!(refused, Err(PlayError::Setup { .. })), "no such checkpoint");

    let arena = scenario("arena-fight");
    let start = RunStart { zone: Some(saved()), flags: Vec::new() };
    let refused =
        Recording::starting_from(arena, arena.config, &LiveMoves::default(), &start, |_, _, _| Ok(()));
    assert!(matches!(refused, Err(PlayError::Setup { .. })), "a scenario with no zone has no place in one");
}
