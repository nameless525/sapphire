//! A chunk swapped in the middle of a real replay, between steps. The board survives it, and the
//! checksum follows only what the new chunk means to change.

mod support;

use std::sync::Mutex;

use sapphire_script::{BOARD_SLOTS, CookedScript, Limits, ScriptStats, cook, host_of, host_of_mut};
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

fn run(reload: Option<&str>) -> Run {
    let mut host = host_running(FIRST, Limits::default());
    host.beat_every(25);
    let mut player = player_with(host, replay_file("arena-fight-won"));
    let (mut sim, mut digest) = (Vec::new(), Vec::new());
    let mut tick = 0;
    while player.step() {
        tick += 1;
        sim.push(player.simulation().checksum());
        let host = host_of(player.simulation());
        digest.push(host.digest());
        host.gc_step().unwrap();
        if tick == RELOAD_AT
            && let Some(source) = reload
        {
            host_of_mut(player.simulation_mut()).bind(&cook(source).unwrap(), "=reloaded").unwrap();
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

    // A beat's mark isn't the simulation's. Only a phase's slow motion can reach it, so it stays put
    // until the first phase after the reload.
    let phase_only = run(Some(TUNED_PHASE_ONLY));
    let digest_moved = first_difference(&plain.digest, &phase_only.digest);
    let sim_moved = first_difference(&plain.sim, &phase_only.sim);
    assert!(sim_moved.is_none_or(|tick| tick >= RELOAD_AT as usize), "{sim_moved:?}");
    assert_eq!(digest_moved, sim_moved, "the same command moved both, on the same tick");
    assert!(sim_moved.is_some(), "a phase has to come after tick {RELOAD_AT} for this to show anything");
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
