//! A real kill: a helper process downloads a patch from this one's localhost host, and this one kills it
//! after a number of requests that changes each round, until a round's download finishes. Each round must
//! fetch nothing the rounds before it had staged whole, and the last must leave the release staged whole.
//!
//! `Child::kill` is `SIGKILL` on Unix and `TerminateProcess` on Windows: nothing runs in the helper after
//! it, so it may die between a write and its rename, or mid-write. A power cut can't be tested here: only
//! the staged manifest is flushed, and every other staged file is hashed before it counts.
//!
//! Its own `main` (`harness = false`), because the test binary is also the helper.

#![allow(clippy::print_stdout, reason = "with no harness, this says how the kills landed")]

mod common;

use std::path::Path;
use std::process::{Command, ExitCode};
use std::thread;
use std::time::Duration;

use common::server::{Fault, Host};
use common::{
    APP_BUILD, EXPIRES, NOW, TRUSTED, TestDir, asked_of, assert_staged_whole, first_release, read_install,
    retries, second_release, staged_hashes,
};
use sapphire_patch::{Download, HttpTransport, Outcome, Request, Roots, Timeouts};

/// The most rounds before the download must have finished.
const ROUNDS: usize = 40;

/// The helper: downloads from `url` over the install in `install` into `staging`, and exits 0 once it's
/// staged.
fn patcher(staging: &Path, url: &str, install: &Path) -> ExitCode {
    let request = Request {
        installed: read_install(install),
        staging: staging.to_owned(),
        now: NOW,
        app_build: APP_BUILD,
        retries: retries(),
    };
    let transport = HttpTransport::new(url, Roots::Platform, Timeouts::default()).expect("a host");
    match Download::start(request, transport).expect("starts").join() {
        Ok(Outcome::Staged(_)) => ExitCode::SUCCESS,
        other => {
            eprintln!("the helper's download: {other:?}");
            ExitCode::from(2)
        }
    }
}

/// xorshift64: the kills only need to spread out, and to be the same each run.
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn kill_loop(dir: &TestDir) {
    let (from, to) = (first_release(), second_release());
    let install = dir.join("install");
    from.write_install(&install, 1);
    let staging = dir.join("staging");
    let host = Host::start(to.host(&to.signed(2, EXPIRES, TRUSTED)));
    // Slow enough that a kill lands while the helper is mid-download.
    host.set_faults(|_| Fault::Delay(4));
    let exe = std::env::current_exe().expect("this test's path");
    let mut random = 0x5361_7070_6869_7265;
    let mut killed = 0;

    for round in 0..ROUNDS {
        let (staged_blocks, staged_tables) = staged_hashes(&staging);
        let first = host.log().len();
        let mut helper = Command::new(&exe)
            .arg("--patcher")
            .arg(&staging)
            .arg(host.url())
            .arg(&install)
            .spawn()
            .expect("starts the helper");
        // Each round gets a few more requests than its manifest, so every round can make progress.
        let allowed = 2 + next_random(&mut random) as usize % 6;
        let status = loop {
            if let Some(status) = helper.try_wait().expect("polls the helper") {
                break Some(status);
            }
            if host.log_so_far().len() - first >= allowed {
                // Somewhere between this request's answer and the next one's.
                thread::sleep(Duration::from_micros(next_random(&mut random) % 3000));
                helper.kill().expect("kills the helper");
                helper.wait().expect("reaps the helper");
                break None;
            }
            thread::sleep(Duration::from_micros(200));
        };

        let (blocks, tables) = asked_of(&to, &host.log()[first..]);
        assert!(
            blocks.iter().all(|hash| !staged_blocks.contains(hash)),
            "round {round}: a block staged whole before it was fetched again"
        );
        assert!(
            tables.iter().all(|hash| !staged_tables.contains(hash)),
            "round {round}: tables staged whole before were fetched again"
        );
        match status {
            Some(status) if status.success() => {
                assert_staged_whole(&staging, &from, &to);
                println!("{killed} kills, then round {round} finished; nothing staged was fetched twice");
                assert!(killed >= 3, "too few kills landed mid-download to prove anything");
                return;
            }
            Some(status) => panic!("round {round}: the helper failed: {status}"),
            None => killed += 1,
        }
    }
    panic!("{ROUNDS} rounds didn't finish the download");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--patcher") => return patcher(Path::new(&args[2]), &args[3], Path::new(&args[4])),
        // Libtest's own questions: this has no ignored tests to run or list.
        Some("--ignored" | "--list") => return ExitCode::SUCCESS,
        _ => {}
    }
    let dir = TestDir::new("kill");
    kill_loop(&dir);
    ExitCode::SUCCESS
}
