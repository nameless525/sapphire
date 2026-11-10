//! `sapphire_testkit`: runs test suites that need a built app, as a process, because nothing may
//! depend on a tool crate (rule R1). `cargo xtask golden` builds the sandbox and this binary,
//! then runs `golden`, and `cargo xtask store-shots` runs `store-shots`. `measure` reads lines and edges from
//! two captures of one shot.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use sapphire_testkit::flashes::{FlashAnalyser, Verdict};
use sapphire_testkit::golden::{self, Image, Thresholds};
use sapphire_testkit::look::{Lines, Region, Spread};
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
  sapphire_testkit measure <with lines.png> <without lines.png> [--region <x,y,width,height>]
      Measure two captures of one shot, drawn with lines and without (the sandbox's
      --outline-distance 0), over the region or the whole image, in output pixels: the
      lines' strength (width times darkness), contrast and width, and the spread of the
      second capture's edges (sapphire_testkit::look). Compare two scales or tiers of a
      shot by their ratios.
  sapphire_testkit compare <reference.png> <capture.png>
      Compare two images as a golden is compared: FLIP's mean, p99, worst 16x16 tile and
      maximum, and whether the golden gates pass.
  sapphire_testkit flashes <dir> [--rate <frames a second>]
      Run WCAG 2.3.1's general and red flash tests over the .png frames in <dir>, in the
      order of their names, captured at --rate (default 60): one line of the most
      transitions a second over a quarter of a 10-degree field and the largest share of a
      field over the limit, for each test (sapphire_testkit::flashes). Fails a sequence
      that doesn't pass.";

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
        _ => {
            return Err(
                "expected the `golden`, `store-shots`, `measure`, `compare` or `flashes` command".into()
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

/// `--scene-timeout`'s value, or the default.
fn timeout(text: Option<String>) -> Result<Duration, String> {
    match text {
        Some(text) => match text.parse::<u64>() {
            Ok(seconds) if seconds > 0 => Ok(Duration::from_secs(seconds)),
            _ => Err(format!("invalid --scene-timeout {text:?}: expected whole seconds from 1")),
        },
        None => Ok(DEFAULT_SCENE_TIMEOUT),
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
    let (dir, rate) = match args {
        [dir] => (dir, 60),
        [dir, flag, rate] if flag == "--rate" => {
            (dir, rate.parse().map_err(|_| format!("invalid --rate {rate:?}: expected frames a second"))?)
        }
        _ => return Err("flashes expects a directory of frames and an optional --rate".into()),
    };
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
    let report = analyser.report();
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
