//! What a patch moves (M4 S7): a release published with `--patch-from` counts, before anything is written,
//! the bytes an install of the release it patches would fetch, by the kind of asset each new block is of, and
//! says which machine cooked each release.
//!
//! The count is `sapphire_patch`'s plan, read from the host directory: the new manifest; the header and table
//! of each pack the earlier release holds no pack of the same header hash for; each block of those packs whose
//! stored bytes' hash no pack of the earlier release holds, once per hash; and each file outside the packs whose
//! blake3 is new. A download also moves the alignment padding between neighbouring blocks it fetches in one
//! request, at most 15 bytes a block, which isn't counted. The earlier release's packs are read where
//! publishing laid them out, `packs/` beside its manifest's `manifests/` folder; one that isn't there counts
//! all its blocks as new, and is named.
//!
//! **Why:** one edit moves kilobytes (P4's tagline: 4,384 bytes), but a cook from another architecture
//! differs in its textures' and many meshes' bytes (C3, Z1), so a release cooked on the wrong machine would
//! ship far more than its edit, silently. [`PatchSize::warnings`] names that: more than [`WARN_SHARE`] of
//! the cook moving, or the two releases cooked on different architectures. Each release records the
//! architecture it was cooked on in `manifests/<version>.arch` ([`ARCH_EXTENSION`]); a release published before
//! M4 S7 has no record, which is said, not warned of.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::Path;

use sapphire_assets::manifest::Manifest;
use sapphire_assets::pack::{HEADER_LEN, PackHeader, Toc};

use super::{ReleaseError, hex};

/// A patch moving more than this share of its cook is warned of. An edit moves a few kilobytes of a cook of
/// tens of megabytes; a cook from another architecture moves most of its textures.
pub const WARN_SHARE: f64 = 0.05;

/// The extension of a release's architecture record beside its manifest, `manifests/<version>.arch`: the
/// architecture of the machine that cooked and published it (`std::env::consts::ARCH`), as one line. Every
/// path through xtask cooks and publishes on one machine, so the publisher's is the cook's. Clients never
/// read it.
pub const ARCH_EXTENSION: &str = "arch";

/// The architecture this pipeline runs on, which a release it publishes records.
#[must_use]
pub fn this_architecture() -> &'static str {
    std::env::consts::ARCH
}

/// What a patch moves over the release it patches (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchSize {
    /// The release it patches.
    pub from: u64,
    /// The patch's own version.
    pub version: u64,
    /// Every byte of the cook the patch releases: its packs and its files outside them.
    pub cook_bytes: u64,
    /// The signed manifest, which every check fetches.
    pub manifest: u64,
    /// The changed packs' headers and tables.
    pub tables: u64,
    /// How many packs changed.
    pub changed_packs: usize,
    /// New blocks' stored bytes by the kind of the asset they're of, and every block's of that kind in the cook.
    pub kinds: BTreeMap<&'static str, KindBytes>,
    /// The changed files outside the packs, whole.
    pub files: u64,
    /// The architecture the earlier release records, if it records one.
    pub from_architecture: Option<String>,
    /// The architecture this release is cooked on.
    pub architecture: String,
    /// The earlier release's packs that weren't beside its manifest, whose blocks all counted as new.
    pub missing: Vec<String>,
}

/// One kind's bytes: new in the patch, and in the whole cook.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KindBytes {
    pub new: u64,
    pub cook: u64,
}

impl PatchSize {
    /// Counts what `release`, the cook in `packs` signed as `signed_len` bytes, moves over `from`, the release
    /// whose manifest is at `from_path`.
    pub(super) fn measure(
        from_path: &Path,
        from: &Manifest,
        packs: &Path,
        release: &Manifest,
        signed_len: u64,
    ) -> Result<Self, ReleaseError> {
        let host = from_path.parent().and_then(Path::parent).unwrap_or(Path::new("."));
        let mut held_blocks = HashSet::new();
        let mut missing = Vec::new();
        for pack in &from.packs {
            let path = host.join("packs").join(format!(
                "{}.{}",
                hex(&pack.header_hash),
                sapphire_assets::pack::PACK_EXTENSION
            ));
            if !path.is_file() {
                missing.push(pack.name.clone());
                continue;
            }
            let toc = read_toc(&path)?;
            held_blocks
                .extend(toc.entries().iter().flat_map(|entry| toc.blocks(entry)).map(|block| block.hash));
        }
        let held_packs: HashSet<[u8; 32]> = from.packs.iter().map(|pack| pack.header_hash).collect();
        let held_files: HashSet<[u8; 32]> = from.files.iter().map(|file| file.hash).collect();

        let mut size = Self {
            from: from.version,
            version: release.version,
            cook_bytes: release
                .packs
                .iter()
                .map(|pack| pack.size)
                .chain(release.files.iter().map(|file| file.size))
                .sum(),
            manifest: signed_len,
            tables: 0,
            changed_packs: 0,
            kinds: BTreeMap::new(),
            files: release
                .files
                .iter()
                .filter(|file| !held_files.contains(&file.hash))
                .map(|file| file.size)
                .sum(),
            from_architecture: read_architecture(&from_path.with_extension(ARCH_EXTENSION))?,
            architecture: this_architecture().to_owned(),
            missing,
        };
        let mut seen = HashSet::new();
        for pack in &release.packs {
            let toc = read_toc(&packs.join(sapphire_assets::pack::file_name(&pack.name)))?;
            let changed = !held_packs.contains(&pack.header_hash);
            if changed {
                size.changed_packs += 1;
                size.tables += toc.header().toc_range().end;
            }
            for entry in toc.entries() {
                let kind = size.kinds.entry(entry.kind.name()).or_default();
                for block in toc.blocks(entry) {
                    let stored = u64::from(block.stored_len);
                    kind.cook += stored;
                    if changed && !held_blocks.contains(&block.hash) && seen.insert(block.hash) {
                        kind.new += stored;
                    }
                }
            }
        }
        Ok(size)
    }

    /// Every byte the patch moves (see the [module docs](self) for what isn't counted).
    #[must_use]
    pub fn total(&self) -> u64 {
        self.manifest + self.tables + self.files + self.kinds.values().map(|kind| kind.new).sum::<u64>()
    }

    /// `bytes` as a share of the cook.
    fn share(&self, bytes: u64) -> f64 {
        if self.cook_bytes == 0 { 0.0 } else { bytes as f64 / self.cook_bytes as f64 }
    }

    /// What's wrong with the patch's size, if anything: more than [`WARN_SHARE`] of the cook moving, the two
    /// releases cooked on different architectures, or the earlier release's packs missing from the host.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        let share = self.share(self.total());
        if share > WARN_SHARE {
            warnings.push(format!(
                "release {} moves {:.1}% of its cook over release {} ({} of {} bytes), more than the {:.0}% an \
                 edit should: was it cooked where release {} was?",
                self.version,
                share * 100.0,
                self.from,
                grouped(self.total()),
                grouped(self.cook_bytes),
                WARN_SHARE * 100.0,
                self.from
            ));
        }
        if let Some(from) = &self.from_architecture
            && *from != self.architecture
        {
            warnings.push(format!(
                "release {} was cooked on {from} and release {} on {}: cooks differ across architectures, so the \
                 patch carries far more than its edit. Cook releases on one machine type",
                self.from, self.version, self.architecture
            ));
        }
        if !self.missing.is_empty() {
            warnings.push(format!(
                "release {}'s packs {} aren't in the host beside its manifest, so all their blocks counted as new",
                self.from,
                self.missing.join(", ")
            ));
        }
        warnings
    }
}

impl fmt::Display for PatchSize {
    /// The summary `release --patch-from` prints: the whole, then the manifest, tables and files, then each
    /// kind with new bytes, most first, with its share of the cook.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let percent = |bytes: u64| self.share(bytes) * 100.0;
        writeln!(
            f,
            "release {} patches release {}: {} bytes to fetch, {:.2}% of the cook's {}",
            self.version,
            self.from,
            grouped(self.total()),
            percent(self.total()),
            grouped(self.cook_bytes)
        )?;
        writeln!(f, "  {:<16}{:>14}", "manifest", grouped(self.manifest))?;
        writeln!(
            f,
            "  {:<16}{:>14}  ({} changed pack{})",
            "tables",
            grouped(self.tables),
            self.changed_packs,
            if self.changed_packs == 1 { "" } else { "s" }
        )?;
        writeln!(f, "  {:<16}{:>14}", "files", grouped(self.files))?;
        let mut kinds: Vec<_> = self.kinds.iter().filter(|(_, kind)| kind.new > 0).collect();
        kinds.sort_by(|a, b| b.1.new.cmp(&a.1.new).then(a.0.cmp(b.0)));
        for (name, kind) in kinds {
            writeln!(
                f,
                "  {name:<16}{:>14}  {:>6.2}% of the cook, {:.1}% of its {name} bytes",
                grouped(kind.new),
                percent(kind.new),
                if kind.cook == 0 { 0.0 } else { kind.new as f64 / kind.cook as f64 * 100.0 }
            )?;
        }
        match &self.from_architecture {
            Some(from) => writeln!(f, "  cooked on {} (release {}: {from})", self.architecture, self.from),
            None => writeln!(
                f,
                "  cooked on {}; release {} records no architecture (published before M4 S7)",
                self.architecture, self.from
            ),
        }
    }
}

/// `bytes` with commas between thousands.
fn grouped(bytes: u64) -> String {
    let digits = bytes.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// The architecture record at `path`, if there's one.
pub(super) fn read_architecture(path: &Path) -> Result<Option<String>, ReleaseError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.trim().to_owned())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ReleaseError::Io { path: path.to_owned(), source }),
    }
}

/// A pack's checked table of contents, read without its data.
fn read_toc(path: &Path) -> Result<Toc, ReleaseError> {
    let io_error = |source| ReleaseError::Io { path: path.to_owned(), source };
    let pack_error = |source| ReleaseError::Pack { path: path.to_owned(), source };
    let mut file = fs::File::open(path).map_err(io_error)?;
    let file_len = file.metadata().map_err(io_error)?.len();
    let mut header = vec![0; HEADER_LEN];
    file.read_exact(&mut header).map_err(io_error)?;
    let header = PackHeader::read(&header).map_err(pack_error)?;
    let range = header.toc_range();
    let mut toc = vec![0; usize::try_from(range.end - range.start).unwrap_or(usize::MAX)];
    file.seek(SeekFrom::Start(range.start)).map_err(io_error)?;
    file.read_exact(&mut toc).map_err(io_error)?;
    Toc::read(header, &toc, file_len).map_err(pack_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size() -> PatchSize {
        let mut kinds = BTreeMap::new();
        kinds.insert("texture", KindBytes { new: 1_545, cook: 30_000_000 });
        kinds.insert("mesh", KindBytes { new: 0, cook: 8_000_000 });
        PatchSize {
            from: 1,
            version: 2,
            cook_bytes: 38_736_717,
            manifest: 1_047,
            tables: 1_792,
            changed_packs: 1,
            kinds,
            files: 0,
            from_architecture: Some("aarch64".to_owned()),
            architecture: "aarch64".to_owned(),
            missing: Vec::new(),
        }
    }

    /// P4's tagline edit, as the summary says it: the whole, its parts and each kind with new bytes.
    #[test]
    fn the_summary_says_each_kinds_new_bytes_and_their_share_of_the_cook() {
        let size = size();
        assert_eq!(size.total(), 4_384);
        let text = size.to_string();
        assert!(
            text.starts_with(
                "release 2 patches release 1: 4,384 bytes to fetch, 0.01% of the cook's 38,736,717\n"
            ),
            "{text}"
        );
        assert!(text.contains("\n  tables                   1,792  (1 changed pack)\n"), "{text}");
        assert!(
            text.contains(
                "\n  texture                  1,545    0.00% of the cook, 0.0% of its texture bytes\n"
            ),
            "{text}"
        );
        assert!(!text.contains("mesh"), "a kind with nothing new isn't listed: {text}");
        assert!(text.ends_with("  cooked on aarch64 (release 1: aarch64)\n"), "{text}");
        assert!(size.warnings().is_empty(), "{:?}", size.warnings());
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(38_740_637), "38,740,637");
    }

    #[test]
    fn a_patch_over_its_share_or_from_another_architecture_is_warned_of() {
        let mut size = size();
        size.kinds.get_mut("mesh").unwrap().new = 2_000_000;
        let warnings = size.warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with(
                "release 2 moves 5.2% of its cook over release 1 (2,004,384 of 38,736,717 bytes)"
            ),
            "{warnings:?}"
        );
        let text = size.to_string();
        let mesh = text.find("  mesh").unwrap();
        assert!(mesh < text.find("  texture").unwrap(), "most bytes first: {text}");

        let mut size = self::size();
        size.from_architecture = Some("x86_64".to_owned());
        let warnings = size.warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("release 1 was cooked on x86_64 and release 2 on aarch64"),
            "{warnings:?}"
        );

        // No record is said, not warned of.
        size.from_architecture = None;
        assert!(size.warnings().is_empty());
        assert!(size.to_string().contains("release 1 records no architecture"));

        size.missing = vec!["shared".to_owned()];
        assert!(size.warnings()[0].contains("packs shared aren't in the host"));
    }
}
