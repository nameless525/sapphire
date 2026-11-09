//! The Lantern Ward's own fights (M3 C2), through their recorded replays: the market's encounter brings
//! Lancers beside the warriors, who fight, and the terrace's wakes its boss, which the player takes past its
//! second wind. Everyone comes from the zone's documents and the kinds' own, the boss's second wind from the
//! script its kind's document names, and the boss is found by being one, so nothing here names it. Here
//! rather than in gameplay because a scenario whose kinds name a script plays with it bound ([`ALL`]).

use sapphire_core::Name;
use sapphire_gameplay::replay::{Player, Replay, find_scenario};
use sapphire_gameplay::{ActorId, MoveSetId};
use sapphire_script::scenarios::ALL;

/// The longest a replay here may play, so a broken one fails rather than hangs.
const MOST_STEPS: u32 = 10_000;

/// What a replay did: who stood there, on which set, and each step's cues, by whom.
struct Played {
    seen: Vec<(ActorId, Option<MoveSetId>)>,
    cues: Vec<(u64, ActorId, Name)>,
    /// When an actor's brain changed, and to which of its kind's brains.
    minds: Vec<(u64, ActorId, u8)>,
    /// Each step's running move, by whom, when it isn't the one before.
    moves: Vec<(u64, ActorId, Option<Name>)>,
}

fn play(bytes: &[u8]) -> (Player, Played) {
    let mut player = Player::new(Replay::decode(bytes).expect("reads"), &ALL).expect("recorded on these");
    let (mut seen, mut cues, mut minds) = (Vec::new(), Vec::new(), Vec::<(u64, ActorId, u8)>::new());
    let mut moves = Vec::<(u64, ActorId, Option<Name>)>::new();
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
        for actor in sim.actors().filter(|actor| actor.brain.is_on()) {
            let last = minds.iter().rev().find(|(_, id, _)| *id == actor.id).map(|(_, _, mind)| *mind);
            if last != Some(actor.brain.mind()) {
                minds.push((tick, actor.id, actor.brain.mind()));
            }
        }
        for actor in sim.actors() {
            let running = actor.moves.current.map(|current| current.name);
            let last = moves.iter().rev().find(|(_, id, _)| *id == actor.id).map(|(_, _, name)| *name);
            if last.is_none_or(|last| last != running) {
                moves.push((tick, actor.id, running));
            }
        }
    }
    (player, Played { seen, cues, minds, moves })
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
    let scenario = find_scenario(&ALL, "ward-market").expect("listed");
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

#[test]
fn the_terrace_boss_takes_its_second_wind_from_its_script_once() {
    let scenario = find_scenario(&ALL, "ward-terrace").expect("listed");
    let scripts = scenario.document().expect("a document").scripts().expect("it can say");
    assert_eq!(scripts.len(), 1, "its boss's kind names a script, which the fight binds");
    let (player, played) = play(include_bytes!("../../../tests/replays/ward-terrace.sreplay"));
    let library = player.simulation().move_library();
    let boss = played.seen.iter().find(|(_, set)| set.and_then(|set| library.mind(set)?.boss()).is_some());
    let &(boss, set) = boss.expect("the terrace's encounter woke a boss");
    let mind = library.mind(set.expect("a set")).expect("a kind of enemy");
    let tolling = mind.brain_named(Name::new("tolling")).expect("its second brain");
    assert!(
        mind.phases().iter().all(|phase| phase.brain.is_none()),
        "no phase changes its brain: its script does"
    );

    let crossed = played.said(boss, "tolling");
    assert_eq!(crossed.len(), 1, "it crossed half its health once: {crossed:?}");
    // The script's answer: its fiercer brain on the crossing's own step, since a hook's commands take effect
    // at the end of the step that fired it, and the bell's toll on the next.
    let changed: Vec<_> = played.minds.iter().filter(|(_, id, _)| *id == boss).skip(1).collect();
    assert_eq!(changed.len(), 1, "one change of mind: {changed:?}");
    assert_eq!((changed[0].0, changed[0].2), (crossed[0], tolling), "{changed:?} after {crossed:?}");
    // The toll starts once, at once, and plays out: its ring lands while it runs.
    let toll = Name::new("phase_shift");
    let runs: Vec<_> = played.moves.iter().filter(|(_, id, _)| *id == boss).collect();
    let tolls: Vec<usize> = (0..runs.len()).filter(|&at| runs[at].2 == Some(toll)).collect();
    assert_eq!(tolls.len(), 1, "it tolled once: {runs:?}");
    let (started, ended) = (runs[tolls[0]].0, runs.get(tolls[0] + 1).map_or(u64::MAX, |run| run.0));
    assert_eq!(started, crossed[0] + 1, "the toll on the step after the crossing");
    assert!(
        played.said(boss, "heavy_land").iter().any(|at| (started..ended).contains(at)),
        "its ring landed before it ended, at {ended}"
    );
}
