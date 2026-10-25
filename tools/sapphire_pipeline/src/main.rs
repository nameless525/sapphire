//! `sapphire_pipeline`: command-line front end to the asset pipeline.
//!
//! The arguments are parsed by hand, as in xtask: three commands and a handful of options don't
//! justify clap's build time, which xtask's cook pays in a release build too.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use anyhow::{Context as _, anyhow, bail};
use sapphire_pipeline::bake::OutlineSettings;
use sapphire_pipeline::cook::{self, Cook, CookOptions, CookReport, CookStatus};
use sapphire_pipeline::meta::{self, MeshSettings};
use sapphire_pipeline::samples;

const USAGE: &str = "\
Sapphire asset pipeline: generate, import, bake and cook source assets.

Usage:
  sapphire_pipeline gen-samples --out <DIR>
      Write the procedural sample source assets (mannequin.glb) into DIR, created if missing.
  sapphire_pipeline cook-mesh <INPUT> <OUTPUT> [--cook <COOK>] [--weld-tolerance <METERS>]
      Cook one .gltf/.glb file into a .smesh file (default cook: desktop), with the settings
      of its .meta sidecar if it has one. Creates no sidecar and uses no cache.
  sapphire_pipeline cook --source <DIR> --out <DIR> --cook <COOK> [--cache <DIR>] [--locked]
      Cook every source asset under --source (skipping hidden entries) into mirrored paths
      under --out, creating missing .meta sidecars next to the sources. Outputs are written
      only when they change, and stale cooked files under --out are removed.

Options:
  --cook <COOK>              Target cook: desktop or mobile.
  --cache <DIR>              Content cache: reuse the cooked output of any asset whose
                             inputs are unchanged, and store new ones.
  --locked                   Fail on a source without a sidecar instead of creating one (CI).
  --weld-tolerance <METERS>  Vertices closer than this share one outline normal.
  -h, --help                 Show this message.
  -V, --version              Show the version.";

#[derive(Debug, PartialEq)]
enum Command {
    GenSamples { out: PathBuf },
    CookMesh { input: PathBuf, output: PathBuf, cook: Cook, outline: Option<OutlineSettings> },
    Cook { source: PathBuf, out: PathBuf, cook: Cook, cache: Option<PathBuf>, locked: bool },
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
        Command::CookMesh { input, output, cook, outline } => {
            let sidecar = meta::sidecar_path(&input);
            let mut settings = if sidecar.exists() {
                let meta = meta::read(&sidecar)?;
                meta.importer
                    .mesh()
                    .with_context(|| format!("{} isn't a mesh's sidecar", sidecar.display()))?
                    .clone()
            } else {
                MeshSettings::default()
            };
            if let Some(outline) = outline {
                settings.outline = outline;
            }
            cook::cook_mesh_file(&input, &output, &settings, cook)?;
            eprintln!("cooked {} -> {} ({cook})", input.display(), output.display());
        }
        Command::Cook { source, out, cook, cache, locked } => {
            let options = CookOptions { cook, cache: cache.as_deref(), locked };
            let report = cook::cook_dir(&source, &out, &options)?;
            print_report(&report, cook, &source, &out);
        }
        Command::Help => println!("{USAGE}"),
        Command::Version => println!("sapphire_pipeline {}", env!("CARGO_PKG_VERSION")),
    }
    Ok(())
}

/// Lists what changed, then a summary line. Assets are named relative to the source and
/// output roots.
fn print_report(report: &CookReport, cook: Cook, source: &Path, out: &Path) {
    let relative = |path: &Path, root: &Path| path.strip_prefix(root).unwrap_or(path).display().to_string();
    for sidecar in &report.created_sidecars {
        eprintln!("created {}: commit it with its source", sidecar.display());
    }
    for sidecar in &report.orphaned_sidecars {
        eprintln!(
            "warning: {} has no source; delete it, or move it back next to its source",
            sidecar.display()
        );
    }
    for path in &report.removed {
        eprintln!("removed stale {}", path.display());
    }
    for asset in &report.assets {
        let how = match asset.status {
            CookStatus::Cooked => "cooked",
            CookStatus::Cached => "from cache",
            CookStatus::UpToDate => continue,
        };
        let (from, to) = (relative(&asset.source, source), relative(&asset.output, out));
        eprintln!("{how} {from} -> {to} ({})", asset.id);
    }
    eprintln!(
        "{cook}: {} asset(s) in {}: {} cooked, {} from cache, {} up to date",
        report.assets.len(),
        out.display(),
        report.count(CookStatus::Cooked),
        report.count(CookStatus::Cached),
        report.count(CookStatus::UpToDate),
    );
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
            let outline = match options.value("--weld-tolerance")? {
                Some(meters) => Some(OutlineSettings::with_weld_tolerance(meters).ok_or_else(|| {
                    anyhow!("invalid --weld-tolerance: must be a positive, finite number of meters")
                })?),
                None => None,
            };
            let [input, output] = options.positional()?;
            Command::CookMesh { input: input.into(), output: output.into(), cook, outline }
        }
        "cook" => Command::Cook {
            source: options.required("--source")?,
            out: options.required("--out")?,
            cook: options.required("--cook")?,
            cache: options.value("--cache")?,
            locked: options.flag("--locked"),
        },
        other => bail!("unknown command `{other}`"),
    };
    options.finish()?;
    Ok(command)
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

    /// Takes a boolean `--name`.
    fn flag(&mut self, name: &str) -> bool {
        let found = self.0.iter().position(|arg| arg == name);
        found.map(|i| self.0.remove(i)).is_some()
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
            parse(&["cook-mesh", "a.glb", "--cook=mobile", "b.smesh", "--weld-tolerance", "0.001"]).unwrap(),
            Command::CookMesh {
                input: "a.glb".into(),
                output: "b.smesh".into(),
                cook: Cook::Mobile,
                outline: OutlineSettings::with_weld_tolerance(0.001),
            }
        );
        assert_eq!(
            parse(&["cook", "--source", "content", "--out", "out", "--cook", "desktop", "--locked"]).unwrap(),
            Command::Cook {
                source: "content".into(),
                out: "out".into(),
                cook: Cook::Desktop,
                cache: None,
                locked: true,
            }
        );
        let Command::Cook { cache, locked, .. } =
            parse(&["cook", "--source=a", "--out=b", "--cook=mobile", "--cache", "target/cache"]).unwrap()
        else {
            panic!("expected cook");
        };
        assert_eq!((cache, locked), (Some("target/cache".into()), false));
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
        assert!(parse(&["cook", "--source", "a", "--out", "b", "--cook", "mobile", "--cache"]).is_err());
        assert!(
            parse(&["cook", "--source", "a", "--out", "b", "--cook", "mobile", "--weld-tolerance", "1"])
                .is_err(),
            "per-asset settings live in sidecars"
        );
        assert!(parse(&["cook-mesh", "a.glb"]).is_err(), "one path");
        assert!(parse(&["cook-mesh", "a.glb", "b", "--weld-tolerance", "-1"]).is_err());
        assert!(parse(&["gen-samples", "--out", "dir", "--extra"]).is_err());
    }
}
