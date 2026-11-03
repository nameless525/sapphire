//! A game's documents, on their way beside its packs: see [`sapphire_assets::documents`].
//!
//! [`ship_documents`] reads `content/game.documents.ron`, checks that each document it names is there and says
//! it's the right kind, and copies them with the manifest under the packs' [`SHIPPED_DIR`], byte for byte. It
//! can't read further than a document's envelope, since what reads the rest links what a cooker may not
//! (gameplay, the UI, input): the game's start reads it all, and fails by name.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sapphire_assets::documents::{GameDocuments, MANIFEST_FILE, SHIPPED_DIR, shipped_path};
use sapphire_serialize::{DocumentError, document, to_ron};

use crate::meta;

/// Why a game's documents can't ship.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DocumentsError {
    #[error("{MANIFEST_FILE}: {0}")]
    Manifest(Box<DocumentError>),
    #[error("{MANIFEST_FILE}: `{field}` names {path:?}: {reason}")]
    BadPath { field: &'static str, path: String, reason: &'static str },
    #[error("{MANIFEST_FILE}: `{field}` names {path:?}, which isn't there: {source}")]
    Missing {
        field: &'static str,
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("{MANIFEST_FILE}: `{field}` names {path:?}, which doesn't read as a document: {source}")]
    NotADocument {
        field: &'static str,
        path: String,
        #[source]
        source: Box<DocumentError>,
    },
    #[error("{MANIFEST_FILE}: `{field}` names {path:?}, which is a {found}, not a {expected}")]
    WrongKind { field: &'static str, path: String, found: String, expected: &'static str },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// What [`ship_documents`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShippedDocuments {
    /// Files written, because they were new or had changed, the manifest among them.
    pub written: Vec<PathBuf>,
    /// Files from an earlier cook that nothing names now.
    pub removed: Vec<PathBuf>,
}

/// Checks the game's documents under `source` and ships them under `packs`. Only what changed is written, and
/// what's no longer named goes: this directory holds what the manifest says and nothing else.
pub fn ship_documents(source: &Path, packs: &Path) -> Result<ShippedDocuments, DocumentsError> {
    let manifest_path = source.join(MANIFEST_FILE);
    let manifest_bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            to_ron(&GameDocuments::default()).map_err(manifest_error)?.into_bytes()
        }
        Err(source) => return Err(DocumentsError::Io { path: manifest_path, source }),
    };
    let text = String::from_utf8_lossy(&manifest_bytes);
    let manifest = GameDocuments::from_ron(&text).map_err(manifest_error)?;

    let dir = packs.join(SHIPPED_DIR);
    let mut shipping = vec![(dir.join(MANIFEST_FILE), manifest_bytes.clone())];
    for named in manifest.named() {
        let (field, path) = (named.field, named.path.to_owned());
        if let Some(reason) = path_problem(&path) {
            return Err(DocumentsError::BadPath { field, path, reason });
        }
        let bytes = fs::read(source.join(&path)).map_err(|source| DocumentsError::Missing {
            field,
            path: path.clone(),
            source,
        })?;
        let found = document::type_name(&String::from_utf8_lossy(&bytes)).map_err(|source| {
            DocumentsError::NotADocument { field, path: path.clone(), source: Box::new(source) }
        })?;
        if found != named.type_name {
            return Err(DocumentsError::WrongKind { field, path, found, expected: named.type_name });
        }
        shipping.push((packs.join(shipped_path(&path)), bytes));
    }

    let mut shipped = ShippedDocuments::default();
    for (path, bytes) in &shipping {
        if fs::read(path).is_ok_and(|current| current == *bytes) {
            continue;
        }
        write_whole(path, bytes).map_err(|source| DocumentsError::Io { path: path.clone(), source })?;
        shipped.written.push(path.clone());
    }
    let kept: HashSet<&Path> = shipping.iter().map(|(path, _)| path.as_path()).collect();
    remove_others(&dir, &kept, &mut shipped.removed)
        .map_err(|source| DocumentsError::Io { path: dir.clone(), source })?;
    Ok(shipped)
}

fn manifest_error(error: DocumentError) -> DocumentsError {
    DocumentsError::Manifest(Box::new(error))
}

/// What an app's content path won't take, as `sapphire_platform`'s `ContentFs` says: a game finds out at its
/// cook, rather than on the first phone that reads it.
fn path_problem(path: &str) -> Option<&'static str> {
    let good = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-');
    if path.is_empty() {
        return Some("it's empty");
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Some("a document's path stays under content/, with no empty step");
        }
        if !component.chars().all(good) {
            return Some("content paths take only lowercase a-z, 0-9, `.`, `_` and `-`");
        }
    }
    None
}

/// Writes a file whole, so a run reading the directory sees no file or a complete one.
fn write_whole(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let staged = meta::staging_path(path);
    fs::write(&staged, bytes).and_then(|()| fs::rename(&staged, path)).inspect_err(|_| {
        let _ = fs::remove_file(&staged);
    })
}

/// Removes every file under `dir` that isn't in `kept`, and any directory that leaves empty.
fn remove_others(dir: &Path, kept: &HashSet<&Path>, removed: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            remove_others(&path, kept, removed)?;
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
        } else if !kept.contains(path.as_path()) {
            fs::remove_file(&path)?;
            removed.push(path);
        }
    }
    Ok(())
}
