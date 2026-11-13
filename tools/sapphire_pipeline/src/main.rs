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
use sapphire_pipeline::code_names;
use sapphire_pipeline::combat;
use sapphire_pipeline::cook::{self, Cook, CookOptions, CookReport, CookStatus};
use sapphire_pipeline::documents;
use sapphire_pipeline::extract::{self, ExtractOptions};
use sapphire_pipeline::figure;
use sapphire_pipeline::generators::{GENERATORS, generate_missing};
use sapphire_pipeline::icon;
use sapphire_pipeline::loc::{Stale, Untranslated};
use sapphire_pipeline::lookdev;
use sapphire_pipeline::meta::{self, ImportSettings, MeshSettings};
use sapphire_pipeline::pack::{self, PackReport, PackStatus};
use sapphire_pipeline::release::{self, ReleaseOptions};
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
  sapphire_pipeline gen-combat --out <DIR> [--force]
      Write the combat slice's generated sources into DIR, created if missing: the enemy's
      model, from the pack's that DIR's ../third_party holds, and its palette and masks. Missing
      files only, as gen-samples, unless --force.
  sapphire_pipeline gen-figures --specs <DIR> --out <DIR> [--force]
      Write every figure spec's model and maps (each <name>.figure.ron in --specs, such as
      tools/figures) under the content directory --out, in the folder each spec names. Missing
      files only, as gen-samples, unless --force.
  sapphire_pipeline icons --source <PNG> --out <DIR>
      Make every platform's icon set from a square, opaque master of at least 1024 px under DIR:
      Apple's asset catalogs (ios/, macos/), Android's adaptive icon and Play's listing icon
      (android/), a Windows .ico (windows/) and a Linux PNG (linux/).
  sapphire_pipeline generate --source <DIR> --lock <FILE> [--locked]
      Run each generator with a declared file missing under the content directory DIR, holding
      FILE's lock while it does. With --locked, a missing file is an error naming it, and nothing
      is written.
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
                         [--cache <DIR>] [--locked] [--editor]
      Cook every source asset under --source (skipping hidden entries, and sources whose
      sidecars say `cook: false`) into mirrored paths under --out, creating missing .meta
      sidecars next to the sources, then pack the cooked
      assets into --packs (shared.spack, audio.spack, fonts.spack, NOTICES.txt). Outputs are written only when they change, and
      stale cooked files under --out, and files no pack claims under --packs, are removed.
      The game's documents (game.documents.ron under --source, if there's one) go beside the packs, under
      documents/, and an unsigned release.smanifest names everything there.
  sapphire_pipeline release --packs <DIR> --out <DIR> --release-version <N>
                            --expires <YYYY-MM-DD> --min-build <N> [--next-keys <HEX>[,<HEX>...]]
                            [--patch-from <MANIFEST>]
      Sign the release of the cook in --packs, which its release.smanifest must still describe,
      with the keys in SAPPHIRE_RELEASE_KEY (each a 32-byte ed25519 seed in hex, comma-separated),
      and lay it out in --out for a static host: packs/ and files/ named by hash,
      manifests/<N>.smanifest, and release.smanifest, written last. --release-version must be
      higher than --out's current release's. The release names the keys that may sign the next
      one: the signing keys, unless --next-keys says. With --patch-from, it's a patch of that
      earlier release: refused, naming the file, if it changes anything the simulation reads, and
      unless an install of the earlier one would take it (same cook, newer, a key it names). It
      prints what an install of the earlier release fetches, by kind, and warns when that's over
      5% of the cook or the two were cooked on different architectures (manifests/<N>.arch).
  sapphire_pipeline release-key new <FILE>
      Write a new release key to FILE, which mustn't be there: an ed25519 seed from the OS's
      generator, in hex, readable only by its owner, as SAPPHIRE_RELEASE_KEY takes it
      (SAPPHIRE_RELEASE_KEY=$(cat FILE)). Prints its public half as platform/patch.toml's
      release-keys takes it. Keep FILE offline, and never in the repository.
  sapphire_pipeline release-key public <FILE>
      Print the public half of the release key in FILE, as platform/patch.toml's release-keys
      takes it; --next-keys takes the quoted digits. The seed is never printed.

Options:
  --all                      extract-materials: every material, not only textured ones.
  --force                    gen-samples, gen-lookdev, gen-combat, gen-figures: also replace generated files that
                             differ from what the generator makes, after changing it.
  --cook <COOK>              Target cook: desktop or mobile.
  --packs <DIR>              Where packs go: the content apps ship.
  --code <DIR>               With --packs: fail if an asset the Rust code under DIR names would
                             leave the shared packs for one zone's.
  --cache <DIR>              Content cache: reuse the cooked output and pack encoding of
                             any asset whose inputs are unchanged, and store new ones.
  --locked                   Fail on a source without a sidecar instead of creating one (CI).
  --editor                   Also cook what only an editor reads (each zone's shapes): never for packs
                             that ship.
  --weld-tolerance <METERS>  Vertices closer than this share one outline normal.
  --release-version <N>      release: the release's version.
  --expires <YYYY-MM-DD>     release: the day, at midnight UTC, it stops being offered as current.
  --min-build <N>            release: the oldest app build it runs on.
  --next-keys <HEX,...>      release: the public keys that may sign the next release.
  --patch-from <MANIFEST>    release: the earlier release's signed manifest, which this one patches.
  -h, --help                 Show this message.
  -V, --version              Show the version.";

#[derive(Debug, PartialEq)]
enum Command {
    Generate {
        source: PathBuf,
        lock: PathBuf,
        locked: bool,
    },
    GenSamples {
        out: PathBuf,
        force: bool,
    },
    GenLookdev {
        out: PathBuf,
        force: bool,
    },
    GenCombat {
        out: PathBuf,
        force: bool,
    },
    GenFigures {
        specs: PathBuf,
        out: PathBuf,
        force: bool,
    },
    Icons {
        source: PathBuf,
        out: PathBuf,
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
        code: Option<PathBuf>,
        cook: Cook,
        cache: Option<PathBuf>,
        locked: bool,
        editor: bool,
    },
    /// `release-key new` (`new: true`) or `release-key public`, on a key file.
    ReleaseKey {
        file: PathBuf,
        new: bool,
    },
    Release {
        packs: PathBuf,
        out: PathBuf,
        options: ReleaseOptions,
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
        Command::Generate { source, lock, locked } => {
            for (command, generated) in generate_missing(&lock, &source, &GENERATORS, locked)? {
                print_generated(&generated, command);
            }
        }
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
        Command::GenCombat { out, force } => {
            let generated = combat::write_all(&out, existing(force))
                .with_context(|| format!("failed to write the combat sources to {}", out.display()))?;
            print_generated(&generated, "gen-combat");
        }
        Command::GenFigures { specs, out, force } => {
            let generated = figure::write_all(&specs, &out, existing(force)).with_context(|| {
                format!("failed to write the figures from {} to {}", specs.display(), out.display())
            })?;
            print_generated(&generated, "gen-figures");
        }
        Command::Icons { source, out } => {
            let written = icon::write_sets(&source, &out)?;
            eprintln!("made {written} icon files from {} under {}", source.display(), out.display());
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
        Command::Cook { source, out, packs, code, cook, cache, locked, editor } => {
            let options = CookOptions { cook, cache: cache.as_deref(), locked, editor };
            let report = cook::cook_dir(&source, &out, &options)?;
            print_report(&report, cook, &source, &out);
            if let Some(packs) = packs {
                if let Some(code) = code {
                    let names = code_names::scan(&code)
                        .with_context(|| format!("reading the code in {}", code.display()))?;
                    pack::check_code_names(&report, cache.as_deref(), &names)?;
                }
                let packed = pack::pack_cooked(&report, &packs, cache.as_deref())?;
                print_packs(&packed, cook);
                let shipped = documents::ship_documents(&source, &packs)?;
                for path in &shipped.written {
                    println!("  shipped {}", path.display());
                }
                for path in &shipped.removed {
                    println!("  removed {}", path.display());
                }
                if let Some(path) = release::write_manifest(&packs, cook)? {
                    println!("  wrote {}", path.display());
                }
            }
        }
        Command::Release { packs, out, options } => {
            let keys = release::keys_from_env(std::env::var(release::KEY_VAR).ok().as_deref())?;
            let published = release::publish(&packs, &out, &options, &keys)?;
            eprintln!(
                "released {} ({} cook, {} packs, {} files) into {}: {} written, {} already there",
                published.manifest.version,
                published.manifest.cook,
                published.manifest.packs.len(),
                published.manifest.files.len(),
                out.display(),
                published.written,
                published.kept
            );
            if let Some(patch) = &published.patch {
                eprint!("{patch}");
                for warning in patch.warnings() {
                    eprintln!("warning: {warning}");
                }
            }
        }
        Command::ReleaseKey { file, new } => {
            let public =
                if new { release::new_key_file(&file)? } else { release::public_key_of_file(&file)? };
            if new {
                eprintln!(
                    "wrote a new release key to {}: keep it offline and out of the repository; \
                     SAPPHIRE_RELEASE_KEY=$(cat <it>) signs with it",
                    file.display()
                );
            }
            println!("release-keys = [\"{}\"]", release::public_key_hex(&public));
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
    for Untranslated { locale, key, served_by } in &report.localization.untranslated {
        eprintln!("untranslated in {locale}: {key} (shows {served_by}'s)");
    }
    for Stale { locale, key } in &report.localization.stale {
        eprintln!("warning: {locale} holds {key}, which nothing down its chain does");
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
        "generate" => Command::Generate {
            source: options.required("--source")?,
            lock: options.required("--lock")?,
            locked: options.flag("--locked"),
        },
        "gen-samples" => {
            Command::GenSamples { out: options.required("--out")?, force: options.flag("--force") }
        }
        "gen-lookdev" => {
            Command::GenLookdev { out: options.required("--out")?, force: options.flag("--force") }
        }
        "gen-combat" => {
            Command::GenCombat { out: options.required("--out")?, force: options.flag("--force") }
        }
        "gen-figures" => Command::GenFigures {
            specs: options.required("--specs")?,
            out: options.required("--out")?,
            force: options.flag("--force"),
        },
        "icons" => Command::Icons { source: options.required("--source")?, out: options.required("--out")? },
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
            code: options.value("--code")?,
            cook: options.required("--cook")?,
            cache: options.value("--cache")?,
            locked: options.flag("--locked"),
            editor: options.flag("--editor"),
        },
        "release" => Command::Release {
            packs: options.required("--packs")?,
            out: options.required("--out")?,
            options: ReleaseOptions {
                version: options.required("--release-version")?,
                expires: release::date(&options.required::<String>("--expires")?)?,
                min_app_build: options.required("--min-build")?,
                successor_keys: match options.value::<String>("--next-keys")? {
                    Some(keys) => Some(release::public_keys(&keys)?),
                    None => None,
                },
                patch_from: options.value("--patch-from")?,
            },
        },
        "release-key" => {
            let [action, file] = options.positional()?;
            let new = match action.as_str() {
                "new" => true,
                "public" => false,
                other => bail!("release-key takes `new` or `public`, not `{other}`"),
            };
            Command::ReleaseKey { file: file.into(), new }
        }
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
            parse(&["gen-combat", "--out", "dir"]).unwrap(),
            Command::GenCombat { out: "dir".into(), force: false }
        );
        assert_eq!(
            parse(&["icons", "--source", "icon.png", "--out", "dir"]).unwrap(),
            Command::Icons { source: "icon.png".into(), out: "dir".into() }
        );
        assert_eq!(
            parse(&["gen-figures", "--specs", "figures", "--out", "content", "--force"]).unwrap(),
            Command::GenFigures { specs: "figures".into(), out: "content".into(), force: true }
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
                code: None,
                cook: Cook::Desktop,
                cache: None,
                locked: true,
                editor: false,
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
        let release = ["release", "--packs", "p", "--out", "o", "--release-version", "3", "--min-build", "9"];
        assert!(parse(&release).is_err(), "no expiry");
        assert!(parse(&[&release[..], &["--expires", "2027-02-30"]].concat()).is_err(), "no such day");
        assert!(parse(&[&release[..], &["--expires", "2027-03-01", "--next-keys", "00"]].concat()).is_err());
    }

    #[test]
    fn parses_a_release() {
        let parsed = parse(&[
            "release",
            "--packs",
            "p",
            "--out",
            "o",
            "--release-version",
            "3",
            "--expires",
            "1970-01-02",
            "--min-build",
            "9",
        ])
        .unwrap();
        let options = ReleaseOptions {
            version: 3,
            expires: 86_400,
            min_app_build: 9,
            successor_keys: None,
            patch_from: None,
        };
        assert_eq!(parsed, Command::Release { packs: "p".into(), out: "o".into(), options: options.clone() });
        let patch = parse(&[
            "release",
            "--packs",
            "p",
            "--out",
            "o",
            "--release-version",
            "3",
            "--expires",
            "1970-01-02",
            "--min-build",
            "9",
            "--patch-from",
            "host/manifests/2.smanifest",
        ])
        .unwrap();
        let options = ReleaseOptions { patch_from: Some("host/manifests/2.smanifest".into()), ..options };
        assert_eq!(patch, Command::Release { packs: "p".into(), out: "o".into(), options });
    }

    #[test]
    fn a_release_key_is_made_or_read_by_its_file() {
        assert_eq!(
            parse(&["release-key", "new", "keys/release.key"]).unwrap(),
            Command::ReleaseKey { file: "keys/release.key".into(), new: true }
        );
        assert_eq!(
            parse(&["release-key", "public", "keys/release.key"]).unwrap(),
            Command::ReleaseKey { file: "keys/release.key".into(), new: false }
        );
        assert!(parse(&["release-key", "print", "keys/release.key"]).is_err());
        assert!(parse(&["release-key", "new"]).is_err(), "a file is named");
    }
}
