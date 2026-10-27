//! `sapphire_pipeline`: command-line front end to the asset pipeline.
//!
//! The arguments are parsed by hand, as in xtask: six commands and a handful of options don't
//! justify clap's build time, which xtask's cook pays in a release build too.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use anyhow::{Context as _, anyhow, bail};
use sapphire_pipeline::bake::OutlineSettings;
use sapphire_pipeline::character_maps::{self, MapKind};
use sapphire_pipeline::cook::{self, Cook, CookOptions, CookReport, CookStatus};
use sapphire_pipeline::extract::{self, ExtractOptions};
use sapphire_pipeline::lookdev;
use sapphire_pipeline::meta::{self, ImportSettings, MeshSettings};
use sapphire_pipeline::pack::{self, PackReport, PackStatus};
use sapphire_pipeline::samples::{self, Existing, Generated};

const USAGE: &str = "\
Sapphire asset pipeline: generate, import, bake and cook source assets.

Usage:
  sapphire_pipeline gen-samples --out <DIR> [--force]
      Write the procedural sample source assets (mannequin.glb, the swatch maps and materials)
      into DIR, created if missing. Only missing files are written: a file that differs from
      what the generator makes (a hand edit) is kept, with a warning, unless --force.
  sapphire_pipeline gen-lookdev --out <DIR> [--force]
      Write the look-dev scene's generated sources into DIR, created if missing: the town's
      painted maps, and the character's hair mask from the model's highlight that DIR's
      ../third_party holds. Missing files only, as gen-samples, unless --force.
  sapphire_pipeline cook-mesh <INPUT> <OUTPUT> [--cook <COOK>] [--weld-tolerance <METERS>]
      Cook one .gltf/.glb/.fbx/.vrm file into a .smesh file (default cook: desktop), with the
      settings of its .meta sidecar if it has one. Creates no sidecar and uses no cache.
  sapphire_pipeline extract-materials <SOURCE> [--all]
      Write the materials of a .vrm/.gltf/.glb/.fbx source out as material documents, with a
      VRM's images as textures and its shading shift textures as ILM maps, and an FBX's base
      color and normal textures bound, into a folder named after the source, and bind them in
      the source's sidecar. By default only the materials that
      bind textures; --all extracts every material. Never overwrites a file, and keeps the
      materials the sidecar already binds.
  sapphire_pipeline bake-map <DOCUMENT> <OUTPUT>
      Generate the map a .facesdf.ron, .ramp.ron or .ilm.ron document describes, with the
      settings of its .meta sidecar if it has one, and write it to OUTPUT as a PNG before
      compression: 16-bit linear values for face SDF and ILM maps, 8-bit sRGB for ramps.
  sapphire_pipeline cook --source <DIR> --out <DIR> --cook <COOK> [--packs <DIR>]
                         [--cache <DIR>] [--locked]
      Cook every source asset under --source (skipping hidden entries, and sources whose
      sidecars say `cook: false`) into mirrored paths under --out, creating missing .meta
      sidecars next to the sources, then pack the cooked
      assets into --packs (shared.spack). Outputs are written only when they change, and
      stale cooked files under --out, and files no pack claims under --packs, are removed.

Options:
  --all                      extract-materials: every material, not only textured ones.
  --force                    gen-samples, gen-lookdev: also replace generated files that
                             differ from what the generator makes, after changing it.
  --cook <COOK>              Target cook: desktop or mobile.
  --packs <DIR>              Where packs go: the content apps ship.
  --cache <DIR>              Content cache: reuse the cooked output and pack encoding of
                             any asset whose inputs are unchanged, and store new ones.
  --locked                   Fail on a source without a sidecar instead of creating one (CI).
  --weld-tolerance <METERS>  Vertices closer than this share one outline normal.
  -h, --help                 Show this message.
  -V, --version              Show the version.";

#[derive(Debug, PartialEq)]
enum Command {
    GenSamples {
        out: PathBuf,
        force: bool,
    },
    GenLookdev {
        out: PathBuf,
        force: bool,
    },
    CookMesh {
        input: PathBuf,
        output: PathBuf,
        cook: Cook,
        outline: Option<OutlineSettings>,
    },
    BakeMap {
        input: PathBuf,
        output: PathBuf,
    },
    ExtractMaterials {
        source: PathBuf,
        all: bool,
    },
    Cook {
        source: PathBuf,
        out: PathBuf,
        packs: Option<PathBuf>,
        cook: Cook,
        cache: Option<PathBuf>,
        locked: bool,
    },
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
        Command::GenSamples { out, force } => {
            let generated = samples::write_all(&out, existing(force))
                .with_context(|| format!("failed to write samples to {}", out.display()))?;
            print_generated(&generated, "gen-samples");
        }
        Command::GenLookdev { out, force } => {
            let generated = lookdev::write_all(&out, existing(force))
                .with_context(|| format!("failed to write the look-dev sources to {}", out.display()))?;
            print_generated(&generated, "gen-lookdev");
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
        Command::BakeMap { input, output } => {
            bake_map(&input, &output)?;
            eprintln!("baked {} -> {}", input.display(), output.display());
        }
        Command::ExtractMaterials { source, all } => {
            let extraction = extract::extract_materials(&source, &ExtractOptions { all })?;
            for path in &extraction.written {
                eprintln!("wrote {}", path.display());
            }
            for slot in &extraction.kept {
                eprintln!("kept {slot:?}: its sidecar binds it already");
            }
            for (slot, id) in &extraction.bound {
                eprintln!("bound {slot:?} to {id}");
            }
            for conflict in &extraction.conflicts {
                eprintln!(
                    "warning: {:?} uses {} as a {} texture, but it keeps its first role, {}: that map \
                     is left unbound; give the use an image of its own to bind it",
                    conflict.slot,
                    conflict.image.display(),
                    conflict.used_as,
                    conflict.keeps
                );
            }
            eprintln!(
                "{}: {} material(s) bound, {} file(s) written; commit them with the source",
                source.display(),
                extraction.bound.len(),
                extraction.written.len()
            );
        }
        Command::Cook { source, out, packs, cook, cache, locked } => {
            let options = CookOptions { cook, cache: cache.as_deref(), locked };
            let report = cook::cook_dir(&source, &out, &options)?;
            print_report(&report, cook, &source, &out);
            if let Some(packs) = packs {
                let packed = pack::pack_cooked(&report, &packs, cache.as_deref())?;
                print_packs(&packed, cook);
            }
        }
        Command::Help => println!("{USAGE}"),
        Command::Version => println!("sapphire_pipeline {}", env!("CARGO_PKG_VERSION")),
    }
    Ok(())
}

/// What a generator does with files that differ from its output: replaces them with `--force`,
/// keeps them otherwise.
fn existing(force: bool) -> Existing {
    if force { Existing::Replace } else { Existing::Keep }
}

/// One line per file written, a warning per file kept, then a summary line.
fn print_generated(generated: &Generated, command: &str) {
    for path in &generated.created {
        eprintln!("generated {}", path.display());
    }
    for path in &generated.replaced {
        eprintln!("replaced {}", path.display());
    }
    for path in &generated.kept {
        eprintln!(
            "warning: kept {}: it differs from what {command} makes (--force replaces it)",
            path.display()
        );
    }
    eprintln!(
        "{command}: {} generated, {} replaced, {} kept, {} up to date",
        generated.created.len(),
        generated.replaced.len(),
        generated.kept.len(),
        generated.unchanged.len()
    );
}

/// Generates the map the document at `input` describes and writes it to `output` as a PNG.
fn bake_map(input: &Path, output: &Path) -> anyhow::Result<()> {
    let sidecar = meta::sidecar_path(input);
    let settings = if sidecar.exists() {
        match meta::read(&sidecar)?.importer {
            ImportSettings::CharacterMap(settings) => settings,
            _ => bail!("{} isn't a character map's sidecar", sidecar.display()),
        }
    } else {
        MapKind::for_path(input)
            .with_context(|| {
                format!("{} isn't a .facesdf.ron, .ramp.ron or .ilm.ron document", input.display())
            })?
            .default_settings()
    };
    let bytes = std::fs::read(input).with_context(|| format!("failed to read {}", input.display()))?;
    let map = character_maps::generate(input, &bytes, &settings)
        .with_context(|| format!("failed to generate the map {} describes", input.display()))?;
    let png = character_maps::to_png(&map).context("failed to encode the PNG")?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(output, png).with_context(|| format!("failed to write {}", output.display()))
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
    for path in &report.uncooked {
        eprintln!("not cooked: {} (its sidecar says `cook: false`)", relative(path, source));
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

/// One line per pack, after any stale files removed.
fn print_packs(report: &PackReport, cook: Cook) {
    for path in &report.removed {
        eprintln!("removed stale {}", path.display());
    }
    for pack in &report.packs {
        let how = match pack.status {
            PackStatus::Packed => "packed",
            PackStatus::UpToDate => "up to date:",
        };
        eprintln!(
            "{cook}: {how} {} ({} asset(s), {} bytes, {} encoded, {} from cache)",
            pack.path.display(),
            pack.assets,
            pack.bytes,
            pack.encoded,
            if pack.status == PackStatus::Packed { pack.assets - pack.encoded } else { 0 },
        );
    }
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
        "gen-samples" => {
            Command::GenSamples { out: options.required("--out")?, force: options.flag("--force") }
        }
        "gen-lookdev" => {
            Command::GenLookdev { out: options.required("--out")?, force: options.flag("--force") }
        }
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
        "bake-map" => {
            let [input, output] = options.positional()?;
            Command::BakeMap { input: input.into(), output: output.into() }
        }
        "extract-materials" => {
            let all = options.flag("--all");
            let [source] = options.positional()?;
            Command::ExtractMaterials { source: source.into(), all }
        }
        "cook" => Command::Cook {
            source: options.required("--source")?,
            out: options.required("--out")?,
            packs: options.value("--packs")?,
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
            Command::GenSamples { out: "dir".into(), force: false }
        );
        assert_eq!(
            parse(&["gen-samples", "--force", "--out", "dir"]).unwrap(),
            Command::GenSamples { out: "dir".into(), force: true }
        );
        assert_eq!(
            parse(&["gen-lookdev", "--out", "dir"]).unwrap(),
            Command::GenLookdev { out: "dir".into(), force: false }
        );
        assert_eq!(
            parse(&["gen-lookdev", "--out=dir", "--force"]).unwrap(),
            Command::GenLookdev { out: "dir".into(), force: true }
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
            parse(&["bake-map", "face.facesdf.ron", "face.png"]).unwrap(),
            Command::BakeMap { input: "face.facesdf.ron".into(), output: "face.png".into() }
        );
        assert_eq!(
            parse(&["extract-materials", "alicia.vrm"]).unwrap(),
            Command::ExtractMaterials { source: "alicia.vrm".into(), all: false }
        );
        assert_eq!(
            parse(&["extract-materials", "--all", "alicia.vrm"]).unwrap(),
            Command::ExtractMaterials { source: "alicia.vrm".into(), all: true }
        );
        assert_eq!(
            parse(&["cook", "--source", "content", "--out", "out", "--cook", "desktop", "--locked"]).unwrap(),
            Command::Cook {
                source: "content".into(),
                out: "out".into(),
                packs: None,
                cook: Cook::Desktop,
                cache: None,
                locked: true,
            }
        );
        let Command::Cook { cache, locked, packs, .. } = parse(&[
            "cook",
            "--source=a",
            "--out=b",
            "--cook=mobile",
            "--cache",
            "target/cache",
            "--packs",
            "p",
        ])
        .unwrap() else {
            panic!("expected cook");
        };
        assert_eq!((cache, locked, packs), (Some("target/cache".into()), false, Some("p".into())));
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
        assert!(parse(&["bake-map", "a.ramp.ron"]).is_err(), "one path");
        assert!(parse(&["extract-materials"]).is_err(), "no source");
        assert!(parse(&["extract-materials", "a.vrm", "--cook", "mobile"]).is_err(), "no cook");
        assert!(
            parse(&["bake-map", "a.ramp.ron", "a.png", "--cook", "mobile"]).is_err(),
            "no cook: before compression"
        );
        assert!(parse(&["cook-mesh", "a.glb", "b", "--weld-tolerance", "-1"]).is_err());
        assert!(parse(&["gen-samples", "--out", "dir", "--extra"]).is_err());
        assert!(parse(&["cook", "--source", "a", "--out", "b", "--cook", "mobile", "--force"]).is_err());
    }
}
