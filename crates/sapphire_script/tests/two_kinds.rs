//! Two kinds of enemy with a script each, in one zone (M3 C2): the Lantern Ward's market, whose encounter
//! brings warriors and Lancers together, fought on its replay's input for long enough that both kinds are
//! hurt, with a script bound for each kind, both in the fight's one VM. Each is the shape a boss's is,
//! phases counting from 1: at its kind's first phase it forces a move of its own set's at once. Each hears
//! its own kind's fighters and nobody else's, and its command lands on the fighter that crossed.

use sapphire_core::Name;
use sapphire_gameplay::replay::{SCENARIOS, Scenario, find_scenario, find_script};
use sapphire_gameplay::{ActorId, MoveSetId, ScriptScope};
use sapphire_script::{Limits, ScriptHost, attach, cook, host_of};

/// The warrior's: it counts what it hears in slot 0 and keeps each in the slots after.
const WARRIOR: &str = r#"
local OVERHEAD = 1
return {
    names = { "overhead" },
    on_phase = function(actor, phase, health)
        local heard = sapphire.board(0) + 1
        sapphire.set_board(0, heard)
        sapphire.set_board(heard, actor * 10 + phase)
        if phase == 1 and health > 0 then
            sapphire.force_move(actor, OVERHEAD)
        end
    end,
}
"#;

/// The Lancer's: the same from slot 32, since the board is the fight's.
const LANCER: &str = r#"
local SWEEP = 1
return {
    names = { "sweep" },
    on_phase = function(actor, phase, health)
        local heard = sapphire.board(32) + 1
        sapphire.set_board(32, heard)
        sapphire.set_board(32 + heard, actor * 10 + phase)
        if phase == 1 and health > 0 then
            sapphire.force_move(actor, SWEEP)
        end
    end,
}
"#;

/// Where each script keeps its count, and the move it forces.
const KEPT: [(usize, &str); 2] = [(0, "overhead"), (32, "sweep")];

/// How long the fight goes on: the recording's 2,400 ticks hurt only a Lancer, and by this the player has
/// fought into both waves.
const TICKS: u64 = 7_200;

fn market() -> &'static Scenario {
    find_scenario(&SCENARIOS, "ward-market").expect("listed")
}

/// The sets the market's warriors and Lancers play, in that order.
fn kinds() -> [MoveSetId; 2] {
    let scenario = market();
    [
        scenario.set_of("warrior.moves.ron").expect("it loads the warrior"),
        scenario.set_of("lancer.moves.ron").expect("and the Lancer"),
    ]
}

#[test]
fn each_kinds_script_answers_its_own_fighters_phases_in_one_zone() {
    // The market as gameplay sets it up, where neither kind's document names a script, then a host with one
    // for each kind.
    let (mut sim, cast) = market().start(market().config).expect("it starts");
    let kinds = kinds();
    let mut host = ScriptHost::new(Limits::default()).expect("the VM starts");
    for (set, (source, file)) in kinds.into_iter().zip([(WARRIOR, "=warrior.luau"), (LANCER, "=lancer.luau")])
    {
        host.bind_for(ScriptScope::Kind(set), &cook(source).expect("cooks"), file).expect("binds");
    }
    attach(&mut sim, host);
    assert!(host_of(&sim).scopes().eq(kinds.map(ScriptScope::Kind)), "a script for each kind, in one host");
    let input = find_script("into-the-market").expect("the market's replay's input");

    // Who played what, what each script had heard by each step's end, and each step's running moves.
    let mut seen: Vec<(ActorId, Option<MoveSetId>)> = Vec::new();
    let mut heard_at: Vec<(u64, usize, usize)> = Vec::new();
    let mut running: Vec<(u64, ActorId, Option<Name>)> = Vec::new();
    while sim.tick() < TICKS {
        sim.step(&input.actions(sim.tick(), &cast));
        let tick = sim.tick();
        for actor in sim.actors() {
            if !seen.iter().any(|(id, _)| *id == actor.id) {
                seen.push((actor.id, actor.moves.set));
            }
            running.push((tick, actor.id, actor.moves.current.map(|current| current.name)));
        }
        let board = sim.scripts().expect("attached").board();
        for (kind, (count, _)) in KEPT.iter().enumerate() {
            let heard = board[*count] as usize;
            let before = heard_at.iter().filter(|(_, of, _)| *of == kind).count();
            heard_at.extend((before..heard).map(|place| (tick, kind, place + 1)));
        }
    }

    let board = *sim.scripts().expect("attached").board();
    for (kind, (&set, (count, forced))) in kinds.iter().zip(KEPT).enumerate() {
        let heard: Vec<(u64, ActorId, u8)> = heard_at
            .iter()
            .filter(|(_, of, _)| *of == kind)
            .map(|&(tick, _, place)| {
                let entry = board[count + place] as u32;
                (tick, ActorId::from_raw(entry / 10).expect("an actor"), (entry % 10) as u8)
            })
            .collect();
        let on_set: Vec<ActorId> =
            seen.iter().filter(|(_, played)| *played == Some(set)).map(|(id, _)| *id).collect();
        assert!(!heard.is_empty(), "{forced}'s kind's script heard a phase");
        for &(tick, actor, phase) in &heard {
            assert!(on_set.contains(&actor), "{forced}'s script heard {actor:?}, of another kind, at {tick}");
            assert!((1..=4).contains(&phase), "phases count from 1: {phase}");
        }
        let firsts: Vec<_> = heard.iter().filter(|(_, _, phase)| *phase == 1).collect();
        assert!(!firsts.is_empty(), "a {forced}'s first phase: {heard:?}");
        // Its answer lands on the fighter that crossed, on the step after the crossing, since a hook's
        // commands take effect at the end of the step that fired it.
        let forced = Some(Name::new(forced));
        for &&(tick, actor, _) in &firsts {
            let next = running.iter().find(|(at, id, _)| *at == tick + 1 && *id == actor);
            let (.., ran) = next.unwrap_or_else(|| panic!("{actor:?} is there after crossing at {tick}"));
            assert_eq!(*ran, forced, "{actor:?} crossed at {tick}");
        }
    }
}
