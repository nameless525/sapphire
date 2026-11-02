//! The simulation beside the renderer (m2-plan D2): `--replay` and `--record` in the real app, against
//! the same run with nothing beside it. A driver that changes a thread's float state, or a step that
//! leans on anything the renderer touches, shows here and nowhere else.
//!
//! Refusals come before any GPU work, so those run everywhere. The runs that draw need a GPU and cooked
//! content, so they're `#[ignore]`d: `cargo xtask content`, then
//! `cargo test -p sapphire_sandbox --test replays -- --ignored --test-threads=1`. CI's `GPU tests` job
//! runs them on lavapipe.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sapphire_gameplay::replay::{
    ARCHITECTURE, ChecksumFile, Played, Player, RecordedTrail, Recording, Replay, SCENARIOS,
    checksums_file_name, drill_sweep,
};
use sapphire_gameplay::{ActorActions, ActorId, Buttons, TickActions};

/// A directory of this test's own, gone when it ends.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("sapphire_sandbox_replays_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn text(&self, name: &str) -> String {
        self.path(name).to_str().expect("a UTF-8 temp path").to_owned()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("apps/<app>").to_owned()
}

fn sandbox(args: &[&str], content: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sapphire_sandbox"))
        .args(args)
        .env("SAPPHIRE_CONTENT_DIR", content)
        .output()
        .expect("the sandbox starts")
}

fn log_of(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stderr), String::from_utf8_lossy(&output.stdout))
}

/// The drill's scenario on the sweep script, as a headless recording does it.
fn record(ticks: u32, source: impl Fn(u64, &[ActorId]) -> TickActions) -> (Replay, Played) {
    let scenario = &SCENARIOS[0];
    let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
    let mut source = source;
    for _ in 0..ticks {
        recording.step(&mut source).expect("short");
    }
    recording.finish()
}

/// Plays a replay's bytes with nothing beside it.
fn headless(bytes: &[u8]) -> Played {
    let mut player = Player::new(Replay::decode(bytes).expect("reads"), SCENARIOS).expect("starts");
    player.run_to_end();
    player.finish()
}

/// A replay and its checksums, from a headless run, in a directory as `tests/replays/` lays them out.
fn suite_of(temp: &Temp, name: &str, replay: &Replay, played: &Played) -> Vec<u8> {
    let bytes = replay.encode();
    fs::write(temp.path(&format!("{name}.sreplay")), &bytes).expect("writes");
    let mut file = ChecksumFile::new(ARCHITECTURE);
    file.set(name, RecordedTrail::from_trail(&played.trail));
    fs::write(temp.path(&checksums_file_name(ARCHITECTURE)), file.to_text()).expect("writes");
    bytes
}

/// Asserts the run failed as it started, with a message holding each of `parts`, and drew nothing.
fn assert_refused(output: &Output, capture: &Path, parts: &[&str]) {
    let log = log_of(output);
    assert!(!output.status.success(), "it exited with {}:\n{log}", output.status);
    let failure = log.lines().find(|line| line.contains("app initialization failed"));
    let failure = failure.unwrap_or_else(|| panic!("its start didn't fail:\n{log}"));
    for part in parts {
        assert!(failure.contains(part), "the failure doesn't say {part:?}:\n{log}");
    }
    assert!(!log.contains("panicked"), "{log}");
    assert!(!capture.exists(), "no capture from a run it refused");
}

/// Every row fails before the GPU: their content directory doesn't exist, and a good file would fail
/// on that later and differently.
#[test]
fn a_replay_that_cant_run_is_refused_before_any_gpu_work() {
    let temp = Temp::new("refused");
    let capture = temp.path("never.png");
    let no_content = temp.path("no-such-content");
    let run = |extra: &[&str]| {
        let capture = capture.to_str().expect("UTF-8");
        let args = [&["--capture", capture, "--size", "16x16"], extra].concat();
        sandbox(&args, &no_content)
    };

    let missing = temp.text("missing.sreplay");
    assert_refused(&run(&["--replay", &missing]), &capture, &["--replay", "missing.sreplay"]);

    let garbage = temp.path("garbage.sreplay");
    fs::write(&garbage, b"not a replay").expect("writes");
    assert_refused(&run(&["--replay", &temp.text("garbage.sreplay")]), &capture, &["isn't a replay file"]);

    // A later format: the version is read before anything that may have moved.
    let (replay, played) = record(30, drill_sweep);
    let bytes = suite_of(&temp, "good", &replay, &played);
    let later = temp.path("later.sreplay");
    let mut newer = bytes.clone();
    newer[8] = 2;
    fs::write(&later, &newer).expect("writes");
    assert_refused(
        &run(&["--replay", &temp.text("later.sreplay")]),
        &capture,
        &["version 2", "record it again"],
    );

    // A move that changed since it was recorded.
    let mut moved = replay.clone();
    moved.set_tables_hash(replay.tables_hash() ^ 1);
    fs::write(temp.path("moved.sreplay"), moved.encode()).expect("writes");
    assert_refused(&run(&["--replay", &temp.text("moved.sreplay")]), &capture, &["re-record", "--update"]);

    // Past its end, and with frames starting before tick 0.
    let good = temp.text("good.sreplay");
    let past = run(&["--replay", &good, "--capture-ticks", "31"]);
    assert_refused(&past, &capture, &["past the replay's end", "tick 30"]);
    let before = run(&["--replay", &good, "--capture-frames", "32", "--capture-step", "1"]);
    assert_refused(&before, &capture, &["start before tick 0"]);

    // The checksums beside it don't read.
    fs::write(temp.path(&checksums_file_name(ARCHITECTURE)), "not checksums\n").expect("writes");
    assert_refused(&run(&["--replay", &good]), &capture, &["checksums.", "line 1"]);

    // What a recording can get wrong.
    let out = temp.text("recorded.sreplay");
    let bad_scenario = run(&["--record", &out, "--scenario", "none", "--script", "idle"]);
    assert_refused(&bad_scenario, &capture, &["no scenario `none`", "drill"]);
    let bad_script = run(&["--record", &out, "--scenario", "drill", "--script", "none"]);
    assert_refused(&bad_script, &capture, &["no script `none`", "drill-sweep"]);
    assert!(!temp.path("recorded.sreplay").exists());
    let taken = run(&["--record", &good, "--scenario", "drill", "--script", "idle"]);
    assert_refused(&taken, &capture, &["exists", "delete it first"]);
    assert_eq!(fs::read(temp.path("good.sreplay")).expect("there"), bytes);
    let nowhere = temp.text("no-such-directory/recorded.sreplay");
    let unwritable = run(&["--record", &nowhere, "--scenario", "drill", "--script", "idle"]);
    assert_refused(&unwritable, &capture, &["directory doesn't exist"]);
}

/// The cheapest frames the renderer draws: the renderer is beside the simulation here, not under test.
/// A `dev` build keeps off the file system and the default live link port, which others may hold.
const PINS: [&str; 13] = [
    "--size",
    "64x36",
    "--aa",
    "msaa",
    "--shadows",
    "off",
    "--fog",
    "off",
    "--plain-sky",
    "--plain-post",
    "--no-hot-reload",
    "--live-link",
    "off",
];

fn content() -> PathBuf {
    workspace().join("target").join("content").join("desktop")
}

fn run_app(args: &[&str]) -> Output {
    sandbox(&[args, &PINS].concat(), &content())
}

fn assert_reached(output: &Output, ticks: u32, end: u64) {
    let log = log_of(output);
    assert!(output.status.success(), "it exited with {}:\n{log}", output.status);
    let line = log.lines().find(|line| line.contains("replay reached its recorded checksums"));
    let line = line.unwrap_or_else(|| panic!("no replay line in its log:\n{log}"));
    for part in [format!("ticks={ticks}"), format!("checksum={end:016x}"), "allocations=0".to_owned()] {
        assert!(line.contains(&part), "{part} isn't in: {line}");
    }
}

/// The property: every tick drawn, with the renderer and its driver on this thread between steps, and
/// the app reaches each of a headless run's checksums. The app compares them itself, per tick, against
/// a file a headless run wrote.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_capture_beside_the_renderer_reaches_the_headless_checksums() {
    let temp = Temp::new("capture");
    let (replay, _) = record(120, drill_sweep);
    let played = headless(&replay.encode());
    suite_of(&temp, "duel", &replay, &played);

    let capture = temp.path("drawn.png");
    let output = run_app(&[
        "--capture",
        capture.to_str().expect("UTF-8"),
        "--capture-frames",
        "120",
        "--capture-step",
        "1",
        "--replay",
        &temp.text("duel.sreplay"),
    ]);
    assert_reached(&output, 120, played.trail.end());
    assert!(capture.exists());
}

/// A bench steps in its frame path, with GPU timing on for the last of it, and goes on past the end.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_bench_beside_the_renderer_reaches_the_headless_checksums() {
    let temp = Temp::new("bench");
    let (replay, _) = record(120, drill_sweep);
    let played = headless(&replay.encode());
    suite_of(&temp, "duel", &replay, &played);

    let output = run_app(&["--bench", "90", "--replay", &temp.text("duel.sreplay")]);
    assert_reached(&output, 120, played.trail.end());
    assert!(log_of(&output).contains("the replay ended early"), "{}", log_of(&output));
}

/// The check has teeth, and a capture of another fight is never written: these checksums come from
/// actions that differ at one tick, and the app names the tick where the worlds part.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_run_that_leaves_its_checksums_fails_naming_the_tick_and_draws_nothing() {
    let temp = Temp::new("diverged");
    let (replay, _) = record(120, drill_sweep);
    let (_, other) = record(120, |tick: u64, cast: &[ActorId]| {
        let mut actions = drill_sweep(tick, cast);
        if tick == 49 {
            let was = actions.get(cast[1]);
            let pressed = ActorActions { buttons: was.buttons | Buttons::JUMP, ..was };
            actions.set(cast[1], pressed).expect("room");
        }
        actions
    });
    suite_of(&temp, "duel", &replay, &other);

    let capture = temp.path("never.png");
    let output =
        run_app(&["--capture", capture.to_str().expect("UTF-8"), "--replay", &temp.text("duel.sreplay")]);
    let log = log_of(&output);
    assert!(!output.status.success(), "{log}");
    assert!(log.contains("first differs at tick 50"), "{log}");
    assert!(!capture.exists(), "{log}");
}

/// A capture part-way through is held to as far as it got, and nothing is recorded to compare with
/// is a warning, for a replay outside the suite.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_capture_part_way_is_held_to_as_far_as_it_got() {
    let temp = Temp::new("partway");
    let (replay, played) = record(120, drill_sweep);
    suite_of(&temp, "duel", &replay, &played);
    let capture = temp.path("drawn.png");
    let capture = capture.to_str().expect("UTF-8");
    let duel = temp.text("duel.sreplay");
    let output = run_app(&["--capture", capture, "--capture-ticks", "45", "--replay", &duel]);
    assert_reached(&output, 45, played.trail.at(45).expect("within the run"));

    fs::remove_file(temp.path(&checksums_file_name(ARCHITECTURE))).expect("removes");
    let output = run_app(&["--capture", capture, "--replay", &duel]);
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    assert!(log.contains("nothing recorded for it"), "{log}");
}

/// What the app writes is what a headless recording writes, byte for byte, with a frame on every tick.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_recording_in_the_app_is_a_headless_recording() {
    let temp = Temp::new("record");
    let capture = temp.path("drawn.png");
    let output = run_app(&[
        "--capture",
        capture.to_str().expect("UTF-8"),
        "--capture-ticks",
        "90",
        "--capture-frames",
        "90",
        "--capture-step",
        "1",
        "--record",
        &temp.text("recorded.sreplay"),
        "--scenario",
        "drill",
        "--script",
        "drill-sweep",
    ]);
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    assert!(log.contains("recorded a replay"), "{log}");
    let written = fs::read(temp.path("recorded.sreplay")).expect("written");
    let (replay, played) = record(90, drill_sweep);
    assert_eq!(written, replay.encode());
    assert_eq!(headless(&written).trail, played.trail);
}

/// A replay of the committed suite, against its committed checksums where this architecture has them.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_committed_suite_runs_in_the_app() {
    let dir = workspace().join("tests").join("replays");
    let replay = dir.join("drill-sweep.sreplay");
    let played = headless(&fs::read(&replay).expect("committed"));
    let temp = Temp::new("committed");
    let capture = temp.path("drawn.png");
    let output = run_app(&[
        "--capture",
        capture.to_str().expect("UTF-8"),
        "--replay",
        replay.to_str().expect("UTF-8"),
    ]);
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    assert!(log.contains(&format!("checksum={:016x}", played.trail.end())), "{log}");
    assert!(log.contains("allocations=0"), "{log}");
}
