//! `sapphire_testkit`: runs test suites that need a built app, as a process, because nothing may
//! depend on a tool crate (rule R1). `cargo xtask golden` builds the sandbox and this binary,
//! then runs `golden`, `cargo xtask store-shots` runs `store-shots` and `cargo xtask counts` runs
//! `counts`, and `cargo xtask bench-scale` runs `scale`. `measure` reads lines and edges from two captures of one
//! shot.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use sapphire_testkit::counts;
use sapphire_testkit::flashes::{FlashAnalyser, Verdict};
use sapphire_testkit::golden::{self, Image, Thresholds};
use sapphire_testkit::look::{Lines, Region, Spread};
use sapphire_testkit::scale::{self, manifest::ScaleBench};
use sapphire_testkit::scenes::{self, DEFAULT_SCENE_TIMEOUT, Mode, Paths, Selection};
use sapphire_testkit::store_shots;

const USAGE: &str = "\
Usage:
  sapphire_testkit golden --sandbox <exe> --content <dir> --goldens <dir> --out <dir>
                          [--update] [--scene <name> | --shard <index>/<count>]
                          [--scene-timeout <seconds>]
      Render the golden scenes with the sandbox and compare them with the goldens.
      --shard renders slice <index> of <count> (from 1), so CI can split a backend's scenes
      across runners: split by the scenes' CI times for the family SAPPHIRE_GPU_BACKEND
      forces (vulkan: lavapipe, dx12: WARP), otherwise every <count>-th scene.
      --scene-timeout stops a scene's sandbox after that long and fails the scene
      (default 900). Each scene's output is in <out>/<scene>.log, and every scene's time
      in <out>/timings.json.
  sapphire_testkit store-shots --sandbox <exe> --content <dir> --tests <dir> --out <dir>
                               [--scene-timeout <seconds>]
      Draw the stores' screenshots (sapphire_testkit::store_shots) into <out>/<store>/<size>/
      <language>/, emptied first, with each capture's log in <out>/logs/.
  sapphire_testkit counts --sandbox <exe> --content <dir> [--mobile-content <dir>] --table <file>
                          --out <dir> [--sources <dir>] [--mobile-sources <dir>] [--update]
                          [--differences <file>] [--walk <name>]...
                          [--walk-timeout <seconds>]
  sapphire_testkit counts --table <file> --differences <file> --refresh <dir>...
      Walk the benchmark district's tour with the sandbox's walk bench at each tier's
      settings, dusk and night, and compare each walk's count rows with the table: one line
      for each row that moved, with what the table holds, what the walk saw and which way.
      --content is a desktop cook with the district's pack and a release manifest naming it,
      which the desktop tiers read, and --mobile-content the same for the mobile cook, which
      the phone tiers read (`cargo xtask counts` makes both). --sources is the district's generated sources, whose
      digest the table names, and --mobile-sources the phones' district's, beside it. --walk runs one walk (more than once for more); --update
      rewrites those walks' rows. Each walk's log is <out>/<walk>.log, and what every walk
      counted is <out>/counts.txt, a table (sapphire_testkit::counts). A phone walk needs
      --mobile-content. --differences holds each walk to the table plus a GPU family's
      differences from it (tests/counts/district-ci-<family>.txt, sapphire_testkit::counts::differences);
      without it the table holds exactly. --refresh rewrites the differences file from finished
      runs' folders, each with the counts.txt a run wrote (CI's scale-lane-<walk> artifacts),
      walking nothing, keeping each line's reason and naming each new line, which needs one.
  sapphire_testkit scale plan --manifest <file> [--smoke] [--out <dir>]
  sapphire_testkit scale run --manifest <file> --sandbox <exe> --desktop <dir> --mobile <dir>
                             --replays <dir> --out <dir> [--smoke] [--sitting <n> | --counts]
  sapphire_testkit scale run --manifest <file> --out <dir> --device <name> [--tier <name>]...
                             [--smoke] --launch <program> [<argument>...]
  sapphire_testkit scale preset --manifest <file> [--device <name>]
  sapphire_testkit scale report --manifest <file> --out <dir> [--baseline <dir>] [--commit <name>]
                                [--smoke]
      The benches at scale (sapphire_testkit::scale, `cargo xtask bench-scale`). `plan` prints the
      manifest's sittings and their minutes by tier, and with --out what is done. `run` runs one
      sitting, the next with a unit left unless --sitting names one, skipping complete units, or
      with --counts every run of counts alone; it takes no gate slot. --desktop and --mobile are
      each cook's packs with the district's beside them. Each unit writes <out>/<unit>.log and
      .status (and .csv where its run traces). --smoke runs every run at 200 frames at 320x180.
      With --device, `run` runs the units that walk on that device (on the tiers --tier names, or
      all), each through --launch and its arguments, which take everything after it: given `--`
      and the unit's sandbox arguments, it starts the sandbox on the booted device and prints its
      log (`cargo xtask bench-scale device` passes `xtask bench-scale launch`). It then prints the
      device's verdict lines and fails on a fail. `preset` prints the preset the batch mounts, or
      the device's.
      `report` writes <out>/report.md from the logs, beside the baseline's folder of logs named
      as the runs they stand beside, and fails on counts that moved where they must be exact.
  sapphire_testkit measure <with lines.png> <without lines.png> [--region <x,y,width,height>]
      Measure two captures of one shot, drawn with lines and without (the sandbox's
      --outline-distance 0), over the region or the whole image, in output pixels: the
      lines' strength (width times darkness), contrast and width, and the spread of the
      second capture's edges (sapphire_testkit::look). Compare two scales or tiers of a
      shot by their ratios.
  sapphire_testkit compare <reference.png> <capture.png>
      Compare two images as a golden is compared: FLIP's mean, p99, worst 16x16 tile and
      maximum, and whether the golden gates pass.
  sapphire_testkit flashes <dir> [--rate <frames a second>] [--frames <length>] [--trace]
      Run WCAG 2.3.1's general and red flash tests over the .png frames in <dir>, in the
      order of their names, captured at --rate (default 60). Refuses fewer frames than a
      second's, or than --frames, the sequence's length (a fight's ticks with
      --capture-frames <ticks> --capture-step 1), as too short to judge. One line of the most
      transitions a second over a quarter of a 10-degree field and the largest share of a
      field over the limit, for each test (sapphire_testkit::flashes). --trace adds a
      line for each frame where a field made a transition, with the most any field made
      in the second ending there. Fails a sequence that doesn't pass.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(failures) if failures.is_empty() => ExitCode::SUCCESS,
        Ok(failures) => {
            eprintln!("{} failed:\n  {}", failures.len(), failures.join("\n  "));
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("error: {error}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(mut args: Vec<String>) -> Result<Vec<String>, String> {
    match args.first().map(String::as_str) {
        Some("golden" | "store-shots") => {}
        Some("measure") => return measure(&args[1..]).map(|()| Vec::new()),
        Some("compare") => return compare(&args[1..]).map(|()| Vec::new()),
        Some("flashes") => return flashes(&args[1..]),
        Some("counts") => return counts(args.split_off(1)),
        Some("scale") => return scale(args.split_off(1)),
        _ => {
            return Err(
                "expected the `golden`, `store-shots`, `counts`, `scale`, `measure`, `compare` or `flashes` command"
                    .into(),
            );
        }
    }
    let shots = args.remove(0) == "store-shots";
    let mut flag = |name: &str| args.iter().position(|arg| arg == name).map(|i| args.remove(i)).is_some();
    let mode = if flag("--update") { Mode::Update } else { Mode::Compare };
    let mut value = |name: &str| -> Result<Option<String>, String> {
        let Some(i) = args.iter().position(|arg| arg == name) else { return Ok(None) };
        args.remove(i);
        (i < args.len()).then(|| args.remove(i)).map(Some).ok_or_else(|| format!("{name} needs a value"))
    };
    let mut path = |name: &str| value(name)?.map(PathBuf::from).ok_or_else(|| format!("missing {name}"));
    if shots {
        let paths = store_shots::Paths {
            sandbox: path("--sandbox")?,
            content: path("--content")?,
            tests: path("--tests")?,
            out: path("--out")?,
        };
        let timeout = timeout(value("--scene-timeout")?)?;
        if let Some(unexpected) = args.first() {
            return Err(format!("unexpected argument `{unexpected}`"));
        }
        return store_shots::run(&paths, timeout);
    }
    let paths = Paths {
        sandbox: path("--sandbox")?,
        content: path("--content")?,
        goldens: path("--goldens")?,
        out: path("--out")?,
    };
    let scene = value("--scene")?;
    let shard = value("--shard")?.map(|text| text.parse()).transpose()?;
    let timeout = timeout(value("--scene-timeout")?)?;
    if let Some(unexpected) = args.first() {
        return Err(format!("unexpected argument `{unexpected}`"));
    }
    let selection = match (scene.as_deref(), shard) {
        (None, None) => Selection::All,
        (Some(name), None) => Selection::Scene(name),
        (None, Some(shard)) => Selection::Shard(shard),
        (Some(_), Some(_)) => return Err("--scene and --shard can't be combined".into()),
    };
    scenes::run(&paths, mode, selection, timeout)
}

/// `counts`: the scale lane's walks against its table.
fn counts(mut args: Vec<String>) -> Result<Vec<String>, String> {
    let update = args.iter().position(|arg| arg == "--update").map(|i| args.remove(i)).is_some();
    let mode = if update { Mode::Update } else { Mode::Compare };
    let mut value = |name: &str| -> Result<Option<String>, String> {
        let Some(i) = args.iter().position(|arg| arg == name) else { return Ok(None) };
        args.remove(i);
        (i < args.len()).then(|| args.remove(i)).map(Some).ok_or_else(|| format!("{name} needs a value"))
    };
    let mut walks = Vec::new();
    while let Some(walk) = value("--walk")? {
        walks.push(walk);
    }
    let mut folders = Vec::new();
    while let Some(folder) = value("--refresh")? {
        folders.push(PathBuf::from(folder));
    }
    let mut named = |name: &str| value(name).map(|found| found.map(PathBuf::from));
    let sandbox = named("--sandbox")?;
    let content = named("--content")?;
    let mobile_content = named("--mobile-content")?;
    let table = named("--table")?;
    let differences = named("--differences")?;
    let out = named("--out")?;
    let sources = named("--sources")?;
    let mobile_sources = named("--mobile-sources")?;
    let walk_timeout = value("--walk-timeout")?;
    if let Some(unexpected) = args.first() {
        return Err(format!("unexpected argument `{unexpected}`"));
    }
    if !folders.is_empty() {
        let (Some(table), Some(differences)) = (table, differences) else {
            return Err("--refresh needs --table and --differences".into());
        };
        if update || !walks.is_empty() || sandbox.is_some() || out.is_some() {
            return Err("--refresh walks nothing: it takes no --update, --walk, --sandbox or --out".into());
        }
        return counts::refresh_differences(&table, &differences, &folders);
    }
    let missing = |name: &str| format!("missing {name}");
    let paths = counts::Paths {
        sandbox: sandbox.ok_or_else(|| missing("--sandbox"))?,
        content: content.ok_or_else(|| missing("--content"))?,
        mobile_content,
        table: table.ok_or_else(|| missing("--table"))?,
        differences,
        out: out.ok_or_else(|| missing("--out"))?,
        sources,
        mobile_sources,
    };
    let timeout = seconds("--walk-timeout", walk_timeout, counts::DEFAULT_WALK_TIMEOUT)?;
    counts::run(&paths, mode, &walks, timeout)
}

/// `scale`: the benches at scale's plan, a sitting of them, or their report.
#[allow(clippy::print_stdout, reason = "the plan and the report's path are the command's output")]
fn scale(mut args: Vec<String>) -> Result<Vec<String>, String> {
    if args.is_empty() {
        return Err("scale expects `plan`, `run`, `preset` or `report`".into());
    }
    let command = args.remove(0);
    // `--launch` takes every argument after it, its program's own.
    let launch = args.iter().position(|arg| arg == "--launch").map(|i| args.split_off(i).split_off(1));
    let mut flag = |name: &str| args.iter().position(|arg| arg == name).map(|i| args.remove(i)).is_some();
    let smoke = flag("--smoke").then_some(scale::manifest::SMOKE);
    let counts = flag("--counts");
    let mut value = |name: &str| -> Result<Option<String>, String> {
        let Some(i) = args.iter().position(|arg| arg == name) else { return Ok(None) };
        args.remove(i);
        (i < args.len()).then(|| args.remove(i)).map(Some).ok_or_else(|| format!("{name} needs a value"))
    };
    let manifest_path = value("--manifest")?.ok_or("missing --manifest")?;
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("failed to read {manifest_path}: {e}"))?;
    let manifest = ScaleBench::parse(&text).map_err(|error| format!("{manifest_path}: {error}"))?;
    let out = value("--out")?.map(PathBuf::from);
    let device = value("--device")?;
    if launch.is_some() && device.is_none() {
        return Err("--launch runs a device's units: it goes with --device".into());
    }
    match (command.as_str(), device) {
        ("plan", None) => {
            let (sandbox, desktop, mobile, replays) =
                (PathBuf::new(), PathBuf::new(), PathBuf::new(), PathBuf::new());
            let paths = out.map(|out| scale::Paths { sandbox, desktop, mobile, replays, out });
            finish(&args)?;
            print!("{}", scale::plan(&manifest, smoke, paths.as_ref()));
            Ok(Vec::new())
        }
        ("run", Some(device)) => {
            let mut tiers = Vec::new();
            while let Some(tier) = value("--tier")? {
                tiers.push(tier);
            }
            let out = out.ok_or("missing --out")?;
            let launch = launch.ok_or("a device's units need --launch <program> [<argument>...]")?;
            if counts || launch.is_empty() {
                return Err("--device takes --launch <program>, and not --counts".into());
            }
            finish(&args)?;
            scale::run_on_device(&manifest, &out, &device, &tiers, &launch, smoke)
        }
        ("preset", device) => {
            finish(&args)?;
            let preset = match device {
                None => &manifest.preset,
                Some(name) => {
                    let index = manifest
                        .device(&name)
                        .ok_or_else(|| format!("the manifest lists no device {name:?}"))?;
                    &manifest.devices[index].preset
                }
            };
            println!("{preset}");
            Ok(Vec::new())
        }
        ("run", None) => {
            let mut path =
                |name: &str| value(name)?.map(PathBuf::from).ok_or_else(|| format!("missing {name}"));
            let paths = scale::Paths {
                sandbox: path("--sandbox")?,
                desktop: path("--desktop")?,
                mobile: path("--mobile")?,
                replays: path("--replays")?,
                out: out.ok_or("missing --out")?,
            };
            let sitting = value("--sitting")?;
            finish(&args)?;
            let select = match (sitting, counts) {
                (None, false) => scale::Select::Next,
                (None, true) => scale::Select::Counts,
                (Some(number), false) => scale::Select::Sitting(
                    number
                        .parse()
                        .map_err(|_| format!("--sitting expects a number from 1, got {number:?}"))?,
                ),
                (Some(_), true) => return Err("--sitting and --counts can't be combined".into()),
            };
            scale::run(&manifest, &paths, select, smoke)
        }
        ("report", None) => {
            let out = out.ok_or("missing --out")?;
            let baseline = value("--baseline")?.map(PathBuf::from);
            let commit = value("--commit")?.unwrap_or_else(|| out.display().to_string());
            finish(&args)?;
            let inputs = scale::report::Inputs {
                manifest: &manifest,
                logs: &out,
                baseline: baseline.as_deref(),
                commit: &commit,
                smoke: smoke.is_some(),
            };
            let (text, failures) = scale::report::report(&inputs);
            let written = out.join("report.md");
            std::fs::write(&written, text)
                .map_err(|e| format!("failed to write {}: {e}", written.display()))?;
            println!("{}", written.display());
            Ok(failures)
        }
        (_, Some(_)) => Err("--device goes with `run` and `preset`".into()),
        (other, None) => Err(format!("scale expects `plan`, `run`, `preset` or `report`, not {other:?}")),
    }
}

/// Fails on an argument nothing took.
fn finish(args: &[String]) -> Result<(), String> {
    args.first().map_or(Ok(()), |unexpected| Err(format!("unexpected argument `{unexpected}`")))
}

/// `--scene-timeout`'s value, or the default.
fn timeout(text: Option<String>) -> Result<Duration, String> {
    seconds("--scene-timeout", text, DEFAULT_SCENE_TIMEOUT)
}

/// A flag's whole seconds from 1, or `default`.
fn seconds(flag: &str, text: Option<String>, default: Duration) -> Result<Duration, String> {
    match text {
        Some(text) => match text.parse::<u64>() {
            Ok(seconds) if seconds > 0 => Ok(Duration::from_secs(seconds)),
            _ => Err(format!("invalid {flag} {text:?}: expected whole seconds from 1")),
        },
        None => Ok(default),
    }
}

/// `measure`: prints one line of the two captures' lines and edges.
#[allow(clippy::print_stdout, reason = "the line is the command's output, for scripts to read")]
fn measure(args: &[String]) -> Result<(), String> {
    let (paths, region) = match args {
        [with, without] => ([with, without], None),
        [with, without, flag, region] if flag == "--region" => {
            ([with, without], Some(region.parse::<Region>()?))
        }
        _ => return Err("measure expects two captures and an optional --region".into()),
    };
    let [with, without] = paths.map(|path| read(path));
    let (with, without) = (with?, without?);
    let region = region.unwrap_or(Region::of(&with));
    let lines = Lines::between(&with, &without, region)?;
    let spread = Spread::of(&without, region, |_, _| false)?;
    println!(
        "lines_strength={:.1} lines_contrast={:.4} lines_width={:.1} edge_total={:.3} edge_spread={:.1}",
        lines.strength,
        lines.contrast(),
        lines.width(),
        spread.total,
        spread.width(),
    );
    Ok(())
}

/// `compare`: prints one line of FLIP's numbers for a capture against a reference.
#[allow(clippy::print_stdout, reason = "the line is the command's output, for scripts to read")]
fn compare(args: &[String]) -> Result<(), String> {
    let [reference, capture] = args else { return Err("compare expects a reference and a capture".into()) };
    let thresholds = Thresholds::default();
    let comparison = golden::compare(&read(reference)?, &read(capture)?, &thresholds)
        .map_err(|error| error.to_string())?;
    println!(
        "flip_mean={:.4} flip_p99={:.4} worst_tile={:.4} max={:.4} gates={}",
        comparison.mean,
        comparison.percentile_value,
        comparison.worst_tile,
        comparison.max,
        if comparison.passes(&thresholds) { "pass" } else { "fail" },
    );
    Ok(())
}

/// `flashes`: prints one line of WCAG 2.3.1's flash tests over a directory of frames, in the
/// order of their names, and fails a sequence that doesn't pass.
#[allow(clippy::print_stdout, reason = "the line is the command's output, for scripts to read")]
fn flashes(args: &[String]) -> Result<Vec<String>, String> {
    let (mut dir, mut rate, mut length, mut trace) = (None, 60, None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut number = |flag: &str, what: &str| {
            let value = args.next().ok_or_else(|| format!("{flag} expects {what}"))?;
            value.parse::<u32>().map_err(|_| format!("invalid {flag} {value:?}: expected {what}"))
        };
        match arg.as_str() {
            "--trace" => trace = true,
            "--rate" => rate = number("--rate", "frames a second")?,
            "--frames" => length = Some(number("--frames", "the sequence's length in frames")?),
            _ if dir.is_none() && !arg.starts_with("--") => dir = Some(arg),
            _ => {
                return Err(
                    "flashes expects a directory of frames, an optional --rate, --frames and --trace".into(),
                );
            }
        }
    }
    let Some(dir) = dir else { return Err("flashes expects a directory of frames".into()) };
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{dir}: {error}"))?;
    let mut frames = entries
        .map(|entry| entry.map(|entry| entry.path()).map_err(|error| format!("{dir}: {error}")))
        .collect::<Result<Vec<_>, _>>()?;
    frames.retain(|path| path.extension().is_some_and(|extension| extension == "png"));
    frames.sort();
    let Some(first) = frames.first() else { return Err(format!("{dir} has no .png frames")) };
    let first = read(&first.to_string_lossy())?;
    let mut analyser =
        FlashAnalyser::new(first.width, first.height, rate).map_err(|error| error.to_string())?;
    for path in &frames {
        let frame = read(&path.to_string_lossy())?;
        analyser.push(&frame).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let report = analyser.report(length).map_err(|error| format!("{dir}: {error}"))?;
    if trace {
        let [general, red] = analyser.trace();
        for (frame, (general, red)) in general.iter().zip(red).enumerate() {
            if general.transition || red.transition {
                println!("trace frame={frame} general={} red={}", general.most, red.most);
            }
        }
        for (test, verdict) in [("general", report.general), ("red", report.red)] {
            if let Some((frame, _)) = verdict.rate_at {
                let frames: Vec<String> = (0..128)
                    .filter(|bit| (verdict.rate_frames >> bit) & 1 != 0)
                    .map(|bit| (frame - bit).to_string())
                    .rev()
                    .collect();
                println!("trace {test}_transitions={}", frames.join(","));
            }
        }
    }
    let at = |verdict: &Verdict| match verdict.rate_at.or(verdict.area_at) {
        Some((frame, field)) => format!("{frame}@{},{},{},{}", field.x, field.y, field.width, field.height),
        None => "-".to_owned(),
    };
    println!(
        "frames={} rate={rate} size={}x{} general_rate={} general_area={:.3} general_at={} red_rate={} \
         red_area={:.3} red_at={} wcag_2_3_1={}",
        report.frames,
        first.width,
        first.height,
        report.general.rate,
        report.general.area,
        at(&report.general),
        report.red.rate,
        report.red.area,
        at(&report.red),
        if report.passes() { "pass" } else { "fail" },
    );
    Ok(if report.passes() { Vec::new() } else { vec![format!("{dir} fails WCAG 2.3.1's flash tests")] })
}

fn read(path: &str) -> Result<Image, String> {
    Image::read_png(path.as_ref()).map_err(|error| error.to_string())
}
