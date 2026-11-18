//! The benchmark nature zone's fight (M6 W1), as `tests/replays/nature-encounter.sreplay` plays it headless on
//! the runner's scenarios, which hand the nature zone's builder over: `bench/scale.ron`'s `nature-fight`. The
//! party of three kinds steps into the clearing, its one wave of six Ember Lancers spawns once, and the six are
//! fought with each kind taking the field, to the recorded end.

use std::path::Path;

use sapphire_gameplay::replay::{Player, Replay};
use sapphire_gameplay::{EncounterPhase, Faction};

/// What the run did, tick by tick.
#[derive(Debug, Default)]
struct Seen {
    /// The tick the clearing's trigger started the fight.
    started: Option<u64>,
    /// The most enemies standing at once.
    densest: usize,
    /// Each member's ticks on the field while an enemy stood.
    fighting: [u64; 3],
    /// Enemies down at the end, and the health they lost all told; members down at the end.
    down: usize,
    lost: u64,
    members_down: usize,
}

#[test]
fn the_party_steps_into_the_clearing_and_fights_the_six() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/nature-encounter.sreplay");
    let replay = Replay::decode(&std::fs::read(path).expect("committed")).expect("it decodes");
    let mut player = Player::new(replay, &sapphire_replay::SCENARIOS).expect("it starts");
    let cast: Vec<_> = player.simulation().party().slots().map(|slot| slot.actor).collect();
    assert_eq!(cast.len(), 3, "a party of three");
    let party = Faction::new(1).expect("a side");
    let mut seen = Seen::default();
    let mut tick = 0;
    while player.step() {
        tick += 1;
        let sim = player.simulation();
        let zone = sim.zone().expect("in the nature zone");
        if seen.started.is_none() && zone.encounters()[0].phase() != EncounterPhase::Waiting {
            seen.started = Some(tick);
        }
        let standing = sim
            .actors()
            .filter(|actor| actor.fighter.faction() != party && !actor.fighter.is_defeated())
            .count();
        seen.densest = seen.densest.max(standing);
        let active = sim.party().active().expect("formed");
        let slot = cast.iter().position(|&member| member == active).expect("a member");
        seen.fighting[slot] += u64::from(standing > 0);
    }
    let sim = player.simulation();
    for enemy in sim.actors().filter(|actor| actor.fighter.faction() != party) {
        seen.down += usize::from(enemy.fighter.is_defeated());
        seen.lost += u64::from(enemy.fighter.max_health() - enemy.fighter.health());
    }
    for member in sim.actors().filter(|actor| cast.contains(&actor.id)) {
        seen.members_down += usize::from(member.fighter.is_defeated());
    }
    let stats = sim.zone().expect("in the nature zone").stats();
    assert_eq!((stats.encounters_started, stats.spawned, stats.spawns_refused), (1, 6, 0), "{seen:?}");
    assert_eq!(seen.started, Some(45), "the clearing's trigger: {seen:?}");
    assert_eq!(seen.densest, 6, "the whole wave stands at once: {seen:?}");
    assert!(seen.fighting.iter().all(|&ticks| ticks >= 420), "each kind fights the six: {seen:?}");
    assert!(seen.lost > 0, "the six are fought: {seen:?}");
    assert!(sim.party().swaps() >= 4, "{seen:?}");
}
