//! `sapphire_pipeline`: command-line front end to the asset pipeline.
//!
//! The arguments are parsed by hand, as in xtask: six commands and a handful of options don't
//! justify clap's build time, which xtask's cook pays in a release build too.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use anyhow::{Context as _, anyhow, bail, ensure};
use sapphire_pipeline::audit;
use sapphire_pipeline::bake::OutlineSettings;
use sapphire_pipeline::bench::{self, Preset};
use sapphire_pipeline::character_maps::{self, FaceSdfSettings, MapKind, MapSettings};
use sapphire_pipeline::check;
use sapphire_pipeline::code_names;
use sapphire_pipeline::combat;
use sapphire_pipeline::content_check;
use sapphire_pipeline::cook::{self, Cook, CookError, CookOptions, CookReport, CookStatus};
use sapphire_pipeline::documents;
use sapphire_pipeline::effect_sheet;
use sapphire_pipeline::extract::{self, ExtractOptions};
use sapphire_pipeline::figure;
use sapphire_pipeline::generators::{GENERATORS, generate_missing};
use sapphire_pipeline::icon;
use sapphire_pipeline::index::{self, AssetIndex};
use sapphire_pipeline::loc::{Stale, Untranslated};
use sapphire_pipeline::lookdev;
use sapphire_pipeline::meta::{self, IdSource, ImportSettings, MeshSettings};
use sapphire_pipeline::pack::{self, PackReport, PackStatus};
use sapphire_pipeline::procedural::{self, ProceduralTexture, SurfaceMap};
use sapphire_pipeline::refusal::Refusal;
use sapphire_pipeline::release::{self, ReleaseOptions};
use sapphire_pipeline::samples::{self, Existing, Generated, write_files};
use sapphire_pipeline::scaffold::{self, NewCharacter};
use sapphire_pipeline::timing::{self, Phase, Slowest};
use sapphire_pipeline::vo;
use sapphire_time::Stopwatch;

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
  sapphire_pipeline gen-effects --out <DIR> [--force]
      Write the placeholder effect sheets into DIR's sheets/, created if missing: each sheet's
      painting and its document (M6 N4a). Missing files only, as gen-samples, unless --force.
  sapphire_pipeline gen-figures --specs <DIR> --out <DIR> [--force]
      Write every figure spec's model and maps (each <name>.figure.ron in --specs, such as
      tools/figures) under the content directory --out, in the folder each spec names. Missing
      files only, as gen-samples, unless --force.
  sapphire_pipeline gen-bench --bench <DIR> --preset <PRESET> [--cook <COOK>] [--cell-levels on|off]
                             --out <DIR>
      Write the benchmark's sources that the documents under --bench (bench/) describe, at a preset's
      scale (ci, budget or double), each with its sidecar, into --out: a source tree for a cook under target/,
      never committed. --cook (default desktop) is the cook the tree is for: the phones' (mobile) district
      draws four materials a cell where the desktop's draws sixteen. Files an earlier run left in --out that
      this one doesn't write are removed; --out must be empty or a tree gen-bench wrote for the same cook.
      --cell-levels off cooks the district's cells with one level, not the document's (the tree the
      benches at scale measure M5 D6 against).
  sapphire_pipeline gen-kinds --bench <DIR> --preset <PRESET> --specs <DIR> --content <DIR> --out <DIR>
      Link the content directory --content into --out and write over it the figures --bench's
      kinds.ron makes again at a preset's scale, from their specs in --specs (tools/figures), under
      the same IDs: a source tree for a cook under target/, never committed. --out must be missing,
      empty or a tree gen-kinds wrote.
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
  sapphire_pipeline new-character --model <MODEL> [--out <DIR>] [--role <ROLE>] [--preset <FILE>]
                                  [--part <SLOT>=<PART>]...
      Scaffold a .vrm/.glb/.gltf/.fbx character so it cooks with no hand edit: copy MODEL into --out
      (default: its own folder), start its sidecar from a preset (FILE, an ImportPreset document, or the
      built-in character preset of its kind) cast as ROLE (hero, enemy, boss, ambient, tree or cover;
      default the preset's), extract every material as extract-materials --all does, and give each slot, by its
      part (skin, face, hair, eye or cloth: from --part, else the importer and the slot's name), a ramp,
      an ILM map and a layer mask, and a face its SDF map from masks baked from its normals, each a
      document with its sidecar under <stem>/shading/, bound in its material. Every ID is new. Never
      overwrites a file: a second run writes only what is missing, and a file of another kind where
      one would go is an error naming it.
  sapphire_pipeline bake-map <DOCUMENT> <OUTPUT> [--sheet <DEGREES>[,<DEGREES>...]]
      Generate the map a .facesdf.ron, .ramp.ron, .ilm.ron or .ptex.ron document describes, with
      the settings of its .meta sidecar if it has one, and write it to OUTPUT as a PNG before
      compression: 16-bit linear values for face SDF and ILM maps, 8-bit sRGB for ramps and a
      procedural texture's colour, 8-bit linear for its normal and mask (colour without a sidecar).
      With --sheet, for a .facesdf.ron, write instead a sheet to judge the face map by: a row per
      light angle (0 to 180) and channel, the face lit from the head's right, from its left, and
      the document's mask at that angle, if any, red where the map doesn't reproduce it.
  sapphire_pipeline cook --source <DIR> --out <DIR> --cook <COOK> [--packs <DIR>]
                         [--cache <DIR>] [--locked] [--editor] [--report] [--refusal <FILE>]
      Cook every source asset under --source (skipping hidden entries, and sources whose
      sidecars say `cook: false`) into mirrored paths under --out, creating missing .meta
      sidecars next to the sources, then pack the cooked
      assets into --packs (shared.spack, audio.spack, vo_<locale>.spack, fonts.spack, NOTICES.txt). Outputs are written only when they change, and
      stale cooked files under --out, and files no pack claims under --packs, are removed.
      The game's documents (game.documents.ron under --source, if there's one) go beside the packs, under
      documents/, and an unsigned release.smanifest names everything there. Beside the cooked files under
      --out go the cook audit (cook_audit.ron) and the asset index (index.json), which never ship.
      With --report, it then prints how long each phase took and the slowest stages and assets, which no
      file it writes holds. With --refusal, a cook that fails also writes why to FILE as RON (the source,
      the field of its document where the refusal knows one, and the message), and one that succeeds
      takes an old FILE away: what the editor reads of a cook it ran.
  sapphire_pipeline graph <ASSET> --index <FILE> [--reverse]
      Print what ASSET uses, all the way down, or with --reverse what uses it, from the asset index a cook
      wrote (FILE: index.json beside its cooked files, target/content/loose/<cook>/index.json for cargo
      xtask content). ASSET is an asset ID, or a source's path under the cooked tree for the asset its
      sidecar names.
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
  sapphire_pipeline manifest --packs <DIR> --cook <COOK>
      Write the unsigned release.smanifest of what DIR holds, as cook --packs does last: for a
      folder put together from more than one cook, such as the shipped packs with the benchmark
      district's pack beside them, which a shipping build then mounts (cargo xtask counts).
  sapphire_pipeline check <DIR> [--json]
      Check the source tree DIR as a locked cook reads it, writing nothing: each mesh whose sidecar names
      a role (`budget: (role: Hero)`) against that role's row of spec §3.4 on both cooks (triangles
      at LOD0, joints, material slots after merges, map sizes), and every source against the content
      rules (maps over 1K with no mobile cap, unbound slots, VRoid-style names the importer didn't
      recognize, duplicate spring-chain names, non-ASCII paths, duplicate texture bytes). Prints each
      role mesh's numbers and every finding, waived ones with their reasons, as text or, with --json, as
      JSON; fails on an error no sidecar waives (`waive: [(rule: Slots, why: \"...\")]`).
  sapphire_pipeline content-check --source <DIR> --out <DIR> [--cook <COOK>] [--cache <DIR>] [--locked]
                                  [<FILE>...]
      Print everything wrong with the source tree DIR, by document and field, as the editor's problems pane
      lists it: each document of a kind the editor opens read as the editor reads it, the content rules
      as check applies them, and a cook (desktop unless --cook says) into the scratch folder --out, which
      must be no folder an app reads. Fails on any problem; refuses a FILE that isn't a file under DIR
      (the hook's staged files): what cargo xtask content-check and the opt-in pre-commit hook run.
  sapphire_pipeline release-key new <FILE>
      Write a new release key to FILE, which mustn't be there: an ed25519 seed from the OS's
      generator, in hex, readable only by its owner, as SAPPHIRE_RELEASE_KEY takes it
      (SAPPHIRE_RELEASE_KEY=$(cat FILE)). Prints its public half as platform/patch.toml's
      release-keys takes it. Keep FILE offline, and never in the repository.
  sapphire_pipeline vo --content <DIR> [--check]
      Import the voice over under DIR/vo (M6 U11): encode each language's WAV masters that changed to
      Ogg Vorbis, with their sidecars and a cue each for a new line, and write each language's voice
      lines, its recording script (script.csv) and its import report (import.csv), all to commit. A
      master the script (vo/script.ron) doesn't list is an error; a line with no master fails the run
      once the rest is written, naming it. With --check, nothing is written, and any file an import
      would change is an error.
  sapphire_pipeline release-key public <FILE>
      Print the public half of the release key in FILE, as platform/patch.toml's release-keys
      takes it; --next-keys takes the quoted digits. The seed is never printed.

Options:
  --all                      extract-materials: every material, not only textured ones.
  --model <MODEL>            new-character: the character's model.
  --role <ROLE>              new-character: the role its new sidecar casts it in.
  --preset <FILE>            new-character: the ImportPreset its new sidecar starts from.
  --part <SLOT>=<PART>       new-character: the part a slot is, in place of the one its name says.
  --check                    vo: write nothing, and fail on any file an import would change.
  --json                     check: the report as JSON on stdout.
  --index <FILE>             graph: the asset index to read.
  --reverse                  graph: what uses the asset, rather than what it uses.
  --force                    gen-samples, gen-lookdev, gen-combat, gen-effects, gen-figures: also replace generated files that
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
  --report                   cook: print each phase's time and the slowest stages and assets.
  --refusal <FILE>           cook: write why a cook failed to FILE, and take it away when one succeeds.
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
    GenEffects {
        out: PathBuf,
        force: bool,
    },
    GenFigures {
        specs: PathBuf,
        out: PathBuf,
        force: bool,
    },
    GenBench {
        bench: PathBuf,
        preset: Preset,
        cook: Cook,
        cells: bench::CellLevels,
        out: PathBuf,
    },
    GenKinds {
        bench: PathBuf,
        preset: Preset,
        specs: PathBuf,
        content: PathBuf,
        out: PathBuf,
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
        /// `--sheet`'s light angles, in degrees: a face map's sheet instead of the map.
        sheet: Option<Vec<f32>>,
    },
    ExtractMaterials {
        source: PathBuf,
        all: bool,
    },
    /// `new-character`: a model's sidecar, and its slots' materials and maps.
    NewCharacter(NewCharacter),
    Cook {
        source: PathBuf,
        out: PathBuf,
        packs: Option<PathBuf>,
        code: Option<PathBuf>,
        cook: Cook,
        cache: Option<PathBuf>,
        locked: bool,
        editor: bool,
        /// Print the cook's times.
        report: bool,
        /// Where to write why the cook failed, if it does.
        refusal: Option<PathBuf>,
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
    /// `manifest`: the unsigned manifest of a folder of packs.
    Manifest {
        packs: PathBuf,
        cook: Cook,
    },
    /// `check`: a source tree against its roles and the content rules.
    Check {
        source: PathBuf,
        json: bool,
    },
    /// `content-check`: a tree's problems by document and field, through a cook into a scratch folder.
    ContentCheck {
        source: PathBuf,
        out: PathBuf,
        cook: Cook,
        cache: Option<PathBuf>,
        locked: bool,
        /// The documents to fail on, or none for every one.
        touched: Vec<PathBuf>,
    },
    /// `graph`: what an asset uses, or what uses it, from a cook's asset index.
    Graph {
        index: PathBuf,
        asset: String,
        reverse: bool,
    },
    /// `vo`: the voice over's import, or with `check`, whether it's current.
    Vo {
        content: PathBuf,
        check: bool,
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
        Command::GenEffects { out, force } => {
            let generated = write_files(&out, effect_sheet::paint::outputs(&out), existing(force))
                .with_context(|| format!("failed to write the effect sheets to {}", out.display()))?;
            print_generated(&generated, "gen-effects");
        }
        Command::GenFigures { specs, out, force } => {
            let generated = figure::write_all(&specs, &out, existing(force)).with_context(|| {
                format!("failed to write the figures from {} to {}", specs.display(), out.display())
            })?;
            print_generated(&generated, "gen-figures");
        }
        Command::GenBench { bench, preset, cook, cells, out } => {
            let generated = bench::generate_with(&bench, preset, cook, cells, &out).with_context(|| {
                format!(
                    "failed to generate the {preset} benchmark for {cook} from {} into {}",
                    bench.display(),
                    out.display()
                )
            })?;
            eprintln!(
                "gen-bench: {preset} for {cook} into {}: {} written, {} unchanged",
                out.display(),
                generated.created.len() + generated.replaced.len(),
                generated.unchanged.len()
            );
        }
        Command::GenKinds { bench, preset, specs, content, out } => {
            let done = bench::kinds::generate(&bench, &specs, preset, &content, &out).with_context(|| {
                format!("failed to make the {preset} kinds from {} into {}", content.display(), out.display())
            })?;
            eprintln!(
                "gen-kinds: {preset} into {}: {} made again ({}), {} files written over",
                out.display(),
                done.figures.len(),
                done.figures.join(", "),
                done.replaced.len()
            );
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
        Command::BakeMap { input, output, sheet } => {
            match sheet {
                Some(angles) => bake_sheet(&input, &output, &angles)?,
                None => bake_map(&input, &output)?,
            }
            eprintln!("baked {} -> {}", input.display(), output.display());
        }
        Command::NewCharacter(request) => {
            let scaffold = scaffold::new_character(&request, &mut IdSource::Random)?;
            for path in &scaffold.written {
                eprintln!("wrote {}", path.display());
            }
            for slot in &scaffold.kept {
                eprintln!("kept {slot:?}: its sidecar binds it already");
            }
            for (slot, part) in &scaffold.parts {
                eprintln!("{slot:?}: {part}");
            }
            for slot in &scaffold.unbound_ilm {
                eprintln!(
                    "note: {slot:?}'s vertices pick its regions, so its ILM document is written but not bound: \
                     paint its material IDs, then bind it as the material's control_map"
                );
            }
            eprintln!(
                "{}: {} slot(s) scaffolded, {} file(s) written; check it with `sapphire_pipeline check`, then \
                 commit them with the model",
                scaffold.model.display(),
                scaffold.parts.len(),
                scaffold.written.len()
            );
        }
        Command::ExtractMaterials { source, all } => {
            let extraction =
                extract::extract_materials(&source, &ExtractOptions { all, ..ExtractOptions::default() })?;
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
        Command::Cook { source, out, packs, code, cook, cache, locked, editor, report: show, refusal } => {
            let options = CookOptions { cook, cache: cache.as_deref(), locked, editor };
            let cooked = cook_all(&source, &out, packs.as_deref(), code.as_deref(), &options, show);
            if let Some(file) = refusal {
                let written = match &cooked {
                    Ok(()) => Refusal::clear(&file),
                    Err(error) => match error.downcast_ref::<CookError>() {
                        Some(error) => Refusal::of_cook(error).write(&file),
                        None => Refusal::of_other(error.as_ref()).write(&file),
                    },
                };
                let written = written.with_context(|| format!("writing {}", file.display()));
                // A refusal that can't be written never hides the cook's own error: that's what's reported, with
                // the refusal's failure after it.
                if let (Err(cook), Err(write)) = (&cooked, &written) {
                    anyhow::bail!("{cook:#}\n(and the refusal wasn't written: {write:#})");
                }
                written?;
            }
            cooked?;
        }
        Command::Check { source, json } => {
            let report = check::check(&source)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report).context("writing the report as JSON")?);
            } else {
                println!("{report}");
            }
            if !report.passes() {
                bail!(
                    "{} failed the check: {} error(s) no sidecar waives",
                    source.display(),
                    report.failures().count()
                );
            }
        }
        Command::ContentCheck { source, out, cook, cache, locked, touched } => {
            let options =
                content_check::Options { source: &source, out: &out, cook, cache: cache.as_deref(), locked };
            let checked = content_check::content_check(&options, &touched)?;
            println!("{}", content_check::report(&checked));
            if !checked.passes() {
                eprintln!("{} failed content-check: {} problem(s)", source.display(), checked.problems.len());
                // Its own exit code, so the hook tells problems from a check that couldn't run.
                std::process::exit(content_check::FOUND_PROBLEMS);
            }
        }
        Command::Graph { index, asset, reverse } => {
            let read = AssetIndex::read(&index)?;
            let id = read.find(&asset).with_context(|| {
                format!(
                    "{asset} is neither an asset ID nor a source of the {} cook {}",
                    read.cook,
                    index.display()
                )
            })?;
            println!("{}", read.show(&read.graph(id, reverse), reverse));
        }
        Command::Manifest { packs, cook } => match release::write_manifest(&packs, cook)? {
            Some(path) => println!("wrote {}", path.display()),
            None => println!("{}'s manifest already names what's there", packs.display()),
        },
        Command::Vo { content, check } => {
            let mode = if check { vo::Mode::Check } else { vo::Mode::Write };
            let imported = vo::import(&content, mode)?;
            let (encoded, stale) = if check { ("to encode", "stale") } else { ("encoded", "wrote") };
            for (language, key, seconds) in &imported.recorded {
                eprintln!("  {language} `{key}`: {seconds:.3} s");
            }
            for path in &imported.encoded {
                eprintln!("{encoded} {}", path.display());
            }
            for path in &imported.stale {
                eprintln!("{stale} {}", path.display());
            }
            if !imported.missing.is_empty() {
                let lines: Vec<String> =
                    imported.missing.iter().map(|(language, key)| format!("{language} `{key}`")).collect();
                bail!("{} lines have no master: {}", lines.len(), lines.join(", "));
            }
            if check && !imported.stale.is_empty() {
                bail!(
                    "the voice over is out of date: run `sapphire_pipeline vo --content {}`",
                    content.display()
                );
            }
            eprintln!(
                "voice over: {} takes, {} files {stale}",
                imported.recorded.len(),
                imported.stale.len()
            );
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
    if procedural::is_source(input) {
        let map = if sidecar.exists() {
            match meta::read(&sidecar)?.importer {
                ImportSettings::ProceduralTexture(settings) => settings.map,
                _ => bail!("{} isn't a procedural texture's sidecar", sidecar.display()),
            }
        } else {
            SurfaceMap::Color
        };
        let bytes = std::fs::read(input).with_context(|| format!("failed to read {}", input.display()))?;
        let surface =
            ProceduralTexture::read(&bytes).with_context(|| format!("failed to read {}", input.display()))?;
        let png = procedural::to_png(&surface.rasterize(map), map).context("failed to encode the PNG")?;
        return write_png(output, &png);
    }
    let settings = if sidecar.exists() {
        match meta::read(&sidecar)?.importer {
            ImportSettings::CharacterMap(settings, _) => settings,
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
    write_png(output, &png)
}

/// `--sheet`'s comma-separated light angles, each 0 to 180 degrees.
fn sheet_angles(text: &str) -> anyhow::Result<Vec<f32>> {
    text.split(',')
        .map(|angle| {
            let degrees: f32 =
                angle.trim().parse().with_context(|| format!("--sheet: {angle:?} isn't a number"))?;
            ensure!((0.0..=180.0).contains(&degrees), "--sheet: {degrees}° isn't between 0° and 180°");
            Ok(degrees)
        })
        .collect()
}

/// Generates the face map the document at `input` describes and writes the sheet an artist judges
/// it by at `angles` to `output` as a PNG.
fn bake_sheet(input: &Path, output: &Path, angles: &[f32]) -> anyhow::Result<()> {
    ensure!(
        MapKind::for_path(input) == Some(MapKind::FaceSdf),
        "--sheet draws face maps: {} isn't a .facesdf.ron document",
        input.display()
    );
    let sidecar = meta::sidecar_path(input);
    let settings = if sidecar.exists() {
        match meta::read(&sidecar)?.importer {
            ImportSettings::CharacterMap(MapSettings::FaceSdf(settings), _) => settings,
            _ => bail!("{} isn't a face map's sidecar", sidecar.display()),
        }
    } else {
        FaceSdfSettings::default()
    };
    let bytes = std::fs::read(input).with_context(|| format!("failed to read {}", input.display()))?;
    let sheet = character_maps::face_sheet(input, &bytes, &settings, angles)
        .with_context(|| format!("failed to draw the sheet of the map {} describes", input.display()))?;
    let mut png = std::io::Cursor::new(Vec::new());
    sheet.write_to(&mut png, image::ImageFormat::Png).context("failed to encode the PNG")?;
    write_png(output, &png.into_inner())
}

/// Cooks `source` into `out` as `options` says, then writes its audit and index beside it and, with `packs`, packs
/// it there, checking the names the code under `code` holds; with `show`, prints the cook's times.
#[expect(
    clippy::print_stdout,
    reason = "what was shipped beside the packs goes to stdout, as the cook's lines always have"
)]
fn cook_all(
    source: &Path,
    out: &Path,
    packs: Option<&Path>,
    code: Option<&Path>,
    options: &CookOptions<'_>,
    show: bool,
) -> anyhow::Result<()> {
    let cook = options.cook;
    let cache = options.cache;
    let mut report = cook::cook_dir(source, out, options)?;
    print_report(&report, cook, source, out);
    let mut watch = Stopwatch::start();
    let audited = audit::write(source, out, cook, cache, &report)?;
    if audited.written == audit::Written::Written {
        eprintln!("{cook}: wrote the cook audit {}", audited.path.display());
    }
    report.timings.ended(Phase::Audit, &mut watch);
    let (path, written) = index::write(source, out, cook, cache, &report, &audited.audit)?;
    if written == audit::Written::Written {
        eprintln!("{cook}: wrote the asset index {}", path.display());
    }
    report.timings.ended(Phase::Index, &mut watch);
    if let Some(packs) = packs {
        if let Some(code) = code {
            let names =
                code_names::scan(code).with_context(|| format!("reading the code in {}", code.display()))?;
            pack::check_code_names(&report, cache, &names)?;
        }
        let packed = pack::pack_cooked(&report, packs, cache)?;
        report.timings.ended(Phase::Pack, &mut watch);
        print_packs(&packed, cook);
        let shipped = documents::ship_documents(source, packs)?;
        for path in &shipped.written {
            println!("  shipped {}", path.display());
        }
        for path in &shipped.removed {
            println!("  removed {}", path.display());
        }
        if let Some(path) = release::write_manifest(packs, cook)? {
            println!("  wrote {}", path.display());
        }
        report.timings.ended(Phase::Ship, &mut watch);
    }
    if show {
        let count = timing::SLOWEST;
        eprint!("{}", Slowest { report: &report, cook, source, out, count });
    }
    Ok(())
}

fn write_png(output: &Path, png: &[u8]) -> anyhow::Result<()> {
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
    for waived in &report.waivers {
        eprintln!("waived {} for {}: {}", waived.rule, relative(&waived.source, source), waived.why);
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
        "gen-effects" => {
            Command::GenEffects { out: options.required("--out")?, force: options.flag("--force") }
        }
        "gen-figures" => Command::GenFigures {
            specs: options.required("--specs")?,
            out: options.required("--out")?,
            force: options.flag("--force"),
        },
        "gen-bench" => Command::GenBench {
            bench: options.required("--bench")?,
            preset: options.required("--preset")?,
            cook: options.value("--cook")?.unwrap_or(Cook::Desktop),
            cells: options.value("--cell-levels")?.unwrap_or_default(),
            out: options.required("--out")?,
        },
        "gen-kinds" => Command::GenKinds {
            bench: options.required("--bench")?,
            preset: options.required("--preset")?,
            specs: options.required("--specs")?,
            content: options.required("--content")?,
            out: options.required("--out")?,
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
            let sheet =
                options.value::<String>("--sheet")?.map(|angles| sheet_angles(&angles)).transpose()?;
            let [input, output] = options.positional()?;
            Command::BakeMap { input: input.into(), output: output.into(), sheet }
        }
        "extract-materials" => {
            let all = options.flag("--all");
            let [source] = options.positional()?;
            Command::ExtractMaterials { source: source.into(), all }
        }
        "new-character" => {
            let role = match options.value::<String>("--role")? {
                Some(name) => Some(check::Role::named(&name).with_context(|| {
                    let roles = check::Role::ALL.map(check::Role::name).join(", ");
                    format!("invalid --role {name:?}: the roles are {roles}")
                })?),
                None => None,
            };
            let mut parts = BTreeMap::new();
            while let Some(part) = options.value::<String>("--part")? {
                let (slot, part) = part
                    .rsplit_once('=')
                    .with_context(|| format!("invalid --part {part:?}: give it as <slot>=<part>"))?;
                ensure!(
                    parts.insert(slot.to_owned(), part.parse()?).is_none(),
                    "--part names {slot:?} twice"
                );
            }
            Command::NewCharacter(NewCharacter {
                model: options.required("--model")?,
                out: options.value("--out")?,
                role,
                preset: options.value("--preset")?,
                parts,
            })
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
            report: options.flag("--report"),
            refusal: options.value("--refusal")?,
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
        "manifest" => {
            Command::Manifest { packs: options.required("--packs")?, cook: options.required("--cook")? }
        }
        "check" => {
            let json = options.flag("--json");
            let [source] = options.positional()?;
            Command::Check { source: source.into(), json }
        }
        "content-check" => Command::ContentCheck {
            source: options.required("--source")?,
            out: options.required("--out")?,
            cook: options.value("--cook")?.unwrap_or(Cook::Desktop),
            cache: options.value("--cache")?,
            locked: options.flag("--locked"),
            touched: options.rest().into_iter().map(PathBuf::from).collect(),
        },
        "graph" => {
            let index = options.required("--index")?;
            let reverse = options.flag("--reverse");
            let [asset] = options.positional()?;
            Command::Graph { index, asset, reverse }
        }
        "vo" => Command::Vo { content: options.required("--content")?, check: options.flag("--check") },
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

    /// Takes every positional argument left, in order.
    fn rest(&mut self) -> Vec<String> {
        self.0.extract_if(.., |arg| !arg.starts_with("--")).collect()
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
            parse(&["gen-bench", "--bench", "bench", "--preset", "budget", "--out", "src"]).unwrap(),
            Command::GenBench {
                bench: "bench".into(),
                preset: Preset::Budget,
                cook: Cook::Desktop,
                cells: bench::CellLevels::On,
                out: "src".into()
            }
        );
        assert_eq!(
            parse(&["gen-bench", "--bench", "bench", "--preset", "ci", "--cook", "mobile", "--out", "src"])
                .unwrap(),
            Command::GenBench {
                bench: "bench".into(),
                preset: Preset::Ci,
                cook: Cook::Mobile,
                cells: bench::CellLevels::On,
                out: "src".into()
            }
        );
        let whole =
            ["gen-bench", "--bench", "bench", "--preset", "ci", "--cell-levels", "off", "--out", "src"];
        assert!(matches!(parse(&whole).unwrap(), Command::GenBench { cells: bench::CellLevels::Off, .. }));
        assert!(
            parse(&["gen-bench", "--bench", "b", "--preset", "ci", "--cell-levels", "x", "--out", "s"])
                .is_err()
        );
        assert!(parse(&["gen-bench", "--bench", "bench", "--preset", "huge", "--out", "src"]).is_err());
        assert_eq!(
            parse(&[
                "gen-kinds",
                "--bench",
                "bench",
                "--preset",
                "budget",
                "--specs",
                "figures",
                "--content",
                "content",
                "--out",
                "src",
            ])
            .unwrap(),
            Command::GenKinds {
                bench: "bench".into(),
                preset: Preset::Budget,
                specs: "figures".into(),
                content: "content".into(),
                out: "src".into(),
            }
        );
        assert!(parse(&["gen-kinds", "--bench", "bench", "--preset", "budget", "--out", "src"]).is_err());
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
            parse(&["gen-effects", "--out", "dir"]).unwrap(),
            Command::GenEffects { out: "dir".into(), force: false }
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
            Command::BakeMap { input: "face.facesdf.ron".into(), output: "face.png".into(), sheet: None }
        );
        assert_eq!(
            parse(&["bake-map", "face.facesdf.ron", "sheet.png", "--sheet", "0, 45,90.5"]).unwrap(),
            Command::BakeMap {
                input: "face.facesdf.ron".into(),
                output: "sheet.png".into(),
                sheet: Some(vec![0.0, 45.0, 90.5]),
            }
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
                report: false,
                refusal: None,
            }
        );
        let Command::Cook { cache, locked, packs, report, refusal, .. } = parse(&[
            "cook",
            "--source=a",
            "--out=b",
            "--cook=mobile",
            "--cache",
            "target/cache",
            "--packs",
            "p",
            "--report",
            "--refusal",
            "why.ron",
        ])
        .unwrap() else {
            panic!("expected cook");
        };
        assert_eq!((cache, locked, packs), (Some("target/cache".into()), false, Some("p".into())));
        assert!(report, "the cook's times printed");
        assert_eq!(refusal, Some("why.ron".into()), "where a failed cook says why");
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
        for angles in ["", "45,", "-10", "181", "front"] {
            assert!(parse(&["bake-map", "a.facesdf.ron", "a.png", "--sheet", angles]).is_err(), "{angles:?}");
        }
        assert_eq!(
            parse(&["new-character", "--model", "hero.glb"]).unwrap(),
            Command::NewCharacter(NewCharacter { model: "hero.glb".into(), ..NewCharacter::default() })
        );
        assert_eq!(
            parse(&[
                "new-character",
                "--model",
                "hero.vrm",
                "--out",
                "content/hero",
                "--role",
                "enemy",
                "--preset",
                "presets/enemy.preset.ron",
                "--part",
                "Ribbon=hair",
                "--part=Face_00_SKIN=face",
            ])
            .unwrap(),
            Command::NewCharacter(NewCharacter {
                model: "hero.vrm".into(),
                out: Some("content/hero".into()),
                role: Some(check::Role::Enemy),
                preset: Some("presets/enemy.preset.ron".into()),
                parts: BTreeMap::from([
                    ("Face_00_SKIN".to_owned(), scaffold::Part::Face),
                    ("Ribbon".to_owned(), scaffold::Part::Hair),
                ]),
            })
        );
        assert!(parse(&["new-character"]).is_err(), "no model");
        assert!(parse(&["new-character", "--model", "a.vrm", "--role", "villain"]).is_err(), "no such role");
        assert!(parse(&["new-character", "--model", "a.vrm", "--part", "Face"]).is_err(), "no part");
        assert!(
            parse(&["new-character", "--model", "a.vrm", "--part", "Face=nose"]).is_err(),
            "no such part"
        );
        assert!(
            parse(&["new-character", "--model", "a.vrm", "--part", "Face=face", "--part", "Face=skin"])
                .is_err(),
            "a slot twice"
        );
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
        assert_eq!(
            parse(&["manifest", "--packs", "p", "--cook", "desktop"]).unwrap(),
            Command::Manifest { packs: "p".into(), cook: Cook::Desktop }
        );
        assert!(parse(&["manifest", "--packs", "p"]).is_err(), "missing --cook");
        assert_eq!(
            parse(&["check", "content"]).unwrap(),
            Command::Check { source: "content".into(), json: false }
        );
        assert_eq!(
            parse(&["check", "--json", "content"]).unwrap(),
            Command::Check { source: "content".into(), json: true }
        );
        assert!(parse(&["check"]).is_err(), "missing the tree");
        assert_eq!(
            parse(&[
                "content-check",
                "--source",
                "content",
                "--out",
                "scratch",
                "content/a.zone.ron",
                "b.cue.ron"
            ])
            .unwrap(),
            Command::ContentCheck {
                source: "content".into(),
                out: "scratch".into(),
                cook: Cook::Desktop,
                cache: None,
                locked: false,
                touched: vec!["content/a.zone.ron".into(), "b.cue.ron".into()],
            }
        );
        assert!(parse(&["content-check", "--source", "content"]).is_err(), "missing --out");
        assert_eq!(
            parse(&["graph", "lookdev/body.png", "--index", "index.json", "--reverse"]).unwrap(),
            Command::Graph { index: "index.json".into(), asset: "lookdev/body.png".into(), reverse: true }
        );
        assert!(parse(&["graph", "lookdev/body.png"]).is_err(), "missing the index");
        assert!(parse(&["graph", "--index", "index.json"]).is_err(), "missing the asset");
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
