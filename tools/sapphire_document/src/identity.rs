//! Which document a file is: its ID, in a sidecar, which outlasts the file's name and place.
//!
//! The ID is the one the cooker already gives a source: a `.meta` beside it, the file's whole name with
//! `.meta` added, so a document and its cooked asset are one thing with one name, and the editor needs no
//! second scheme. The rules here are the cooker's (`sapphire_pipeline::meta`), and a test there holds them
//! together, since this crate may not depend on that one (R1):
//!
//! - **An ID never changes.** A sidecar is only read, never rewritten, and one that can't be read is an error
//!   rather than a reason to make another.
//! - **A missing sidecar** is made on opening, as an unlocked cook makes it, whole or not at all. It's to be
//!   committed with its source, since a locked cook, which is what CI runs, refuses a source without one.
//! - **A source without a sidecar while a sidecar has no source** is most likely a move made without its
//!   sidecar. Making a new ID would lose the old one, so nothing's made until a person says which.
//! - **Two sidecars with one ID** is a copy. Neither opens until a person deletes the copy's sidecar, which
//!   gives the copy a new ID.
//!
//! A look at the sidecars walks the roots and reads each one, so a [`crate::set::DocumentSet`] keeps its last
//! look and asks again only when it has to. Two roots that lead to one place are one root, whatever they're
//! called, so a directory isn't seen twice and a file doesn't claim its own ID against itself.
//!
//! A document no importer reads has a sidecar of [`DOCUMENT_TYPE`], holding only an ID. A cook leaves that
//! alone, as it leaves any sidecar whose source it has no importer for.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

use sapphire_assets::AssetId;

use crate::kinds::Kind;
use crate::text;

/// What a sidecar's name adds to its source's.
pub const SIDECAR_EXTENSION: &str = "meta";
/// The header of a sidecar the cooker wrote.
pub const ASSET_TYPE: &str = "sapphire::assets::AssetMeta";
/// The header of a sidecar that only says which document a file is, with no importer.
pub const DOCUMENT_TYPE: &str = "sapphire::document::DocumentMeta";

/// A document's ID that can't be known, or can't be trusted.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} isn't a sidecar that can be read: {why}. It's left as it is, since an ID never changes", path.display())]
    Unreadable { path: PathBuf, why: String },
    #[error(
        "{} and {} hold the same ID {id}; delete the sidecar of the copy to give it a new ID",
        first.display(),
        second.display()
    )]
    Duplicate { id: AssetId, first: PathBuf, second: PathBuf },
    #[error(
        "{} has no sidecar, and {} {} no source. If it was moved or renamed, move its sidecar with it to \
         keep its ID; otherwise delete the orphaned sidecar",
        source_file.display(),
        list(orphaned),
        if orphaned.len() == 1 { "has" } else { "have" }
    )]
    Moved { source_file: PathBuf, orphaned: Vec<PathBuf> },
    #[error("{} has a sidecar already, with no file: move that file back, or delete the sidecar", path.display())]
    SidecarWaiting { path: PathBuf },
    #[error("failed to get randomness for a new ID: {0}")]
    Entropy(String),
}

fn list(paths: &[PathBuf]) -> String {
    paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
}

/// The sidecar of `source`: its whole file name with `.meta` added.
#[must_use]
pub fn sidecar_path(source: &Path) -> PathBuf {
    let mut path = source.as_os_str().to_owned();
    path.push(".");
    path.push(SIDECAR_EXTENSION);
    PathBuf::from(path)
}

/// The source a sidecar belongs to.
#[must_use]
pub fn source_path(sidecar: &Path) -> PathBuf {
    sidecar.with_extension("")
}

/// A fresh random ID, laid out as a version 4 UUID, as the cooker's are.
pub fn new_id() -> Result<AssetId, IdentityError> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|error| IdentityError::Entropy(error.to_string()))?;
    bytes[6] = 0x40 | (bytes[6] & 0x0f);
    bytes[8] = 0x80 | (bytes[8] & 0x3f);
    Ok(AssetId::from_bytes(bytes))
}

/// The ID in a sidecar's text, read by its shape alone: its importer's settings are the cooker's business,
/// and may name importers this crate has never heard of.
pub fn parse_id(sidecar: &str) -> Result<AssetId, String> {
    let tree = text::scan(sidecar).map_err(|error| format!("{error:?}"))?;
    let header = tree.field("type").and_then(|node| node.string(sidecar));
    if !matches!(header, Some(ASSET_TYPE | DOCUMENT_TYPE)) {
        return Err(format!("its type is {header:?}"));
    }
    let id = tree
        .field("data")
        .and_then(|data| data.field("id"))
        .and_then(|node| node.string(sidecar))
        .ok_or_else(|| "it has no `data.id`".to_owned())?;
    id.parse::<AssetId>().map_err(|error| error.to_string())
}

/// The ID in the sidecar at `path`.
pub fn read_id(path: &Path) -> Result<AssetId, IdentityError> {
    let text = fs::read_to_string(path).map_err(|source| match source.kind() {
        io::ErrorKind::InvalidData => {
            IdentityError::Unreadable { path: path.to_owned(), why: source.to_string() }
        }
        _ => IdentityError::Io { path: path.to_owned(), source },
    })?;
    parse_id(&text).map_err(|why| IdentityError::Unreadable { path: path.to_owned(), why })
}

/// A new sidecar's text. `importer` is RON for the settings a new source of this kind gets, and with it the
/// bytes are the ones a cook would write. Without it, the sidecar only says which document the file is.
#[must_use]
pub fn sidecar_text(id: AssetId, importer: Option<&str>) -> String {
    match importer {
        Some(importer) => format!(
            "(\n    type: \"{ASSET_TYPE}\",\n    version: 1,\n    data: (\n        id: \"{id}\",\n        importer: {importer},\n    ),\n)\n"
        ),
        None => format!(
            "(\n    type: \"{DOCUMENT_TYPE}\",\n    version: 1,\n    data: (\n        id: \"{id}\",\n    ),\n)\n"
        ),
    }
}

/// What's beside the documents under some roots, looked at once: who claims which ID.
#[derive(Clone, Debug, Default)]
pub struct Sidecars {
    /// The ID each sidecar with a source holds, and which those are, in path order.
    claims: BTreeMap<AssetId, Vec<PathBuf>>,
    /// Sidecars whose source doesn't exist, with the IDs they hold, if they can be read.
    orphaned: BTreeMap<PathBuf, Option<AssetId>>,
    /// The files whose names say they're documents, sidecar or none.
    documents: BTreeMap<PathBuf, Kind>,
}

impl Sidecars {
    /// Walks `roots`, passing over hidden files and links: a cook is what refuses a link. A root that isn't
    /// there holds nothing, and [`distinct_roots`] says which ones to walk.
    pub fn scan(roots: &[PathBuf]) -> Result<Self, IdentityError> {
        let mut sidecars = Self::default();
        for root in distinct_roots(roots) {
            sidecars.walk(root)?;
        }
        Ok(sidecars)
    }

    fn walk(&mut self, dir: &Path) -> Result<(), IdentityError> {
        let read_error = |source| IdentityError::Io { path: dir.to_owned(), source };
        // A directory that's gone since it was listed has nothing in it.
        let listing = match fs::read_dir(dir) {
            Ok(listing) => listing,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(read_error(error)),
        };
        let mut entries = listing.collect::<Result<Vec<_>, _>>().map_err(read_error)?;
        entries.sort_by_key(fs::DirEntry::path);
        for entry in entries {
            let path = entry.path();
            let hidden =
                path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with('.'));
            let kind = entry.file_type().map_err(read_error)?;
            if hidden || kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                self.walk(&path)?;
            } else if path.extension().is_some_and(|extension| extension == SIDECAR_EXTENSION) {
                // One that can't be read matters when its own file opens, and to no other file.
                let id = read_id(&path).ok();
                if source_path(&path).exists() {
                    if let Some(id) = id {
                        let claims = self.claims.entry(id).or_default();
                        if !claims.contains(&path) {
                            claims.push(path);
                        }
                    }
                } else {
                    self.orphaned.insert(path, id);
                }
            } else if let Some(kind) = Kind::of(&path) {
                self.documents.insert(path, kind);
            }
        }
        Ok(())
    }

    /// Refuses `id` if two sidecars claim it.
    pub fn check_one_claim(&self, id: AssetId) -> Result<(), IdentityError> {
        match self.claims.get(&id).map(Vec::as_slice) {
            Some([first, second, ..]) => {
                Err(IdentityError::Duplicate { id, first: first.clone(), second: second.clone() })
            }
            _ => Ok(()),
        }
    }

    /// Whether this look saw this sidecar holding this ID, and no other sidecar holding it. It's what lets a
    /// later look go unmade: anything else is worth another look before it's believed.
    #[must_use]
    pub fn knows_alone(&self, sidecar: &Path, id: AssetId) -> bool {
        matches!(self.claims.get(&id).map(Vec::as_slice), Some([only]) if only == sidecar)
    }

    /// Adds a sidecar just written, so a look a moment old needn't be made again for it.
    pub fn note(&mut self, sidecar: PathBuf, id: AssetId) {
        let claims = self.claims.entry(id).or_default();
        if !claims.contains(&sidecar) {
            claims.push(sidecar);
        }
    }

    /// The files whose names say they're documents, in path order, as of this look.
    pub fn documents(&self) -> impl Iterator<Item = (&Path, Kind)> {
        self.documents.iter().map(|(path, kind)| (path.as_path(), *kind))
    }

    /// The sidecars with no source.
    #[must_use]
    pub fn orphaned(&self) -> Vec<PathBuf> {
        self.orphaned.keys().cloned().collect()
    }

    /// Where the file with this ID is now, if exactly one sidecar says.
    #[must_use]
    pub fn source_of(&self, id: AssetId) -> Option<PathBuf> {
        match self.claims.get(&id).map(Vec::as_slice) {
            Some([only]) => Some(source_path(only)),
            _ => None,
        }
    }
}

/// The roots worth walking: two that lead to one directory are one, and the first spelling stands, and one
/// inside another adds nothing to it. They're compared by where they lead, so `a/../b`, a link to `b` and, on a
/// file system that doesn't care, `B` are all `b`. A root that isn't there yet is compared as it's written.
#[must_use]
pub fn distinct_roots(roots: &[PathBuf]) -> Vec<&Path> {
    let places: Vec<PathBuf> =
        roots.iter().map(|root| fs::canonicalize(root).unwrap_or_else(|_| root.clone())).collect();
    let mut kept = Vec::with_capacity(roots.len());
    for (at, place) in places.iter().enumerate() {
        let covered = places.iter().enumerate().any(|(other, there)| {
            other != at && if there == place { other < at } else { place.starts_with(there) }
        });
        if !covered {
            kept.push(roots[at].as_path());
        }
    }
    kept
}

/// The ID of the document at `path`, making its sidecar if it has none and that's safe.
pub fn identify(path: &Path, importer: Option<&str>, sidecars: &Sidecars) -> Result<AssetId, IdentityError> {
    let sidecar = sidecar_path(path);
    if fs::symlink_metadata(&sidecar).is_ok() {
        return existing(&sidecar, sidecars);
    }
    let orphaned = sidecars.orphaned();
    if !orphaned.is_empty() {
        return Err(IdentityError::Moved { source_file: path.to_owned(), orphaned });
    }
    let id = new_id()?;
    write_sidecar(&sidecar, id, importer)?;
    // Another editor or a cook may have got there first, and then theirs is the ID.
    existing(&sidecar, sidecars)
}

/// The ID in `sidecar`, if nothing else claims it: a second sidecar under the roots, or, for a file outside
/// them, the one sidecar under them that does.
fn existing(sidecar: &Path, sidecars: &Sidecars) -> Result<AssetId, IdentityError> {
    let id = read_id(sidecar)?;
    sidecars.check_one_claim(id)?;
    if let Some(claim) = sidecars.claims.get(&id).and_then(|claims| claims.first())
        && !same_place(claim, sidecar)
    {
        return Err(IdentityError::Duplicate { id, first: claim.clone(), second: sidecar.to_owned() });
    }
    Ok(id)
}

fn same_place(a: &Path, b: &Path) -> bool {
    a == b
        || match (fs::canonicalize(a), fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// Writes a new sidecar with this ID, leaving one that's already there alone.
pub fn write_sidecar(sidecar: &Path, id: AssetId, importer: Option<&str>) -> Result<bool, IdentityError> {
    crate::disk::create_whole(sidecar, sidecar_text(id, importer).as_bytes())
        .map_err(|source| IdentityError::Io { path: sidecar.to_owned(), source })
}
