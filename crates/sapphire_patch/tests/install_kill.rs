//! A real kill: a helper process installs a staged release, and this one kills it at a moment that changes
//! each round. After every kill the app must start on exactly one whole release, the one it had or the one
//! being installed; a round after a kill goes on from what the kill left, and once an install finishes the
//! next round starts afresh.
//!
//! `Child::kill` is `SIGKILL` on Unix and `TerminateProcess` on Windows: nothing runs in the helper after
//! it. It's the process that dies, not the OS, so the page cache survives every kill; a power cut is
//! `durable_replace`'s flushes, whose docs say where each OS's guarantee stops.
//!
//! Its own `main` (`harness = false`), because the test binary is also the helper.

#![allow(clippy::print_stdout, reason = "with no harness, this says how the kills landed")]

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    APP_BUILD, NOW, ShippedDir, TestDir, assert_starts_on, first_release, listing, second_release, stage,
};
use sapphire_patch::{InstallOutcome, Installation};

const ROUNDS: usize = 120;

/// The helper: installs what's staged in `staging` over the app that shipped `shipped`, with its root at
/// `root`, and exits 0 once it's installed.
fn installer(shipped: &Path, root: &Path, staging: &Path) -> ExitCode {
    let mut installation = Installation::open(ShippedDir(shipped.to_owned()), root).expect("opens");
    match installation.install(staging, NOW, APP_BUILD) {
        Ok(InstallOutcome::Installed { .. } | InstallOutcome::UpToDate { .. }) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("the helper's install: {error}");
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
    let (first, second) = (first_release(), second_release());
    let (shipped, root, staging) = (dir.join("shipped"), dir.join("root"), dir.join("staging"));
    first.write_install(&shipped, 1);
    let exe = std::env::current_exe().expect("this test's path");
    let spawn = || {
        Command::new(&exe).arg("--installer").arg(&shipped).arg(&root).arg(&staging).spawn().expect("starts")
    };
    let afresh = || {
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&staging);
        stage(&staging, &first, &second, 2);
    };

    // How long a whole install takes, process start included: the kills spread over a little more.
    afresh();
    #[allow(
        clippy::disallowed_methods,
        reason = "spreads the kills over an install's length; nothing here ships"
    )]
    let started = Instant::now();
    assert!(spawn().wait().expect("waits").success(), "an install that isn't killed finishes");
    let whole = started.elapsed();
    afresh();

    let mut random = 0x5361_7070_6869_7265;
    let (mut before, mut after, mut finished, mut mid) = (0, 0, 0, 0);
    for round in 0..ROUNDS {
        let mut helper = spawn();
        let wait = whole.mul_f64(1.2).as_micros() as u64;
        thread::sleep(Duration::from_micros(next_random(&mut random) % wait.max(1)));
        let killed = helper.try_wait().expect("polls").is_none();
        if killed {
            helper.kill().expect("kills the helper");
        }
        let status = helper.wait().expect("reaps the helper");
        assert!(killed || status.success(), "round {round}: the helper failed: {status}");

        let mut installation = Installation::open(ShippedDir(shipped.clone()), &root).expect("opens");
        if installation.version() == 2 {
            assert_starts_on(&installation, &shipped, &second, 2);
            if killed {
                after += 1
            } else {
                finished += 1
            }
            installation.confirm_launch().expect("confirms");
            afresh();
        } else {
            assert!(killed, "round {round}: the install finished on release {}", installation.version());
            assert_starts_on(&installation, &shipped, &first, 1);
            before += 1;
            // Killed with some of release 2 already in place under the root.
            if listing(&root).iter().any(|name| name.starts_with("packs/")) {
                mid += 1;
            }
        }
    }
    println!(
        "{ROUNDS} rounds over a {whole:?} install: {before} killed before the switch ({mid} with packs half \
         made), {after} killed after it, {finished} finished"
    );
    assert!(mid >= 5, "too few kills landed mid-install to prove anything");
    assert!(after + finished >= 5, "too few installs finished");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--installer") => {
            return installer(Path::new(&args[2]), Path::new(&args[3]), Path::new(&args[4]));
        }
        // Libtest's own questions: this has no ignored tests to run or list.
        Some("--ignored" | "--list") => return ExitCode::SUCCESS,
        _ => {}
    }
    let dir = TestDir::new("install_kill");
    kill_loop(&dir);
    ExitCode::SUCCESS
}
