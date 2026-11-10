//! The release manifest (`release.smanifest`): what one release of a game's content is, and who signed it
//! (m4-plan §3 "What's signed", M4 P1).
//!
//! A manifest names every pack of a release by its name, size and header hash, and every other file the
//! content ships (`NOTICES.txt` and `documents/`) by its path, size and blake3. A pack's header hash covers its
//! table of contents, which covers every block's hash ([`crate::pack`]'s "Integrity"), so one manifest pins
//! every byte a reader uses. Packs stay unsigned and their header's signature block stays reserved: one
//! signature over the manifest covers the whole set, so packs from two releases can't be mixed, and its
//! version, so an old release can't be replayed. Its expiry keeps a host from freezing a client on an old
//! release forever. The patch unit is the pack's 64 KiB stored block ([`crate::pack::BLOCK_SIZE`]), which the
//! packs' own tables name: a manifest holds no block list.
//!
//! Every cook writes an unsigned manifest beside its packs, and a shipping build mounts only packs it names
//! ([`AssetServer::mount_only_released`](crate::AssetServer::mount_only_released)). A release signs that same
//! body: the pipeline's `release` command, with a key from the environment, which no build of the game links.
//!
//! # Keys
//!
//! A manifest's signatures are ed25519, checked with `verify_strict`. A build embeds the keys it trusts, and
//! each manifest names the keys that may sign its successor ([`Manifest::successor_keys`]), so a signed
//! successor rotates them, one hop at a time: [`Manifest::accept_successor`] trusts only what the installed
//! release names. A manifest may carry up to [`MAX_SIGNATURES`] signatures, so a release can be signed by an
//! old key and a new one while clients move from one to the other; a signature by a key the reader doesn't
//! trust is passed over, and one by a key it trusts must verify.
//!
//! # Layout
//!
//! Every integer is little-endian. The body:
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic `SMAN` |
//! | 4 | 4 | format, [`FORMAT_VERSION`] |
//! | 8 | 8 | the release's version, which only grows |
//! | 16 | 8 | its expiry, in seconds since the Unix epoch (UTC) |
//! | 24 | 4 | the oldest app build it runs on |
//! | 28 | 1 | successor keys, at most [`MAX_KEYS`] |
//! | 29 | 1 | bytes of the cook's name, 1 to [`MAX_COOK_LEN`] |
//! | 30 | 2 | packs, at most [`MAX_PACKS`] |
//! | 32 | 2 | files, at most [`MAX_FILES`] |
//! | 34 | 2 | reserved, 0 |
//! | 36 | 32 × keys | successor keys: ed25519 public keys, distinct |
//! | … | name | the cook's name: `desktop` or `mobile` |
//! | … | 41 + name, each | packs, in name order: name length (u8), name, size (u64), header hash (32) |
//! | … | 42 + path, each | files, in path order: flags (u8), path length (u8), path, size (u64), blake3 (32) |
//!
//! Then the signatures, which cover the body's bytes: their count (u8, at most [`MAX_SIGNATURES`]), then for
//! each the signer's public key (32) and the signature (64). An unsigned manifest is its body and a zero.
//! Nothing follows. A file's only flag is bit 0, [`ReleasedFile::patchable`]; the others are 0.
//!
//! A pack's name is 1 to [`MAX_NAME_LEN`] bytes of `a-z`, `0-9`, `_` and `-`. A file's path is 1 to
//! [`MAX_PATH_LEN`] bytes of `/`-separated steps, each of ASCII letters, digits, `.`, `_` and `-` and none
//! starting with `.`, so no entry can name a place outside the install, or a hidden file; no two paths differ
//! only by case, and none ends in `.spack` or is the manifest's own name.
//!
//! # Integrity
//!
//! Reading checks everything above, and a manifest that breaks any of it is an error, never a panic: one
//! flipped byte anywhere is refused, by the signature if not before it. A manifest is at most [`MAX_LEN`] bytes,
//! a pack at most [`MAX_PACK_SIZE`] and a file at most [`MAX_FILE_SIZE`], so what reads one can cap every read
//! by what it says. A manifest of `n` packs and `m` files with one key and one signature is
//! `165 + cook + Σ(41 + name) + Σ(42 + path)` bytes: 1,047 for the game's desktop cook at M4 P1, with
//! twelve packs and five files.

use std::collections::HashSet;
use std::fmt;

use ed25519_dalek::{Signature, VerifyingKey};

use crate::pack::{HEADER_LEN, PACK_EXTENSION};

/// The manifest's file, at the root of an app's content beside the packs.
pub const FILE_NAME: &str = "release.smanifest";
/// The layout this build reads and writes. Readers refuse others.
pub const FORMAT_VERSION: u32 = 1;
/// The most bytes a manifest may be, which also caps a download of one.
pub const MAX_LEN: usize = 1 << 20;
/// The most packs one release may have.
pub const MAX_PACKS: usize = 1024;
/// The most files outside packs one release may have.
pub const MAX_FILES: usize = 4096;
/// The most keys a manifest may name for its successor.
pub const MAX_KEYS: usize = 4;
/// The most signatures a manifest may carry.
pub const MAX_SIGNATURES: usize = 4;
/// The longest a pack's name may be.
pub const MAX_NAME_LEN: usize = 64;
/// The longest a file's path may be.
pub const MAX_PATH_LEN: usize = 255;
/// The longest a cook's name may be.
pub const MAX_COOK_LEN: usize = 16;
/// The largest a pack may be: 16 GiB, far past the game's largest (`shared`, 20 MB on desktop).
pub const MAX_PACK_SIZE: u64 = 16 << 30;
/// The largest a file outside the packs may be: 64 MiB. They're documents and notices, kilobytes each.
pub const MAX_FILE_SIZE: u64 = 64 << 20;

const MAGIC: [u8; 4] = *b"SMAN";
/// Bytes of the body before its keys.
const FIXED_LEN: usize = 36;
/// [`ReleasedFile::patchable`]'s bit.
const FLAG_PATCHABLE: u8 = 1;

/// An ed25519 public key, as a manifest stores it.
pub type PublicKey = [u8; 32];

/// One release of a game's content (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The release's version: a successor's is higher.
    pub version: u64,
    /// When the release stops being offered as current, in seconds since the Unix epoch (UTC). An installed
    /// release keeps running past it; a client won't take it as a successor once it has passed.
    pub expires: u64,
    /// The oldest app build it runs on: a build number `cargo xtask package` counted.
    pub min_app_build: u32,
    /// The cook its packs are: `desktop` or `mobile`, 1 to [`MAX_COOK_LEN`] bytes of `a-z`, `0-9` and `_`.
    pub cook: String,
    /// The keys that may sign the release after this one, distinct. None for a cook's own unsigned manifest,
    /// which nothing succeeds but an app build.
    pub successor_keys: Vec<PublicKey>,
    /// Every pack, in name order.
    pub packs: Vec<ReleasedPack>,
    /// Every file the content ships outside the packs, in path order.
    pub files: Vec<ReleasedFile>,
}

/// A pack, as a release names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleasedPack {
    /// The name it's mounted by: its file is [`crate::pack::file_name`] of it.
    pub name: String,
    /// The whole file's length.
    pub size: u64,
    /// Its header hash: [`crate::pack::PackHeader::hash`].
    pub header_hash: [u8; 32],
}

/// A file outside the packs, as a release names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleasedFile {
    /// Its path from the content's root, `/`-separated.
    pub path: String,
    pub size: u64,
    /// blake3 of its bytes.
    pub hash: [u8; 32],
    /// Whether a patch may change it. What the simulation reads (the flow, the flags, the tuning a replay's
    /// run depends on) changes only with an app build (m4-plan §3 "What a patch may carry"); what's only
    /// presentation (screens, the HUD, notices) may change in a patch.
    pub patchable: bool,
}

/// One signature of a manifest's body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManifestSignature {
    /// The signer's public key.
    pub key: PublicKey,
    pub signature: [u8; 64],
}

/// A manifest that can't be read or isn't to be trusted.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("not a release manifest (no `SMAN` magic)")]
    NotAManifest,
    #[error("manifest format {0} is not supported (this build reads format {FORMAT_VERSION})")]
    UnsupportedFormat(u32),
    #[error("the manifest is {0} bytes, past the {MAX_LEN} a manifest may be")]
    TooLong(usize),
    #[error("the manifest is truncated")]
    Truncated,
    #[error("the manifest's {part} {index} is invalid: {reason}")]
    Invalid { part: &'static str, index: usize, reason: &'static str },
    #[error("the manifest has {0} bytes after its signatures")]
    TrailingBytes(usize),
    #[error("no key this install trusts signed the manifest")]
    Untrusted,
    #[error("the manifest's signature by key {key} doesn't verify")]
    BadSignature { key: KeyId },
    #[error("release {offered} isn't newer than the installed release {installed}")]
    NotNewer { installed: u64, offered: u64 },
    #[error("release {version} expired at {expires}, and it's {now}")]
    Expired { version: u64, expires: u64, now: u64 },
    #[error("release {version} needs app build {needs} or later, and this is build {build}")]
    AppTooOld { version: u64, needs: u32, build: u32 },
    #[error("release {version} is the {offered} cook, and this install is the {installed} cook")]
    OtherCook { version: u64, offered: String, installed: String },
}

/// A public key's first eight bytes in hex, for messages: enough to tell a release's keys apart.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct KeyId([u8; 8]);

impl KeyId {
    pub fn of(key: &PublicKey) -> Self {
        Self(*key.first_chunk().expect("a key is 32 bytes"))
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Manifest {
    /// Reads a manifest without checking who signed it: an install's own copy, which came with the app or
    /// was checked on its way in, and tools. Its signatures are read and must be well formed.
    pub fn read(bytes: &[u8]) -> Result<Self, ManifestError> {
        Ok(parse(bytes)?.manifest)
    }

    /// Reads a manifest and checks it was signed by a key in `trusted`: at least one of its signatures is by
    /// such a key, and every one that is verifies (`verify_strict`). Signatures by other keys are passed over.
    pub fn verify(bytes: &[u8], trusted: &[PublicKey]) -> Result<Self, ManifestError> {
        let parsed = parse(bytes)?;
        let body = &bytes[..parsed.body_len];
        let mut verified = false;
        for signed in parsed.signatures.iter().filter(|signed| trusted.contains(&signed.key)) {
            let bad = || ManifestError::BadSignature { key: KeyId::of(&signed.key) };
            let key = VerifyingKey::from_bytes(&signed.key).map_err(|_| bad())?;
            key.verify_strict(body, &Signature::from_bytes(&signed.signature)).map_err(|_| bad())?;
            verified = true;
        }
        if verified { Ok(parsed.manifest) } else { Err(ManifestError::Untrusted) }
    }

    /// Takes `next` as this release's successor, at `now` (seconds since the Unix epoch, from the caller's
    /// clock) and in app build `app_build`: signed by a key this release names, newer, not expired, for an app
    /// this old, and of the same cook. What a patcher asks of a download before it fetches anything it names.
    pub fn accept_successor(&self, next: &[u8], now: u64, app_build: u32) -> Result<Self, ManifestError> {
        let next = Self::verify(next, &self.successor_keys)?;
        let version = next.version;
        if version <= self.version {
            return Err(ManifestError::NotNewer { installed: self.version, offered: version });
        }
        if next.is_expired(now) {
            return Err(ManifestError::Expired { version, expires: next.expires, now });
        }
        if next.min_app_build > app_build {
            return Err(ManifestError::AppTooOld { version, needs: next.min_app_build, build: app_build });
        }
        if next.cook != self.cook {
            return Err(ManifestError::OtherCook {
                version,
                offered: next.cook,
                installed: self.cook.clone(),
            });
        }
        Ok(next)
    }

    /// Whether the release has expired at `now`, in seconds since the Unix epoch.
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires
    }

    /// The pack called `name`, if the release has it.
    pub fn pack(&self, name: &str) -> Option<&ReleasedPack> {
        self.packs.binary_search_by(|pack| pack.name.as_str().cmp(name)).ok().map(|index| &self.packs[index])
    }

    /// The file at `path`, if the release has it.
    pub fn file(&self, path: &str) -> Option<&ReleasedFile> {
        self.files.binary_search_by(|file| file.path.as_str().cmp(path)).ok().map(|index| &self.files[index])
    }

    /// The bytes a signature covers. Fails if the manifest breaks a rule a reader checks.
    pub fn body(&self) -> Result<Vec<u8>, ManifestError> {
        self.check()?;
        let mut out = Vec::with_capacity(
            FIXED_LEN
                + 32 * self.successor_keys.len()
                + self.cook.len()
                + self.packs.iter().map(|pack| 41 + pack.name.len()).sum::<usize>()
                + self.files.iter().map(|file| 42 + file.path.len()).sum::<usize>(),
        );
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.expires.to_le_bytes());
        out.extend_from_slice(&self.min_app_build.to_le_bytes());
        // Each count is within its cap, checked above.
        out.push(self.successor_keys.len() as u8);
        out.push(self.cook.len() as u8);
        out.extend_from_slice(&(self.packs.len() as u16).to_le_bytes());
        out.extend_from_slice(&(self.files.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0; 2]);
        for key in &self.successor_keys {
            out.extend_from_slice(key);
        }
        out.extend_from_slice(self.cook.as_bytes());
        for pack in &self.packs {
            out.push(pack.name.len() as u8);
            out.extend_from_slice(pack.name.as_bytes());
            out.extend_from_slice(&pack.size.to_le_bytes());
            out.extend_from_slice(&pack.header_hash);
        }
        for file in &self.files {
            out.push(if file.patchable { FLAG_PATCHABLE } else { 0 });
            out.push(file.path.len() as u8);
            out.extend_from_slice(file.path.as_bytes());
            out.extend_from_slice(&file.size.to_le_bytes());
            out.extend_from_slice(&file.hash);
        }
        Ok(out)
    }

    /// The manifest's file: its [body](Self::body) and `signatures`, made over that body. With none, the
    /// unsigned manifest a cook writes.
    pub fn encode(&self, signatures: &[ManifestSignature]) -> Result<Vec<u8>, ManifestError> {
        check_signatures(signatures)?;
        let mut out = self.body()?;
        out.push(signatures.len() as u8);
        for signed in signatures {
            out.extend_from_slice(&signed.key);
            out.extend_from_slice(&signed.signature);
        }
        if out.len() > MAX_LEN {
            return Err(ManifestError::TooLong(out.len()));
        }
        Ok(out)
    }

    /// Checks every rule of the layout a reader checks (see the [module docs](self)).
    fn check(&self) -> Result<(), ManifestError> {
        let invalid = |part, index, reason| ManifestError::Invalid { part, index, reason };
        if self.cook.is_empty()
            || self.cook.len() > MAX_COOK_LEN
            || !self
                .cook
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(invalid("cook", 0, "a cook's name is 1 to 16 bytes of a-z, 0-9 and `_`"));
        }
        if self.successor_keys.len() > MAX_KEYS {
            return Err(invalid("key", MAX_KEYS, "a manifest names at most 4 keys"));
        }
        for (index, key) in self.successor_keys.iter().enumerate() {
            if self.successor_keys[..index].contains(key) {
                return Err(invalid("key", index, "a key repeats"));
            }
            if VerifyingKey::from_bytes(key).is_err() {
                return Err(invalid("key", index, "not an ed25519 public key"));
            }
        }
        if self.packs.len() > MAX_PACKS {
            return Err(invalid("pack", MAX_PACKS, "a release has at most 1024 packs"));
        }
        for (index, pack) in self.packs.iter().enumerate() {
            let fail = |reason| invalid("pack", index, reason);
            if let Some(reason) = name_problem(&pack.name) {
                return Err(fail(reason));
            }
            if index > 0 && self.packs[index - 1].name >= pack.name {
                return Err(fail("packs aren't in name order, or a name repeats"));
            }
            if pack.size < HEADER_LEN as u64 || pack.size > MAX_PACK_SIZE {
                return Err(fail("a pack is shorter than its header or larger than 16 GiB"));
            }
        }
        if self.files.len() > MAX_FILES {
            return Err(invalid("file", MAX_FILES, "a release has at most 4096 files outside its packs"));
        }
        let mut folded = HashSet::with_capacity(self.files.len());
        for (index, file) in self.files.iter().enumerate() {
            let fail = |reason| invalid("file", index, reason);
            if let Some(reason) = path_problem(&file.path) {
                return Err(fail(reason));
            }
            if index > 0 && self.files[index - 1].path >= file.path {
                return Err(fail("files aren't in path order, or a path repeats"));
            }
            if !folded.insert(file.path.to_ascii_lowercase()) {
                return Err(fail("two paths differ only by case"));
            }
            if file.size > MAX_FILE_SIZE {
                return Err(fail("a file is larger than 64 MiB"));
            }
        }
        Ok(())
    }
}

/// What a pack's name breaks, if anything.
fn name_problem(name: &str) -> Option<&'static str> {
    let good = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-');
    if name.is_empty() || name.len() > MAX_NAME_LEN || !name.bytes().all(good) {
        return Some("a pack's name is 1 to 64 bytes of a-z, 0-9, `_` and `-`");
    }
    None
}

/// What a file's path breaks, if anything.
fn path_problem(path: &str) -> Option<&'static str> {
    let good = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-');
    if path.is_empty() || path.len() > MAX_PATH_LEN {
        return Some("a path is 1 to 255 bytes");
    }
    if path.split('/').any(|step| step.is_empty() || step.starts_with('.') || !step.bytes().all(good)) {
        return Some(
            "a path's steps are ASCII letters, digits, `.`, `_` and `-`, none empty or starting with `.`",
        );
    }
    if path == FILE_NAME || path.rsplit_once('.').is_some_and(|(_, extension)| extension == PACK_EXTENSION) {
        return Some("a file outside the packs can't be a pack or the manifest");
    }
    None
}

fn check_signatures(signatures: &[ManifestSignature]) -> Result<(), ManifestError> {
    if signatures.len() > MAX_SIGNATURES {
        return Err(ManifestError::Invalid {
            part: "signature",
            index: MAX_SIGNATURES,
            reason: "a manifest carries at most 4 signatures",
        });
    }
    for (index, signed) in signatures.iter().enumerate() {
        if signatures[..index].iter().any(|earlier| earlier.key == signed.key) {
            return Err(ManifestError::Invalid { part: "signature", index, reason: "a key signed twice" });
        }
    }
    Ok(())
}

/// A manifest read, its signatures not yet checked.
struct Parsed {
    manifest: Manifest,
    /// Bytes of the body, which the signatures cover.
    body_len: usize,
    signatures: Vec<ManifestSignature>,
}

fn parse(bytes: &[u8]) -> Result<Parsed, ManifestError> {
    if bytes.len() > MAX_LEN {
        return Err(ManifestError::TooLong(bytes.len()));
    }
    let mut reader = Reader { bytes, at: 0 };
    if reader.take(4)? != MAGIC {
        return Err(ManifestError::NotAManifest);
    }
    let format = reader.u32()?;
    if format != FORMAT_VERSION {
        return Err(ManifestError::UnsupportedFormat(format));
    }
    let version = reader.u64()?;
    let expires = reader.u64()?;
    let min_app_build = reader.u32()?;
    let key_count = usize::from(reader.u8()?);
    let cook_len = usize::from(reader.u8()?);
    let pack_count = usize::from(reader.u16()?);
    let file_count = usize::from(reader.u16()?);
    let invalid = |part, index, reason| ManifestError::Invalid { part, index, reason };
    if reader.take(2)? != [0; 2] {
        return Err(invalid("header", 0, "reserved bytes aren't zero"));
    }
    // Caps before anything is allocated by a count.
    if key_count > MAX_KEYS || pack_count > MAX_PACKS || file_count > MAX_FILES {
        return Err(invalid("header", 0, "more keys, packs or files than a manifest may have"));
    }
    let successor_keys = (0..key_count).map(|_| reader.array()).collect::<Result<Vec<PublicKey>, _>>()?;
    let cook = reader.text(cook_len, "cook", 0)?;
    let mut packs = Vec::with_capacity(pack_count);
    for index in 0..pack_count {
        let len = usize::from(reader.u8()?);
        let name = reader.text(len, "pack", index)?;
        packs.push(ReleasedPack { name, size: reader.u64()?, header_hash: reader.array()? });
    }
    let mut files = Vec::with_capacity(file_count);
    for index in 0..file_count {
        let flags = reader.u8()?;
        if flags & !FLAG_PATCHABLE != 0 {
            return Err(invalid("file", index, "unknown flags"));
        }
        let len = usize::from(reader.u8()?);
        let path = reader.text(len, "file", index)?;
        files.push(ReleasedFile {
            path,
            size: reader.u64()?,
            hash: reader.array()?,
            patchable: flags & FLAG_PATCHABLE != 0,
        });
    }
    let manifest = Manifest { version, expires, min_app_build, cook, successor_keys, packs, files };
    manifest.check()?;
    let body_len = reader.at;

    let signature_count = usize::from(reader.u8()?);
    if signature_count > MAX_SIGNATURES {
        return Err(invalid("signature", MAX_SIGNATURES, "a manifest carries at most 4 signatures"));
    }
    let signatures = (0..signature_count)
        .map(|_| Ok(ManifestSignature { key: reader.array()?, signature: reader.array()? }))
        .collect::<Result<Vec<_>, ManifestError>>()?;
    check_signatures(&signatures)?;
    let rest = bytes.len() - reader.at;
    if rest != 0 {
        return Err(ManifestError::TrailingBytes(rest));
    }
    Ok(Parsed { manifest, body_len, signatures })
}

/// Reads a manifest front to back, failing at its end rather than past it.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], ManifestError> {
        let end = self.at.checked_add(len).ok_or(ManifestError::Truncated)?;
        let taken = self.bytes.get(self.at..end).ok_or(ManifestError::Truncated)?;
        self.at = end;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ManifestError> {
        Ok(*self.take(N)?.first_chunk().expect("took N bytes"))
    }

    fn u8(&mut self) -> Result<u8, ManifestError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ManifestError> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, ManifestError> {
        self.array().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64, ManifestError> {
        self.array().map(u64::from_le_bytes)
    }

    /// `len` bytes of text, which [`Manifest::check`] holds to their part's rules.
    fn text(&mut self, len: usize, part: &'static str, index: usize) -> Result<String, ManifestError> {
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ManifestError::Invalid {
            part,
            index,
            reason: "not text",
        })
    }
}

#[cfg(test)]
mod tests;
