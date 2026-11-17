//! The cook audit (M6 K1): a report every cook writes beside its cooked files, `cook_audit.ron`, saying
//! what the cooked representation holds and what it could fold or drop, with no change to any cooked byte.
//! It decides how deep character levels can go (M6 K3), which control maps can fold into their materials
//! (K4) and whether an alpha-tested coverage cook is worth making.
//!
//! - **Maps:** each texture's cooked bytes, tail and levels together, and the sum by texture class.
//! - **Materials:** the census of alpha modes by family, with the alpha-tested and blended ones named; and
//!   for each anime character material, which channels of its control map and layer mask are constant,
//!   constant per region (one value a material ID, the region the control map's alpha picks) or vary
//!   within a region (a gradient or painted detail), with each region's range.
//! - **Meshes:** each cooked level's triangles, vertices and error; for a mesh cooked from a mesh source,
//!   the share of LOD0's vertices the simplifier locks by cause and the chain built again with nothing
//!   locked at the same targets and error budgets ([`optimize::audit_chain`], a dry run); and for each
//!   material slot whose material has a control map, how many triangles disagree with it: a triangle
//!   disagrees where its vertices' region ID ([`VertexFlags::MATERIAL_ID_MASK`]) differs among them or from
//!   the control map's ID at any texel whose centre it covers in UV space; and, whatever its vertices hold,
//!   how many straddle regions, covering texels of more than one ID (the triangles no vertex ID could
//!   stand in for the map at). A mesh without a `_MATERIAL_ID` attribute holds ID 0 everywhere.
//!
//! Maps are read as their sources generate them (a map document's image, a texture's decoded source), at
//! full size, before a cook's size cap or block compression. Region agreement is read on the imported
//! mesh before a merge group (M5 L15) folds its slots, so it is each authored slot's. A zone's cells report
//! their levels alone.
//!
//! **Determinism:** the report names sources and outputs relative to the cook's roots, sorts every list
//! and map, and holds no time, so a second cook writes it byte for byte. Each mesh's and material's part
//! is kept in the content cache under its asset's cook key ([`CacheKey::audit`]) and the whole report
//! under a key over every asset's, so a cook that changed nothing reads one entry.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;

use bevy_reflect::TypePath;
use sapphire_assets::character_maps::ilm;
use sapphire_assets::material::{
    AnimeCharacterMaterial, AnyMaterialDocument, CharacterSurface, Material, MaterialFamily, MaterialFlags,
};
use sapphire_assets::mesh::{Mesh, VertexFlags};
use sapphire_assets::texture::Swizzle;
use sapphire_assets::{AssetId, AssetKind};
use sapphire_reflect::{Decoder, VersionError, Versioned};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::cache::{Cache, CacheError, CacheKey, Stage};
use crate::character_maps;
use crate::cook::{self, Cook, CookError, CookReport, CookedAsset};
use crate::import::image::SourceImage;
use crate::material;
use crate::mesh::ImportedMesh;
use crate::meta::{ImportSettings, SourceAsset};
use crate::optimize::{self, ChainAudit};
use crate::procedural::ProceduralTexture;
use crate::texture;
use crate::{bake, bake::foliage};

/// The report's file name, beside a cook's cooked files.
pub const FILE_NAME: &str = "cook_audit.ron";

/// The audit, as cache keys know it. Bump the version whenever what it reports changes for the same cook.
pub const AUDITOR: Stage = Stage { name: "audit", version: 1 };

/// How far a channel's values may spread and still count as one value: half an 8-bit step, finer than any
/// cooked map holds.
pub const SAME_VALUE: f32 = 0.5 / 255.0;

/// Meshes and materials audited at once, at most: as many as a cook's workers.
const WORKERS: usize = 4;

/// What a cook holds, as `cook_audit.ron` says it: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct CookAudit {
    /// `desktop` or `mobile`.
    pub cook: String,
    /// Cooked texture bytes by class (`color`, `normal`, `mask`, `ramp`, `ILM`, `SDF`, `HDR`).
    pub textures: BTreeMap<String, ClassBytes>,
    pub alpha: AlphaCensus,
    /// Every texture, by source then output.
    pub maps: Vec<MapBytes>,
    /// Every material document, by source.
    pub materials: Vec<MaterialAudit>,
    /// Every cooked mesh, by source then output.
    pub meshes: Vec<MeshAudit>,
}

impl Versioned for CookAudit {
    /// 1: M6 K1.
    const VERSION: u32 = 1;

    fn upgrade<D: Decoder>(version: u32, _data: D) -> Result<Self, VersionError<D::Error>> {
        Err(VersionError::TooOld { found: version, oldest: Self::VERSION })
    }
}

impl CookAudit {
    /// `cook_audit.ron`'s text.
    pub fn to_ron(&self) -> Result<String, sapphire_serialize::DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// Reads `cook_audit.ron`'s text.
    pub fn from_ron(text: &str) -> Result<Self, sapphire_serialize::DocumentError> {
        sapphire_serialize::from_ron(text)
    }
}

/// The textures of one class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassBytes {
    pub maps: u32,
    /// Their cooked files' bytes, levels included.
    pub bytes: u64,
}

/// One texture's cooked bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapBytes {
    pub source: String,
    /// The texture's own cooked file: its tail, where it is split into levels.
    pub output: String,
    pub class: String,
    /// Its cooked files' bytes: the tail and every level above it.
    pub bytes: u64,
    /// How many level files it is split into, beside the tail.
    pub levels: u32,
}

/// Every material, a mesh's own generated ones included, by how it treats alpha, as its cooked flags say.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlphaCensus {
    /// By family (`anime character`, `environment`, …).
    pub families: BTreeMap<String, AlphaCounts>,
    /// The alpha-tested ones: a material document by its source, a mesh's generated one as
    /// `<mesh source>#<slot>`. An anime character material alpha-tests for a `Mask` alpha or for a region
    /// that sees through (M5 L11).
    pub alpha_tested: Vec<String>,
    /// The blended ones, named as the alpha-tested are.
    pub blended: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlphaCounts {
    pub opaque: u32,
    pub alpha_tested: u32,
    pub blended: u32,
}

/// One material document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialAudit {
    pub source: String,
    pub family: String,
    /// `opaque`, `alpha-tested` or `blended`, as its cooked flags say.
    pub alpha: String,
    /// An anime character material's own maps.
    pub character: Option<CharacterAudit>,
}

/// What an anime character material's maps hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterAudit {
    /// `body`, `face`, `hair` or `eye`.
    pub surface: String,
    /// The regions its document sets parameters for.
    pub regions: u32,
    /// Regions that see through (M5 L11).
    pub see_through: u32,
    /// The control map: R highlight mask, G shadow bias, B highlight size, by the regions of its A.
    pub control: Option<MapChannels>,
    /// The layer mask (R matcap, G metallic, B emission, A stocking density), by the control map's
    /// regions where the material has one.
    pub layer_mask: Option<MapChannels>,
}

/// One map's channels, read at its source's size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapChannels {
    pub source: String,
    pub width: u32,
    pub height: u32,
    /// The material IDs the control map's alpha holds over this map, with how many of its texels each
    /// covers; empty when the material has no control map.
    pub regions: Vec<RegionTexels>,
    pub channels: Vec<ChannelAudit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionTexels {
    pub id: u8,
    pub texels: u32,
}

/// One channel of a map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelAudit {
    /// `r`, `g`, `b` or `a`.
    pub channel: String,
    pub shape: ChannelShape,
    pub min: f32,
    pub max: f32,
    /// Each region's range, by ID; empty without regions.
    pub regions: Vec<RegionRange>,
}

/// How a channel's values lie over its map, each within [`SAME_VALUE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelShape {
    /// One value everywhere.
    Constant,
    /// One value in each region: what a material's per-region parameters can hold instead.
    PerRegion,
    /// Values that change within a region: a gradient or painted detail.
    Varies,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionRange {
    pub id: u8,
    pub min: f32,
    pub max: f32,
}

/// One cooked mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshAudit {
    pub source: String,
    pub output: String,
    /// Its cooked levels, LOD0 first.
    pub levels: Vec<CookedLevel>,
    /// The simplifier's locks and the chain relaxed of them: meshes cooked from a mesh source alone.
    pub chain: Option<ChainAudit>,
    /// Each material slot of LOD0 as imported, by name.
    pub slots: Vec<SlotAudit>,
}

/// One cooked level.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CookedLevel {
    pub triangles: u32,
    /// The vertices its triangles use.
    pub vertices: u32,
    /// Its error as cooked, in object-space meters.
    pub error: f32,
}

/// One material slot of a mesh's LOD0, as imported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotAudit {
    pub slot: String,
    /// The material document the sidecar binds, by source; `None` for the importer's own.
    pub material: Option<String>,
    /// The material's control map, by source.
    pub control: Option<String>,
    pub triangles: u32,
    /// Triangles whose vertices hold different region IDs.
    pub split: u32,
    /// Triangles that disagree with the control map (split ones included); `None` without one.
    pub disagreeing: Option<u32>,
    /// Triangles that cover the control map's texels of more than one region, whatever their vertices'
    /// IDs; `None` without one.
    pub straddling: Option<u32>,
}

/// An audit that couldn't be made.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("auditing {}", path.display())]
    Cook {
        path: PathBuf,
        #[source]
        source: Box<CookError>,
    },
    #[error("auditing {}: failed to read it", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("auditing {}: {why}", path.display())]
    Invalid { path: PathBuf, why: String },
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error("writing {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: Box<CookError>,
    },
}

/// Whether [`write()`] wrote the report or found it as it would write it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Written {
    Written,
    UpToDate,
}

/// What [`write()`] did: the report's path, whether it wrote it, and the report, which the asset index folds
/// in ([`crate::index`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Audited {
    pub path: PathBuf,
    pub written: Written,
    pub audit: CookAudit,
}

/// Audits the cook `report` describes, of the sources under `source_root` into `out_root`, and writes
/// [`FILE_NAME`] into `out_root` unless it already holds those bytes. With a `cache`, each mesh's and
/// material's audit is kept there, and so is the whole report.
pub fn write(
    source_root: &Path,
    out_root: &Path,
    cook: Cook,
    cache: Option<&Path>,
    report: &CookReport,
) -> Result<Audited, AuditError> {
    let cache = cache.map(Cache::new);
    let audit = whole(source_root, out_root, cook, cache.as_ref(), report)?;
    let path = out_root.join(FILE_NAME);
    let text = audit.to_ron().map_err(|error| AuditError::Invalid {
        path: path.clone(),
        why: format!("the report doesn't write as RON: {error}"),
    })?;
    if fs::read(&path).is_ok_and(|bytes| bytes == text.as_bytes()) {
        return Ok(Audited { path, written: Written::UpToDate, audit });
    }
    cook::write_atomically(&path, text.as_bytes())
        .map_err(|source| AuditError::Write { path: path.clone(), source: Box::new(source) })?;
    Ok(Audited { path, written: Written::Written, audit })
}

/// The whole report, from the cache where it has it.
fn whole(
    source_root: &Path,
    out_root: &Path,
    cook: Cook,
    cache: Option<&Cache>,
    report: &CookReport,
) -> Result<CookAudit, AuditError> {
    let names = Names { source_root, out_root };
    let mut assets: Vec<(AssetId, CacheKey, String, String)> = report
        .assets
        .iter()
        .map(|asset| (asset.id, asset.key, names.source(&asset.source), names.output(&asset.output)))
        .collect();
    assets.sort();
    let key = CacheKey::audit(AUDITOR, "whole", &(cook.name(), assets));
    cached(cache, key, || audit(source_root, out_root, cook, cache, report))
}

/// Relative names, with `/` between their parts on every host.
struct Names<'a> {
    source_root: &'a Path,
    out_root: &'a Path,
}

impl Names<'_> {
    fn source(&self, path: &Path) -> String {
        relative(path, self.source_root)
    }

    fn output(&self, path: &Path) -> String {
        relative(path, self.out_root)
    }
}

/// `path` under `root`, with `/` between its parts on every host: how the audit and the asset index name
/// sources and cooked files.
pub(crate) fn relative(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// The audit of the cook `report` describes: see the [module docs](self).
pub fn audit(
    source_root: &Path,
    out_root: &Path,
    cook: Cook,
    cache: Option<&Cache>,
    report: &CookReport,
) -> Result<CookAudit, AuditError> {
    let names = Names { source_root, out_root };
    let resolved = cook::resolve_tree(source_root)
        .map_err(|source| AuditError::Cook { path: source_root.to_owned(), source: Box::new(source) })?;
    let sources: HashMap<AssetId, &SourceAsset> =
        resolved.assets.iter().map(|asset| (asset.meta.id, asset)).collect();
    let cooked: HashMap<AssetId, &CookedAsset> =
        report.assets.iter().map(|asset| (asset.id, asset)).collect();
    let tree = Tree { sources: &sources, cook };

    let (textures, maps) = map_bytes(report, &names)?;

    let material_assets: Vec<(&CookedAsset, &SourceAsset)> = report
        .assets
        .iter()
        .filter_map(|asset| {
            let source = sources.get(&asset.id)?;
            matches!(source.meta.importer, ImportSettings::Material(_)).then_some((asset, *source))
        })
        .collect();
    let material_records = on_workers(&material_assets, |&(asset, source)| {
        cached(cache, CacheKey::audit(AUDITOR, "material", &asset.key), || {
            material_record(asset, source, &tree)
        })
    })?;

    let mesh_assets: Vec<(&CookedAsset, Option<&SourceAsset>)> = report
        .assets
        .iter()
        .filter(|asset| has_extension(&asset.output, AssetKind::Mesh))
        .map(|asset| {
            (asset, sources.get(&asset.id).copied().filter(|source| source.meta.importer.mesh().is_some()))
        })
        .collect();
    let mesh_records = on_workers(&mesh_assets, |&(asset, source)| {
        // A mesh's slots read its materials, whose keys hold their textures'.
        let mut materials: Vec<(AssetId, CacheKey)> = source
            .and_then(|source| source.meta.importer.mesh())
            .map(|settings| {
                settings.materials.values().filter_map(|id| Some((*id, cooked.get(id)?.key))).collect()
            })
            .unwrap_or_default();
        materials.sort();
        let key = CacheKey::audit(AUDITOR, "mesh", &(asset.key, materials));
        cached(cache, key, || mesh_record(asset, source, &tree))
    })?;

    let name_of = |id: AssetId| sources.get(&id).map(|source| names.source(&source.path));
    let mut alpha = AlphaCensus::default();
    let mut materials = Vec::new();
    for ((asset, _), record) in material_assets.iter().zip(material_records) {
        let source = names.source(&asset.source);
        alpha.count(&record.family, record.alpha, &source);
        let character = record.character.map(|character| CharacterAudit {
            surface: character.surface,
            regions: character.regions,
            see_through: character.see_through,
            control: character.control.map(|(id, channels)| channels.named(name_of(id))),
            layer_mask: character.layer_mask.map(|(id, channels)| channels.named(name_of(id))),
        });
        materials.push(MaterialAudit {
            source,
            family: record.family,
            alpha: record.alpha.name().to_owned(),
            character,
        });
    }
    let mut meshes = Vec::new();
    for ((asset, _), record) in mesh_assets.iter().zip(mesh_records) {
        let source = names.source(&asset.source);
        for (slot, alpha_kind) in &record.generated {
            alpha.count(MaterialFamily::AnimeCharacter.name(), *alpha_kind, &format!("{source}#{slot}"));
        }
        let slots = record
            .slots
            .into_iter()
            .map(|slot| SlotAudit {
                slot: slot.slot,
                material: slot.material.and_then(name_of),
                control: slot.control.and_then(name_of),
                triangles: slot.triangles,
                split: slot.split,
                disagreeing: slot.disagreeing,
                straddling: slot.straddling,
            })
            .collect();
        meshes.push(MeshAudit {
            source,
            output: names.output(&asset.output),
            levels: record.levels,
            chain: record.chain,
            slots,
        });
    }
    materials.sort_by(|a, b| a.source.cmp(&b.source));
    meshes.sort_by(|a, b| (&a.source, &a.output).cmp(&(&b.source, &b.output)));
    alpha.alpha_tested.sort();
    alpha.blended.sort();
    Ok(CookAudit { cook: cook.name().to_owned(), textures, alpha, maps, materials, meshes })
}

/// What the audit reads beside each asset: every source by ID, and the cook.
struct Tree<'a> {
    sources: &'a HashMap<AssetId, &'a SourceAsset>,
    cook: Cook,
}

fn has_extension(path: &Path, kind: AssetKind) -> bool {
    path.extension().is_some_and(|extension| extension == kind.extension())
}

/// Each texture's bytes, and their sums by class.
fn map_bytes(
    report: &CookReport,
    names: &Names<'_>,
) -> Result<(BTreeMap<String, ClassBytes>, Vec<MapBytes>), AuditError> {
    let mut by_texture: BTreeMap<AssetId, MapBytes> = BTreeMap::new();
    for asset in &report.assets {
        let Some(of) = asset.texture else { continue };
        let bytes = fs::metadata(&asset.output)
            .map_err(|source| AuditError::Read { path: asset.output.clone(), source })?
            .len();
        let map = by_texture.entry(of.texture).or_insert_with(|| MapBytes {
            source: names.source(&asset.source),
            output: String::new(),
            class: of.class.name().to_owned(),
            bytes: 0,
            levels: 0,
        });
        map.bytes += bytes;
        if asset.id == of.texture {
            map.output = names.output(&asset.output);
        } else {
            map.levels += 1;
        }
    }
    let mut maps: Vec<MapBytes> = by_texture.into_values().collect();
    maps.sort_by(|a, b| (&a.source, &a.output).cmp(&(&b.source, &b.output)));
    let mut classes: BTreeMap<String, ClassBytes> = BTreeMap::new();
    for map in &maps {
        let class = classes.entry(map.class.clone()).or_default();
        class.maps += 1;
        class.bytes += map.bytes;
    }
    Ok((classes, maps))
}

/// How a material treats alpha, as its cooked flags say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum AlphaKind {
    Opaque,
    AlphaTested,
    Blended,
}

impl AlphaKind {
    fn of(flags: MaterialFlags) -> Self {
        if flags.contains(MaterialFlags::TRANSPARENT) {
            Self::Blended
        } else if flags.contains(MaterialFlags::ALPHA_TEST) {
            Self::AlphaTested
        } else {
            Self::Opaque
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Opaque => "opaque",
            Self::AlphaTested => "alpha-tested",
            Self::Blended => "blended",
        }
    }
}

impl AlphaCensus {
    fn count(&mut self, family: &str, alpha: AlphaKind, name: &str) {
        let counts = self.families.entry(family.to_owned()).or_default();
        match alpha {
            AlphaKind::Opaque => counts.opaque += 1,
            AlphaKind::AlphaTested => {
                counts.alpha_tested += 1;
                self.alpha_tested.push(name.to_owned());
            }
            AlphaKind::Blended => {
                counts.blended += 1;
                self.blended.push(name.to_owned());
            }
        }
    }
}

/// `job` over every item, on up to [`WORKERS`] threads, its results in the items' order: the first
/// failure, by the items' order, if any fails.
fn on_workers<T: Sync, R: Send>(
    items: &[T],
    job: impl Fn(&T) -> Result<R, AuditError> + Sync,
) -> Result<Vec<R>, AuditError> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result<R, AuditError>>>> = Mutex::new(items.iter().map(|_| None).collect());
    let work = || {
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(item) = items.get(index) else { break };
            let result = job(item);
            results.lock().unwrap_or_else(PoisonError::into_inner)[index] = Some(result);
        }
    };
    let workers = thread::available_parallelism().map_or(1, usize::from).min(WORKERS).min(items.len());
    thread::scope(|scope| {
        for _ in 0..workers {
            thread::Builder::new()
                .stack_size(8 << 20)
                .spawn_scoped(scope, work)
                .expect("the system starts an audit worker");
        }
    });
    results
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .into_iter()
        .map(|result| result.expect("every item ran"))
        .collect()
}

/// The record `key` holds in `cache`, or `make`'s, kept there.
fn cached<R: Serialize + DeserializeOwned>(
    cache: Option<&Cache>,
    key: CacheKey,
    make: impl FnOnce() -> Result<R, AuditError>,
) -> Result<R, AuditError> {
    if let Some(cache) = cache
        && let Some(bytes) = cache.get(&key)?
        && let Ok(record) = sapphire_serialize::postcard::from_bytes(&bytes)
    {
        return Ok(record);
    }
    let record = make()?;
    if let Some(cache) = cache {
        let bytes =
            sapphire_serialize::postcard::to_allocvec(&record).expect("an audit record always encodes");
        cache.put(&key, &bytes)?;
    }
    Ok(record)
}

/// One material document's audit, with maps by ID, as the cache keeps it.
#[derive(Serialize, Deserialize)]
struct MaterialRecord {
    family: String,
    alpha: AlphaKind,
    character: Option<CharacterRecord>,
}

#[derive(Serialize, Deserialize)]
struct CharacterRecord {
    surface: String,
    regions: u32,
    see_through: u32,
    control: Option<(AssetId, Channels)>,
    layer_mask: Option<(AssetId, Channels)>,
}

/// [`MapChannels`] without the map's name.
#[derive(Serialize, Deserialize)]
struct Channels {
    width: u32,
    height: u32,
    regions: Vec<RegionTexels>,
    channels: Vec<ChannelAudit>,
}

impl Channels {
    fn named(self, source: Option<String>) -> MapChannels {
        MapChannels {
            source: source.unwrap_or_default(),
            width: self.width,
            height: self.height,
            regions: self.regions,
            channels: self.channels,
        }
    }
}

fn material_record(
    asset: &CookedAsset,
    source: &SourceAsset,
    tree: &Tree<'_>,
) -> Result<MaterialRecord, AuditError> {
    let path = &source.path;
    let bytes = fs::read(path).map_err(|error| AuditError::Read { path: path.clone(), source: error })?;
    let document = material::read(&bytes)
        .map_err(|error| AuditError::Invalid { path: path.clone(), why: error.to_string() })?;
    let cooked = cook::read_cooked(asset)
        .map_err(|error| AuditError::Read { path: asset.output.clone(), source: error })?;
    let flags = Material::read(&cooked)
        .map_err(|error| AuditError::Invalid { path: asset.output.clone(), why: error.to_string() })?
        .flags();
    let character = match &document {
        AnyMaterialDocument::AnimeCharacter(character) => Some(character_record(character, tree)?),
        _ => None,
    };
    Ok(MaterialRecord { family: document.family().name().to_owned(), alpha: AlphaKind::of(flags), character })
}

fn character_record(
    material: &AnimeCharacterMaterial,
    tree: &Tree<'_>,
) -> Result<CharacterRecord, AuditError> {
    let control =
        material.control_map.map(|id| Ok::<_, AuditError>((id, source_image(id, tree)?))).transpose()?;
    let regions = control.as_ref().and_then(|(_, image)| image.as_ref()).map(Regions::of);
    let control_channels = control.as_ref().and_then(|(id, image)| {
        let image = image.as_ref()?;
        Some((*id, channels(image, regions.as_ref(), &[0, 1, 2])))
    });
    let layer_mask = match material.layer_mask {
        Some(id) => {
            source_image(id, tree)?.map(|image| (id, channels(&image, regions.as_ref(), &[0, 1, 2, 3])))
        }
        None => None,
    };
    let surface = match material.surface {
        CharacterSurface::Body => "body",
        CharacterSurface::Face(_) => "face",
        CharacterSurface::Hair(_) => "hair",
        CharacterSurface::Eye(_) => "eye",
    };
    Ok(CharacterRecord {
        surface: surface.to_owned(),
        regions: material.regions.len() as u32,
        see_through: material.regions.iter().filter(|region| region.sees_through()).count() as u32,
        control: control_channels,
        layer_mask,
    })
}

/// The image the texture `id` cooks from, as its source makes it: `None` for an asset that isn't a
/// texture source of this tree.
fn source_image(id: AssetId, tree: &Tree<'_>) -> Result<Option<SourceImage>, AuditError> {
    let Some(source) = tree.sources.get(&id) else { return Ok(None) };
    let path = &source.path;
    let invalid = |why: String| AuditError::Invalid { path: path.clone(), why };
    let read = || fs::read(path).map_err(|error| AuditError::Read { path: path.clone(), source: error });
    Ok(match &source.meta.importer {
        ImportSettings::Texture(settings) => {
            Some(texture::import(path, &read()?, settings).map_err(|error| invalid(error.to_string()))?)
        }
        ImportSettings::CharacterMap(settings, _) => Some(
            character_maps::generate(path, &read()?, settings)
                .map_err(|error| invalid(error.to_string()))?
                .image,
        ),
        ImportSettings::ProceduralTexture(settings) => Some(
            ProceduralTexture::read(&read()?)
                .map_err(|error| invalid(error.to_string()))?
                .rasterize(settings.map),
        ),
        _ => None,
    })
}

/// The material ID of each texel of a control map, from its alpha.
struct Regions {
    width: u32,
    height: u32,
    ids: Vec<u8>,
}

impl Regions {
    fn of(control: &SourceImage) -> Self {
        let ids = control.pixels.iter().map(|pixel| ilm::decode_material_id(pixel[3]).get()).collect();
        Self { width: control.width, height: control.height, ids }
    }

    /// The ID at texel `(x, y)` of a map `width` × `height` laid over the control map, by nearest texel.
    fn at(&self, x: u32, y: u32, width: u32, height: u32) -> u8 {
        let x = (u64::from(x) * u64::from(self.width) / u64::from(width)) as usize;
        let y = (u64::from(y) * u64::from(self.height) / u64::from(height)) as usize;
        self.ids[y * self.width as usize + x]
    }
}

/// `image`'s `channels` (0 to 3 for R to A), each by `regions` where there are some.
fn channels(image: &SourceImage, regions: Option<&Regions>, channels: &[usize]) -> Channels {
    let (width, height) = (image.width, image.height);
    let region_of = |index: usize| {
        regions.map(|regions| regions.at(index as u32 % width, index as u32 / width, width, height))
    };
    let mut texels: BTreeMap<u8, u32> = BTreeMap::new();
    if regions.is_some() {
        for index in 0..image.pixels.len() {
            *texels.entry(region_of(index).expect("with regions")).or_default() += 1;
        }
    }
    let audits = channels
        .iter()
        .map(|&channel| {
            let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
            let mut ranges: BTreeMap<u8, (f32, f32)> = BTreeMap::new();
            for (index, pixel) in image.pixels.iter().enumerate() {
                let value = pixel[channel];
                (min, max) = (min.min(value), max.max(value));
                if let Some(id) = region_of(index) {
                    let range = ranges.entry(id).or_insert((value, value));
                    *range = (range.0.min(value), range.1.max(value));
                }
            }
            let shape = if max - min <= SAME_VALUE {
                ChannelShape::Constant
            } else if !ranges.is_empty() && ranges.values().all(|(min, max)| max - min <= SAME_VALUE) {
                ChannelShape::PerRegion
            } else {
                ChannelShape::Varies
            };
            ChannelAudit {
                channel: ["r", "g", "b", "a"][channel].to_owned(),
                shape,
                min,
                max,
                regions: ranges.into_iter().map(|(id, (min, max))| RegionRange { id, min, max }).collect(),
            }
        })
        .collect();
    Channels {
        width,
        height,
        regions: texels.into_iter().map(|(id, texels)| RegionTexels { id, texels }).collect(),
        channels: audits,
    }
}

/// One mesh's audit, with materials by ID, as the cache keeps it.
#[derive(Serialize, Deserialize)]
struct MeshRecord {
    levels: Vec<CookedLevel>,
    chain: Option<ChainAudit>,
    slots: Vec<SlotRecord>,
    /// The importer's own materials, for slots the sidecar doesn't bind, by slot name.
    generated: Vec<(String, AlphaKind)>,
}

#[derive(Serialize, Deserialize)]
struct SlotRecord {
    slot: String,
    material: Option<AssetId>,
    control: Option<AssetId>,
    triangles: u32,
    split: u32,
    disagreeing: Option<u32>,
    straddling: Option<u32>,
}

fn mesh_record(
    asset: &CookedAsset,
    source: Option<&SourceAsset>,
    tree: &Tree<'_>,
) -> Result<MeshRecord, AuditError> {
    let levels = cooked_levels(asset)?;
    // A source's own mesh: its cells and other parts aren't what its importer reads.
    let Some(source) = source.filter(|source| source.meta.id == asset.id) else {
        return Ok(MeshRecord { levels, chain: None, slots: Vec::new(), generated: Vec::new() });
    };
    let path = &source.path;
    let settings = source.meta.importer.mesh().expect("filtered on mesh importers");
    let cook_error = |error| AuditError::Cook { path: path.clone(), source: Box::new(error) };
    let bytes = fs::read(path).map_err(|error| AuditError::Read { path: path.clone(), source: error })?;
    let mut mesh = cook::guarded(path, || {
        let mut mesh = cook::imported_mesh(path, &bytes, &source.meta.importer, tree.cook)?;
        cook::validate_mesh_inputs(path, &mesh)?;
        Ok(mesh_as_optimized(&mut mesh, settings, path).map(|()| mesh))
    })
    .map_err(cook_error)??;
    let slots = slot_records(&mesh, &settings.materials, tree)?;
    let generated = mesh
        .materials
        .iter()
        .filter(|slot| !settings.materials.contains_key(&slot.name))
        .map(|slot| {
            let document = AnyMaterialDocument::AnimeCharacter(slot.generated.clone());
            let slots = document.family().texture_slots().len();
            let flags = document
                .cook(&vec![Swizzle::IDENTITY; slots])
                .map(|material| MaterialFlags::from_bits(material.flags).unwrap_or_default())
                .unwrap_or_default();
            (slot.name.clone(), AlphaKind::of(flags))
        })
        .collect();
    let chain = optimize::audit_chain(&mut mesh, &settings.lods, settings.outline.weld_tolerance(), &[]);
    Ok(MeshRecord { levels, chain: Some(chain), slots, generated })
}

/// Bakes `mesh` as the cook does before its chain: outlines, and foliage where the sidecar asks.
fn mesh_as_optimized(
    mesh: &mut ImportedMesh,
    settings: &crate::meta::MeshSettings,
    path: &Path,
) -> Result<(), AuditError> {
    bake::outline_normals(mesh, &settings.outline);
    if let Some(settings_foliage) = &settings.foliage {
        foliage::bake(mesh, settings_foliage, settings.outline.weld_tolerance())
            .map_err(|error| AuditError::Invalid { path: path.to_owned(), why: error.to_string() })?;
    }
    Ok(())
}

/// Each cooked level of the mesh `asset`.
fn cooked_levels(asset: &CookedAsset) -> Result<Vec<CookedLevel>, AuditError> {
    let bytes = cook::read_cooked(asset)
        .map_err(|error| AuditError::Read { path: asset.output.clone(), source: error })?;
    let mesh = Mesh::from_cooked(&bytes)
        .map_err(|error| AuditError::Invalid { path: asset.output.clone(), why: error.to_string() })?;
    Ok(mesh
        .lods
        .iter()
        .map(|lod| {
            let mut used: Vec<u32> = lod
                .submeshes
                .iter()
                .flat_map(|submesh| {
                    let range =
                        submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
                    mesh.indices[range].iter().copied()
                })
                .collect();
            let triangles = used.len() / 3;
            used.sort_unstable();
            used.dedup();
            CookedLevel { triangles: triangles as u32, vertices: used.len() as u32, error: lod.error }
        })
        .collect())
}

/// Each material slot of `mesh`'s LOD0, with the material `bindings` gives it and how its triangles agree
/// with that material's control map.
fn slot_records(
    mesh: &ImportedMesh,
    bindings: &BTreeMap<String, AssetId>,
    tree: &Tree<'_>,
) -> Result<Vec<SlotRecord>, AuditError> {
    let mut records = Vec::with_capacity(mesh.materials.len());
    for (index, slot) in mesh.materials.iter().enumerate() {
        let material = bindings.get(&slot.name).copied();
        let control = match material.and_then(|id| tree.sources.get(&id)) {
            Some(source) if matches!(source.meta.importer, ImportSettings::Material(_)) => {
                let bytes = fs::read(&source.path)
                    .map_err(|error| AuditError::Read { path: source.path.clone(), source: error })?;
                match material::read(&bytes) {
                    Ok(AnyMaterialDocument::AnimeCharacter(character)) => character.control_map,
                    _ => None,
                }
            }
            _ => None,
        };
        let regions = match control {
            Some(id) => source_image(id, tree)?.as_ref().map(Regions::of),
            None => None,
        };
        let triangles: Vec<[u32; 3]> = mesh.lods[0]
            .submeshes
            .iter()
            .filter(|submesh| submesh.material as usize == index)
            .flat_map(|submesh| {
                let range =
                    submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
                mesh.indices[range].as_chunks::<3>().0.to_vec()
            })
            .collect();
        let agreement = agreement(mesh, &triangles, regions.as_ref());
        records.push(SlotRecord {
            slot: slot.name.clone(),
            material,
            control,
            triangles: triangles.len() as u32,
            split: agreement.split,
            disagreeing: regions.is_some().then_some(agreement.disagreeing),
            straddling: regions.is_some().then_some(agreement.straddling),
        });
    }
    records.sort_by(|a, b| a.slot.cmp(&b.slot));
    Ok(records)
}

/// How a slot's triangles agree with their region IDs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Agreement {
    /// Triangles whose vertices hold different IDs.
    split: u32,
    /// Triangles split, or whose ID differs from the control map's at a texel they cover.
    disagreeing: u32,
    /// Triangles covering the control map's texels of more than one ID.
    straddling: u32,
}

/// How `triangles` of `mesh` agree among their vertices and with `regions`, if there is a control map: a
/// triangle covers each texel whose centre lies in it in UV space (edges included, UVs wrapping as a
/// repeating sampler's do), or, covering none, the texel under its centroid.
fn agreement(mesh: &ImportedMesh, triangles: &[[u32; 3]], regions: Option<&Regions>) -> Agreement {
    let mut agreement = Agreement::default();
    for triangle in triangles {
        let vertices = triangle.map(|index| &mesh.vertices[index as usize]);
        let ids = vertices.map(|vertex| vertex.flags.bits() & VertexFlags::MATERIAL_ID_MASK);
        let split = ids[0] != ids[1] || ids[1] != ids[2];
        agreement.split += u32::from(split);
        let covered = regions.map_or(0, |regions| covered_ids(regions, vertices.map(|vertex| vertex.uv)));
        let disagrees = split || covered & !(1 << ids[0]) != 0;
        agreement.disagreeing += u32::from(disagrees);
        agreement.straddling += u32::from(covered.count_ones() > 1);
    }
    agreement
}

/// The IDs of `regions` a triangle at `uvs` covers, a bit each.
fn covered_ids(regions: &Regions, uvs: [[f32; 2]; 3]) -> u8 {
    let (width, height) = (regions.width as f32, regions.height as f32);
    let points = uvs.map(|[u, v]| [u * width, v * height]);
    let id_at = |x: i64, y: i64| {
        let x = x.rem_euclid(i64::from(regions.width)) as usize;
        let y = y.rem_euclid(i64::from(regions.height)) as usize;
        regions.ids[y * regions.width as usize + x]
    };
    // Twice the signed area of (a, b, c).
    let edge =
        |a: [f32; 2], b: [f32; 2], c: [f32; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let area = edge(points[0], points[1], points[2]);
    let lowest = |axis: usize| points.iter().map(|point| point[axis]).fold(f32::INFINITY, f32::min);
    let highest = |axis: usize| points.iter().map(|point| point[axis]).fold(f32::NEG_INFINITY, f32::max);
    let (x0, x1) = ((lowest(0) - 0.5).ceil() as i64, (highest(0) - 0.5).floor() as i64);
    let (y0, y1) = ((lowest(1) - 0.5).ceil() as i64, (highest(1) - 0.5).floor() as i64);
    let mut covered = 0u8;
    if area != 0.0 {
        for y in y0..=y1 {
            for x in x0..=x1 {
                let centre = [x as f32 + 0.5, y as f32 + 0.5];
                let weights = [
                    edge(points[1], points[2], centre),
                    edge(points[2], points[0], centre),
                    edge(points[0], points[1], centre),
                ];
                if weights.iter().all(|&weight| weight * area >= 0.0) {
                    covered |= 1 << id_at(x, y);
                }
            }
        }
    }
    if covered == 0 {
        let centroid = [0, 1].map(|axis| points.iter().map(|point| point[axis]).sum::<f32>() / 3.0);
        covered = 1 << id_at(centroid[0].floor() as i64, centroid[1].floor() as i64);
    }
    covered
}

#[cfg(test)]
mod tests;
