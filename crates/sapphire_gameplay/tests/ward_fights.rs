//! The Lantern Ward's own fights (M3 C2), through their recorded replays: the market's encounter brings
//! Lancers beside the warriors, who fight. Everyone comes from the zone's documents and the kinds' own.

use sapphire_core::Name;
use sapphire_gameplay::replay::{Player, Replay, SCENARIOS, find_scenario};
use sapphire_gameplay::{ActorId, MoveSetId};

/// The longest a replay here may play, so a broken one fails rather than hangs.
const MOST_STEPS: u32 = 10_000;

/// What a replay did: who stood there, on which set, and each step's cues, by whom.
struct Played {
    seen: Vec<(ActorId, Option<MoveSetId>)>,
    cues: Vec<(u64, ActorId, Name)>,
}

fn play(bytes: &[u8]) -> (Player, Played) {
    let mut player =
        Player::new(Replay::decode(bytes).expect("reads"), &SCENARIOS).expect("recorded on these");
    let (mut seen, mut cues) = (Vec::new(), Vec::new());
    let mut steps = 0;
    while player.step() {
        steps += 1;
        assert!(steps <= MOST_STEPS, "it ends");
        let sim = player.simulation();
        for actor in sim.actors() {
            if !seen.iter().any(|(id, _)| *id == actor.id) {
                seen.push((actor.id, actor.moves.set));
            }
        }
        let tick = sim.tick();
        cues.extend(sim.cues().map(|cue| (tick, cue.actor, cue.cue)));
    }
    (player, Played { seen, cues })
}

impl Played {
    fn on(&self, set: MoveSetId) -> Vec<ActorId> {
        self.seen.iter().filter(|(_, on)| *on == Some(set)).map(|(id, _)| *id).collect()
    }

    fn said(&self, by: ActorId, cue: &str) -> Vec<u64> {
        let cue = Name::new(cue);
        self.cues
            .iter()
            .filter(|(_, actor, said)| *actor == by && *said == cue)
            .map(|(tick, ..)| *tick)
            .collect()
    }
}

#[test]
fn the_market_brings_lancers_beside_warriors_and_they_fight() {
    let scenario = find_scenario(&SCENARIOS, "ward-market").expect("listed");
    let lancer = scenario.set_of("lancer.moves.ron").expect("it loads the Lancer");
    let warrior = scenario.set_of("warrior.moves.ron").expect("and the warrior");
    let (player, played) = play(include_bytes!("../../../tests/replays/ward-market.sreplay"));
    let library = player.simulation().move_library();
    assert!(library.mind(lancer).is_some_and(|mind| mind.boss().is_none()), "a kind of enemy, and no boss");

    let (lancers, warriors) = (played.on(lancer), played.on(warrior));
    assert_eq!((lancers.len(), warriors.len()), (2, 2), "both waves came, a Lancer and a warrior in each");
    for lancer in &lancers {
        assert!(!played.said(*lancer, "swing").is_empty(), "{lancer:?} swung");
    }
    // Its gap closer: the spear down on landing, a few steps after it leaves the ground.
    assert!(!played.said(lancers[0], "lunge_land").is_empty(), "it vaulted in");
    // Its poise is half a warrior's, so the player's strings break it.
    assert!(!played.said(lancers[0], "stagger_heavy").is_empty(), "it was broken");
}
