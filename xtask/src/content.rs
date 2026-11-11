//! `cargo xtask content`: makes sure the sample sources exist, then cooks and packs `content/`
//! with the pipeline binary (spec §5.27, §7.4).
//!
//! Each cook writes loose cooked files to `target/content/loose/<cook>/` and packs them into
//! `target/content/<cook>/`, which then holds packs (`shared.spack`, a `zone_<stem>.spack` per zone, and
//! `audio.spack` and `fonts.spack`, which only apps that play sound or show text mount), `NOTICES.txt`,
//! the fonts' licenses, the game's `documents/`, and `release.smanifest`, the unsigned release manifest that
//! names all of it: what apps ship and desktop runs read (M1 A4, M3 U3, M4 P1). A build without `dev` mounts
//! only packs that manifest names, and `cargo xtask release` signs it (`crate::release`).
//!
//! Every run runs the pipeline, whose content cache (`target/content/cache/`, shared by both
//! cooks) makes it incremental: an asset is cooked only when an input its cache key covers
//! changed (its source bytes, sidecar settings, importer and cooker versions, the cook or
//! a file it references), an asset is compressed for a pack only when its cook changed, an
//! output or pack is written only when its bytes change, and cooked files whose source is gone
//! are removed. Rebuilding the pipeline recooks nothing, so a change to an importer, cooker or
//! the packer must bump that stage's version (see `sapphire_pipeline::cache`).
//!
//! With `--locked` (CI), a source without a committed `.meta` sidecar fails the cook instead
//! of getting a fresh ID that would be thrown away. A lock file per cook keeps concurrent
//! xtask runs from changing content another run is still copying or running against.
//!
//! Generating missing sources comes first, under a lock of its own
//! (`target/content/generate.lock`), taken and released before the cook's: concurrent runs (a
//! desktop and a mobile cook in one checkout) never generate at once, and none cooks while
//! another is generating. The generators write only missing files, so generation never
//! overwrites a hand edit. With `--locked`, nothing is generated: a missing generated source
//! fails the run, naming it, since generated sources are committed and `--locked` never writes
//! sources.

use std::fmt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use anyhow::anyhow;

use crate::workspace::{self, BuildProfile, CargoBuild};

/// Source assets live here, relative to the workspace root.
pub const SOURCE_DIR: &str = "content";

/// A target cook (spec §2): `desktop` for macOS, Windows and Linux, `mobile` for iOS and
/// Android. Mirrors `sapphire_pipeline::cook::Cook`; xtask runs the pipeline as a
/// process rather than linking it, since nothing may depend on a tool crate.
///
/// `editor` is the desktop cook and what only an editor reads (each zone's shapes, M3 E5), in a folder of its
/// own, so no pack an app ships holds editor data: what `cargo xtask editor` opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cook {
    Desktop,
    Mobile,
    Editor,
}

impl Cook {
    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Mobile => "mobile",
            Self::Editor => "editor",
        }
    }

    /// The pipeline's cook it is.
    fn target(self) -> &'static str {
        match self {
            Self::Desktop | Self::Editor => "desktop",
            Self::Mobile => "mobile",
        }
    }
}

impl fmt::Display for Cook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Cook {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        [Self::Desktop, Self::Mobile, Self::Editor]
            .into_iter()
            .find(|cook| cook.name() == s)
            .ok_or_else(|| anyhow!("unknown cook `{s}`; expected `desktop`, `mobile` or `editor`"))
    }
}

/// Where a cook's packs go: `target/content/<cook>/`, the content root apps ship.
pub fn cooked_dir(cook: Cook) -> PathBuf {
    workspace::output_dir().join("content").join(cook.name())
}

/// Where a cook's loose cooked files go: `target/content/loose/<cook>/`, which tools read and
/// which never ships.
fn loose_dir(cook: Cook) -> PathBuf {
    workspace::output_dir().join("content").join("loose").join(cook.name())
}

/// The content cache both cooks share: `target/content/cache/`. Keys include the cook.
fn cache_dir() -> PathBuf {
    workspace::output_dir().join("content").join("cache")
}

/// A cooked content directory, held under a shared lock: no other xtask run recooks it
/// until this is dropped.
#[derive(Debug)]
pub struct Cooked {
    pub dir: PathBuf,
    _lock: File,
}

/// Generates whatever sources are missing, then cooks all of `content/` for `cook`. Keep the
/// result alive while copying from or running against its directory.
pub fn cook(cook: Cook) -> anyhow::Result<Cooked> {
    let pipeline = pipeline()?;

    let source = workspace::root().join(SOURCE_DIR);
    fs::create_dir_all(workspace::output_dir().join("content"))?;
    // Each generator says what it writes, so what's missing is the pipeline's to find. It holds its lock
    // only while it does: this and the cook's are never nested.
    let mut generate = Command::new(&pipeline);
    generate.arg("generate").arg("--source").arg(&source).arg("--lock").arg(generate_lock());
    if workspace::locked() {
        generate.arg("--locked");
    }
    workspace::run(&mut generate)?;

    let out = cooked_dir(cook);
    let lock_path = out.with_extension("lock");
    let lock = workspace::lock_file(&lock_path)?;
    workspace::wait_for_lock(
        &lock,
        File::try_lock,
        File::lock,
        &format!("to finish with the {cook} content"),
    )?;
    run_cook(&pipeline, &source, &loose_dir(cook), &out, cook)?;
    // Another run may recook in the gap before the shared lock; its output is the same.
    lock.unlock()?;
    workspace::wait_for_lock(
        &lock,
        File::try_lock_shared,
        File::lock_shared,
        &format!("to finish with the {cook} content"),
    )?;
    Ok(Cooked { dir: out, _lock: lock })
}

/// Cooks `source`, a copy of `content/` with its generated sources, for `cook` into `loose` and `packs`, through
/// the shared cache: what `cargo xtask patch rehearse` cooks its edited release from (M4 P4). Nothing else
/// cooks into `packs`, so it takes no lock.
pub fn cook_copy(source: &Path, loose: &Path, packs: &Path, cook: Cook) -> anyhow::Result<()> {
    run_cook(&pipeline()?, source, loose, packs, cook)
}

/// Runs the pipeline's cook of `source` into `loose` and `packs`.
fn run_cook(pipeline: &Path, source: &Path, loose: &Path, packs: &Path, cook: Cook) -> anyhow::Result<()> {
    let mut command = Command::new(pipeline);
    command
        .arg("cook")
        .arg("--source")
        .arg(source)
        .arg("--out")
        .arg(loose)
        .arg("--packs")
        .arg(packs)
        .arg("--code")
        .arg(workspace::root())
        .args(["--cook", cook.target()])
        .arg("--cache")
        .arg(cache_dir());
    if workspace::locked() {
        command.arg("--locked");
    }
    if cook == Cook::Editor {
        command.arg("--editor");
    }
    workspace::run(&mut command)
}

/// Builds the pipeline and returns its executable: optimized, without paying for cross-crate LTO after
/// each cooker edit. Cargo reports the path, including custom target directories and suffixes.
pub fn pipeline() -> anyhow::Result<PathBuf> {
    workspace::build_binary(
        &CargoBuild {
            package: "sapphire_pipeline",
            target: None,
            profile: BuildProfile::Tools,
            features: &[],
            env: &[],
        },
        "sapphire_pipeline",
    )
}

/// The lock generation runs under: `target/content/generate.lock`.
fn generate_lock() -> PathBuf {
    workspace::output_dir().join("content").join("generate.lock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cooks_and_places_their_output() {
        assert_eq!("desktop".parse::<Cook>().unwrap(), Cook::Desktop);
        assert_eq!("mobile".parse::<Cook>().unwrap(), Cook::Mobile);
        assert_eq!("editor".parse::<Cook>().unwrap(), Cook::Editor);
        assert_eq!(Cook::Editor.target(), "desktop", "a desktop cook, with more");
        assert!("console".parse::<Cook>().is_err());
        assert!(cooked_dir(Cook::Editor).ends_with("target/content/editor"));
        assert!(cooked_dir(Cook::Mobile).ends_with("target/content/mobile"));
        assert!(loose_dir(Cook::Mobile).ends_with("target/content/loose/mobile"));
        assert!(cache_dir().ends_with("target/content/cache"));
        assert!(generate_lock().ends_with("target/content/generate.lock"));
    }
}
