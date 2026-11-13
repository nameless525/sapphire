//! The golden-image scenes' runner, behind `cargo xtask golden` (spec §9 gate 3; M1 plan §3, open
//! question Q6).
//!
//! The scenes are a document beside their goldens, `tests/golden/scenes.ron` (M4 R1, [`document`]): each a
//! sandbox `--capture` at 640×360, or a size of its own, at a fixed tick, with no window, no focus, and fast
//! enough on CPU rasterizers. The capture's adapter record names the backend family ([`backend_key`]), and the
//! capture is compared with `<golden dir>/<scene>.<family>.png`.
//!
//! - A failing comparison leaves `<scene>.<family>.actual.png` and `.diff.png` (the FLIP error
//!   map) in the output directory, which CI uploads.
//! - A scene with no golden for this family fails in CI (`CI` is set) and is skipped locally.
//!   Its capture is kept as `.actual.png`, ready to commit.
//! - A golden or capture that can't be read or compared (another size, a capture cut short) fails
//!   its scene as an error, naming it and keeping the capture, and the run goes on (review DV-8).
//! - [`Mode::Update`] writes this family's goldens from the captures instead of comparing.
//!
//! Each scene's sandbox writes its output to `<scene>.log` in the output directory and has a
//! deadline (`--scene-timeout`, [`DEFAULT_SCENE_TIMEOUT`]): one that runs past it is killed, fails
//! as timed out with its log's last lines, and the run goes on to the next scene (review BC-008).
//! Every scene's line gives its wall time, and `timings.json` in the output directory lists them
//! all (scene, family, seconds, status), rewritten after each scene so a cancelled run keeps what
//! it had. `--shard` splits the scenes by those times as measured in CI ([`Shard`], review
//! BC-107).

mod costs;
pub mod document;

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use sapphire_time::Stopwatch;

use self::costs::{Family, costs};
pub use self::document::{CiSeconds, SceneListError};
use crate::golden::{self, Image, Thresholds, backend_key};

/// The environment variable the sandbox reads its cooked content from.
pub(crate) const CONTENT_DIR_ENV: &str = "SAPPHIRE_CONTENT_DIR";
/// The environment variable that forces the sandbox's GPU backend (`sapphire_rhi`'s
/// `BACKEND_OVERRIDE_ENV`), which CI's golden jobs set: it says whose costs `--shard` splits by.
const BACKEND_ENV: &str = "SAPPHIRE_GPU_BACKEND";
/// A scene's size unless it gives one: small enough for lavapipe and WARP to render each scene in a few
/// seconds.
const SIZE: &str = "640x360";
/// How long a scene may take unless `--scene-timeout` says otherwise: about seven times the
/// slowest scene on a CPU rasterizer (WARP's `foliage-ground`, about 130 s in CI), since a slow
/// runner must not fail a scene that isn't hung.
pub const DEFAULT_SCENE_TIMEOUT: Duration = Duration::from_secs(900);
/// How many of a failed scene's last log lines its failure quotes.
const LOG_TAIL_LINES: usize = 40;
/// Waiting on a sandbox polls with exponential backoff from [`FIRST_POLL`] to [`MAX_POLL`]: quick
/// scenes end within milliseconds of exiting, and slow ones cost no CPU.
const FIRST_POLL: Duration = Duration::from_millis(1);
const MAX_POLL: Duration = Duration::from_millis(50);

/// A golden scene, as the runner captures it: a row of the document (`tests/golden/scenes.ron`) with its base
/// and its size and tick resolved into one command line.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// The goldens' file stem.
    pub name: String,
    /// The sandbox's arguments after `--capture <png>`: `--size` and `--capture-ticks` from the row, then its
    /// base's arguments and its own.
    pub args: Vec<String>,
    pub thresholds: Thresholds,
    /// Its CI times on the families `--shard` splits by, once measured.
    pub ci_seconds: Option<CiSeconds>,
}

impl Scene {
    /// Whether its command line holds `arg`.
    pub fn has(&self, arg: &str) -> bool {
        self.args.iter().any(|given| given == arg)
    }

    /// The value its command line gives `flag`, the last if it gives it twice, as the sandbox reads it.
    pub fn value(&self, flag: &str) -> Option<&str> {
        self.args.windows(2).rev().find(|pair| pair[0] == flag).map(|pair| pair[1].as_str())
    }
}

/// The scenes of the document in `goldens` ([`document::FILE_NAME`]), in its order.
pub fn scenes(goldens: &Path) -> Result<Vec<Scene>, String> {
    document::read(goldens).map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Compare,
    Update,
}

/// Where the runner finds and puts things.
pub struct Paths {
    /// The sandbox executable.
    pub sandbox: PathBuf,
    /// Cooked desktop content.
    pub content: PathBuf,
    /// `tests/golden`.
    pub goldens: PathBuf,
    /// Captures, actuals, diffs, the scenes' logs and `timings.json`; emptied first. Its tree
    /// must be disjoint from all inputs, including existing symlink aliases.
    pub out: PathBuf,
}

/// Which scenes a run renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection<'a> {
    All,
    /// One scene, by name.
    Scene(&'a str),
    /// One of several slices of the list, so CI can split a backend's scenes across runners.
    Shard(Shard),
}

/// Slice `index` of `count` (from 1), split by what the scenes cost on the run's backend family
/// (review BC-107): costliest first, each scene joins the slice with the least so far, and each
/// slice keeps the list's order. On lavapipe and WARP a scene takes from under a second to two
/// minutes, so dealing the scenes out in turn, as the runner did before, left one of lavapipe's two
/// slices taking 10.4 minutes against the other's 3.7 to 7.
///
/// The costs are the CI times of the family that `SAPPHIRE_GPU_BACKEND` forces (`vulkan`
/// lavapipe's, `dx12` WARP's): each scene's `ci_seconds` in the document. Without them (another backend, or a
/// local run that forces none) every scene costs the same, which deals them out in turn. The split
/// depends on nothing else, so each of a family's runners computes the same one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shard {
    pub index: usize,
    pub count: usize,
}

impl std::str::FromStr for Shard {
    type Err = String;

    /// Parses `<index>/<count>`, such as `2/4`.
    fn from_str(text: &str) -> Result<Self, String> {
        let invalid = || format!("invalid shard {text:?}: expected <index>/<count> from 1, such as 2/4");
        let (index, count) = text.split_once('/').ok_or_else(invalid)?;
        let index = index.parse::<usize>().map_err(|_| invalid())?;
        let count = count.parse::<usize>().map_err(|_| invalid())?;
        if index == 0 || index > count {
            return Err(invalid());
        }
        Ok(Shard { index, count })
    }
}

/// Which of `count` slices (from 0) each scene goes to, by position, given each scene's cost:
/// costliest first (equal costs in list order), each to the slice with the least cost so far (the
/// first of equals). Equal costs deal the scenes out in turn.
fn split(costs: &[u32], count: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..costs.len()).collect();
    order.sort_by_key(|&position| (std::cmp::Reverse(costs[position]), position));
    let mut loads = vec![0_u64; count];
    let mut slices = vec![0; costs.len()];
    for position in order {
        let lightest = (0..count).min_by_key(|&slice| (loads[slice], slice)).expect("a shard count from 1");
        loads[lightest] += u64::from(costs[position]);
        slices[position] = lightest;
    }
    slices
}

/// Runs the selected scenes and returns one message per failure (empty: all passed). Each scene
/// gets `timeout` (`--scene-timeout`; [`DEFAULT_SCENE_TIMEOUT`]).
pub fn run(
    paths: &Paths,
    mode: Mode,
    selection: Selection<'_>,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let scenes = scenes(&paths.goldens)?;
    let family = std::env::var(BACKEND_ENV).ok().and_then(|backend| Family::from_backend(&backend));
    let selected = select(&scenes, selection, family)?;
    let sandbox = |_: &Scene| {
        let mut command = Command::new(&paths.sandbox);
        command.env(CONTENT_DIR_ENV, &paths.content);
        command
    };
    run_scenes(&selected, paths, mode, timeout, &sandbox)
}

/// The scenes `selection` names, in their order.
fn select<'s>(
    scenes: &'s [Scene],
    selection: Selection<'_>,
    family: Option<Family>,
) -> Result<Vec<&'s Scene>, String> {
    match selection {
        Selection::All => Ok(scenes.iter().collect()),
        Selection::Scene(name) => match scenes.iter().find(|scene| scene.name == name) {
            Some(scene) => Ok(vec![scene]),
            None => {
                let names: Vec<&str> = scenes.iter().map(|scene| scene.name.as_str()).collect();
                Err(format!("no golden scene named {name:?}; scenes: {}", names.join(", ")))
            }
        },
        Selection::Shard(shard) => {
            let costs = costs(scenes, family);
            let slices = split(&costs, shard.count);
            let held: Vec<usize> =
                (0..scenes.len()).filter(|&position| slices[position] == shard.index - 1).collect();
            if held.is_empty() {
                return Err(format!(
                    "shard {}/{} holds none of the {} scenes",
                    shard.index,
                    shard.count,
                    scenes.len()
                ));
            }
            let (index, count, total) = (shard.index, shard.count, scenes.len());
            match family {
                Some(family) => {
                    let minutes = |tenths: u64| tenths as f64 / 600.0;
                    let ours: u64 = held.iter().map(|&position| u64::from(costs[position])).sum();
                    let all: u64 = costs.iter().copied().map(u64::from).sum();
                    eprintln!(
                        "shard {index}/{count}: {} of {total} scenes, split by {}'s CI times: about {:.1} of their {:.1} \
                         minutes",
                        held.len(),
                        family.name(),
                        minutes(ours),
                        minutes(all)
                    );
                }
                None => eprintln!(
                    "shard {index}/{count}: {} of {total} scenes, dealt out in turn ({BACKEND_ENV} names no family with \
                     CI times)",
                    held.len()
                ),
            }
            Ok(held.into_iter().map(|position| &scenes[position]).collect())
        }
    }
}

/// How a scene ended, as `timings.json` records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// Within its gates.
    Ok,
    /// Its golden written (`--update`).
    Updated,
    /// No golden for its family: a failure in CI, skipped locally.
    NoGolden,
    /// Outside its gates.
    Failed,
    /// The sandbox failed or left no adapter record, or its capture or golden can't be read or
    /// compared.
    Error,
    /// Killed at its deadline.
    TimedOut,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Updated => "updated",
            Self::NoGolden => "no-golden",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::TimedOut => "timed-out",
        }
    }
}

/// One scene's result.
struct Outcome {
    status: Status,
    /// Its backend family, when its sandbox wrote an adapter record.
    family: Option<String>,
    failure: Option<String>,
}

/// One scene's entry in `timings.json`.
struct Timing {
    scene: String,
    family: Option<String>,
    seconds: f64,
    status: Status,
}

/// Runs `selected` with the sandbox `sandbox` starts for each (its program and environment; the
/// capture's arguments are added here).
fn run_scenes(
    selected: &[&Scene],
    paths: &Paths,
    mode: Mode,
    timeout: Duration,
    sandbox: &dyn Fn(&Scene) -> Command,
) -> Result<Vec<String>, String> {
    crate::output_paths::disjoint(&paths.out, &[&paths.goldens, &paths.content, &paths.sandbox])?;
    if paths.out.exists() {
        fs::remove_dir_all(&paths.out).map_err(|e| io_error("failed to empty", &paths.out, e))?;
    }
    fs::create_dir_all(&paths.out).map_err(|e| io_error("failed to create", &paths.out, e))?;
    let in_ci = std::env::var_os("CI").is_some();
    let timings_path = paths.out.join("timings.json");

    let run_clock = Stopwatch::start();
    let mut failures = Vec::new();
    let mut timings = Vec::with_capacity(selected.len());
    for &scene in selected {
        let clock = Stopwatch::start();
        let outcome = run_scene(scene, paths, mode, timeout, in_ci, sandbox, &clock)?;
        failures.extend(outcome.failure);
        timings.push(Timing {
            scene: scene.name.clone(),
            family: outcome.family,
            seconds: clock.elapsed().as_secs_f64(),
            status: outcome.status,
        });
        write_timings(&timings_path, &timings)?;
    }
    eprintln!(
        "{} scene(s) in {:.1} min; their times are in {}",
        timings.len(),
        run_clock.elapsed().as_secs_f64() / 60.0,
        timings_path.display()
    );
    Ok(failures)
}

/// What the sandbox is given for an argument of a scene. A path under `tests/`, such as a committed
/// replay's, is found from the goldens, which sit there too, wherever the run was started from.
fn scene_arg(arg: &str, paths: &Paths) -> OsString {
    match (arg.strip_prefix("tests/"), paths.goldens.parent()) {
        (Some(path), Some(tests)) => tests.join(path).into_os_string(),
        _ => arg.into(),
    }
}

/// Renders `scene` and checks it, printing one line with its time. `Err` stops the whole run: the
/// sandbox can't be started, or the output can't be written. A scene that fails is an [`Outcome`].
fn run_scene(
    scene: &Scene,
    paths: &Paths,
    mode: Mode,
    timeout: Duration,
    in_ci: bool,
    sandbox: &dyn Fn(&Scene) -> Command,
    clock: &Stopwatch,
) -> Result<Outcome, String> {
    let seconds = || clock.elapsed().as_secs_f64();
    let capture = paths.out.join(format!("{}.png", scene.name));
    let log = paths.out.join(format!("{}.log", scene.name));
    let mut command = sandbox(scene);
    command.arg("--capture").arg(&capture);
    command.args(scene.args.iter().map(|arg| scene_arg(arg, paths)));
    let exit = run_with_deadline(&mut command, &log, timeout)
        .map_err(|e| format!("failed to run the sandbox for {} ({}): {e}", scene.name, log.display()))?;
    let error = |status: Status, family: Option<String>, what: String| {
        eprintln!("{:<8} {}: {what} (log: {})", status.name(), scene.name, log.display());
        Outcome {
            status,
            family,
            failure: Some(format!(
                "{}: {what}; its log, {}, ends:\n{}",
                scene.name,
                log.display(),
                log_tail(&log)
            )),
        }
    };
    let Some(exit) = exit else {
        return Ok(error(Status::TimedOut, None, format!("timed out after {} s", timeout.as_secs())));
    };
    if !exit.success() {
        let family = backend_family(&capture).ok();
        return Ok(error(
            Status::Error,
            family,
            format!("the sandbox failed ({exit}) after {:.1} s", seconds()),
        ));
    }
    let family = match backend_family(&capture) {
        Ok(family) => family,
        Err(message) => return Ok(error(Status::Error, None, message)),
    };
    if let Some(builds) = fs::read_to_string(&log).ok().and_then(|text| frame_builds(&text, scene)) {
        return Ok(error(Status::Error, Some(family), builds));
    }
    let outcome =
        |status: Status, failure: Option<String>| Outcome { status, family: Some(family.clone()), failure };
    let golden_path = paths.goldens.join(format!("{}.{family}.png", scene.name));
    let kept = paths.out.join(format!("{}.{family}.actual.png", scene.name));
    let keep = || fs::copy(&capture, &kept).map(drop).map_err(|e| io_error("failed to write", &kept, e));
    // This scene's failure, not the run's: a golden of another size, a capture cut short.
    let unreadable = |why: golden::GoldenError| -> Result<Outcome, String> {
        let what = if capture.is_file() {
            keep()?;
            format!("{why}; the capture is at {}", kept.display())
        } else {
            why.to_string()
        };
        Ok(error(Status::Error, Some(family.clone()), what))
    };

    if mode == Mode::Update {
        // Read first: a capture cut short would pass for a golden until a run compared against it.
        if let Err(why) = Image::read_png(&capture) {
            return unreadable(why);
        }
        fs::create_dir_all(&paths.goldens).map_err(|e| io_error("failed to create", &paths.goldens, e))?;
        fs::copy(&capture, &golden_path).map_err(|e| io_error("failed to write", &golden_path, e))?;
        eprintln!("updated  {} {:.1} s", golden_path.display(), seconds());
        return Ok(outcome(Status::Updated, None));
    }
    if !golden_path.is_file() {
        keep()?;
        let message = format!(
            "{} [{family}] {:.1} s: no golden yet; the capture is at {} (`cargo xtask golden --update` adds it)",
            scene.name,
            seconds(),
            kept.display()
        );
        eprintln!("{:<8} {message}", if in_ci { "failed" } else { "skipped" });
        return Ok(outcome(Status::NoGolden, in_ci.then_some(message)));
    }

    let compared = Image::read_png(&golden_path).and_then(|golden| {
        let actual = Image::read_png(&capture)?;
        golden::compare(&golden, &actual, &scene.thresholds)
    });
    let comparison = match compared {
        Ok(comparison) => comparison,
        Err(why) => return unreadable(why),
    };
    let summary = format!(
        "{} [{family}] {:.1} s: mean {:.4} (≤ {}), p{} {:.4} (≤ {}), worst tile {:.4} (≤ {}), max {:.3}",
        scene.name,
        seconds(),
        comparison.mean,
        scene.thresholds.mean,
        comparison.percentile,
        comparison.percentile_value,
        scene.thresholds.percentile_max,
        comparison.worst_tile,
        scene.thresholds.tile_max,
        comparison.max,
    );
    if comparison.passes(&scene.thresholds) {
        eprintln!("ok       {summary}");
        return Ok(outcome(Status::Ok, None));
    }
    keep()?;
    let diff = paths.out.join(format!("{}.{family}.diff.png", scene.name));
    comparison.error_map.write_png(&diff).map_err(|e| e.to_string())?;
    eprintln!("failed   {summary}");
    Ok(outcome(Status::Failed, Some(format!("{summary}\n    see {} and {}", kept.display(), diff.display()))))
}

/// Runs `command` with its output going to a new file at `log`, for at most `timeout`: the exit
/// status, or `None` once a command that ran out of time has been killed and reaped.
///
/// A file, not a pipe: the sandbox's crash monitor inherits its output, so a pipe would stay open
/// after the sandbox had gone, and an undrained one stalls the sandbox once it fills (64 KiB on
/// Linux). Only the sandbox is killed: its crash monitor exits when its lifeline, a pipe only the
/// sandbox holds, closes (`sapphire_crash`). A kill is `SIGKILL` (`TerminateProcess` on Windows),
/// so a hung sandbox leaves no minidump; std has no portable way to send `SIGABRT` first.
pub(crate) fn run_with_deadline(
    command: &mut Command,
    log: &Path,
    timeout: Duration,
) -> io::Result<Option<ExitStatus>> {
    let stdout = File::create(log)?;
    let stderr = stdout.try_clone()?;
    let mut child = command.stdin(Stdio::null()).stdout(stdout).stderr(stderr).spawn()?;
    let waited = wait_with_deadline(&mut child, timeout);
    if waited.is_err() {
        // Never leave it running.
        let _ = child.kill();
        let _ = child.wait();
    }
    waited
}

fn wait_with_deadline(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let clock = Stopwatch::start();
    let mut delay = FIRST_POLL;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let elapsed = clock.elapsed();
        if elapsed >= timeout {
            // An error means it exited meanwhile; the wait reaps it either way.
            let _ = child.kill();
            child.wait()?;
            return Ok(None);
        }
        std::thread::sleep(delay.min(timeout - elapsed));
        delay = (delay * 2).min(MAX_POLL);
    }
}

/// The last [`LOG_TAIL_LINES`] lines of a scene's log, indented, for its failure.
/// What a scene's frames built that its load didn't declare (M3 K1), from its log's "captured frame" line:
/// `None` if nothing, or if the scene shows a debug view. A capture counts what its load builds as prewarmed, so
/// a first use there is a frame's, which a game's frame would stall on: the load's inventory, or what the scene
/// declares, should list it. A debug view alone builds in its frame: it copies what that frame draws into its own
/// target, and no load declares a view a tool turns on. The colour-vision view is declared when it's on (M4 H3),
/// so a scene with it builds nothing in its frames either.
fn frame_builds(log: &str, scene: &Scene) -> Option<String> {
    if scene.has("--debug-view") {
        return None;
    }
    let line = log.lines().rfind(|line| line.contains("captured frame"))?;
    let count: u32 = line.split_once(" first_use=")?.1.split(' ').next()?.parse().ok()?;
    (count > 0).then(|| {
        format!(
            "its frames built {count} pipelines its load didn't declare; its log's \"built on first use\" lines \
             name them"
        )
    })
}

pub(crate) fn log_tail(path: &Path) -> String {
    const MAX_BYTES: u64 = 16 * 1024;
    let read = || -> io::Result<String> {
        let mut file = File::open(path)?;
        let start = file.metadata()?.len().saturating_sub(MAX_BYTES);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        // A read from the middle starts inside a line.
        let lines: Vec<&str> = text.lines().skip(usize::from(start > 0)).collect();
        let first = lines.len().saturating_sub(LOG_TAIL_LINES);
        Ok(lines[first..].iter().fold(String::new(), |mut tail, line| {
            let _ = writeln!(tail, "    {line}");
            tail
        }))
    };
    match read() {
        Ok(tail) if tail.is_empty() => "    (the log is empty)\n".to_owned(),
        Ok(tail) => tail,
        Err(error) => format!("    (the log can't be read: {error})\n"),
    }
}

/// Writes every scene's time so far as a JSON array of `{scene, family, seconds, status}`. A run
/// renders every scene on one backend, so a scene that left no adapter record (it timed out, say)
/// takes the family the others named.
fn write_timings(path: &Path, timings: &[Timing]) -> Result<(), String> {
    let run_family = timings.iter().find_map(|timing| timing.family.as_deref());
    let mut json = String::from("[\n");
    for (i, timing) in timings.iter().enumerate() {
        let family = timing.family.as_deref().or(run_family).map_or_else(|| "null".to_owned(), json_string);
        let comma = if i + 1 < timings.len() { "," } else { "" };
        let _ = writeln!(
            json,
            "  {{\"scene\": {}, \"family\": {family}, \"seconds\": {:.3}, \"status\": \"{}\"}}{comma}",
            json_string(&timing.scene),
            timing.seconds,
            timing.status.name()
        );
    }
    json.push_str("]\n");
    fs::write(path, json).map_err(|e| io_error("failed to write", path, e))
}

/// `text` as a JSON string.
fn json_string(text: &str) -> String {
    let mut json = String::with_capacity(text.len() + 2);
    json.push('"');
    for c in text.chars() {
        match c {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(json, "\\u{:04x}", u32::from(c));
            }
            c => json.push(c),
        }
    }
    json.push('"');
    json
}

fn io_error(what: &str, path: &Path, error: io::Error) -> String {
    format!("{what} {}: {error}", path.display())
}

/// The backend family of a capture, from the `<capture>.adapter.txt` the sandbox writes.
fn backend_family(capture: &Path) -> Result<String, String> {
    let path = capture.with_extension("adapter.txt");
    let text = fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .ok_or_else(|| format!("{} has no `{key}=` line", path.display()))
    };
    Ok(backend_key(field("backend")?, field("adapter")?))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// The committed goldens and their document.
    fn goldens() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
    }

    /// The committed document's scenes.
    fn committed_scenes() -> Vec<Scene> {
        scenes(&goldens()).unwrap()
    }

    /// A scene of `args` alone, with no CI times, for the runner's own tests.
    fn scene(name: &str, args: &[&str]) -> Scene {
        Scene {
            name: name.to_owned(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            thresholds: Thresholds::default(),
            ci_seconds: None,
        }
    }

    /// The families with committed goldens.
    const GOLDEN_FAMILIES: [&str; 3] = ["metal", "vulkan-lavapipe", "dx12-warp"];

    #[test]
    fn the_document_holds_every_golden_and_nothing_else() {
        // It reads, which refuses a name given twice or a field it doesn't know (`document`'s tests). Each scene
        // has its Metal golden, and once it has lavapipe's and WARP's, which come from its first CI run, the CI
        // times that run measured. Every other file here is a scene's golden.
        let scenes = committed_scenes();
        let dir = goldens();
        for scene in &scenes {
            let has = |family: &str| dir.join(format!("{}.{family}.png", scene.name)).is_file();
            assert!(
                has("metal"),
                "{0} has no Metal golden: `cargo xtask golden --update --scene {0}`",
                scene.name
            );
            if has("vulkan-lavapipe") && has("dx12-warp") {
                assert!(
                    scene.ci_seconds.is_some(),
                    "{} has lavapipe's and WARP's goldens but not their CI times: take its `ci_seconds` from the \
                     `timings.json` of the run they came from",
                    scene.name
                );
            }
        }
        let mut strays = Vec::new();
        for entry in fs::read_dir(&dir).unwrap() {
            let file = entry.unwrap().file_name().into_string().unwrap();
            let golden = file.strip_suffix(".png").and_then(|stem| stem.rsplit_once('.')).is_some_and(
                |(name, family)| {
                    GOLDEN_FAMILIES.contains(&family) && scenes.iter().any(|scene| scene.name == name)
                },
            );
            if !golden && file != document::FILE_NAME {
                strays.push(file);
            }
        }
        assert!(strays.is_empty(), "files in tests/golden that are no scene's golden: {strays:?}");
    }

    #[test]
    fn a_frame_s_first_use_fails_a_scene_unless_it_shows_a_debug_view() {
        let line = |count: u32| {
            format!(
                "INFO sapphire::sandbox: captured frame path=x.png ticks=90 prewarmed=40 first_use={count}\n"
            )
        };
        let plain = scene("plain", &["--capture-ticks", "90"]);
        let viewed = scene("viewed", &["--debug-view", "wireframe"]);
        assert_eq!(frame_builds(&line(0), &plain), None);
        assert!(frame_builds(&line(3), &plain).is_some_and(|why| why.contains("built 3 pipelines")));
        assert_eq!(frame_builds(&line(3), &viewed), None, "a debug view builds in its frame");
        let seen = scene("seen", &["--color-vision", "deuteranopia"]);
        assert!(frame_builds(&line(1), &seen).is_some(), "the colour-vision view is declared");
        assert_eq!(frame_builds("INFO sapphire::sandbox: contact sheet written\n", &plain), None);
    }

    #[test]
    fn shards_parse_from_one() {
        assert_eq!("2/4".parse(), Ok(Shard { index: 2, count: 4 }));
        assert_eq!("1/1".parse(), Ok(Shard { index: 1, count: 1 }));
        for invalid in ["0/4", "5/4", "1/0", "2", "a/4", "2/b", "", "/", "-1/4"] {
            assert!(invalid.parse::<Shard>().is_err(), "{invalid:?} parsed");
        }
    }

    /// The shard counts CI runs (lavapipe 3, WARP 6), one, and a few more.
    const SHARD_COUNTS: [usize; 6] = [1, 2, 3, 4, 5, 6];
    const FAMILIES: [Option<Family>; 3] = [None, Some(Family::Lavapipe), Some(Family::Warp)];

    /// Each shard's scenes, by name, as `select` picks them.
    fn shards(scenes: &[Scene], count: usize, family: Option<Family>) -> Vec<Vec<&str>> {
        (1..=count)
            .map(|index| {
                let held =
                    select(scenes, Selection::Shard(Shard { index, count }), family).unwrap_or_default();
                held.iter().map(|scene| scene.name.as_str()).collect()
            })
            .collect()
    }

    #[test]
    fn shards_hold_every_scene_once_in_order() {
        let scenes = committed_scenes();
        let names: Vec<&str> = scenes.iter().map(|scene| scene.name.as_str()).collect();
        for family in FAMILIES {
            for count in SHARD_COUNTS {
                let shards = shards(&scenes, count, family);
                assert!(
                    shards.iter().all(|shard| !shard.is_empty()),
                    "{family:?}, {count} shards: an empty one"
                );
                for shard in &shards {
                    let positions: Vec<usize> =
                        shard.iter().map(|name| names.iter().position(|n| n == name).unwrap()).collect();
                    assert!(positions.is_sorted(), "{family:?}, {count} shards: out of order");
                }
                let mut held: Vec<&str> = shards.concat();
                held.sort_unstable();
                let mut all = names.clone();
                all.sort_unstable();
                assert_eq!(held, all, "{family:?}, {count} shards: every scene exactly once");
            }
        }
    }

    #[test]
    fn without_costs_shards_deal_the_scenes_out_in_turn() {
        // As the runner split them before it had costs.
        for count in SHARD_COUNTS {
            let slices = split(&[1; 95], count);
            assert!(slices.iter().enumerate().all(|(position, &slice)| slice == position % count), "{count}");
        }
        assert_eq!(costs(&committed_scenes(), None), vec![1; committed_scenes().len()]);
        assert_eq!(Family::from_backend("metal"), None);
        assert_eq!(Family::from_backend("Vulkan"), Some(Family::Lavapipe));
        assert_eq!(Family::from_backend("dx12"), Some(Family::Warp));
    }

    #[test]
    fn a_scene_without_a_cost_still_lands_in_one_shard() {
        let mut scenes = committed_scenes();
        scenes.insert(3, scene("scene-added-since", &[]));
        for family in [Family::Lavapipe, Family::Warp] {
            let costs = costs(&scenes, Some(family));
            assert_eq!(costs[3], *costs.iter().max().unwrap(), "{family:?}: counts as the costliest");
            for count in SHARD_COUNTS {
                let shards = shards(&scenes, count, Some(family));
                let holding = shards.iter().filter(|shard| shard.contains(&"scene-added-since")).count();
                assert_eq!(holding, 1, "{family:?}, {count} shards");
                assert_eq!(shards.iter().map(Vec::len).sum::<usize>(), scenes.len());
            }
        }
    }

    #[test]
    fn splitting_by_cost_shortens_ci_s_slowest_shard() {
        // CI's split: lavapipe in 3, WARP in 6.
        let scenes = committed_scenes();
        for (family, count) in [(Family::Lavapipe, 3), (Family::Warp, 6)] {
            let costs = costs(&scenes, Some(family));
            let slowest = |slices: &[usize]| {
                let mut loads = vec![0_u64; count];
                slices.iter().zip(&costs).for_each(|(&slice, &cost)| loads[slice] += u64::from(cost));
                loads.into_iter().max().unwrap()
            };
            let in_turn: Vec<usize> = (0..scenes.len()).map(|position| position % count).collect();
            let by_cost = slowest(&split(&costs, count));
            assert!(by_cost < slowest(&in_turn), "{family:?}");
            // Longest-first is within 4/3 of the best split, and an equal share bounds that below.
            let share = costs.iter().copied().map(u64::from).sum::<u64>().div_ceil(count as u64);
            assert!(by_cost * 3 <= share * 4, "{family:?}: {by_cost} against a share of {share}");
        }
    }

    #[test]
    fn only_the_ambient_scenes_take_ambient_occlusion_and_a_grid() {
        // Every scene from before M1 W5 pins AO off and loads no grid, so it draws as it did; the
        // ambient scenes pin fog off instead. A zone's brings its grid and pins nothing: it's its look.
        for scene in committed_scenes() {
            let pinned = |flag: &str, value: &str| scene.args.windows(2).any(|pair| pair == [flag, value]);
            if scene.name.starts_with("ambient-") {
                assert!(pinned("--fog", "off"), "{}", scene.name);
            } else if scene.name.starts_with("zone-trip-") {
                // A trip's zones bring their grids, a replay's trips taking them there (M5 M6).
                assert!(scene.has("--capture-through-trips") && pinned("--ao", "off"), "{}", scene.name);
                assert!(!scene.has("--probes"), "{}", scene.name);
            } else if scene.name.starts_with("zone-") {
                assert!(scene.has("--zone"), "{}", scene.name);
            } else if scene.name.starts_with("game-ward-") {
                // The game as it ships, on the zone's own look, at a tier no device chooses for it.
                assert!(scene.has("--game"), "{}", scene.name);
                assert!(pinned("--tier", "desktop-high"), "{}", scene.name);
            } else if scene.name.starts_with("tier-") {
                // A tier as it ships (M4 T0), whose GTAO the thermal ladder's rungs shed.
                assert!(scene.has("--tier") && !scene.has("--ao"), "{}", scene.name);
            } else {
                assert!(pinned("--ao", "off"), "{}", scene.name);
                assert!(!scene.has("--probes"), "{}", scene.name);
            }
        }
    }

    #[test]
    fn only_the_hud_scenes_draw_the_interface_and_they_cover_spec_5_17_s_aspect_ratios() {
        let mut sizes = Vec::new();
        for scene in committed_scenes() {
            let hud = scene.has("--hud");
            // `arena-fight-hit`, `arena-skirmish`, `arena-boss`, `arena-captain` and the ward's fights show the
            // bars reading a real fight.
            let name = scene.name.as_str();
            let expected = name.starts_with("hud-")
                || name.starts_with("game-hud-")
                || ["arena-fight-hit", "arena-skirmish", "arena-boss", "arena-captain"].contains(&name)
                || ["fight-ward-market", "fight-ward-terrace"].contains(&name);
            assert_eq!(hud, expected, "{name}");
            let display_text = scene.has("--display-text");
            assert_eq!(display_text, name.starts_with("display-text-"), "{name}");
            if name.starts_with("game-") {
                continue;
            }
            let size = scene.value("--size").expect("the runner's size").to_owned();
            if hud {
                sizes.push(size);
            } else if !display_text && !name.starts_with("tier-") {
                // The tier scenes (M4 T0) draw a phone's frame at a phone's shape.
                assert_eq!(size, SIZE, "{name}");
            }
        }
        for (ratio, size) in [("4:3", "480x360"), ("16:9", "640x360"), ("21:9", "840x360")] {
            assert!(sizes.iter().any(|given| given == size), "no HUD scene at {ratio}");
        }
    }

    /// Spec §5.17's aspect ratios again, for the game's screens: Japanese in all three, as exit test 4 wants.
    #[test]
    fn the_screens_scenes_cover_the_aspect_ratios_in_japanese() {
        let scenes = committed_scenes();
        let size_of = |name: &str| {
            let scene = scenes.iter().find(|scene| scene.name == name).expect("the scene");
            assert_eq!(scene.value("--ui-locale"), Some("ja"), "{name}");
            let (width, height) =
                scene.value("--size").and_then(|size| size.split_once('x')).expect("a size");
            width.parse::<f32>().expect("a width") / height.parse::<f32>().expect("a height")
        };
        assert!((size_of("game-title-tablet-ja") - 4.0 / 3.0).abs() < 0.01);
        assert!((size_of("game-title-wide-ja") - 16.0 / 9.0).abs() < 0.01);
        assert!((size_of("game-title-ultrawide-ja") - 21.0 / 9.0).abs() < 0.01);
        // And the Steam Deck's 16:10 (M4 S4), every screen in each language.
        assert!((size_of("game-title-deck-ja") - 16.0 / 10.0).abs() < 0.01);
        // Every screen at 16:9 has its Deck scene. The ward's gate is a fight's, A2's option variants (larger
        // text, subtitle bands) and A3's notice at the larger text show screens the Deck already has, and the
        // boot's update is the boot screen's layout, which the boot's scenes hold at the widest and narrowest
        // aspects, the Deck's between them.
        let names: Vec<&str> = scenes.iter().map(|scene| scene.name.as_str()).collect();
        let missing: Vec<&str> = names
            .iter()
            .copied()
            .filter(|name| name.starts_with("game-") && name.contains("-wide-"))
            .filter(|name| {
                ![
                    "game-ward-gate-",
                    "game-display-larger-text-",
                    "game-subtitle-band-",
                    "game-notice-larger-text-",
                    "game-boot-update-",
                ]
                .iter()
                .any(|variant| name.starts_with(variant))
            })
            .filter(|name| !names.contains(&name.replace("-wide-", "-deck-").as_str()))
            .collect();
        assert!(missing.is_empty(), "screens at 16:9 without a Deck scene: {missing:?}");
    }

    #[test]
    fn a_path_under_tests_is_found_from_the_goldens_and_exists() {
        let paths = Paths {
            sandbox: PathBuf::new(),
            content: PathBuf::new(),
            goldens: goldens(),
            out: PathBuf::new(),
        };
        assert_eq!(scene_arg("--replay", &paths), OsString::from("--replay"));
        let mut replays = 0;
        for scene in committed_scenes() {
            for arg in scene.args.iter().filter(|arg| arg.starts_with("tests/")) {
                assert!(Path::new(&scene_arg(arg, &paths)).is_file(), "{}: {arg}", scene.name);
                replays += 1;
            }
        }
        assert!(replays > 0, "a scene plays a committed replay");
    }

    #[test]
    fn only_the_debug_draw_scene_turns_debug_draw_on() {
        for scene in committed_scenes() {
            assert_eq!(scene.has("--debug-draw"), scene.name.starts_with("debugdraw-"), "{}", scene.name);
        }
    }

    #[test]
    fn only_the_debug_scenes_show_a_debug_view_and_each_its_own() {
        for scene in committed_scenes() {
            let view = scene.value("--debug-view");
            match scene.name.strip_prefix("debug-") {
                Some(name) => assert!(view.is_some_and(|view| name.starts_with(view)), "{}", scene.name),
                None => assert_eq!(view, None, "{}", scene.name),
            }
        }
    }

    #[test]
    fn only_the_taa_render_scale_pair_has_gates_of_its_own() {
        // The document says why theirs is a tighter mean (review DV-2); the same gates hold each against the
        // other in `a_capture_that_lost_its_scene_s_feature_fails_the_golden`.
        let default = Thresholds::default();
        for scene in committed_scenes() {
            match scene.name.as_str() {
                "turntable-taa" | "turntable-taa-upsampled" => {
                    assert!(scene.thresholds.mean < default.mean, "{}", scene.name);
                    assert_eq!(scene.thresholds, Thresholds { mean: scene.thresholds.mean, ..default });
                }
                _ => assert_eq!(scene.thresholds, default, "{}", scene.name),
            }
        }
    }

    /// A committed golden, from `tests/golden`.
    fn committed(scene: &str, family: &str) -> Image {
        let name = format!("{scene}.{family}.png");
        Image::read_png(&goldens().join(name)).unwrap()
    }

    #[test]
    fn a_capture_that_lost_its_scene_s_feature_fails_the_golden() {
        // Review DV-2. Each pair's arguments differ in one thing a scene exists to show: the bangs'
        // soft shadow, MSAA, the render scale; so each golden read as the other is that feature
        // gone wrong. The tier pairs aren't here: `eyes-front` and `eyes-mobile`, and `hair-shadow`
        // and `hair-shadow-mobile`, stay within the gates (mean 0.0096, p99 0.054, worst tile 0.057
        // at most), moving a little everywhere and no more locally than lavapipe's noise.
        let scenes = committed_scenes();
        let thresholds = |name: &str| scenes.iter().find(|scene| scene.name == name).unwrap().thresholds;
        for family in GOLDEN_FAMILIES {
            for (first, second) in [
                ("hair-shadow-soft", "hair-shadow"),
                ("turntable", "turntable-msaa-off"),
                ("turntable", "turntable-render-scale"),
                ("turntable-taa", "turntable-taa-upsampled"),
            ] {
                let first_image = committed(first, family);
                let second_image = committed(second, family);
                for (golden, actual, golden_image, actual_image) in [
                    (first, second, &first_image, &second_image),
                    (second, first, &second_image, &first_image),
                ] {
                    let thresholds = thresholds(golden);
                    let comparison = golden::compare(golden_image, actual_image, &thresholds).unwrap();
                    assert!(
                        !comparison.passes(&thresholds),
                        "{actual} passes as {golden} on {family}: mean {}, p{} {}, worst tile {}",
                        comparison.mean,
                        comparison.percentile,
                        comparison.percentile_value,
                        comparison.worst_tile,
                    );
                }
            }
        }
    }

    #[test]
    fn a_small_wrong_patch_fails_the_golden() {
        // Review DV-2: before the tile gate a centered 40 px patch passed these scenes; on
        // `lookdev-face` an eye is about 30 px wide, so a whole eye could go wrong.
        const SIDE: usize = 6;
        let scenes = committed_scenes();
        for family in GOLDEN_FAMILIES {
            for name in ["eyes-front", "lookdev-face", "hair-shadow", "turntable", "lookdev-town-hero"] {
                let golden = committed(name, family);
                let mut actual = golden.clone();
                let (width, height) = (actual.width as usize, actual.height as usize);
                for y in (height - SIDE) / 2..(height + SIDE) / 2 {
                    for x in (width - SIDE) / 2..(width + SIDE) / 2 {
                        let start = (y * width + x) * 4;
                        actual.rgba[start..start + 3].fill(128);
                    }
                }
                let thresholds = scenes.iter().find(|scene| scene.name == name).unwrap().thresholds;
                let comparison = golden::compare(&golden, &actual, &thresholds).unwrap();
                assert!(
                    !comparison.passes(&thresholds),
                    "a {SIDE} px patch passes {name} on {family}: mean {}, p{} {}, worst tile {}",
                    comparison.mean,
                    comparison.percentile,
                    comparison.percentile_value,
                    comparison.worst_tile,
                );
            }
        }
    }

    #[test]
    fn reads_the_backend_family_from_the_adapter_record() {
        let dir = std::env::temp_dir().join(format!("sapphire_testkit_scenes_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let capture = dir.join("scene.png");
        fs::write(dir.join("scene.adapter.txt"), "backend=vulkan\nadapter=llvmpipe (LLVM 19)\n").unwrap();
        assert_eq!(backend_family(&capture).unwrap(), "vulkan-lavapipe");
        fs::write(dir.join("scene.adapter.txt"), "backend=metal\n").unwrap();
        assert!(backend_family(&capture).is_err(), "no adapter line");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_unknown_scenes_and_empty_shards() {
        let paths = Paths {
            sandbox: PathBuf::from("unused"),
            content: PathBuf::from("unused"),
            goldens: goldens(),
            out: PathBuf::from("unused"),
        };
        let run = |selection| run(&paths, Mode::Compare, selection, DEFAULT_SCENE_TIMEOUT).unwrap_err();
        assert!(run(Selection::Scene("no-such-scene")).contains("turntable"));
        assert!(run(Selection::Shard(Shard { index: 999, count: 999 })).contains("holds none"));
        // A run with no document stops before any scene, naming it.
        let paths = Paths { goldens: PathBuf::from("unused"), ..paths };
        let missing = super::run(&paths, Mode::Compare, Selection::All, DEFAULT_SCENE_TIMEOUT).unwrap_err();
        assert!(missing.contains(document::FILE_NAME), "{missing}");
    }

    /// The side of a fake sandbox's capture.
    const FAKE_SIDE: u32 = 8;

    fn fake_image(side: u32) -> Image {
        Image { width: side, height: side, rgba: [40, 80, 120, 255].repeat((side * side) as usize) }
    }

    /// In a fake sandbox's environment: what it does (see [`fake_sandbox`]).
    const FAKE_ENV: &str = "SAPPHIRE_TESTKIT_FAKE_SANDBOX";
    /// In a fake crash monitor's environment: the file it writes as it exits.
    const MONITOR_MARKER_ENV: &str = "SAPPHIRE_TESTKIT_FAKE_MONITOR_MARKER";

    /// This test binary started again as a fake sandbox doing `behaviour`, with only
    /// [`fake_sandbox`] selected. The runner adds the capture's arguments after the `--`, where the
    /// test harness takes them as test names to match exactly, which they never do.
    fn fake(behaviour: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("the test binary"));
        command
            .args(["--exact", "scenes::tests::fake_sandbox", "--nocapture", "--"])
            .env(FAKE_ENV, behaviour);
        command
    }

    /// Not a test of its own: the stand-in for the sandbox and its crash monitor that
    /// [`a_scene_past_its_deadline_is_stopped_and_the_run_goes_on`] starts (see [`fake`]). Without
    /// [`FAKE_ENV`] it does nothing.
    #[test]
    fn fake_sandbox() {
        let Ok(behaviour) = std::env::var(FAKE_ENV) else { return };
        if behaviour == "monitor" {
            // As `sapphire_crash`'s monitor does, it exits once the sandbox's end of its stdin closes.
            let _ = io::stdin().read_to_end(&mut Vec::new());
            fs::write(std::env::var(MONITOR_MARKER_ENV).expect("the marker's path"), "exited")
                .expect("the marker");
            return;
        }
        let args: Vec<String> = std::env::args().collect();
        let capture = args.windows(2).find(|pair| pair[0] == "--capture").map(|pair| PathBuf::from(&pair[1]));
        let capture = capture.expect("--capture <png>");
        // Like the real monitor, it inherits this process's output and holds a lifeline only this
        // process has, until this process is gone.
        let mut monitor = fake("monitor")
            .env(MONITOR_MARKER_ENV, capture.with_extension("monitor-exited"))
            .stdin(Stdio::piped())
            .spawn()
            .expect("the fake monitor");
        let lifeline = monitor.stdin.take();
        // More than a pipe holds (64 KiB on Linux) on each of stdout and stderr.
        let chatter = |lines: usize| {
            for line in 0..lines {
                writeln!(io::stdout(), "stdout line {line}: {}", "x".repeat(100)).expect("stdout");
                eprintln!("stderr line {line}: {}", "x".repeat(100));
            }
        };
        match behaviour.as_str() {
            "hang" => {}
            "stall" => {
                chatter(1000);
                eprintln!("stalling now");
            }
            "fail-one" | "fail-two" => {
                eprintln!("failing now");
                drop(lifeline);
                monitor.wait().expect("the fake monitor's exit");
                std::process::exit(42);
            }
            "chatty" | "wrong-size-golden" | "corrupt-golden" | "truncated" => {
                if behaviour == "chatty" {
                    chatter(2000);
                }
                fake_image(FAKE_SIDE).write_png(&capture).expect("the capture");
                if behaviour == "truncated" {
                    let bytes = fs::read(&capture).expect("the capture");
                    fs::write(&capture, &bytes[..bytes.len() / 2]).expect("half the capture");
                }
                let record = "backend=vulkan\nadapter=llvmpipe (fake)\n";
                fs::write(capture.with_extension("adapter.txt"), record).expect("the adapter record");
                // A clean exit closes the lifeline, as the real sandbox's does, and waits.
                drop(lifeline);
                monitor.wait().expect("the fake monitor's exit");
                return;
            }
            other => panic!("no fake sandbox does {other:?}"),
        }
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    #[test]
    fn a_scene_past_its_deadline_is_stopped_and_the_run_goes_on() {
        // Both deadlines are real and unchanged. Independent output directories let these
        // process-lifecycle cases wait concurrently; each must continue to a successful capture.
        std::thread::scope(|scope| {
            let hang = scope.spawn(|| {
                let expected = [("fake-hang", "timed-out"), ("fake-chatty", "updated")];
                check_fake_run("deadline-hang", Mode::Update, &expected);
            });
            let stall = scope.spawn(|| {
                let expected = [("fake-stall", "timed-out"), ("fake-chatty", "updated")];
                check_fake_run("deadline-stall", Mode::Update, &expected);
            });
            hang.join().unwrap();
            stall.join().unwrap();
        });
    }

    #[test]
    fn consecutive_failures_are_collected_before_the_next_success() {
        // Failure accumulation still covers consecutive errors in one runner, without paying
        // two deadlines serially. Each child really exits unsuccessfully and reaps its monitor.
        check_fake_run(
            "failures",
            Mode::Update,
            &[("fake-fail-one", "error"), ("fake-fail-two", "error"), ("fake-chatty", "updated")],
        );
    }

    #[test]
    fn a_golden_or_capture_that_can_t_be_compared_fails_its_scene_and_the_run_goes_on() {
        // Review DV-8: each of these stopped the whole run, naming no scene.
        check_fake_run(
            "unreadable",
            Mode::Compare,
            &[
                ("fake-wrong-size-golden", "error"),
                ("fake-corrupt-golden", "error"),
                ("fake-truncated", "error"),
                ("fake-chatty", "ok"),
            ],
        );
    }

    #[test]
    fn update_keeps_a_capture_cut_short_out_of_the_goldens() {
        // It would pass for a golden until the next run compared against it.
        check_fake_run(
            "update-truncated",
            Mode::Update,
            &[("fake-truncated", "error"), ("fake-chatty", "updated")],
        );
    }

    fn check_fake_run(group: &str, mode: Mode, expected: &[(&'static str, &'static str)]) {
        let dir = std::env::temp_dir().join(format!("sapphire_testkit_{group}_{}", std::process::id()));
        let paths = Paths {
            sandbox: PathBuf::from("unused"),
            content: PathBuf::from("unused"),
            goldens: dir.join("goldens"),
            out: dir.join("out"),
        };
        let scenes: Vec<Scene> = expected.iter().map(|&(name, _)| scene(name, &["--plain-post"])).collect();
        let selected: Vec<&Scene> = scenes.iter().collect();
        let timeout = Duration::from_secs(3);
        let golden_of = |name: &str| paths.goldens.join(format!("{name}.vulkan-lavapipe.png"));
        if mode == Mode::Compare {
            fs::create_dir_all(&paths.goldens).unwrap();
            for &(name, _) in expected {
                match name {
                    "fake-wrong-size-golden" => {
                        fake_image(FAKE_SIDE * 2).write_png(&golden_of(name)).unwrap()
                    }
                    "fake-corrupt-golden" => fs::write(golden_of(name), "not a PNG").unwrap(),
                    _ => fake_image(FAKE_SIDE).write_png(&golden_of(name)).unwrap(),
                }
            }
        }
        let sandbox = |scene: &Scene| fake(scene.name.strip_prefix("fake-").expect("a fake"));
        let failures = run_scenes(&selected, &paths, mode, timeout, &sandbox).unwrap();
        let expected_failures: Vec<_> =
            expected.iter().filter(|(_, status)| !matches!(*status, "updated" | "ok")).collect();
        assert_eq!(failures.len(), expected_failures.len(), "{failures:#?}");
        for (failure, &&(name, status)) in failures.iter().zip(&expected_failures) {
            let unreadable = match name {
                "fake-wrong-size-golden" => Some("size mismatch: the golden is (16, 16), the capture (8, 8)"),
                "fake-corrupt-golden" => Some("not a PNG this harness reads"),
                "fake-truncated" => Some("not a PNG this harness reads"),
                _ => None,
            };
            if status == "timed-out" {
                assert!(failure.starts_with(&format!("{name}: timed out after 3 s")), "{failure}");
            } else if let Some(why) = unreadable {
                assert!(mode == Mode::Compare || !golden_of(name).exists(), "{name} became a golden");
                assert!(failure.starts_with(&format!("{name}: ")) && failure.contains(why), "{failure}");
                // The capture is kept, bytes as the sandbox wrote them, and the failure says where.
                let kept = paths.out.join(format!("{name}.vulkan-lavapipe.actual.png"));
                assert!(failure.contains(&kept.display().to_string()), "{failure}");
                assert_eq!(
                    fs::read(&kept).unwrap(),
                    fs::read(paths.out.join(format!("{name}.png"))).unwrap()
                );
            } else {
                assert!(failure.starts_with(&format!("{name}: the sandbox failed")), "{failure}");
                assert!(failure.contains("failing now"), "{failure}");
            }
            if name == "fake-stall" {
                assert!(failure.contains("stalling now"), "{failure}");
                // Its log is kept, all of it, including more output than a pipe holds.
                let log = fs::read_to_string(paths.out.join("fake-stall.log")).unwrap();
                assert!(
                    log.contains("stdout line 999")
                        && log.contains("stderr line 999")
                        && log.contains("stalling now")
                );
            }
        }
        // Every run goes on to a scene that writes far more than a pipe holds and finishes.
        assert!(paths.goldens.join("fake-chatty.vulkan-lavapipe.png").is_file());
        let log = fs::read_to_string(paths.out.join("fake-chatty.log")).unwrap();
        assert!(log.contains("stdout line 1999") && log.contains("stderr line 1999"));

        // Timings cover every scene in order, with the original real-time deadline and slack.
        let timings = fs::read_to_string(paths.out.join("timings.json")).unwrap();
        let entries: Vec<&str> = timings.lines().filter(|line| line.contains("\"scene\"")).collect();
        assert_eq!(entries.len(), expected.len(), "{timings}");
        for (entry, &(scene, status)) in entries.iter().zip(expected) {
            assert!(entry.contains(&format!("\"scene\": \"{scene}\"")), "{entry}");
            assert!(entry.contains(&format!("\"status\": \"{status}\"")), "{entry}");
            assert!(entry.contains("\"family\": \"vulkan-lavapipe\""), "{entry}");
            let seconds: f64 = entry
                .split("\"seconds\": ")
                .nth(1)
                .and_then(|rest| rest.split(',').next())
                .unwrap()
                .parse()
                .unwrap();
            if status == "timed-out" {
                assert!((3.0..8.0).contains(&seconds), "{entry}");
            }
        }

        // No crash monitor outlives its sandbox: each observed its lifeline closing.
        let clock = Stopwatch::start();
        for scene in &scenes {
            let marker = paths.out.join(format!("{}.monitor-exited", scene.name));
            while !marker.is_file() {
                assert!(clock.elapsed() < Duration::from_secs(30), "{}'s monitor still runs", scene.name);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
