//! The runner as a process, on directories of its own: what `cargo xtask replay` and CI's job do.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sapphire_gameplay::replay::{ARCHITECTURE, ChecksumFile, Recorder, Replay, checksums_file_name};
use sapphire_gameplay::{ActorActions, Buttons};

/// A replay directory and an output directory of this test's own, gone when it ends.
struct Suite(PathBuf);

impl Suite {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("sapphire_replay_cli_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("replays")).expect("a temp dir");
        Self(root)
    }

    fn dir(&self) -> PathBuf {
        self.0.join("replays")
    }

    fn out(&self) -> PathBuf {
        self.0.join("out")
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir().join(name)
    }

    fn this_architecture(&self) -> PathBuf {
        self.file(&checksums_file_name(ARCHITECTURE))
    }

    fn run(&self, command: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sapphire_replay"))
            .arg(command)
            .args(args)
            .arg("--dir")
            .arg(self.dir())
            .arg("--out")
            .arg(self.out())
            .output()
            .expect("the runner starts")
    }

    fn check(&self, args: &[&str]) -> Output {
        self.run("check", args)
    }

    /// Records a replay of the melee scenario.
    fn record(&self, name: &str, script: &str, ticks: u32) {
        let ticks = ticks.to_string();
        let output =
            self.run("record", &[name, "--scenario", "melee", "--script", script, "--ticks", &ticks]);
        assert!(output.status.success(), "{}", log(&output));
    }
}

impl Drop for Suite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn log(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stderr), String::from_utf8_lossy(&output.stdout))
}

fn assert_passes(output: &Output) {
    assert!(output.status.success(), "{}", log(output));
}

fn assert_fails_saying(output: &Output, parts: &[&str]) {
    let log = log(output);
    assert!(!output.status.success(), "it passed:\n{log}");
    assert!(!log.contains("panicked"), "{log}");
    for part in parts {
        assert!(log.contains(part), "it doesn't say {part:?}:\n{log}");
    }
}

/// Edits one replay's line of a checksum file.
fn edit_line(path: &Path, tick: u32, edit: impl Fn(&mut String)) {
    let text = fs::read_to_string(path).expect("written");
    let prefix = format!("{tick} ");
    let edited: Vec<String> = text
        .lines()
        .map(|line| {
            let mut line = line.to_owned();
            if line.starts_with(&prefix) {
                edit(&mut line);
            }
            line
        })
        .collect();
    fs::write(path, edited.join("\n") + "\n").expect("writes");
}

/// A hex digit, changed.
fn flip_hex(line: &mut String, at: usize) {
    let new = if line.as_bytes()[at] == b'0' { "1" } else { "0" };
    line.replace_range(at..=at, new);
}

const OTHER: &str = "checksums.another_architecture.txt";

#[test]
fn update_touches_only_this_architectures_file() {
    let suite = Suite::new("update");
    suite.record("duel", "melee-play", 300);
    let good = fs::read_to_string(suite.this_architecture()).expect("written");
    let replay = fs::read(suite.file("duel.sreplay")).expect("written");
    let other =
        "architecture another_architecture\n\nreplay duel 1\n0 0000000000000001\n1 0000000000000002\n";
    fs::write(suite.file(OTHER), other).expect("writes");
    assert_passes(&suite.check(&[]));

    // Its last checkpoint moves: a run now differs.
    edit_line(&suite.this_architecture(), 300, |line| flip_hex(line, 4));
    assert_fails_saying(&suite.check(&[]), &["duel", "first differs at tick 300", "--update"]);
    assert_eq!(fs::read_to_string(suite.file(OTHER)).expect("there"), other, "a check writes nothing here");

    let output = suite.check(&["--update"]);
    assert_passes(&output);
    let said = log(&output);
    assert!(said.contains(&checksums_file_name(ARCHITECTURE)), "{said}");
    assert!(said.contains("alone: no other architecture's file is touched"), "{said}");
    assert!(said.contains("first differs at tick 300"), "it says what moved: {said}");
    assert_eq!(fs::read_to_string(suite.this_architecture()).expect("there"), good);
    assert_eq!(fs::read_to_string(suite.file(OTHER)).expect("there"), other, "the other's isn't touched");
    assert_eq!(fs::read(suite.file("duel.sreplay")).expect("there"), replay, "nor is the replay");

    // Nothing to do, and it says so.
    let output = suite.check(&["--update"]);
    assert_passes(&output);
    assert!(log(&output).contains("already holds these values"), "{}", log(&output));
    assert_passes(&suite.check(&[]));
}

/// A difference in the world, from the actions in the file: the runner names the tick they took
/// effect, whether a fingerprint or a checkpoint is what sees it.
#[test]
fn a_failure_names_the_first_tick_that_differs() {
    let suite = Suite::new("divergence");
    suite.record("duel", "melee-sweep", 200);
    let path = suite.file("duel.sreplay");
    let original = Replay::decode(&fs::read(&path).expect("written")).expect("reads");
    for changed in [0_usize, 30, 59, 60, 119, 199] {
        let mut recorder =
            Recorder::new(original.scenario(), original.config(), original.tables_hash()).expect("good");
        for (tick, mut actions) in original.actions().enumerate() {
            if tick == changed {
                let (actor, was) = actions.iter().next().expect("the script moves everyone");
                actions
                    .set(actor, ActorActions { buttons: was.buttons | Buttons::JUMP, ..was })
                    .expect("room");
            }
            recorder.push(&actions).expect("short");
        }
        fs::write(&path, recorder.finish().encode()).expect("writes");
        let expected = format!("first differs at tick {}", changed + 1);
        assert_fails_saying(&suite.check(&[]), &["duel", "diverged on", &expected]);
    }
}

#[test]
fn a_missing_file_fails_with_the_values_and_they_bootstrap_it() {
    let suite = Suite::new("bootstrap");
    suite.record("duel", "melee-play", 90);
    suite.record("spar", "idle", 90);
    let good = fs::read_to_string(suite.this_architecture()).expect("written");
    fs::remove_file(suite.this_architecture()).expect("removes");
    fs::remove_dir_all(suite.out()).expect("removes");

    let output = suite.check(&[]);
    let file_name = checksums_file_name(ARCHITECTURE);
    let last_line_of_values = good.lines().last().expect("values");
    assert_fails_saying(
        &output,
        &[
            "Nothing is recorded",
            "duel, spar",
            "gh run download",
            "replay-checksums-",
            &file_name,
            last_line_of_values,
        ],
    );

    // What CI uploads is the file to commit, and with it in place the suite passes.
    let uploaded = suite.out().join(&file_name);
    assert_eq!(fs::read_to_string(&uploaded).expect("written"), good);
    fs::copy(&uploaded, suite.this_architecture()).expect("copies");
    assert_passes(&suite.check(&[]));
}

#[test]
fn a_changed_move_hash_refuses_until_update_stamps_it() {
    let suite = Suite::new("moves");
    suite.record("duel", "melee-play", 90);
    let path = suite.file("duel.sreplay");
    let good = fs::read(&path).expect("written");
    let mut replay = Replay::decode(&good).expect("reads");
    let hash = replay.tables_hash();
    replay.set_tables_hash(hash ^ 1);
    fs::write(&path, replay.encode()).expect("writes");
    fs::write(suite.file(OTHER), "architecture another_architecture\n").expect("writes");

    assert_fails_saying(
        &suite.check(&[]),
        &["duel", "changed since this replay was recorded", "re-record", "--update"],
    );
    assert_ne!(fs::read(&path).expect("there"), good, "a check leaves the file as it was");

    let output = suite.check(&["--update"]);
    assert_passes(&output);
    assert!(log(&output).contains("stamped the new move data's hash"), "{}", log(&output));
    assert_eq!(fs::read(&path).expect("there"), good, "only the hash moved, back");
    assert_eq!(fs::read_to_string(suite.file(OTHER)).expect("there"), "architecture another_architecture\n");
    assert_passes(&suite.check(&[]));
}

#[test]
fn a_crafted_replay_fails_cleanly_and_the_rest_still_run() {
    let suite = Suite::new("crafted");
    suite.record("good", "idle", 30);
    fs::write(suite.file("bad.sreplay"), b"SREPLAY\0not really").expect("writes");
    fs::write(suite.file("nameless no.sreplay"), b"").expect("writes");
    let output = suite.check(&[]);
    assert_fails_saying(&output, &["FAILED bad:", "ok good:", "FAILED nameless no:", "can't name a replay"]);
    // The file CI uploads still holds what ran.
    let uploaded = suite.out().join(checksums_file_name(ARCHITECTURE));
    let file = ChecksumFile::parse(&fs::read_to_string(uploaded).expect("written")).expect("reads");
    assert!(file.get("good").is_some());
    assert!(file.get("bad").is_none());
}

#[test]
fn checksums_with_no_replay_fail_until_updated() {
    let suite = Suite::new("stale");
    suite.record("gone", "idle", 30);
    suite.record("kept", "idle", 30);
    fs::remove_file(suite.file("gone.sreplay")).expect("removes");
    assert_fails_saying(&suite.check(&[]), &["gone", "no replay file"]);
    // One named replay says nothing about the rest.
    assert_passes(&suite.check(&["--replay", "kept"]));
    let output = suite.check(&["--update"]);
    assert_passes(&output);
    assert!(log(&output).contains("its replay is gone"), "{}", log(&output));
    assert_passes(&suite.check(&[]));
    let file = ChecksumFile::parse(&fs::read_to_string(suite.this_architecture()).expect("written"));
    assert!(file.expect("reads").get("gone").is_none());
}

#[test]
fn a_checksum_file_for_another_architecture_or_that_doesnt_read_fails() {
    let suite = Suite::new("wrong");
    suite.record("duel", "idle", 30);
    let good = fs::read_to_string(suite.this_architecture()).expect("written");

    let elsewhere = good.replace(&format!("architecture {ARCHITECTURE}"), "architecture elsewhere");
    fs::write(suite.this_architecture(), elsewhere).expect("writes");
    assert_fails_saying(&suite.check(&[]), &["says it's for elsewhere"]);

    fs::write(suite.this_architecture(), "not checksums\n").expect("writes");
    assert_fails_saying(&suite.check(&[]), &["line 1"]);

    // An update replaces either.
    assert_passes(&suite.check(&["--update"]));
    assert_eq!(fs::read_to_string(suite.this_architecture()).expect("there"), good);
}

#[test]
fn one_replay_can_be_run_or_updated_alone() {
    let suite = Suite::new("only");
    suite.record("first", "idle", 30);
    suite.record("second", "melee-play", 30);
    let good = fs::read_to_string(suite.this_architecture()).expect("written");
    // Spoil both ends; the second replay is listed after the first, by name.
    edit_line(&suite.this_architecture(), 30, |line| flip_hex(line, 4));
    let spoiled = fs::read_to_string(suite.this_architecture()).expect("written");

    assert_fails_saying(&suite.check(&["--replay", "second"]), &["second", "tick 30"]);
    assert_passes(&suite.check(&["--replay", "second", "--update"]));
    let partly = fs::read_to_string(suite.this_architecture()).expect("written");
    assert_ne!(partly, spoiled);
    assert_ne!(partly, good, "the first is as it was");
    assert_fails_saying(&suite.check(&[]), &["first", "tick 30"]);
    assert_passes(&suite.check(&["--replay", "first", "--update"]));
    assert_eq!(fs::read_to_string(suite.this_architecture()).expect("there"), good);

    assert_fails_saying(&suite.check(&["--replay", "none"]), &["no replay `none`", "first"]);
}

#[test]
fn an_empty_suite_fails_rather_than_passing_nothing() {
    let suite = Suite::new("empty");
    assert_fails_saying(&suite.check(&[]), &["no replays in"]);
}

/// The file holds other replays' values, so a recording doesn't write over one it can't read.
#[test]
fn recording_into_a_checksum_file_that_doesnt_read_is_refused() {
    let suite = Suite::new("spoiled");
    fs::write(suite.this_architecture(), "not checksums\n").expect("writes");
    let args = ["new", "--scenario", "melee", "--script", "idle", "--ticks", "30"];
    assert_fails_saying(&suite.run("record", &args), &["line 1"]);
    assert_eq!(fs::read_to_string(suite.this_architecture()).expect("there"), "not checksums\n");
    assert!(!suite.file("new.sreplay").exists());
}

#[test]
fn recording_refuses_what_it_cant_do() {
    let suite = Suite::new("refusals");
    suite.record("taken", "idle", 30);
    let replay = fs::read(suite.file("taken.sreplay")).expect("written");
    let record = |name: &str, scenario: &str, script: &str, ticks: &str| {
        suite.run("record", &[name, "--scenario", scenario, "--script", script, "--ticks", ticks])
    };
    assert_fails_saying(&record("taken", "melee", "idle", "30"), &["exists", "delete it first"]);
    assert_eq!(fs::read(suite.file("taken.sreplay")).expect("there"), replay);
    assert_fails_saying(&record("../up", "melee", "idle", "30"), &["can't name a replay"]);
    assert_fails_saying(&record("new", "none", "idle", "30"), &["no scenario `none`", "melee"]);
    assert_fails_saying(&record("new", "melee", "none", "30"), &["no script `none`", "melee-sweep"]);
    assert_fails_saying(&record("new", "melee", "idle", "0"), &["at least one tick"]);
    assert_fails_saying(&record("new", "melee", "idle", "many"), &["--ticks expects a number"]);
    assert!(!suite.file("new.sreplay").exists());
}

#[test]
fn a_seed_goes_in_the_file_and_moves_the_checksums() {
    let suite = Suite::new("seed");
    let record = |name: &str, seed: &str| {
        let args = [name, "--scenario", "melee", "--script", "melee-sweep", "--ticks", "60", "--seed", seed];
        assert_passes(&suite.run("record", &args));
    };
    record("one", "1");
    record("two", "2");
    let seed_of = |name: &str| {
        let path = suite.file(name);
        Replay::decode(&fs::read(path).expect("written")).expect("reads").config().seed
    };
    assert_eq!((seed_of("one.sreplay"), seed_of("two.sreplay")), (1, 2));
    let file = ChecksumFile::parse(&fs::read_to_string(suite.this_architecture()).expect("written"));
    let file = file.expect("reads");
    assert_ne!(file.get("one").expect("recorded").end(), file.get("two").expect("recorded").end());
    assert_passes(&suite.check(&[]));
}

#[test]
fn it_lists_what_there_is() {
    let suite = Suite::new("list");
    suite.record("duel", "idle", 30);
    fs::write(suite.file("bad.sreplay"), b"nope").expect("writes");
    let output = suite.run("list", &[]);
    assert_passes(&output);
    let said = log(&output);
    for part in
        ["melee:", "melee-sweep (1200 ticks)", "duel: 30 ticks of `melee`", "bad: this isn't a replay"]
    {
        assert!(said.contains(part), "{part:?}:\n{said}");
    }
}

/// The files in the repository read, whichever architecture this is, and agree with each other.
#[test]
fn the_committed_suite_reads() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays");
    let mut replays = Vec::new();
    let mut checksum_files = Vec::new();
    for entry in fs::read_dir(&dir).expect("committed") {
        let path = entry.expect("an entry").path();
        let name = path.file_name().and_then(|name| name.to_str()).expect("a UTF-8 name").to_owned();
        if let Some(name) = name.strip_suffix(".sreplay") {
            let replay = Replay::decode(&fs::read(&path).expect("reads")).expect("decodes");
            replays.push((name.to_owned(), replay));
        } else {
            let text = fs::read_to_string(&path).expect("reads");
            let file = ChecksumFile::parse(&text).expect("parses");
            assert_eq!(
                name,
                checksums_file_name(file.architecture()),
                "{name} is named for its architecture"
            );
            assert_eq!(file.to_text(), text, "{name} is as a run writes it");
            checksum_files.push(file);
        }
    }
    assert!(!replays.is_empty());
    assert!(!checksum_files.is_empty());
    for (name, replay) in &replays {
        for file in &checksum_files {
            if let Some(recorded) = file.get(name) {
                assert_eq!(recorded.ticks(), replay.ticks(), "{name} on {}", file.architecture());
            }
        }
    }
    for file in &checksum_files {
        for name in file.names() {
            assert!(replays.iter().any(|(replay, _)| replay == name), "{name}");
        }
    }
}
