//! `cargo xtask bench-content`: generates the benchmark's sources from `bench/` at a preset's scale and cooks
//! them, all under `target/bench/` (M5 B1).
//!
//! The pipeline's `gen-bench` writes the preset's sources into `target/bench/<preset>/sources/`, each with its
//! sidecar (`sapphire_pipeline::bench`); the pipeline's cook then reads them as it reads `content/`, into
//! loose files in `target/bench/<preset>/loose/<cook>/` and packs in `target/bench/<preset>/<cook>/`, through
//! a cache of its own in `target/bench/cache/`. So `rm -rf target/bench` takes all of it, and nothing it
//! makes is committed. A sources folder that holds files but not `gen-bench`'s marker, as a tool that prunes
//! `target/` can leave it, is removed first and generated anew, since `gen-bench` replaces only its own trees.
//!
//! **Nothing ships it.** Every app, package and release takes its content from [`crate::content::cook`],
//! which cooks `content/` alone into `target/content/`: `bench/` is beside `content/`, never in it, and the
//! modules that ship content never name this one (`shipping_code_never_reaches_the_bench`).
//!
//! Generating takes `target/bench/<preset>/generate.lock` and a cook `target/bench/<preset>/<cook>.lock`, as
//! `content` locks its cooks, so a desktop and a mobile cook of one preset can run at once.
//!
//! **The kinds** ([`cook_kinds`], M5 C12b): a preset past `ci` cooks the shipped content with the party's kinds
//! and the wave's at spec §3.4's rows, as `bench/kinds.ron` says. The pipeline's `gen-kinds` links `content/`
//! into `target/bench/<preset>/kinds/source/` and writes over it those figures, made again from their specs in
//! `tools/figures/` under the same IDs; the tree cooks as `content/` does, through its cache
//! ([`content::cook_copy`]), into `target/bench/<preset>/kinds/<cook>/`. With the preset's district beside it
//! ([`with_content_for`]) that is the one root a run at the preset mounts: the district's walks, the arena's
//! fight of the party's kinds, and the district's encounter.
//!
//! **What tests draw** ([`with_content`], M5 X4): the golden scenes and CI's scale lane (`cargo xtask counts`)
//! run a shipping build, which mounts only what its release manifest names, so they take the shipped desktop
//! cook with the `ci` district's pack beside it, in `target/bench/ci/with-content/desktop/`, and a manifest
//! naming both that the pipeline writes (`sapphire_pipeline manifest`). The folder is a test's, never an
//! app's or a release's.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use anyhow::{anyhow, bail};

use crate::content::{self, Cook, CookRun};
use crate::workspace;

/// The benchmark's documents, relative to the workspace root.
pub const SOURCE_DIR: &str = "bench";

/// How big the generated content is: `sapphire_pipeline::bench::Preset`, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Small, for CI: the route end to end in seconds.
    Ci,
    /// The shipped game's load, which the benchmark measures: gigabytes.
    Budget,
    /// The budget with twice the pool's textures (M5 B2), for the texture streamer's proof: never cooked
    /// unless asked.
    Double,
}

impl Preset {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ci => "ci",
            Self::Budget => "budget",
            Self::Double => "double",
        }
    }
}

impl FromStr for Preset {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        [Self::Ci, Self::Budget, Self::Double]
            .into_iter()
            .find(|preset| preset.name() == s)
            .ok_or_else(|| anyhow!("unknown preset `{s}`; expected `ci`, `budget` or `double`"))
    }
}

/// Everything the benchmark's content makes: `target/bench/`.
pub fn output_dir() -> PathBuf {
    workspace::output_dir().join("bench")
}

/// A preset's generated sources: `target/bench/<preset>/sources/`.
pub fn source_dir(preset: Preset) -> PathBuf {
    output_dir().join(preset.name()).join("sources")
}

/// A preset's packs for a cook: `target/bench/<preset>/<cook>/`, what a benchmark run mounts.
pub fn cooked_dir(preset: Preset, cook: Cook) -> PathBuf {
    output_dir().join(preset.name()).join(cook.name())
}

fn loose_dir(preset: Preset, cook: Cook) -> PathBuf {
    output_dir().join(preset.name()).join("loose").join(cook.name())
}

/// The benchmark's own content cache, apart from `content`'s so that removing `target/bench/` takes it.
fn cache_dir() -> PathBuf {
    output_dir().join("cache")
}

/// A preset's cooked packs, held under a shared lock until dropped.
#[derive(Debug)]
pub struct BenchCooked {
    pub dir: PathBuf,
    _lock: File,
}

/// Generates `preset`'s sources and cooks them for `cook`, desktop or mobile.
pub fn cook(preset: Preset, cook: Cook) -> anyhow::Result<BenchCooked> {
    if cook == Cook::Editor {
        bail!("the benchmark cooks for desktop or mobile, not the editor");
    }
    let pipeline = content::pipeline()?;
    let preset_dir = output_dir().join(preset.name());
    fs::create_dir_all(&preset_dir)?;

    let source = source_dir(preset);
    {
        let lock = workspace::lock_file(&preset_dir.join("generate.lock"))?;
        workspace::wait_for_lock(
            &lock,
            File::try_lock,
            File::lock,
            &format!("to finish generating {}", preset.name()),
        )?;
        if clear_unmarked(&source)? {
            eprintln!(
                "removed {}: it held files but no {MARKER}, so something pruned it (a build cache, a cleaner); \
                 generating it anew",
                source.display()
            );
        }
        let mut generate = Command::new(&pipeline);
        generate
            .arg("gen-bench")
            .arg("--bench")
            .arg(workspace::root().join(SOURCE_DIR))
            .args(["--preset", preset.name()])
            .arg("--out")
            .arg(&source);
        workspace::run(&mut generate)?;
    }

    let out = cooked_dir(preset, cook);
    let lock = workspace::lock_file(&out.with_extension("lock"))?;
    let waiting = format!("to finish with the {} {cook} cook", preset.name());
    workspace::wait_for_lock(&lock, File::try_lock, File::lock, &waiting)?;
    // Every source is generated with its sidecar, so one missing is a fault: always locked.
    let run = CookRun {
        source: &source,
        loose: &loose_dir(preset, cook),
        packs: &out,
        cook,
        cache: &cache_dir(),
        code: None,
        locked: true,
    };
    content::run_pipeline_cook(&pipeline, &run)?;
    lock.unlock()?;
    workspace::wait_for_lock(&lock, File::try_lock_shared, File::lock_shared, &waiting)?;
    Ok(BenchCooked { dir: out, _lock: lock })
}

/// The file `gen-bench` writes into every tree it generates: `sapphire_pipeline::bench::MARKER`, which xtask
/// doesn't link (`the_marker_is_the_pipelines`).
const MARKER: &str = ".bench-source";

/// Removes `dir` if it holds anything but no [`MARKER`], and says whether it did. `gen-bench` refuses to
/// replace such a folder, rightly for a path a person typed; but this one is xtask's own, under `target/`,
/// and a tool that prunes `target/` (CI's build cache does) can leave a generated tree without its marker.
/// A tree with its marker is left to `gen-bench`, which rewrites and prunes it.
fn clear_unmarked(dir: &Path) -> anyhow::Result<bool> {
    if !dir.is_dir() || dir.join(MARKER).is_file() || fs::read_dir(dir)?.next().is_none() {
        return Ok(false);
    }
    fs::remove_dir_all(dir)?;
    Ok(true)
}

/// The district's pack in a preset's cook: the zone the generated sources are.
pub const DISTRICT_PACK: &str = "zone_district.spack";

/// Whether `preset` makes the kinds again: every preset past `ci`, whose kinds are the shipped ones
/// (`bench/kinds.ron` gives `ci` none).
pub fn has_kinds(preset: Preset) -> bool {
    preset != Preset::Ci
}

/// A preset's kinds: `target/bench/<preset>/kinds/`, its source tree in `source/` and a cook's packs in
/// `<cook>/`.
pub fn kinds_dir(preset: Preset) -> PathBuf {
    output_dir().join(preset.name()).join("kinds")
}

/// The figure specs the kinds are made again from, relative to the workspace root.
const SPECS_DIR: &str = "tools/figures";

/// The content with a preset's kinds, cooked: what stands in for the shipped cook in a run at the preset,
/// held under a shared lock until dropped.
#[derive(Debug)]
pub struct KindsCooked {
    pub dir: PathBuf,
    _lock: File,
    _source: File,
}

/// Makes the kinds' source tree for `preset` anew and cooks it for `cook`, desktop or mobile (see the module
/// docs). The tree is held shared while it cooks, so a desktop and a mobile cook can share it, and neither
/// makes it again under the other.
pub fn cook_kinds(preset: Preset, cook: Cook) -> anyhow::Result<KindsCooked> {
    if cook == Cook::Editor {
        bail!("the kinds cook for desktop or mobile, not the editor");
    }
    if !has_kinds(preset) {
        bail!("the {} preset's kinds are the shipped ones: cook content/", preset.name());
    }
    let pipeline = content::pipeline()?;
    content::generate(&pipeline)?;
    let dir = kinds_dir(preset);
    fs::create_dir_all(&dir)?;
    let source = dir.join("source");
    let source_lock = workspace::lock_file(&dir.join("source.lock"))?;
    let making = format!("to finish with the {} kinds' source", preset.name());
    workspace::wait_for_lock(&source_lock, File::try_lock, File::lock, &making)?;
    let root = workspace::root();
    let mut generate = Command::new(&pipeline);
    generate
        .arg("gen-kinds")
        .arg("--bench")
        .arg(root.join(SOURCE_DIR))
        .args(["--preset", preset.name()])
        .arg("--specs")
        .arg(root.join(SPECS_DIR))
        .arg("--content")
        .arg(root.join(content::SOURCE_DIR))
        .arg("--out")
        .arg(&source);
    workspace::run(&mut generate)?;
    source_lock.unlock()?;
    workspace::wait_for_lock(&source_lock, File::try_lock_shared, File::lock_shared, &making)?;

    let out = dir.join(cook.name());
    let lock = workspace::lock_file(&out.with_extension("lock"))?;
    let waiting = format!("to finish with the {} kinds' {cook} cook", preset.name());
    workspace::wait_for_lock(&lock, File::try_lock, File::lock, &waiting)?;
    content::cook_copy(&source, &dir.join("loose").join(cook.name()), &out, cook)?;
    lock.unlock()?;
    workspace::wait_for_lock(&lock, File::try_lock_shared, File::lock_shared, &waiting)?;
    Ok(KindsCooked { dir: out, _lock: lock, _source: source_lock })
}

/// A shipped cook with the `ci` district's pack beside it, for tests, held under a shared lock
/// until dropped (see the module docs).
#[derive(Debug)]
pub struct WithContent {
    pub dir: PathBuf,
    _lock: File,
    _content: content::Cooked,
    _bench: BenchCooked,
}

/// Cooks the `ci` district for `shipped`'s cook and puts it together with `shipped`, the desktop or mobile cook
/// of `content/`, for a shipping build: every file of the shipped cook but its manifest, linked (or copied where
/// links can't be made), the district's pack beside them, and a manifest of the folder from the pipeline, in
/// `target/bench/ci/with-content/<cook>/`. It takes the shipped cook already held, since a run that holds it
/// can't cook it again. The golden scenes mount the desktop one; the scale lane's phone walks the mobile one
/// (M5 Z3).
pub fn with_content(shipped: content::Cooked) -> anyhow::Result<WithContent> {
    let Some(target) =
        [Cook::Desktop, Cook::Mobile].into_iter().find(|&cook| shipped.dir == content::cooked_dir(cook))
    else {
        bail!("the district goes beside the desktop or the mobile cook, not {}", shipped.dir.display());
    };
    let bench = cook(Preset::Ci, target)?;
    let out = with_content_dir_for(Preset::Ci, target);
    fs::create_dir_all(out.parent().expect("under target/bench"))?;
    let lock = workspace::lock_file(&out.with_extension("lock"))?;
    let waiting = "to finish putting the content and the district together";
    workspace::wait_for_lock(&lock, File::try_lock, File::lock, waiting)?;
    put_beside(&shipped.dir, &bench.dir, &out, target)?;
    lock.unlock()?;
    workspace::wait_for_lock(&lock, File::try_lock_shared, File::lock_shared, waiting)?;
    Ok(WithContent { dir: out, _lock: lock, _content: shipped, _bench: bench })
}

/// A preset's district beside the content at that preset, for `cook`: `target/bench/<preset>/with-content/<cook>/`.
pub fn with_content_dir_for(preset: Preset, cook: Cook) -> PathBuf {
    output_dir().join(preset.name()).join("with-content").join(cook.name())
}

/// The content at a preset past `ci` (the shipped content with its kinds, [`cook_kinds`]) with the preset's
/// district beside it, for `cook`: the one root a run at the preset mounts, held under a shared lock until
/// dropped. `ci`'s is [`with_content`].
pub fn with_content_for(preset: Preset, cook: Cook) -> anyhow::Result<WithKinds> {
    let kinds = cook_kinds(preset, cook)?;
    let bench = self::cook(preset, cook)?;
    let out = with_content_dir_for(preset, cook);
    fs::create_dir_all(out.parent().expect("under target/bench"))?;
    let lock = workspace::lock_file(&out.with_extension("lock"))?;
    let waiting = format!("to finish putting the {} content and district together", preset.name());
    workspace::wait_for_lock(&lock, File::try_lock, File::lock, &waiting)?;
    put_beside(&kinds.dir, &bench.dir, &out, cook)?;
    lock.unlock()?;
    workspace::wait_for_lock(&lock, File::try_lock_shared, File::lock_shared, &waiting)?;
    Ok(WithKinds { dir: out, _lock: lock, _kinds: kinds, _bench: bench })
}

/// [`with_content_for`]'s root, held.
#[derive(Debug)]
pub struct WithKinds {
    pub dir: PathBuf,
    _lock: File,
    _kinds: KindsCooked,
    _bench: BenchCooked,
}

/// Makes `out` anew: every file of `shipped` (a cook of `content/`) but its manifest, linked, the district's pack
/// from `bench` (a preset's cook) beside them, and a manifest of the folder from the pipeline, so a shipping build
/// mounts both and a `dev` one finds both. [`with_content`]'s, and the benches at scale's (M5 Z1) for each cook.
pub fn put_beside(shipped: &Path, bench: &Path, out: &Path, cook: Cook) -> anyhow::Result<()> {
    if out.exists() {
        fs::remove_dir_all(out)?;
    }
    fs::create_dir_all(out)?;
    link_tree(shipped, out, &|name| name != MANIFEST)?;
    let district = bench.join(DISTRICT_PACK);
    if !district.is_file() {
        bail!("the benchmark's cook in {} has no {DISTRICT_PACK}", bench.display());
    }
    link(&district, &out.join(DISTRICT_PACK))?;
    let mut manifest = Command::new(content::pipeline()?);
    manifest.arg("manifest").arg("--packs").arg(out).args(["--cook", cook.name()]);
    workspace::run(&mut manifest)
}

/// A cook's release manifest, which [`put_beside`] writes anew.
const MANIFEST: &str = "release.smanifest";

/// Links every file under `from` into `to`, keeping the tree, but a top-level entry `keep` refuses.
fn link_tree(from: &Path, to: &Path, keep: &dyn Fn(&str) -> bool) -> anyhow::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if !keep(&name.to_string_lossy()) {
            continue;
        }
        let (source, target) = (entry.path(), to.join(&name));
        if entry.file_type()?.is_dir() {
            fs::create_dir_all(&target)?;
            link_tree(&source, &target, &|_| true)?;
        } else {
            link(&source, &target)?;
        }
    }
    Ok(())
}

/// A hard link from `to` to `from`'s bytes, or a copy where the filesystem won't link.
fn link(from: &Path, to: &Path) -> anyhow::Result<()> {
    if fs::hard_link(from, to).is_err() {
        fs::copy(from, to)
            .map_err(|error| anyhow!("failed to copy {} to {}: {error}", from.display(), to.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Whether `path` lies under `dir`, both made absolute from the workspace root.
    fn inside(path: &Path, dir: &Path) -> bool {
        let absolute =
            |path: &Path| if path.is_absolute() { path.to_owned() } else { workspace::root().join(path) };
        absolute(path).starts_with(absolute(dir))
    }

    #[test]
    fn parses_presets_and_places_their_output() {
        assert_eq!("ci".parse::<Preset>().unwrap(), Preset::Ci);
        assert_eq!("budget".parse::<Preset>().unwrap(), Preset::Budget);
        assert_eq!("double".parse::<Preset>().unwrap(), Preset::Double);
        assert!("huge".parse::<Preset>().is_err());
        assert!(source_dir(Preset::Budget).ends_with("target/bench/budget/sources"));
        assert!(cooked_dir(Preset::Ci, Cook::Mobile).ends_with("target/bench/ci/mobile"));
        assert!(loose_dir(Preset::Ci, Cook::Desktop).ends_with("target/bench/ci/loose/desktop"));
        assert!(cache_dir().ends_with("target/bench/cache"));
        assert!(
            with_content_dir_for(Preset::Ci, Cook::Desktop).ends_with("target/bench/ci/with-content/desktop")
        );
        assert!(kinds_dir(Preset::Budget).ends_with("target/bench/budget/kinds"));
        assert!(
            with_content_dir_for(Preset::Budget, Cook::Mobile)
                .ends_with("target/bench/budget/with-content/mobile")
        );
        assert!(!has_kinds(Preset::Ci) && has_kinds(Preset::Budget) && has_kinds(Preset::Double));
        assert!(workspace::root().join(SPECS_DIR).join("wren.figure.ron").is_file());
        assert!(workspace::root().join(SOURCE_DIR).join("kinds.ron").is_file());
    }

    #[test]
    fn a_tree_links_but_what_it_leaves_out() {
        let root = std::env::temp_dir().join(format!("xtask_link_tree_{}", std::process::id()));
        let (from, to) = (root.join("from"), root.join("to"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(from.join("documents")).unwrap();
        fs::create_dir_all(&to).unwrap();
        for (name, text) in [("shared.spack", "a"), (MANIFEST, "b"), ("documents/game.ron", "c")] {
            fs::write(from.join(name), text).unwrap();
        }
        link_tree(&from, &to, &|name| name != MANIFEST).unwrap();
        assert_eq!(fs::read_to_string(to.join("shared.spack")).unwrap(), "a");
        assert_eq!(fs::read_to_string(to.join("documents/game.ron")).unwrap(), "c");
        assert!(!to.join(MANIFEST).exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn only_a_tree_without_its_marker_is_cleared() {
        let root = std::env::temp_dir().join(format!("xtask_clear_unmarked_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        // Missing, then empty: nothing to do.
        assert!(!clear_unmarked(&root).unwrap());
        fs::create_dir_all(&root).unwrap();
        assert!(!clear_unmarked(&root).unwrap());
        assert!(root.is_dir());
        // Files with the marker: a generated tree, left to `gen-bench`.
        fs::create_dir_all(root.join("district")).unwrap();
        fs::write(root.join("district/block.ron"), "a").unwrap();
        fs::write(root.join(MARKER), "b").unwrap();
        assert!(!clear_unmarked(&root).unwrap());
        assert!(root.join("district/block.ron").is_file());
        // The same files without it: removed whole.
        fs::remove_file(root.join(MARKER)).unwrap();
        assert!(clear_unmarked(&root).unwrap());
        assert!(!root.exists());
    }

    #[test]
    fn the_marker_is_the_pipelines() {
        let source = workspace::root().join("tools/sapphire_pipeline/src/bench.rs");
        let text = fs::read_to_string(&source).unwrap();
        assert!(text.contains(&format!("pub const MARKER: &str = \"{MARKER}\";")), "{}", source.display());
    }

    #[test]
    fn the_bench_and_shipping_content_never_overlap() {
        // The documents are beside content/, never in it, so no shipping cook reads them.
        let (bench, shipped) =
            (workspace::root().join(SOURCE_DIR), workspace::root().join(content::SOURCE_DIR));
        assert!(bench.join("district/district.ron").is_file(), "bench/ is at the workspace's root");
        assert!(!inside(&bench, &shipped) && !inside(&shipped, &bench));
        // And what the bench makes is apart from what `content` makes, which every app ships.
        let shipping = workspace::output_dir().join("content");
        for cook in [Cook::Desktop, Cook::Mobile] {
            assert!(inside(&content::cooked_dir(cook), &shipping));
            for preset in [Preset::Ci, Preset::Budget, Preset::Double] {
                let kinds = [kinds_dir(preset), with_content_dir_for(preset, cook)];
                for dir in [source_dir(preset), cooked_dir(preset, cook), loose_dir(preset, cook)]
                    .into_iter()
                    .chain(kinds)
                {
                    assert!(!inside(&dir, &shipping) && !inside(&shipping, &dir), "{}", dir.display());
                }
            }
        }
        assert!(!inside(&cache_dir(), &shipping));
    }

    #[test]
    fn shipping_code_never_reaches_the_bench() {
        // Every app, package and release takes content from `content::cook`; these modules ship it, and none
        // may name the benchmark's content or where it goes.
        let src = workspace::root().join("xtask/src");
        let mut shipping = vec![src.join("content.rs"), src.join("release.rs"), src.join("patch.rs")];
        for entry in fs::read_dir(src.join("platforms")).unwrap() {
            shipping.push(entry.unwrap().path());
        }
        for path in shipping {
            let text = fs::read_to_string(&path).unwrap();
            for word in ["bench::", "target/bench", "gen-bench", "join(\"bench\")"] {
                assert!(!text.contains(word), "{} names `{word}`", path.display());
            }
        }
    }
}
