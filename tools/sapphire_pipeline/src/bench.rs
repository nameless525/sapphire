//! The benchmark's content (M5 B1): a source tree generated from the documents under `bench/`, at a preset's
//! scale, for a cook into `target/` and nothing else.
//!
//! The scale benchmark needs content a shipped game's size: hundreds of 2K texture sets, a district of
//! cells, a cast. Committed, that would be gigabytes; drawn by hand, it's a team's year. So `bench/` holds
//! documents only (a zone's layout, prefabs, procedural texture specs and figure specs), and
//! `sapphire_pipeline gen-bench --bench bench --preset <ci|budget|double> --out <DIR>` writes the sources they
//! describe into `DIR`, with a sidecar each, as an ordinary source tree a cook reads. `cargo xtask
//! bench-content` runs it and cooks the result under `target/bench/`. Nothing it writes is committed, and
//! no package ships it: shipping cooks read `content/` alone.
//!
//! # Parts
//!
//! The tree is made of parts ([`PARTS`]), each a folder of `bench/` and a generator of its own: the
//! district ([`district`], M5 B2's) and the cast ([`cast`], M5 B3's). A part reads its own folder, so its
//! presets and documents are its own, and returns its sources ([`Source`]) relative to its folder in the
//! output. This module writes them, gives each its sidecar with an ID derived from its path ([`asset_id`]),
//! and removes whatever an earlier run wrote that this one didn't. A part names another part's assets by
//! the same IDs, without reading its files.
//!
//! # Presets
//!
//! [`Preset::Ci`] is small, for CI to prove the route end to end in seconds. [`Preset::Budget`] is the
//! shipped game's load, which the benchmark measures: its textures fill the desktop texture pool.
//! [`Preset::Double`] (M5 B2) is the budget at twice the pool, for the texture streamer's proof; a part
//! that gives no `double` makes its budget. Each part says what a preset means for it in its own document.
//!
//! # Determinism
//!
//! Generating twice writes the same bytes (`generating_twice_writes_the_same_bytes`): parts place things
//! with `noise::hash` and the four operations, and every document goes through `sapphire_serialize`. A
//! figure's model is as deterministic as [`crate::figure`] says.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use bevy_reflect::TypePath;
use sapphire_assets::AssetId;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::figure::FigureError;
use crate::generators::Output;
use crate::meta::{self, AssetMeta, ImportSettings};
use crate::samples::{Existing, Generated, write_files};

pub mod cast;
pub mod district;
#[cfg(test)]
mod tests;

/// The root every benchmark asset's ID derives from ([`asset_id`]).
pub const BENCH_ROOT: AssetId = AssetId::from_text("4b0c8e31-5d27-4f6a-9a83-2e61c07db5f4");

/// The file a generated tree holds at its root, which says what made it. [`generate`] only removes files
/// from a directory that holds one, or an empty one, so it can't empty a directory it didn't write. Hidden,
/// so cooks skip it.
pub const MARKER: &str = ".bench-source";

/// Every part: its folder under `bench/` and in the output, and its generator.
pub const PARTS: [(&str, Generator); 2] = [("district", district::generate), ("cast", cast::generate)];

/// A part's generator: its folder under `bench/` and the preset, to its sources.
pub type Generator = fn(&Path, Preset) -> Result<Vec<Source>, BenchError>;

/// How big the generated content is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Preset {
    /// Small, for CI: proves the route end to end.
    Ci,
    /// The shipped game's load, which the benchmark measures.
    Budget,
    /// The budget with twice the pool's textures (M5 B2), for the texture streamer's proof: a part that says
    /// nothing of it makes its budget.
    Double,
}

impl Preset {
    pub const ALL: [Self; 3] = [Self::Ci, Self::Budget, Self::Double];

    pub fn name(self) -> &'static str {
        match self {
            Self::Ci => "ci",
            Self::Budget => "budget",
            Self::Double => "double",
        }
    }
}

impl fmt::Display for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Preset {
    type Err = BenchError;

    fn from_str(s: &str) -> Result<Self, BenchError> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.name() == s)
            .ok_or_else(|| BenchError::Preset(s.to_owned()))
    }
}

/// What each preset means for a part, in its document: `presets: (ci: (...), budget: (...))`, and
/// `double: Some((...))` for a part the double preset changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presets<T> {
    pub ci: T,
    pub budget: T,
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub double: Option<T>,
}

/// A missing `double`: a plain `default` would ask every preset's type for `Default`.
fn none<T>() -> Option<T> {
    None
}

impl<T> Presets<T> {
    pub fn get(&self, preset: Preset) -> &T {
        match preset {
            Preset::Ci => &self.ci,
            Preset::Budget => &self.budget,
            Preset::Double => self.double.as_ref().unwrap_or(&self.budget),
        }
    }
}

/// One generated file, relative to its part's folder in the output.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// With `/` between parts.
    pub path: String,
    pub bytes: Vec<u8>,
    /// Its sidecar's importer and whether it cooks, or none for a file only another source reads (a
    /// control map an ILM document packs).
    pub sidecar: Option<(ImportSettings, bool)>,
}

impl Source {
    /// A source that cooks, as `importer` reads it.
    pub fn asset(path: impl Into<String>, bytes: Vec<u8>, importer: ImportSettings) -> Self {
        Self { path: path.into(), bytes, sidecar: Some((importer, true)) }
    }

    /// A source that's only read by others: a prefab, which zones place.
    pub fn uncooked(path: impl Into<String>, bytes: Vec<u8>, importer: ImportSettings) -> Self {
        Self { path: path.into(), bytes, sidecar: Some((importer, false)) }
    }

    /// A file without a sidecar, read by another source.
    pub fn input(path: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self { path: path.into(), bytes, sidecar: None }
    }
}

/// The ID of the generated source at `path` in `part`'s folder: the same on every run and machine.
pub fn asset_id(part: &str, path: &str) -> AssetId {
    BENCH_ROOT.sub_asset(&format!("{part}/{path}"))
}

/// Why the benchmark's content can't be generated.
#[derive(Debug, thiserror::Error)]
pub enum BenchError {
    #[error("unknown preset `{0}`; expected `ci`, `budget` or `double`")]
    Preset(String),
    #[error("{path}: {source}")]
    Document { path: PathBuf, source: Box<DocumentError> },
    #[error("{path}: {why}")]
    Invalid { path: PathBuf, why: String },
    #[error("{part}: {path:?} isn't a plain relative path, or is written twice")]
    Path { part: &'static str, path: String },
    #[error(
        "{} holds files and no {MARKER}: it isn't a generated benchmark tree, so nothing in it is replaced",
        .0.display()
    )]
    NotGenerated(PathBuf),
    #[error(transparent)]
    Figure(#[from] FigureError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Reads a document of type `T` from `path`.
pub(crate) fn read_document<T: Versioned + DeserializeOwned + TypePath>(
    path: &Path,
) -> Result<T, BenchError> {
    let text = fs::read_to_string(path)?;
    sapphire_serialize::from_ron(&text)
        .map_err(|error| BenchError::Document { path: path.to_owned(), source: Box::new(error) })
}

/// A document's text, as every generated document is written.
pub(crate) fn document_bytes<T: Versioned + Serialize + TypePath>(document: &T) -> Vec<u8> {
    sapphire_serialize::to_ron(document).expect("a generated document serializes").into_bytes()
}

/// Generates every part's sources for `preset` from the documents under `bench` into `out`, each with its
/// sidecar, and removes what an earlier run left there that this one didn't write. `out` must be empty,
/// missing or a tree this wrote ([`MARKER`]).
pub fn generate(bench: &Path, preset: Preset, out: &Path) -> Result<Generated, BenchError> {
    let marker = out.join(MARKER);
    if out.is_dir() && !marker.is_file() && fs::read_dir(out)?.next().is_some() {
        return Err(BenchError::NotGenerated(out.to_owned()));
    }
    let mut outputs = Vec::new();
    let mut written = BTreeSet::new();
    for (part, generator) in PARTS {
        for source in generator(&bench.join(part), preset)? {
            let relative = Path::new(&source.path);
            let plain =
                relative.components().all(|component| matches!(component, std::path::Component::Normal(_)));
            let name = format!("{part}/{}", source.path);
            if !plain || source.path.contains('\\') || !written.insert(name.clone()) {
                return Err(BenchError::Path { part, path: source.path });
            }
            if let Some((importer, cook)) = source.sidecar {
                let meta = AssetMeta { id: asset_id(part, &source.path), importer, cook };
                let text = sapphire_serialize::to_ron(&meta).expect("a sidecar serializes");
                let sidecar = meta::sidecar_path(Path::new(&name)).to_string_lossy().replace('\\', "/");
                written.insert(sidecar.clone());
                outputs.push(Output::plain(sidecar, move || text.into_bytes()));
            }
            let bytes = source.bytes;
            outputs.push(Output::plain(name, move || bytes));
        }
    }
    fs::create_dir_all(out)?;
    meta::replace_file(
        &marker,
        format!("Generated by `sapphire_pipeline gen-bench --preset {preset}`: never commit it.\n")
            .as_bytes(),
    )?;
    let generated = write_files(out, outputs, Existing::Replace)?;
    remove_stale(out, out, &written)?;
    Ok(generated)
}

/// Removes every file under `dir` but the marker that `written` doesn't name, by its path from `root`, and
/// the directories that leaves empty.
fn remove_stale(root: &Path, dir: &Path, written: &BTreeSet<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            remove_stale(root, &path, written)?;
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
            continue;
        }
        let relative = path.strip_prefix(root).expect("under the root").to_string_lossy().replace('\\', "/");
        if relative != MARKER && !written.contains(&relative) {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}
