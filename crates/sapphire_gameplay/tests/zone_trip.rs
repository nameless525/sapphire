//! What a change of zone does to the simulation (M3 W4), and that a replay across one reproduces.

use sapphire_core::Name;
use sapphire_gameplay::replay::{
    BAKED_ZONES, GROUND_START, HUB_FROM_GROUND, HUB_START, PROVING_GROUND, PROVING_HUB, Player,
    RecordedTrail, Recording, Replay, ReplayCommand, SCENARIOS, Scenario, ZONE_TRIP_ENTRIES, find_scenario,
    find_script,
};
use sapphire_gameplay::{ActorId, Simulation, TickActions, ZoneEntry, ZoneEntryError, ZoneId};
use sapphire_math::Vec3;

fn scenario() -> &'static Scenario {
    find_scenario(&SCENARIOS, "zone-trip").expect("listed")
}

fn started() -> (Simulation, Vec<ActorId>) {
    let scenario = scenario();
    scenario.start(scenario.config).expect("it sets up")
}

fn neutral(sim: &mut Simulation, steps: u32) {
    for _ in 0..steps {
        sim.step(&TickActions::NEUTRAL);
    }
}

/// A script names spawners by where they fall among a zone's IDs: this is what says it has them right.
#[test]
fn the_scripts_entries_are_the_spawners_it_means() {
    let (sim, _) = started();
    let index =
        |zone: ZoneId, name: &str| sim.zone_ground(zone).expect("shelved").spawner(Name::new(name).as_u64());
    assert_eq!(index(PROVING_HUB, "player_start"), Some(HUB_START.spawner));
    assert_eq!(index(PROVING_HUB, "from_ground"), Some(HUB_FROM_GROUND.spawner));
    assert_eq!(index(PROVING_GROUND, "player_start"), Some(GROUND_START.spawner));
    assert_eq!(index(PROVING_GROUND, "nobody"), None);
    for (row, baked) in BAKED_ZONES.iter().enumerate() {
        let id = ZoneId::from_raw(row as u16);
        assert_eq!(sim.find_zone(baked.asset), Some(id));
    }
}

#[test]
fn an_entry_swaps_the_world_and_leaves_only_the_traveller_standing() {
    let (mut sim, cast) = started();
    let (player, enemy) = (cast[0], cast[1]);
    neutral(&mut sim, 30);
    let arena_statics = sim.physics().statics_digest();
    let (tick, checksum) = (sim.tick(), sim.checksum());
    assert_eq!(sim.current_zone(), None);

    sim.enter_zone(HUB_FROM_GROUND, player).expect("it can");
    assert_eq!(sim.current_zone(), Some(PROVING_HUB));
    assert_eq!(sim.tick(), tick, "no time passes");
    assert_ne!(sim.checksum(), checksum, "the checksum sees it at once");
    assert_ne!(sim.physics().statics_digest(), arena_statics);
    assert!(sim.actor(enemy).is_none(), "who stayed behind is gone");
    assert_eq!(sim.actors().len(), 1);
    let standing = sim.actor(player).expect("the traveller");
    assert_eq!(standing.position, Vec3::new(0.0, 0.0, 10.0));
    assert_eq!(standing.yaw, 180.0_f32.to_radians());
    assert!(sim.navigation().is_some(), "the hub's navmesh");
    let hub = sim.physics().statics_digest();

    // The hub's ground holds them up, and its post stands in the way.
    neutral(&mut sim, 30);
    let settled = sim.actor(player).expect("the traveller");
    assert_eq!(settled.controller.state.mode, sapphire_gameplay::GroundMode::Grounded);
    assert!(settled.position.y.abs() < 0.01, "{}", settled.position);

    sim.enter_zone(GROUND_START, player).expect("it can");
    let ground = sim.physics().statics_digest();
    assert_ne!(ground, hub);
    sim.enter_zone(HUB_START, player).expect("back");
    assert_eq!(sim.physics().statics_digest(), hub, "the hub's statics went back on its row");
    sim.enter_zone(HUB_START, player).expect("again, where it stands");
    assert_eq!(sim.physics().statics_digest(), hub);
    sim.enter_zone(GROUND_START, player).expect("and on");
    assert_eq!(sim.physics().statics_digest(), ground);
}

#[test]
fn a_traveller_arrives_doing_nothing_with_what_it_had() {
    let (mut sim, cast) = started();
    let player = cast[0];
    let mut actions = TickActions::NEUTRAL;
    let swing = sapphire_gameplay::ActorActions {
        buttons: sapphire_gameplay::Buttons::LIGHT,
        stick: sapphire_gameplay::Stick::CENTER,
    };
    actions.set(player, swing).expect("fits");
    sim.step(&actions);
    neutral(&mut sim, 4);
    let swinging = sim.actor(player).expect("the player");
    assert!(swinging.moves.current.is_some(), "under way");
    let clock = swinging.clock_now;
    let fighter = swinging.fighter;

    sim.enter_zone(HUB_START, player).expect("it can");
    let arrived = sim.actor(player).expect("the player");
    assert!(arrived.moves.current.is_none(), "it stopped");
    assert_eq!(arrived.clock_now, clock, "its clock goes on");
    assert_eq!(arrived.fighter, fighter, "and it's as hurt as it was");
    assert_eq!(arrived.controller.drive, sapphire_gameplay::ControllerDrive::NONE);
    assert_eq!(sim.physics().body_count(), 1, "its capsule alone: its hurtboxes come with its next move");
}

#[test]
fn an_entry_it_cant_make_changes_nothing() {
    let (mut sim, cast) = started();
    let checksum = sim.checksum();
    let nowhere = ZoneEntry { zone: ZoneId::from_raw(9), spawner: 0 };
    assert!(matches!(sim.enter_zone(nowhere, cast[0]), Err(ZoneEntryError::UnknownZone(_))));
    let no_spawner = ZoneEntry { zone: PROVING_HUB, spawner: 9 };
    assert!(matches!(sim.enter_zone(no_spawner, cast[0]), Err(ZoneEntryError::UnknownSpawner { .. })));
    assert!(sim.despawn(cast[1]));
    let checksum_without = sim.checksum();
    assert_ne!(checksum, checksum_without);
    assert!(matches!(sim.enter_zone(HUB_START, cast[1]), Err(ZoneEntryError::NoTraveller(_))));
    assert_eq!(sim.checksum(), checksum_without);
    assert_eq!(sim.current_zone(), None);
}

/// A zone can't go from under whoever's standing on it, and a row that's gone leaves the others theirs.
#[test]
fn the_shelf_gives_back_what_it_cant_reach_and_keeps_its_rows() {
    let (mut sim, cast) = started();
    sim.enter_zone(HUB_START, cast[0]).expect("it can");
    assert!(!sim.remove_zone(PROVING_HUB), "it's standing there");
    assert!(sim.remove_zone(PROVING_GROUND));
    assert!(!sim.remove_zone(PROVING_GROUND), "gone");
    assert!(matches!(sim.enter_zone(GROUND_START, cast[0]), Err(ZoneEntryError::UnknownZone(_))));
    let row = sim.add_zone(BAKED_ZONES[1].load().expect("it reads"));
    assert_eq!(row, PROVING_GROUND, "the first free row");
    sim.enter_zone(GROUND_START, cast[0]).expect("it can");
    assert!(sim.remove_zone(PROVING_HUB), "it left");
    assert!(sim.find_zone(BAKED_ZONES[0].asset).is_none());
}

/// A row's digest is in the tables, so a replay won't start on a zone it didn't record.
#[test]
fn the_shelf_is_in_the_tables_hash_and_an_empty_one_isnt() {
    let plain = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let (plain, _) = plain.start(plain.config).expect("it sets up");
    let (mut sim, _) = started();
    let hash = sim.tables_hash();
    assert_ne!(hash, plain.tables_hash());
    assert!(sim.remove_zone(PROVING_GROUND));
    assert_ne!(sim.tables_hash(), hash);
}

fn record() -> (Replay, RecordedTrail) {
    let scenario = scenario();
    let script = find_script("zone-trip").expect("listed");
    let mut recording = Recording::new(scenario, scenario.config).expect("it starts");
    recording.run_script(script, script.ticks).expect("it records");
    let (replay, played) = recording.finish();
    (replay, RecordedTrail::from_trail(&played.trail))
}

/// The whole trip twice over, and from its file: the same on every tick, with no step allocating.
#[test]
fn a_replay_across_a_trip_reproduces() {
    let (replay, recorded) = record();
    assert_eq!(replay.commands().len(), ZONE_TRIP_ENTRIES.len());
    assert_eq!(
        replay.commands()[0],
        (ZONE_TRIP_ENTRIES[0].0, ReplayCommand::EnterZone(ZONE_TRIP_ENTRIES[0].1))
    );
    let bytes = replay.encode();
    let decoded = Replay::decode(&bytes).expect("it reads");
    assert_eq!(decoded.commands(), replay.commands());
    let mut player = Player::new(decoded, &SCENARIOS).expect("it starts");
    player.run_to_end();
    assert_eq!(player.simulation().current_zone(), Some(PROVING_HUB));
    assert_eq!(recorded.first_difference(&player.finish().trail), None);
}

/// However long a show beside a replay takes to cover a load, what it enters is what a run with none enters,
/// on the same tick, and a tick that hasn't come yet says nothing.
#[test]
fn a_show_can_ask_and_enter_ahead_of_the_step_and_it_changes_nothing() {
    let (replay, recorded) = record();
    let mut player = Player::new(replay, &SCENARIOS).expect("it starts");
    let mut asked = Vec::new();
    loop {
        if let Some(entry) = player.due_zone_entry() {
            asked.push(entry);
            assert_eq!(player.due_zone_entry(), Some(entry), "it says so until it's entered");
            player.enter_due_zone().expect("it can");
            assert_eq!(player.simulation().current_zone(), Some(entry.zone));
        }
        if !player.step() {
            break;
        }
    }
    assert_eq!(asked, ZONE_TRIP_ENTRIES.map(|(_, entry)| entry));
    assert_eq!(recorded.first_difference(&player.finish().trail), None);
}

/// Repeated round trips hold the simulation's bodies and table to what one did.
#[test]
fn a_repeated_trip_holds_the_simulations_counts() {
    let (mut sim, cast) = started();
    sim.enter_zone(HUB_START, cast[0]).expect("it can");
    neutral(&mut sim, 2);
    let counts =
        |sim: &Simulation| (sim.physics().body_count(), sim.physics().static_count(), sim.actors().len());
    let first = counts(&sim);
    for _ in 0..16 {
        sim.enter_zone(GROUND_START, cast[0]).expect("it can");
        neutral(&mut sim, 2);
        sim.enter_zone(HUB_START, cast[0]).expect("it can");
        neutral(&mut sim, 2);
        assert_eq!(counts(&sim), first);
    }
}
