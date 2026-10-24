use std::path::PathBuf;

use wgpu::Backend;

/// Errors from GPU bring-up, surfaces and the pipeline cache.
#[derive(Debug, thiserror::Error)]
pub enum RhiError {
    #[error("invalid {var} value {value:?} (expected vulkan, metal or dx12)", var = crate::BACKEND_OVERRIDE_ENV)]
    InvalidBackendOverride { value: String },
    #[error("invalid {var} value {value:?} (expected none, validation, labels or all)", var = crate::DEBUG_OVERRIDE_ENV)]
    InvalidDebugOverride { value: String },
    #[error("GPU backend {0} is not available on this platform or in this build")]
    BackendUnavailable(Backend),
    #[error("no usable GPU adapter on backends {tried:?} (see the log for why each was rejected)")]
    NoUsableAdapter { tried: Vec<Backend> },
    #[error("failed to create a surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    #[error("surface target {0} is not supported on this platform")]
    UnsupportedSurfaceTarget(&'static str),
    #[error("the selected adapter cannot present to this surface")]
    SurfaceIncompatible,
    #[error("the surface offers no sRGB format and cannot reinterpret one")]
    NoSrgbSurfaceFormat,
    #[error("pipeline cache I/O on {path}: {source}")]
    PipelineCacheIo { path: PathBuf, source: std::io::Error },
}
