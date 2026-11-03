//! Packing, the pipeline's last stage (spec §5.27, M1 A4): a cook's outputs into packs, the
//! files content ships in. The format, its writer and reader are `sapphire_assets::pack`'s.
//!
//! [`pack_cooked`] takes a [`CookReport`], which gives every asset's ID, cooked file and cache
//! key, and writes one pack per group into a directory of its own, holding nothing but packs and
//! their [notices](#notices): what `cargo xtask` bundles into apps and points desktop runs at. Each
//! asset goes in exactly as the cook wrote it, with its kind (from the cooked format's extension) and
//! the assets it references, read from its bytes by the runtime's own check
//! ([`AssetKind::references`]), as dependencies. A mesh goes in with what each of its slots draws through
//! ([`PipelineKey`], M3 K1), from its materials wherever they're packed; nothing else has any. Packs are
//! deterministic: the same cooked assets make the same bytes.
//!
//! # Groups
//!
//! Spec §5.10 groups packs per zone, per character and per encounter, plus one shared pack. The
//! rule: each is a root, and its pack holds what it alone reaches through dependencies; an asset two
//! roots reach goes in the shared pack, which every app mounts first, so a root's pack only ever
//! depends on itself and the shared pack. Zones are the roots so far (M3 W1, `groups`), and an encounter
//! is reached from its zone, so it goes in that zone's pack (M3 B2): characters aren't documents with a pack
//! of their own yet, so everything else goes in [`SHARED`], but for sound.
//!
//! A zone reaches further than it depends: loading one doesn't load its cells' meshes
//! ([`AssetKind::reach`]), but they ship with it. Which pack an asset goes in is decided like so. An
//! asset no zone reaches, or two do, is shared. So is whatever a shared asset reaches, even if only
//! one zone does too, as the shared pack can't lean on a zone's. Anything else is its one zone's, in
//! `zone_<stem>` ([`sapphire_zone::pack_name`]), with the zone itself. An asset named only by code (the
//! look-dev scene's plants, say) reaches the app from nothing a pack can see, so were one zone all that
//! names it, it would move to that zone's pack and leave every other app without it:
//! [`check_code_names`] fails the cook instead, naming the line ([`crate::code_names`]). Give the zone a
//! copy of its own, or name the asset through the zone. A zone's navmesh is named by that zone alone, so
//! it's always in that zone's pack.
//!
//! Sounds and cues go in [`AUDIO`] ([`group`], M2 E1). The content tree holds about 280 sounds, about
//! 3 MB against the shared pack's 23 MB, and nothing yet says which a game needs. A pack of their own
//! leaves the shared pack byte for byte as it was, so a capture or a bench mounts and reads none of
//! it: an app that plays no sound pays for none but the file's place in its bundle. When cues have
//! roots, the sounds nothing reaches drop out.
//!
//! String tables go in [`LOC`] (M3 V2), for the same reason: an app that shows no text pays for none of
//! them, and every locale's table is wanted whichever zones a game holds. A table reaches nothing and no
//! zone reaches a table, so zones never draw one into their packs.
//!
//! UI documents go in [`UI`] (M3 U4), for the same reason: a screen's change leaves the shared pack
//! alone. A document reaches nothing and no zone reaches one.
//!
//! Font chains go in [`FONTS`], and each font in the pack its sidecar names, which it carries in its
//! cooked file (M3 U3, [`crate::font`]). That can be a zone's own pack, but never [`SHARED`], [`AUDIO`],
//! [`LOC`] or [`UI`], whose bytes a font's change must not move. No zone reaches a font or a chain, so what
//! zones reach never moves one.
//!
//! # Notices
//!
//! A font's license can require every build that ships it to ship the license too: the OFL's second
//! condition does. So a cook that packs fonts writes [`NOTICES`] beside the packs, each family with its
//! license as its authors wrote it, and since every bundler copies this directory whole, a build meets
//! it at `content/NOTICES.txt`. It's written only when it changes, and goes with the last font.
//!
//! # Codecs
//!
//! Blocks are compressed by kind ([`codec`]): **zstd** at level [`ZSTD_LEVEL`] for textures and
//! meshes, the bulk of a pack, loaded at zone and character transitions where install and
//! download size matter most (on the M5, level 19 stores a cooked mesh at 0.37 of its size and BC7
//! or ASTC textures at 0.66–0.69, against lz4's 0.63 and 0.86–0.88); **lz4** for materials, small
//! and decoded mid-play when live link or hot reload swaps one, for rigs, skeletons and clips,
//! animation's data, loaded with a character swapping in: zstd's 4–5× slower decoding buys little
//! there, and for cues and font chains; **none** for sounds, whose Vorbis doesn't compress further.
//! Fonts take zstd, as they load once, with the app, and outlines compress well. A block that
//! doesn't shrink by 1/64 is stored raw either way.
//!
//! # Caching
//!
//! Compressing is the slow part (zstd 19 runs at 13–20 MB/s a core), so each asset's encoding is
//! cached in the content cache under [`CacheKey::packed`]: the cooked bytes' hash, the packer and the
//! codec. A pack records its own hash under [`CacheKey::pack`], over its assets' IDs and packed
//! keys, so an unchanged pack is checked by hashing the file, without reading any encoding. A
//! changed asset re-encodes alone, and a pack deleted or edited by hand is rebuilt from cached
//! encodings. A pack is written only when its bytes change, and files in the directory that no
//! pack claims are removed (stale packs, temporary files, and the loose cooked files that shipped
//! before packs). A pack directory that holds cooked files, which pruning would delete, is refused.
//!
//! **A report describes files that must still be there.** Every cooked file is hashed against its
//! report before either cache is asked, so a file edited or gone since its cook is refused the same
//! way, by name, however warm the cache: what a run says never hangs on what an earlier run left. It
//! costs an unchanged pack one read of its cooked files on every worker, about 50 ms of a warm cook's
//! 650 on the M5 under load.
//!
//! Missing encodings run on at most four workers, with one job per content key. Each encoder
//! still runs serially, so neither the compressed bytes nor the pack layout depends on the
//! worker count. Results and errors are consumed in asset-ID order. The worker limit bounds
//! zstd's compression contexts and source buffers while leaving the host usable during a cook.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::code_names::CodeName;
use sapphire_assets::font::{FONTS_PACK, Font};
use sapphire_assets::material::Material;
use sapphire_assets::mesh::Mesh;
use sapphire_assets::pack::{self as format, Codec, PACK_EXTENSION, PackAsset};
use sapphire_assets::{AssetError, AssetId, AssetKind, PipelineKey, SlotMaterial};
use sapphire_serialize::cooked::AlignedVec;
use sapphire_serialize::postcard;

use crate::cache::{Cache, CacheError, CacheKey, Stage};
use crate::cook::{self, CookError, CookReport, CookedAsset};
use crate::zone;

/// The packer as cache keys know it. Its name carries the encoders' versions, which a test keeps
/// in step with `Cargo.lock`: a new zstd or lz4_flex may compress differently, so it re-encodes
/// everything. Bump the version when the codec policy, block encoding or pack layout changes.
pub const PACKER: Stage = Stage { name: "spack (zstd 1.5.7, lz4_flex 0.14.0)", version: 4 };

/// What a cooked asset reaches, as cache keys know it. Bump the version whenever that changes for the
/// same bytes ([`AssetKind::reach`]).
const REACH: Stage = Stage { name: "pack reach", version: 1 };

/// What reads a mesh's pipeline keys, as cache keys know it. Bump the version whenever they change for the
/// same bytes ([`PipelineKey::of_mesh`]).
const PIPELINE_KEYS: Stage = Stage { name: "pack pipeline keys", version: 2 };

/// zstd's level for pack blocks. Encoding is cached, and decoding speed barely depends on the
/// level, so packs take the best ratio short of zstd's slower "ultra" levels.
pub const ZSTD_LEVEL: i32 = 19;

/// The pack every asset goes in until groups have roots (see the [module docs](self)).
pub const SHARED: &str = "shared";

/// The pack sounds and cues go in (see the [module docs](self)).
pub const AUDIO: &str = sapphire_assets::sound::AUDIO_PACK;

/// The pack string tables go in (see the [module docs](self)).
pub const LOC: &str = "loc";

/// The pack font chains go in (see the [module docs](self)).
pub const FONTS: &str = FONTS_PACK;

/// The pack UI documents go in (see the [module docs](self)).
pub const UI: &str = sapphire_assets::ui::UI_PACK;

/// What the licenses of the fonts in a directory of packs are written to, beside the packs (see the
/// [module docs](self)).
pub const NOTICES: &str = "NOTICES.txt";

/// The pack an asset of `kind` goes in (see the [module docs](self)), but for a font, which says. A
/// file of no kind goes where the packer reports it.
pub fn group(kind: Option<AssetKind>) -> &'static str {
    match kind {
        Some(AssetKind::Sound | AssetKind::Cue) => AUDIO,
        Some(AssetKind::Strings) => LOC,
        Some(AssetKind::Font | AssetKind::FontChains) => FONTS,
        Some(AssetKind::Ui) => UI,
        _ => SHARED,
    }
}

/// The codec a kind's blocks are compressed with (see the [module docs](self)).
pub fn codec(kind: AssetKind) -> Codec {
    match kind {
        AssetKind::Mesh | AssetKind::Texture | AssetKind::ProbeGrid | AssetKind::Font => Codec::Zstd,
        AssetKind::Material
        | AssetKind::Rig
        | AssetKind::Skeleton
        | AssetKind::Clip
        | AssetKind::Colliders
        | AssetKind::Cue
        | AssetKind::MoveSet
        | AssetKind::AnimGraph
        | AssetKind::Zone
        | AssetKind::Strings
        | AssetKind::FontChains
        | AssetKind::NavMesh
        | AssetKind::Timeline
        | AssetKind::Ui
        | AssetKind::Encounter
        | AssetKind::Script => Codec::Lz4,
        AssetKind::Sound => Codec::None,
    }
}

/// The kind of a cooked file, from its format's extension.
pub fn kind(output: &Path) -> Option<AssetKind> {
    let extension = output.extension()?.to_str()?;
    AssetKind::ALL.into_iter().find(|kind| kind.extension() == extension)
}

/// Errors from packing. Each names the file it concerns.
#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error(
        "{} would go in pack {pack}, as one zone is all that reaches it, but {}:{line} names it, so apps \
         that don't mount that zone would lose it: name it through the zone, or give the zone its own copy",
        source_path.display(),
        code.display()
    )]
    CodeNamedInZone { source_path: PathBuf, pack: String, code: PathBuf, line: usize },
    #[error("{} isn't a cooked format packs hold", path.display())]
    UnknownKind { path: PathBuf },
    #[error("cooked asset {} is invalid", path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: AssetError,
    },
    #[error("{} references asset {id}, which no pack holds", path.display())]
    MissingDependency { path: PathBuf, id: AssetId },
    #[error(
        "zones {} and {} would share one pack: packs go by a zone's file name, so rename one",
        first.display(),
        second.display()
    )]
    ZoneNameClash { first: PathBuf, second: PathBuf },
    #[error(
        "{} and {} are both called the same: live tuning follows a move set by its file's name, so rename one",
        first.display(),
        second.display()
    )]
    MoveSetNameClash { first: PathBuf, second: PathBuf },
    #[error("{} changed after it was cooked; cook again", path.display())]
    Changed { path: PathBuf },
    #[error("the pack directory {} holds cooked file {}: packing would prune it", packs.display(), output.display())]
    Overlap { packs: PathBuf, output: PathBuf },
    #[error("failed to pack {}", path.display())]
    Format {
        path: PathBuf,
        #[source]
        source: format::PackError,
    },
    #[error("failed to read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Files(#[from] CookError),
    #[error(transparent)]
    Cache(#[from] CacheError),
}

/// What [`pack_cooked`] did with a pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackStatus {
    /// Laid out and written, or found byte-identical on disk.
    Packed,
    /// Its recorded hash matched the file, so no encoding was read and nothing written.
    UpToDate,
}

/// One pack of a [`PackReport`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedPack {
    pub name: String,
    pub path: PathBuf,
    pub assets: usize,
    /// The pack file's length.
    pub bytes: u64,
    /// Assets compressed by this run; the others came from the cache.
    pub encoded: usize,
    pub status: PackStatus,
}

/// What [`pack_cooked`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackReport {
    pub packs: Vec<PackedPack>,
    /// Files removed from the pack directory.
    pub removed: Vec<PathBuf>,
    /// The licenses of the fonts in these packs, if there are any.
    pub notices: Option<PathBuf>,
}

/// Fails if an asset that code names (`names`) would leave the shared packs for a zone's. A zone itself
/// may be named: that's how an app opens it, and it mounts the pack that goes with it.
pub fn check_code_names(
    report: &CookReport,
    cache: Option<&Path>,
    names: &[CodeName],
) -> Result<(), PackError> {
    let cache = cache.map(Cache::new);
    let (font_packs, _) = fonts(report)?;
    let groups = groups(report, &font_packs, cache.as_ref())?;
    let by_id: HashMap<AssetId, &CodeName> = names.iter().map(|name| (name.id, name)).collect();
    let is_zone = |asset: &&CookedAsset| kind(&asset.output) == Some(AssetKind::Zone);
    let zone_packs: HashSet<String> = report
        .assets
        .iter()
        .filter(is_zone)
        .map(|asset| sapphire_zone::pack_name(zone::stem(&asset.source)))
        .collect();
    for (pack, assets) in &groups {
        if !zone_packs.contains(pack) {
            continue;
        }
        for asset in assets {
            if kind(&asset.output) != Some(AssetKind::Zone)
                && let Some(name) = by_id.get(&asset.id)
            {
                return Err(PackError::CodeNamedInZone {
                    source_path: asset.source.clone(),
                    pack: pack.clone(),
                    code: name.path.clone(),
                    line: name.line,
                });
            }
        }
    }
    Ok(())
}

/// Packs every asset of `report` into `out` (see the [module docs](self)), caching encodings in
/// the content cache at `cache` if given. The caller serializes cooks and packs targeting
/// the same output tree (xtask holds that tree's content lock). Publication is atomic per
/// file; the directory is not a transaction across packs, notices and pruning.
pub fn pack_cooked(report: &CookReport, out: &Path, cache: Option<&Path>) -> Result<PackReport, PackError> {
    // Pruning the pack directory would delete loose cooked files kept in it, including
    // symlink names whose targets lie elsewhere.
    if let Some(asset) = report.assets.iter().find(|asset| asset.output.starts_with(out)) {
        return Err(PackError::Overlap { packs: out.to_owned(), output: asset.output.clone() });
    }
    let resolved_out =
        resolved_path(out).map_err(|source| PackError::Read { path: out.to_owned(), source })?;
    for asset in &report.assets {
        let resolved_input = fs::canonicalize(&asset.output)
            .map_err(|source| PackError::Read { path: asset.output.clone(), source })?;
        let parent =
            asset.output.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let input_location = fs::canonicalize(parent)
            .map_err(|source| PackError::Read { path: asset.output.clone(), source })?
            .join(asset.output.file_name().expect("cooked outputs have file names"));
        if resolved_input.starts_with(&resolved_out) || input_location.starts_with(&resolved_out) {
            return Err(PackError::Overlap { packs: out.to_owned(), output: asset.output.clone() });
        }
    }
    let cache = cache.map(Cache::new);
    check_move_set_names(report)?;
    let ids: HashMap<AssetId, &CookedAsset> = report.assets.iter().map(|asset| (asset.id, asset)).collect();
    let (font_packs, licenses) = fonts(report)?;
    let groups = groups(report, &font_packs, cache.as_ref())?;
    let mut packs = Vec::with_capacity(groups.len());
    let workers = std::thread::available_parallelism().map_or(1, usize::from).min(4);
    for (name, assets) in groups {
        packs.push(pack_group(&name, &assets, &ids, out, cache.as_ref(), workers)?);
    }
    let paths: HashSet<&Path> = packs.iter().map(|pack| pack.path.as_path()).collect();
    let mut extensions = vec![PACK_EXTENSION];
    extensions.extend(AssetKind::ALL.map(AssetKind::extension));
    let mut removed = cook::prune(out, &|path| paths.contains(path), &extensions)?;
    let notices = write_notices(out, &licenses, &mut removed)?;
    Ok(PackReport { packs, removed, notices })
}

/// Resolves existing ancestors too, so an output directory that has not been created yet
/// still cannot alias a cooked input through a symlink in one of its parents.
fn resolved_path(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent =
                path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let name = path.file_name().ok_or(error)?;
            Ok(resolved_path(parent)?.join(name))
        }
        Err(error) => Err(error),
    }
}

/// Brings the notices beside the packs up to date: a family's license, for each of `licenses`.
fn write_notices(
    out: &Path,
    licenses: &BTreeSet<(String, String)>,
    removed: &mut Vec<PathBuf>,
) -> Result<Option<PathBuf>, PackError> {
    let path = out.join(NOTICES);
    let read_error = |source| PackError::Read { path: path.clone(), source };
    if licenses.is_empty() {
        return match fs::remove_file(&path) {
            Ok(()) => {
                removed.push(path);
                Ok(None)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(read_error(error)),
        };
    }
    let mut text = String::from("The fonts in this build and the licenses they ship under.\n");
    for (family, license) in licenses {
        text.push_str(&format!("\n{family}\n{}\n\n{}\n", "=".repeat(family.len()), license.trim_end()));
    }
    let current = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(read_error(error)),
    };
    if current.as_deref() != Some(text.as_bytes()) {
        cook::write_atomically(&path, text.as_bytes())?;
    }
    Ok(Some(path))
}

/// The pack each font says it goes in.
type FontPacks = HashMap<AssetId, String>;

/// A family and its license.
type License = (String, String);

/// The pack each font of `report` says it goes in, and every family with its license.
fn fonts(report: &CookReport) -> Result<(FontPacks, BTreeSet<License>), PackError> {
    let mut packs = HashMap::new();
    let mut licenses = BTreeSet::new();
    for asset in &report.assets {
        if kind(&asset.output) == Some(AssetKind::Font) {
            let bytes = read_cooked(asset)?;
            let font = Font::read(&bytes)
                .map_err(|source| PackError::Invalid { path: asset.output.clone(), source: source.into() })?;
            licenses.insert((font.family.to_string(), font.license.to_string()));
            packs.insert(asset.id, font.pack.to_string());
        }
    }
    Ok((packs, licenses))
}

/// Which pack each asset of `report` goes in (see the [module docs](self)), given the fonts' from
/// [`fonts`]. A tree without zones reads no further asset for this.
fn groups<'a>(
    report: &'a CookReport,
    font_packs: &FontPacks,
    cache: Option<&Cache>,
) -> Result<BTreeMap<String, Vec<&'a CookedAsset>>, PackError> {
    // Where an asset goes if no one zone holds it.
    let home = |asset: &CookedAsset| {
        font_packs.get(&asset.id).cloned().unwrap_or_else(|| group(kind(&asset.output)).to_owned())
    };
    let is_zone = |asset: &&CookedAsset| kind(&asset.output) == Some(AssetKind::Zone);
    let zones: Vec<&CookedAsset> = report.assets.iter().filter(is_zone).collect();
    let mut groups: BTreeMap<String, Vec<&CookedAsset>> = BTreeMap::new();
    if zones.is_empty() {
        for asset in &report.assets {
            groups.entry(home(asset)).or_default().push(asset);
        }
        return Ok(groups);
    }

    // Each zone's pack, by its name folded to lowercase: one file where case doesn't count.
    let mut names: BTreeMap<String, &Path> = BTreeMap::new();
    let mut packs = HashMap::with_capacity(zones.len());
    for &asset in &zones {
        let name = sapphire_zone::pack_name(zone::stem(&asset.source));
        if let Some(first) = names.insert(name.to_lowercase(), &asset.source) {
            return Err(PackError::ZoneNameClash { first: first.to_owned(), second: asset.source.clone() });
        }
        packs.insert(asset.id, name);
    }

    let by_id: HashMap<AssetId, &CookedAsset> = report.assets.iter().map(|asset| (asset.id, asset)).collect();
    let mut reach = HashMap::new();
    for asset in &report.assets {
        // A kind that may depend on nothing reaches nothing: it needn't be read.
        let reaches = kind(&asset.output).filter(|&of| AssetKind::ALL.iter().any(|&to| of.may_depend_on(to)));
        if let Some(of) = reaches {
            reach.insert(asset.id, reach_of(asset, of, cache)?);
        }
    }
    let walk = |from: Vec<AssetId>| -> Result<HashSet<AssetId>, PackError> {
        let mut seen: HashSet<AssetId> = from.iter().copied().collect();
        let mut stack = from;
        while let Some(id) = stack.pop() {
            for &next in reach.get(&id).map_or(&[][..], Vec::as_slice) {
                if !by_id.contains_key(&next) {
                    return Err(PackError::MissingDependency { path: by_id[&id].output.clone(), id: next });
                }
                if seen.insert(next) {
                    stack.push(next);
                }
            }
        }
        Ok(seen)
    };

    // The one zone that reaches each asset, or `None` once a second has.
    let mut owner: HashMap<AssetId, Option<AssetId>> = HashMap::new();
    for asset in &zones {
        for id in walk(vec![asset.id])? {
            owner.entry(id).and_modify(|owner| *owner = None).or_insert(Some(asset.id));
        }
    }
    let shared = walk(
        report
            .assets
            .iter()
            .map(|asset| asset.id)
            .filter(|id| owner.get(id).is_none_or(Option::is_none))
            .collect(),
    )?;
    for asset in &report.assets {
        let name = match owner.get(&asset.id).copied().flatten() {
            Some(zone) if !shared.contains(&asset.id) => packs[&zone].clone(),
            _ => home(asset),
        };
        groups.entry(name).or_default().push(asset);
    }
    Ok(groups)
}

/// What cooked `asset`, of kind `of`, reaches, from the cache if its bytes have been asked before.
fn reach_of(asset: &CookedAsset, of: AssetKind, cache: Option<&Cache>) -> Result<Vec<AssetId>, PackError> {
    let key = CacheKey::reach(&asset.output_hash, REACH);
    if let Some(cache) = cache
        && let Some(cached) = cache.get(&key)?
        && let Ok(reach) = postcard::from_bytes(&cached)
    {
        return Ok(reach);
    }
    let path = &asset.output;
    let bytes = cooked_bytes(asset)?;
    let reach = of.reach(&bytes).map_err(|source| PackError::Invalid { path: path.clone(), source })?;
    if let Some(cache) = cache {
        cache.put(&key, &postcard::to_allocvec(&reach).expect("asset IDs always encode"))?;
    }
    Ok(reach)
}

/// What mesh `asset` draws through ([`PipelineKey::of_mesh`]), from the cache if its bytes and those of what
/// it references have been asked before. Its materials may be in any pack, so `assets` is the whole cook's.
fn pipeline_keys_of(
    asset: &CookedAsset,
    assets: &HashMap<AssetId, &CookedAsset>,
    cache: Option<&Cache>,
) -> Result<Vec<PipelineKey>, PackError> {
    let path = &asset.output;
    let missing = |id| PackError::MissingDependency { path: path.clone(), id };
    let references = reach_of(asset, AssetKind::Mesh, cache)?
        .into_iter()
        .map(|id| assets.get(&id).copied().ok_or_else(|| missing(id)))
        .collect::<Result<Vec<_>, _>>()?;
    let hashes: Vec<_> = references.iter().map(|reference| *reference.output_hash.as_bytes()).collect();
    let key = CacheKey::pipeline_keys(&asset.output_hash, &hashes, PIPELINE_KEYS);
    if let Some(cache) = cache
        && let Some(cached) = cache.get(&key)?
        && let Some(keys) = keys_of_rows(&cached)
    {
        return Ok(keys);
    }
    let mut materials: HashMap<AssetId, SlotMaterial> = HashMap::new();
    for reference in references {
        if kind(&reference.output) == Some(AssetKind::Material) {
            let bytes = cooked_bytes(reference)?;
            let material = Material::read(&bytes).map_err(|source| PackError::Invalid {
                path: reference.output.clone(),
                source: source.into(),
            })?;
            materials.insert(reference.id, (material.family, material.flags(), material.reads_reflection()));
        }
    }
    let bytes = cooked_bytes(asset)?;
    let mesh = Mesh::read(&bytes)
        .map_err(|source| PackError::Invalid { path: path.clone(), source: source.into() })?;
    let keys = PipelineKey::of_mesh(mesh, |id| materials.get(&id).copied()).map_err(missing)?;
    if let Some(cache) = cache {
        cache.put(&key, &rows_of_keys(&keys))?;
    }
    Ok(keys)
}

/// The rows a pack's table stores `keys` as.
fn rows_of_keys(keys: &[PipelineKey]) -> Vec<u8> {
    keys.iter().flat_map(|key| key.to_row()).collect()
}

/// The keys of `rows`, or `None` if they aren't rows of keys.
fn keys_of_rows(rows: &[u8]) -> Option<Vec<PipelineKey>> {
    let (rows, rest) = rows.as_chunks::<{ PipelineKey::ROW_LEN }>();
    let keys = rows.iter().map(|row| PipelineKey::from_row(row).ok());
    if rest.is_empty() { keys.collect() } else { None }
}

/// The bytes of cooked `asset`, aligned as cooked files are read, if they're still what its cook wrote.
fn cooked_bytes(asset: &CookedAsset) -> Result<AlignedVec<16>, PackError> {
    let path = &asset.output;
    let read_error = |source| PackError::Read { path: path.clone(), source };
    let mut bytes = AlignedVec::<16>::new();
    bytes.extend_from_reader(&mut fs::File::open(path).map_err(read_error)?).map_err(read_error)?;
    if blake3::hash(&bytes) != asset.output_hash {
        return Err(PackError::Changed { path: path.clone() });
    }
    Ok(bytes)
}

/// An asset of a pack, with its kind, codec and packed key, and for a mesh what it draws through.
struct Packing<'a> {
    asset: &'a CookedAsset,
    kind: AssetKind,
    codec: Codec,
    key: CacheKey,
    pipeline_keys: Vec<PipelineKey>,
}

/// Whether a kind's assets carry their names into a pack: those a person asks for by name, so the tools can
/// find them with no list in code. The rest stay IDs (rule R5).
fn carries_a_name(kind: AssetKind) -> bool {
    matches!(kind, AssetKind::Clip | AssetKind::MoveSet)
}

/// Two move sets with one name can't be told apart by what follows a document's edits by its file's name.
fn check_move_set_names(report: &CookReport) -> Result<(), PackError> {
    let mut seen: HashMap<&str, &Path> = HashMap::new();
    for asset in report.assets.iter().filter(|asset| kind(&asset.output) == Some(AssetKind::MoveSet)) {
        if let Some(first) = seen.insert(&asset.name, &asset.source) {
            return Err(PackError::MoveSetNameClash {
                first: first.to_owned(),
                second: asset.source.clone(),
            });
        }
    }
    Ok(())
}

/// Brings pack `name`, holding `assets`, up to date.
fn pack_group(
    name: &str,
    assets: &[&CookedAsset],
    ids: &HashMap<AssetId, &CookedAsset>,
    out: &Path,
    cache: Option<&Cache>,
    workers: usize,
) -> Result<PackedPack, PackError> {
    let path = out.join(format::file_name(name));
    let mut packing = assets
        .iter()
        .map(|&asset| {
            let kind =
                kind(&asset.output).ok_or_else(|| PackError::UnknownKind { path: asset.output.clone() })?;
            let codec = codec(kind);
            let pipeline_keys =
                if kind == AssetKind::Mesh { pipeline_keys_of(asset, ids, cache)? } else { Vec::new() };
            Ok(Packing {
                asset,
                kind,
                codec,
                key: CacheKey::packed(&asset.output_hash, PACKER, codec, ZSTD_LEVEL),
                pipeline_keys,
            })
        })
        .collect::<Result<Vec<_>, PackError>>()?;
    packing.sort_by_key(|packing| packing.asset.id);
    check_unchanged(&packing, workers)?;
    let keys: Vec<(AssetId, CacheKey)> =
        packing.iter().map(|packing| (packing.asset.id, packing.key)).collect();
    let rows: Vec<u8> = packing.iter().flat_map(|item| rows_of_keys(&item.pipeline_keys)).collect();
    let names: Vec<(AssetId, &str)> = packing
        .iter()
        .filter(|packing| carries_a_name(packing.kind))
        .map(|packing| (packing.asset.id, packing.asset.name.as_str()))
        .collect();
    let pack_key = CacheKey::pack(PACKER, name, &keys, &rows, &names);
    let current = file_hash(&path)?;
    let done = |encoded, bytes, status| PackedPack {
        name: name.to_owned(),
        path: path.clone(),
        assets: packing.len(),
        bytes,
        encoded,
        status,
    };
    if let (Some(cache), Some((hash, len))) = (cache, current)
        && cache.get(&pack_key)?.is_some_and(|recorded| recorded == hash.as_bytes())
    {
        return Ok(done(0, len, PackStatus::UpToDate));
    }

    // A cache miss for duplicated content must encode once, just as the serial cache did.
    // The first asset in ID order supplies the cached value; each occurrence gets its own ID.
    let mut by_key = HashMap::new();
    let mut jobs = Vec::new();
    let mut job_for_asset = Vec::with_capacity(packing.len());
    let mut remaining = Vec::new();
    for (index, item) in packing.iter().enumerate() {
        let job = cache.and_then(|_| by_key.get(&item.key).copied()).unwrap_or_else(|| {
            let job = jobs.len();
            jobs.push(index);
            remaining.push(0usize);
            by_key.insert(item.key, job);
            job
        });
        job_for_asset.push(job);
        remaining[job] += 1;
    }
    let mut results = encode_jobs(&packing, &jobs, cache, workers);
    let mut encoded = 0;
    let mut packed = Vec::with_capacity(packing.len());
    for (index, item) in packing.iter().enumerate() {
        let asset = item.asset;
        let job = job_for_asset[index];
        remaining[job] -= 1;
        let (mut pack_asset, fresh) = if remaining[job] == 0 {
            results[job].take().expect("every encoding job finished")?
        } else {
            match results[job].as_ref().expect("shared encoding remains until its last asset") {
                Ok((asset, fresh)) => (asset.clone(), *fresh),
                Err(_) => return Err(results[job].take().expect("the failed job is present").unwrap_err()),
            }
        };
        encoded += usize::from(fresh && jobs[job] == index);
        // Identical cooks share an encoding; each asset keeps its own ID, and what it draws through,
        // which its materials say rather than its bytes.
        pack_asset.id = asset.id;
        pack_asset.pipeline_keys.clone_from(&item.pipeline_keys);
        pack_asset.name = carries_a_name(item.kind).then(|| asset.name.clone());
        if let Some(&missing) = pack_asset.dependencies.iter().find(|id| !ids.contains_key(*id)) {
            return Err(PackError::MissingDependency { path: asset.output.clone(), id: missing });
        }
        packed.push(pack_asset);
    }
    let bytes =
        format::write_pack(packed).map_err(|source| PackError::Format { path: path.clone(), source })?;
    let hash = blake3::hash(&bytes);
    if current.map(|(hash, _)| hash) != Some(hash) {
        cook::write_atomically(&path, &bytes)?;
    }
    if let Some(cache) = cache {
        cache.put(&pack_key, hash.as_bytes())?;
    }
    Ok(done(encoded, bytes.len() as u64, PackStatus::Packed))
}

/// An encoding and whether this run compressed it rather than reading its cache entry.
type Encoded = Result<(PackAsset, bool), PackError>;

/// Complete each independent encoding, retaining stable result order despite varied asset sizes.
fn encode_jobs(
    packing: &[Packing<'_>],
    jobs: &[usize],
    cache: Option<&Cache>,
    workers: usize,
) -> Vec<Option<Encoded>> {
    let encode_job = |job: usize| {
        let Packing { asset, kind, codec, key, .. } = &packing[jobs[job]];
        if let Some(cache) = cache
            && let Some(cached) = cache.get(key)?
            && let Ok(packed) = postcard::from_bytes::<PackAsset>(&cached)
        {
            return Ok((packed, false));
        }
        let packed = encode(asset, *kind, *codec)?;
        if let Some(cache) = cache {
            cache.put(key, &postcard::to_allocvec(&packed).expect("pack assets always encode"))?;
        }
        Ok((packed, true))
    };
    encode_jobs_with(packing, jobs, workers, encode_job)
}

/// Keeps job-indexed results even when the size-based schedule completes in another order.
fn encode_jobs_with(
    packing: &[Packing<'_>],
    jobs: &[usize],
    workers: usize,
    encode_job: impl Fn(usize) -> Encoded + Sync,
) -> Vec<Option<Encoded>> {
    let workers = workers.max(1).min(jobs.len());
    if workers <= 1 {
        return (0..jobs.len()).map(|job| Some(encode_job(job))).collect();
    }
    // Start larger files first to avoid leaving the longest compression to the last worker.
    // Metadata failure is not an error here: encode reads the file and reports errors in ID order.
    let mut schedule: Vec<usize> = (0..jobs.len()).collect();
    schedule.sort_by_cached_key(|&job| {
        std::cmp::Reverse(fs::metadata(&packing[jobs[job]].asset.output).map_or(0, |meta| meta.len()))
    });
    let next = AtomicUsize::new(0);
    let batches = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut completed = Vec::new();
                    while let Some(&job) = schedule.get(next.fetch_add(1, Ordering::Relaxed)) {
                        completed.push((job, encode_job(job)));
                    }
                    completed
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("pack encoding worker panicked"))
            .collect::<Vec<_>>()
    });
    let mut results: Vec<_> = (0..jobs.len()).map(|_| None).collect();
    for batch in batches {
        for (job, result) in batch {
            results[job] = Some(result);
        }
    }
    results
}

/// The blake3 hash and length of the file at `path`, streamed, or `None` if there is none.
fn file_hash(path: &Path) -> Result<Option<(blake3::Hash, u64)>, PackError> {
    let read_error = |source| PackError::Read { path: path.to_owned(), source };
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(read_error(error)),
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(file).map_err(read_error)?;
    Ok(Some((hasher.finalize(), hasher.count())))
}

/// Refuses cooked files that aren't what their cook wrote, naming the first in asset order (see the
/// [module docs](self)). On every worker, since an unchanged pack spends more time here than anywhere.
fn check_unchanged(packing: &[Packing<'_>], workers: usize) -> Result<(), PackError> {
    let unchanged = |asset: &CookedAsset| {
        let path = &asset.output;
        let read_error = |source| PackError::Read { path: path.clone(), source };
        let mut hasher = blake3::Hasher::new();
        hasher.update_reader(fs::File::open(path).map_err(read_error)?).map_err(read_error)?;
        if hasher.finalize() == asset.output_hash {
            Ok(())
        } else {
            Err(PackError::Changed { path: path.clone() })
        }
    };
    let next = AtomicUsize::new(0);
    let failures: Vec<(AssetId, PackError)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers.max(1))
            .map(|_| {
                scope.spawn(|| {
                    let mut failures = Vec::new();
                    while let Some(item) = packing.get(next.fetch_add(1, Ordering::Relaxed)) {
                        if let Err(error) = unchanged(item.asset) {
                            failures.push((item.asset.id, error));
                        }
                    }
                    failures
                })
            })
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().expect("a freshness check panicked")).collect()
    });
    failures.into_iter().min_by_key(|(id, _)| *id).map_or(Ok(()), |(_, error)| Err(error))
}

/// The bytes of a cooked asset, as its cook wrote them.
fn read_cooked(asset: &CookedAsset) -> Result<AlignedVec<16>, PackError> {
    let path = &asset.output;
    let read_error = |source| PackError::Read { path: path.clone(), source };
    let mut file = fs::File::open(path).map_err(read_error)?;
    let mut bytes = AlignedVec::<16>::new();
    bytes.extend_from_reader(&mut file).map_err(read_error)?;
    if blake3::hash(&bytes) != asset.output_hash {
        return Err(PackError::Changed { path: path.clone() });
    }
    Ok(bytes)
}

/// Reads a cooked asset, checks it as the runtime will, and encodes it.
fn encode(asset: &CookedAsset, kind: AssetKind, codec: Codec) -> Result<PackAsset, PackError> {
    let path = &asset.output;
    let bytes = read_cooked(asset)?;
    let dependencies =
        kind.references(&bytes).map_err(|source| PackError::Invalid { path: path.clone(), source })?;
    PackAsset::encode(asset.id, kind, codec, &bytes, dependencies, ZSTD_LEVEL)
        .map_err(|source| PackError::Format { path: path.clone(), source })
}

#[cfg(test)]
mod tests;
