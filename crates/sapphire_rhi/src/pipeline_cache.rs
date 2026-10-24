//! Persistent pipeline cache (spec §5.11): loaded at boot, saved at shutdown and after load
//! screens. wgpu 30 implements [`wgpu::PipelineCache`] on Vulkan only, which is where it
//! matters most: Android drivers generally leave pipeline caching to the application.
//!
//! Two guards sit on top of wgpu's own header check:
//!
//! - **Identity.** The file is only used by the same driver build (`driver`, `driver_info`)
//!   and the same app build. wgpu's key covers vendor and device, but some Android driver
//!   updates keep their cache UUID, and an app update leaves stale pipelines behind.
//! - **Crash sentinel.** A `.pending` file is written before the cache is handed to the
//!   driver and removed once the app has presented a frame
//!   ([`crate::GpuContext::confirm_pipeline_cache`]). If it is still there at boot, the last
//!   run died in between, quite possibly inside the driver while it parsed the blob, so the
//!   cache is deleted instead of being fed to the driver again.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use wgpu::{AdapterInfo, Device, PipelineCache, PipelineCacheDescriptor};
use xxhash_rust::xxh3::xxh3_64;

use crate::{LOG_TARGET, RhiError};

/// Envelope around wgpu's blob: magic, identity hash, payload hash, payload.
const MAGIC: [u8; 8] = *b"SPPLCAC2";
const HEADER_LEN: usize = MAGIC.len() + 2 * size_of::<u64>();

/// Where and for which app build the pipeline cache is persisted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineCacheDesc {
    /// Directory for the cache files, normally under the app's cache directory.
    pub dir: PathBuf,
    /// Identifies the app build. A different value discards the cache.
    pub build_id: String,
}

/// A pipeline cache bound to a file named after the adapter.
#[derive(Debug)]
pub(crate) struct PersistentPipelineCache {
    cache: PipelineCache,
    path: PathBuf,
    sentinel: PathBuf,
    identity: u64,
    confirmed: AtomicBool,
}

impl PersistentPipelineCache {
    /// Opens the cache for this adapter, seeding it from disk when a valid file exists and the
    /// previous run confirmed it. Returns `None` when wgpu has no cache key for this backend.
    pub(crate) fn open(device: &Device, adapter: &AdapterInfo, desc: &PipelineCacheDesc) -> Option<Self> {
        // The key covers backend, vendor and device, so a GPU swap never reads a foreign cache.
        let key = wgpu::util::pipeline_cache_key(adapter)?;
        let path = desc.dir.join(&key).with_extension("bin");
        let sentinel = desc.dir.join(&key).with_extension("pending");
        let identity = identity(adapter, &desc.build_id);
        let file = read_seed(&path, &sentinel, identity);
        // Written even when there is nothing to seed, so a cache this run saves before
        // confirming is distrusted too. Without a sentinel there is no guard: don't seed.
        let guarded = write_sentinel(&desc.dir, &sentinel);
        let payload = file.as_deref().and_then(|bytes| unwrap_envelope(bytes, identity)).filter(|_| guarded);
        // SAFETY: `payload` is either `None` or bytes that `save` wrote from
        // `PipelineCache::get_data`: the envelope checksum proves the file is complete and
        // unchanged since then, and the identity hash that it came from this driver and app
        // build. wgpu additionally checks its own header against this adapter and, with
        // `fallback: true`, starts empty when the data doesn't match. The file lives in the
        // app's private storage, so deliberate tampering is out of scope.
        let cache = unsafe {
            device.create_pipeline_cache(&PipelineCacheDescriptor {
                label: Some("sapphire pipeline cache"),
                data: payload,
                fallback: true,
            })
        };
        tracing::info!(
            target: LOG_TARGET,
            path = %path.display(),
            seeded_bytes = payload.map_or(0, <[u8]>::len),
            "pipeline cache opened"
        );
        Some(Self { cache, path, sentinel, identity, confirmed: AtomicBool::new(false) })
    }

    pub(crate) fn cache(&self) -> &PipelineCache {
        &self.cache
    }

    /// Removes the crash sentinel. Only the first call does any I/O.
    pub(crate) fn confirm(&self) {
        if self.confirmed.swap(true, Ordering::AcqRel) {
            return;
        }
        match std::fs::remove_file(&self.sentinel) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                tracing::warn!(target: LOG_TARGET, path = %self.sentinel.display(), %err, "can't remove pipeline cache sentinel");
            }
        }
    }

    /// Writes the cache atomically (temporary file, then rename).
    pub(crate) fn save(&self) -> Result<(), RhiError> {
        let Some(data) = self.cache.get_data() else {
            return Ok(());
        };
        let io_err = |path: &Path| {
            let path = path.to_path_buf();
            move |source| RhiError::PipelineCacheIo { path, source }
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(io_err(dir))?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, wrap_envelope(&data, self.identity)).map_err(io_err(&tmp))?;
        std::fs::rename(&tmp, &self.path).map_err(io_err(&self.path))?;
        tracing::info!(target: LOG_TARGET, path = %self.path.display(), bytes = data.len(), "pipeline cache saved");
        Ok(())
    }
}

fn identity(adapter: &AdapterInfo, build_id: &str) -> u64 {
    // Separators keep ("ab", "c") and ("a", "bc") apart.
    xxh3_64(format!("{}\0{}\0{}", adapter.driver, adapter.driver_info, build_id).as_bytes())
}

/// Reads the cache file, unless the previous run never confirmed it: then the file is deleted.
fn read_seed(path: &Path, sentinel: &Path, identity: u64) -> Option<Vec<u8>> {
    if sentinel.exists() {
        tracing::warn!(
            target: LOG_TARGET,
            path = %path.display(),
            "the last run ended before confirming its pipeline cache; discarding it"
        );
        let _ = std::fs::remove_file(path);
        return None;
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(target: LOG_TARGET, path = %path.display(), %err, "can't read pipeline cache");
            return None;
        }
    };
    if let Err(reason) = check_envelope(&bytes, identity) {
        tracing::info!(target: LOG_TARGET, path = %path.display(), reason, "pipeline cache not usable; starting empty");
        return None;
    }
    Some(bytes)
}

/// Creates the crash sentinel. Returns whether it exists afterwards.
fn write_sentinel(dir: &Path, sentinel: &Path) -> bool {
    match std::fs::create_dir_all(dir).and_then(|()| std::fs::write(sentinel, [])) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(target: LOG_TARGET, path = %sentinel.display(), %err, "can't write pipeline cache sentinel; not seeding the cache");
            false
        }
    }
}

fn wrap_envelope(payload: &[u8], identity: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&identity.to_le_bytes());
    out.extend_from_slice(&xxh3_64(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn unwrap_envelope(bytes: &[u8], identity: u64) -> Option<&[u8]> {
    check_envelope(bytes, identity).ok()
}

/// The payload, or why the file can't be used.
fn check_envelope(bytes: &[u8], identity: u64) -> Result<&[u8], &'static str> {
    let (header, payload) = bytes.split_at_checked(HEADER_LEN).ok_or("truncated")?;
    let (magic, hashes) = header.split_at(MAGIC.len());
    let (stored_identity, stored_hash) = hashes.split_at(size_of::<u64>());
    let read_u64 = |bytes: &[u8]| bytes.try_into().map(u64::from_le_bytes).map_err(|_| "truncated");
    if magic != MAGIC {
        return Err("unknown format");
    }
    if read_u64(stored_identity)? != identity {
        return Err("different driver or app build");
    }
    if read_u64(stored_hash)? != xxh3_64(payload) {
        return Err("checksum mismatch");
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: u64 = 0x5eed;

    #[test]
    fn envelope_round_trips() {
        let payload = b"driver blob".as_slice();
        assert_eq!(unwrap_envelope(&wrap_envelope(payload, ID), ID), Some(payload));
        assert_eq!(unwrap_envelope(&wrap_envelope(&[], ID), ID), Some([].as_slice()));
    }

    #[test]
    fn envelope_rejects_damage_and_other_builds() {
        let good = wrap_envelope(b"driver blob", ID);
        assert_eq!(unwrap_envelope(&good, ID + 1), None, "different driver or app build");
        assert_eq!(unwrap_envelope(&good[..good.len() - 1], ID), None, "truncated");
        assert_eq!(unwrap_envelope(&good[..4], ID), None, "shorter than the header");

        let mut flipped = good.clone();
        *flipped.last_mut().expect("non-empty") ^= 1;
        assert_eq!(unwrap_envelope(&flipped, ID), None, "bit flip");

        let mut bad_magic = good;
        bad_magic[0] = b'X';
        assert_eq!(unwrap_envelope(&bad_magic, ID), None, "magic");
    }

    #[test]
    fn identity_covers_driver_and_build() {
        let mut adapter = AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, wgpu::Backend::Vulkan);
        adapter.driver = "Mali".into();
        adapter.driver_info = "v1.r38".into();
        let base = identity(&adapter, "1.0");
        assert_ne!(base, identity(&adapter, "1.1"));
        adapter.driver_info = "v1.r40".into();
        assert_ne!(base, identity(&adapter, "1.0"));
    }

    #[test]
    fn unconfirmed_cache_is_discarded() {
        let dir = std::env::temp_dir().join(format!("sapphire_rhi_pipeline_cache_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let (path, sentinel) = (dir.join("cache.bin"), dir.join("cache.pending"));
        std::fs::write(&path, wrap_envelope(b"blob", ID)).expect("write cache");

        // A confirmed cache is read and guarded by a new sentinel.
        assert_eq!(
            read_seed(&path, &sentinel, ID).as_deref().and_then(|b| unwrap_envelope(b, ID)),
            Some(&b"blob"[..])
        );
        assert!(write_sentinel(&dir, &sentinel) && sentinel.exists());

        // The next boot finds the sentinel: the run crashed before confirming.
        assert_eq!(read_seed(&path, &sentinel, ID), None);
        assert!(!path.exists(), "the suspect cache is deleted");

        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
