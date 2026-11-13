//! `cargo xtask bench-content`: generates the benchmark's sources from `bench/` at a preset's scale and cooks
//! them, all under `target/bench/` (M5 B1).
//!
//! The pipeline's `gen-bench` writes the preset's sources into `target/bench/<preset>/sources/`, each with its
//! sidecar (`sapphire_pipeline::bench`); the pipeline's cook then reads them as it reads `content/`, into
//! loose files in `target/bench/<preset>/loose/<cook>/` and packs in `target/bench/<preset>/<cook>/`, through
//! a cache of its own in `target/bench/cache/`. So `rm -rf target/bench` takes all of it, and nothing it
//! makes is committed.
//!
//! **Nothing ships it.** Every app, package and release takes its content from [`crate::content::cook`],
//! which cooks `content/` alone into `target/content/`: `bench/` is beside `content/`, never in it, and the
//! modules that ship content never name this one (`shipping_code_never_reaches_the_bench`).
//!
//! Generating takes `target/bench/<preset>/generate.lock` and a cook `target/bench/<preset>/<cook>.lock`, as
//! `content` locks its cooks, so a desktop and a mobile cook of one preset can run at once.

use std::fs::{self, File};
use std::path::PathBuf;
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
                for dir in [source_dir(preset), cooked_dir(preset, cook), loose_dir(preset, cook)] {
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
