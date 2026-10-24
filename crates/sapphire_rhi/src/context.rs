//! Adapter selection and device creation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wgpu::{Adapter, Backend, Device, DeviceType, Queue, TextureFormat};

use crate::caps::{FormatSupport, GpuCaps, REQUIRED_DOWNLEVEL};
use crate::pipeline_cache::{PersistentPipelineCache, PipelineCacheDesc};
use crate::{GpuInstance, LOG_TARGET, RenderSurface, RhiError};

/// Device-level GPU options.
#[derive(Clone, Debug, Default)]
pub struct GpuContextDesc {
    /// Where to persist the pipeline cache. `None` disables persistence. Only Vulkan devices
    /// use it (wgpu 30).
    pub pipeline_cache: Option<PipelineCacheDesc>,
}

/// The GPU device the engine renders with: adapter, device, queue and probed capabilities.
///
/// wgpu handles are internally reference-counted, so the accessors hand out references and
/// subsystems clone what they need to keep.
///
/// # Device loss
///
/// A device can be lost at any time (driver reset, GPU removed, `Device::destroy`). Once
/// [`Self::is_lost`] returns `true`, everything created from this context is dead and every
/// [`RenderSurface`] reports [`crate::SkipReason::DeviceLost`]. To recover, the app drops its
/// renderer, surfaces and this context, in that order, creates a new [`GpuInstance`] and
/// context, rebuilds its surfaces from fresh native handles and re-uploads every asset. GPU
/// work issued meanwhile is ignored rather than reported as an error.
#[derive(Debug)]
pub struct GpuContext {
    instance: GpuInstance,
    adapter: Adapter,
    device: Device,
    queue: Queue,
    caps: GpuCaps,
    pipeline_cache: Option<PersistentPipelineCache>,
    lost: Arc<AtomicBool>,
}

impl GpuContext {
    /// Picks an adapter and creates the device. Blocks until done.
    ///
    /// Backends are tried in [`GpuInstance::backends`] order; within a backend, discrete GPUs
    /// before integrated, virtual and software ones. An adapter is skipped if it can't present
    /// to `compatible_surface`, lacks a required capability or limit, or fails device creation,
    /// which is how Windows falls back from Vulkan to DX12 (spec §2 note 2).
    pub fn new(
        instance: &GpuInstance,
        desc: &GpuContextDesc,
        compatible_surface: Option<&RenderSurface>,
    ) -> Result<Self, RhiError> {
        let surface = compatible_surface.map(RenderSurface::raw);
        for (i, &backend) in instance.backends().iter().enumerate() {
            let Some((adapter, device, queue, caps)) = open_device(instance, backend, surface) else {
                let next = instance.backends().get(i + 1);
                tracing::warn!(target: LOG_TARGET, %backend, fallback = ?next, "no usable adapter on backend");
                continue;
            };
            caps.log();
            let lost = Arc::new(AtomicBool::new(false));
            install_error_handlers(&device, Arc::clone(&lost));
            let pipeline_cache = match (&desc.pipeline_cache, caps.pipeline_cache) {
                (Some(cache), true) => PersistentPipelineCache::open(&device, &caps.adapter, cache),
                _ => None,
            };
            return Ok(Self {
                instance: instance.clone(),
                adapter,
                device,
                queue,
                caps,
                pipeline_cache,
                lost,
            });
        }
        Err(RhiError::NoUsableAdapter { tried: instance.backends().to_vec() })
    }

    /// The instance this context was created from. New surfaces (for example after an
    /// Android resume) must come from it.
    pub fn instance(&self) -> &GpuInstance {
        &self.instance
    }

    pub fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn caps(&self) -> &GpuCaps {
        &self.caps
    }

    /// What this device may do with `format`: render, blend, filter, and which sample counts.
    /// Use this, not the adapter's report, before creating attachments.
    pub fn format_support(&self, format: TextureFormat) -> FormatSupport {
        self.caps.format_support(format, self.adapter.get_texture_format_features(format))
    }

    /// `Some(label)` when object labels are on ([`crate::GpuDebug::object_labels`]), else
    /// `None`. Use it for objects created every frame: wgpu copies each label into a `String`
    /// even when it later discards it.
    pub fn label(&self, label: &'static str) -> Option<&'static str> {
        self.instance.debug().object_labels.then_some(label)
    }

    /// Whether the device has been lost. See the type docs for how to recover.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }

    pub(crate) fn lost_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.lost)
    }

    /// The cache to pass as `cache` in pipeline descriptors, when persistence is enabled.
    pub fn pipeline_cache(&self) -> Option<&wgpu::PipelineCache> {
        self.pipeline_cache.as_ref().map(PersistentPipelineCache::cache)
    }

    /// Persists the pipeline cache. Call at shutdown and after load screens (spec §5.11).
    /// Does nothing when persistence is off or unsupported.
    pub fn save_pipeline_cache(&self) -> Result<(), RhiError> {
        self.pipeline_cache.as_ref().map_or(Ok(()), PersistentPipelineCache::save)
    }

    /// Marks the pipeline cache as good. Call after the first frame has been presented: until
    /// then a crash, possibly inside the driver while it reads the cache, makes the next boot
    /// discard the cache instead of trying it again. Cheap to call more than once.
    pub fn confirm_pipeline_cache(&self) {
        if let Some(cache) = &self.pipeline_cache {
            cache.confirm();
        }
    }
}

type OpenDevice = (Adapter, Device, Queue, GpuCaps);

fn open_device(
    instance: &GpuInstance,
    backend: Backend,
    surface: Option<&wgpu::Surface<'static>>,
) -> Option<OpenDevice> {
    let mut adapters = pollster::block_on(instance.raw().enumerate_adapters(backend.into()));
    adapters.sort_by_key(|adapter| performance_rank(adapter.get_info().device_type));
    for adapter in adapters {
        let caps = GpuCaps::probe(&adapter);
        if let Some(reason) = rejection_reason(&adapter, &caps, surface) {
            tracing::info!(target: LOG_TARGET, adapter = %caps.adapter.name, %backend, reason, "skipping adapter");
            continue;
        }
        let desc = wgpu::DeviceDescriptor {
            label: Some("sapphire"),
            required_features: caps.features,
            required_limits: caps.limits.clone(),
            ..Default::default()
        };
        match pollster::block_on(adapter.request_device(&desc)) {
            Ok((device, queue)) => return Some((adapter, device, queue, caps)),
            Err(err) => {
                tracing::warn!(target: LOG_TARGET, adapter = %caps.adapter.name, %backend, %err, "device creation failed");
            }
        }
    }
    None
}

/// Why an adapter can't be used, checked before `request_device` (which panics on
/// unsupported features or limits rather than returning an error).
fn rejection_reason(
    adapter: &Adapter,
    caps: &GpuCaps,
    surface: Option<&wgpu::Surface<'_>>,
) -> Option<String> {
    if surface.is_some_and(|surface| !adapter.is_surface_supported(surface)) {
        return Some("cannot present to the window surface".to_owned());
    }
    let missing = REQUIRED_DOWNLEVEL - adapter.get_downlevel_capabilities().flags;
    if !missing.is_empty() {
        return Some(format!("missing downlevel capabilities {missing:?}"));
    }
    let mut reason = None;
    caps.limits.check_limits_with_fail_fn(&caps.adapter_limits, true, |name, needed, available| {
        reason = Some(format!("limit {name}: engine needs {needed}, adapter has {available}"));
    });
    reason
}

/// High-performance preference: lower ranks are tried first.
fn performance_rank(device_type: DeviceType) -> u8 {
    match device_type {
        DeviceType::DiscreteGpu => 0,
        DeviceType::IntegratedGpu => 1,
        DeviceType::VirtualGpu => 2,
        DeviceType::Other => 3,
        // Software rasterizers (lavapipe, WARP) stay usable for CI golden images.
        DeviceType::Cpu => 4,
    }
}

fn install_error_handlers(device: &Device, lost: Arc<AtomicBool>) {
    device.on_uncaptured_error(Arc::new(|error: wgpu::Error| {
        tracing::error!(target: LOG_TARGET, %error, "uncaptured GPU error");
        // Development builds stop at the first error, like wgpu's default handler; shipping
        // builds log and keep running.
        if cfg!(debug_assertions) {
            panic!("uncaptured GPU error: {error}");
        }
    }));
    device.set_device_lost_callback(move |reason, message| {
        lost.store(true, Ordering::Release);
        match reason {
            wgpu::DeviceLostReason::Destroyed => {
                tracing::debug!(target: LOG_TARGET, message, "GPU device destroyed");
            }
            wgpu::DeviceLostReason::Unknown => {
                tracing::error!(target: LOG_TARGET, message, "GPU device lost");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_high_performance_adapters() {
        let mut types =
            [DeviceType::Cpu, DeviceType::IntegratedGpu, DeviceType::Other, DeviceType::DiscreteGpu];
        types.sort_by_key(|&t| performance_rank(t));
        assert_eq!(
            types,
            [DeviceType::DiscreteGpu, DeviceType::IntegratedGpu, DeviceType::Other, DeviceType::Cpu]
        );
    }

    /// Needs a GPU. Run with `cargo test -p sapphire_rhi -- --ignored`.
    #[test]
    #[ignore = "needs a GPU"]
    fn creates_a_headless_device() {
        let instance = GpuInstance::new(&crate::GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        assert!(instance.backends().contains(&gpu.caps().backend()));
        assert!(gpu.device().limits().max_storage_buffers_per_shader_stage >= 4);
        let rgba = gpu.format_support(TextureFormat::Rgba8UnormSrgb);
        assert!(rgba.renderable && rgba.supports_samples(4));
    }

    /// Needs a GPU. Run with `cargo test -p sapphire_rhi -- --ignored`.
    #[test]
    #[ignore = "needs a GPU"]
    fn reports_device_loss() {
        let instance = GpuInstance::new(&crate::GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        assert!(!gpu.is_lost());
        gpu.device().destroy();
        // wgpu reports a destroyed device once its queue drains, during a poll.
        let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
        assert!(gpu.is_lost());
    }
}
