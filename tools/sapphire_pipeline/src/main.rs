//! `sapphire_pipeline`: command-line front end to the asset pipeline.
//!
//! The arguments are parsed by hand, as in xtask: three commands and four options don't
//! justify clap's build time, which xtask's cook pays in a release build too.

use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use anyhow::{Context as _, anyhow, bail};
use sapphire_pipeline::bake::{self, OutlineSettings};
use sapphire_pipeline::cook::{self, Cook, CookSettings};
use sapphire_pipeline::samples;

const USAGE: &str = "\
Sapphire asset pipeline: generate, import, bake and cook source assets.

Usage:
  sapphire_pipeline gen-samples --out <DIR>
      Write the procedural sample source assets (mannequin.glb) into DIR, created if missing.
  sapphire_pipeline cook-mesh <INPUT> <OUTPUT> [--cook <COOK>] [--weld-tolerance <METERS>]
      Cook one .gltf/.glb file into a .smesh file (default cook: desktop).
  sapphire_pipeline cook --source <DIR> --out <DIR> --cook <COOK> [--weld-tolerance <METERS>]
      Cook every .gltf/.glb under DIR (skipping hidden entries) into mirrored .smesh paths.

Options:
  --cook <COOK>              Target cook: desktop or mobile.
  --weld-tolerance <METERS>  Vertices closer than this share one outline normal.
  -h, --help                 Show this message.
  -V, --version              Show the version.";

#[derive(Debug, PartialEq)]
enum Command {
    GenSamples { out: PathBuf },
    CookMesh { input: PathBuf, output: PathBuf, settings: CookSettings },
    Cook { source: PathBuf, out: PathBuf, settings: CookSettings },
    Help,
    Version,
}

fn main() -> ExitCode {
    let args: Result<Vec<String>, _> = std::env::args_os().skip(1).map(|arg| arg.into_string()).collect();
    let command = match args.map_err(|arg| anyhow!("argument {arg:?} is not valid UTF-8")).and_then(parse) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("error: {error:#}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:?}");
            ExitCode::FAILURE
        }
    }
}

#[expect(clippy::print_stdout, reason = "--help and --version answer on stdout; progress goes to stderr")]
fn run(command: Command) -> anyhow::Result<()> {
    match command {
        Command::GenSamples { out } => {
            let written = samples::write_all(&out)
                .with_context(|| format!("failed to write samples to {}", out.display()))?;
            for path in written {
                eprintln!("generated {}", path.display());
            }
        }
        Command::CookMesh { input, output, settings } => {
            cook::cook_mesh_file(&input, &output, &settings)?;
            eprintln!("cooked {} -> {} ({})", input.display(), output.display(), settings.cook);
        }
        Command::Cook { source, out, settings } => {
            let cooked = cook::cook_dir(&source, &out, &settings)?;
            for file in &cooked {
                eprintln!("cooked {} -> {}", file.source.display(), file.output.display());
            }
            eprintln!("{} asset(s) cooked for {} into {}", cooked.len(), settings.cook, out.display());
        }
        Command::Help => println!("{USAGE}"),
        Command::Version => println!("sapphire_pipeline {}", env!("CARGO_PKG_VERSION")),
    }
    Ok(())
}

fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Command> {
    let mut args: Vec<String> = args.into_iter().collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(Command::Help);
    }
    if args.iter().any(|arg| arg == "-V" || arg == "--version") {
        return Ok(Command::Version);
    }
    if args.is_empty() {
        bail!("missing command");
    }
    let name = args.remove(0);
    let mut options = Options(args);
    let command = match name.as_str() {
        "gen-samples" => Command::GenSamples { out: options.required("--out")? },
        "cook-mesh" => {
            let cook = options.value("--cook")?.unwrap_or(Cook::Desktop);
            let settings = settings(cook, &mut options)?;
            let [input, output] = options.positional()?;
            Command::CookMesh { input: input.into(), output: output.into(), settings }
        }
        "cook" => {
            let source = options.required("--source")?;
            let out = options.required("--out")?;
            let cook = options.required("--cook")?;
            Command::Cook { source, out, settings: settings(cook, &mut options)? }
        }
        other => bail!("unknown command `{other}`"),
    };
    options.finish()?;
    Ok(command)
}

/// Cook settings from `--weld-tolerance`.
fn settings(cook: Cook, options: &mut Options) -> anyhow::Result<CookSettings> {
    let tolerance = options.value("--weld-tolerance")?.unwrap_or(bake::DEFAULT_WELD_TOLERANCE);
    let outline = OutlineSettings::with_weld_tolerance(tolerance)
        .ok_or_else(|| anyhow!("invalid --weld-tolerance: must be a positive, finite number of meters"))?;
    Ok(CookSettings { cook, outline })
}

/// The arguments after the command, consumed option by option.
struct Options(Vec<String>);

impl Options {
    /// Takes `--name value` or `--name=value` and parses the value.
    fn value<T>(&mut self, name: &str) -> anyhow::Result<Option<T>>
    where
        T: FromStr<Err: std::error::Error + Send + Sync + 'static>,
    {
        let prefix = format!("{name}=");
        let Some(i) = self.0.iter().position(|arg| *arg == name || arg.starts_with(&prefix)) else {
            return Ok(None);
        };
        let arg = self.0.remove(i);
        let value = match arg.strip_prefix(&prefix) {
            Some(value) => value.to_owned(),
            None if i < self.0.len() && !self.0[i].starts_with("--") => self.0.remove(i),
            None => bail!("{name} needs a value"),
        };
        value.parse().map(Some).with_context(|| format!("invalid {name} {value:?}"))
    }

    fn required<T>(&mut self, name: &str) -> anyhow::Result<T>
    where
        T: FromStr<Err: std::error::Error + Send + Sync + 'static>,
    {
        self.value(name)?.with_context(|| format!("missing required option {name}"))
    }

    /// Takes exactly `N` positional arguments, in order.
    fn positional<const N: usize>(&mut self) -> anyhow::Result<[String; N]> {
        let found: Vec<String> = self.0.extract_if(.., |arg| !arg.starts_with("--")).collect();
        let count = found.len();
        found.try_into().map_err(|_| anyhow!("expected {N} paths, got {count}"))
    }

    fn finish(self) -> anyhow::Result<()> {
        match self.0.first() {
            Some(unexpected) => bail!("unexpected argument `{unexpected}`"),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<Command> {
        super::parse(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(
            parse(&["gen-samples", "--out", "dir"]).unwrap(),
            Command::GenSamples { out: "dir".into() }
        );
        assert_eq!(
            parse(&["cook-mesh", "a.glb", "--cook=mobile", "b.smesh"]).unwrap(),
            Command::CookMesh {
                input: "a.glb".into(),
                output: "b.smesh".into(),
                settings: CookSettings::new(Cook::Mobile)
            }
        );
        let Command::Cook { source, out, settings } = parse(&[
            "cook",
            "--source",
            "content",
            "--out",
            "out",
            "--cook",
            "desktop",
            "--weld-tolerance",
            "0.001",
        ])
        .unwrap() else {
            panic!("expected cook");
        };
        assert_eq!((source, out, settings.cook), ("content".into(), "out".into(), Cook::Desktop));
        assert_eq!(settings.outline, OutlineSettings::with_weld_tolerance(0.001).unwrap());
        assert_eq!(parse(&["cook", "--help"]).unwrap(), Command::Help);
        assert_eq!(parse(&["-V"]).unwrap(), Command::Version);
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["bake"]).is_err(), "unknown command");
        assert!(parse(&["gen-samples"]).is_err(), "missing --out");
        assert!(parse(&["gen-samples", "--out"]).is_err(), "--out without a value");
        assert!(parse(&["cook", "--source", "a", "--out", "b"]).is_err(), "missing --cook");
        assert!(parse(&["cook", "--source", "a", "--out", "b", "--cook", "console"]).is_err());
        assert!(parse(&["cook-mesh", "a.glb"]).is_err(), "one path");
        assert!(parse(&["cook-mesh", "a.glb", "b", "--weld-tolerance", "-1"]).is_err());
        assert!(parse(&["gen-samples", "--out", "dir", "--extra"]).is_err());
    }
}
