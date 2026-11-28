//! The asset index (M6 C2): `index.json`, written beside a cook's loose cooked files, where the cook audit
//! is, and never into a pack or beside the packs an app ships. It says, for every asset the cook made, what
//! it is and what it costs, and what it uses and what uses it, so a tool (the editor's browser, `sapphire_pipeline
//! graph`) can answer "what uses this texture" or "what does this character cost" without cooking or reading
//! a pack.
//!
//! - **Every asset**, by ID: its kind, its source and cooked file (relative to the cook's roots), whether it's
//!   a part its source cooks beside its own asset (a texture's level, a model's clip, a zone's cell), the
//!   pack it goes in, its cooked file's bytes, and its dependencies both ways: what it reaches as packing
//!   reads it ([`pack::layout`]: a cooked asset's references, what it names for presentation, a texture's
//!   levels), and what reaches it. A texture's level is part of its texture and never uses it: packing has a
//!   level reach back to its texture, as the level's bytes name it, but the index keeps only the texture's
//!   edge to its levels, so what uses a texture is what binds it. An ID `uses` names that isn't an asset of
//!   the index is one this cook doesn't hold, which packing refuses.
//! - **A texture:** its class, cooked format (as the KTX2 file's `VkFormat`), size and levels in this cook, how
//!   many of its levels are assets of their own, its bytes with every level, and its **thumbnail**: a level
//!   the cook already wrote, the first of its tail's levels of [`TAIL_SIZE`] or less a side, named by its byte
//!   range in the texture's cooked file. Nothing is decoded or encoded for it: a reader uploads the blocks
//!   as the renderer would.
//! - **A mesh:** each cooked level's triangles, vertices and error, the skin's joints, its material slots as
//!   cooked (after its merges), and the simplifier's locks and its slots' region agreement.
//! - **A material document:** its family, how it treats alpha and what its maps hold.
//! - **A source's own asset:** `sapphire_pipeline check`'s results for that source: the role its sidecar names,
//!   its numbers against the role's row on both cooks, and every finding, waived ones with their reasons.
//!
//! What the cook audit measures (a texture's bytes and class, a mesh's levels and locks, a material's maps) is
//! the audit's own record, taken from it ([`audit`]) and never measured again; the index adds what the audit
//! doesn't say. A cook never judges a source by the check: the index records it, as the cook report lists
//! waivers.
//!
//! **Bytes per cook and tier:** an index is one cook's, and the tiers it names are the ones that draw that
//! cook. No tier picks cooked bytes: every tier of a cook mounts the same files, and which levels of a texture
//! are resident is the streamer's choice, under the tier's pool, at run time. A texture's tail (its cooked
//! file's bytes) is what stays resident when every level above it is let go.
//!
//! **Determinism:** sources and outputs are named relative to the cook's roots, with `/` between their parts,
//! every map and list is sorted, and nothing holds a time, so a second cook, or a cook of the same tree checked
//! out elsewhere, writes it byte for byte. The whole index is kept in the content cache under a key over the
//! stages whose output it holds ([`INDEXER`], [`audit::AUDITOR`], [`check::CHECKER`], [`pack::PACKER`] and
//! packing's reach), the check's rows ([`check::ROWS`]), every asset's cook key and cooked bytes and every
//! sidecar (whose `budget` no cook key holds), so a cook that changed nothing
//! reads one entry. A cook that changed something checks its tree again, beside the cook ([`check::Kept`]): a
//! source's measurements are kept under its cook key, so only what changed is measured again.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::mesh::Mesh;
use sapphire_assets::texture::{TAIL_SIZE, TextureTail};
use sapphire_assets::{AssetId, AssetKind};
use serde::{Deserialize, Serialize};

use crate::audit::{self, CookAudit, CookedLevel, MaterialAudit, SlotAudit};
use crate::cache::{Cache, CacheError, CacheKey, Stage};
use crate::check::{self, CheckError, Severity};
use crate::cook::{self, Cook, CookError, CookReport, CookedAsset};
use crate::optimize::ChainAudit;
use crate::pack::{self, PackError};

/// The index's file name, beside a cook's loose cooked files.
pub const FILE_NAME: &str = "index.json";

/// The index's format, which [`AssetIndex::version`] holds: bump it whenever a field changes, a field of the
/// audit's records it embeds included ([`MaterialAudit`], [`ChainAudit`], [`SlotAudit`], [`CookedLevel`]).
pub const VERSION: u32 = 1;

/// The indexer, as cache keys know it. Bump its version whenever the index changes for the same cook. What
/// it holds of the audit's, the check's and packing's is keyed by theirs ([`audit::AUDITOR`];
/// [`check::CHECKER`] and the check's [`check::ROWS`]; [`pack::PACKER`] for each entry's pack, and packing's
/// reach for what an entry uses and what uses it).
pub const INDEXER: Stage = Stage { name: "asset index", version: 2 };

/// The stages whose output an index holds, as its key has them.
const STAGES: [Stage; 5] = [INDEXER, audit::AUDITOR, check::CHECKER, pack::PACKER, pack::REACH];

/// What `index.json` holds: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetIndex {
    /// [`VERSION`].
    pub version: u32,
    /// `desktop` or `mobile`.
    pub cook: String,
    /// The tiers that draw this cook, by their names in `tiers.ron`.
    pub tiers: Vec<String>,
    /// Assets and their cooked bytes by kind.
    pub kinds: BTreeMap<String, Totals>,
    /// Assets and their cooked bytes by the pack they go in (cooked, not as the pack compresses them).
    pub packs: BTreeMap<String, Totals>,
    pub check: CheckSummary,
    /// Every asset the cook made.
    pub assets: BTreeMap<AssetId, IndexedAsset>,
}

/// How many assets, and their cooked files' bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Totals {
    pub assets: u32,
    pub bytes: u64,
}

/// The check of the cook's source tree, in sum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckSummary {
    /// No error goes unwaived: what `sapphire_pipeline check` passes on.
    pub passes: bool,
    /// Errors no sidecar waives.
    pub errors: u32,
    /// Warnings no sidecar waives.
    pub warnings: u32,
    pub waived: u32,
    /// Meshes judged at their roles.
    pub roles: u32,
}

/// One asset of the index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedAsset {
    /// Its kind's name (`texture`, `mesh`, `material`, …).
    pub kind: String,
    /// Its source, under the source root.
    pub source: String,
    /// Its cooked file, under the cook's output root.
    pub output: String,
    /// What its author calls it: a clip's name, or its source's file name up to the first dot.
    pub name: String,
    /// Whether it's a part its source cooks beside the asset its sidecar names: a texture's level, a model's
    /// clip, a zone's cell.
    pub part: bool,
    /// The pack it goes in.
    pub pack: String,
    /// Its cooked file's bytes.
    pub bytes: u64,
    /// What it reaches, as packing reads it, by ID.
    pub uses: Vec<AssetId>,
    /// What reaches it, by ID.
    pub used_by: Vec<AssetId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<TextureEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshEntry>,
    /// A material document's audit, as the cook audit records it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<MaterialAudit>,
    /// The check's results for its source, on a source's own asset that the check reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckEntry>,
}

/// A texture: its own asset, which is its tail where it's split into levels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureEntry {
    /// `color`, `normal`, `mask`, `ramp`, `ILM`, `SDF` or `HDR`.
    pub class: String,
    /// The cooked format, as the KTX2 header's `VkFormat`.
    pub vk_format: u32,
    /// Level 0's size in this cook.
    pub width: u32,
    pub height: u32,
    /// Its levels, down to the last.
    pub levels: u32,
    /// How many of its top levels are assets of their own, above the tail.
    pub split: u32,
    /// Its cooked files' bytes: the tail and every level above it.
    pub bytes: u64,
    pub thumbnail: Thumbnail,
}

/// A level already cooked into a texture's file, small enough to show as its thumbnail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thumbnail {
    /// Which of the texture's levels it is, 0 the largest.
    pub level: u32,
    pub width: u32,
    pub height: u32,
    /// Where its blocks lie in the texture's cooked file ([`IndexedAsset::output`]).
    pub offset: u64,
    pub bytes: u64,
}

/// A cooked mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshEntry {
    /// Each cooked level, LOD0 first, as the cook audit records it.
    pub levels: Vec<CookedLevel>,
    /// The skin's joints, secondary joints included; 0 for a mesh with no skin.
    pub joints: u32,
    /// Its material slots as cooked, after its merges, by name.
    pub slots: Vec<String>,
    /// The simplifier's locks and the chain built without them, for a mesh cooked from a mesh source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainAudit>,
    /// Each slot of LOD0 as imported, before merges, with how its triangles agree with its control map.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imported_slots: Vec<SlotAudit>,
}

/// What `sapphire_pipeline check` says of a source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckEntry {
    /// The role its sidecar names, if any.
    pub role: Option<String>,
    /// A role mesh's numbers on each cook, desktop then mobile, against its row.
    pub cooks: Vec<RoleCook>,
    /// Every finding about it, waived ones too.
    pub findings: Vec<CheckFinding>,
}

/// A role mesh on one cook, against its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleCook {
    pub cook: CookName,
    pub triangles: u32,
    pub joints: u32,
    pub slots: u32,
    /// The longest side of its largest map.
    pub map: u32,
    pub row: RoleRow,
}

/// A role's limits on one cook ([`check::ROWS`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleRow {
    pub triangles: u32,
    pub joints: u32,
    pub slots: u32,
    pub map: u32,
}

/// One finding of the check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckFinding {
    /// The rule's name (`triangles`, `uncapped-map`, …).
    pub rule: String,
    /// `error` or `warning`.
    pub severity: String,
    pub cook: Option<CookName>,
    pub message: String,
    /// The waiver's reason, where the sidecar waives the rule.
    pub waived: Option<String>,
}

/// A cook, by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CookName {
    Desktop,
    Mobile,
}

impl From<Cook> for CookName {
    fn from(cook: Cook) -> Self {
        match cook {
            Cook::Desktop => Self::Desktop,
            Cook::Mobile => Self::Mobile,
        }
    }
}

/// An index that couldn't be made or read.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("indexing {}", path.display())]
    Cook {
        path: PathBuf,
        #[source]
        source: Box<CookError>,
    },
    #[error("indexing the cook")]
    Pack(#[source] Box<PackError>),
    #[error("indexing the cook's check")]
    Check(#[source] Box<CheckError>),
    #[error("indexing {}: failed to read it", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("indexing {}: {why}", path.display())]
    Invalid { path: PathBuf, why: String },
    #[error("{} isn't an asset index this pipeline reads (version {VERSION})", path.display())]
    Unreadable {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error("writing {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: Box<CookError>,
    },
}

/// The tiers that draw `cook`, by their names in `tiers.ron`.
pub fn tiers(cook: Cook) -> [&'static str; 2] {
    match cook {
        Cook::Desktop => ["desktop-high", "desktop-low"],
        Cook::Mobile => ["mobile-high", "mobile-low"],
    }
}

/// What [`write()`] did: the index's path, whether it wrote it, and, unless the whole index came from the
/// cache, what its check measured anew and what it took from the cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Indexed {
    pub path: PathBuf,
    pub written: audit::Written,
    pub checked: Option<check::Work>,
}

/// Indexes the cook `report` describes, of the sources under `source_root` into `out_root`, given its
/// `audit`, and writes [`FILE_NAME`] into `out_root` unless it already holds those bytes. With a `cache`, the
/// whole index is kept there, and so is what its check measures of each source.
pub fn write(
    source_root: &Path,
    out_root: &Path,
    cook: Cook,
    cache: Option<&Path>,
    report: &CookReport,
    audit: &CookAudit,
) -> Result<Indexed, IndexError> {
    let (text, checked) = index_text(source_root, out_root, cook, cache, report, audit)?;
    let path = out_root.join(FILE_NAME);
    if fs::read(&path).is_ok_and(|bytes| bytes == text.as_bytes()) {
        return Ok(Indexed { path, written: audit::Written::UpToDate, checked });
    }
    cook::write_atomically(&path, text.as_bytes())
        .map_err(|source| IndexError::Write { path: path.clone(), source: Box::new(source) })?;
    Ok(Indexed { path, written: audit::Written::Written, checked })
}

/// The key the whole index is kept under: the `stages` whose output it holds, the check's rows of limits
/// (so that a changed row misses it with no bump to forget), and what it reads.
fn key<A: Serialize, S: Serialize>(
    stages: &[Stage],
    rows: &[[check::Limits; 2]],
    cook: Cook,
    assets: &A,
    sidecars: &S,
) -> CacheKey {
    CacheKey::index(stages, &(cook.name(), rows, assets, sidecars))
}

/// The index's text, from the cache where it has it, and what its check measured if it ran.
fn index_text(
    source_root: &Path,
    out_root: &Path,
    cook: Cook,
    cache: Option<&Path>,
    report: &CookReport,
    audit: &CookAudit,
) -> Result<(String, Option<check::Work>), IndexError> {
    let resolved = cook::resolve_tree(source_root)
        .map_err(|source| IndexError::Cook { path: source_root.to_owned(), source: Box::new(source) })?;
    let cache_dir = cache;
    let cache = cache.map(Cache::new);
    let mut assets: Vec<(AssetId, CacheKey, [u8; blake3::OUT_LEN], String, String)> = report
        .assets
        .iter()
        .map(|asset| {
            (
                asset.id,
                asset.key,
                *asset.output_hash.as_bytes(),
                audit::relative(&asset.source, source_root),
                audit::relative(&asset.output, out_root),
            )
        })
        .collect();
    assets.sort();
    // The check reads every sidecar, and its `budget` is in no cook key.
    let sidecars: Vec<(String, Vec<u8>)> = resolved
        .assets
        .iter()
        .map(|asset| {
            let meta =
                sapphire_serialize::postcard::to_allocvec(&asset.meta).expect("a sidecar always encodes");
            (audit::relative(&asset.path, source_root), meta)
        })
        .collect();
    let key = key(&STAGES, &check::ROWS, cook, &assets, &sidecars);
    if let Some(cache) = &cache
        && let Some(bytes) = cache.get(&key)?
        && let Ok(text) = String::from_utf8(bytes)
    {
        return Ok((text, None));
    }
    let (index, work) = index(source_root, out_root, cook, cache_dir, report, audit, &resolved)?;
    let text = index.to_json(&out_root.join(FILE_NAME))?;
    if let Some(cache) = &cache {
        cache.put(&key, text.as_bytes())?;
    }
    Ok((text, Some(work)))
}

/// The index of the cook `report` describes, and what its check measured anew and took from the `cache`:
/// see the [module docs](self).
pub fn index(
    source_root: &Path,
    out_root: &Path,
    cook: Cook,
    cache: Option<&Path>,
    report: &CookReport,
    audit: &CookAudit,
    resolved: &crate::meta::Resolved,
) -> Result<(AssetIndex, check::Work), IndexError> {
    let layout = pack::layout(report, cache).map_err(|error| IndexError::Pack(Box::new(error)))?;
    let store = cache.map(Cache::new);
    let keys: HashMap<AssetId, CacheKey> = report.assets.iter().map(|asset| (asset.id, asset.key)).collect();
    let kept = store.as_ref().map(|cache| check::Kept { cache, cook, keys: &keys });
    let (checked, work) = check::check_resolved(source_root, resolved, kept)
        .map_err(|error| IndexError::Check(Box::new(error)))?;
    let own: HashMap<AssetId, &crate::meta::SourceAsset> =
        resolved.assets.iter().map(|asset| (asset.meta.id, asset)).collect();

    let maps: HashMap<&str, &audit::MapBytes> =
        audit.maps.iter().map(|map| (map.output.as_str(), map)).collect();
    let meshes: HashMap<&str, &audit::MeshAudit> =
        audit.meshes.iter().map(|mesh| (mesh.output.as_str(), mesh)).collect();
    let materials: HashMap<&str, &MaterialAudit> =
        audit.materials.iter().map(|material| (material.source.as_str(), material)).collect();

    // A texture's level reaches back to its texture, as packing reads it; the index keeps only the
    // texture's edge to the level (see the module docs).
    let owner: HashMap<AssetId, AssetId> = report
        .assets
        .iter()
        .filter_map(|asset| {
            asset.texture.filter(|of| of.texture != asset.id).map(|of| (asset.id, of.texture))
        })
        .collect();
    let uses_of = |from: AssetId| -> Vec<AssetId> {
        let mut uses: Vec<AssetId> = layout
            .reach
            .get(&from)
            .map(|reached| reached.iter().copied().filter(|&to| owner.get(&from) != Some(&to)).collect())
            .unwrap_or_default();
        uses.sort_unstable();
        uses.dedup();
        uses
    };
    let mut used_by: HashMap<AssetId, BTreeSet<AssetId>> = HashMap::new();
    for &from in layout.reach.keys() {
        for to in uses_of(from) {
            used_by.entry(to).or_default().insert(from);
        }
    }

    let mut entries = BTreeMap::new();
    for asset in &report.assets {
        let kind = pack::kind(&asset.output);
        let source = audit::relative(&asset.source, source_root);
        let output = audit::relative(&asset.output, out_root);
        let bytes = fs::metadata(&asset.output)
            .map_err(|error| IndexError::Read { path: asset.output.clone(), source: error })?
            .len();
        let part = !own.contains_key(&asset.id);
        let texture = match asset.texture {
            Some(of) if of.texture == asset.id => {
                Some(texture_entry(asset, maps.get(output.as_str()).copied())?)
            }
            _ => None,
        };
        let mesh = match kind {
            Some(AssetKind::Mesh) => Some(mesh_entry(asset, meshes.get(output.as_str()).copied())?),
            _ => None,
        };
        let material = match kind {
            Some(AssetKind::Material) if !part => {
                materials.get(source.as_str()).map(|&material| material.clone())
            }
            _ => None,
        };
        let check = (!part).then(|| check_entry(&checked, &source, own[&asset.id])).flatten();
        let uses = uses_of(asset.id);
        let entry = IndexedAsset {
            kind: kind.map_or("unknown", AssetKind::name).to_owned(),
            source,
            output,
            name: asset.name.clone(),
            part,
            pack: layout.packs.get(&asset.id).cloned().unwrap_or_default(),
            bytes,
            uses,
            used_by: used_by.get(&asset.id).map(|set| set.iter().copied().collect()).unwrap_or_default(),
            texture,
            mesh,
            material,
            check,
        };
        entries.insert(asset.id, entry);
    }

    let mut kinds: BTreeMap<String, Totals> = BTreeMap::new();
    let mut packs: BTreeMap<String, Totals> = BTreeMap::new();
    for entry in entries.values() {
        for totals in
            [kinds.entry(entry.kind.clone()).or_default(), packs.entry(entry.pack.clone()).or_default()]
        {
            totals.assets += 1;
            totals.bytes += entry.bytes;
        }
    }
    let count =
        |keep: fn(&check::Finding) -> bool| checked.findings.iter().filter(|finding| keep(finding)).count();
    let summary = CheckSummary {
        passes: checked.passes(),
        errors: count(check::Finding::fails) as u32,
        warnings: count(|finding| finding.severity == Severity::Warning && finding.waived.is_none()) as u32,
        waived: count(|finding| finding.waived.is_some()) as u32,
        roles: checked.meshes.len() as u32,
    };
    let index = AssetIndex {
        version: VERSION,
        cook: cook.name().to_owned(),
        tiers: tiers(cook).map(str::to_owned).to_vec(),
        kinds,
        packs,
        check: summary,
        assets: entries,
    };
    Ok((index, work))
}

/// A texture's entry, from its tail's cooked file and the audit's record of its bytes.
fn texture_entry(asset: &CookedAsset, map: Option<&audit::MapBytes>) -> Result<TextureEntry, IndexError> {
    let path = &asset.output;
    let invalid = |why: String| IndexError::Invalid { path: path.clone(), why };
    let map = map.ok_or_else(|| invalid("the cook audit has no record of this texture".to_owned()))?;
    let bytes = fs::read(path).map_err(|source| IndexError::Read { path: path.clone(), source })?;
    let tail = TextureTail::from_ktx2(&bytes).map_err(|error| invalid(error.to_string()))?;
    let whole = tail.whole();
    let desc = tail.texture.desc;
    // The tail's first level of `TAIL_SIZE` or less a side, or its last.
    let in_tail = (0..desc.mip_level_count)
        .find(|&level| {
            let (width, height) = desc.level_extent(level);
            width.max(height) <= TAIL_SIZE
        })
        .unwrap_or(desc.mip_level_count - 1);
    let (width, height) = desc.level_extent(in_tail);
    let blocks = tail.texture.levels[in_tail as usize];
    let thumbnail = Thumbnail {
        level: tail.above + in_tail,
        width,
        height,
        offset: (blocks.as_ptr().addr() - bytes.as_ptr().addr()) as u64,
        bytes: blocks.len() as u64,
    };
    if map.levels != tail.above {
        return Err(invalid(format!(
            "its file says {} levels lie above it, and the cook made {}",
            tail.above, map.levels
        )));
    }
    Ok(TextureEntry {
        class: map.class.clone(),
        vk_format: desc.format.vk_format(),
        width: whole.width,
        height: whole.height,
        levels: whole.mip_level_count,
        split: tail.above,
        bytes: map.bytes,
        thumbnail,
    })
}

/// A mesh's entry, from the audit's record of its levels and its cooked file's skin and slots.
fn mesh_entry(asset: &CookedAsset, audited: Option<&audit::MeshAudit>) -> Result<MeshEntry, IndexError> {
    let path = &asset.output;
    let invalid = |why: String| IndexError::Invalid { path: path.clone(), why };
    let audited = audited.ok_or_else(|| invalid("the cook audit has no record of this mesh".to_owned()))?;
    let bytes = cook::read_cooked(asset).map_err(|source| IndexError::Read { path: path.clone(), source })?;
    let mesh = Mesh::from_cooked(&bytes).map_err(|error| invalid(error.to_string()))?;
    Ok(MeshEntry {
        levels: audited.levels.clone(),
        joints: mesh.skin.as_ref().map_or(0, |skin| skin.joint_count),
        slots: mesh.materials.iter().map(|slot| slot.name.to_string()).collect(),
        chain: audited.chain.clone(),
        imported_slots: audited.slots.clone(),
    })
}

/// What the check says of `source`, whose sidecar is `asset`'s, if it says anything or names a role.
fn check_entry(
    checked: &check::Report,
    source: &str,
    asset: &crate::meta::SourceAsset,
) -> Option<CheckEntry> {
    let role = asset.meta.budget.as_ref().and_then(|budget| budget.role).map(|role| role.name().to_owned());
    let cooks: Vec<RoleCook> = checked
        .meshes
        .iter()
        .filter(|judged| judged.source == source)
        .flat_map(|judged| &judged.cooks)
        .map(|measured| RoleCook {
            cook: measured.cook.into(),
            triangles: measured.triangles,
            joints: measured.joints,
            slots: measured.slots,
            map: measured.map,
            row: RoleRow {
                triangles: measured.limits.triangles,
                joints: measured.limits.joints,
                slots: measured.limits.slots,
                map: measured.limits.map,
            },
        })
        .collect();
    let findings: Vec<CheckFinding> = checked
        .findings
        .iter()
        .filter(|finding| finding.source == source)
        .map(|finding| CheckFinding {
            rule: finding.rule.name().to_owned(),
            severity: match finding.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            }
            .to_owned(),
            cook: finding.cook.map(CookName::from),
            message: finding.message.clone(),
            waived: finding.waived.clone(),
        })
        .collect();
    (role.is_some() || !findings.is_empty()).then_some(CheckEntry { role, cooks, findings })
}

impl AssetIndex {
    /// `index.json`'s text: pretty JSON, ending with a newline.
    fn to_json(&self, path: &Path) -> Result<String, IndexError> {
        let mut text = serde_json::to_string_pretty(self).map_err(|error| IndexError::Invalid {
            path: path.to_owned(),
            why: format!("the index doesn't write as JSON: {error}"),
        })?;
        text.push('\n');
        Ok(text)
    }

    /// Reads the index at `path`.
    pub fn read(path: &Path) -> Result<Self, IndexError> {
        let text = fs::read(path).map_err(|source| IndexError::Read { path: path.to_owned(), source })?;
        let unreadable = |source| IndexError::Unreadable { path: path.to_owned(), source };
        // The version first, so an index of another version says so rather than naming a field.
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        let Version { version } = serde_json::from_slice(&text).map_err(unreadable)?;
        if version != VERSION {
            return Err(IndexError::Invalid {
                path: path.to_owned(),
                why: format!(
                    "it is an index of version {version}, and this pipeline reads {VERSION}: cook again"
                ),
            });
        }
        serde_json::from_slice(&text).map_err(unreadable)
    }

    /// The asset `query` names: an asset ID, or a source under the source root, whose own asset it is.
    pub fn find(&self, query: &str) -> Option<AssetId> {
        if let Ok(id) = query.parse::<AssetId>() {
            return self.assets.contains_key(&id).then_some(id);
        }
        let query = query.replace('\\', "/");
        self.assets.iter().find(|(_, asset)| !asset.part && asset.source == query).map(|(&id, _)| id)
    }

    /// What `root` uses, or with `reverse` what uses it, all the way down: each asset once, depth first in ID
    /// order, a step for each time one is reached, marked when it was walked from an earlier step.
    pub fn graph(&self, root: AssetId, reverse: bool) -> Vec<Step> {
        let mut steps = Vec::new();
        let mut walked = BTreeSet::new();
        let mut stack = vec![(root, 0)];
        while let Some((id, depth)) = stack.pop() {
            let first = walked.insert(id);
            steps.push(Step { id, depth, repeated: !first });
            if !first {
                continue;
            }
            let Some(asset) = self.assets.get(&id) else { continue };
            let next = if reverse { &asset.used_by } else { &asset.uses };
            stack.extend(next.iter().rev().map(|&next| (next, depth + 1)));
        }
        steps
    }

    /// The text `sapphire_pipeline graph` prints for `steps` ([`Self::graph`]).
    pub fn show<'a>(&'a self, steps: &'a [Step], reverse: bool) -> impl fmt::Display + 'a {
        Shown { index: self, steps, reverse }
    }
}

/// One asset reached in a walk of the graph ([`AssetIndex::graph`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub id: AssetId,
    /// How far from the root: 0 for the root.
    pub depth: u32,
    /// Reached before, and walked from there.
    pub repeated: bool,
}

struct Shown<'a> {
    index: &'a AssetIndex,
    steps: &'a [Step],
    reverse: bool,
}

impl fmt::Display for Shown<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let relation = if self.reverse { "used by" } else { "uses" };
        for step in self.steps {
            let indent = "  ".repeat(step.depth as usize);
            let lead = if step.depth == 0 { String::new() } else { format!("{relation} ") };
            write!(f, "{indent}{lead}{}", step.id)?;
            match self.index.assets.get(&step.id) {
                Some(asset) => write!(f, " {} {} [{}]", asset.kind, asset.source, asset.pack)?,
                None => write!(f, " (not in this cook)")?,
            }
            if step.repeated {
                write!(f, " (as above)")?;
            }
            writeln!(f)?;
        }
        let reached = self.steps.iter().filter(|step| step.depth > 0 && !step.repeated).count();
        let what = if self.reverse { "use it" } else { "it uses" };
        write!(f, "{reached} asset(s) {what}, all the way down")
    }
}

#[cfg(test)]
mod tests;
