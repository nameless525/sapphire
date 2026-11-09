//! The simulation beside the renderer (m2-plan D2): `--replay` and `--record` in the real app, against
//! the same run with nothing beside it. A driver that changes a thread's float state, or a step that
//! leans on anything the renderer touches, shows here and nowhere else.
//!
//! Refusals come before any GPU work, so those run everywhere. The runs that draw need a GPU and cooked
//! content, so they're `#[ignore]`d: `cargo xtask content`, then
//! `cargo test -p sapphire_sandbox --test replays -- --ignored --test-threads=1`. CI's `App tests` job
//! runs them on lavapipe.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sapphire_gameplay::replay::{
    ARCHITECTURE, ChecksumFile, FORMAT_VERSION, Played, Player, RecordedTrail, Recording, Replay, SCENARIOS,
    checksums_file_name, melee_sweep,
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
        .env("SAPPHIRE_LOG", "info")
        .env_remove("RUST_LOG")
        .env("SAPPHIRE_CONTENT_DIR", content)
        .output()
        .expect("the sandbox starts")
}

fn log_of(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stderr), String::from_utf8_lossy(&output.stdout))
}

/// The melee scenario on the sweep script, as a headless recording does it.
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
    let mut player = Player::new(Replay::decode(bytes).expect("reads"), &SCENARIOS).expect("starts");
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
    let (replay, played) = record(30, melee_sweep);
    let bytes = suite_of(&temp, "good", &replay, &played);
    let later = temp.path("later.sreplay");
    let mut newer = bytes.clone();
    newer[8] = FORMAT_VERSION as u8 + 1;
    fs::write(&later, &newer).expect("writes");
    assert_refused(
        &run(&["--replay", &temp.text("later.sreplay")]),
        &capture,
        &[&format!("version {}", FORMAT_VERSION + 1), "record it again"],
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

    // The arena plays only its own scenario's fights: another's actors aren't standing in the square.
    fs::remove_file(temp.path(&checksums_file_name(ARCHITECTURE))).expect("there");
    let elsewhere = run(&["--arena", "--replay", &good]);
    assert_refused(&elsewhere, &capture, &["--arena", "arena's colliders", "`melee`"]);

    // A fight with no file plays a script it has, and fights only in the arena.
    let bad_fight = run(&["--arena", "--fight", "--script", "none"]);
    assert_refused(&bad_fight, &capture, &["no script `none`", "fight-back"]);
    assert_refused(&run(&["--fight", "--script", "idle"]), &capture, &["--fight", "--arena"]);

    // What a recording can get wrong.
    let out = temp.text("recorded.sreplay");
    let bad_scenario = run(&["--record", &out, "--scenario", "none", "--script", "idle"]);
    assert_refused(&bad_scenario, &capture, &["no scenario `none`", "melee"]);
    let bad_script = run(&["--record", &out, "--scenario", "melee", "--script", "none"]);
    assert_refused(&bad_script, &capture, &["no script `none`", "melee-sweep"]);
    assert!(!temp.path("recorded.sreplay").exists());
    let taken = run(&["--record", &good, "--scenario", "melee", "--script", "idle"]);
    assert_refused(&taken, &capture, &["exists", "delete it first"]);
    assert_eq!(fs::read(temp.path("good.sreplay")).expect("there"), bytes);
    let nowhere = temp.text("no-such-directory/recorded.sreplay");
    let unwritable = run(&["--record", &nowhere, "--scenario", "melee", "--script", "idle"]);
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
    let (replay, _) = record(120, melee_sweep);
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

/// The views of a fight (M2 T1) look at every tick of it, through the facade, and the fight is the same
/// one: the app compares each checksum with a headless run's, and each view's lines reach the frame.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_combat_views_change_nothing_the_checksums_see() {
    let temp = Temp::new("views");
    let (replay, _) = record(120, melee_sweep);
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
        "--debug-draw",
        "combat",
    ]);
    assert_reached(&output, 120, played.trail.end());
    assert!(capture.exists());
}

/// The arena's replay in the arena, as a golden scene plays it, from the committed file: the app stands
/// its character on the actor and reaches the checksum the headless runner reaches.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_arena_plays_its_committed_replay() {
    let replay = workspace().join("tests").join("replays").join("arena-walk.sreplay");
    let played = headless(&fs::read(&replay).expect("committed"));
    let temp = Temp::new("arena");
    let capture = temp.path("drawn.png");
    let output = run_app(&[
        "--capture",
        capture.to_str().expect("UTF-8"),
        "--arena",
        "--replay",
        replay.to_str().expect("UTF-8"),
    ]);
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    assert!(log.contains(&format!("checksum={:016x}", played.trail.end())), "{log}");
    assert!(log.contains("allocations=0"), "{log}");
    assert!(capture.exists(), "{log}");
}

/// A replay that goes between zones (M3 W4), in the arena, as its goldens play it: each trip holds the fight
/// under the wipe while the next zone comes in, and every tick reaches the headless run's checksum, with no
/// allocation in a fixed step. A bench plays it to its end, however long the loads held it, and so does a
/// capture of its end, whose held steps draw the frames a trip waits on (M3 Z2: it never ended before).
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_arena_goes_between_zones_and_reaches_its_checksums() {
    let replay = workspace().join("tests").join("replays").join("zone-trip.sreplay");
    let played = headless(&fs::read(&replay).expect("committed"));
    let output = run_app(&[
        "--bench",
        "1",
        "--draw-every",
        "8",
        "--size",
        "64x36",
        "--arena",
        "--replay",
        replay.to_str().expect("UTF-8"),
    ]);
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    assert!(log.contains(&format!("checksum={:016x}", played.trail.end())), "{log}");
    assert!(log.contains("allocations=0"), "{log}");
    let arrivals = log.lines().filter(|line| line.contains("arrived in a zone")).count();
    assert_eq!(arrivals, 5, "each entry a trip: {log}");
    assert!(!log.contains("a trip went wrong"), "{log}");

    let temp = Temp::new("zone_trip_end");
    let capture = temp.path("end.png");
    let output = run_app(&[
        "--capture",
        capture.to_str().expect("UTF-8"),
        "--size",
        "64x36",
        "--arena",
        "--replay",
        replay.to_str().expect("UTF-8"),
    ]);
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    assert!(log.contains(&format!("checksum={:016x}", played.trail.end())), "{log}");
    assert_eq!(log.lines().filter(|line| line.contains("arrived in a zone")).count(), 5, "{log}");
    assert!(capture.exists(), "{log}");
}

/// The rule that governs a zone's streaming (M3 W2): the simulation streams nothing. The arena's replay
/// walks through the town with the town's zone off, on as its tier has it, on another budget and radius
/// by which cells come and go all the way, and on a phone's settings, and every run reaches every one of
/// a headless run's checksums, with no allocation in a fixed step.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_town_streaming_changes_nothing_the_checksums_see() {
    let replay = workspace().join("tests").join("replays").join("arena-walk.sreplay");
    let played = headless(&fs::read(&replay).expect("committed"));
    let temp = Temp::new("streaming");
    let capture = temp.path("drawn.png");
    let runs: [&[&str]; 6] = [
        &["--stream", "off"],
        &[],
        &["--stream", "1:8"],
        &["--stream", "1000:48"],
        &["--tier", "mobile-low"],
        &["--tier", "mobile-low", "--stream", "4:16"],
    ];
    for extra in runs {
        let mut args = vec![
            "--capture",
            capture.to_str().expect("UTF-8"),
            "--arena",
            "--replay",
            replay.to_str().expect("UTF-8"),
        ];
        args.extend_from_slice(extra);
        let output = run_app(&args);
        let log = log_of(&output);
        assert!(output.status.success(), "{extra:?}: {log}");
        assert!(log.contains(&format!("checksum={:016x}", played.trail.end())), "{extra:?}: {log}");
        assert!(log.contains("allocations=0"), "{extra:?}: {log}");
    }
}

/// The Lantern Ward's gate cutscene (M3 Q3) on the arena's stage stood in the ward's court, from the
/// committed replay that watches it: on a desktop's tier and a phone's, the app plays it between the steps
/// its recording started it between, holds the fight through it, lets it end by itself, and reaches every
/// one of a headless run's checksums with no allocation in a fixed step. No frame of it ends with a place
/// in reach of the view still to come, the cuts' places asked for ahead of them included.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_wards_gate_plays_between_its_steps_and_streams_nothing_late() {
    let replay = workspace().join("tests").join("replays").join("ward-court-watched.sreplay");
    let played = headless(&fs::read(&replay).expect("committed"));
    let temp = Temp::new("ward");
    let capture = temp.path("drawn.png");
    for tier in ["desktop-low", "mobile-low"] {
        let output = run_app(&[
            "--capture",
            capture.to_str().expect("UTF-8"),
            "--arena",
            "--replay",
            replay.to_str().expect("UTF-8"),
            "--tier",
            tier,
        ]);
        let log = log_of(&output);
        assert!(output.status.success(), "{tier}: {log}");
        assert!(log.contains(&format!("checksum={:016x}", played.trail.end())), "{tier}: {log}");
        assert!(log.contains("allocations=0"), "{tier}: {log}");
        let started = log.lines().find(|line| line.contains("a timeline started")).expect("the gate started");
        assert!(started.contains("id=0") && started.contains("tick=30"), "{tier}: {started}");
        let ended = log.lines().find(|line| line.contains("a timeline ended")).expect("the gate ended");
        assert!(ended.contains("state=Finished") && ended.contains("stream_misses=0"), "{tier}: {ended}");
    }
}

/// The player's ultimate (M3 Q3b) in the arena, from the committed replay that fires it: on a desktop's tier
/// and a phone's, its cinematic starts on the move's first step, never holds the fight, can't be skipped and
/// so ends by itself, and the run reaches a headless run's checksums with no allocation in a fixed step and
/// nothing streamed late.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_players_ultimate_plays_its_cinematic_on_its_steps() {
    let replay = workspace().join("tests").join("replays").join("arena-ultimate.sreplay");
    let played = headless(&fs::read(&replay).expect("committed"));
    let temp = Temp::new("ultimate");
    let capture = temp.path("drawn.png");
    for tier in ["desktop-low", "mobile-low"] {
        let output = run_app(&[
            "--capture",
            capture.to_str().expect("UTF-8"),
            "--arena",
            "--replay",
            replay.to_str().expect("UTF-8"),
            "--tier",
            tier,
        ]);
        let log = log_of(&output);
        assert!(output.status.success(), "{tier}: {log}");
        assert!(log.contains(&format!("checksum={:016x}", played.trail.end())), "{tier}: {log}");
        assert!(log.contains("allocations=0"), "{tier}: {log}");
        let started = log.lines().find(|line| line.contains("a timeline started")).expect("it started");
        assert!(started.contains("id=0") && started.contains("tick=61"), "{tier}: {started}");
        let ended = log.lines().find(|line| line.contains("a timeline ended")).expect("it ended");
        assert!(ended.contains("state=Finished") && ended.contains("stream_misses=0"), "{tier}: {ended}");
    }
}

/// One `a fight ended` line of a log, as the numbers it gives.
struct FightEnded {
    ticks: u32,
    fell_at: Option<u32>,
    checksum: u64,
    line: String,
}

fn fights_ended(log: &str) -> Vec<FightEnded> {
    let field = |line: &str, name: &str| {
        let start = line.find(name).unwrap_or_else(|| panic!("no {name} in: {line}")) + name.len();
        line[start..].split_whitespace().next().expect("a value").to_owned()
    };
    log.lines()
        .filter(|line| line.contains("a fight ended"))
        .map(|line| {
            let fell_at = field(line, " fell_at=");
            let fell_at = match fell_at.as_str() {
                "None" => None,
                some => Some(
                    some.strip_prefix("Some(")
                        .and_then(|tick| tick.strip_suffix(')'))
                        .expect("a reported defeat tick")
                        .parse()
                        .expect("a number"),
                ),
            };
            FightEnded {
                ticks: field(line, " ticks=").parse().expect("a number"),
                fell_at,
                checksum: u64::from_str_radix(&field(line, " checksum="), 16).expect("hex"),
                line: line.to_owned(),
            }
        })
        .collect()
}

/// Runs the window's stock fight with the same script and end tick in two captures. The fixed bodies
/// participate in combat, so both runs load them from the same cook. A frame every eight ticks, animated,
/// prepared and recorded, must produce the same simulation results as drawing one frame at the end, through
/// two full fights and the first part of a third. One frame in [`FIGHT_DRAW_EVERY`] runs its graph, small:
/// what the simulation could see of the renderer is all on the CPU's side, and lavapipe takes over a second
/// to draw each of these frames at full size.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_fight_with_no_file_plays_beside_the_renderer_and_starts_over() {
    const CAPTURE_TICK: u32 = 4400;
    const DEFEAT_TICK: u32 = 1455;
    const RESTART_TICK: u32 = DEFEAT_TICK + 240;

    let temp = Temp::new("fight");
    let capture = |name, cadence: &[&str], drawn: u32| {
        let path = temp.path(name);
        let tick = CAPTURE_TICK.to_string();
        let mut args = vec![
            "--capture",
            path.to_str().expect("UTF-8"),
            "--capture-ticks",
            &tick,
            "--size",
            "320x180",
            "--arena",
            "--fight",
            "--script",
            "fight-back",
        ];
        args.extend_from_slice(cadence);
        let output = run_app(&args);
        let log = log_of(&output);
        assert!(output.status.success(), "{log}");
        assert!(path.exists(), "{log}");
        let captured = log.lines().find(|line| line.contains("captured frame")).expect("a capture line");
        assert!(captured.contains(&format!(" drawn={drawn} ")), "{captured}");
        let fights = fights_ended(&log);
        assert_eq!(fights.len(), 3, "two ended and one going on:\n{log}");
        for (number, fight) in fights.iter().enumerate() {
            assert!(fight.line.contains(&format!(" fight={} ", number + 1)), "{}", fight.line);
            assert!(fight.line.contains("allocations=0"), "{}", fight.line);
        }
        fights
    };

    let every = &["--capture-frames", "550", "--capture-step", "8", "--draw-every", FIGHT_DRAW_EVERY];
    // 549 frames before the captured one, every 50th from the first drawn.
    let frequent = capture("frequent.png", every, 12);
    let single = capture("single.png", &[], 1);
    for (fight, reference) in frequent.iter().zip(&single) {
        assert_eq!(
            (fight.ticks, fight.fell_at, fight.checksum),
            (reference.ticks, reference.fell_at, reference.checksum),
            "{}\n{}",
            fight.line,
            reference.line
        );
    }
    // Both completed fights end the same way, then the third starts from tick zero again.
    assert_eq!(frequent[0].checksum, frequent[1].checksum, "the second fight repeats the first");
    for fight in &frequent[..2] {
        assert!(fight.line.contains("enemy_health=0"), "{}", fight.line);
        assert!(!fight.line.contains("player_health=0"), "{}", fight.line);
        assert_eq!(fight.fell_at, Some(DEFEAT_TICK), "{}", fight.line);
        assert_eq!(fight.ticks, RESTART_TICK, "{}", fight.line);
    }
    assert_eq!(frequent[2].fell_at, None, "{}", frequent[2].line);
    assert_eq!(frequent[2].ticks, CAPTURE_TICK - 2 * RESTART_TICK, "{}", frequent[2].line);
}

/// A long run on a tier that draws every other tick (M2 Z1's soak): `--offscreen` takes two ticks a frame
/// on Mobile-Low, as a phone does, plays a fight's sound through the mixer with no device, and logs what
/// a long run needs to say whether anything grows.
#[cfg(feature = "dev")]
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn offscreen_runs_mobile_low_at_its_own_rate() {
    let replay = workspace().join("tests").join("replays").join("arena-fight-still.sreplay");
    let played = headless(&fs::read(&replay).expect("committed"));
    let output = run_app(&[
        "--offscreen",
        "--frames",
        "150",
        "--size",
        "320x180",
        "--tier",
        "mobile-low",
        "--arena",
        "--fight",
        "--script",
        "idle",
        "--cues",
        "--hud",
        "live",
        "--live-link",
        "off",
    ]);
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    let fights = fights_ended(&log);
    assert_eq!(fights.len(), 1, "{log}");
    assert_eq!(fights[0].ticks, 300, "two a frame: {}", fights[0].line);
    assert_eq!(fights[0].checksum, played.trail.at(300).expect("within it"), "{}", fights[0].line);
    let stats = log.lines().find(|line| line.contains("frame stats")).expect("a stats line");
    for field in ["mean_ms=", "allocs_per_frame=", "live_bytes="] {
        assert!(stats.contains(field), "{field}: {stats}");
    }
}

/// The same with `--offscreen`, as an agent runs a fight, for as long as it's left running, every frame
/// beside the renderer and one in [`FIGHT_DRAW_EVERY`] drawn. Only `dev` builds have it.
#[cfg(feature = "dev")]
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn an_agent_runs_a_fight_offscreen_by_its_script() {
    let replay = workspace().join("tests").join("replays").join("arena-fight-still.sreplay");
    let played = headless(&fs::read(&replay).expect("committed"));
    let output = run_app(&[
        "--offscreen",
        "--frames",
        "300",
        "--size",
        "320x180",
        "--draw-every",
        FIGHT_DRAW_EVERY,
        "--arena",
        "--fight",
        "--script",
        "idle",
    ]);
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    let fights = fights_ended(&log);
    assert_eq!(fights.len(), 1, "{log}");
    assert_eq!(fights[0].ticks, 300, "{}", fights[0].line);
    assert_eq!(fights[0].checksum, played.trail.at(300).expect("within it"), "{}", fights[0].line);
}

/// A bench of a fight plays it whole, three times, starting over between: first as it comes, then for
/// CPU times, then with GPU timing on. Each reaches the headless checksums and none goes past the end.
/// Its trace has a line a frame, each with the tick it fell on, and its fixed steps allocate nothing.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_bench_of_a_fight_plays_it_whole_beside_the_renderer() {
    let temp = Temp::new("bench");
    let (replay, _) = record(120, melee_sweep);
    let played = headless(&replay.encode());
    suite_of(&temp, "duel", &replay, &played);

    let trace = temp.path("trace.csv");
    let output = run_app(&[
        "--bench",
        "1",
        "--replay",
        &temp.text("duel.sreplay"),
        "--size",
        "320x180",
        "--bench-trace",
        trace.to_str().expect("UTF-8"),
    ]);
    assert_reached(&output, 120, played.trail.end());
    let log = log_of(&output);
    assert!(!log.contains("the replay ended early"), "{log}");
    assert_eq!(log.matches("replay reached its recorded checksums").count(), 3, "{log}");
    for row in ["row=\"cpu frame\"", "row=\"simulation\"", "row=\"animation and ik\""] {
        assert!(log.contains(row), "{row}: {log}");
    }

    let trace = fs::read_to_string(&trace).expect("written");
    let mut lines = trace.lines();
    let header: Vec<&str> = lines.next().expect("a header").split(',').collect();
    let column = |name: &str| header.iter().position(|field| *field == name).expect(name);
    let (phase, tick, step_allocations) = (column("phase"), column("tick"), column("step_allocations"));
    let rows: Vec<Vec<&str>> = lines.map(|line| line.split(',').collect()).collect();
    for name in ["first", "cpu", "gpu"] {
        let ticks: Vec<&str> = rows.iter().filter(|row| row[phase] == name).map(|row| row[tick]).collect();
        assert_eq!(ticks.first().copied(), Some("1"), "{name} starts at the fight's first tick");
        assert_eq!(ticks.last().copied(), Some("120"), "{name} reaches its last");
        assert_eq!(ticks.len(), 120, "{name}: a frame a tick on this tier");
    }
    for row in rows.iter().filter(|row| row[phase] != "first") {
        assert_eq!(row[step_allocations], "0", "rule R6, at tick {}", row[tick]);
    }
    // A timing flag can accompany partial pass timings without a complete frame span.
    // This fixture plays one fight per phase. GPU frame 0 restarts it and is excluded from the summary.
    let (frame, timed, span) = (column("frame"), column("gpu_timed"), column("gpu_span"));
    let span_samples = rows
        .iter()
        .filter(|row| row[phase] == "gpu" && row[frame] != "0" && row[timed] == "1" && !row[span].is_empty())
        .count();
    let frame_row = log.lines().find(|line| line.contains("row=\"gpu frame\""));
    assert_eq!(frame_row.is_some(), span_samples > 0, "{log}");
    if let Some(frame_row) = frame_row {
        assert!(frame_row.contains(&format!(" samples={span_samples} ")), "{log}");
    }
}

/// An encounter's waves come in the middle of a fight, each warrior in a fixed step of its own, on the real
/// show's stage, and the gate holds there as it does in a fight whose everyone stands at the start.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_wave_comes_on_in_a_bench_with_no_heap_in_its_steps() {
    let temp = Temp::new("waves");
    let trace = temp.path("trace.csv");
    let replay = workspace().join("tests").join("replays").join("arena-waves.sreplay");
    let output = sandbox(
        &[
            "--bench",
            "1",
            "--bench-trace",
            trace.to_str().expect("UTF-8"),
            "--arena",
            "--replay",
            replay.to_str().expect("UTF-8"),
            "--tier",
            "desktop-low",
            "--size",
            "64x36",
            "--cues",
            "--hud",
            "live",
            "--no-hot-reload",
            "--live-link",
            "off",
        ],
        &content(),
    );
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    assert_eq!(log.matches("replay reached its recorded checksums").count(), 3, "{log}");

    let trace = fs::read_to_string(&trace).expect("written");
    let mut lines = trace.lines();
    let header: Vec<&str> = lines.next().expect("a header").split(',').collect();
    let column = |name: &str| header.iter().position(|field| *field == name).expect(name);
    let (tick, step_allocations) = (column("tick"), column("step_allocations"));
    let rows: Vec<Vec<&str>> = lines.map(|line| line.split(',').collect()).collect();
    assert!(rows.iter().any(|row| row[tick] == "900"), "it reached the last wave");
    // Tick 1's are what stands the warriors in the first pass, since a bench's first follow of a fight is after
    // its first step, and any arena fight's start over in the others.
    for row in rows.iter().filter(|row| row[tick] != "1") {
        assert_eq!(row[step_allocations], "0", "rule R6, at tick {}", row[tick]);
    }
}

/// An encounter someone walks into (M3 W5), on each tier's settings: its fighters stand by on the way in, so its
/// trigger finds them ready and no step stands one; no pipeline builds after the load; and while it runs the zone's
/// streaming is held and asks for nothing. That a wave then takes no heap is the stage's own test
/// (`an_encounter_warms_on_the_way_in_and_its_waves_take_no_heap`).
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn an_encounter_walked_into_is_warm_before_its_trigger_fires() {
    for tier in ["desktop-low", "mobile-low"] {
        let replay = workspace().join("tests").join("replays").join("square-street.sreplay");
        let output = sandbox(
            &[
                "--bench",
                "1",
                "--draw-every",
                DRAW_EVERY,
                "--arena",
                "--replay",
                replay.to_str().expect("UTF-8"),
                "--tier",
                tier,
                "--size",
                "64x36",
                "--cues",
                "--hud",
                "live",
                "--no-hot-reload",
                "--live-link",
                "off",
            ],
            &content(),
        );
        let log = log_of(&output);
        assert!(output.status.success(), "{tier}: {log}");
        // A bench that draws one frame in many plays the fight once.
        assert_eq!(log.matches("replay reached its recorded checksums").count(), 1, "{tier}: {log}");
        let started: Vec<&str> = log.lines().filter(|line| line.contains("an encounter started")).collect();
        assert_eq!(started.len(), 1, "{tier}: {log}");
        for line in started {
            assert!(line.contains("warm=true"), "{tier}: {line}");
        }
        assert!(log.contains("an encounter holds the zone's streaming"), "{tier}: {log}");
        let stats = log.lines().find(|line| line.contains("warmed a fight's encounters")).expect("logged");
        for part in ["started_cold: 0", "stood_in_step: 0", "asked_while_held: 0"] {
            assert!(stats.contains(part), "{tier}: {stats}");
        }
        assert!(!stats.contains("held_frames: 0,"), "{tier}: {stats}");
        let rows: Vec<&str> = log.lines().filter(|line| line.contains("fight pipelines")).collect();
        assert!(!rows.is_empty(), "{tier}: {log}");
        for row in rows {
            assert!(row.contains("first_use=0"), "{tier}: {row}");
        }
    }
}

/// A walk through a zone as a bench reads it (M3 Z1, exit test 8): the ward's walk stands on the ward's own ground
/// and streams it, reaching every checksum with nothing built after the load, and says what it held at its start,
/// its most and its end; and each of the trip's five goes through every step, from its exit to its wipe gone.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_bench_of_a_zone_walk_reads_its_memory_and_times_each_trip() {
    let bench = |replay: &str, tier: &str| {
        let output = inventory_bench(replay, tier, None);
        let log = log_of(&output);
        assert!(output.status.success(), "{replay} on {tier}: {log}");
        assert_eq!(
            log.matches("replay reached its recorded checksums").count(),
            1,
            "{replay} on {tier}: {log}"
        );
        let memory = log.lines().find(|line| line.contains("zone memory")).expect("logged");
        for part in ["resident_mib=", "mesh_pool_mib=", "entities=", "process_mib="] {
            assert!(memory.contains(part), "{replay} on {tier}: {memory}");
        }
        log
    };
    for tier in ["desktop-low", "mobile-low"] {
        let log = bench("walk-the-ward", tier);
        let pipelines = log.lines().find(|line| line.contains("fight pipelines")).expect("logged");
        assert!(pipelines.contains("first_use=0"), "{tier}: {pipelines}");
        assert!(!log.contains("zone trip, from its exit"), "{tier}: the ward's walk takes no exit");
    }
    let log = bench("zone-trip", "desktop-low");
    let trips: Vec<&str> = log.lines().filter(|line| line.contains("zone trip, from its exit")).collect();
    assert_eq!(trips.len(), 5, "{log}");
    for trip in trips {
        for step in ["covered=", "swapped=", "first_cells=", "clear="] {
            assert!(trip.contains(step), "{trip}");
        }
    }
}

/// A bench of a committed replay on `tier`'s settings, as a phone's or a laptop's run has them: not [`PINS`],
/// because the count is of what those settings build. Every tick of the replay steps, and every frame prepares and
/// records, but only one frame in [`DRAW_EVERY`] runs its graph: lavapipe takes over a second to draw a frame here,
/// and under one to build the load's pipelines. So a build anywhere but a graph's passes counts on the frame it
/// happens, and a build in its passes on the next frame drawn.
fn inventory_bench(replay: &str, tier: &str, without: Option<&str>) -> Output {
    let replay = workspace().join("tests").join("replays").join(format!("{replay}.sreplay"));
    let mut args = vec![
        "--bench",
        "1",
        "--draw-every",
        DRAW_EVERY,
        "--arena",
        "--replay",
        replay.to_str().expect("UTF-8"),
        "--tier",
        tier,
        "--size",
        "64x36",
        "--cues",
        "--hud",
        "live",
        "--no-hot-reload",
        "--live-link",
        "off",
    ];
    if let Some(entry) = without {
        args.extend(["--inventory-without", entry]);
    }
    sandbox(&args, &content())
}

/// One frame in this many runs its graph: 17 of the 3,147 frames two tiers' whole plays below take, and 2 of the 300
/// of each zone walk.
const DRAW_EVERY: &str = "200";

/// A long fight's frames beside the renderer, one in this many drawn: each is animated, prepared and recorded,
/// and the rest of lavapipe's second a frame is lost.
const FIGHT_DRAW_EVERY: &str = "50";

/// Every shipped tier, whose settings each build a set of their own: the rule is no first-use build on any of them.
const TIERS: [&str; 4] = ["desktop-high", "desktop-low", "mobile-high", "mobile-low"];

/// The arena's stage draws through this and nothing else does, on every tier.
const UNIQUE_KEY: &str = "mesh environment flags 0x0 Float32";

/// A bench of `replay` on `tier` builds nothing after its load, and says so.
fn assert_no_misses(replay: &str, tier: &str) {
    let output = inventory_bench(replay, tier, None);
    let log = log_of(&output);
    assert!(output.status.success(), "{replay} on {tier}: {log}");
    let rows: Vec<&str> = log.lines().filter(|line| line.contains("fight pipelines")).collect();
    assert!(!rows.is_empty(), "{replay} on {tier}: {log}");
    for row in rows {
        for part in ["first_use=0", "rebuilt=0"] {
            assert!(row.contains(part), "{replay} on {tier}: {row}");
        }
    }
}

/// Exit test 7: after a load, no frame of the recorded walk builds a pipeline, nor of a recorded fight, whose hits
/// draw what a walk doesn't, on every tier's settings; and take one entry of what the load declared out and the
/// same bench fails, naming what it had to build. A key is missed at the load, so it costs no frames on any tier.
/// What no table lists doesn't vary by tier, so it runs on one.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_load_leaves_no_pipeline_to_build_and_a_missing_entry_fails_naming_it() {
    for tier in TIERS {
        assert_no_misses("arena-walk", tier);
        assert_no_misses("arena-fight-won", tier);
        assert_no_misses("zone-trip", tier);
        assert_inventory_miss("arena-walk", tier, UNIQUE_KEY, UNIQUE_KEY);
    }
    for (entry, named) in [("skinning", "skinning: "), ("sky", "sky: "), ("ui", "ui: ")] {
        assert_inventory_miss("arena-walk", "mobile-low", entry, named);
    }
    for (entry, named) in [("effects", "effects: "), ("post", "post: ")] {
        assert_inventory_miss("arena-fight-won", "mobile-low", entry, named);
    }
}

/// Exit test 7 where exit test 1 walks (M3 C1): a zone found by its stem declares its keys and its probe grid
/// when it opens, so no frame of walking it builds a pipeline on any tier's settings; and leave its grid out
/// and the same bench fails, naming the ambient pass. `--bench` adds its warm-up to the frames it asks for. Every
/// frame walks, streams, prepares and records, but only one in [`DRAW_EVERY`] runs its graph: on lavapipe the
/// High tiers' 300 drawn frames, twice each, ran past CI's 45 minutes.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn walking_a_zone_builds_no_pipeline_and_a_grid_left_out_is_named() {
    let bench = |tier: &str, without: &[&'static str]| {
        let mut args = vec![
            "--bench",
            "240",
            "--zone",
            "lantern_ward",
            "--zone-walk",
            "tour",
            "--draw-every",
            DRAW_EVERY,
            "--tier",
            tier,
            "--size",
            "64x36",
            "--no-hot-reload",
            "--live-link",
            "off",
        ];
        args.extend(without);
        sandbox(&args, &content())
    };
    for tier in TIERS {
        let output = bench(tier, &[]);
        let log = log_of(&output);
        assert!(output.status.success(), "{tier}: {log}");
        let row =
            log.lines().find(|line| line.contains("zone walk")).unwrap_or_else(|| panic!("{tier}: {log}"));
        for part in ["first_use=0", "rebuilt=0"] {
            assert!(row.contains(part), "{tier}: {row}");
        }
        let streaming =
            log.lines().rfind(|line| line.contains(" streaming ")).unwrap_or_else(|| panic!("{tier}: {log}"));
        assert!(streaming.contains("failed=0"), "{tier}: {streaming}");
        let output = bench(tier, &["--inventory-without", "ambient"]);
        let log = log_of(&output);
        assert!(!output.status.success(), "{tier} without its grid: it passed: {log}");
        assert!(log.contains("pipeline inventory missed") && log.contains("ambient: "), "{tier}: {log}");
    }
}

/// A capture declares no inventory: what its load uploads builds as it goes, and none of that stalls a frame, so
/// it counts as prewarmed and its log names no first use. What a frame builds still counts: a debug view, built
/// on the frame that first shows it.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_capture_counts_its_load_as_prewarmed_and_names_what_a_frame_built() {
    let temp = Temp::new("capture_first_use");
    let capture = |name: &str, extra: &[&str]| {
        let path = temp.text(name);
        let mut args = vec!["--capture", &path, "--size", "64x36", "--capture-ticks", "2", "--arena"];
        args.extend(extra);
        let output = sandbox(&args, &content());
        let log = log_of(&output);
        assert!(output.status.success(), "{name}: {log}");
        let row = log.lines().find(|line| line.contains("captured frame"));
        let row = row.unwrap_or_else(|| panic!("{name}: {log}")).to_owned();
        (row, log)
    };
    let (row, log) = capture("arena.png", &[]);
    assert!(row.contains(" first_use=0") && !row.contains(" prewarmed=0 "), "{row}");
    assert!(!log.contains("built on first use"), "{log}");
    let (row, log) = capture("wireframe.png", &["--debug-view", "wireframe"]);
    assert!(!row.contains(" first_use=0"), "{row}");
    assert!(log.contains("built on first use") && log.contains("family=debug"), "{log}");
}

/// A bench on `tier` with `entry` left out fails, saying `named`.
fn assert_inventory_miss(replay: &str, tier: &str, entry: &str, named: &str) {
    let output = inventory_bench(replay, tier, Some(entry));
    let log = log_of(&output);
    assert!(!output.status.success(), "{tier} without {entry}: it passed: {log}");
    let failure = log.lines().position(|line| line.contains("pipeline inventory missed"));
    let failure = failure.unwrap_or_else(|| panic!("{tier} without {entry}: {log}"));
    let names = log.lines().skip(failure).any(|line| line.contains(named));
    assert!(names, "{tier} without {entry} doesn't say {named:?}: {log}");
}

/// A fight with no file has no end of its own: a bench measures the frames it asked for in each phase,
/// starting over between them, with the real mixer and no device, and the whole frame's rows.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_bench_of_a_fight_with_no_file_starts_it_over_between_phases() {
    let output = run_app(&[
        "--bench",
        "8",
        "--size",
        "160x90",
        "--arena",
        "--fight",
        "--script",
        "fight-back",
        "--cues",
        "--hud",
        "live",
    ]);
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    assert_eq!(log.matches("a fight ended").count(), 3, "two start-overs and the end: {log}");
    assert!(log.contains("measured=[8, 8, 8]"), "{log}");
    assert!(log.contains("row=\"audio update\""), "{log}");
}

/// The check has teeth, and a capture of another fight is never written: these checksums come from
/// actions that differ at one tick, and the app names the tick where the worlds part.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn a_run_that_leaves_its_checksums_fails_naming_the_tick_and_draws_nothing() {
    let temp = Temp::new("diverged");
    let (replay, _) = record(120, melee_sweep);
    let (_, other) = record(120, |tick: u64, cast: &[ActorId]| {
        let mut actions = melee_sweep(tick, cast);
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
    let (replay, played) = record(120, melee_sweep);
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
        "melee",
        "--script",
        "melee-sweep",
    ]);
    let log = log_of(&output);
    assert!(output.status.success(), "{log}");
    assert!(log.contains("recorded a replay"), "{log}");
    let written = fs::read(temp.path("recorded.sreplay")).expect("written");
    let (replay, played) = record(90, melee_sweep);
    assert_eq!(written, replay.encode());
    assert_eq!(headless(&written).trail, played.trail);
}

/// A replay of the committed suite, against its committed checksums where this architecture has them.
#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_committed_suite_runs_in_the_app() {
    let dir = workspace().join("tests").join("replays");
    let replay = dir.join("melee-sweep.sreplay");
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
