//! Cooking: source assets and their sidecars in, the runtime's cooked files out, through the
//! content cache (spec §5.27).
//!
//! [`cook_dir`] cooks a content tree:
//!
//! 1. Finds the source assets and resolves their sidecars ([`meta`]), creating missing
//!    ones.
//! 2. Plans each asset's output: its path under the content root, mirrored under the output
//!    directory with the cooked format's extension (`samples/mannequin.glb` →
//!    `samples/mannequin.smesh`). The runtime still finds cooked files by these paths; it
//!    addresses assets by ID once packs land (M1 A4).
//! 3. Removes stale files from the output directory: cooked files no source produces any
//!    more, and temporary files from an interrupted run. Other files are left alone.
//! 4. Keys each asset ([`CacheKey`]) and takes its output from the [`Cache`], or imports,
//!    bakes, validates and cooks it and stores the result. An output is written only when its
//!    bytes change, so a file watcher (hot reload, A8) sees exactly the assets that changed.
//!
//! Each kind of source cooks through a [`Recipe`]: the importer, the cooker and the cooked
//! format, whose versions are part of the cache key:
//!
//! - **Meshes** cook glTF → `.smesh` (A1, A5): import, outline bake,
//!   [optimization and LODs](crate::optimize), packing, then a read-back of the cooked bytes
//!   through the runtime's own checks. The two cooks differ in the LOD chain: the mobile cook
//!   drops its first levels, for a lower-detail LOD0 and fewer LODs (spec §5.27).
//! - **Textures** cook PNG, TGA and EXR → `.ktx2` (A2), BCn on desktop and ASTC on mobile
//!   ([`texture`]).
//!
//! Assets cook one after another. Cooking in parallel (rayon) waits for content that makes it
//! worth it.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sapphire_assets::AssetId;
use sapphire_assets::mesh::{MESH_EXTENSION, Mesh, MeshError};
use sapphire_assets::texture::{TEXTURE_EXTENSION, TEXTURE_VERSION};
use sapphire_reflect::Versioned;

use crate::cache::{Cache, CacheError, CacheKey, Dependency, KeyInputs, Stage};
use crate::import::gltf::{self, GltfImportError};
use crate::import::image;
use crate::meta::{self, ImportSettings, MeshSettings, MetaError, SourceAsset, SourceTree};
use crate::texture::{self, TextureError};
use crate::{bake, optimize};

/// A target cook (spec §2): `desktop` for macOS, Windows and Linux, `mobile` for iOS and
/// Android.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cook {
    Desktop,
    Mobile,
}

impl Cook {
    /// Every cook, in a stable order.
    pub const ALL: [Self; 2] = [Self::Desktop, Self::Mobile];

    /// The name used on command lines, in output directories and in cache keys.
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

/// Returned when parsing an unknown cook name.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown cook {0:?}; expected `desktop` or `mobile`")]
pub struct UnknownCook(pub String);

impl FromStr for Cook {
    type Err = UnknownCook;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|cook| cook.name() == s).ok_or_else(|| UnknownCook(s.to_owned()))
    }
}

/// How one kind of source is cooked: the stages that run, and the file they write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recipe {
    pub importer: Stage,
    pub cooker: Stage,
    /// The cooked format's own version, so a format change recooks its assets even without a
    /// stage bump.
    pub format: u32,
    /// The cooked file's extension.
    pub extension: &'static str,
}

/// Bake, optimization and LODs, packing and the `.smesh` writer. Bump its version whenever
/// they change output for the same imported mesh.
pub const MESH_COOKER: Stage = Stage { name: "smesh", version: 2 };

/// Every extension a recipe writes: stale files with these are removed from the output.
const COOKED_EXTENSIONS: &[&str] = &[MESH_EXTENSION, TEXTURE_EXTENSION];

/// Extension of the temporary files outputs are written through.
const TEMP_EXTENSION: &str = "tmp";

impl Recipe {
    /// The recipe for sources read with `settings`.
    pub fn for_settings(settings: &ImportSettings) -> Self {
        match settings {
            ImportSettings::Gltf(_) => Self {
                importer: gltf::IMPORTER,
                cooker: MESH_COOKER,
                format: Mesh::VERSION,
                extension: MESH_EXTENSION,
            },
            ImportSettings::Texture(_) => Self {
                importer: image::IMPORTER,
                cooker: texture::COOKER,
                format: TEXTURE_VERSION,
                extension: TEXTURE_EXTENSION,
            },
        }
    }
}

/// Errors from cooking. Each names the file it concerns.
#[derive(Debug, thiserror::Error)]
pub enum CookError {
    #[error("{} is not a supported mesh source (expected .gltf or .glb)", path.display())]
    UnsupportedSource { path: PathBuf },
    #[error(transparent)]
    Meta(#[from] MetaError),
    #[error("failed to import {}", path.display())]
    Import {
        path: PathBuf,
        #[source]
        source: GltfImportError,
    },
    #[error("failed to cook texture {}", path.display())]
    Texture {
        path: PathBuf,
        /// Boxed: decoder and encoder errors are large, and every cook result carries this type.
        #[source]
        source: Box<TextureError>,
    },
    #[error("cooked mesh from {} is invalid", path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: MeshError,
    },
    #[error("{} and {} would both cook to {}", first.display(), second.display(), output.display())]
    OutputCollision { first: PathBuf, second: PathBuf, output: PathBuf },
    #[error("failed to read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Cache(#[from] CacheError),
}

/// Options for [`cook_dir`].
#[derive(Clone, Copy, Debug)]
pub struct CookOptions<'a> {
    pub cook: Cook,
    /// The content cache's directory. Without one, every asset is cooked.
    pub cache: Option<&'a Path>,
    /// Fail on a source without a sidecar instead of creating one (CI; see [`meta`]).
    pub locked: bool,
}

/// What a cook did with one asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CookStatus {
    /// Cooked from its source: a cache miss, or no cache.
    Cooked,
    /// Taken from the cache and written to its output.
    Cached,
    /// The output already held the cached bytes, so nothing was written.
    UpToDate,
}

/// One asset of a [`CookReport`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookedAsset {
    pub id: AssetId,
    pub source: PathBuf,
    pub output: PathBuf,
    pub key: CacheKey,
    pub status: CookStatus,
}

/// What [`cook_dir`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CookReport {
    /// Every asset, in source path order.
    pub assets: Vec<CookedAsset>,
    /// Sidecars this run created, to commit with their sources.
    pub created_sidecars: Vec<PathBuf>,
    /// Sidecars whose source doesn't exist.
    pub orphaned_sidecars: Vec<PathBuf>,
    /// Stale files removed from the output directory.
    pub removed: Vec<PathBuf>,
}

impl CookReport {
    /// How many assets ended with `status`.
    pub fn count(&self, status: CookStatus) -> usize {
        self.assets.iter().filter(|asset| asset.status == status).count()
    }
}

/// Cooks every source asset under `source_root` into `out_root`, as the [module docs](self)
/// describe.
pub fn cook_dir(
    source_root: &Path,
    out_root: &Path,
    options: &CookOptions<'_>,
) -> Result<CookReport, CookError> {
    let tree = SourceTree::scan(source_root)?;
    let resolved = meta::resolve(&tree, options.locked)?;

    let mut outputs: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut plan = Vec::with_capacity(resolved.assets.len());
    for asset in resolved.assets {
        let recipe = Recipe::for_settings(&asset.meta.importer);
        let relative = asset.path.strip_prefix(source_root).expect("scanned under the source root");
        let output = out_root.join(relative).with_extension(recipe.extension);
        if let Some(first) = outputs.insert(output.clone(), asset.path.clone()) {
            return Err(CookError::OutputCollision { first, second: asset.path, output });
        }
        plan.push((asset, recipe, output));
    }
    // Before writing, so an output whose name only changed case (on a case-insensitive file
    // system) is replaced rather than removed as stale.
    let removed = prune(out_root, &outputs)?;

    let cache = options.cache.map(Cache::new);
    let assets = plan
        .into_iter()
        .map(|(asset, recipe, output)| cook_asset(asset, &recipe, output, cache.as_ref(), options.cook))
        .collect::<Result<_, _>>()?;
    Ok(CookReport {
        assets,
        created_sidecars: resolved.created,
        orphaned_sidecars: resolved.orphaned,
        removed,
    })
}

/// Brings one asset's output up to date.
fn cook_asset(
    asset: SourceAsset,
    recipe: &Recipe,
    output: PathBuf,
    cache: Option<&Cache>,
    cook: Cook,
) -> Result<CookedAsset, CookError> {
    let SourceAsset { path, meta } = asset;
    let bytes = fs::read(&path).map_err(|source| CookError::Read { path: path.clone(), source })?;
    let dependencies = dependencies(&path, &bytes, &meta.importer)?;
    let key = CacheKey::new(&KeyInputs {
        source: blake3::hash(&bytes),
        recipe,
        settings: &meta.importer,
        cook,
        dependencies: &dependencies,
    });
    let current = file_hash(&output)?;
    let done =
        |status| CookedAsset { id: meta.id, source: path.clone(), output: output.clone(), key, status };

    let mut cached = None;
    if let Some(cache) = cache
        && let Some(hash) = cache.output_hash(&key)?
    {
        if current == Some(hash) {
            return Ok(done(CookStatus::UpToDate));
        }
        cached = cache.get(&key)?;
    }
    let (status, cooked) = match cached {
        Some(cooked) => (CookStatus::Cached, cooked),
        None => {
            let cooked = cook_source(&path, &bytes, &meta.importer, cook)?;
            if let Some(cache) = cache {
                cache.put(&key, &cooked)?;
            }
            (CookStatus::Cooked, cooked)
        }
    };
    if current != Some(blake3::hash(&cooked)) {
        write_atomically(&output, &cooked)?;
    }
    Ok(done(status))
}

/// Imports, bakes and cooks one source: the bytes of its cooked file.
fn cook_source(
    path: &Path,
    bytes: &[u8],
    settings: &ImportSettings,
    cook: Cook,
) -> Result<Vec<u8>, CookError> {
    match settings {
        ImportSettings::Gltf(settings) => {
            let mesh = import_mesh(path, bytes, settings, cook)?;
            Ok(cooked_bytes(path, &mesh)?.into_vec())
        }
        ImportSettings::Texture(settings) => texture::cook_source(path, bytes, settings, cook)
            .map_err(|source| CookError::Texture { path: path.to_owned(), source: Box::new(source) }),
    }
}

/// Everything besides its own bytes that cooking `path` reads: part of its cache key.
fn dependencies(path: &Path, bytes: &[u8], settings: &ImportSettings) -> Result<Vec<Dependency>, CookError> {
    match settings {
        ImportSettings::Gltf(_) => {
            let files = gltf::external_files(bytes)
                .map_err(|source| CookError::Import { path: path.to_owned(), source })?;
            let dir = path.parent().unwrap_or(Path::new(""));
            files
                .into_iter()
                .map(|file| {
                    let full = dir.join(&file);
                    let bytes = fs::read(&full).map_err(|source| CookError::Read { path: full, source })?;
                    Ok(Dependency::File { path: file, hash: blake3::hash(&bytes) })
                })
                .collect()
        }
        // Images are self-contained.
        ImportSettings::Texture(_) => Ok(Vec::new()),
    }
}

/// Imports, bakes, optimizes and packs one mesh source read from `path`, for `cook`.
fn import_mesh(path: &Path, bytes: &[u8], settings: &MeshSettings, cook: Cook) -> Result<Mesh, CookError> {
    let mut mesh = gltf::import_slice(bytes, path.parent())
        .map_err(|source| CookError::Import { path: path.to_owned(), source })?;
    bake::outline_normals(&mut mesh, &settings.outline);
    optimize::optimize(&mut mesh, &settings.lods, settings.outline.weld_tolerance(), cook);
    Ok(mesh.pack(settings.position_format))
}

/// `mesh` as cooked bytes, read back through the runtime's checks ([`Mesh::read`]) so a cook
/// never writes a mesh the runtime would refuse.
fn cooked_bytes(path: &Path, mesh: &Mesh) -> Result<sapphire_serialize::cooked::AlignedVec, CookError> {
    let invalid = |source| CookError::Invalid { path: path.to_owned(), source };
    let bytes = mesh.to_cooked().map_err(|error| invalid(MeshError::Cooked(error)))?;
    Mesh::read(&bytes).map_err(invalid)?;
    Ok(bytes)
}

/// Imports, bakes, optimizes, packs and validates one mesh source.
pub fn cook_mesh(source: &Path, settings: &MeshSettings, cook: Cook) -> Result<Mesh, CookError> {
    cook_mesh_with_bytes(source, settings, cook).map(|(mesh, _)| mesh)
}

/// Cooks one mesh source to `output` (a `.smesh` file), creating parent directories. It
/// reads and writes no sidecar and uses no cache.
pub fn cook_mesh_file(
    source: &Path,
    output: &Path,
    settings: &MeshSettings,
    cook: Cook,
) -> Result<(), CookError> {
    let (_, bytes) = cook_mesh_with_bytes(source, settings, cook)?;
    write_atomically(output, &bytes)
}

/// [`cook_mesh`], with the checked cooked bytes.
fn cook_mesh_with_bytes(
    source: &Path,
    settings: &MeshSettings,
    cook: Cook,
) -> Result<(Mesh, sapphire_serialize::cooked::AlignedVec), CookError> {
    if !matches!(ImportSettings::for_source(source), Some(ImportSettings::Gltf(_))) {
        return Err(CookError::UnsupportedSource { path: source.to_owned() });
    }
    let bytes = fs::read(source).map_err(|e| CookError::Read { path: source.to_owned(), source: e })?;
    let mesh = import_mesh(source, &bytes, settings, cook)?;
    let cooked = cooked_bytes(source, &mesh)?;
    Ok((mesh, cooked))
}

/// The blake3 hash of the file at `path`, or `None` if there is none.
fn file_hash(path: &Path) -> Result<Option<blake3::Hash>, CookError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(blake3::hash(&bytes))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CookError::Read { path: path.to_owned(), source }),
    }
}

/// Removes cooked and temporary files under `out_root` that aren't in `outputs`, then any
/// directories that leaves empty. Returns the removed files.
fn prune(out_root: &Path, outputs: &HashMap<PathBuf, PathBuf>) -> Result<Vec<PathBuf>, CookError> {
    let mut removed = Vec::new();
    if out_root.is_dir() {
        prune_dir(out_root, outputs, &mut removed)?;
    }
    Ok(removed)
}

/// [`prune`] for one directory; returns whether it is now empty.
fn prune_dir(
    dir: &Path,
    outputs: &HashMap<PathBuf, PathBuf>,
    removed: &mut Vec<PathBuf>,
) -> Result<bool, CookError> {
    let read_error = |source| CookError::Read { path: dir.to_owned(), source };
    let mut entries =
        fs::read_dir(dir).map_err(read_error)?.collect::<Result<Vec<_>, _>>().map_err(read_error)?;
    entries.sort_by_key(fs::DirEntry::path);
    let mut empty = true;
    for entry in entries {
        let path = entry.path();
        let is_dir =
            entry.file_type().map_err(|source| CookError::Read { path: path.clone(), source })?.is_dir();
        let remove_error = |source| CookError::Write { path: path.clone(), source };
        if is_dir {
            if prune_dir(&path, outputs, removed)? {
                fs::remove_dir(&path).map_err(remove_error)?;
            } else {
                empty = false;
            }
        } else if is_cooked_or_temporary(&path) && !outputs.contains_key(&path) {
            fs::remove_file(&path).map_err(remove_error)?;
            removed.push(path);
        } else {
            empty = false;
        }
    }
    Ok(empty)
}

fn is_cooked_or_temporary(path: &Path) -> bool {
    let extension = path.extension().and_then(|extension| extension.to_str());
    extension.is_some_and(|extension| extension == TEMP_EXTENSION || COOKED_EXTENSIONS.contains(&extension))
}

/// Writes via a temporary sibling and a rename, so a watcher (hot reload, M1) never sees
/// a half-written file.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), CookError> {
    let write_error = |source| CookError::Write { path: path.to_owned(), source };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(write_error)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".");
    temp.push(TEMP_EXTENSION);
    let temp = PathBuf::from(temp);
    fs::write(&temp, bytes).map_err(write_error)?;
    fs::rename(&temp, path).map_err(write_error)
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use sapphire_assets::mesh::PositionFormat;
    use serde_json::json;

    use super::*;
    use crate::bake::OutlineSettings;
    use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet, MorphGeometry};
    use crate::meta::{AssetMeta, sidecar_path};
    use crate::optimize::LodSettings;
    use crate::samples;
    use crate::test_util::{TempDir, read_aligned};

    /// A source tree, an output directory and a cache, all in one scratch directory.
    struct Scratch {
        dir: TempDir,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            Self { dir: TempDir::new(name) }
        }

        fn source(&self) -> PathBuf {
            self.dir.path().join("content")
        }

        /// The desktop cook's output.
        fn out(&self) -> PathBuf {
            self.out_for(Cook::Desktop)
        }

        fn out_for(&self, cook: Cook) -> PathBuf {
            self.dir.path().join("out").join(cook.name())
        }

        /// Writes the mannequin to `relative` under the source root.
        fn add_mannequin(&self, relative: &str) -> PathBuf {
            let path = self.source().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, samples::mannequin_glb()).unwrap();
            path
        }

        fn cook_with(&self, cook: Cook, cache: bool) -> CookReport {
            let cache_dir = self.dir.path().join("cache");
            let options = CookOptions { cook, cache: cache.then_some(cache_dir.as_path()), locked: false };
            cook_dir(&self.source(), &self.out_for(cook), &options).unwrap()
        }

        fn cook(&self) -> CookReport {
            self.cook_with(Cook::Desktop, true)
        }
    }

    fn statuses(report: &CookReport) -> Vec<CookStatus> {
        report.assets.iter().map(|asset| asset.status).collect()
    }

    fn set_weld_tolerance(source: &Path, meters: f32) {
        let sidecar = sidecar_path(source);
        let mut meta = meta::read(&sidecar).unwrap();
        meta.importer = ImportSettings::Gltf(MeshSettings {
            outline: OutlineSettings::with_weld_tolerance(meters).unwrap(),
            ..MeshSettings::default()
        });
        fs::write(&sidecar, sapphire_serialize::to_ron::<AssetMeta>(&meta).unwrap()).unwrap();
    }

    /// The cooked mesh at `path`.
    fn read_mesh(path: &Path) -> Mesh {
        Mesh::from_cooked(&read_aligned(path)).unwrap()
    }

    #[test]
    fn parses_cook_names() {
        for cook in Cook::ALL {
            assert_eq!(cook.name().parse(), Ok(cook));
            assert_eq!(cook.to_string(), cook.name());
        }
        assert_eq!("console".parse::<Cook>(), Err(UnknownCook("console".into())));
    }

    #[test]
    fn cooks_a_tree_into_mirrored_paths() {
        let scratch = Scratch::new("cook_tree");
        let source = scratch.add_mannequin("characters/mannequin/mannequin.glb");
        scratch.add_mannequin(".hidden/mannequin.glb");
        fs::write(scratch.source().join("notes.txt"), "not an asset").unwrap();

        for cook in Cook::ALL {
            let report = scratch.cook_with(cook, false);
            let expected = scratch.out_for(cook).join("characters/mannequin/mannequin.smesh");
            let [asset] = &report.assets[..] else { panic!("one asset") };
            assert_eq!(
                (&asset.source, &asset.output, asset.status),
                (&source, &expected, CookStatus::Cooked)
            );
            assert_eq!(asset.id, meta::read(&sidecar_path(&source)).unwrap().id);

            let mesh = read_mesh(&expected);
            // Outline normals are baked: at a pedestal corner they differ from the split
            // shading normals.
            let corner = (0..mesh.streams.len())
                .filter_map(|index| mesh.streams.vertex(index))
                .find(|v| Vec3::from(v.position).abs_diff_eq(Vec3::new(0.55, 0.0, 0.55), 1e-4))
                .expect("pedestal bottom corner");
            assert_ne!(corner.outline_normal, corner.normal);
            assert!(
                Vec3::from(corner.outline_normal).abs_diff_eq(Vec3::new(1.0, -1.0, 1.0).normalize(), 1e-4)
            );
        }
        assert!(!scratch.out().join(".hidden").exists());
        assert!(!scratch.source().join(".hidden/mannequin.glb.meta").exists());
    }

    #[test]
    fn the_first_cook_creates_sidecars_and_later_ones_keep_them() {
        let scratch = Scratch::new("cook_sidecars");
        let source = scratch.add_mannequin("mannequin.glb");
        let first = scratch.cook();
        assert_eq!(first.created_sidecars, [sidecar_path(&source)]);
        let sidecar = fs::read_to_string(sidecar_path(&source)).unwrap();

        let second = scratch.cook();
        assert!(second.created_sidecars.is_empty());
        assert_eq!(fs::read_to_string(sidecar_path(&source)).unwrap(), sidecar);
        assert_eq!(second.assets[0].id, first.assets[0].id);

        let locked = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
        let unsidecarred = scratch.add_mannequin("new.glb");
        let error = cook_dir(&scratch.source(), &scratch.out(), &locked).unwrap_err();
        assert!(matches!(error, CookError::Meta(MetaError::Missing { .. })), "{error:?}");
        assert!(!sidecar_path(&unsidecarred).exists());
    }

    #[test]
    fn unchanged_assets_come_from_the_cache_without_rewriting_outputs() {
        let scratch = Scratch::new("cook_cache_hits");
        scratch.add_mannequin("mannequin.glb");
        let output = scratch.out().join("mannequin.smesh");
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cooked]);
        let written = fs::metadata(&output).unwrap().modified().unwrap();
        let cooked = fs::read(&output).unwrap();

        // Touching or rewriting a source with the same bytes changes nothing.
        let source = scratch.source().join("mannequin.glb");
        fs::write(&source, samples::mannequin_glb()).unwrap();
        let report = scratch.cook();
        assert_eq!(statuses(&report), [CookStatus::UpToDate]);
        assert_eq!(fs::metadata(&output).unwrap().modified().unwrap(), written, "not rewritten");

        // Deleted or edited outputs come back from the cache.
        fs::remove_file(&output).unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cached]);
        assert_eq!(fs::read(&output).unwrap(), cooked);
        fs::write(&output, b"edited by hand").unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cached]);
        assert_eq!(fs::read(&output).unwrap(), cooked);

        // The other cook has keys of its own.
        assert_eq!(statuses(&scratch.cook_with(Cook::Mobile, true)), [CookStatus::Cooked]);
        assert_eq!(statuses(&scratch.cook_with(Cook::Mobile, true)), [CookStatus::UpToDate]);

        // Without a cache every asset cooks, but unchanged outputs still aren't rewritten.
        let written = fs::metadata(&output).unwrap().modified().unwrap();
        assert_eq!(statuses(&scratch.cook_with(Cook::Desktop, false)), [CookStatus::Cooked]);
        assert_eq!(fs::metadata(&output).unwrap().modified().unwrap(), written, "not rewritten");
    }

    #[test]
    fn only_assets_whose_inputs_changed_are_cooked() {
        let scratch = Scratch::new("cook_changes");
        let a = scratch.add_mannequin("a.glb");
        scratch.add_mannequin("b/b.glb");
        // Same bytes and settings: one cook serves both.
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cooked, CookStatus::Cached]);

        set_weld_tolerance(&a, 0.05);
        let report = scratch.cook();
        assert_eq!(statuses(&report), [CookStatus::Cooked, CookStatus::UpToDate]);
        let [a_report, b_report] = &report.assets[..] else { panic!("two assets") };
        assert_ne!(a_report.key, b_report.key);
        assert_ne!(fs::read(&a_report.output).unwrap(), fs::read(&b_report.output).unwrap());

        // Files no asset reads don't matter.
        fs::write(scratch.source().join("notes.txt"), "edited").unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::UpToDate, CookStatus::UpToDate]);
    }

    #[test]
    fn moved_sources_keep_their_id_and_cached_output() {
        let scratch = Scratch::new("cook_moves");
        let source = scratch.add_mannequin("old/mannequin.glb");
        let first = scratch.cook();
        let moved = scratch.source().join("new/renamed.glb");
        fs::create_dir_all(moved.parent().unwrap()).unwrap();
        fs::rename(&source, &moved).unwrap();
        fs::rename(sidecar_path(&source), sidecar_path(&moved)).unwrap();
        fs::write(scratch.out().join("old/leftover.smesh.tmp"), b"interrupted").unwrap();
        fs::write(scratch.out().join("readme.txt"), b"not ours").unwrap();

        let report = scratch.cook();
        let [asset] = &report.assets[..] else { panic!("one asset") };
        assert_eq!(
            (asset.id, asset.key, asset.status),
            (first.assets[0].id, first.assets[0].key, CookStatus::Cached)
        );
        assert_eq!(asset.output, scratch.out().join("new/renamed.smesh"));
        assert_eq!(
            report.removed,
            [scratch.out().join("old/leftover.smesh.tmp"), scratch.out().join("old/mannequin.smesh")]
        );
        assert!(!scratch.out().join("old").exists(), "emptied directories are removed");
        assert!(scratch.out().join("readme.txt").exists(), "only cooked and temporary files are pruned");
    }

    #[test]
    fn damaged_cache_entries_are_cooked_again() {
        let scratch = Scratch::new("cook_damaged");
        scratch.add_mannequin("mannequin.glb");
        let report = scratch.cook();
        let entry = Cache::new(scratch.dir.path().join("cache")).entry_path(&report.assets[0].key);
        let mut bytes = fs::read(&entry).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        fs::write(&entry, bytes).unwrap();
        fs::remove_file(&report.assets[0].output).unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cooked]);
        assert_eq!(statuses(&scratch.cook()), [CookStatus::UpToDate]);
    }

    #[test]
    fn external_buffers_are_part_of_the_key() {
        let scratch = Scratch::new("cook_external");
        let mut writer = GltfWriter::default();
        let triangle = Geometry {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Geometry::default()
        };
        let mesh = writer.add_mesh("Triangle", &[(&triangle, None)]);
        let node = writer.add_node(json!({ "mesh": mesh }));
        writer.add_root(node);
        fs::create_dir_all(scratch.source()).unwrap();
        let document = serde_json::to_vec(&writer.document(Some("triangle%20data.bin"))).unwrap();
        fs::write(scratch.source().join("triangle.gltf"), document).unwrap();
        let buffer = scratch.source().join("triangle data.bin");
        fs::write(&buffer, writer.bin()).unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cooked]);
        assert_eq!(statuses(&scratch.cook()), [CookStatus::UpToDate]);

        // Move the triangle's first vertex.
        let mut bin = writer.bin().to_vec();
        bin[..4].copy_from_slice(&0.5f32.to_le_bytes());
        fs::write(&buffer, &bin).unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cooked]);

        fs::remove_file(&buffer).unwrap();
        let error = scratch_error(&scratch);
        assert!(matches!(&error, CookError::Read { path, .. } if *path == buffer), "{error:?}");
    }

    fn scratch_error(scratch: &Scratch) -> CookError {
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false };
        cook_dir(&scratch.source(), &scratch.out(), &options).unwrap_err()
    }

    #[test]
    fn settings_reach_the_bake() {
        let source = TempDir::new("cook_settings");
        let path = samples::write_all(source.path()).unwrap().remove(0);
        // Without LODs, so the vertex order can't change with the seams the weld finds.
        let no_lods = MeshSettings { lods: LodSettings::none(), ..MeshSettings::default() };
        let default = cook_mesh(&path, &no_lods, Cook::Desktop).unwrap();
        let coarse =
            MeshSettings { outline: OutlineSettings::with_weld_tolerance(0.05).unwrap(), ..no_lods.clone() };
        let welded = cook_mesh(&path, &coarse, Cook::Desktop).unwrap();
        assert_eq!(default.streams.len(), welded.streams.len());
        assert_eq!(default.streams.positions, welded.streams.positions);
        assert_ne!(
            default.streams.shading, welded.streams.shading,
            "a 5 cm weld must change some outline normals"
        );
    }

    #[test]
    fn lod_and_position_settings_reach_the_cook() {
        let source = TempDir::new("cook_lod_settings");
        let path = samples::write_all(source.path()).unwrap().remove(0);
        let cook = |settings: &MeshSettings, cook| cook_mesh(&path, settings, cook).unwrap();
        let default = cook(&MeshSettings::default(), Cook::Desktop);
        assert!(default.lods.len() > 1, "the mannequin simplifies");
        assert_eq!(default.streams.position_format(), PositionFormat::Unorm16);
        let single = MeshSettings { lods: LodSettings::none(), ..MeshSettings::default() };
        for target in Cook::ALL {
            assert_eq!(cook(&single, target).lods.len(), 1);
        }
        let float = MeshSettings { position_format: PositionFormat::Float32, ..MeshSettings::default() };
        let float = cook(&float, Cook::Desktop);
        assert_eq!(float.streams.position_format(), PositionFormat::Float32);
        assert_eq!((float.indices, float.streams.shading), (default.indices, default.streams.shading));
    }

    #[test]
    fn the_mobile_cook_has_a_lower_detail_lod0_and_fewer_lods() {
        let source = TempDir::new("cook_mobile");
        let path = samples::write_all(source.path()).unwrap().remove(0);
        let desktop = cook_mesh(&path, &MeshSettings::default(), Cook::Desktop).unwrap();
        let mobile = cook_mesh(&path, &MeshSettings::default(), Cook::Mobile).unwrap();
        let triangles = |mesh: &Mesh, lod: usize| -> u32 {
            mesh.lods[lod].submeshes.iter().map(|submesh| submesh.index_count / 3).sum()
        };
        assert_eq!(mobile.lods.len(), desktop.lods.len() - 1);
        for lod in 0..mobile.lods.len() {
            assert_eq!(triangles(&mobile, lod), triangles(&desktop, lod + 1), "mobile LOD{lod}");
            assert_eq!(mobile.lods[lod].error, desktop.lods[lod + 1].error);
        }
        assert!(mobile.lods[0].error > 0.0, "mobile LOD0 is simplified");
        assert!(mobile.streams.len() < desktop.streams.len(), "vertices only desktop LOD0 used are dropped");
        assert_eq!(mobile.streams.len() as u32, desktop.lods[1].vertex_count);
        eprintln!(
            "mannequin: desktop LOD triangles {:?}, errors {:?} m; {} vertices desktop, {} mobile",
            (0..desktop.lods.len()).map(|lod| triangles(&desktop, lod)).collect::<Vec<_>>(),
            desktop.lods.iter().map(|lod| lod.error).collect::<Vec<_>>(),
            desktop.streams.len(),
            mobile.streams.len()
        );
    }

    #[test]
    fn cooks_are_deterministic() {
        let scratch = Scratch::new("cook_determinism");
        scratch.add_mannequin("mannequin.glb");
        let output = scratch.out().join("mannequin.smesh");
        scratch.cook_with(Cook::Desktop, false);
        let first = fs::read(&output).unwrap();
        fs::remove_file(&output).unwrap();
        scratch.cook_with(Cook::Desktop, false);
        assert!(fs::read(&output).unwrap() == first, "two cooks of one source write the same bytes");
        scratch.cook_with(Cook::Mobile, false);
        assert!(fs::read(scratch.out_for(Cook::Mobile).join("mannequin.smesh")).unwrap() != first);
    }

    /// A hard-edged roof: two quads meeting at 90° along the x axis, with split normals.
    fn roof(outline_normals: bool) -> Geometry {
        let (left, right) = (Vec3::new(0.0, 1.0, -1.0).normalize(), Vec3::new(0.0, 1.0, 1.0).normalize());
        Geometry {
            positions: vec![
                [0.0, 0.0, -1.0],
                [1.0, 0.0, -1.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            normals: [left; 4].into_iter().chain([right; 4]).map(|n| n.to_array()).collect(),
            outline_normals: if outline_normals { vec![[0.0, 1.0, 0.0]; 8] } else { Vec::new() },
            indices: vec![0, 2, 1, 0, 3, 2, 4, 6, 5, 4, 7, 6],
            ..Geometry::default()
        }
    }

    #[test]
    fn authored_outline_normals_win_over_the_welded_bake() {
        let source = TempDir::new("cook_authored_outline");
        let cook = |outline_normals: bool| {
            let mut writer = GltfWriter::default();
            let mesh = writer.add_mesh("Roof", &[(&roof(outline_normals), None)]);
            let node = writer.add_node(json!({ "mesh": mesh }));
            writer.add_root(node);
            let path = source.path().join("roof.glb");
            fs::write(&path, writer.to_glb()).unwrap();
            let mesh = cook_mesh(&path, &MeshSettings::default(), Cook::Desktop).unwrap();
            (0..mesh.streams.len()).map(|index| mesh.streams.vertex(index).unwrap()).collect::<Vec<_>>()
        };
        // Baked: the ridge vertices (y = 1) average both faces, the eaves keep their face's.
        for vertex in cook(false) {
            let expected = if vertex.position[1] == 1.0 { Vec3::Y } else { Vec3::from(vertex.normal) };
            assert!(Vec3::from(vertex.outline_normal).abs_diff_eq(expected, 1e-4), "{vertex:?}");
        }
        // Authored: every vertex keeps the authored direction, eaves included.
        for vertex in cook(true) {
            assert!(Vec3::from(vertex.outline_normal).abs_diff_eq(Vec3::Y, 1e-4), "{vertex:?}");
            assert!(!Vec3::from(vertex.normal).abs_diff_eq(Vec3::Y, 0.1), "shading normals stay split");
        }
    }

    /// Vertices above this height are raised by [`skinned_strip`]'s morph target. It lies
    /// between rows, so quantization can't move a vertex across it.
    const RAISED_ABOVE: f32 = 0.51;

    /// A vertical strip of `rows` (40) quads from y = 0 to 1, skinned to two joints (the upper
    /// joint's weight is y) with one morph target, `raise`, that lifts the top half by 0.1.
    fn skinned_strip(rows: u32) -> Vec<u8> {
        let mut geometry = Geometry::default();
        let mut influences = InfluenceSet::default();
        let mut raise = MorphGeometry::default();
        for row in 0..=rows {
            let y = row as f32 / rows as f32;
            for x in [0.0, 0.2] {
                geometry.positions.push([x, y, 0.0]);
                geometry.normals.push([0.0, 0.0, 1.0]);
                influences.joints.push([0, 1, 0, 0]);
                influences.weights.push([1.0 - y, y, 0.0, 0.0]);
                raise.positions.push(if y > RAISED_ABOVE { [0.0, 0.1, 0.0] } else { [0.0; 3] });
            }
        }
        for row in 0..rows {
            let a = 2 * row;
            geometry.indices.extend([a, a + 1, a + 3, a, a + 3, a + 2]);
        }
        geometry.influences = vec![influences];
        geometry.targets = vec![raise];
        let mut writer = GltfWriter::default();
        let lower = writer.add_node(json!({ "name": "Lower" }));
        let upper = writer.add_node(json!({ "name": "Upper", "translation": [0.0, 1.0, 0.0] }));
        let mut doc_nodes = vec![lower, upper];
        let skin = writer.add_skin(&doc_nodes, None);
        let mesh = writer.add_mesh("Strip", &[(&geometry, None)]);
        writer.set_mesh_field(mesh, "extras", json!({ "targetNames": ["raise"] }));
        let body = writer.add_node(json!({ "name": "Body", "mesh": mesh, "skin": skin }));
        doc_nodes.push(body);
        for node in doc_nodes {
            writer.add_root(node);
        }
        writer.to_glb()
    }

    #[test]
    fn skins_and_morph_targets_follow_the_vertex_reorder() {
        let source = TempDir::new("cook_skinned");
        let path = source.path().join("strip.glb");
        fs::write(&path, skinned_strip(40)).unwrap();
        for target in Cook::ALL {
            let mesh = cook_mesh(&path, &MeshSettings::default(), target).unwrap();
            assert!(mesh.lods.len() > 1, "{target}: a flat strip simplifies");
            let skin = mesh.skin.as_ref().expect("skinned");
            let names: Vec<&str> = skin.joints.iter().map(|joint| joint.name.as_str()).collect();
            assert_eq!(names, ["Lower", "Upper"]);
            for index in 0..mesh.streams.len() {
                let y = mesh.streams.vertex(index).unwrap().position[1];
                let influence = skin.influences.get(index).unwrap();
                let upper = influence.binds.iter().zip(influence.weights).find(|(bind, _)| **bind == 1);
                let upper = upper.map_or(0.0, |(_, weight)| f32::from(weight) / 255.0);
                assert!((upper - y).abs() <= 1.0 / 255.0, "{target}: vertex {index} at y {y} weighs {upper}");
            }
            let [raise] = &mesh.morph_targets[..] else { panic!("one target") };
            assert_eq!(raise.name, "raise");
            let deltas = &mesh.morph_deltas[raise.first_delta as usize..][..raise.delta_count as usize];
            let height = |index: usize| mesh.streams.vertex(index).unwrap().position[1];
            let raised = (0..mesh.streams.len()).filter(|&index| height(index) > RAISED_ABOVE).count();
            assert_eq!(deltas.len(), raised, "{target}: one delta per raised vertex");
            for delta in deltas {
                let y = height(delta.vertex() as usize);
                assert!(
                    y > RAISED_ABOVE && (delta.position()[1] - 0.1).abs() < 1e-4,
                    "{target}: {delta:?} at y {y}"
                );
            }
        }
    }

    #[test]
    fn reports_bad_inputs_with_their_paths() {
        let scratch = Scratch::new("cook_errors");
        fs::create_dir_all(scratch.source()).unwrap();
        fs::write(scratch.source().join("model.gltf"), "{}").unwrap();
        scratch.add_mannequin("model.glb");
        let error = scratch_error(&scratch);
        assert!(matches!(error, CookError::OutputCollision { .. }), "{error:?}");

        fs::remove_file(scratch.source().join("model.glb")).unwrap();
        fs::remove_file(sidecar_path(&scratch.source().join("model.glb"))).unwrap();
        let error = scratch_error(&scratch);
        assert!(
            matches!(&error, CookError::Import { path, .. } if path.ends_with("model.gltf")),
            "{error:?}"
        );

        let error = cook_mesh(Path::new("model.fbx"), &MeshSettings::default(), Cook::Desktop).unwrap_err();
        assert!(matches!(error, CookError::UnsupportedSource { .. }));
    }
}
