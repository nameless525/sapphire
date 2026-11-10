//! What the download's tests share: two releases of content shaped like the game's, built as real packs by
//! the packer's writer, signed with keys from fixed seeds and laid out as `xtask release` lays a host out;
//! and a localhost host ([`server`]).

#![allow(dead_code, reason = "each test binary uses part of this")]

pub mod server;

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use sapphire_assets::manifest::{Manifest, ManifestSignature, ReleasedFile, ReleasedPack};
use sapphire_assets::pack::{Codec, HEADER_LEN, PackAsset, PackHeader, Toc, write_pack};
use sapphire_assets::{AssetId, AssetKind};
use sapphire_patch::{
    Installation, Installed, Location, MANIFEST_PATH, Retries, Shipped, Staging, file_path, pack_path,
};

/// Release keys' seeds: the installed release names `TRUSTED`'s key for its successor.
pub const TRUSTED: u8 = 1;
pub const UNTRUSTED: u8 = 2;
/// The clock every test reads, and when its releases expire.
pub const NOW: u64 = 1_900_000_000;
pub const EXPIRES: u64 = 2_000_000_000;
pub const APP_BUILD: u32 = 1300;
const BLOCK: usize = 65_536;

/// A key from a fixed seed: no key is ever made from entropy or kept in a file.
pub fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// Bytes that don't compress, from `seed`: xorshift, so the same everywhere.
pub fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// Bytes that compress: a texture's flat runs, a table's repeats.
fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|at| ((at / 97) as u8).wrapping_add(seed)).collect()
}

fn asset(id: u8, codec: Codec, bytes: &[u8]) -> PackAsset {
    PackAsset::encode(AssetId::from_bytes([id; 16]), AssetKind::Texture, codec, bytes, vec![], 3)
        .expect("encodes")
}

/// A release's content: its packs by name and its files outside them by path, with whether a patch may
/// change each.
#[derive(Clone, Debug)]
pub struct Content {
    pub packs: BTreeMap<String, Vec<u8>>,
    pub files: BTreeMap<String, (Vec<u8>, bool)>,
}

/// The texture: compressible halves and noise, ten blocks; the second release's edit changes blocks 4 and 5,
/// and the third's block 8 too.
fn texture(generation: u8) -> Vec<u8> {
    let mut bytes = pattern(5 * BLOCK, 3);
    bytes.extend(noise(5 * BLOCK, 7));
    if generation >= 1 {
        for byte in &mut bytes[4 * BLOCK + 100..6 * BLOCK - 100] {
            *byte = !*byte;
        }
    }
    if generation >= 2 {
        for byte in &mut bytes[8 * BLOCK + 50..8 * BLOCK + 60] {
            *byte = !*byte;
        }
    }
    bytes
}

/// The release the install has.
pub fn first_release() -> Content {
    content(0)
}

/// Its successor, with an edit of each kind P0 measured: two neighbouring blocks of a texture, a mesh
/// rewritten whole at a new size (which moves every block after it, though not their bytes), a string, a
/// zone's new piece, a screen file outside the packs, and a new pack holding only blocks the install has.
pub fn second_release() -> Content {
    content(1)
}

/// The release after that: another of the texture's blocks, the strings again, the second's new pack gone,
/// and the screen file once more.
pub fn third_release() -> Content {
    content(2)
}

fn content(generation: u8) -> Content {
    let edited = generation >= 1;
    let mesh = if edited { noise(3 * BLOCK + 4100, 22) } else { noise(3 * BLOCK, 21) };
    let strings = match generation {
        0 => "title-tagline = Lanterns\n",
        1 => "title-tagline = A lantern for every door\n",
        _ => "title-tagline = Every door, a lantern\n",
    };
    let strings: Vec<u8> = strings.bytes().cycle().take(3000).collect();
    let sound = noise(2 * BLOCK - 300, 31);
    let mut zone =
        vec![asset(40, Codec::Zstd, &noise(4 * BLOCK, 41)), asset(42, Codec::Lz4, &pattern(9000, 5))];
    if edited {
        zone.push(asset(43, Codec::Zstd, &noise(BLOCK + 10, 43)));
    }
    let mut packs = BTreeMap::from([
        (
            "shared".to_owned(),
            vec![
                asset(10, Codec::Zstd, &texture(generation)),
                asset(11, Codec::Zstd, &mesh),
                asset(12, Codec::Lz4, &pattern(2000, 9)),
            ],
        ),
        ("audio".to_owned(), vec![asset(30, Codec::None, &sound)]),
        ("loc".to_owned(), vec![asset(50, Codec::Zstd, &strings)]),
        ("zone_lantern_ward".to_owned(), zone),
    ]);
    if generation == 1 {
        // The same sound under another ID: a new pack, whose only block the install already holds.
        packs.insert("audio_extra".to_owned(), vec![asset(31, Codec::None, &sound)]);
    }
    let menus: &[u8] = match generation {
        0 => b"(screens: [title, pause])",
        1 => b"(screens: [title, pause, settings])",
        _ => b"(screens: [title, pause, settings, credits])",
    };
    Content {
        packs: packs.into_iter().map(|(name, assets)| (name, write_pack(assets).expect("packs"))).collect(),
        files: BTreeMap::from([
            ("NOTICES.txt".to_owned(), (b"the fonts' licenses".to_vec(), true)),
            ("documents/game.flow.ron".to_owned(), (b"(start: title)".to_vec(), false)),
            ("documents/ui/menus.ron".to_owned(), (menus.to_vec(), true)),
        ]),
    }
}

impl Content {
    pub fn manifest(&self, version: u64, expires: u64) -> Manifest {
        Manifest {
            version,
            expires,
            min_app_build: 1200,
            cook: "desktop".to_owned(),
            successor_keys: vec![key(TRUSTED).verifying_key().to_bytes()],
            packs: self
                .packs
                .iter()
                .map(|(name, bytes)| ReleasedPack {
                    name: name.clone(),
                    size: bytes.len() as u64,
                    header_hash: header(bytes).hash(),
                })
                .collect(),
            files: self
                .files
                .iter()
                .map(|(path, (bytes, patchable))| ReleasedFile {
                    path: path.clone(),
                    size: bytes.len() as u64,
                    hash: *blake3::hash(bytes).as_bytes(),
                    patchable: *patchable,
                })
                .collect(),
        }
    }

    /// Its manifest, signed by `signer`.
    pub fn signed(&self, version: u64, expires: u64, signer: u8) -> Vec<u8> {
        let manifest = self.manifest(version, expires);
        let body = manifest.body().expect("a valid manifest");
        let key = key(signer);
        let signature =
            ManifestSignature { key: key.verifying_key().to_bytes(), signature: key.sign(&body).to_bytes() };
        manifest.encode(&[signature]).expect("encodes")
    }

    /// A static host's files for it: `xtask release`'s layout, with `manifest` as the current release.
    pub fn host(&self, manifest: &[u8]) -> BTreeMap<String, Arc<[u8]>> {
        let mut files: BTreeMap<String, Arc<[u8]>> = BTreeMap::new();
        files.insert(MANIFEST_PATH.to_owned(), manifest.into());
        for bytes in self.packs.values() {
            files.insert(pack_path(&header(bytes).hash()), bytes.as_slice().into());
        }
        for (bytes, _) in self.files.values() {
            files.insert(file_path(blake3::hash(bytes).as_bytes()), bytes.as_slice().into());
        }
        files
    }

    /// It installed as `version`, its packs read from memory.
    pub fn installed(&self, version: u64) -> Installed {
        Installed::read(self.manifest(version, EXPIRES), |pack, offset, len| {
            Ok(self.packs[&pack.name][offset as usize..offset as usize + len].to_vec())
        })
        .expect("installs")
    }

    /// Writes it as an install of `version` under `dir`, as an app ships it: `<name>.spack` and the files
    /// outside the packs beside `release.smanifest`.
    pub fn write_install(&self, dir: &Path, version: u64) {
        fs::create_dir_all(dir).expect("makes the install");
        for (name, bytes) in &self.packs {
            fs::write(dir.join(sapphire_assets::pack::file_name(name)), bytes).expect("writes a pack");
        }
        for (path, (bytes, _)) in &self.files {
            let path = dir.join(path);
            fs::create_dir_all(path.parent().expect("in a directory")).expect("makes its directory");
            fs::write(path, bytes).expect("writes a file");
        }
        fs::write(dir.join(MANIFEST_PATH), self.signed(version, EXPIRES, TRUSTED))
            .expect("writes the manifest");
    }
}

/// Content an app shipped, in a directory as [`Content::write_install`] writes it.
#[derive(Clone, Debug)]
pub struct ShippedDir(pub PathBuf);

impl Shipped for ShippedDir {
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        fs::read(self.0.join(path))
    }

    fn read_at(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut file = fs::File::open(self.0.join(path))?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::with_capacity(len);
        file.take(len as u64).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

/// Stages `to`, signed as `version`, in `staging_dir` as a download over an install of `from` leaves it: its
/// manifest, and the tables, blocks and files [`Expected::between`] says it fetches.
pub fn stage(staging_dir: &Path, from: &Content, to: &Content, version: u64) {
    let staging = Staging::open(staging_dir).expect("opens the staging");
    let expected = Expected::between(from, to);
    let pack_of = |hash: &[u8; 32]| {
        to.packs.values().find(|pack| header(pack).hash() == *hash).expect("one of its packs")
    };
    for (hash, len) in &expected.tables {
        fs::write(staging.table_path(hash), &pack_of(hash)[..*len as usize]).expect("stages tables");
    }
    for (hash, range) in &expected.blocks {
        let bytes = &pack_of(hash)[range.start as usize..range.end as usize];
        fs::write(staging.block_path(blake3::hash(bytes).as_bytes()), bytes).expect("stages a block");
    }
    let held: HashSet<&Vec<u8>> = from.files.values().map(|(bytes, _)| bytes).collect();
    for (bytes, _) in to.files.values().filter(|(bytes, _)| !held.contains(bytes)) {
        fs::write(staging.file_path(blake3::hash(bytes).as_bytes()), bytes).expect("stages a file");
    }
    fs::write(staging.manifest_path(), to.signed(version, EXPIRES, TRUSTED)).expect("stages the manifest");
}

/// Checks `installation` starts on `content` as `version`: every pack and file of it, wherever it's kept,
/// byte for byte; packs the app shipped (under `shipped`) as they are, the rest by their hashes.
pub fn assert_starts_on<S: Shipped>(
    installation: &Installation<S>,
    shipped: &Path,
    content: &Content,
    version: u64,
) {
    assert_eq!(installation.version(), version, "the current release");
    assert_eq!(installation.current(), &content.manifest(version, EXPIRES), "the current manifest");
    let read = |location: Location, shipped_path: &str| match location {
        Location::Shipped => fs::read(shipped.join(shipped_path)),
        Location::Patched(path) => fs::read(path),
    };
    for (name, bytes) in &content.packs {
        let location = installation.pack(name).expect("the release has it");
        let file_name = sapphire_assets::pack::file_name(name);
        assert!(read(location, &file_name).is_ok_and(|read| read == *bytes), "pack {name} of {version}");
    }
    for (path, (bytes, _)) in &content.files {
        let location = installation.file(path).expect("the release has it");
        assert!(read(location, path).is_ok_and(|read| read == *bytes), "file {path} of {version}");
    }
}

/// The files under `dir`, from it, `/`-separated and sorted.
pub fn listing(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut dirs = vec![dir.to_owned()];
    while let Some(next) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&next) else { continue };
        for entry in entries {
            let path = entry.expect("lists").path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let relative = path.strip_prefix(dir).expect("under it");
                names.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    names.sort();
    names
}

/// An install written by [`Content::write_install`], read back.
pub fn read_install(dir: &Path) -> Installed {
    let manifest = Manifest::read(&fs::read(dir.join(MANIFEST_PATH)).expect("reads")).expect("a manifest");
    Installed::read(manifest, |pack, offset, len| {
        let bytes = fs::read(dir.join(sapphire_assets::pack::file_name(&pack.name)))?;
        Ok(bytes[offset as usize..offset as usize + len].to_vec())
    })
    .expect("installs")
}

pub fn header(pack: &[u8]) -> PackHeader {
    PackHeader::read(pack).expect("a pack")
}

pub fn toc(pack: &[u8]) -> Toc {
    let header = header(pack);
    Toc::read(header, &pack[HEADER_LEN..header.data_offset as usize], pack.len() as u64).expect("a table")
}

/// Retries quick enough for a test.
pub fn retries() -> Retries {
    Retries { attempts: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_millis(20) }
}

/// What a patch from `from` to `to` must move, worked out apart from the patcher: the changed packs' tables,
/// and each block whose hash no pack of `from` holds, once.
pub struct Expected {
    /// Each changed pack's header hash and its tables' length.
    pub tables: Vec<([u8; 32], u64)>,
    /// Each new block: its pack's header hash and its range.
    pub blocks: Vec<([u8; 32], std::ops::Range<u64>)>,
    /// The changed files' bytes.
    pub file_bytes: u64,
}

impl Expected {
    pub fn between(from: &Content, to: &Content) -> Self {
        let blocks_of = |pack: &[u8]| {
            let toc = toc(pack);
            toc.entries().iter().flat_map(|entry| toc.blocks(entry).to_vec()).collect::<Vec<_>>()
        };
        let held_packs: HashSet<[u8; 32]> = from.packs.values().map(|pack| header(pack).hash()).collect();
        let mut held_blocks: HashSet<[u8; 32]> =
            from.packs.values().flat_map(|pack| blocks_of(pack)).map(|block| block.hash).collect();
        let mut expected = Self { tables: Vec::new(), blocks: Vec::new(), file_bytes: 0 };
        for pack in to.packs.values().filter(|pack| !held_packs.contains(&header(pack).hash())) {
            let hash = header(pack).hash();
            expected.tables.push((hash, header(pack).data_offset));
            for block in blocks_of(pack) {
                if held_blocks.insert(block.hash) {
                    expected.blocks.push((hash, block.range()));
                }
            }
        }
        let held_files: HashSet<&Vec<u8>> = from.files.values().map(|(bytes, _)| bytes).collect();
        expected.file_bytes = to
            .files
            .values()
            .filter(|(bytes, _)| !held_files.contains(bytes))
            .map(|(bytes, _)| bytes.len() as u64)
            .sum();
        expected
    }

    pub fn block_bytes(&self) -> u64 {
        self.blocks.iter().map(|(_, range)| range.end - range.start).sum()
    }

    pub fn table_bytes(&self) -> u64 {
        self.tables.iter().map(|(_, len)| len).sum()
    }
}

/// Builds each pack of `to` that the install of `from` doesn't hold from the staged tables and blocks and the
/// install's own blocks, as an installer will, and checks it's `to`'s pack byte for byte; checks each changed
/// file too.
pub fn assert_staged_whole(staging_dir: &Path, from: &Content, to: &Content) {
    let installed = from.installed(1);
    let staging = Staging::open(staging_dir).expect("opens the staging");
    let staged =
        Manifest::read(&staging.manifest().expect("reads").expect("a staged manifest")).expect("reads");
    assert_eq!(staged.packs, to.manifest(0, 0).packs);
    for (name, pack) in &to.packs {
        if from.packs.values().any(|held| held == pack) {
            continue;
        }
        let head = header(pack);
        let mut built =
            fs::read(staging.table_path(&head.hash())).unwrap_or_else(|_| panic!("{name}'s tables"));
        let toc = Toc::read(header(&built), &built[HEADER_LEN..], pack.len() as u64).expect("staged tables");
        for entry in toc.entries() {
            for block in toc.blocks(entry) {
                built.resize(block.offset as usize, 0);
                let bytes = match installed.block(&block.hash) {
                    Some(kept) => {
                        from.packs[&kept.pack][kept.range.start as usize..kept.range.end as usize].to_vec()
                    }
                    None => fs::read(staging.block_path(&block.hash)).expect("a staged block"),
                };
                built.extend_from_slice(&bytes);
            }
        }
        built.resize(pack.len(), 0);
        assert!(built == *pack, "{name} builds back from what's kept and staged");
    }
    for (path, (bytes, _)) in &to.files {
        if !from.files.values().any(|(held, _)| held == bytes) {
            let staged = fs::read(staging.file_path(blake3::hash(bytes).as_bytes())).expect("a staged file");
            assert_eq!(&staged, bytes, "{path}");
        }
    }
}

/// The blocks staged under `staging_dir` whose bytes match their names, and the packs whose staged tables do.
pub fn staged_hashes(staging_dir: &Path) -> (HashSet<[u8; 32]>, HashSet<[u8; 32]>) {
    let read = |sub: &str, whole: &dyn Fn(&[u8], &[u8; 32]) -> bool| -> HashSet<[u8; 32]> {
        let Ok(entries) = fs::read_dir(staging_dir.join(sub)) else { return HashSet::new() };
        entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let hash = *blake3::Hash::from_hex(path.file_name()?.to_str()?).ok()?.as_bytes();
                whole(&fs::read(&path).ok()?, &hash).then_some(hash)
            })
            .collect()
    };
    let blocks = read("blocks", &|bytes, hash| blake3::hash(bytes).as_bytes() == hash);
    let tables = read("tables", &|bytes, hash| {
        PackHeader::read(bytes)
            .is_ok_and(|header| header.hash() == *hash && bytes.len() as u64 == header.data_offset)
    });
    (blocks, tables)
}

/// What `served` asked of `content`'s packs: the blocks inside each block request, by hash, and the packs
/// whose tables were asked for, by header hash.
pub fn asked_of(content: &Content, served: &[server::Served]) -> (Vec<[u8; 32]>, Vec<[u8; 32]>) {
    let (mut blocks, mut tables) = (Vec::new(), Vec::new());
    for served in served {
        let Some(range) = &served.asked.range else { continue };
        let Some(pack) =
            content.packs.values().find(|pack| pack_path(&header(pack).hash()) == served.asked.path)
        else {
            continue;
        };
        if is_table_request(range) {
            tables.push(header(pack).hash());
            continue;
        }
        let toc = toc(pack);
        for entry in toc.entries() {
            for block in toc.blocks(entry) {
                if block.offset >= range.start && block.range().end <= range.end {
                    blocks.push(block.hash);
                }
            }
        }
    }
    (blocks, tables)
}

/// Whether a range request is for a pack's tables: its header from 0, or its table of contents from 256.
pub fn is_table_request(range: &std::ops::Range<u64>) -> bool {
    range.start == 0 || range.start == HEADER_LEN as u64
}

/// A directory of the test's own, removed afterward.
pub struct TestDir(pub PathBuf);

impl TestDir {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sapphire_patch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("makes the test's directory");
        Self(path)
    }

    pub fn join(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
