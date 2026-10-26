//! `sapphire_testkit`: runs test suites that need a built app, as a process, because nothing may
//! depend on a tool crate (rule R1). `cargo xtask golden` builds the sandbox and this binary,
//! then runs `golden`.

use std::path::PathBuf;
use std::process::ExitCode;

use sapphire_testkit::scenes::{self, Mode, Paths, Selection};

const USAGE: &str = "\
Usage:
  sapphire_testkit golden --sandbox <exe> --content <dir> --goldens <dir> --out <dir>
                          [--update] [--scene <name> | --shard <index>/<count>]
      Render the golden scenes with the sandbox and compare them with the goldens.
      --shard renders every <count>-th scene from the <index>-th (from 1), so CI can split
      a backend's scenes across runners.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(failures) if failures.is_empty() => ExitCode::SUCCESS,
        Ok(failures) => {
            eprintln!("{} golden scene(s) failed:\n  {}", failures.len(), failures.join("\n  "));
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("error: {error}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(mut args: Vec<String>) -> Result<Vec<String>, String> {
    if args.first().map(String::as_str) != Some("golden") {
        return Err("expected the `golden` command".into());
    }
    args.remove(0);
    let mut flag = |name: &str| args.iter().position(|arg| arg == name).map(|i| args.remove(i)).is_some();
    let mode = if flag("--update") { Mode::Update } else { Mode::Compare };
    let mut value = |name: &str| -> Result<Option<String>, String> {
        let Some(i) = args.iter().position(|arg| arg == name) else { return Ok(None) };
        args.remove(i);
        (i < args.len()).then(|| args.remove(i)).map(Some).ok_or_else(|| format!("{name} needs a value"))
    };
    let mut path = |name: &str| value(name)?.map(PathBuf::from).ok_or_else(|| format!("missing {name}"));
    let paths = Paths {
        sandbox: path("--sandbox")?,
        content: path("--content")?,
        goldens: path("--goldens")?,
        out: path("--out")?,
    };
    let scene = value("--scene")?;
    let shard = value("--shard")?.map(|text| text.parse()).transpose()?;
    if let Some(unexpected) = args.first() {
        return Err(format!("unexpected argument `{unexpected}`"));
    }
    let selection = match (scene.as_deref(), shard) {
        (None, None) => Selection::All,
        (Some(name), None) => Selection::Scene(name),
        (None, Some(shard)) => Selection::Shard(shard),
        (Some(_), Some(_)) => return Err("--scene and --shard can't be combined".into()),
    };
    scenes::run(&paths, mode, selection)
}
