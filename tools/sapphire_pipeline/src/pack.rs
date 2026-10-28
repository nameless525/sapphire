//! Packing, the pipeline's last stage (spec §5.27, M1 A4): a cook's outputs into packs, the
//! files content ships in. The format, its writer and reader are `sapphire_assets::pack`'s.
//!
//! [`pack_cooked`] takes a [`CookReport`], which gives every asset's ID, cooked file and cache
//! key, and writes one pack per group into a directory of its own, holding nothing but packs:
//! what `cargo xtask` bundles into apps and points desktop runs at. Each asset goes in exactly as
//! the cook wrote it, with its kind (from the cooked format's extension) and the assets it
//! references, read from its bytes by the runtime's own check ([`AssetKind::references`]), as
//! dependencies. Packs are deterministic: the same cooked assets make the same bytes.
//!
//! # Groups
//!
//! Spec §5.10 groups packs per zone, per character and per encounter, plus one shared pack. The
//! rule, for when zones, characters and encounters exist as documents (M2, M3): each is a root,
//! and its pack holds what it reaches through dependencies; an asset two groups reach goes in the
//! shared pack, which every app mounts first, so a group pack only ever depends on itself and the
//! shared pack. Until there are roots, every asset goes in [`SHARED`].
//!
//! # Codecs
//!
//! Blocks are compressed by kind ([`codec`]): **zstd** at level [`ZSTD_LEVEL`] for textures and
//! meshes, the bulk of a pack, loaded at zone and character transitions where install and
//! download size matter most (on the M5, level 19 stores a cooked mesh at 0.37 of its size and BC7
//! or ASTC textures at 0.66–0.69, against lz4's 0.63 and 0.86–0.88); **lz4** for materials, small
//! and decoded mid-play when live link or hot reload swaps one, and for rigs, animation's data,
//! loaded with a character swapping in: zstd's 4–5× slower decoding buys little there. A block
//! that doesn't shrink by 1/64 is stored raw either way.
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
//! before packs). Keyed by bytes, an encoding can't outlive the cooked file it was made from: a
//! file edited after its cook is refused, and so is a pack directory that holds cooked files,
//! which pruning would delete.
//!
//! Missing encodings run on at most four workers, with one job per content key. Each encoder
//! still runs serially, so neither the compressed bytes nor the pack layout depends on the
//! worker count. Results and errors are consumed in asset-ID order. The worker limit bounds
//! zstd's compression contexts and source buffers while leaving the host usable during a cook.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use sapphire_assets::material::MATERIAL_EXTENSION;
use sapphire_assets::mesh::MESH_EXTENSION;
use sapphire_assets::pack::{self as format, Codec, PACK_EXTENSION, PackAsset};
use sapphire_assets::probes::PROBE_GRID_EXTENSION;
use sapphire_assets::rig::RIG_EXTENSION;
use sapphire_assets::texture::TEXTURE_EXTENSION;
use sapphire_assets::{AssetError, AssetId, AssetKind};
use sapphire_serialize::cooked::AlignedVec;
use sapphire_serialize::postcard;

use crate::cache::{Cache, CacheError, CacheKey, Stage};
use crate::cook::{self, CookError, CookReport, CookedAsset};

/// The packer as cache keys know it. Its name carries the encoders' versions, which a test keeps
/// in step with `Cargo.lock`: a new zstd or lz4_flex may compress differently, so it re-encodes
/// everything. Bump the version when the codec policy, block encoding or pack layout changes.
pub const PACKER: Stage = Stage { name: "spack (zstd 1.5.7, lz4_flex 0.14.0)", version: 1 };

/// zstd's level for pack blocks. Encoding is cached, and decoding speed barely depends on the
/// level, so packs take the best ratio short of zstd's slower "ultra" levels.
pub const ZSTD_LEVEL: i32 = 19;

/// The pack every asset goes in until groups have roots (see the [module docs](self)).
pub const SHARED: &str = "shared";

/// The codec a kind's blocks are compressed with (see the [module docs](self)).
pub fn codec(kind: AssetKind) -> Codec {
    match kind {
        AssetKind::Mesh | AssetKind::Texture | AssetKind::ProbeGrid => Codec::Zstd,
        AssetKind::Material | AssetKind::Rig => Codec::Lz4,
    }
}

/// The kind of a cooked file, from its format's extension.
pub fn kind(output: &Path) -> Option<AssetKind> {
    match output.extension()?.to_str()? {
        MESH_EXTENSION => Some(AssetKind::Mesh),
        TEXTURE_EXTENSION => Some(AssetKind::Texture),
        MATERIAL_EXTENSION => Some(AssetKind::Material),
        RIG_EXTENSION => Some(AssetKind::Rig),
        PROBE_GRID_EXTENSION => Some(AssetKind::ProbeGrid),
        _ => None,
    }
}

/// Errors from packing. Each names the file it concerns.
#[derive(Debug, thiserror::Error)]
pub enum PackError {
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
    /// Its recorded hash matched the file, so nothing was read or written.
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
}

/// Packs every asset of `report` into `out` (see the [module docs](self)), caching encodings in
/// the content cache at `cache` if given.
pub fn pack_cooked(report: &CookReport, out: &Path, cache: Option<&Path>) -> Result<PackReport, PackError> {
    // Pruning the pack directory would delete loose cooked files kept in it.
    if let Some(asset) = report.assets.iter().find(|asset| asset.output.starts_with(out)) {
        return Err(PackError::Overlap { packs: out.to_owned(), output: asset.output.clone() });
    }
    let cache = cache.map(Cache::new);
    let ids: HashSet<AssetId> = report.assets.iter().map(|asset| asset.id).collect();
    let mut groups: BTreeMap<&str, Vec<&CookedAsset>> = BTreeMap::new();
    for asset in &report.assets {
        groups.entry(SHARED).or_default().push(asset);
    }
    let mut packs = Vec::with_capacity(groups.len());
    let workers = std::thread::available_parallelism().map_or(1, usize::from).min(4);
    for (name, assets) in groups {
        packs.push(pack_group(name, &assets, &ids, out, cache.as_ref(), workers)?);
    }
    let paths: HashSet<&Path> = packs.iter().map(|pack| pack.path.as_path()).collect();
    let mut extensions = vec![PACK_EXTENSION];
    extensions.extend_from_slice(cook::COOKED_EXTENSIONS);
    let removed = cook::prune(out, &|path| paths.contains(path), &extensions)?;
    Ok(PackReport { packs, removed })
}

/// An asset of a pack, with its kind, codec and packed key.
struct Packing<'a> {
    asset: &'a CookedAsset,
    kind: AssetKind,
    codec: Codec,
    key: CacheKey,
}

/// Brings pack `name`, holding `assets`, up to date.
fn pack_group(
    name: &str,
    assets: &[&CookedAsset],
    ids: &HashSet<AssetId>,
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
            Ok(Packing {
                asset,
                kind,
                codec,
                key: CacheKey::packed(&asset.output_hash, PACKER, codec, ZSTD_LEVEL),
            })
        })
        .collect::<Result<Vec<_>, PackError>>()?;
    packing.sort_by_key(|packing| packing.asset.id);
    let keys: Vec<(AssetId, CacheKey)> =
        packing.iter().map(|packing| (packing.asset.id, packing.key)).collect();
    let pack_key = CacheKey::pack(PACKER, name, &keys);
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
        // Identical cooks share an encoding; each asset keeps its own ID.
        pack_asset.id = asset.id;
        if let Some(&missing) = pack_asset.dependencies.iter().find(|id| !ids.contains(id)) {
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
        let Packing { asset, kind, codec, key } = &packing[jobs[job]];
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

/// Reads a cooked asset, checks it as the runtime will, and encodes it.
fn encode(asset: &CookedAsset, kind: AssetKind, codec: Codec) -> Result<PackAsset, PackError> {
    let path = &asset.output;
    let read_error = |source| PackError::Read { path: path.clone(), source };
    let mut file = fs::File::open(path).map_err(read_error)?;
    let mut bytes = AlignedVec::<16>::new();
    bytes.extend_from_reader(&mut file).map_err(read_error)?;
    if blake3::hash(&bytes) != asset.output_hash {
        return Err(PackError::Changed { path: path.clone() });
    }
    let dependencies =
        kind.references(&bytes).map_err(|source| PackError::Invalid { path: path.clone(), source })?;
    PackAsset::encode(asset.id, kind, codec, &bytes, dependencies, ZSTD_LEVEL)
        .map_err(|source| PackError::Format { path: path.clone(), source })
}

#[cfg(test)]
mod tests;
