//! `sapphire_testkit`: runs test suites that need a built app, as a process, because nothing may
//! depend on a tool crate (rule R1). `cargo xtask golden` builds the sandbox and this binary,
//! then runs `golden`.

use std::path::PathBuf;
use std::process::ExitCode;

use sapphire_testkit::scenes::{self, Mode, Paths};

const USAGE: &str = "\
Usage:
  sapphire_testkit golden --sandbox <exe> --content <dir> --goldens <dir> --out <dir>
                          [--update] [--scene <name>]
      Render the golden scenes with the sandbox and compare them with the goldens.";

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
    if let Some(unexpected) = args.first() {
        return Err(format!("unexpected argument `{unexpected}`"));
    }
    scenes::run(&paths, mode, scene.as_deref())
}
