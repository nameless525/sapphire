//! The benches at scale, behind `cargo xtask bench-scale` (M5 Z1): one batch that measures everything exit tests
//! 1, 2, 5, 6 and 7 ask to be measured, and the packages' costs on and off beside exit test 3, on each tier, at any
//! commit, and one report of it for spec §3.8, with a verdict for each exit test and tier (M5 Z2).
//!
//! - **The manifest** ([`manifest`], `bench/scale.ron`) lists every run as data: its sandbox arguments, tiers,
//!   frames, rounds, what it's read for, a pair's variants and the baseline run it stands beside. Adding a run
//!   is a line of it.
//! - **The plan:** each run on each tier, each variant, each round is a unit; units are packed in order into
//!   sittings, each one tier's and none longer than a whole-machine hold by the units' stated lengths
//!   ([`manifest::ScaleBench::sittings`]). A pair's variants for a round share a sitting, their order turned each
//!   round, so neither always runs first or on a cooler machine.
//! - **The runner** ([`run`]) runs one sitting, the next with anything left unless told which, and never takes
//!   a gate slot itself: the orchestrator wraps each sitting in `slot --all --load 8`. Each unit writes
//!   `<unit>.log`, a trace where its run takes one, and `<unit>.status` with its exit, its time and the load at
//!   its start and end ([`status`]). A complete unit is skipped, so a batch cut short goes on where it stopped,
//!   and the folder is the commit's (`target/bench-scale/<commit>/`, never committed). Runs of counts alone
//!   (`--draw-every`) aren't timed and run apart, in an ordinary slot ([`Select::Counts`]).
//! - **Devices** (M5 Z2): a run naming `devices` walks on the iOS Simulator or the Android emulator instead
//!   ([`run_on_device`]), each unit through a launcher (`cargo xtask bench-scale launch`) on a device
//!   `cargo xtask bench-scale device` booted, into the same folder as `<run>.<tier>.<device>.r<n>`; its verdicts
//!   are lines of their own, by counts alone.
//! - **The report** ([`report`]) reads a folder's logs into Markdown.
//!
//! Nothing here reads a time it then compares: the bench reads its own frames, and the runner's wall clock only
//! bounds a unit and records how long it took.

pub mod log;
pub mod manifest;
pub mod report;
pub mod status;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sapphire_time::Stopwatch;

use self::manifest::{Cook, ScaleBench, Sitting, Smoke, Unit};
use self::status::Status;
use crate::scenes::{CONTENT_DIR_ENV, run_with_deadline};

/// The manifest's place, relative to the workspace root.
pub const MANIFEST: &str = "bench/scale.ron";

/// Where the runner finds and puts things.
pub struct Paths {
    /// The release `dev` sandbox (the district opens only in a `dev` build).
    pub sandbox: PathBuf,
    /// Each cook's folder: the shipped packs with the preset's district beside them.
    pub desktop: PathBuf,
    pub mobile: PathBuf,
    /// The replays `prepare` recorded, which `{replays}` in a run's arguments names.
    pub replays: PathBuf,
    /// The commit's folder: every unit's log, trace and status.
    pub out: PathBuf,
}

/// Which units a `run` runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Select {
    /// The first sitting with a unit left.
    Next,
    /// The sitting of this number, from 1.
    Sitting(usize),
    /// Every run of counts alone, not timed.
    Counts,
}

/// The plan as text: each sitting's tier, units and minutes, with what's done, and the minutes by tier.
pub fn plan(manifest: &ScaleBench, smoke: Option<Smoke>, paths: Option<&Paths>) -> String {
    let sittings = manifest.sittings(smoke);
    let complete = |unit: &Unit| paths.is_some_and(|paths| done(&paths.out, unit));
    let mut text = String::new();
    let mut by_tier = vec![(0_u32, 0_u32, 0_usize); manifest.tiers.len()];
    for (index, sitting) in sittings.iter().enumerate() {
        let left = sitting.units.iter().filter(|unit| !complete(unit)).count();
        let tier = &mut by_tier[sitting.tier];
        tier.0 += 1;
        tier.1 += sitting.seconds();
        tier.2 += usize::from(left > 0);
        let _ = writeln!(
            text,
            "sitting {:>2} {:<13} {:>4.1} min  {:>2} units{}",
            index + 1,
            manifest.tiers[sitting.tier].name,
            f64::from(sitting.seconds()) / 60.0,
            sitting.units.len(),
            if left == 0 && paths.is_some() {
                "  done".to_owned()
            } else if left < sitting.units.len() {
                format!("  {left} left")
            } else {
                String::new()
            }
        );
    }
    let _ = writeln!(text, "by tier:");
    for (tier, (count, seconds, left)) in manifest.tiers.iter().zip(by_tier) {
        // What the tier's own runs add, such as exit test 2's pools: `streaming: --texture-pool 333`.
        let own: Vec<String> = manifest
            .runs
            .iter()
            .filter_map(|run| Some(format!("{}: {}", run.name, run.tier_args.get(&tier.name)?.join(" "))))
            .collect();
        let _ = writeln!(
            text,
            "  {:<13} {} {} cook, {count} sittings, {:.0} min{}{}",
            tier.name,
            smoke.map_or(tier.size.as_str(), |smoke| smoke.size),
            tier.cook.name(),
            f64::from(seconds) / 60.0,
            if paths.is_some() { format!(", {left} with units left") } else { String::new() },
            if own.is_empty() { String::new() } else { format!("; {}", own.join(", ")) }
        );
    }
    let total: u32 = sittings.iter().map(Sitting::seconds).sum();
    let counting = manifest.counting(smoke);
    let _ = writeln!(
        text,
        "{} sittings, {:.0} min by the units' stated lengths; {} units of counts alone ({:.0} min, an ordinary slot)",
        sittings.len(),
        f64::from(total) / 60.0,
        counting.len(),
        f64::from(counting.iter().map(|unit| unit.seconds).sum::<u32>()) / 60.0
    );
    text
}

/// Runs what `select` names, skipping complete units. Returns one line for each unit that failed.
pub fn run(
    manifest: &ScaleBench,
    paths: &Paths,
    select: Select,
    smoke: Option<Smoke>,
) -> Result<Vec<String>, String> {
    crate::output_paths::disjoint(&paths.out, &[&paths.sandbox, &paths.desktop, &paths.mobile])?;
    fs::create_dir_all(&paths.out).map_err(|e| format!("failed to create {}: {e}", paths.out.display()))?;
    let sittings = manifest.sittings(smoke);
    let units: Vec<Unit> = match select {
        Select::Counts => manifest.counting(smoke),
        Select::Sitting(number) => {
            let sitting = sittings
                .get(number.wrapping_sub(1))
                .ok_or_else(|| format!("there are {} sittings; no sitting {number}", sittings.len()))?;
            sitting.units.clone()
        }
        Select::Next => {
            match sittings.iter().position(|sitting| sitting.units.iter().any(|unit| !done(&paths.out, unit)))
            {
                Some(index) => {
                    eprintln!("sitting  {} of {}", index + 1, sittings.len());
                    sittings[index].units.clone()
                }
                None => {
                    eprintln!("every sitting is complete in {}", paths.out.display());
                    return Ok(Vec::new());
                }
            }
        }
    };
    let clock = Stopwatch::start();
    let mut failures = Vec::new();
    for unit in &units {
        if done(&paths.out, unit) {
            eprintln!("skipped  {} (complete)", unit.name);
            continue;
        }
        if let Some(failure) = run_unit(manifest, paths, unit, select == Select::Counts, smoke)? {
            failures.push(failure);
        }
    }
    let left: Vec<&Sitting> =
        sittings.iter().filter(|sitting| sitting.units.iter().any(|unit| !done(&paths.out, unit))).collect();
    eprintln!(
        "ran      {} unit(s) in {:.1} min; {} sitting(s) left, {:.0} min by their stated lengths{}",
        units.len(),
        clock.elapsed().as_secs_f64() / 60.0,
        left.len(),
        f64::from(left.iter().map(|sitting| sitting.seconds()).sum::<u32>()) / 60.0,
        left.first().map_or_else(String::new, |next| format!(
            "; the next, {}, about {:.1} min",
            manifest.tiers[next.tier].name,
            f64::from(next.seconds()) / 60.0
        ))
    );
    Ok(failures)
}

/// Whether a unit's status in `out` says it's complete.
fn done(out: &Path, unit: &Unit) -> bool {
    Status::read(out, &unit.name).is_some_and(|status| status.complete())
}

/// Runs one unit; returns why it failed, if it did.
fn run_unit(
    manifest: &ScaleBench,
    paths: &Paths,
    unit: &Unit,
    counting: bool,
    smoke: Option<Smoke>,
) -> Result<Option<String>, String> {
    let tier = &manifest.tiers[unit.tier];
    let trace = paths.out.join(format!("{}.csv", unit.name));
    let replays = paths.replays.to_string_lossy();
    let args = manifest.args(unit, smoke, &replays, &trace.to_string_lossy());
    let tier_content = match tier.cook {
        Cook::Desktop => &paths.desktop,
        Cook::Mobile => &paths.mobile,
    };
    // A variant may mount another cook beside the tier's (D6's pair: the cells with one level, M5 Z2).
    let content = match unit.variant {
        Some(variant) => manifest.runs[unit.run].variants[variant].content_dir(tier_content),
        None => tier_content.clone(),
    };
    // A timed unit stops within its sitting's hold; one of counts alone gets room for a slow machine.
    let stated = u64::from(unit.seconds);
    let deadline = if counting {
        Duration::from_secs(stated * 2 + 300)
    } else {
        Duration::from_secs((stated * 2 + 60).min(u64::from(manifest.sitting_seconds)).max(stated + 30))
    };
    let mut command = Command::new(&paths.sandbox);
    command.env(CONTENT_DIR_ENV, content).args(&args);
    let line = format!("{} {}", paths.sandbox.display(), args.join(" "));
    run_command(&paths.out, unit, command, line, deadline)
}

/// Runs every unit that walks on the device called `device`, on the tiers `tiers` names (each its runs are on,
/// when empty), skipping complete ones (M5 Z2). Each goes through `launch`, a program and its leading
/// arguments, given `--` and the unit's sandbox arguments: it starts the sandbox on the device, already booted
/// with the bundle installed, prints the app's log as it runs and exits with its status
/// (`cargo xtask bench-scale launch`). Returns a line for each unit that failed, then for each verdict that
/// failed on the device.
pub fn run_on_device(
    manifest: &ScaleBench,
    out: &Path,
    device: &str,
    tiers: &[String],
    launch: &[String],
    smoke: Option<Smoke>,
) -> Result<Vec<String>, String> {
    let index = manifest.device(device).ok_or_else(|| {
        let names: Vec<&str> = manifest.devices.iter().map(|device| device.name.as_str()).collect();
        format!("the manifest lists no device {device:?}; it lists {}", names.join(", "))
    })?;
    let Some((program, leading)) = launch.split_first() else {
        return Err("a device's units need a launcher".into());
    };
    let units = manifest.on_device(index, tiers, smoke);
    if units.is_empty() {
        return Err(format!("no run walks on {device} on the tiers asked for"));
    }
    fs::create_dir_all(out).map_err(|e| format!("failed to create {}: {e}", out.display()))?;
    let mut failures = Vec::new();
    for unit in &units {
        if done(out, unit) {
            eprintln!("skipped  {} (complete)", unit.name);
            continue;
        }
        // No trace and no replays: a device's walk is counts alone, its frames and files on the device.
        let args = manifest.args(unit, smoke, "", "");
        let mut command = Command::new(program);
        command.args(leading).arg("--").args(&args);
        let line = format!("{} -- {}", launch.join(" "), args.join(" "));
        // A device draws slower than this Mac, and its stated length is this Mac's: four times it, and the
        // bundle's install and the device's first compile beside.
        let deadline = Duration::from_secs(u64::from(unit.seconds) * 4 + 600);
        if let Some(failure) = run_command(out, unit, command, line, deadline)? {
            failures.push(failure);
        }
    }
    let (lines, failed) = report::device_verdicts(manifest, out, index);
    for line in lines {
        eprintln!("{line}");
    }
    failures.extend(failed);
    Ok(failures)
}

/// Runs a unit's `command` into its log by `deadline` and writes its status; returns why it failed, if it did.
fn run_command(
    out: &Path,
    unit: &Unit,
    mut command: Command,
    line: String,
    deadline: Duration,
) -> Result<Option<String>, String> {
    let log = out.join(format!("{}.log", unit.name));
    let load = load();
    eprintln!(
        "running  {} ({} s stated, load {})",
        unit.name,
        unit.seconds,
        load.map_or_else(|| "unknown".to_owned(), |load| format!("{load:.1}"))
    );
    let clock = Stopwatch::start();
    let exit = run_with_deadline(&mut command, &log, deadline)
        .map_err(|e| format!("failed to run the sandbox for {} ({}): {e}", unit.name, log.display()))?;
    let status = Status {
        exit: exit.and_then(|exit| exit.code()),
        timed_out: exit.is_none(),
        seconds: clock.elapsed().as_secs_f64(),
        load,
        load_end: self::load(),
        command: line,
    };
    status.write(out, &unit.name).map_err(|e| format!("failed to write {}'s status: {e}", unit.name))?;
    eprintln!(
        "{} {} in {:.0} s",
        if status.complete() { "done    " } else { "FAILED  " },
        unit.name,
        status.seconds
    );
    Ok((!status.complete()).then(|| {
        format!(
            "{}: {}; its log is {}:\n{}",
            unit.name,
            status.describe(),
            log.display(),
            crate::scenes::log_tail(&log)
        )
    }))
}

/// The machine's one-minute load: `sysctl` on macOS, `/proc/loadavg` on Linux, `None` where neither answers.
fn load() -> Option<f64> {
    if let Ok(text) = fs::read_to_string("/proc/loadavg") {
        return text.split_whitespace().next()?.parse().ok();
    }
    let output = Command::new("sysctl").args(["-n", "vm.loadavg"]).output().ok()?;
    // `{ 3.12 2.90 2.51 }`
    String::from_utf8_lossy(&output.stdout).split_whitespace().find_map(|word| word.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(out: PathBuf) -> Paths {
        Paths {
            sandbox: PathBuf::from("/nonexistent/sandbox"),
            desktop: PathBuf::from("/nonexistent/desktop"),
            mobile: PathBuf::from("/nonexistent/mobile"),
            replays: PathBuf::from("/nonexistent/replays"),
            out,
        }
    }

    #[test]
    fn the_runner_skips_complete_units_and_the_plan_says_whats_left() {
        let manifest = ScaleBench::parse(
            &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/scale/fixtures/scale.ron"))
                .unwrap(),
        )
        .unwrap();
        let out = std::env::temp_dir().join(format!("sapphire_scale_skip_{}", std::process::id()));
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(&out).unwrap();
        let paths = paths(out.clone());
        let first = manifest.sittings(None).remove(0);
        for unit in &first.units {
            let status = Status {
                exit: Some(0),
                timed_out: false,
                seconds: 1.0,
                load: Some(1.0),
                load_end: Some(1.0),
                command: String::new(),
            };
            status.write(&out, &unit.name).unwrap();
        }
        // Every unit of sitting 1 is complete, so running it starts no sandbox (whose path doesn't exist).
        assert_eq!(run(&manifest, &paths, Select::Sitting(1), None), Ok(Vec::new()));
        let text = plan(&manifest, None, Some(&paths));
        assert!(text.lines().next().unwrap().ends_with("done"), "{text}");
        // Each tier says the size and cook its units run at.
        assert!(text.contains("1280x590 mobile cook,"), "{text}");
        // Sitting 2 isn't, and its sandbox can't start: the run says so rather than skip it.
        assert!(run(&manifest, &paths, Select::Sitting(2), None).is_err());
        assert!(run(&manifest, &paths, Select::Sitting(99), None).unwrap_err().contains("no sitting 99"));
        fs::remove_dir_all(&out).unwrap();
    }
}
