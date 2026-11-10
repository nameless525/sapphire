//! What a download fetches, by hash: the changed packs, their missing blocks, and the changed files.

use std::collections::{HashMap, HashSet};
use std::io;
use std::ops::Range;

use sapphire_assets::manifest::{Manifest, ReleasedFile, ReleasedPack};
use sapphire_assets::pack::{Block, HEADER_LEN, PackError, PackHeader, Toc};

/// An install as a plan sees it: its manifest, and where each block of its packs lies.
#[derive(Debug)]
pub struct Installed {
    manifest: Manifest,
    /// Each block's stored bytes by their hash: the first place they lie.
    blocks: HashMap<[u8; 32], InstalledBlock>,
}

/// Where an installed block's stored bytes lie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledBlock {
    /// The pack's name, as the installed manifest has it.
    pub pack: String,
    /// The block's stored bytes, in that pack's file.
    pub range: Range<u64>,
}

/// An installed pack that can't be read, or isn't the pack its manifest names.
#[derive(Debug, thiserror::Error)]
pub enum InstalledError {
    #[error("installed pack {pack}: {source}")]
    Read {
        pack: String,
        #[source]
        source: io::Error,
    },
    #[error("installed pack {pack}: {source}")]
    Pack {
        pack: String,
        #[source]
        source: PackError,
    },
    #[error("installed pack {pack} isn't the pack its manifest names")]
    NotAsReleased { pack: String },
}

impl Installed {
    /// Reads the tables of every pack `manifest` names, through `read_at(pack, offset, len)`, which reads
    /// `len` bytes of that pack's file from `offset` wherever the install keeps it (a directory, an APK's
    /// assets). Each must be the pack the manifest names, by size and header hash.
    pub fn read(
        manifest: Manifest,
        mut read_at: impl FnMut(&ReleasedPack, u64, usize) -> io::Result<Vec<u8>>,
    ) -> Result<Self, InstalledError> {
        let mut blocks = HashMap::new();
        for pack in &manifest.packs {
            let read_error = |source| InstalledError::Read { pack: pack.name.clone(), source };
            let pack_error = |source| InstalledError::Pack { pack: pack.name.clone(), source };
            let mut read = |offset, len| {
                let bytes = read_at(pack, offset, len).map_err(read_error)?;
                if bytes.len() != len {
                    let short = io::Error::new(io::ErrorKind::UnexpectedEof, "read past the pack's end");
                    return Err(read_error(short));
                }
                Ok(bytes)
            };
            let header = PackHeader::read(&read(0, HEADER_LEN)?).map_err(pack_error)?;
            if header.hash() != pack.header_hash || header.file_len() != pack.size {
                return Err(InstalledError::NotAsReleased { pack: pack.name.clone() });
            }
            let toc_range = header.toc_range();
            let toc_len = usize::try_from(toc_range.end - toc_range.start)
                .map_err(|_| pack_error(PackError::Header("the table of contents is too long to read")))?;
            let toc = Toc::read(header, &read(toc_range.start, toc_len)?, pack.size).map_err(pack_error)?;
            for block in blocks_of(&toc) {
                blocks
                    .entry(block.hash)
                    .or_insert_with(|| InstalledBlock { pack: pack.name.clone(), range: block.range() });
            }
        }
        Ok(Self { manifest, blocks })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Where the install holds a block whose stored bytes hash to `hash`, if it does.
    pub fn block(&self, hash: &[u8; 32]) -> Option<&InstalledBlock> {
        self.blocks.get(hash)
    }
}

/// What a download of a release over an install fetches (see the [crate docs](crate)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    next: Manifest,
    packs: Vec<ReleasedPack>,
    files: Vec<ReleasedFile>,
}

/// A release that a patch can't bring.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error(
        "release {version} changes {path}, which the simulation reads, so only an app build may change it"
    )]
    NotPatchable { version: u64, path: String },
}

impl Plan {
    /// Plans `next` over `installed`. Every file a patch may not change (m4-plan §3 "What a patch may
    /// carry") must be in both releases, the same, and marked so in both: a patch can't add, change, drop
    /// or unmark one ([`Manifest::unpatchable_change`]).
    pub fn new(installed: &Installed, next: Manifest) -> Result<Self, PlanError> {
        let current = &installed.manifest;
        if let Some(path) = current.unpatchable_change(&next) {
            return Err(PlanError::NotPatchable { version: next.version, path: path.to_owned() });
        }
        let held_packs: HashSet<[u8; 32]> = current.packs.iter().map(|pack| pack.header_hash).collect();
        let held_files: HashSet<[u8; 32]> = current.files.iter().map(|file| file.hash).collect();
        let packs =
            next.packs.iter().filter(|pack| !held_packs.contains(&pack.header_hash)).cloned().collect();
        let files = next.files.iter().filter(|file| !held_files.contains(&file.hash)).cloned().collect();
        Ok(Self { next, packs, files })
    }

    /// The release being fetched.
    pub fn manifest(&self) -> &Manifest {
        &self.next
    }

    /// The packs whose tables a download fetches: those the install holds no pack of the same header hash
    /// for, in name order.
    pub fn packs(&self) -> &[ReleasedPack] {
        &self.packs
    }

    /// The files outside the packs a download fetches: those the install holds no file of the same blake3
    /// for, in path order.
    pub fn files(&self) -> &[ReleasedFile] {
        &self.files
    }

    /// The blocks of [`Self::packs`] that the install doesn't hold, by hash, given each one's table of
    /// contents in the same order: once per hash, pack by pack in their order and in file order within one.
    ///
    /// # Panics
    ///
    /// If `tables` isn't one table for each of [`Self::packs`].
    pub fn blocks(&self, installed: &Installed, tables: &[Toc]) -> Vec<PlannedBlock> {
        assert_eq!(tables.len(), self.packs.len(), "one table for each changed pack");
        let mut seen = HashSet::new();
        let mut planned = Vec::new();
        for (pack, toc) in tables.iter().enumerate() {
            for block in blocks_of(toc) {
                if installed.block(&block.hash).is_none() && seen.insert(block.hash) {
                    planned.push(PlannedBlock { hash: block.hash, pack, range: block.range() });
                }
            }
        }
        planned
    }
}

/// A block a download fetches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedBlock {
    /// blake3 of its stored bytes.
    pub hash: [u8; 32],
    /// The pack it's fetched from: an index into [`Plan::packs`].
    pub pack: usize,
    /// Its stored bytes, in that pack's file.
    pub range: Range<u64>,
}

impl PlannedBlock {
    /// Its stored bytes: what it costs to fetch.
    pub fn stored_len(&self) -> u64 {
        self.range.end - self.range.start
    }
}

/// Every block of a pack, in file order: blocks lie in entry order.
fn blocks_of(toc: &Toc) -> impl Iterator<Item = &Block> {
    toc.entries().iter().flat_map(|entry| toc.blocks(entry))
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use sapphire_assets::pack::{Codec, PackAsset, write_pack};
    use sapphire_assets::{AssetId, AssetKind};

    use super::*;

    /// Bytes that don't compress, from `seed`: xorshift.
    fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    fn asset(id: u8, bytes: &[u8]) -> PackAsset {
        PackAsset::encode(AssetId::from_bytes([id; 16]), AssetKind::Texture, Codec::Zstd, bytes, vec![], 3)
            .expect("encodes")
    }

    fn released(name: &str, bytes: &[u8]) -> ReleasedPack {
        let header = PackHeader::read(bytes).expect("a pack");
        ReleasedPack { name: name.to_owned(), size: bytes.len() as u64, header_hash: header.hash() }
    }

    fn file(path: &str, text: &str, patchable: bool) -> ReleasedFile {
        ReleasedFile {
            path: path.to_owned(),
            size: text.len() as u64,
            hash: *blake3::hash(text.as_bytes()).as_bytes(),
            patchable,
        }
    }

    fn manifest(version: u64, packs: &[(&str, &[u8])], files: Vec<ReleasedFile>) -> Manifest {
        Manifest {
            version,
            expires: 2_000_000_000,
            min_app_build: 1,
            cook: "desktop".to_owned(),
            successor_keys: vec![SigningKey::from_bytes(&[1; 32]).verifying_key().to_bytes()],
            packs: packs.iter().map(|(name, bytes)| released(name, bytes)).collect(),
            files,
        }
    }

    fn installed(manifest: Manifest, packs: &[(&str, &[u8])]) -> Result<Installed, InstalledError> {
        Installed::read(manifest, |pack, offset, len| {
            let (_, bytes) = packs.iter().find(|(name, _)| *name == pack.name).expect("a pack it names");
            let start = offset as usize;
            Ok(bytes.get(start..(start + len).min(bytes.len())).unwrap_or_default().to_vec())
        })
    }

    fn table(bytes: &[u8]) -> Toc {
        let header = PackHeader::read(bytes).expect("a pack");
        Toc::read(header, &bytes[HEADER_LEN..header.data_offset as usize], bytes.len() as u64)
            .expect("a table")
    }

    #[test]
    fn a_plan_names_only_what_the_install_lacks_by_hash() {
        let texture = noise(5 * 65_536 + 100, 1);
        let mut edited = texture.clone();
        edited[2 * 65_536 + 10] ^= 0xff;
        let sound = noise(70_000, 2);
        let old_shared = write_pack(vec![asset(1, &texture), asset(2, &sound)]).expect("packs");
        // The texture's third block changes; the sound moves to another pack whole.
        let new_shared = write_pack(vec![asset(1, &edited)]).expect("packs");
        let audio = write_pack(vec![asset(2, &sound)]).expect("packs");
        let fonts = write_pack(vec![asset(3, &noise(1000, 3))]).expect("packs");
        let old = [("fonts", &fonts[..]), ("shared", &old_shared[..])];
        let new = [("audio", &audio[..]), ("fonts", &fonts[..]), ("shared", &new_shared[..])];
        let notices = file("NOTICES.txt", "notices", true);
        let install = installed(
            manifest(1, &old, vec![notices.clone(), file("documents/ui/menus.ron", "menus", true)]),
            &old,
        )
        .expect("installs");
        let next = manifest(2, &new, vec![notices, file("documents/ui/menus.ron", "menus 2", true)]);

        let plan = Plan::new(&install, next).expect("plans");
        assert_eq!(
            plan.packs().iter().map(|pack| pack.name.as_str()).collect::<Vec<_>>(),
            ["audio", "shared"]
        );
        assert_eq!(
            plan.files().iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
            ["documents/ui/menus.ron"]
        );
        let tables = [table(&audio), table(&new_shared)];
        let blocks = plan.blocks(&install, &tables);
        // The sound's two blocks are installed in the old `shared`; of the texture, only the edited block.
        let edited_block = &tables[1].blocks(&tables[1].entries()[0])[2];
        assert_eq!(blocks, [PlannedBlock { hash: edited_block.hash, pack: 1, range: edited_block.range() }]);
        let kept = install.block(&tables[0].blocks(&tables[0].entries()[0])[0].hash).expect("kept");
        assert_eq!(kept.pack, "shared");
    }

    #[test]
    fn a_patch_may_not_touch_what_the_simulation_reads() {
        let pack = write_pack(vec![asset(1, &noise(100, 1))]).expect("packs");
        let packs = [("shared", &pack[..])];
        let flow = file("documents/game.flow.ron", "the flow", false);
        let install = installed(manifest(1, &packs, vec![flow.clone()]), &packs).expect("installs");
        let refused = |files: Vec<ReleasedFile>| Plan::new(&install, manifest(2, &packs, files)).unwrap_err();
        let not_patchable = |path: &str| PlanError::NotPatchable { version: 2, path: path.to_owned() };

        assert_eq!(
            refused(vec![file("documents/game.flow.ron", "a new flow", false)]),
            not_patchable("documents/game.flow.ron")
        );
        assert_eq!(
            refused(vec![file("documents/game.flow.ron", "the flow", true)]),
            not_patchable("documents/game.flow.ron")
        );
        assert_eq!(refused(vec![]), not_patchable("documents/game.flow.ron"));
        assert_eq!(
            refused(vec![file("documents/flags.ron", "flags", false), flow.clone()]),
            not_patchable("documents/flags.ron")
        );
        assert!(Plan::new(&install, manifest(2, &packs, vec![flow])).is_ok());
    }

    #[test]
    fn an_installed_pack_must_be_the_one_its_manifest_names() {
        let pack = write_pack(vec![asset(1, &noise(100, 1))]).expect("packs");
        let other = write_pack(vec![asset(1, &noise(100, 2))]).expect("packs");
        let error = installed(manifest(1, &[("shared", &pack)], vec![]), &[("shared", &other)]).unwrap_err();
        assert!(matches!(error, InstalledError::NotAsReleased { .. }), "{error}");
        let truncated =
            installed(manifest(1, &[("shared", &pack)], vec![]), &[("shared", &pack[..200])]).unwrap_err();
        assert!(matches!(truncated, InstalledError::Read { .. }), "{truncated}");
    }
}
