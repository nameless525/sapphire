//! A script's source reloaded in the middle of a real replay, between steps, as hot reload and live link
//! bring it. The board survives it, and the checksum follows only what the new chunk means to change.

mod support;

use std::sync::Mutex;

use sapphire_gameplay::replay::{LiveMoves, Recording, SCENARIOS};
use sapphire_gameplay::script::BOARD_SLOTS;
use sapphire_gameplay::{ActorId, TickActions};
use sapphire_script::scenarios::{ARENA_SCRIPTED, ARENA_SCRIPTED_FILE, SCRIPTED};
use sapphire_script::{CookedScript, Limits, ReloadError, Reloaded, ScriptStats, cook, host_of, reload};
use support::{finish, host_running, player_with, replay_file};

/// The first chunk. Every number it leaves goes in the digest, and the board keeps a count of hooks.
const FIRST: &str = r#"
return {
    on_beat = function(tick)
        sapphire.set_board(1, sapphire.board(1) + 1)
        sapphire.mark(tick)
    end,
    on_phase = function(actor, phase)
        sapphire.set_board(2, sapphire.board(2) + 1)
        sapphire.slow_motion(500, 10)
    end,
}
"#;

/// The same, with a comment and a line more of nothing: other bytes, no other meaning.
const FIRST_REWORDED: &str = r#"
-- Reworded in the middle of a fight.

return {
    on_beat = function(tick)
        -- A count of beats.
        sapphire.set_board(1, sapphire.board(1) + 1)
        sapphire.mark(tick)
    end,
    on_phase = function(actor, phase)
        sapphire.set_board(2, sapphire.board(2) + 1)
        sapphire.slow_motion(500, 10)
    end,
}
"#;

/// A new slow motion for a phase, and a new number for a beat.
const TUNED: &str = r#"
return {
    on_beat = function(tick)
        sapphire.set_board(1, sapphire.board(1) + 1)
        sapphire.mark(tick * 2)
    end,
    on_phase = function(actor, phase)
        sapphire.set_board(2, sapphire.board(2) + 1)
        sapphire.slow_motion(200, 30)
    end,
}
"#;

/// The same as `TUNED` for phases, with the beat left alone: only a phase's slow motion can move the
/// simulation, so the simulation's checksum moves only from the next phase on.
const TUNED_PHASE_ONLY: &str = r#"
return {
    on_beat = function(tick)
        sapphire.set_board(1, sapphire.board(1) + 1)
        sapphire.mark(tick)
    end,
    on_phase = function(actor, phase)
        sapphire.set_board(2, sapphire.board(2) + 1)
        sapphire.slow_motion(200, 30)
    end,
}
"#;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const RELOAD_AT: u32 = 400;

struct Run {
    /// The simulation's checksum after each tick.
    sim: Vec<u64>,
    /// The script's digest after each tick.
    digest: Vec<u64>,
    board: [f64; BOARD_SLOTS],
    stats: ScriptStats,
}

fn run(edit: Option<&str>) -> Run {
    let mut host = host_running(FIRST, Limits::default());
    host.beat_every(25);
    let mut player = player_with(host, replay_file("arena-fight-won"));
    let (mut sim, mut digest) = (Vec::new(), Vec::new());
    let mut tick = 0;
    while player.step() {
        tick += 1;
        sim.push(player.simulation().checksum());
        digest.push(player.simulation().scripts().expect("attached").digest());
        if tick == RELOAD_AT
            && let Some(source) = edit
        {
            assert_eq!(reload(player.simulation_mut(), "test.luau", source).unwrap(), Reloaded::Replaced);
        }
    }
    let finished = finish(player);
    Run { sim, digest, board: finished.board, stats: finished.stats }
}

fn first_difference(a: &[u64], b: &[u64]) -> Option<usize> {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).position(|(a, b)| a != b)
}

#[test]
fn a_reload_of_the_same_meaning_moves_nothing() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    let plain = run(None);
    assert_ne!(cook(FIRST).unwrap(), cook(FIRST_REWORDED).unwrap(), "other bytes");
    let reloaded = run(Some(FIRST_REWORDED));
    assert_eq!(first_difference(&plain.sim, &reloaded.sim), None);
    assert_eq!(first_difference(&plain.digest, &reloaded.digest), None);
    assert!(plain.stats.hooks_fired > 50);
    assert_eq!(plain.board, reloaded.board, "and the board went through it");
}

#[test]
fn a_reload_moves_only_what_it_means_to_from_when_it_means_to() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    let plain = run(None);
    let tuned = run(Some(TUNED));
    let digest_moved = first_difference(&plain.digest, &tuned.digest).expect("a new mark moves the digest");
    assert!(digest_moved >= RELOAD_AT as usize, "moved before the reload, at {digest_moved}");
    assert!(
        digest_moved < RELOAD_AT as usize + 25,
        "the first beat after it should move it, at {digest_moved}"
    );
    // The count on the board kept going through the reload.
    assert_eq!(plain.board[1].to_bits(), tuned.board[1].to_bits());
    assert!(tuned.board[1] > 10.0);

    // Everything a hook leaves is in the simulation's checksum, so this reload's only change, a phase's slow
    // motion, moves both at the first phase after it.
    let phase_only = run(Some(TUNED_PHASE_ONLY));
    let digest_moved = first_difference(&plain.digest, &phase_only.digest);
    let sim_moved = first_difference(&plain.sim, &phase_only.sim);
    assert!(sim_moved.is_none_or(|tick| tick >= RELOAD_AT as usize), "{sim_moved:?}");
    assert_eq!(digest_moved, sim_moved, "the same command moved both, on the same tick");
    assert!(sim_moved.is_some(), "a phase has to come after tick {RELOAD_AT} for this to show anything");
}

/// A reload that can't land, or lands on what's there, leaves the fight as it was, `tables_hash` and all, so
/// a recording goes on; one that lands says so in `tables_hash`.
#[test]
fn a_reload_that_changes_nothing_or_fails_keeps_what_runs() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    let mut player = player_with(host_running(FIRST, Limits::default()), replay_file("arena-fight-won"));
    for _ in 0..10 {
        player.step();
    }
    let sim = player.simulation_mut();
    let tables = sim.tables_hash();
    assert_eq!(reload(sim, "test.luau", FIRST).unwrap(), Reloaded::Unchanged);
    let breaks_a_rule = "return { on_beat = function(tick) sapphire.mark(tick ^ tick) end }";
    assert!(matches!(reload(sim, "test.luau", breaks_a_rule), Err(ReloadError::Vm(_))));
    assert!(matches!(reload(sim, "test.luau", "return {"), Err(ReloadError::Vm(_))));
    let elsewhere = reload(sim, "boss.luau", TUNED).unwrap_err();
    assert!(matches!(&elsewhere, ReloadError::NotBound { bound, .. } if bound == "test.luau"), "{elsewhere}");
    assert_eq!(sim.tables_hash(), tables, "nothing landed");
    assert_eq!(host_of(sim).source_name(), "test.luau");

    assert_eq!(reload(sim, "test.luau", TUNED).unwrap(), Reloaded::Replaced);
    assert_ne!(sim.tables_hash(), tables, "a replay recorded before refuses what comes after");
    assert_eq!(reload(sim, "test.luau", TUNED).unwrap(), Reloaded::Unchanged);
}

/// A fight started after an edit binds the edit, as a build with it in its file would, so starting over
/// doesn't quietly go back; and a recording ends its file at the first edit that changes something.
#[test]
fn a_new_fight_starts_on_the_newest_edit_and_a_recording_ends_at_one() {
    let scripted = &SCRIPTED[0];
    let edited = format!("-- An edit: other lines, other bytes.\n{ARENA_SCRIPTED}");
    let (mut unscripted, _) = SCENARIOS[0].start(SCENARIOS[0].config).unwrap();
    let none = reload(&mut unscripted, ARENA_SCRIPTED_FILE, ARENA_SCRIPTED).unwrap_err();
    assert!(matches!(none, ReloadError::NoScript { .. }), "{none}");
    let (mut plain, _) = LiveMoves::default().start(scripted, scripted.config).unwrap();
    let mut live = LiveMoves::default();
    live.remember_script(ARENA_SCRIPTED_FILE, &edited);
    assert_eq!(live.latest_script(ARENA_SCRIPTED_FILE), Some(edited.as_str()));
    let (on_the_edit, _) = live.start(scripted, scripted.config).unwrap();
    assert_ne!(plain.tables_hash(), on_the_edit.tables_hash());
    assert_eq!(reload(&mut plain, ARENA_SCRIPTED_FILE, &edited).unwrap(), Reloaded::Replaced);
    assert_eq!(plain.tables_hash(), on_the_edit.tables_hash(), "the same as reloading the first");

    let mut recording = Recording::new(scripted, scripted.config).unwrap();
    for _ in 0..10 {
        recording.step(&mut |_: u64, _: &[ActorId]| TickActions::default()).unwrap();
    }
    let unchanged = recording.edit(|sim| reload(sim, ARENA_SCRIPTED_FILE, ARENA_SCRIPTED)).unwrap();
    assert_eq!((unchanged, recording.edited_at()), (Reloaded::Unchanged, None));
    let replaced = recording.edit(|sim| reload(sim, ARENA_SCRIPTED_FILE, &edited)).unwrap();
    assert_eq!((replaced, recording.edited_at()), (Reloaded::Replaced, Some(10)));
}

#[test]
fn a_chunk_that_breaks_a_rule_never_replaces_the_one_running() {
    let mut host = host_running(FIRST, Limits::default());
    let plain = mlua::chunk::Compiler::new().set_optimization_level(2);
    let bytes = plain.compile("return { on_beat = function(x) return math.sin(x) end }").unwrap();
    // SAFETY: the unchanged bytes came from the same pinned compiler, without its builtin restrictions.
    let compiled = unsafe { CookedScript::from_trusted_bytes(bytes) };
    assert!(host.bind(&compiled, "=bad").is_err(), "refused at the VM, for a pushed chunk");
    assert_eq!(host.stats().hooks_fired, 0);
}
