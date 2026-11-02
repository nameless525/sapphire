//! The sandbox as a process: a scripted run with an argument it doesn't know exits with an error
//! before it touches a GPU, so a misspelled pin can't bootstrap a golden of the wrong picture
//! (review DV-9). Headless: `--capture` and `--bench` start no window or display.

use std::process::{Command, Output};

/// Runs the sandbox with `args`, its content directory pointed nowhere: refusing the arguments
/// must come first.
fn sandbox(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sapphire_sandbox"))
        .args(args)
        .env("SAPPHIRE_CONTENT_DIR", "no-such-content")
        .output()
        .expect("the sandbox starts")
}

/// Asserts the run failed, refusing `flags` by name as it started.
fn assert_refused(output: &Output, flags: &[&str]) {
    let log =
        format!("{}{}", String::from_utf8_lossy(&output.stderr), String::from_utf8_lossy(&output.stdout));
    assert!(!output.status.success(), "it exited with {}:\n{log}", output.status);
    let failure = log.lines().find(|line| line.contains("app initialization failed"));
    let failure = failure.unwrap_or_else(|| panic!("its start didn't fail:\n{log}"));
    for flag in flags {
        assert!(failure.contains(flag), "the failure doesn't name {flag}:\n{log}");
    }
}

#[test]
fn a_capture_with_an_unknown_argument_fails_and_writes_nothing() {
    let capture = std::env::temp_dir().join(format!("sapphire_sandbox_refused_{}.png", std::process::id()));
    let path = capture.to_str().expect("a UTF-8 temp path");
    let output = sandbox(&["--capture", path, "--size", "16x16", "--plain-psot", "--shadow", "off"]);
    assert_refused(&output, &["--plain-psot", "--shadow"]);
    assert!(!capture.exists(), "no capture from a run it refused");
}

/// Spec §5.15: capture and bench never open an audio device, and say so rather than draw silently.
#[test]
fn a_capture_or_bench_asked_to_play_a_cue_fails() {
    let capture = std::env::temp_dir().join(format!("sapphire_sandbox_cue_{}.png", std::process::id()));
    let path = capture.to_str().expect("a UTF-8 temp path");
    let output = sandbox(&["--capture", path, "--size", "16x16", "--cue", "hit"]);
    assert_refused(&output, &["--cue"]);
    assert!(!capture.exists(), "no capture from a run it refused");
    let output = sandbox(&["--bench", "1", "--size", "16x16", "--cue", "hit"]);
    assert_refused(&output, &["--cue"]);
}

#[test]
fn a_bench_with_an_unknown_argument_fails() {
    let output = sandbox(&["--bench", "1", "--size", "16x16", "--plain-psot"]);
    assert_refused(&output, &["--plain-psot"]);
}
