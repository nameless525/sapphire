//! Cooking: source assets and their sidecars in, the runtime's cooked files out, through the
//! content cache (spec §5.27).
//!
//! [`cook_dir`] cooks a content tree:
//!
//! 1. Finds the source assets, leaving out the images that map documents read (they are
//!    part of those assets), and resolves their sidecars ([`meta`]), creating missing ones.
//!    Sources whose sidecars say `cook: false` go no further: the report lists them.
//! 2. Plans each asset's output: its path under the content root, mirrored under the output
//!    directory with the cooked format's extension (`samples/mannequin.glb` →
//!    `samples/mannequin.smesh`). Tools read cooked files by these paths; the runtime reads
//!    packs ([`crate::pack`], the stage after this one) and addresses assets by ID.
//! 3. Removes stale files from the output directory: cooked files no source produces any
//!    more, and temporary files from an interrupted run. Other files are left alone.
//! 4. Keys each asset ([`CacheKey`]) and takes its output from the [`Cache`], or imports,
//!    bakes, validates and cooks it and stores the result. An output is written only when its
//!    bytes change, so a file watcher (hot reload, A8) sees exactly the assets that changed.
//!    Materials read the cooked textures they reference, so they cook after every other asset
//!    but probe grids, which read cooked meshes and materials; their keys depend on those
//!    assets' keys.
//!
//! Each kind of source cooks through a [`Recipe`]: the importer, the cooker and the cooked
//! format, whose versions are part of the cache key:
//!
//! - **Meshes** cook glTF and FBX → `.smesh` (A1, A5, A10): import, outline bake,
//!   [optimization and LODs](crate::optimize), packing, then a read-back of the cooked bytes
//!   through the runtime's own checks. The two cooks differ in the LOD chain: the mobile cook
//!   drops its first levels, for a lower-detail LOD0 and fewer LODs (spec §5.27).
//! - **Textures** cook PNG, TGA and EXR → `.ktx2` (A2), BCn on desktop and ASTC on mobile
//!   ([`texture`]).
//! - **Anime character maps** cook `.facesdf.ron`, `.ramp.ron` and `.ilm.ron` documents →
//!   `.ktx2` (A7): the map a document describes is generated, then cooked as its texture
//!   class ([`character_maps`]).
//! - **Materials** cook `.material.ron` documents → `.smat` (A3), checked against the textures
//!   they reference ([`material`]). A mesh's slots embed the materials the importer generated,
//!   or name the material assets its sidecar binds them to, which must be materials of the same
//!   tree.
//! - **VRM characters** cook `.vrm` → `.smesh` like glTF meshes, plus a `.srig` rig (A6,
//!   [`vrm`]).
//! - **Ambient probe grids** cook `.probes.ron` documents → `.sprobe` (W5, [`probes`]): a bake of
//!   the meshes the document names, lit by their materials under time of day's keys. They read
//!   cooked meshes and materials' sources, so they cook last; their keys depend on those assets'
//!   keys.
//! - **Assemblies** cook `.assembly.ron` documents → `.smesh` like glTF meshes: kit pieces
//!   placed into one mesh in slots of the document's own (M1 X1, [`assembly`]). The pieces'
//!   files are dependencies of the key.
//!
//! A source may cook to more than one asset: its own, and [`SubAsset`]s, each with its own
//! recipe, output beside the source's (`alicia.smesh`, `alicia.srig`) and an ID derived from the
//! source's ([`meta::sub_asset_id`]). Each is planned, keyed and cached as an asset of its own,
//! so a report lists it and a pack (A4) can address it; each imports the source itself.
//!
//! Assets cook one after another. Cooking in parallel (rayon) waits for content that makes it
//! worth it.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sapphire_assets::AssetId;
use sapphire_assets::material::{MATERIAL_EXTENSION, Material};
use sapphire_assets::mesh::{MESH_EXTENSION, Mesh, MeshError};
use sapphire_assets::probes::{PROBE_GRID_EXTENSION, ProbeGrid};
use sapphire_assets::rig::{CharacterRig, RIG_EXTENSION, RigError};
use sapphire_assets::texture::{TEXTURE_EXTENSION, TEXTURE_VERSION};
use sapphire_reflect::Versioned;

use crate::assembly::{self, AssemblyError};
use crate::bake::foliage::FoliageError;
use crate::cache::{Cache, CacheError, CacheKey, Dependency, KeyInputs, Stage};
use crate::character_maps::{self, MapError, MapKind};
use crate::import::fbx::{self, FbxImportError};
use crate::import::gltf::{self, GltfImportError};
use crate::import::image;
use crate::import::vrm::{self, VrmImportError, VrmSettings};
use crate::material::{self, MaterialCookError, Referenced, TextureKind};
use crate::mesh::MaterialSlotError;
use crate::meta::{self, ImportSettings, MeshSettings, MetaError, SourceAsset, SourceTree};
use crate::probes::{self, ProbeCookError, ProbeInput};
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
/// they change output for the same imported mesh. 3: deformation-aware LODs (the review's
/// AP-005): what morph targets move is locked, and skinned meshes simplify in test poses too.
pub const MESH_COOKER: Stage = Stage { name: "smesh", version: 3 };

/// Every extension a recipe writes: stale files with these are removed from the output.
pub(crate) const COOKED_EXTENSIONS: &[&str] =
    &[MESH_EXTENSION, TEXTURE_EXTENSION, MATERIAL_EXTENSION, RIG_EXTENSION, PROBE_GRID_EXTENSION];

/// An asset a source cooks to besides its own (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubAsset {
    /// Unique among the source's sub-assets, and never changed: the sub-asset's ID is derived
    /// from it.
    pub name: &'static str,
    pub recipe: Recipe,
}

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
            ImportSettings::Fbx(_) => Self {
                importer: fbx::IMPORTER,
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
            // The importer generates the map as an image, which the texture cooker cooks.
            ImportSettings::CharacterMap(settings) => Self {
                importer: settings.kind().importer(),
                cooker: texture::COOKER,
                format: TEXTURE_VERSION,
                extension: TEXTURE_EXTENSION,
            },
            ImportSettings::Material(_) => Self {
                importer: material::IMPORTER,
                cooker: material::COOKER,
                format: Material::VERSION,
                extension: MATERIAL_EXTENSION,
            },
            ImportSettings::Vrm(_) => Self {
                importer: vrm::IMPORTER,
                cooker: MESH_COOKER,
                format: Mesh::VERSION,
                extension: MESH_EXTENSION,
            },
            ImportSettings::ProbeGrid(_) => Self {
                importer: probes::IMPORTER,
                cooker: probes::COOKER,
                format: ProbeGrid::VERSION,
                extension: PROBE_GRID_EXTENSION,
            },
            ImportSettings::Assembly(_) => Self {
                importer: assembly::IMPORTER,
                cooker: MESH_COOKER,
                format: Mesh::VERSION,
                extension: MESH_EXTENSION,
            },
        }
    }

    /// The sub-assets sources read with `settings` cook to besides their own asset.
    pub fn sub_assets(settings: &ImportSettings) -> &'static [SubAsset] {
        match settings {
            ImportSettings::Vrm(_) => &[vrm::RIG],
            ImportSettings::Gltf(_)
            | ImportSettings::Fbx(_)
            | ImportSettings::Assembly(_)
            | ImportSettings::Texture(_)
            | ImportSettings::CharacterMap(_)
            | ImportSettings::Material(_)
            | ImportSettings::ProbeGrid(_) => &[],
        }
    }

    /// Where a source at `relative` cooks to, relative to the output root: its path with the
    /// cooked extension in place of the source's (`samples/mannequin.glb` →
    /// `samples/mannequin.smesh`, `skin.material.ron` → `skin.smat`, `courtyard.probes.ron` →
    /// `courtyard.sprobe`).
    pub fn output_path(&self, relative: &Path) -> PathBuf {
        if material::is_source(relative) {
            material::output_path(relative, self.extension)
        } else if probes::is_source(relative) {
            probes::output_path(relative, self.extension)
        } else if assembly::is_source(relative) {
            assembly::output_path(relative, self.extension)
        } else {
            relative.with_extension(self.extension)
        }
    }
}

/// Errors from cooking. Each names the file it concerns.
#[derive(Debug, thiserror::Error)]
pub enum CookError {
    #[error("{} is not a supported mesh source (expected .gltf, .glb, .fbx or .vrm)", path.display())]
    UnsupportedSource { path: PathBuf },
    #[error(transparent)]
    Meta(#[from] MetaError),
    #[error("failed to import {}", path.display())]
    Import {
        path: PathBuf,
        #[source]
        source: GltfImportError,
    },
    #[error("failed to import FBX {}", path.display())]
    Fbx {
        path: PathBuf,
        /// Boxed, as VRM errors are.
        #[source]
        source: Box<FbxImportError>,
    },
    #[error("failed to assemble {}", path.display())]
    Assembly {
        path: PathBuf,
        /// Boxed: document and glTF errors are large, and every cook result carries this type.
        #[source]
        source: Box<AssemblyError>,
    },
    #[error("failed to import VRM {}", path.display())]
    Vrm {
        path: PathBuf,
        /// Boxed: glTF and JSON errors are large, and every cook result carries this type.
        #[source]
        source: Box<VrmImportError>,
    },
    #[error("cooked rig from {} is invalid", path.display())]
    InvalidRig {
        path: PathBuf,
        #[source]
        source: RigError,
    },
    #[error("failed to cook texture {}", path.display())]
    Texture {
        path: PathBuf,
        /// Boxed: decoder and encoder errors are large, and every cook result carries this type.
        #[source]
        source: Box<TextureError>,
    },
    #[error("failed to cook character map {}", path.display())]
    Map {
        path: PathBuf,
        /// Boxed, as texture errors are.
        #[source]
        source: Box<MapError>,
    },
    #[error("cooked mesh from {} is invalid", path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: MeshError,
    },
    #[error("failed to cook material {}", path.display())]
    Material {
        path: PathBuf,
        /// Boxed: document errors are large, and every cook result carries this type.
        #[source]
        source: Box<MaterialCookError>,
    },
    #[error("failed to bake the foliage of {}", path.display())]
    Foliage {
        path: PathBuf,
        #[source]
        source: FoliageError,
    },
    #[error("failed to bake probe grid {}", path.display())]
    ProbeGrid {
        path: PathBuf,
        /// Boxed, as material errors are.
        #[source]
        source: Box<ProbeCookError>,
    },
    #[error("failed to resolve the material slots of {}", path.display())]
    MaterialSlot {
        path: PathBuf,
        #[source]
        source: MaterialSlotError,
    },
    #[error(
        "{} binds material slot {slot:?} to {id}, which is not a material document of this content tree",
        path.display()
    )]
    NotAMaterial { path: PathBuf, slot: String, id: AssetId },
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
    /// blake3 of the cooked file's bytes, as written: what the packer keys its encoding by.
    pub output_hash: blake3::Hash,
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
    /// Sources whose sidecars keep them out of the cook (`cook: false`, see [`meta`]).
    pub uncooked: Vec<PathBuf>,
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
    let mut tree = SourceTree::scan(source_root)?;
    let inputs = map_inputs(source_root, &tree)?;
    tree.sources.retain(|source| !inputs.contains(source));
    let resolved = meta::resolve(&tree, options.locked)?;
    let (cooked_assets, uncooked): (Vec<SourceAsset>, Vec<SourceAsset>) =
        resolved.assets.into_iter().partition(|asset| asset.meta.cook);

    let mut outputs: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut plan = Vec::with_capacity(cooked_assets.len());
    for asset in cooked_assets {
        let own = (asset.meta.id, None, Recipe::for_settings(&asset.meta.importer));
        let sub_assets = Recipe::sub_assets(&asset.meta.importer)
            .iter()
            .map(|sub| (meta::sub_asset_id(asset.meta.id, sub.name), Some(sub.name), sub.recipe));
        for (id, sub_asset, recipe) in std::iter::once(own).chain(sub_assets) {
            let relative = asset.path.strip_prefix(source_root).expect("scanned under the source root");
            let output = out_root.join(recipe.output_path(relative));
            if let Some(first) = outputs.insert(output.clone(), asset.path.clone()) {
                return Err(CookError::OutputCollision { first, second: asset.path, output });
            }
            plan.push(Planned { asset: asset.clone(), id, sub_asset, recipe, output });
        }
    }
    // Before writing, so an output whose name only changed case (on a case-insensitive file
    // system) is replaced rather than removed as stale.
    let removed = prune(out_root, &|path| outputs.contains_key(path), COOKED_EXTENSIONS)?;

    let cache = options.cache.map(Cache::new);
    let mut by_id = HashMap::with_capacity(plan.len());
    for (index, planned) in plan.iter().enumerate() {
        // Sidecar IDs are unique (checked by `meta::resolve`) and random; a derived ID equal to
        // another is beyond unlikely, but would silently alias two assets.
        if let Some(first) = by_id.insert(planned.id, index) {
            return Err(CookError::Meta(MetaError::DuplicateId {
                id: planned.id,
                first: plan[first].asset.path.clone(),
                second: planned.asset.path.clone(),
            }));
        }
    }
    // Materials read the cooked textures they reference, so everything else cooks first; probe
    // grids read cooked meshes and their materials, so they cook last.
    let (probe_grids, rest): (Vec<usize>, Vec<usize>) =
        (0..plan.len()).partition(|&index| plan[index].is_probe_grid());
    let (materials, others): (Vec<usize>, Vec<usize>) =
        rest.into_iter().partition(|&index| plan[index].is_material());
    let mut cooked: Vec<Option<CookedAsset>> = vec![None; plan.len()];
    for index in others.into_iter().chain(materials).chain(probe_grids) {
        let assets = Assets { plan: &plan, by_id: &by_id, cooked: &cooked };
        let asset = cook_asset(&plan[index], &assets, cache.as_ref(), options.cook)?;
        cooked[index] = Some(asset);
    }
    let assets = cooked.into_iter().map(|asset| asset.expect("every planned asset was cooked")).collect();
    Ok(CookReport {
        assets,
        created_sidecars: resolved.created,
        orphaned_sidecars: resolved.orphaned,
        uncooked: uncooked.into_iter().map(|asset| asset.path).collect(),
        removed,
    })
}

/// A source asset, its recipe and where it cooks to: the source's own asset, or one of its
/// sub-assets.
struct Planned {
    asset: SourceAsset,
    /// The sidecar's ID, or the sub-asset's derived one.
    id: AssetId,
    /// The sub-asset's name, `None` for the source's own asset.
    sub_asset: Option<&'static str>,
    recipe: Recipe,
    output: PathBuf,
}

impl Planned {
    fn is_material(&self) -> bool {
        matches!(self.asset.meta.importer, ImportSettings::Material(_))
    }

    fn is_probe_grid(&self) -> bool {
        matches!(self.asset.meta.importer, ImportSettings::ProbeGrid(_))
    }
}

/// The content tree's assets by ID, with what has cooked so far.
struct Assets<'a> {
    plan: &'a [Planned],
    by_id: &'a HashMap<AssetId, usize>,
    cooked: &'a [Option<CookedAsset>],
}

impl<'a> Assets<'a> {
    fn get(&self, id: AssetId) -> Option<&'a Planned> {
        self.by_id.get(&id).map(|&index| &self.plan[index])
    }

    /// What a material's cook knows about asset `id`.
    fn referenced(&self, id: AssetId) -> Option<Referenced<'a>> {
        let index = *self.by_id.get(&id)?;
        let planned = &self.plan[index];
        Some(Referenced {
            source: &planned.asset.path,
            class: TextureKind::of(&planned.asset.meta.importer),
            cooked: self.cooked[index].as_ref().map(|asset| (asset.output.as_path(), asset.key)),
        })
    }

    /// What a probe grid's cook knows about asset `id`.
    fn probe_input(&self, id: AssetId) -> Option<ProbeInput<'a>> {
        let index = *self.by_id.get(&id)?;
        let planned = &self.plan[index];
        Some(ProbeInput {
            source: &planned.asset.path,
            settings: &planned.asset.meta.importer,
            cooked: self.cooked[index].as_ref().map(|asset| (asset.output.as_path(), asset.key)),
        })
    }
}

/// Brings one asset's output up to date.
fn cook_asset(
    planned: &Planned,
    assets: &Assets<'_>,
    cache: Option<&Cache>,
    cook: Cook,
) -> Result<CookedAsset, CookError> {
    let Planned { asset: SourceAsset { path, meta }, id, sub_asset, recipe, output } = planned;
    let bytes = fs::read(path).map_err(|source| CookError::Read { path: path.clone(), source })?;
    let dependencies = dependencies(path, &bytes, &meta.importer, assets)?;
    let key = CacheKey::new(&KeyInputs {
        source: blake3::hash(&bytes),
        recipe,
        settings: &meta.importer,
        cook,
        dependencies: &dependencies,
    });
    let current = file_hash(output)?;
    let done = |status, output_hash| CookedAsset {
        id: *id,
        source: path.clone(),
        output: output.clone(),
        key,
        output_hash,
        status,
    };

    let mut cached = None;
    if let Some(cache) = cache
        && let Some(hash) = cache.output_hash(&key)?
    {
        if current == Some(hash) {
            return Ok(done(CookStatus::UpToDate, hash));
        }
        cached = cache.get(&key)?;
    }
    let (status, cooked) = match cached {
        Some(cooked) => (CookStatus::Cached, cooked),
        None => {
            let cooked = cook_source(path, &bytes, &meta.importer, *sub_asset, cook, assets)?;
            if let Some(cache) = cache {
                cache.put(&key, &cooked)?;
            }
            (CookStatus::Cooked, cooked)
        }
    };
    let hash = blake3::hash(&cooked);
    if current != Some(hash) {
        write_atomically(output, &cooked)?;
    }
    Ok(done(status, hash))
}

/// Imports, bakes and cooks one source, or the sub-asset `sub_asset` of it: the bytes of its
/// cooked file.
fn cook_source(
    path: &Path,
    bytes: &[u8],
    settings: &ImportSettings,
    sub_asset: Option<&str>,
    cook: Cook,
    assets: &Assets<'_>,
) -> Result<Vec<u8>, CookError> {
    match settings {
        ImportSettings::Gltf(_)
        | ImportSettings::Fbx(_)
        | ImportSettings::Vrm(_)
        | ImportSettings::Assembly(_)
            if sub_asset.is_none() =>
        {
            let mesh = import_mesh(path, bytes, settings, cook)?;
            Ok(cooked_bytes(path, &mesh)?.into_vec())
        }
        ImportSettings::Vrm(settings) => {
            debug_assert_eq!(sub_asset, Some(vrm::RIG.name), "a VRM's only sub-asset is its rig");
            let rig = vrm::import_rig(bytes, settings.version).map_err(vrm_error(path))?;
            let invalid = |source| CookError::InvalidRig { path: path.to_owned(), source };
            let cooked = rig.to_cooked().map_err(|error| invalid(RigError::Cooked(error)))?;
            // The runtime's checks, so a cook never writes a rig animation would refuse.
            CharacterRig::read(&cooked).map_err(invalid)?;
            Ok(cooked.into_vec())
        }
        ImportSettings::Gltf(_) | ImportSettings::Fbx(_) | ImportSettings::Assembly(_) => {
            unreachable!("glTF and FBX sources and assemblies have no sub-assets")
        }
        ImportSettings::Texture(settings) => texture::cook_source(path, bytes, settings, cook)
            .map_err(|source| CookError::Texture { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::CharacterMap(settings) => {
            character_maps::cook_source(path, bytes, settings, cook).map_err(map_error(path))
        }
        ImportSettings::Material(_) => material::read(bytes)
            .and_then(|document| material::cook(&document, |id| assets.referenced(id)))
            .map_err(|source| CookError::Material { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::ProbeGrid(_) => probes::read(bytes)
            .and_then(|document| probes::cook(&document, cook, |id| assets.probe_input(id)))
            .map_err(probe_error(path)),
    }
}

/// Wraps a probe grid error as the cook error for the document at `path`.
fn probe_error(path: &Path) -> impl Fn(ProbeCookError) -> CookError + '_ {
    move |source| CookError::ProbeGrid { path: path.to_owned(), source: Box::new(source) }
}

/// Wraps a VRM import error as the cook error for the source at `path`.
fn vrm_error(path: &Path) -> impl Fn(VrmImportError) -> CookError + '_ {
    move |source| CookError::Vrm { path: path.to_owned(), source: Box::new(source) }
}

/// Wraps a map error as the cook error for the document at `path`.
fn map_error(path: &Path) -> impl Fn(MapError) -> CookError + '_ {
    move |source| CookError::Map { path: path.to_owned(), source: Box::new(source) }
}

/// The images that the map documents in `tree` read: inputs of those assets rather than
/// assets of their own. Names that aren't images are left for the map's cook to reject.
///
/// An input inside the content tree must name a scanned file exactly, case included, so a
/// cook claims the same files on every file system. It must not have a sidecar either: that
/// would make it an asset of its own, which claiming it would silently drop.
fn map_inputs(source_root: &Path, tree: &SourceTree) -> Result<HashSet<PathBuf>, CookError> {
    let sources: HashSet<&Path> = tree.sources.iter().map(PathBuf::as_path).collect();
    let sidecars: HashSet<&Path> = tree.sidecars.iter().map(PathBuf::as_path).collect();
    let mut inputs = HashSet::new();
    for document in &tree.sources {
        let Some(kind) = MapKind::for_path(document) else { continue };
        let fail = map_error(document);
        let bytes =
            fs::read(document).map_err(|source| CookError::Read { path: document.clone(), source })?;
        let mut with_sidecars = Vec::new();
        for input in character_maps::inputs(kind, &bytes).map_err(&fail)? {
            let path = character_maps::resolve_input(document, &input).map_err(&fail)?;
            if image::SourceFormat::from_path(&path).is_none() {
                continue;
            }
            let in_tree = path.strip_prefix(source_root).is_ok_and(|relative| !is_hidden(relative));
            if in_tree && !sources.contains(path.as_path()) {
                return Err(fail(MapError::MissingInput(path)));
            }
            if sidecars.contains(meta::sidecar_path(&path).as_path()) {
                with_sidecars.push(path.clone());
            }
            inputs.insert(path);
        }
        if !with_sidecars.is_empty() {
            return Err(fail(MapError::InputsWithSidecars(with_sidecars)));
        }
    }
    Ok(inputs)
}

/// Whether a scan skips `relative`: some component of it is hidden, as [`SourceTree::scan`]
/// treats names starting with a dot.
fn is_hidden(relative: &Path) -> bool {
    relative.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if name.to_str().is_some_and(|name| name.starts_with('.')))
    })
}

/// Everything besides its own bytes that cooking `path` reads: part of its cache key. Also
/// checks the asset references the source makes, on every cook, cached or not, since a
/// referenced asset can change or disappear while the source stays the same.
fn dependencies(
    path: &Path,
    bytes: &[u8],
    settings: &ImportSettings,
    assets: &Assets<'_>,
) -> Result<Vec<Dependency>, CookError> {
    match settings {
        ImportSettings::Gltf(_)
        | ImportSettings::Fbx(_)
        | ImportSettings::Vrm(_)
        | ImportSettings::Assembly(_) => {
            let mesh = settings.mesh().expect("mesh importers have mesh settings");
            // The mesh holds only the bound materials' IDs, so their contents aren't inputs.
            for (slot, &id) in &mesh.materials {
                if !assets.get(id).is_some_and(Planned::is_material) {
                    return Err(CookError::NotAMaterial { path: path.to_owned(), slot: slot.clone(), id });
                }
            }
            if matches!(settings, ImportSettings::Vrm(_) | ImportSettings::Fbx(_)) {
                // VRM files are GLB, and the FBX importer never opens the files an FBX names:
                // they read nothing besides themselves.
                return Ok(Vec::new());
            }
            if matches!(settings, ImportSettings::Assembly(_)) {
                return assembly_dependencies(path, bytes);
            }
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
        ImportSettings::CharacterMap(settings) => {
            let inputs = character_maps::inputs(settings.kind(), bytes).map_err(map_error(path))?;
            inputs
                .into_iter()
                .map(|input| {
                    let full = character_maps::resolve_input(path, &input).map_err(map_error(path))?;
                    let bytes = fs::read(&full).map_err(|source| CookError::Read { path: full, source })?;
                    Ok(Dependency::File { path: input, hash: blake3::hash(&bytes) })
                })
                .collect()
        }
        ImportSettings::Material(_) => material::read(bytes)
            .and_then(|document| material::dependencies(&document, |id| assets.referenced(id)))
            .map_err(|source| CookError::Material { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::ProbeGrid(_) => probes::read(bytes)
            .and_then(|document| probes::dependencies(&document, |id| assets.probe_input(id)))
            .map_err(probe_error(path)),
    }
}

/// The pieces an assembly at `path` places: files it reads, by the path it names them with.
fn assembly_dependencies(path: &Path, bytes: &[u8]) -> Result<Vec<Dependency>, CookError> {
    let fail = |source| CookError::Assembly { path: path.to_owned(), source: Box::new(source) };
    let assembly = assembly::read(bytes).map_err(fail)?;
    let mut dependencies = Vec::new();
    for file in assembly.files().map_err(fail)? {
        let full = character_maps::resolve_input(path, &file)
            .map_err(|_| fail(AssemblyError::PiecePath(file.clone())))?;
        let piece = fs::read(&full).map_err(|source| CookError::Read { path: full.clone(), source })?;
        // A piece's external buffers are read with it.
        let dir = Path::new(&file).parent().unwrap_or(Path::new(""));
        for buffer in
            gltf::external_files(&piece).map_err(|source| CookError::Import { path: full.clone(), source })?
        {
            let buffer_path = full.parent().unwrap_or(Path::new("")).join(&buffer);
            let bytes =
                fs::read(&buffer_path).map_err(|source| CookError::Read { path: buffer_path, source })?;
            let name = dir.join(&buffer).to_string_lossy().replace('\\', "/");
            dependencies.push(Dependency::File { path: name, hash: blake3::hash(&bytes) });
        }
        dependencies.push(Dependency::File { path: file, hash: blake3::hash(&piece) });
    }
    Ok(dependencies)
}

/// The detail an assembly's shapes are built in for `cook`: the mobile cook's lighter.
fn assembly_detail(cook: Cook) -> assembly::Detail {
    match cook {
        Cook::Desktop => assembly::Detail::Full,
        Cook::Mobile => assembly::Detail::Light,
    }
}

/// Imports, bakes, optimizes and packs one mesh source read from `path` with `importer` (glTF,
/// FBX, VRM or an assembly), for `cook`.
fn import_mesh(path: &Path, bytes: &[u8], importer: &ImportSettings, cook: Cook) -> Result<Mesh, CookError> {
    let (mut mesh, settings) = match importer {
        ImportSettings::Gltf(settings) => (
            gltf::import_slice(bytes, path.parent())
                .map_err(|source| CookError::Import { path: path.to_owned(), source })?,
            settings,
        ),
        ImportSettings::Fbx(settings) => (
            fbx::import_slice(bytes)
                .map_err(|source| CookError::Fbx { path: path.to_owned(), source: Box::new(source) })?,
            settings,
        ),
        ImportSettings::Vrm(settings) => {
            (vrm::import(bytes, settings.version).map_err(vrm_error(path))?.mesh, &settings.mesh)
        }
        ImportSettings::Assembly(settings) => (
            assembly::import(path, bytes, assembly_detail(cook))
                .map_err(|source| CookError::Assembly { path: path.to_owned(), source: Box::new(source) })?,
            settings,
        ),
        ImportSettings::Texture(_)
        | ImportSettings::CharacterMap(_)
        | ImportSettings::Material(_)
        | ImportSettings::ProbeGrid(_) => {
            return Err(CookError::UnsupportedSource { path: path.to_owned() });
        }
    };
    bake::outline_normals(&mut mesh, &settings.outline);
    if let Some(foliage) = &settings.foliage {
        bake::foliage::bake(&mut mesh, foliage, settings.outline.weld_tolerance())
            .map_err(|source| CookError::Foliage { path: path.to_owned(), source })?;
    }
    optimize::optimize(&mut mesh, &settings.lods, settings.outline.weld_tolerance(), cook);
    mesh.pack(settings.position_format, &settings.materials)
        .map_err(|source| CookError::MaterialSlot { path: path.to_owned(), source })
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
    // The importer by extension, with `settings` for the mesh; a VRM's version isn't checked.
    let importer = match ImportSettings::for_source(source) {
        Some(ImportSettings::Gltf(_)) => ImportSettings::Gltf(settings.clone()),
        Some(ImportSettings::Fbx(_)) => ImportSettings::Fbx(settings.clone()),
        Some(ImportSettings::Vrm(_)) => {
            ImportSettings::Vrm(VrmSettings { mesh: settings.clone(), version: None })
        }
        Some(ImportSettings::Assembly(_)) => ImportSettings::Assembly(settings.clone()),
        _ => return Err(CookError::UnsupportedSource { path: source.to_owned() }),
    };
    let bytes = fs::read(source).map_err(|e| CookError::Read { path: source.to_owned(), source: e })?;
    let mesh = import_mesh(source, &bytes, &importer, cook)?;
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

/// Removes files under `out_root` with one of `extensions`, or temporary ones, that `keep`
/// doesn't keep, then any directories that leaves empty. Returns the removed files. The cook
/// prunes its cooked files and the packer its packs.
pub(crate) fn prune(
    out_root: &Path,
    keep: &dyn Fn(&Path) -> bool,
    extensions: &[&str],
) -> Result<Vec<PathBuf>, CookError> {
    let mut removed = Vec::new();
    if out_root.is_dir() {
        prune_dir(out_root, keep, extensions, &mut removed)?;
    }
    Ok(removed)
}

/// [`prune`] for one directory; returns whether it is now empty.
fn prune_dir(
    dir: &Path,
    keep: &dyn Fn(&Path) -> bool,
    extensions: &[&str],
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
            if prune_dir(&path, keep, extensions, removed)? {
                fs::remove_dir(&path).map_err(remove_error)?;
            } else {
                empty = false;
            }
        } else if has_extension(&path, extensions) && !keep(&path) {
            fs::remove_file(&path).map_err(remove_error)?;
            removed.push(path);
        } else {
            empty = false;
        }
    }
    Ok(empty)
}

/// Whether `path` is a temporary file or has one of `extensions`.
fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    let extension = path.extension().and_then(|extension| extension.to_str());
    extension.is_some_and(|extension| extension == TEMP_EXTENSION || extensions.contains(&extension))
}

/// Writes via a temporary sibling and a rename, so a watcher (hot reload, M1) never sees
/// a half-written file.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), CookError> {
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
    use sapphire_assets::material::{
        AnimeCharacterMaterial, CharacterSurface, DiffuseMode, HairShading, RampDiffuse, TextureBinding,
    };
    use sapphire_assets::mesh::{PositionFormat, SlotMaterial};
    use sapphire_assets::texture::{Component, Swizzle};
    use serde_json::json;

    use super::*;
    use crate::bake::OutlineSettings;
    use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet, MorphGeometry};
    use crate::material::MaterialSettings;
    use crate::meta::{AssetMeta, sidecar_path};
    use crate::optimize::LodSettings;
    use crate::samples;
    use crate::test_util::{TempDir, read_aligned};
    use crate::texture::{
        ColorSettings, MaskChannels, MaskSettings, RampSettings, TextureClass, TextureSettings,
    };

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
    /// between rows, so quantization can't move a vertex across it, and high enough that the
    /// strip still simplifies on both cooks: the simplifier never moves a vertex a morph target
    /// moves, nor its neighbors (the review's AP-005).
    const RAISED_ABOVE: f32 = 0.76;

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

    const ALBEDO: &str = "a1b2c3d4-0000-4000-8000-000000000001";
    const RAMP: &str = "a1b2c3d4-0000-4000-8000-000000000002";
    const HAIR_MASK: &str = "a1b2c3d4-0000-4000-8000-000000000003";
    const HAIR: &str = "a1b2c3d4-0000-4000-8000-000000000004";

    fn id(text: &str) -> AssetId {
        text.parse().unwrap()
    }

    /// Writes a sidecar with a known ID, so documents can reference the asset.
    fn write_sidecar(source: &Path, id: &str, importer: ImportSettings) {
        let meta = AssetMeta::new(id.parse().unwrap(), importer);
        fs::write(sidecar_path(source), sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
    }

    impl Scratch {
        /// A `width × height` RGBA PNG at `relative`, cooked as `class`, with ID `id`.
        fn add_texture(&self, relative: &str, id: &str, class: TextureClass, seed: u8) -> PathBuf {
            let path = self.source().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            // 16 rows tall, as ramps must be (A7's layout).
            let image = ::image::RgbaImage::from_fn(16, 16, |x, y| {
                ::image::Rgba([(x * 16) as u8 ^ seed, (y * 16) as u8, seed, 255 - (x * 8) as u8])
            });
            image.save_with_format(&path, ::image::ImageFormat::Png).unwrap();
            write_sidecar(&path, id, ImportSettings::Texture(TextureSettings { class }));
            path
        }

        /// A material document at `relative` with ID `id`.
        fn add_material(&self, relative: &str, id: &str, material: &AnimeCharacterMaterial) -> PathBuf {
            let path = self.source().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, sapphire_serialize::to_ron(material).unwrap()).unwrap();
            write_sidecar(&path, id, ImportSettings::Material(MaterialSettings::default()));
            path
        }

        /// Hair textures and a hair material using them: albedo, a ramp, and a two-channel
        /// highlight mask (which ASTC stores with a swizzle).
        fn add_hair(&self) -> PathBuf {
            self.add_texture("hair/albedo.png", ALBEDO, TextureClass::Color(ColorSettings::default()), 1);
            self.add_texture("hair/ramp.png", RAMP, TextureClass::Ramp(RampSettings::default()), 2);
            let two = MaskSettings { channels: MaskChannels::Two, ..MaskSettings::default() };
            self.add_texture("hair/mask.png", HAIR_MASK, TextureClass::Mask(two), 3);
            self.add_material("hair/hair.material.ron", HAIR, &hair_material(id(RAMP)))
        }
    }

    fn hair_material(ramp: AssetId) -> AnimeCharacterMaterial {
        AnimeCharacterMaterial {
            surface: CharacterSurface::Hair(HairShading {
                mask_map: Some(id(HAIR_MASK)),
                ..HairShading::default()
            }),
            base_color_map: Some(id(ALBEDO)),
            diffuse: DiffuseMode::Ramp(RampDiffuse { map: ramp, mix: 0.5 }),
            ..AnimeCharacterMaterial::default()
        }
    }

    fn asset<'a>(report: &'a CookReport, source: &Path) -> &'a CookedAsset {
        report.assets.iter().find(|asset| asset.source == source).expect("the asset was cooked")
    }

    #[test]
    fn materials_cook_against_the_textures_they_reference() {
        let scratch = Scratch::new("cook_materials");
        let hair = scratch.add_hair();
        let ra01 = Swizzle([Component::R, Component::A, Component::Zero, Component::One]);
        for (cook, mask_swizzle) in [(Cook::Desktop, Swizzle::IDENTITY), (Cook::Mobile, ra01)] {
            let report = scratch.cook_with(cook, true);
            let material = asset(&report, &hair);
            assert_eq!(material.output, scratch.out_for(cook).join("hair/hair.smat"));
            assert_eq!(material.id, id(HAIR));
            let cooked = Material::from_cooked(&read_aligned(&material.output)).unwrap();
            let binding = |slot: usize| cooked.textures[slot];
            assert_eq!(binding(AnimeCharacterMaterial::BASE_COLOR).texture, Some(id(ALBEDO)));
            assert_eq!(binding(AnimeCharacterMaterial::RAMP).texture, Some(id(RAMP)));
            let mask = binding(AnimeCharacterMaterial::SURFACE_MASK);
            assert_eq!((mask.texture, mask.swizzle), (Some(id(HAIR_MASK)), mask_swizzle.to_ktx()), "{cook}");
            assert_eq!(binding(AnimeCharacterMaterial::CONTROL), TextureBinding::UNBOUND);
        }
        // Deterministic: without a cache, a second cook writes the same bytes.
        let output = scratch.out().join("hair/hair.smat");
        let first = fs::read(&output).unwrap();
        fs::remove_file(&output).unwrap();
        scratch.cook_with(Cook::Desktop, false);
        assert!(fs::read(&output).unwrap() == first);
    }

    #[test]
    fn a_material_recooks_when_a_texture_it_references_changes() {
        let scratch = Scratch::new("cook_material_dependencies");
        let hair = scratch.add_hair();
        let first = scratch.cook();
        assert!(statuses(&scratch.cook()).iter().all(|&status| status == CookStatus::UpToDate));

        // New ramp pixels: the ramp and the material (whose key holds the ramp's) recook; the
        // other textures don't.
        let ramp = scratch.add_texture("hair/ramp.png", RAMP, TextureClass::Ramp(RampSettings::default()), 9);
        let report = scratch.cook();
        assert_eq!(asset(&report, &ramp).status, CookStatus::Cooked);
        assert_eq!(asset(&report, &hair).status, CookStatus::Cooked);
        assert_ne!(asset(&report, &hair).key, asset(&first, &hair).key);
        let albedo = scratch.source().join("hair/albedo.png");
        assert_eq!(asset(&report, &albedo).status, CookStatus::UpToDate);

        // A texture that disappears fails the material, cached or not.
        fs::remove_file(&ramp).unwrap();
        fs::remove_file(sidecar_path(&ramp)).unwrap();
        let error = scratch_error(&scratch);
        let CookError::Material { path, source } = &error else { panic!("{error:?}") };
        assert_eq!(path, &hair);
        assert!(matches!(**source, MaterialCookError::MissingTexture { slot: "ramp", .. }), "{source}");
    }

    #[test]
    fn material_references_must_be_textures_of_the_right_class() {
        let scratch = Scratch::new("cook_material_references");
        scratch.add_hair();
        let mannequin = scratch.add_mannequin("mannequin.glb");
        let mesh = "a1b2c3d4-0000-4000-8000-000000000005";
        write_sidecar(&mannequin, mesh, ImportSettings::Gltf(MeshSettings::default()));
        let error_for = |ramp: AssetId| {
            scratch.add_material("hair/hair.material.ron", HAIR, &hair_material(ramp));
            let CookError::Material { source, .. } = scratch_error(&scratch) else {
                panic!("a material error")
            };
            *source
        };
        let error = error_for(id(ALBEDO));
        assert!(
            matches!(&error, MaterialCookError::WrongClass { slot: "ramp", class: "color", .. }),
            "{error}"
        );
        assert!(error.to_string().contains("needs a ramp"), "{error}");
        assert!(matches!(error_for(id(mesh)), MaterialCookError::NotATexture { slot: "ramp", .. }));
        let unknown = id("a1b2c3d4-0000-4000-8000-0000000000ff");
        assert!(matches!(error_for(unknown), MaterialCookError::MissingTexture { id, .. } if id == unknown));

        // A ramp that doesn't have A7's 16 rows.
        scratch.add_material("hair/hair.material.ron", HAIR, &hair_material(id(RAMP)));
        let ramp = scratch.source().join("hair/ramp.png");
        ::image::RgbaImage::new(16, 8).save_with_format(&ramp, ::image::ImageFormat::Png).unwrap();
        let CookError::Material { source, .. } = scratch_error(&scratch) else { panic!("a material error") };
        assert!(matches!(*source, MaterialCookError::RampRows { height: 8 }), "{source}");
    }

    #[test]
    fn mesh_sidecars_bind_slots_to_material_documents() {
        let scratch = Scratch::new("cook_material_bindings");
        scratch.add_hair();
        let mannequin = scratch.add_mannequin("mannequin.glb");
        let mut settings = MeshSettings::default();
        let bind = |settings: &MeshSettings| {
            let sidecar = sidecar_path(&mannequin);
            let id = meta::read(&sidecar).map_or_else(|_| meta::new_id().unwrap(), |meta| meta.id);
            let meta = AssetMeta::new(id, ImportSettings::Gltf(settings.clone()));
            fs::write(sidecar, sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
        };
        settings.materials.insert("Hair".into(), id(HAIR));
        bind(&settings);
        scratch.cook();
        let mesh = read_mesh(&scratch.out().join("mannequin.smesh"));
        for slot in &mesh.materials {
            match (&slot.material, slot.name.as_str()) {
                (SlotMaterial::Asset(bound), "Hair") => assert_eq!(*bound, id(HAIR)),
                (SlotMaterial::Embedded(_), name) => assert_ne!(name, "Hair"),
                (other, name) => panic!("slot {name}: {other:?}"),
            }
        }
        assert_eq!(mesh.materials.len(), 5);

        settings.materials.insert("Cape".into(), id(HAIR));
        bind(&settings);
        let error = scratch_error(&scratch);
        assert!(
            matches!(&error, CookError::MaterialSlot { source: MaterialSlotError::UnknownSlot { name, .. }, .. } if name == "Cape"),
            "{error:?}"
        );
        settings.materials.remove("Cape");
        settings.materials.insert("Skin".into(), id(ALBEDO));
        bind(&settings);
        let error = scratch_error(&scratch);
        assert!(matches!(&error, CookError::NotAMaterial { slot, .. } if slot == "Skin"), "{error:?}");
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

        let error = cook_mesh(Path::new("model.obj"), &MeshSettings::default(), Cook::Desktop).unwrap_err();
        assert!(matches!(error, CookError::UnsupportedSource { .. }));
    }

    /// The FBX importer's fixtures (`tools/blender/fbx_fixtures.py`), by file name.
    fn fbx_fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fbx").join(name)
    }

    #[test]
    fn fbx_sources_cook_as_their_gltf_twins_do() {
        let scratch = Scratch::new("cook_fbx");
        let add = |name: &str| {
            let path = scratch.source().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::copy(fbx_fixture(Path::new(name).file_name().unwrap().to_str().unwrap()), &path).unwrap();
            path
        };
        let pairs = ["static_multi", "skinned_morph"]
            .map(|name| (add(&format!("fbx/{name}.fbx")), add(&format!("gltf/{name}.glb"))));
        for cook in Cook::ALL {
            let report = scratch.cook_with(cook, false);
            for (fbx, gltf) in &pairs {
                assert!(matches!(meta::read(&sidecar_path(fbx)).unwrap().importer, ImportSettings::Fbx(_)));
                let (fbx_mesh, gltf_mesh) =
                    (read_mesh(&asset(&report, fbx).output), read_mesh(&asset(&report, gltf).output));
                // The same vertices and triangles, through the same bake, LODs and packing.
                assert_eq!(fbx_mesh.streams.len(), gltf_mesh.streams.len(), "{cook}: {}", fbx.display());
                let triangles = |mesh: &Mesh| -> Vec<u32> {
                    mesh.lods
                        .iter()
                        .map(|lod| lod.submeshes.iter().map(|s| s.index_count / 3).sum())
                        .collect()
                };
                assert_eq!(triangles(&fbx_mesh), triangles(&gltf_mesh), "{cook}: {}", fbx.display());
                let slots = |mesh: &Mesh| -> Vec<String> {
                    let mut names: Vec<String> =
                        mesh.materials.iter().map(|slot| slot.name.clone()).collect();
                    names.sort();
                    names
                };
                assert_eq!(slots(&fbx_mesh), slots(&gltf_mesh));
                let joints = |mesh: &Mesh| -> Vec<String> {
                    mesh.skin
                        .iter()
                        .flat_map(|skin| skin.joints.iter().map(|joint| joint.name.clone()))
                        .collect()
                };
                assert_eq!(joints(&fbx_mesh), joints(&gltf_mesh));
                let targets = |mesh: &Mesh| -> Vec<String> {
                    let mut names: Vec<String> = mesh.morph_targets.iter().map(|t| t.name.clone()).collect();
                    names.sort();
                    names
                };
                assert_eq!(targets(&fbx_mesh), targets(&gltf_mesh));
            }
        }
    }

    #[test]
    fn sources_whose_sidecars_say_so_are_not_cooked() {
        let scratch = Scratch::new("cook_uncooked");
        let source = scratch.add_mannequin("props/mannequin.glb");
        let output = scratch.out().join("props/mannequin.smesh");
        scratch.cook();
        assert!(output.exists());
        let sidecar = sidecar_path(&source);
        let text = fs::read_to_string(&sidecar).unwrap();
        assert!(!text.contains("cook:"), "written only when false: {text}");

        let mut meta = meta::read(&sidecar).unwrap();
        meta.cook = false;
        fs::write(&sidecar, sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
        assert!(fs::read_to_string(&sidecar).unwrap().contains("cook: false"));
        let report = scratch.cook();
        assert!(report.assets.is_empty());
        assert_eq!((report.uncooked, report.removed), (vec![source.clone()], vec![output.clone()]));
        assert!(!output.exists());
        // It keeps its ID, and a locked cook still needs its sidecar.
        assert_eq!(meta::read(&sidecar).unwrap().id, meta.id);
        fs::remove_file(&sidecar).unwrap();
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
        let error = cook_dir(&scratch.source(), &scratch.out(), &options).unwrap_err();
        assert!(matches!(error, CookError::Meta(MetaError::Missing { .. })), "{error:?}");

        // Nothing may reference it: a slot bound to an uncooked material fails as a missing one.
        scratch.add_hair();
        let hair = scratch.source().join("hair/hair.material.ron");
        let mut hair_meta = meta::read(&sidecar_path(&hair)).unwrap();
        hair_meta.cook = false;
        fs::write(sidecar_path(&hair), sapphire_serialize::to_ron(&hair_meta).unwrap()).unwrap();
        let mut settings = MeshSettings::default();
        settings.materials.insert("Hair".into(), id(HAIR));
        let bound = AssetMeta::new(meta.id, ImportSettings::Gltf(settings));
        fs::write(&sidecar, sapphire_serialize::to_ron(&bound).unwrap()).unwrap();
        let error = scratch_error(&scratch);
        assert!(matches!(&error, CookError::NotAMaterial { slot, .. } if slot == "Hair"), "{error:?}");
    }
}
