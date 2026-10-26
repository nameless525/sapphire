//! `cargo xtask content`: makes sure the sample sources exist, then cooks and packs `content/`
//! with the pipeline binary (spec §5.27, §7.4).
//!
//! Each cook writes loose cooked files to `target/content/loose/<cook>/` and packs them into
//! `target/content/<cook>/`, which then holds packs only (`shared.spack`): what apps ship and
//! desktop runs read (M1 A4).
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

use std::fmt;
use std::fs::{self, File, TryLockError};
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;

use anyhow::{Context as _, anyhow};

use crate::workspace::{self, CargoBuild};

/// Source assets live here, relative to the workspace root.
pub const SOURCE_DIR: &str = "content";

/// Generated sample sources, relative to [`SOURCE_DIR`]. `gen-samples` writes all of them, and
/// the face map's threshold masks (`samples/face/masks/`), which its document names: a cook
/// missing one fails, naming it.
const SAMPLE_SOURCES: &[&str] = &[
    "samples/mannequin.glb",
    "samples/swatch.ramp.ron",
    "samples/swatch.ilm.ron",
    "samples/swatch_ids.png",
    "samples/swatch_bias.png",
    "samples/swatch_ramp.material.ron",
    "samples/swatch_subsurface.material.ron",
    "samples/swatch_bands.material.ron",
    "samples/face/face_head.glb",
    "samples/face/face.facesdf.ron",
    "samples/face/face_base.png",
    "samples/face/face_mask.png",
    "samples/face/face_expression.png",
    "samples/face/face.material.ron",
    "samples/face/hair.material.ron",
    "samples/face/bangs.material.ron",
    "samples/face/brow.material.ron",
    "samples/face/hair_mask.png",
    "samples/sky_clouds.png",
    "samples/sky_cirrus.png",
    "samples/sky_moon.png",
    "samples/sky_stars.png",
    "samples/sky_milky_way.png",
    "samples/sky.material.ron",
    "samples/environment/courtyard.glb",
    "samples/environment/ground.material.ron",
    "samples/environment/rock.material.ron",
    "samples/environment/stone.material.ron",
    "samples/environment/wall.material.ron",
    "samples/environment/lantern.material.ron",
    "samples/environment/terminator_band.png",
    "samples/environment/ground.png",
    "samples/environment/mist_sheet.glb",
    "samples/environment/haze_card.glb",
    "samples/environment/mist.material.ron",
    "samples/environment/haze.material.ron",
    "samples/environment/mist.png",
    "samples/hard_edges.glb",
];

/// A target cook (spec §2): `desktop` for macOS, Windows and Linux, `mobile` for iOS and
/// Android. Mirrors `sapphire_pipeline::cook::Cook`; xtask runs the pipeline as a
/// process rather than linking it, since nothing may depend on a tool crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cook {
    Desktop,
    Mobile,
}

impl Cook {
    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
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
        [Self::Desktop, Self::Mobile]
            .into_iter()
            .find(|cook| cook.name() == s)
            .ok_or_else(|| anyhow!("unknown cook `{s}`; expected `desktop` or `mobile`"))
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

/// Generates missing sample sources, then cooks all of `content/` for `cook`. Keep the
/// result alive while copying from or running against its directory.
pub fn cook(cook: Cook) -> anyhow::Result<Cooked> {
    // Release: cooking is the slow part, and the pipeline itself rarely changes.
    let pipeline = workspace::build_binary(
        &CargoBuild { package: "sapphire_pipeline", target: None, release: true, features: &[], env: &[] },
        "sapphire_pipeline",
    )?;

    let source = workspace::root().join(SOURCE_DIR);
    if SAMPLE_SOURCES.iter().any(|sample| !source.join(sample).exists()) {
        workspace::run(Command::new(&pipeline).arg("gen-samples").arg("--out").arg(source.join("samples")))?;
    }

    let out = cooked_dir(cook);
    let lock_path = out.with_extension("lock");
    fs::create_dir_all(workspace::output_dir().join("content"))?;
    let lock =
        File::create(&lock_path).with_context(|| format!("failed to create {}", lock_path.display()))?;
    wait_for(&lock, File::try_lock, File::lock, cook)?;
    let mut command = Command::new(&pipeline);
    command
        .arg("cook")
        .arg("--source")
        .arg(&source)
        .arg("--out")
        .arg(loose_dir(cook))
        .arg("--packs")
        .arg(&out)
        .args(["--cook", cook.name()])
        .arg("--cache")
        .arg(cache_dir());
    if workspace::locked() {
        command.arg("--locked");
    }
    workspace::run(&mut command)?;
    // Another run may recook in the gap before the shared lock; its output is the same.
    lock.unlock()?;
    wait_for(&lock, File::try_lock_shared, File::lock_shared, cook)?;
    Ok(Cooked { dir: out, _lock: lock })
}

/// Takes a lock with `try_lock`, falling back to the blocking `lock` with a message when
/// another xtask run holds it.
fn wait_for(
    file: &File,
    try_lock: fn(&File) -> Result<(), TryLockError>,
    lock: fn(&File) -> std::io::Result<()>,
    cook: Cook,
) -> anyhow::Result<()> {
    match try_lock(file) {
        Ok(()) => Ok(()),
        Err(TryLockError::WouldBlock) => {
            eprintln!("waiting for another xtask run to finish with the {cook} content...");
            Ok(lock(file)?)
        }
        Err(TryLockError::Error(error)) => Err(error).context("failed to lock cooked content"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cooks_and_places_their_output() {
        assert_eq!("desktop".parse::<Cook>().unwrap(), Cook::Desktop);
        assert_eq!("mobile".parse::<Cook>().unwrap(), Cook::Mobile);
        assert!("console".parse::<Cook>().is_err());
        assert!(cooked_dir(Cook::Mobile).ends_with("target/content/mobile"));
        assert!(loose_dir(Cook::Mobile).ends_with("target/content/loose/mobile"));
        assert!(cache_dir().ends_with("target/content/cache"));
    }
}
