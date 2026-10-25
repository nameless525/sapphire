//! `cargo xtask content`: makes sure the sample sources exist, then cooks `content/` with
//! the pipeline binary (spec §5.27, §7.4).
//!
//! A cook is skipped while nothing it depends on has changed: a stamp next to the output
//! records the pipeline binary and every source file's path, size and modification time.
//! Anything else recooks everything into a fresh directory; per-asset content hashes and
//! `--only` arrive in M1. A lock file per cook keeps concurrent xtask runs from deleting
//! content another run is still copying or running against.

use std::fmt;
use std::fs::{self, File, TryLockError};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use anyhow::{Context as _, anyhow};

use crate::workspace::{self, CargoBuild};

/// Source assets live here, relative to the workspace root.
pub const SOURCE_DIR: &str = "content";

/// Generated sample sources, relative to [`SOURCE_DIR`]. `gen-samples` writes all of them.
const SAMPLE_SOURCES: &[&str] = &["samples/mannequin.glb"];

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

/// Where a cook's output goes: `target/content/<cook>/`.
pub fn cooked_dir(cook: Cook) -> PathBuf {
    workspace::output_dir().join("content").join(cook.name())
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
        &CargoBuild { package: "sapphire_pipeline", target: None, release: true, env: &[] },
        "sapphire_pipeline",
    )?;

    let source = workspace::root().join(SOURCE_DIR);
    if SAMPLE_SOURCES.iter().any(|sample| !source.join(sample).exists()) {
        workspace::run(Command::new(&pipeline).arg("gen-samples").arg("--out").arg(source.join("samples")))?;
    }

    let out = cooked_dir(cook);
    let lock_path = out.with_extension("lock");
    let stamp_path = out.with_extension("stamp");
    fs::create_dir_all(workspace::output_dir().join("content"))?;
    let lock =
        File::create(&lock_path).with_context(|| format!("failed to create {}", lock_path.display()))?;
    wait_for(&lock, File::try_lock, File::lock, cook)?;
    let inputs = fingerprint(&pipeline, &source, cook)?;
    let stamp = |out: &Path| tree_fingerprint(out).map(|outputs| format!("{inputs}{outputs}"));
    let current = out.is_dir().then(|| stamp(&out).ok()).flatten();
    if current.is_some() && fs::read_to_string(&stamp_path).ok() == current {
        eprintln!("{cook} content is up to date");
    } else {
        // Remove the stamp first: an interrupted cook must never look up to date.
        if stamp_path.exists() {
            fs::remove_file(&stamp_path)?;
        }
        // Start clean so assets deleted from `content/` don't linger in builds.
        workspace::remove_dir_if_exists(&out)?;
        workspace::run(
            Command::new(&pipeline)
                .arg("cook")
                .arg("--source")
                .arg(&source)
                .arg("--out")
                .arg(&out)
                .args(["--cook", cook.name()]),
        )?;
        fs::write(&stamp_path, stamp(&out)?)
            .with_context(|| format!("failed to write {}", stamp_path.display()))?;
    }
    // Another run may recook in the gap before the shared lock; its output is the same.
    lock.unlock()?;
    wait_for(&lock, File::try_lock_shared, File::lock_shared, cook)?;
    Ok(Cooked { dir: out, _lock: lock })
}

/// Identifies everything a cook depends on: the cook, the pipeline binary and each source file
/// under `source` (see [`tree_fingerprint`]). Sources a glTF file references from outside
/// `source` aren't tracked. Only ever compared with the stamp of an earlier run of the same
/// xtask build; the stamp also holds the output's [`tree_fingerprint`], so cooked files that
/// were deleted or edited are cooked again.
fn fingerprint(pipeline: &Path, source: &Path, cook: Cook) -> anyhow::Result<String> {
    let mut hasher = DefaultHasher::new();
    cook.name().hash(&mut hasher);
    hash_file(pipeline, pipeline, &mut hasher)?;
    Ok(format!("{:016x}\n{}", hasher.finish(), tree_fingerprint(source)?))
}

/// Every file under `root`, by relative path, size and modification time. Hidden entries are
/// skipped, as the pipeline skips them.
fn tree_fingerprint(root: &Path) -> anyhow::Result<String> {
    let mut hasher = DefaultHasher::new();
    let mut pending = vec![root.to_owned()];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).with_context(|| format!("failed to list {}", dir.display()))? {
            let path = entry?.path();
            if path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')) {
                continue;
            }
            if path.is_dir() { pending.push(path) } else { files.push(path) }
        }
    }
    files.sort();
    for file in &files {
        hash_file(file.strip_prefix(root).unwrap_or(file), file, &mut hasher)?;
    }
    Ok(format!("{:016x}\n", hasher.finish()))
}

fn hash_file(name: &Path, path: &Path, hasher: &mut DefaultHasher) -> anyhow::Result<()> {
    let metadata = fs::metadata(path).with_context(|| format!("failed to read {}", path.display()))?;
    name.hash(hasher);
    metadata.len().hash(hasher);
    metadata.modified()?.hash(hasher);
    Ok(())
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
    fn parses_cooks() {
        assert_eq!("desktop".parse::<Cook>().unwrap(), Cook::Desktop);
        assert_eq!("mobile".parse::<Cook>().unwrap(), Cook::Mobile);
        assert!("console".parse::<Cook>().is_err());
        assert!(cooked_dir(Cook::Mobile).ends_with("target/content/mobile"));
    }

    #[test]
    fn fingerprints_change_with_sources() {
        let dir = std::env::temp_dir().join(format!("xtask_fingerprint_{}", std::process::id()));
        let source = dir.join("content");
        fs::create_dir_all(source.join("samples")).unwrap();
        fs::write(source.join("samples/a.glb"), b"one").unwrap();
        fs::write(source.join(".hidden"), b"ignored").unwrap();
        let pipeline = dir.join("pipeline");
        fs::write(&pipeline, b"binary").unwrap();

        let first = fingerprint(&pipeline, &source, Cook::Desktop).unwrap();
        assert_eq!(fingerprint(&pipeline, &source, Cook::Desktop).unwrap(), first, "stable");
        assert_ne!(fingerprint(&pipeline, &source, Cook::Mobile).unwrap(), first, "per cook");
        fs::write(source.join(".hidden"), b"still ignored").unwrap();
        assert_eq!(fingerprint(&pipeline, &source, Cook::Desktop).unwrap(), first, "hidden files");
        fs::write(source.join("samples/b.glb"), b"two").unwrap();
        assert_ne!(fingerprint(&pipeline, &source, Cook::Desktop).unwrap(), first, "new source");

        let out = dir.join("out");
        fs::create_dir_all(&out).unwrap();
        let empty = tree_fingerprint(&out).unwrap();
        fs::write(out.join("a.smesh"), b"cooked").unwrap();
        assert_ne!(tree_fingerprint(&out).unwrap(), empty, "outputs are fingerprinted too");
        fs::remove_dir_all(&dir).unwrap();
    }
}
