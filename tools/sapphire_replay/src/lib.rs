//! The headless replay runner (spec §9, gates 2 and 4, m2-plan D2): what `cargo xtask replay` and CI's
//! `Replays` job run.
//!
//! Owns the suite's files on disk: it plays every `<name>.sreplay` in a directory against that
//! directory's `checksums.<architecture>.txt`, fails a run that differs or allocates in a step,
//! rewrites this architecture's file on `--update`, and records new replays from scripts.
//!
//! Doesn't own what a replay is or how one runs: that's `sapphire_gameplay::replay`, which the sandbox
//! steps beside its renderer through the same code. Its dependency tree holds no SDL, wgpu or platform
//! layer, which `cargo xtask check-layers` enforces (`[simulation]`'s `runners`), so the job builds
//! quickly and needs no GPU.
//!
//! The binary installs the counting allocator, and the suite refuses to run without one, because
//! gate 4 reads zero then, whatever the steps do.

mod output_paths;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail, ensure};
use sapphire_core::alloc;
use sapphire_gameplay::replay::{
    ChecksumFile, PlayError, Played, Player, RecordedTrail, Recording, Replay, SCRIPTS, Scenario,
    checksums_file_name, find_scenario, find_script, is_valid_name,
};

/// A replay file's extension.
pub const EXTENSION: &str = "sreplay";

/// Most lines of values a failing run prints: the file holds them all.
const PRINTED_LINES: usize = 200;

/// What [`check`] runs.
pub struct Suite<'a> {
    /// The directory of replays and checksum files: `tests/replays/`.
    pub dir: &'a Path,
    /// Where this architecture's file goes as it should be, for CI to upload. Must be
    /// disjoint from `dir`, including existing symlink aliases.
    pub out: &'a Path,
    /// Take this run's values, rather than comparing with what's recorded.
    pub update: bool,
    /// Only this replay.
    pub only: Option<&'a str>,
    pub scenarios: &'a [Scenario],
    /// Whose checksums these are: `sapphire_gameplay::replay::ARCHITECTURE`.
    pub architecture: &'a str,
}

/// What a [`check`] found.
#[derive(Debug, Default)]
pub struct Outcome {
    /// One line per failure, naming its replay. Empty is a pass.
    pub failures: Vec<String>,
    /// Replays with nothing recorded for this architecture.
    pub unrecorded: Vec<String>,
    /// How many replays ran to their end.
    pub played: usize,
}

fn fail_without_counting() -> anyhow::Result<()> {
    ensure!(
        alloc::is_installed(),
        "no counting allocator is installed, so gate 4 would read zero whatever a step does"
    );
    ensure!(alloc::PER_THREAD_COUNTS, "this target can't count one thread's allocations");
    Ok(())
}

/// The names of the replays in `dir`, sorted.
fn replay_names(dir: &Path) -> anyhow::Result<Vec<String>> {
    let entries = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    let mut names = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == EXTENSION)
            && let Some(name) = path.file_stem().and_then(|stem| stem.to_str())
        {
            names.push(name.to_owned());
        }
    }
    names.sort_unstable();
    Ok(names)
}

fn replay_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.{EXTENSION}"))
}

fn describe_steps(played: &Played) -> String {
    let stats = played.stats;
    format!(
        "{} allocations in the step, mean step {:.1} µs, longest {:.1} µs",
        stats.allocations,
        stats.mean_step().as_secs_f64() * 1e6,
        stats.longest_step.as_secs_f64() * 1e6,
    )
}

/// What's recorded for this architecture, which is nothing if there's no file. An `Err` of the outer
/// result is a file that can't be read at all; of the inner, one that isn't a checksum file for this
/// architecture.
fn read_recorded(path: &Path, architecture: &str) -> anyhow::Result<Result<ChecksumFile, String>> {
    let name = path.display();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Ok(ChecksumFile::new(architecture)));
        }
        Err(error) => return Err(error).with_context(|| format!("reading {name}")),
    };
    Ok(match ChecksumFile::parse(&text) {
        Ok(file) if file.architecture() == architecture => Ok(file),
        Ok(file) => {
            Err(format!("{name} says it's for {}, and its name says {architecture}", file.architecture()))
        }
        Err(error) => Err(format!("{name}: {error}")),
    })
}

/// Plays one replay to its end. With `update`, a replay whose move data has changed takes the new
/// hash first. An `Err` is that replay's failure.
fn play_one(suite: &Suite<'_>, name: &str) -> Result<Played, String> {
    if !is_valid_name(name) {
        return Err("that can't name a replay: 1 to 64 of a-z, 0-9, - and _".to_owned());
    }
    let path = replay_path(suite.dir, name);
    let bytes = fs::read(&path).map_err(|error| format!("reading it: {error}"))?;
    let replay = Replay::decode(&bytes).map_err(|error| error.to_string())?;
    let started = match Player::new(replay, suite.scenarios) {
        Err(PlayError::MovesChanged { current, .. }) if suite.update => {
            // Only the exceptional migration path needs the replay again. Normal checks move
            // its action stream into the player without a second full copy.
            let mut replay = Replay::decode(&bytes).map_err(|error| error.to_string())?;
            replay.set_tables_hash(current);
            fs::write(&path, replay.encode()).map_err(|error| format!("writing it: {error}"))?;
            eprintln!(
                "{name}: stamped the new move data's hash, {current:016x}: its actions are as they were"
            );
            Player::new(replay, suite.scenarios)
        }
        started => started,
    };
    let mut player = started.map_err(|error| error.to_string())?;
    player.run_to_end();
    let played = player.finish();
    played.allocation_gate().map_err(|error| error.to_string())?;
    Ok(played)
}

/// Runs the suite and reports what it found. An `Err` is a suite that couldn't run at all.
pub fn check(suite: &Suite<'_>) -> anyhow::Result<Outcome> {
    fail_without_counting()?;
    output_paths::disjoint(suite.out, &[suite.dir]).map_err(anyhow::Error::msg)?;
    let mut outcome = Outcome::default();
    let file_name = checksums_file_name(suite.architecture);
    output_paths::disjoint(&suite.out.join(&file_name), &[suite.dir]).map_err(anyhow::Error::msg)?;
    let file_path = suite.dir.join(&file_name);
    // One that doesn't read is a failure, until an update replaces it.
    let recorded = read_recorded(&file_path, suite.architecture)?.unwrap_or_else(|problem| {
        if suite.update {
            eprintln!("replacing what's there: {problem}");
        } else {
            outcome.failures.push(problem);
        }
        ChecksumFile::new(suite.architecture)
    });
    let names = replay_names(suite.dir)?;
    let selected: Vec<&String> =
        names.iter().filter(|name| suite.only.is_none_or(|only| only == name.as_str())).collect();
    if let Some(only) = suite.only
        && selected.is_empty()
    {
        bail!("there's no replay `{only}` in {}: it has {names:?}", suite.dir.display());
    }
    ensure!(!selected.is_empty(), "no replays in {}", suite.dir.display());

    let mut current = recorded;
    for name in selected {
        let played = match play_one(suite, name) {
            Ok(played) => played,
            Err(failure) => {
                outcome.failures.push(format!("{name}: {failure}"));
                continue;
            }
        };
        outcome.played += 1;
        let verdict = compare(suite, name, current.get(name), &played);
        current.set(name, RecordedTrail::from_trail(&played.trail));
        let summary = format!(
            "{} ticks, ends at {:016x}, {}",
            played.trail.ticks(),
            played.trail.end(),
            describe_steps(&played)
        );

        match verdict {
            Verdict::Same => eprintln!("ok {name}: {summary}"),
            Verdict::Moved(how) => eprintln!("moved {name}: {how}; {summary}"),
            Verdict::Unrecorded => {
                outcome.unrecorded.push(name.clone());
                outcome
                    .failures
                    .push(format!("{name}: nothing is recorded for it on {}", suite.architecture));
            }
            Verdict::Differs(failure) => outcome.failures.push(format!("{name}: {failure}")),
        }
    }

    // A checksum with no replay behind it is one nobody is checking.
    if suite.only.is_none() {
        let stale: Vec<String> = current
            .names()
            .filter(|name| !names.iter().any(|other| other == name))
            .map(str::to_owned)
            .collect();
        for name in stale {
            current.remove(&name);
            if suite.update {
                eprintln!("{name}: its replay is gone, so its checksums are too");
            } else {
                outcome.failures.push(format!("{name}: {file_name} lists it, and there's no replay file"));
            }
        }
    }

    fs::create_dir_all(suite.out).with_context(|| format!("creating {}", suite.out.display()))?;
    let text = current.to_text();
    let out_path = suite.out.join(&file_name);
    fs::write(&out_path, &text).with_context(|| format!("writing {}", out_path.display()))?;

    if suite.update {
        update_file(&file_path, &file_name, suite.architecture, &text)?;
    } else if outcome.failures.is_empty() {
        eprintln!("{} replays reach their checksums on {}", outcome.played, suite.architecture);
    }
    Ok(outcome)
}

/// How a run compares with what's recorded for it.
enum Verdict {
    Same,
    /// `--update` took new values, and this says from where they moved.
    Moved(String),
    Unrecorded,
    Differs(String),
}

fn compare(suite: &Suite<'_>, name: &str, recorded: Option<&RecordedTrail>, played: &Played) -> Verdict {
    let architecture = suite.architecture;
    let ticks = played.trail.ticks();
    let Some(recorded) = recorded else {
        return if suite.update { Verdict::Moved("newly recorded".to_owned()) } else { Verdict::Unrecorded };
    };
    let difference = recorded.first_difference(&played.trail);
    if suite.update {
        return match difference {
            Some(difference) => Verdict::Moved(format!("{difference}")),
            None if recorded.ticks() != ticks => {
                Verdict::Moved(format!("it was {} ticks long", recorded.ticks()))
            }
            None => Verdict::Same,
        };
    }
    let review = "If that's intended, run `cargo xtask replay --update` and review what moved. If it isn't, \
                  the same actions no longer give the same world";
    if let Some(difference) = difference {
        return Verdict::Differs(format!("diverged on {architecture}: {difference}. {review}."));
    }
    if recorded.ticks() != ticks {
        return Verdict::Differs(format!(
            "{name}'s checksums are for {} ticks, and it now runs for {ticks}. {review}.",
            recorded.ticks()
        ));
    }
    Verdict::Same
}

/// Writes this architecture's file, if it changed, and says that nothing else did.
fn update_file(path: &Path, file_name: &str, architecture: &str, text: &str) -> anyhow::Result<()> {
    let before = fs::read_to_string(path).ok();
    if before.as_deref() == Some(text) {
        eprintln!("{file_name} already holds these values");
        return Ok(());
    }
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    eprintln!(
        "updated {file_name}, this architecture's ({architecture}) alone: no other architecture's file is \
         touched. Review the diff; CI's Replays job asks for any other architecture's new values."
    );
    Ok(())
}

/// What a failed run prints after its failures: where this run's values are, how to take them if the
/// change is intended, and, for anything with nothing recorded, the values themselves, as on x86-64
/// until CI's first run has made its file.
#[must_use]
pub fn values_help(outcome: &Outcome, out: &Path, architecture: &str) -> String {
    let file_name = checksums_file_name(architecture);
    let mut help = String::from("\n");
    if !outcome.unrecorded.is_empty() {
        help.push_str(&format!(
            "Nothing is recorded on {architecture} for: {}.\n",
            outcome.unrecorded.join(", ")
        ));
    }
    help.push_str(&format!(
        "This run's values are in {}, as the whole file should be, and CI uploads that directory.\n\
         To take them on a machine of this architecture: cargo xtask replay --update\n\
         To take them from a CI run, as x86-64's come from Linux:\n\
         \x20   gh run download <run> -n replay-checksums-ubuntu-24.04 -D <dir>\n\
         \x20   copy <dir>/{file_name} to tests/replays/{file_name}, and commit it\n",
        out.join(&file_name).display(),
    ));
    if !outcome.unrecorded.is_empty()
        && let Ok(text) = fs::read_to_string(out.join(&file_name))
    {
        help.push_str("\nThe values:\n");
        let lines = text.lines().count();
        for line in text.lines().take(PRINTED_LINES) {
            help.push_str(line);
            help.push('\n');
        }
        if lines > PRINTED_LINES {
            help.push_str(&format!("… and {} more lines, in the file\n", lines - PRINTED_LINES));
        }
    }
    help
}

/// What [`record`] makes.
pub struct Record<'a> {
    pub dir: &'a Path,
    /// A directory disjoint from `dir`, for this architecture's output checksum file.
    pub out: &'a Path,
    /// The new replay's name.
    pub name: &'a str,
    pub scenario: &'a str,
    pub script: &'a str,
    /// Ticks to record, instead of the script's own number.
    pub ticks: Option<u32>,
    /// The seed, instead of the scenario's.
    pub seed: Option<u64>,
    pub scenarios: &'a [Scenario],
    pub architecture: &'a str,
}

/// Records a new replay from a script, and this architecture's checksums for it. Refuses a name
/// that's taken: a replay may be a session someone played, which no script can make again.
pub fn record(request: &Record<'_>) -> anyhow::Result<()> {
    fail_without_counting()?;
    output_paths::disjoint(request.out, &[request.dir]).map_err(anyhow::Error::msg)?;
    let Record { dir, out, name, scenario, script, ticks, seed, scenarios, architecture } = *request;
    output_paths::disjoint(&out.join(checksums_file_name(architecture)), &[dir])
        .map_err(anyhow::Error::msg)?;
    ensure!(is_valid_name(name), "{name:?} can't name a replay: 1 to 64 of a-z, 0-9, - and _");
    let path = replay_path(dir, name);
    ensure!(!path.exists(), "{} exists: delete it first to replace it", path.display());
    let known: Vec<&str> = scenarios.iter().map(|scenario| scenario.name).collect();
    let scenario = find_scenario(scenarios, scenario)
        .with_context(|| format!("no scenario `{scenario}`: there's {known:?}"))?;
    let scripts: Vec<&str> = SCRIPTS.iter().map(|script| script.name.as_str()).collect();
    let script = find_script(script).with_context(|| format!("no script `{script}`: there's {scripts:?}"))?;

    let mut config = scenario.config;
    if let Some(seed) = seed {
        config.seed = seed;
    }
    let ticks = ticks.unwrap_or(script.ticks);
    ensure!(ticks > 0, "a replay needs at least one tick");
    let mut recording = Recording::new(scenario, config)?;
    recording.run_script(script, ticks)?;
    let (replay, played) = recording.finish();
    played.allocation_gate()?;

    // It must play back to what it just did, through its own bytes, before it's worth keeping.
    let bytes = replay.encode();
    let mut player = Player::new(Replay::decode(&bytes)?, scenarios)?;
    player.run_to_end();
    let replayed = player.finish();
    ensure!(replayed.trail == played.trail, "the recording doesn't play back to the same checksums");

    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let file_name = checksums_file_name(architecture);
    let file_path = dir.join(&file_name);
    // Before anything is written: it holds other replays' values, which a recording mustn't lose.
    let mut file = read_recorded(&file_path, architecture)?.map_err(anyhow::Error::msg)?;
    fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    file.set(name, RecordedTrail::from_trail(&played.trail));
    let text = file.to_text();
    fs::write(&file_path, &text).with_context(|| format!("writing {}", file_path.display()))?;
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    fs::write(out.join(&file_name), &text)?;

    let minutes = f64::from(replay.ticks()) / 3600.0;
    eprintln!(
        "recorded {name}: {} ticks of `{}` by `{}`, {} bytes ({:.0} a minute of play), ends at {:016x}, {}. \
         Added to {file_name}. Commit both; CI's Replays job asks for any other architecture's values.",
        replay.ticks(),
        scenario.name,
        script.name,
        bytes.len(),
        bytes.len() as f64 / minutes,
        played.trail.end(),
        describe_steps(&played),
    );
    Ok(())
}

/// Lists what there is to record from, and what's recorded.
pub fn list(dir: &Path, scenarios: &[Scenario]) -> anyhow::Result<()> {
    eprintln!("scenarios:");
    for scenario in scenarios {
        eprintln!("  {}: {}", scenario.name, scenario.about);
    }
    eprintln!("scripts:");
    for script in SCRIPTS.iter() {
        eprintln!("  {} ({} ticks): {}", script.name, script.ticks, script.about);
    }
    eprintln!("replays in {}:", dir.display());
    for name in replay_names(dir)? {
        let bytes = fs::read(replay_path(dir, &name))?;
        match Replay::decode(&bytes) {
            Ok(replay) => eprintln!(
                "  {name}: {} ticks of `{}`, {} bytes",
                replay.ticks(),
                replay.scenario(),
                bytes.len()
            ),
            Err(error) => eprintln!("  {name}: {error}"),
        }
    }
    Ok(())
}
