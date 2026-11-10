//! Release manifests (M4 P1): the unsigned one every cook writes beside its packs, and a signed release laid
//! out for a static host. The format and its checks are `sapphire_assets::manifest`'s; signing is only here,
//! so no build of the game links a signing key's code.
//!
//! [`write_manifest`] names everything a cook's directory ships: each pack by its size and header hash, and
//! `NOTICES.txt` and every file under `documents/` by its size and blake3, with whether a patch may change it
//! (`sapphire_assets::documents`' module docs say which). Anything else in the directory is an error, so the
//! manifest can't leave a shipped file out.
//!
//! [`publish`] signs that body with keys from the environment ([`keys_from_env`], [`KEY_VAR`]) and lays the
//! release out where any static host can serve it:
//!
//! ```text
//! <out>/release.smanifest               the current release: the one name that changes, written last
//! <out>/manifests/<version>.smanifest   every release published there, never rewritten
//! <out>/packs/<header hash>.spack       each pack, by its header hash in hex
//! <out>/files/<blake3>                  each file outside the packs, by its hash in hex
//! ```
//!
//! Every name but the first is its content's hash, so a host's copy of them never changes under a client
//! halfway through a download, and a new release adds files beside the old without touching them: a client
//! that read release 7's manifest keeps fetching release 7's bytes while release 8 goes up. Publishing to a
//! host means copying everything but `release.smanifest` first, and it last. The same `release.smanifest` is
//! what an app's own content carries in place of its cook's unsigned one.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use sapphire_assets::documents::{GameDocuments, MANIFEST_FILE, SHIPPED_DIR, shipped_path};
use sapphire_assets::manifest::{
    self, Manifest, ManifestError, ManifestSignature, PublicKey, ReleasedFile, ReleasedPack,
};
use sapphire_assets::pack::{HEADER_LEN, PACK_EXTENSION, PackError, PackHeader};

use crate::cook::Cook;
use crate::meta;
use crate::pack::NOTICES;

/// The environment variable a release's signing keys come from: each key's 32-byte ed25519 seed in hex,
/// separated by commas when a release is signed by an old key and a new one.
pub const KEY_VAR: &str = "SAPPHIRE_RELEASE_KEY";

/// Why a manifest couldn't be written or a release published.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{}: {source}", path.display())]
    Pack {
        path: PathBuf,
        #[source]
        source: PackError,
    },
    #[error("{}: {source}", path.display())]
    Manifest {
        path: PathBuf,
        #[source]
        source: ManifestError,
    },
    #[error("{}: {source}", path.display())]
    Documents {
        path: PathBuf,
        #[source]
        source: Box<sapphire_serialize::DocumentError>,
    },
    #[error("{} ships beside the packs, but no cook writes it: remove it, or teach the manifest it", path.display())]
    Unknown { path: PathBuf },
    #[error("{} is larger than the 64 MiB a release's file outside its packs may be", path.display())]
    TooLarge { path: PathBuf },
    #[error("{}: the content changed since its cook wrote the manifest: cook again", path.display())]
    Stale { path: PathBuf },
    #[error("{} already holds release {existing}, and release {version} isn't newer", out.display())]
    NotNewer { out: PathBuf, existing: u64, version: u64 },
    #[error("{} holds the {existing} cook's releases, not the {cook} cook's", out.display())]
    OtherCook { out: PathBuf, existing: String, cook: String },
    #[error("{} is there with other bytes: a host's copy is never rewritten", path.display())]
    Collision { path: PathBuf },
    #[error(
        "{KEY_VAR} isn't set: it holds the release key's 32-byte seed in hex, which only the maintainer has"
    )]
    NoKey,
    #[error("{KEY_VAR}'s key {index} isn't 64 hex digits")]
    BadKey { index: usize },
    #[error("successor key {index} isn't an ed25519 public key in 64 hex digits")]
    BadSuccessorKey { index: usize },
    #[error("a release names at least one key for its successor, and at most {}", manifest::MAX_KEYS)]
    SuccessorKeys,
    #[error("{0:?} isn't a date as YYYY-MM-DD")]
    BadDate(String),
}

/// What [`publish`] did.
#[derive(Debug)]
pub struct Published {
    /// The release, as signed.
    pub manifest: Manifest,
    /// `<out>/release.smanifest`: the release's signed manifest, which an app's content carries too.
    pub current: PathBuf,
    /// Packs and files copied, as against those the host directory already held.
    pub written: usize,
    pub kept: usize,
}

/// What a release says beyond its content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseOptions {
    /// Higher than every release published to the same place before.
    pub version: u64,
    /// When a client stops taking it as current, in seconds since the Unix epoch.
    pub expires: u64,
    /// The oldest app build it runs on.
    pub min_app_build: u32,
    /// The keys that may sign the next release: the signing keys' public halves when `None`. Rotating to a new
    /// key names it here, signed by the old one (and, while clients move, the new one too).
    pub successor_keys: Option<Vec<PublicKey>>,
}

/// What the cook's directory `packs` ships, as an unsigned manifest of `cook` (see the module docs).
pub fn manifest_of(packs: &Path, cook: Cook) -> Result<Manifest, ReleaseError> {
    let io_error = |path: &Path| {
        let path = path.to_owned();
        move |source| ReleaseError::Io { path, source }
    };
    let simulation = simulation_documents(packs)?;
    let mut released = Vec::new();
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(packs).map_err(io_error(packs))? {
        let path = entry.map_err(io_error(packs))?.path();
        let name = path.file_name().and_then(|name| name.to_str()).map(str::to_owned);
        let Some(name) = name else {
            return Err(ReleaseError::Unknown { path });
        };
        if name == manifest::FILE_NAME {
            continue;
        }
        if path.is_dir() && name == SHIPPED_DIR {
            collect_files(packs, &path, &simulation, &mut files)?;
        } else if name == NOTICES && path.is_file() {
            files.insert(name, file_entry(&path, NOTICES, true)?);
        } else if let Some(stem) = name.strip_suffix(&format!(".{PACK_EXTENSION}"))
            && path.is_file()
        {
            released.push(pack_entry(&path, stem)?);
        } else {
            return Err(ReleaseError::Unknown { path });
        }
    }
    released.sort_by(|a, b| a.name.cmp(&b.name));
    let manifest = Manifest {
        version: 0,
        expires: 0,
        min_app_build: 0,
        cook: cook.name().to_owned(),
        successor_keys: Vec::new(),
        packs: released,
        files: files.into_values().collect(),
    };
    manifest.body().map_err(|source| ReleaseError::Manifest { path: packs.to_owned(), source })?;
    Ok(manifest)
}

/// Writes the unsigned manifest of what `packs` ships, if it changed: what `cook --packs` does last, so every
/// cook has one. Returns its path if it was written.
pub fn write_manifest(packs: &Path, cook: Cook) -> Result<Option<PathBuf>, ReleaseError> {
    let path = packs.join(manifest::FILE_NAME);
    let manifest = manifest_of(packs, cook)?;
    let bytes =
        manifest.encode(&[]).map_err(|source| ReleaseError::Manifest { path: path.clone(), source })?;
    if fs::read(&path).is_ok_and(|current| current == bytes) {
        return Ok(None);
    }
    meta::replace_file(&path, &bytes).map_err(|source| ReleaseError::Io { path: path.clone(), source })?;
    Ok(Some(path))
}

/// Signs the release of the cook's directory `packs` with `keys` and lays it out under `out` (see the module
/// docs). The directory must be what its cook's manifest says, and `out` may hold earlier releases of the same
/// cook, older than this one.
pub fn publish(
    packs: &Path,
    out: &Path,
    options: &ReleaseOptions,
    keys: &[SigningKey],
) -> Result<Published, ReleaseError> {
    let cooked_path = packs.join(manifest::FILE_NAME);
    let cooked = read_manifest(&cooked_path)?;
    let cook = Cook::ALL
        .into_iter()
        .find(|cook| cook.name() == cooked.cook)
        .ok_or(ReleaseError::Stale { path: cooked_path.clone() })?;
    let current = manifest_of(packs, cook)?;
    if current.packs != cooked.packs || current.files != cooked.files {
        return Err(ReleaseError::Stale { path: cooked_path });
    }
    if keys.is_empty() {
        return Err(ReleaseError::NoKey);
    }
    let signers: Vec<PublicKey> = keys.iter().map(|key| key.verifying_key().to_bytes()).collect();
    let successor_keys = options.successor_keys.clone().unwrap_or_else(|| signers.clone());
    if successor_keys.is_empty() || successor_keys.len() > manifest::MAX_KEYS {
        return Err(ReleaseError::SuccessorKeys);
    }
    let release = Manifest {
        version: options.version,
        expires: options.expires,
        min_app_build: options.min_app_build,
        successor_keys,
        ..current
    };
    let current_path = out.join(manifest::FILE_NAME);
    let manifest_error = |source| ReleaseError::Manifest { path: current_path.clone(), source };
    let body = release.body().map_err(manifest_error)?;
    let signatures: Vec<ManifestSignature> = keys
        .iter()
        .map(|key| ManifestSignature {
            key: key.verifying_key().to_bytes(),
            signature: key.sign(&body).to_bytes(),
        })
        .collect();
    let signed = release.encode(&signatures).map_err(manifest_error)?;
    // What a client will do with it, before anything's written.
    Manifest::verify(&signed, &signers).map_err(manifest_error)?;

    if current_path.exists() {
        let existing = read_manifest(&current_path)?;
        if existing.cook != release.cook {
            return Err(ReleaseError::OtherCook {
                out: out.to_owned(),
                existing: existing.cook,
                cook: release.cook,
            });
        }
        if existing.version >= release.version {
            return Err(ReleaseError::NotNewer {
                out: out.to_owned(),
                existing: existing.version,
                version: release.version,
            });
        }
    }
    // Each copy is checked against what the release names as it's read, so a file changed since the check
    // above can't go up under a name that isn't its hash.
    let mut copies = Vec::with_capacity(release.packs.len() + release.files.len());
    for pack in &release.packs {
        let from = packs.join(sapphire_assets::pack::file_name(&pack.name));
        let to = out.join("packs").join(format!("{}.{PACK_EXTENSION}", hex(&pack.header_hash)));
        copies.push((from, to, Expected::Pack(pack.size, pack.header_hash)));
    }
    for file in &release.files {
        copies.push((
            packs.join(&file.path),
            out.join("files").join(hex(&file.hash)),
            Expected::File(file.hash),
        ));
    }
    let mut written = 0;
    for (from, to, expected) in &copies {
        let bytes = fs::read(from).map_err(|source| ReleaseError::Io { path: from.clone(), source })?;
        let named = match *expected {
            Expected::Pack(size, hash) => {
                bytes.len() as u64 == size
                    && PackHeader::read(&bytes).is_ok_and(|header| header.hash() == hash)
            }
            Expected::File(hash) => *blake3::hash(&bytes).as_bytes() == hash,
        };
        if !named {
            return Err(ReleaseError::Stale { path: from.clone() });
        }
        written += usize::from(write_once(to, &bytes)?);
    }
    let history = out.join("manifests").join(format!("{}.smanifest", release.version));
    write_once(&history, &signed)?;
    meta::replace_file(&current_path, &signed)
        .map_err(|source| ReleaseError::Io { path: current_path, source })?;
    Ok(Published {
        manifest: release,
        current: out.join(manifest::FILE_NAME),
        written,
        kept: copies.len() - written,
    })
}

/// The signing keys in `value`, [`KEY_VAR`]'s text: never echoed, even when it's wrong.
pub fn keys_from_env(value: Option<&str>) -> Result<Vec<SigningKey>, ReleaseError> {
    let value = value.filter(|value| !value.trim().is_empty()).ok_or(ReleaseError::NoKey)?;
    value
        .split(',')
        .enumerate()
        .map(|(index, seed)| {
            unhex(seed.trim()).map(|seed| SigningKey::from_bytes(&seed)).ok_or(ReleaseError::BadKey { index })
        })
        .collect()
}

/// Public keys, as `--next-keys` gives them: 64 hex digits each, separated by commas.
pub fn public_keys(value: &str) -> Result<Vec<PublicKey>, ReleaseError> {
    value
        .split(',')
        .enumerate()
        .map(|(index, key)| {
            unhex(key.trim())
                .filter(|key| ed25519_dalek::VerifyingKey::from_bytes(key).is_ok())
                .ok_or(ReleaseError::BadSuccessorKey { index })
        })
        .collect()
}

/// `YYYY-MM-DD`, as seconds since the Unix epoch at that day's start in UTC: what `--expires` takes.
pub fn date(text: &str) -> Result<u64, ReleaseError> {
    let bad = || ReleaseError::BadDate(text.to_owned());
    let shaped = text.len() == 10
        && text
            .bytes()
            .enumerate()
            .all(|(at, byte)| if at == 4 || at == 7 { byte == b'-' } else { byte.is_ascii_digit() });
    if !shaped {
        return Err(bad());
    }
    let number = |range: std::ops::Range<usize>| text[range].parse::<u32>().expect("digits");
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    if year < 1970 || !(1..=12).contains(&month) {
        return Err(bad());
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days_in_month = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=days_in_month[month as usize - 1]).contains(&day) {
        return Err(bad());
    }
    // Days from the epoch to the civil date (Howard Hinnant's `days_from_civil`), in years from March.
    let (year, month) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let (era, of_era) = (year / 400, year % 400);
    let of_year = (153 * month + 2) / 5 + day - 1;
    let of_era = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = u64::from(era) * 146_097 + u64::from(of_era) - 719_468;
    Ok(days * 86_400)
}

/// Which files under `documents/` a replay's run reads, by their path from the content's root: what the
/// shipped list of documents names as the simulation's, and the list itself.
fn simulation_documents(packs: &Path) -> Result<BTreeMap<String, bool>, ReleaseError> {
    let list = packs.join(SHIPPED_DIR).join(MANIFEST_FILE);
    let mut documents = BTreeMap::new();
    let text = match fs::read_to_string(&list) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(documents),
        Err(source) => return Err(ReleaseError::Io { path: list, source }),
    };
    let named = GameDocuments::from_ron(&text)
        .map_err(|source| ReleaseError::Documents { path: list.clone(), source: Box::new(source) })?;
    // The list says which file is the flow, so changing it changes what the simulation reads.
    documents.insert(format!("{SHIPPED_DIR}/{MANIFEST_FILE}"), true);
    for document in named.named() {
        let path = shipped_path(document.path);
        let simulation = documents.get(&path).copied().unwrap_or(false) || document.simulation;
        documents.insert(path, simulation);
    }
    Ok(documents)
}

/// Every file under `dir`, which must be one the shipped list names.
fn collect_files(
    packs: &Path,
    dir: &Path,
    simulation: &BTreeMap<String, bool>,
    files: &mut BTreeMap<String, ReleasedFile>,
) -> Result<(), ReleaseError> {
    let io_error = |source| ReleaseError::Io { path: dir.to_owned(), source };
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        if path.is_dir() {
            collect_files(packs, &path, simulation, files)?;
            continue;
        }
        let relative =
            path.strip_prefix(packs).expect("under the packs").to_str().map(|path| path.replace('\\', "/"));
        let Some((relative, &reads)) =
            relative.as_ref().and_then(|relative| simulation.get_key_value(relative))
        else {
            return Err(ReleaseError::Unknown { path });
        };
        files.insert(relative.clone(), file_entry(&path, relative, !reads)?);
    }
    Ok(())
}

fn file_entry(path: &Path, relative: &str, patchable: bool) -> Result<ReleasedFile, ReleaseError> {
    let bytes = fs::read(path).map_err(|source| ReleaseError::Io { path: path.to_owned(), source })?;
    if bytes.len() as u64 > manifest::MAX_FILE_SIZE {
        return Err(ReleaseError::TooLarge { path: path.to_owned() });
    }
    Ok(ReleasedFile {
        path: relative.to_owned(),
        size: bytes.len() as u64,
        hash: *blake3::hash(&bytes).as_bytes(),
        patchable,
    })
}

fn pack_entry(path: &Path, name: &str) -> Result<ReleasedPack, ReleaseError> {
    let io_error = |source| ReleaseError::Io { path: path.to_owned(), source };
    let size = fs::metadata(path).map_err(io_error)?.len();
    // A file shorter than a header is the pack's error to name, not the file system's.
    let mut header = Vec::with_capacity(HEADER_LEN);
    let file = fs::File::open(path).map_err(io_error)?;
    io::Read::read_to_end(&mut io::Read::take(file, HEADER_LEN as u64), &mut header).map_err(io_error)?;
    let header =
        PackHeader::read(&header).map_err(|source| ReleaseError::Pack { path: path.to_owned(), source })?;
    if header.file_len() != size {
        let source = PackError::Truncated { expected: header.file_len(), found: size };
        return Err(ReleaseError::Pack { path: path.to_owned(), source });
    }
    Ok(ReleasedPack { name: name.to_owned(), size, header_hash: header.hash() })
}

fn read_manifest(path: &Path) -> Result<Manifest, ReleaseError> {
    let bytes = fs::read(path).map_err(|source| ReleaseError::Io { path: path.to_owned(), source })?;
    Manifest::read(&bytes).map_err(|source| ReleaseError::Manifest { path: path.to_owned(), source })
}

/// What a copy into a release must be: a pack of this size and header hash, or a file of this blake3.
enum Expected {
    Pack(u64, [u8; 32]),
    File([u8; 32]),
}

/// Writes `bytes` to `path` unless it holds them already; fails if it holds others. Returns whether it wrote.
fn write_once(path: &Path, bytes: &[u8]) -> Result<bool, ReleaseError> {
    match fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(false),
        Ok(_) => return Err(ReleaseError::Collision { path: path.to_owned() }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(ReleaseError::Io { path: path.to_owned(), source }),
    }
    let io_error = |source| ReleaseError::Io { path: path.to_owned(), source };
    fs::create_dir_all(path.parent().expect("under the release")).map_err(io_error)?;
    meta::replace_file(path, bytes).map_err(io_error)?;
    Ok(true)
}

fn hex(bytes: &[u8; 32]) -> String {
    blake3::Hash::from_bytes(*bytes).to_hex().to_string()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let digit = |byte: u8| char::from(byte).to_digit(16);
    let mut out = [0; 32];
    for (byte, &[high, low]) in out.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        *byte = (digit(high)? * 16 + digit(low)?) as u8;
    }
    Some(out)
}

#[cfg(test)]
mod tests;
