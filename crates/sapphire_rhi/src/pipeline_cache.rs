//! Persistent pipeline cache (spec §5.11): loaded at boot, saved at shutdown and after load
//! screens. wgpu 30 implements [`wgpu::PipelineCache`] on Vulkan only, which is where it
//! matters most: Android drivers generally leave pipeline caching to the application.

use std::path::{Path, PathBuf};

use wgpu::{AdapterInfo, Device, PipelineCache, PipelineCacheDescriptor};
use xxhash_rust::xxh3::xxh3_64;

use crate::{LOG_TARGET, RhiError};

/// Envelope around wgpu's blob: magic, then an xxh3-64 of the payload.
const MAGIC: [u8; 8] = *b"SPPLCACH";
const HEADER_LEN: usize = MAGIC.len() + size_of::<u64>();

/// A pipeline cache bound to a file named after the adapter and driver.
#[derive(Debug)]
pub(crate) struct PersistentPipelineCache {
    cache: PipelineCache,
    path: PathBuf,
}

impl PersistentPipelineCache {
    /// Opens the cache for this adapter under `dir`, seeding it from disk when a valid file
    /// exists. Returns `None` when wgpu has no cache key for this backend.
    pub(crate) fn open(device: &Device, adapter: &AdapterInfo, dir: &Path) -> Option<Self> {
        // The key covers backend, vendor and device, so a GPU swap never reads a foreign cache.
        let key = wgpu::util::pipeline_cache_key(adapter)?;
        let path = dir.join(key).with_extension("bin");
        let file = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                tracing::warn!(target: LOG_TARGET, path = %path.display(), %err, "can't read pipeline cache");
                None
            }
        };
        let payload = file.as_deref().and_then(unwrap_envelope);
        if file.is_some() && payload.is_none() {
            tracing::warn!(target: LOG_TARGET, path = %path.display(), "discarding corrupt pipeline cache");
        }
        // SAFETY: `payload` is either `None` or bytes that `save` wrote from
        // `PipelineCache::get_data`: the envelope checksum proves the file is complete and
        // unchanged since then. wgpu additionally checks its own header against this adapter and
        // driver version and, with `fallback: true`, starts empty when the data doesn't match.
        // The file lives in the app's private storage, so deliberate tampering is out of scope.
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
        Some(Self { cache, path })
    }

    pub(crate) fn cache(&self) -> &PipelineCache {
        &self.cache
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
        std::fs::write(&tmp, wrap_envelope(&data)).map_err(io_err(&tmp))?;
        std::fs::rename(&tmp, &self.path).map_err(io_err(&self.path))?;
        tracing::info!(target: LOG_TARGET, path = %self.path.display(), bytes = data.len(), "pipeline cache saved");
        Ok(())
    }
}

fn wrap_envelope(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&xxh3_64(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn unwrap_envelope(bytes: &[u8]) -> Option<&[u8]> {
    let (header, payload) = bytes.split_at_checked(HEADER_LEN)?;
    let (magic, hash) = header.split_at(MAGIC.len());
    let hash = u64::from_le_bytes(hash.try_into().ok()?);
    (magic == MAGIC && hash == xxh3_64(payload)).then_some(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips() {
        let payload = b"driver blob".as_slice();
        assert_eq!(unwrap_envelope(&wrap_envelope(payload)), Some(payload));
        assert_eq!(unwrap_envelope(&wrap_envelope(&[])), Some([].as_slice()));
    }

    #[test]
    fn envelope_rejects_damage() {
        let good = wrap_envelope(b"driver blob");
        assert_eq!(unwrap_envelope(&good[..good.len() - 1]), None, "truncated");
        assert_eq!(unwrap_envelope(&good[..4]), None, "shorter than the header");

        let mut flipped = good.clone();
        *flipped.last_mut().expect("non-empty") ^= 1;
        assert_eq!(unwrap_envelope(&flipped), None, "bit flip");

        let mut bad_magic = good;
        bad_magic[0] = b'X';
        assert_eq!(unwrap_envelope(&bad_magic), None, "magic");
    }
}
