//! The benchmark district's fight (M5 B5), as `tests/replays/district-encounter.sreplay` plays it headless on the
//! runner's scenarios, which hand the district's builder over: exit test 1's walk into an encounter of twelve.
//! The party of three kinds walks up the avenue into the plaza, the wave of twelve Ember Lancers spawns once,
//! each kind takes the field while all twelve stand, and the twelve are fought, to the recorded end.

use std::path::Path;

use sapphire_gameplay::replay::{Player, Replay};
use sapphire_gameplay::{EncounterPhase, Faction};

/// What the run did, tick by tick.
#[derive(Debug, Default)]
struct Seen {
    /// The tick the plaza's trigger started the fight.
    started: Option<u64>,
    /// Where the party's lead stood when it did.
    lead_z: f32,
    /// The most enemies standing at once, and the first tick they did.
    densest: (usize, u64),
    /// Each member's ticks on the field, and of those, the ticks all twelve stood.
    on_field: [u64; 3],
    against_twelve: [u64; 3],
    /// Enemies down at the end, and the health they lost all told; members down at the end.
    down: usize,
    lost: u64,
    members_down: usize,
}

#[test]
fn the_party_walks_into_the_plaza_and_fights_the_twelve() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/district-encounter.sreplay");
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
        let zone = sim.zone().expect("in the district");
        if seen.started.is_none() && zone.encounters()[0].phase() != EncounterPhase::Waiting {
            seen.started = Some(tick);
            let lead =
                sim.actors().find(|actor| !actor.off_field && cast.contains(&actor.id)).expect("a lead");
            seen.lead_z = lead.position.z;
        }
        let standing = sim
            .actors()
            .filter(|actor| actor.fighter.faction() != party && !actor.fighter.is_defeated())
            .count();
        if standing > seen.densest.0 {
            seen.densest = (standing, tick);
        }
        let active = sim.party().active().expect("formed");
        let slot = cast.iter().position(|&member| member == active).expect("a member");
        seen.on_field[slot] += 1;
        seen.against_twelve[slot] += u64::from(standing == 12);
    }
    let sim = player.simulation();
    for enemy in sim.actors().filter(|actor| actor.fighter.faction() != party) {
        seen.down += usize::from(enemy.fighter.is_defeated());
        seen.lost += u64::from(enemy.fighter.max_health() - enemy.fighter.health());
    }
    for member in sim.actors().filter(|actor| cast.contains(&actor.id)) {
        seen.members_down += usize::from(member.fighter.is_defeated());
    }
    let stats = sim.zone().expect("in the district").stats();
    assert_eq!((stats.encounters_started, stats.spawned, stats.spawns_refused), (1, 12, 0), "{seen:?}");
    assert!(seen.started.is_some_and(|tick| tick < 420), "{seen:?}");
    assert!(seen.lead_z > 20.0 && seen.lead_z < 30.0, "at the plaza's edge: {seen:?}");
    assert_eq!(seen.densest.0, 12, "the whole wave stands at once: {seen:?}");
    // Each kind fights all twelve: the heroine, Aster and Wren each take the field while the whole wave stands,
    // and none falls before the end, so the one on the field is always a fighter standing.
    assert!(seen.against_twelve.iter().all(|&ticks| ticks >= 250), "each kind fights the twelve: {seen:?}");
    assert_eq!(seen.members_down, 0, "the party stands to the end: {seen:?}");
    assert!(seen.lost > 0, "the twelve are fought: {seen:?}");
    assert!(sim.party().swaps() >= 4, "{seen:?}");
}
