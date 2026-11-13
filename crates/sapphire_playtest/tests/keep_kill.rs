//! A real kill (M4 F3): a helper process plays a session, keeping as a game does (on a job, every 100 frames of
//! 100 ms), run after run, until this one kills it at a moment that changes each round. After every kill the
//! session must read as of its last whole keep, every run it names replaying to the checksums the device kept.
//!
//! `Child::kill` is `SIGKILL` on Unix and `TerminateProcess` on Windows: nothing runs in the helper after it,
//! so it may die mid-append or between a run's record and the document. The disk is `common::Plain`, without
//! the flushes only a power cut needs.
//!
//! Its own `main` (`harness = false`), because the test binary is also the helper.

#![allow(clippy::print_stdout, reason = "with no harness, this says how the kills landed")]

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use common::{Plain, Played, Scratch, about, every_run_replays};
use sapphire_playtest::{FRAMES_FILE, FrameRow, Session, journal};

/// Kills at least, and at most: past [`ROUNDS`] it kills on until [`MID_KEEP`] kills have landed mid-keep.
const ROUNDS: usize = 40;
const MAX_ROUNDS: usize = 120;

/// Kills that must land with a keep part way written past the document, for the rounds to prove that case. How
/// many of 40 do swings with the machine's load (M4 F4 measured 1 to 11), so the rounds go on until they have.
const MID_KEEP: usize = 3;

/// Each run's length, in ticks: a run ends, and another begins.
const RUN: u32 = 700;

/// The helper: plays runs into a session under `sessions` until it's killed.
fn player(sessions: &Path) -> ExitCode {
    let mut session = Session::begin(sessions, about(None), Arc::new(Plain)).expect("begins");
    loop {
        let mut played = Played::new();
        played.play_with(&mut session, RUN, Duration::from_millis(100), false);
        let (replay, trail) = played.so_far();
        session.run_ended(replay, trail);
    }
}

/// xorshift64: the kills only need to spread out, and to be the same each run.
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Whether the kill left part of a keep past what the document names: rows of frames, or a run's records.
fn left_a_keep_part_way(dir: &Path, kept: &sapphire_playtest::Kept) -> bool {
    let text = fs::read_to_string(dir.join(FRAMES_FILE)).unwrap_or_default();
    let rows = FrameRow::read_all(&text).map_or(0, |rows| rows.len());
    let torn = !text.is_empty() && !text.ends_with('\n');
    let ahead = kept.document.runs.iter().any(|run| {
        let Some(path) = &run.journal else { return false };
        let bytes = fs::read(dir.join(path)).unwrap_or_default();
        matches!(
            journal::read(&bytes, (u32::MAX, true), "run", "here"),
            Err(journal::JournalError::Short { reached, .. }) if reached > run.ticks
        )
    });
    torn || ahead || kept.document.frames.is_some_and(|counted| rows > counted as usize)
}

fn kill_loop(scratch: &Scratch) {
    let exe = std::env::current_exe().expect("this test's path");
    let mut random = 0x5361_7070_6869_7265;
    let (mut read, mut part_way, mut runs, mut rounds) = (0, 0, 0, 0);
    for round in 0..MAX_ROUNDS {
        if round >= ROUNDS && part_way >= MID_KEEP {
            break;
        }
        rounds += 1;
        let sessions = scratch.sessions();
        let mut helper =
            Command::new(&exe).arg("--player").arg(&sessions).spawn().expect("starts the helper");
        thread::sleep(Duration::from_millis(200 + next_random(&mut random) % 2_000));
        helper.kill().expect("kills the helper");
        helper.wait().expect("reaps the helper");

        let dirs: Vec<_> = fs::read_dir(&sessions).unwrap().map(|entry| entry.unwrap().path()).collect();
        assert_eq!(dirs.len(), 1, "round {round}");
        let dir = &dirs[0];
        if dir.join(sapphire_playtest::DOCUMENT_FILE).exists() {
            let kept = every_run_replays(dir);
            read += 1;
            runs += kept.document.runs.len();
            part_way += usize::from(left_a_keep_part_way(dir, &kept));
        }
        fs::remove_dir_all(&sessions).unwrap();
    }
    println!(
        "{rounds} kills: {read} left a session ({runs} runs, each replaying to the device's checksums), \
         {part_way} of them with a keep part way written past their document"
    );
    assert!(read >= rounds / 2, "too few kills landed after a first keep to prove anything");
    assert!(part_way >= MID_KEEP, "too few kills landed mid-keep to prove anything: {part_way} of {rounds}");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--player") => return player(Path::new(&args[2])),
        // Libtest's own questions: this has no ignored tests to run or list.
        Some("--ignored" | "--list") => return ExitCode::SUCCESS,
        _ => {}
    }
    let scratch = Scratch::new("keep_kill");
    kill_loop(&scratch);
    ExitCode::SUCCESS
}
