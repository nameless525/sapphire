//! The content cache: cooked outputs stored under a key over everything that determines them
//! (spec §5.27).
//!
//! # The key
//!
//! A [`CacheKey`] is blake3 over:
//!
//! - the source file's bytes;
//! - the [`Recipe`]: the importer's and cooker's names and versions, and the cooked format's
//!   own version;
//! - the import settings from the sidecar;
//! - the target cook;
//! - every [`Dependency`]: other files the importer reads (a glTF's external buffers), and
//!   other assets whose cooked output the cooker reads, each by its own key.
//!
//! It leaves out what can't change the output: the source's path (so a moved or renamed
//! source is a hit), its asset ID (so a copied source is a hit), and the pipeline binary (so
//! rebuilding the pipeline recooks nothing). That makes one rule binding: **a change to an
//! importer or cooker that changes its output for the same input must bump that
//! [`Stage`]'s version.** Nothing checks keys against the code, and deleting the cache
//! directory is the remedy for a forgotten bump. A cooked format's version is in the key, so
//! bumping one (`Mesh::VERSION`) recooks everything in that format without a stage bump.
//!
//! The packer (M1 A4) caches each asset's encoding for packs under [`CacheKey::packed`], over
//! the cooked bytes' hash, the packer's stage and the codec, and records each pack's hash under
//! [`CacheKey::pack`], over its assets' IDs and packed keys, in the same store.
//!
//! New kinds of asset plug in without changing the key: textures, materials and skinned
//! meshes each brought an [`ImportSettings`] variant and a [`Recipe`], and a material, which
//! reads its textures' cooked output, lists each as a [`Dependency::Asset`] (M1 A3). The key's inputs are
//! encoded with postcard, so it is the same on every machine and build, as a shared cache
//! will need.
//!
//! # The store
//!
//! [`Cache`] is the local store: a directory (`target/content/cache` under xtask) with one
//! file per key. Each entry carries the blake3 hash of its output, so a damaged entry reads
//! as a miss and is cooked again, and an output can be checked against an entry without
//! reading the entry's payload. Writes go through a uniquely named temporary file and a
//! rename, so concurrent cooks (desktop and mobile, whose keys differ) share one directory.
//!
//! # Later milestones
//!
//! - A shared cache (CI and other machines) behind the same key, checked before the local one.
//! - Pruning: entries are never removed today. Deleting the directory, or `cargo clean`, is
//!   always safe.
//! - A stat cache (path, size, modification time → source hash), once content is large
//!   enough that hashing every source on every run shows up.

use std::fmt;
use std::fs;
use std::io::{self, Read as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use sapphire_assets::AssetId;
use sapphire_assets::pack::Codec;
use serde::Serialize;

use crate::cook::{Cook, Recipe};
use crate::meta::ImportSettings;

/// Separates cache keys from every other use of blake3. Changing the key's encoding changes
/// keys anyway, so this only changes if keys ever need to be told apart from an older scheme.
const KEY_CONTEXT: &str = "sapphire_pipeline 2026-10-24 content cache key";
/// Separates the packer's keys from cook keys and from each other.
const PACKED_CONTEXT: &str = "sapphire_pipeline 2026-10-25 packed asset key";
const PACK_CONTEXT: &str = "sapphire_pipeline 2026-10-25 pack key";

/// A pipeline stage as cache keys know it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stage {
    pub name: &'static str,
    /// Bump whenever the stage's output changes for the same input, so outputs cached by
    /// the old code aren't reused.
    pub version: u32,
}

/// Another input a cook reads besides its source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dependency {
    /// A file the importer reads, by the path the source names it with (relative to the
    /// source's directory), and the blake3 hash of its bytes.
    File { path: String, hash: blake3::Hash },
    /// Another asset whose cooked output the cooker reads, by its ID and cache key.
    Asset { id: AssetId, key: CacheKey },
}

/// Everything that determines one cooked output.
#[derive(Clone, Copy, Debug)]
pub struct KeyInputs<'a> {
    /// blake3 of the source file's bytes.
    pub source: blake3::Hash,
    pub recipe: &'a Recipe,
    pub settings: &'a ImportSettings,
    pub cook: Cook,
    /// In any order.
    pub dependencies: &'a [Dependency],
}

/// The key a cooked output is cached under: see the [module docs](self).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheKey([u8; blake3::OUT_LEN]);

impl CacheKey {
    pub fn new(inputs: &KeyInputs<'_>) -> Self {
        /// The key's inputs as postcard encodes them: every field is length-prefixed or
        /// fixed-size, so no two sets of inputs encode the same.
        #[derive(Serialize)]
        struct Encoded<'a> {
            source: [u8; blake3::OUT_LEN],
            importer: (&'a str, u32),
            cooker: (&'a str, u32),
            format: u32,
            settings: &'a ImportSettings,
            cook: &'a str,
            dependencies: Vec<EncodedDependency<'a>>,
        }

        #[derive(Serialize, PartialEq, Eq, PartialOrd, Ord)]
        enum EncodedDependency<'a> {
            File(&'a str, [u8; blake3::OUT_LEN]),
            Asset(AssetId, [u8; blake3::OUT_LEN]),
        }

        let KeyInputs { source, recipe, settings, cook, dependencies } = *inputs;
        let mut dependencies: Vec<_> = dependencies
            .iter()
            .map(|dependency| match dependency {
                Dependency::File { path, hash } => EncodedDependency::File(path, *hash.as_bytes()),
                Dependency::Asset { id, key } => EncodedDependency::Asset(*id, key.0),
            })
            .collect();
        // The cook reads its dependencies as a set.
        dependencies.sort();
        dependencies.dedup();
        let encoded = Encoded {
            source: *source.as_bytes(),
            importer: (recipe.importer.name, recipe.importer.version),
            cooker: (recipe.cooker.name, recipe.cooker.version),
            format: recipe.format,
            settings,
            cook: cook.name(),
            dependencies,
        };
        Self::derive(KEY_CONTEXT, &encoded)
    }

    /// The key a cooked asset's encoding for packs is cached under (M1 A4, see [`crate::pack`]):
    /// the blake3 hash of the cooked bytes it encodes, the packer's stage, and the codec and zstd
    /// level. Keyed by the bytes rather than the cook's key, so an encoding can never outlive the
    /// bytes it was made from; the asset's ID is left out, so identical cooks share one.
    pub fn packed(cooked: &blake3::Hash, packer: Stage, codec: Codec, zstd_level: i32) -> Self {
        Self::derive(PACKED_CONTEXT, &(cooked.as_bytes(), (packer.name, packer.version), codec, zstd_level))
    }

    /// The key a pack's hash is recorded under: the packer's stage, the pack's name, and its
    /// assets by ID and [`Self::packed`] key, in ID order. Equal keys mean equal pack bytes.
    pub fn pack(packer: Stage, name: &str, assets: &[(AssetId, CacheKey)]) -> Self {
        let assets: Vec<(AssetId, [u8; blake3::OUT_LEN])> =
            assets.iter().map(|(id, key)| (*id, key.0)).collect();
        Self::derive(PACK_CONTEXT, &((packer.name, packer.version), name, assets))
    }

    /// blake3 in derive-key mode over `inputs` as postcard encodes them.
    fn derive(context: &str, inputs: &impl Serialize) -> Self {
        let bytes = sapphire_serialize::postcard::to_allocvec(inputs).expect("key inputs always encode");
        Self(*blake3::Hasher::new_derive_key(context).update(&bytes).finalize().as_bytes())
    }
}

impl fmt::Display for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&blake3::Hash::from_bytes(self.0).to_hex())
    }
}

impl fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CacheKey({self})")
    }
}

/// A cache directory that can't be read or written.
#[derive(Debug, thiserror::Error)]
#[error("content cache: failed to access {}", path.display())]
pub struct CacheError {
    pub path: PathBuf,
    #[source]
    pub source: io::Error,
}

/// Marks a cache entry.
const ENTRY_MAGIC: [u8; 4] = *b"SPCE";
/// The entry layout this build reads and writes.
const ENTRY_FORMAT: u32 = 1;
/// Magic, format, payload length (u64) and the payload's blake3 hash, little-endian.
const ENTRY_HEADER_LEN: usize = 4 + 4 + 8 + blake3::OUT_LEN;

/// The local content cache: cooked outputs stored by [`CacheKey`] under a directory, at
/// `<root>/<first two hex digits>/<the other 62>`.
#[derive(Clone, Debug)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where the entry for `key` lives.
    pub fn entry_path(&self, key: &CacheKey) -> PathBuf {
        let hex = key.to_string();
        self.root.join(&hex[..2]).join(&hex[2..])
    }

    /// The blake3 hash of the output cached under `key`, from the entry's header, without
    /// reading the output itself. `None` if there is no usable entry.
    pub fn output_hash(&self, key: &CacheKey) -> Result<Option<blake3::Hash>, CacheError> {
        let path = self.entry_path(key);
        let mut header = [0; ENTRY_HEADER_LEN];
        let read = fs::File::open(&path).and_then(|mut file| file.read_exact(&mut header));
        match read {
            Ok(()) => Ok(parse_header(&header).map(|(_, hash)| hash)),
            Err(error) if is_absent(&error) => Ok(None),
            Err(source) => Err(CacheError { path, source }),
        }
    }

    /// The output cached under `key`, or `None` if there is none or its entry is damaged
    /// (the next [`put`](Self::put) replaces it).
    pub fn get(&self, key: &CacheKey) -> Result<Option<Vec<u8>>, CacheError> {
        let path = self.entry_path(key);
        let mut entry = match fs::read(&path) {
            Ok(entry) => entry,
            Err(error) if is_absent(&error) => return Ok(None),
            Err(source) => return Err(CacheError { path, source }),
        };
        let Some((header, payload)) = entry.split_first_chunk::<ENTRY_HEADER_LEN>() else {
            return Ok(None);
        };
        let Some((len, hash)) = parse_header(header) else {
            return Ok(None);
        };
        if payload.len() as u64 != len || blake3::hash(payload) != hash {
            return Ok(None);
        }
        Ok(Some(entry.split_off(ENTRY_HEADER_LEN)))
    }

    /// Stores `output` under `key`, replacing any entry there.
    pub fn put(&self, key: &CacheKey, output: &[u8]) -> Result<(), CacheError> {
        /// Distinguishes the temporary files of one process's writes.
        static WRITES: AtomicU64 = AtomicU64::new(0);

        let path = self.entry_path(key);
        let error = |source| CacheError { path: path.clone(), source };
        let mut entry = Vec::with_capacity(ENTRY_HEADER_LEN + output.len());
        entry.extend_from_slice(&ENTRY_MAGIC);
        entry.extend_from_slice(&ENTRY_FORMAT.to_le_bytes());
        entry.extend_from_slice(&(output.len() as u64).to_le_bytes());
        entry.extend_from_slice(blake3::hash(output).as_bytes());
        entry.extend_from_slice(output);

        let dir = path.parent().expect("entries live in a fan-out directory");
        fs::create_dir_all(dir).map_err(error)?;
        let write = WRITES.fetch_add(1, Ordering::Relaxed);
        let mut temp = path.as_os_str().to_owned();
        temp.push(format!(".{}-{write}.tmp", std::process::id()));
        let temp = PathBuf::from(temp);
        fs::write(&temp, &entry).map_err(error)?;
        fs::rename(&temp, &path).map_err(|source| {
            let _ = fs::remove_file(&temp);
            error(source)
        })
    }
}

/// An entry header's payload length and hash, or `None` if it isn't one this build wrote.
fn parse_header(header: &[u8; ENTRY_HEADER_LEN]) -> Option<(u64, blake3::Hash)> {
    let (magic, rest) = header.split_first_chunk::<4>()?;
    let (format, rest) = rest.split_first_chunk::<4>()?;
    let (len, hash) = rest.split_first_chunk::<8>()?;
    let hash: [u8; blake3::OUT_LEN] = hash.try_into().ok()?;
    (*magic == ENTRY_MAGIC && u32::from_le_bytes(*format) == ENTRY_FORMAT)
        .then(|| (u64::from_le_bytes(*len), blake3::Hash::from_bytes(hash)))
}

/// A missing or truncated entry: a miss rather than an error.
fn is_absent(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::UnexpectedEof)
}

#[cfg(test)]
mod tests {
    use sapphire_assets::mesh::PositionFormat;

    use super::*;
    use crate::bake::OutlineSettings;
    use crate::meta::MeshSettings;
    use crate::optimize::{LodLevel, LodSettings};
    use crate::test_util::TempDir;

    const RECIPE: Recipe = Recipe {
        importer: Stage { name: "gltf", version: 1 },
        cooker: Stage { name: "smesh", version: 1 },
        format: 1,
        extension: "smesh",
    };

    fn settings(weld_tolerance: f32) -> ImportSettings {
        let outline = OutlineSettings::with_weld_tolerance(weld_tolerance).unwrap();
        ImportSettings::Gltf(MeshSettings { outline, ..MeshSettings::default() })
    }

    fn file(path: &str, contents: &[u8]) -> Dependency {
        Dependency::File { path: path.to_owned(), hash: blake3::hash(contents) }
    }

    #[test]
    fn keys_change_with_every_input() {
        let other_asset = Dependency::Asset { id: AssetId::from_bytes([1; 16]), key: CacheKey([2; 32]) };
        let dependencies = [file("mesh.bin", b"buffer"), other_asset.clone()];
        let (default_settings, coarse_settings) = (settings(1e-4), settings(1e-2));
        let base = KeyInputs {
            source: blake3::hash(b"source"),
            recipe: &RECIPE,
            settings: &default_settings,
            cook: Cook::Desktop,
            dependencies: &dependencies,
        };
        let key = CacheKey::new(&base);
        assert_eq!(CacheKey::new(&base), key, "deterministic");
        let reordered = [other_asset.clone(), file("mesh.bin", b"buffer"), file("mesh.bin", b"buffer")];
        assert_eq!(CacheKey::new(&KeyInputs { dependencies: &reordered, ..base }), key, "a set");

        let recipe = |edit: fn(&mut Recipe)| {
            let mut recipe = RECIPE;
            edit(&mut recipe);
            recipe
        };
        let recipes = [
            recipe(|r| r.importer.version = 2),
            recipe(|r| r.importer.name = "fbx"),
            recipe(|r| r.cooker.version = 2),
            recipe(|r| r.cooker.name = "smesh2"),
            recipe(|r| r.format = 2),
        ];
        let dependency_sets = [
            vec![file("mesh.bin", b"edited"), other_asset.clone()],
            vec![file("other.bin", b"buffer"), other_asset.clone()],
            vec![file("mesh.bin", b"buffer")],
            vec![
                file("mesh.bin", b"buffer"),
                Dependency::Asset { id: AssetId::from_bytes([1; 16]), key: CacheKey([3; 32]) },
            ],
            vec![
                file("mesh.bin", b"buffer"),
                Dependency::Asset { id: AssetId::from_bytes([4; 16]), key: CacheKey([2; 32]) },
            ],
        ];
        // Every mesh setting: the bake's weld, the LOD chain (each field of a level, the level
        // count, the mobile skip and the border lock), the position format and the material
        // bindings.
        fn mesh_settings(edit: impl FnOnce(&mut MeshSettings)) -> ImportSettings {
            let mut mesh = MeshSettings::default();
            edit(&mut mesh);
            ImportSettings::Gltf(mesh)
        }
        let level = |triangle_ratio, max_error| LodLevel { triangle_ratio, max_error };
        let mesh_variants = [
            mesh_settings(|m| m.lods = LodSettings::none()),
            mesh_settings(|m| m.lods = LodSettings::new(vec![level(0.5, 0.002)], 1).unwrap()),
            mesh_settings(|m| m.lods = LodSettings::new(vec![level(0.4, 0.002)], 0).unwrap()),
            mesh_settings(|m| m.lods = LodSettings::new(vec![level(0.5, 0.003)], 0).unwrap()),
            mesh_settings(|m| m.lods = LodSettings::new(vec![level(0.5, 0.002)], 0).unwrap()),
            mesh_settings(|m| m.lods = LodSettings::default().lock_border(true)),
            mesh_settings(|m| m.position_format = PositionFormat::Float32),
            mesh_settings(|m| {
                m.materials.insert("Skin".into(), AssetId::from_bytes([5; 16]));
            }),
        ];
        let mut variants = vec![
            KeyInputs { source: blake3::hash(b"edited source"), ..base },
            KeyInputs { settings: &coarse_settings, ..base },
            KeyInputs { cook: Cook::Mobile, ..base },
        ];
        variants.extend(mesh_variants.iter().map(|settings| KeyInputs { settings, ..base }));
        variants.extend(recipes.iter().map(|recipe| KeyInputs { recipe, ..base }));
        variants.extend(dependency_sets.iter().map(|dependencies| KeyInputs { dependencies, ..base }));

        let mut keys: Vec<CacheKey> = variants.iter().map(CacheKey::new).collect();
        keys.push(key);
        let count = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), count, "every input changes the key");
    }

    #[test]
    fn packer_keys_change_with_every_input() {
        let (cooked, packer) = (blake3::hash(b"cooked"), Stage { name: "spack", version: 1 });
        let key = CacheKey::packed(&cooked, packer, Codec::Zstd, 19);
        assert_eq!(CacheKey::packed(&cooked, packer, Codec::Zstd, 19), key, "deterministic");
        let mut keys = vec![
            key,
            CacheKey::packed(&blake3::hash(b"recooked"), packer, Codec::Zstd, 19),
            CacheKey::packed(&cooked, Stage { version: 2, ..packer }, Codec::Zstd, 19),
            CacheKey::packed(&cooked, Stage { name: "spack2", ..packer }, Codec::Zstd, 19),
            CacheKey::packed(&cooked, packer, Codec::Lz4, 19),
            CacheKey::packed(&cooked, packer, Codec::Zstd, 3),
        ];
        let (a, b) = (AssetId::from_bytes([1; 16]), AssetId::from_bytes([2; 16]));
        let other = CacheKey([9; 32]);
        let pack = CacheKey::pack(packer, "shared", &[(a, key), (b, other)]);
        assert_eq!(CacheKey::pack(packer, "shared", &[(a, key), (b, other)]), pack);
        keys.extend([
            pack,
            CacheKey::pack(packer, "zone", &[(a, key), (b, other)]),
            CacheKey::pack(packer, "shared", &[(a, key)]),
            CacheKey::pack(packer, "shared", &[(a, key), (b, key)]),
            CacheKey::pack(packer, "shared", &[(a, key), (AssetId::from_bytes([3; 16]), other)]),
            CacheKey::pack(Stage { version: 2, ..packer }, "shared", &[(a, key), (b, other)]),
        ]);
        let count = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), count, "every input changes the key");
    }

    #[test]
    fn keys_are_the_same_on_every_machine() {
        let default_settings = settings(1e-4);
        let key = CacheKey::new(&KeyInputs {
            source: blake3::hash(b"source"),
            recipe: &RECIPE,
            settings: &default_settings,
            cook: Cook::Mobile,
            dependencies: &[file("mesh.bin", b"buffer")],
        });
        // Recorded once. A change here means every cache, local or shared, misses.
        assert_eq!(key.to_string(), "7d8afd3e19f7c21d075598cdf6f14f268df6352f9ac4ae6a861fb33d9b80c3fe");
    }

    #[test]
    fn entries_round_trip_and_damaged_ones_miss() {
        let dir = TempDir::new("cache_entries");
        let cache = Cache::new(dir.path().join("cache"));
        let (key, other) = (CacheKey([0xab; 32]), CacheKey([0xcd; 32]));
        assert_eq!(cache.get(&key).unwrap(), None);
        assert_eq!(cache.output_hash(&key).unwrap(), None);

        cache.put(&key, b"cooked bytes").unwrap();
        assert_eq!(cache.get(&key).unwrap().as_deref(), Some(&b"cooked bytes"[..]));
        assert_eq!(cache.output_hash(&key).unwrap(), Some(blake3::hash(b"cooked bytes")));
        assert_eq!(cache.get(&other).unwrap(), None);
        let path = cache.entry_path(&key);
        assert!(path.starts_with(dir.path().join("cache/ab")));
        let leftovers = fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary files are left behind");

        // A flipped payload byte or a truncated entry is a miss, and `put` repairs it.
        let mut entry = fs::read(&path).unwrap();
        *entry.last_mut().unwrap() ^= 1;
        fs::write(&path, &entry).unwrap();
        assert_eq!(cache.get(&key).unwrap(), None);
        fs::write(&path, &entry[..20]).unwrap();
        assert_eq!((cache.get(&key).unwrap(), cache.output_hash(&key).unwrap()), (None, None));
        fs::write(&path, b"not an entry at all, but long enough to hold a header").unwrap();
        assert_eq!((cache.get(&key).unwrap(), cache.output_hash(&key).unwrap()), (None, None));
        cache.put(&key, b"cooked again").unwrap();
        assert_eq!(cache.get(&key).unwrap().as_deref(), Some(&b"cooked again"[..]));
    }
}
