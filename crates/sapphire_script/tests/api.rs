//! The declared API, against what a VM has and what `sapphire.d.luau` says, and what each command does to a
//! real fight.
//!
//! xtask's own test holds the checked-in files to `api.toml`. This one reads the other side: the table that
//! compiled against what the definitions tell an editor, so a binding and its `.d.luau` can't disagree
//! however they got that way.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use mlua::{Table, Value};
use sapphire_bytecode::ScriptChunk;
use sapphire_core::Name;
use sapphire_gameplay::replay::{
    PlayError, Player, Recording, SCENARIOS, Scenario, ScenarioError, ScenarioSetup, arena_fight,
    find_scenario, find_script,
};
use sapphire_gameplay::script::BOARD_SLOTS;
use sapphire_gameplay::{ActorId, ActorSpawn, Faction, HookFrame, ScriptCommand, Simulation, TickActions};
use sapphire_script::scenarios::{ARENA_SCRIPTED, SCRIPTED};
use sapphire_script::{
    API, CookedScript, Declared, HOOKS, Limits, ScriptHost, ScriptVm, attach, cook, host_of, rebind,
};
use support::run_hook;

const DEFINITIONS: &str = include_str!("../sapphire.d.luau");

/// What `.d.luau` says of a function or a hook: its arguments' names, and how many it returns.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Signature {
    args: Vec<String>,
    returns: usize,
}

/// `a: number, b: number` as its names, or `None` if anything in it isn't a number.
fn args_of(text: &str) -> Option<Vec<String>> {
    if text.trim().is_empty() {
        return Some(Vec::new());
    }
    text.split(',').map(|arg| arg.trim().strip_suffix(": number").map(str::to_owned)).collect()
}

/// `name: (a: number) -> number,` as its name and signature.
fn function_line(line: &str) -> Option<(String, Signature)> {
    let (name, rest) = line.trim().split_once(": (")?;
    let (args, returns) = rest.split_once(") -> ")?;
    let returns = returns.strip_suffix(',')?;
    let returns = match returns {
        "()" => 0,
        "number" => 1,
        many => many.strip_prefix('(')?.strip_suffix(')')?.split(',').count(),
    };
    Some((name.to_owned(), Signature { args: args_of(args)?, returns }))
}

/// `on_it: ((a: number) -> ())?,` as its name and arguments.
fn hook_line(line: &str) -> Option<(String, Vec<String>)> {
    let (name, rest) = line.trim().split_once(": ((")?;
    let args = rest.strip_suffix(") -> ())?,")?;
    Some((name.to_owned(), args_of(args)?))
}

/// The `{ ... }` block that follows a line starting `opening`, without its comments.
fn block<'a>(text: &'a str, opening: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|line| !line.starts_with(opening))
        .skip(1)
        .take_while(|line| !line.starts_with('}'))
        .filter(|line| !line.trim().starts_with("--"))
        .collect()
}

/// Everything on which the compiled table and a `.d.luau`'s text disagree, in words.
fn disagreements(api: &[Declared], hooks: &[(&str, &[&str])], text: &str) -> Vec<String> {
    let mut said = BTreeMap::new();
    let mut wrong = Vec::new();
    for line in block(text, "declare sapphire:") {
        match function_line(line) {
            Some((name, signature)) => {
                said.insert(name, signature);
            }
            None => wrong.push(format!("can't read `{line}`")),
        }
    }
    for row in api {
        let name = row.binding.name;
        let declared = Signature {
            args: row.args.iter().map(|arg| (*arg).to_owned()).collect(),
            returns: row.returns.len(),
        };
        match said.remove(name) {
            None => wrong.push(format!("{name} has no definition")),
            Some(signature) if signature != declared => {
                wrong.push(format!("{name} is {declared:?} and defined as {signature:?}"));
            }
            Some(_) => {}
        }
    }
    wrong.extend(said.keys().map(|name| format!("{name} is defined and isn't bound")));

    let mut said = BTreeMap::new();
    for line in block(text, "export type Hooks") {
        if line.trim() == "names: { string }?," {
            continue;
        }
        match hook_line(line) {
            Some((name, args)) => {
                said.insert(name, args);
            }
            None => wrong.push(format!("can't read `{line}`")),
        }
    }
    for (name, args) in hooks {
        match said.remove(*name) {
            None => wrong.push(format!("{name} has no definition")),
            Some(defined) if defined != *args => {
                wrong.push(format!("{name} takes {args:?}, defined {defined:?}"))
            }
            Some(_) => {}
        }
    }
    wrong.extend(said.keys().map(|name| format!("{name} is defined and never fired")));
    wrong
}

#[test]
fn the_bindings_and_the_definitions_agree() {
    assert_eq!(disagreements(API, HOOKS, DEFINITIONS), Vec::<String>::new());
    assert!(API.len() >= 16, "the table's there");
    assert!(HOOKS.len() >= 5, "the table's there");
}

/// Each way the two could part, which the test above would then catch.
#[test]
fn a_binding_and_a_definition_that_disagree_fail() {
    let edited = |old: &str, new: &str| {
        assert!(DEFINITIONS.contains(old), "{old}");
        DEFINITIONS.replacen(old, new, 1)
    };
    let fails = |text: &str, says: &str| {
        let wrong = disagreements(API, HOOKS, text);
        assert!(wrong.iter().any(|line| line.contains(says)), "wanted {says}, got {wrong:?}");
    };

    // A definition left behind, gone, or differing in its arguments, its results or its kinds.
    fails(&edited("    tick: () -> number,\n", ""), "tick has no definition");
    fails(&edited("    tick: () -> number,\n", "    tick: () -> number,\n    gone: () -> (),\n"), "gone");
    fails(&edited("health: (actor: number) -> number", "health: (slot: number) -> number"), "health is");
    fails(
        &edited("health: (actor: number) -> number", "health: (actor: number, b: number) -> number"),
        "health is",
    );
    fails(
        &edited(
            "position: (actor: number) -> (number, number, number)",
            "position: (actor: number) -> (number, number)",
        ),
        "position is",
    );
    fails(&edited("tick: () -> number", "tick: () -> ()"), "tick is");
    fails(&edited("health: (actor: number) -> number", "health: (actor: string) -> number"), "can't read");

    // And a hook's.
    fails(&edited("    on_beat: ((tick: number) -> ())?,\n", ""), "on_beat has no definition");
    fails(&edited("on_beat: ((tick: number) -> ())?", "on_beat: ((step: number) -> ())?"), "on_beat takes");
    fails(
        &edited("names: { string }?,\n", "names: { string }?,\n    on_gone: (() -> ())?,\n"),
        "on_gone is defined",
    );

    // A row the table grew and nobody said.
    unsafe extern "C-unwind" fn added(_: *mut mlua::lua_State) -> std::ffi::c_int {
        sapphire_script::bindings::in_host(|| 0)
    }
    // SAFETY: `added` reads nothing, returns no results, goes through `in_host` and can't raise.
    let binding = unsafe { sapphire_script::Binding::new("added", added) };
    let added = Declared { binding, args: &[], returns: &[] };
    let rows: Vec<Declared> = API.iter().copied().chain([added]).collect();
    let wrong = disagreements(&rows, HOOKS, DEFINITIONS);
    assert_eq!(wrong, ["added has no definition"]);
}

/// What a VM has in `sapphire` is the table, whole: nothing's bound past what's declared.
#[test]
fn a_vm_has_what_the_table_declares_and_nothing_else() {
    let vm = ScriptVm::new(Limits::default()).unwrap();
    // SAFETY: this reads one global table's keys, and runs no bytecode.
    let sapphire: Table = unsafe { vm.lua_for_tests() }.globals().get("sapphire").unwrap();
    let mut found = BTreeSet::new();
    sapphire
        .for_each(|name: String, _: Value| {
            found.insert(name);
            Ok(())
        })
        .unwrap();
    let declared: BTreeSet<String> = API.iter().map(|row| row.binding.name.to_owned()).collect();
    assert_eq!(found, declared);
}

/// A wrapper does what the table says of what it pushes: a query's numbers come back, in order.
#[test]
fn a_query_returns_what_it_declares() {
    let source = "return { hook = function()
        local x, y, z = sapphire.position(0)
        local first = sapphire.tick()
        sapphire.mark(x) sapphire.mark(y) sapphire.mark(z) sapphire.mark(first)
        sapphire.mark(select('#', sapphire.position(0)))
        sapphire.mark(select('#', sapphire.tick()))
        sapphire.mark(select('#', sapphire.mark(1)))
    end }";
    let (_, frame) = run_hook(source).unwrap();
    let values: Vec<f64> = frame
        .queued()
        .iter()
        .filter_map(|command| if let ScriptCommand::Mark(value) = command { Some(*value) } else { None })
        .collect();
    assert_eq!(values, [0.0, 0.0, 0.0, 0.0, 3.0, 1.0, 1.0, 0.0]);
}

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
static WAITING: Mutex<Option<ScriptHost>> = Mutex::new(None);

fn skirmish_with_a_host(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = find_scenario(&SCENARIOS, "arena-skirmish").expect("listed").set_up(sim)?;
    attach(sim, WAITING.lock().unwrap().take().expect("a host is waiting"));
    Ok(cast)
}

/// `arena-scripted`'s world, with whatever host is waiting rather than its chunk.
fn skirmish() -> Scenario {
    Scenario { setup: ScenarioSetup::Code(skirmish_with_a_host), ..SCRIPTED[0] }
}

/// The arena's warrior: the player's 1.
const WARRIOR: ActorId = ActorId::from_raw(2).unwrap();

/// The skirmish, with `source` on it, stepped `ticks` times with nobody touching anything.
fn fight_with(source: &str, beat: u64, ticks: u32, limits: Limits) -> Simulation {
    let mut host = ScriptHost::new(limits).unwrap();
    host.bind(&cook(source).unwrap(), "=test").unwrap();
    host.beat_every(beat);
    *WAITING.lock().unwrap() = Some(host);
    let (mut sim, _) = skirmish().start(SCRIPTED[0].config).unwrap();
    for _ in 0..ticks {
        sim.step(&TickActions::default());
    }
    sim
}

fn stats(sim: &Simulation) -> sapphire_script::ScriptStats {
    host_of(sim).stats()
}

#[test]
fn a_force_move_starts_it_on_the_next_step_and_a_bad_place_loses_the_hook() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let source = "return {
        names = { 'overhead' },
        on_beat = function(tick) sapphire.force_move(2, 1) sapphire.set_board(0, tick) end,
    }";
    let sim = fight_with(source, 5, 6, Limits::default());
    let warrior = sim.actor(WARRIOR).unwrap();
    assert_eq!(warrior.moves.current.map(|current| current.name), Some(Name::new("overhead")));
    assert_eq!(sim.scripts().unwrap().commands_applied(), 1);
    assert_eq!(sim.scripts().unwrap().board()[0], 5.0);

    // Place 2 isn't listed: nothing it asked for counts, and what it wrote goes back.
    let source = "return {
        names = { 'overhead' },
        on_beat = function(tick) sapphire.set_board(0, tick) sapphire.mark(1) sapphire.force_move(2, 2) end,
    }";
    let sim = fight_with(source, 5, 6, Limits::default());
    assert_eq!(sim.scripts().unwrap().commands_applied(), 0);
    assert_eq!(sim.scripts().unwrap().board()[0], 0.0);
    assert_eq!(stats(&sim).commands_discarded, 1);
}

#[test]
fn a_hook_hears_a_move_by_its_place_and_every_other_as_nothing() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let source = "return {
        names = { 'overhead' },
        on_beat = function() sapphire.force_move(2, 1) end,
        on_move = function(actor, name) sapphire.set_board(name, sapphire.board(name) + 1) end,
    }";
    let sim = fight_with(source, 5, 20, Limits::default());
    let board = sim.scripts().unwrap().board();
    assert!(board[1] >= 1.0, "the listed one: {:?}", &board[..3]);
}

#[test]
fn slow_motion_reaches_the_clock_and_a_bad_rate_doesnt() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let clock_after = |rate: u32| {
        let source = format!("return {{ on_beat = function() sapphire.slow_motion({rate}, 200) end }}");
        let sim = fight_with(&source, 10, 60, Limits::default());
        (sim.actor(WARRIOR).unwrap().clock_now, sim.scripts().unwrap().commands_applied())
    };
    let plain = {
        let sim = fight_with("return { on_phase = function() end }", 10, 60, Limits::default());
        sim.actor(WARRIOR).unwrap().clock_now
    };
    let (slowed, applied) = clock_after(500);
    assert!(applied > 0);
    assert!(slowed < plain, "{slowed:?} against {plain:?}");
    // Three doesn't divide time evenly: refused at the one point commands apply, and hashed all the same.
    let (refused, applied) = clock_after(3);
    assert!(applied > 0);
    assert_eq!(refused, plain);
}

/// What a hook asks of someone who's fallen doesn't get them up, in the replay where the warrior falls.
#[test]
fn a_fallen_fighter_stays_down() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let source = "return {
        names = { 'overhead' },
        on_defeat = function(actor) sapphire.force_move(actor, 1) sapphire.mark(actor) end,
    }";
    let mut player = support::player_with(
        support::host_running(source, Limits::default()),
        support::replay_file("arena-fight-won"),
    );
    let mut fell = false;
    while player.step() {
        let warrior = player.simulation().actor(WARRIOR).unwrap();
        fell |= warrior.fighter.is_defeated();
        if fell {
            assert_ne!(warrior.moves.current.map(|current| current.name), Some(Name::new("overhead")));
        }
    }
    assert!(fell, "this replay's the one where the warrior falls");
    assert!(player.simulation().scripts().unwrap().commands_applied() >= 2, "it asked");
}

/// `arena_fight`, with a third who's been given nothing, for a hook to give it.
fn fight_with_a_bystander(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = arena_fight(sim)?;
    let warrior = sim.actor(cast[1]).expect("just spawned");
    let point = sapphire_gameplay::arena::spawns().crowd[0];
    let bystander = sim.spawn(ActorSpawn {
        position: point.position(),
        yaw: point.yaw(),
        moves: warrior.moves.set,
        controller: warrior.controller.profile(),
        faction: Faction::new(3).expect("a faction's number"),
    })?;
    assert_eq!(bystander, BYSTANDER);
    attach(sim, WAITING.lock().unwrap().take().expect("a host is waiting"));
    Ok(cast)
}

const BYSTANDER: ActorId = ActorId::from_raw(3).unwrap();

#[test]
fn a_brain_and_a_soft_lock_given_by_a_hook_take_and_keep() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let source = "return { on_beat = function()
        sapphire.give_soft_lock(3)
        sapphire.give_brain(3)
        sapphire.set_board(0, sapphire.soft_lock_target(3))
        sapphire.set_board(1, sapphire.brain_target(3))
    end }";
    let mut host = ScriptHost::new(Limits::default()).unwrap();
    host.bind(&cook(source).unwrap(), "=test").unwrap();
    host.beat_every(5);
    *WAITING.lock().unwrap() = Some(host);
    let scenario = Scenario { setup: ScenarioSetup::Code(fight_with_a_bystander), ..SCRIPTED[0] };
    let (mut sim, _) = scenario.start(scenario.config).unwrap();
    let bystander = sim.actor(BYSTANDER).unwrap();
    assert!(!bystander.brain.is_on() && !bystander.soft_lock.is_on());
    for _ in 0..6 {
        sim.step(&TickActions::default());
    }
    let given = sim.actor(BYSTANDER).unwrap();
    assert!(given.brain.is_on() && given.soft_lock.is_on());

    // A hook asking again each beat doesn't start them over, and what they chose reads back.
    let mut chosen = None;
    for _ in 0..60 {
        sim.step(&TickActions::default());
        chosen = chosen.or(sim.actor(BYSTANDER).unwrap().brain.target());
    }
    assert!(chosen.is_some(), "it picked a foe");
    let bystander = sim.actor(BYSTANDER).unwrap();
    let board = sim.scripts().unwrap().board();
    assert!(bystander.soft_lock.target().is_some(), "it chose someone");
    assert!(board[0] > 0.0 && board[1] > 0.0, "{:?}", &board[..2]);
}

/// Once a step's whole allowance has gone, what's left of its events is skipped, the same on every run,
/// rather than a crowd's hits costing a hook's budget each.
#[test]
fn a_step_stops_answering_when_its_interrupts_have_gone() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let limits = Limits { interrupts: 1000, step_interrupts: 900, ..Limits::default() };
    let source = "return {
        on_beat = function() while true do end end,
        on_phase = function() while true do end end,
        on_move = function() while true do end end,
    }";
    let mut host = ScriptHost::new(limits).unwrap();
    host.bind(&cook(source).unwrap(), "=test").unwrap();
    host.beat_every(1);
    *WAITING.lock().unwrap() = Some(host);
    let (mut sim, cast) = skirmish().start(SCRIPTED[0].config).unwrap();
    // Every warrior starts a move at some point: the step that has more than three events shows it.
    let fight_back = find_script("fight-back").unwrap();
    for tick in 0..400 {
        sim.step(&(fight_back.actions)(tick, &cast));
    }
    let stats = stats(&sim);
    assert!(stats.hooks_over_budget > 0, "{stats:?}");
    assert!(stats.hooks_skipped > 0, "{stats:?}");
}

/// The hash of the bytes, in the replay's `tables_hash`: a chunk that means something else refuses a
/// recording made with the old one, as a move's new number does, and one with no script stays as it was.
#[test]
fn bytecode_is_in_the_tables_hash_and_a_recording_refuses_another() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let start = |source: &str| {
        let mut host = ScriptHost::new(Limits::default()).unwrap();
        host.bind(&cook(source).unwrap(), "=test").unwrap();
        *WAITING.lock().unwrap() = Some(host);
        skirmish()
    };
    let chunk = "return { on_phase = function() sapphire.mark(1) end }";
    let changed = "return { on_phase = function() sapphire.mark(2) end }";

    let scenario = start(chunk);
    let mut recording = Recording::new(&scenario, scenario.config).unwrap();
    for _ in 0..10 {
        recording.step(&mut |_: u64, _: &[ActorId]| TickActions::default()).unwrap();
    }
    let (replay, _) = recording.finish();

    let scenario = start(chunk);
    assert!(Player::new(replay.clone(), &[scenario]).is_ok(), "the same chunk plays");
    let scenario = start(changed);
    assert!(
        matches!(Player::new(replay, &[scenario]), Err(PlayError::MovesChanged { .. })),
        "another chunk is another fight",
    );

    // And a reload moves it at once.
    let mut host = ScriptHost::new(Limits::default()).unwrap();
    host.bind(&cook(chunk).unwrap(), "=test").unwrap();
    *WAITING.lock().unwrap() = Some(host);
    let (mut sim, _) = skirmish().start(SCRIPTED[0].config).unwrap();
    let before = sim.tables_hash();
    rebind(&mut sim, &cook(changed).unwrap(), "=reloaded").unwrap();
    assert_ne!(sim.tables_hash(), before);
}

/// A fight nobody asked scripts of hashes as it did: nothing goes in either hash until something does.
#[test]
fn a_world_with_no_scripts_hashes_as_it_did() {
    let plain = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let (mut sim, _) = plain.start(plain.config).unwrap();
    let (tables, checksum) = (sim.tables_hash(), sim.checksum());
    assert!(sim.scripts().is_none());
    sim.enable_scripts();
    assert_ne!((sim.tables_hash(), sim.checksum()), (tables, checksum), "asked, and it's there");
    let (mut again, _) = plain.start(plain.config).unwrap();
    assert_eq!((again.tables_hash(), again.checksum()), (tables, checksum), "and not asked, it isn't");
    again.enable_scripts();
    let (once, twice) = (again.tables_hash(), {
        again.enable_scripts();
        again.tables_hash()
    });
    assert_eq!(once, twice, "asking twice is asking");
}

/// The committed chunk cooks, loads and uses the lot, and its cooked form reaches a VM by the road a pack's
/// does: `sapphire_pipeline` writes this envelope, and `sapphire_assets` reads it.
#[test]
fn the_scenarios_chunk_goes_all_the_way() {
    let cooked = ScriptChunk { bytecode: cook(ARENA_SCRIPTED).unwrap().into_bytes() }.to_cooked().unwrap();
    let archived = ScriptChunk::read(&cooked).unwrap();
    let mut host = ScriptHost::new(Limits::default()).unwrap();
    // SAFETY: the pinned cooker made these bytes, and the envelope gives them back unchanged.
    let chunk = unsafe { CookedScript::from_trusted_bytes(archived.bytecode.to_vec()) };
    host.bind(&chunk, "=arena-scripted").unwrap();

    for row in API {
        assert!(
            ARENA_SCRIPTED.contains(&format!("sapphire.{}(", row.binding.name)),
            "`arena-scripted` should use every binding, and doesn't use {}",
            row.binding.name
        );
    }
    for (hook, _) in HOOKS {
        assert!(ARENA_SCRIPTED.contains(&format!("{hook} = function")), "{hook}");
    }
}

/// What `HookFrame` keeps fits what the table's slots say.
#[test]
fn the_definitions_say_where_the_board_ends() {
    assert_eq!(BOARD_SLOTS, 64);
    assert!(DEFINITIONS.contains("from slot 0 to 63"));
    let frame = HookFrame::new(0);
    assert_eq!(frame.board.len(), BOARD_SLOTS);
}
