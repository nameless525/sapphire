//! GPU device, capability probing, surfaces and resource helpers over wgpu (spec §5.11).
//!
//! # Owns
//!
//! - Backend policy: Metal on Apple, Vulkan elsewhere, with a DX12 fallback on Windows
//!   (spec §2). This is the only place in the engine where platform `cfg` picks a GPU API
//!   (rule R7).
//! - Device creation with a deliberately chosen limit floor, and [`GpuCaps`], the probed
//!   feature set the renderer selects passes by.
//! - Presentable surfaces ([`RenderSurface`]) that are cheap to drop and recreate, because
//!   Android destroys the native window on every background.
//! - The persistent Vulkan pipeline cache.
//!
//! # Does not own
//!
//! - Windows, events or native handles: the platform layer produces those and the app turns
//!   them into a [`SurfaceTarget`].
//! - Anything that draws. That is `sapphire_render`, the only other crate allowed to touch
//!   wgpu, through the re-export below.
//!
//! # Startup
//!
//! ```no_run
//! # fn target() -> sapphire_rhi::SurfaceTarget { unimplemented!() }
//! use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, PresentPolicy, RenderSurface};
//!
//! # fn main() -> Result<(), sapphire_rhi::RhiError> {
//! let instance = GpuInstance::new(&GpuInstanceDesc::default())?;
//! // SAFETY: the window behind `target()` lives as long as the process (see
//! // `RenderSurface::new`); a real app passes its platform window guard instead of `()`.
//! let mut surface = unsafe { RenderSurface::new(&instance, target(), ())? };
//! let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), Some(&surface))?;
//! surface.configure(&gpu, 1280, 720, PresentPolicy::Fifo)?;
//! # Ok(()) }
//! ```

mod backend;
mod caps;
mod context;
mod error;
mod pipeline_cache;
mod surface;
mod transient;

pub use backend::{BACKEND_OVERRIDE_ENV, DEBUG_OVERRIDE_ENV, GpuDebug, GpuInstance, GpuInstanceDesc};
pub use caps::{FormatSupport, GpuCaps};
pub use context::{GpuContext, GpuContextDesc};
pub use error::RhiError;
pub use pipeline_cache::PipelineCacheDesc;
pub use surface::{PresentPolicy, RenderSurface, SkipReason, SurfaceAcquire, SurfaceFrame, SurfaceTarget};
pub use transient::{EVICT_AFTER_FRAMES, TransientDesc, TransientPool, TransientStats, TransientTexture};

/// wgpu, re-exported so `sapphire_render` uses exactly this version (spec §5.11).
pub use wgpu;

/// Log target for this crate.
pub(crate) const LOG_TARGET: &str = "sapphire::rhi";
