//! Presentable surfaces.
//!
//! A [`RenderSurface`] owns only the wgpu surface, its configuration and a guard keeping the
//! native window alive, so it is cheap to drop and recreate: Android destroys the native
//! window on every background (spec §6.1), and the app simply drops the surface then and
//! creates a new one on resume.

use std::any::Any;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use wgpu::{CompositeAlphaMode, CurrentSurfaceTexture, PresentMode, TextureFormat};

use crate::{GpuContext, GpuInstance, LOG_TARGET, RhiError};

/// The native object a surface presents to. The app builds it from the platform layer.
#[derive(Clone, Copy, Debug)]
pub enum SurfaceTarget {
    /// A `CAMetalLayer` on macOS or iOS, such as the one from `SDL_Metal_GetLayer`.
    CoreAnimationLayer(NonNull<c_void>),
    /// Window plus display handles: Win32, Xlib, Wayland, or an Android `ANativeWindow`.
    Raw { window: RawWindowHandle, display: RawDisplayHandle },
}

/// How finished frames reach the display (spec §5.11, §6.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresentPolicy {
    /// Vsync through a queue; never tears and is supported everywhere. The default on every
    /// platform, and the only mode mobile uses.
    #[default]
    Fifo,
    /// Vsync where the newest frame replaces a queued one: lower latency, never tears. A
    /// desktop user option; falls back to [`Self::Fifo`] where unsupported.
    Mailbox,
    /// No vsync; may tear. A desktop user option; falls back to [`Self::Fifo`] where
    /// unsupported.
    Immediate,
}

impl PresentPolicy {
    fn resolve(self, supported: &[PresentMode]) -> PresentMode {
        let wanted = match self {
            Self::Fifo => return PresentMode::Fifo,
            Self::Mailbox => PresentMode::Mailbox,
            Self::Immediate => PresentMode::Immediate,
        };
        if supported.contains(&wanted) {
            wanted
        } else {
            tracing::info!(target: LOG_TARGET, ?wanted, "present mode unsupported; using Fifo");
            PresentMode::Fifo
        }
    }
}

/// Swapchain rebuilds triggered by `Suboptimal` are at most this often, in acquired frames.
/// A rebuild waits for the GPU to go idle, and some drivers report every frame as suboptimal
/// (Android, when the swapchain's pre-rotation doesn't match the display), so rebuilding on
/// each one would stall every frame for nothing.
const SUBOPTIMAL_REBUILD_INTERVAL: u32 = 120;

/// Why [`RenderSurface::acquire`] produced no frame. The app skips rendering this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The GPU device is lost ([`GpuContext::is_lost`]). Checked before anything else and
    /// permanent: the app must drop its renderer, surfaces and context and rebuild them, then
    /// re-upload its assets (see [`GpuContext`]). After an explicit `Device::destroy`, wgpu
    /// only notices once the queue is idle, so the frames still in flight report
    /// [`Self::Validation`] first.
    DeviceLost,
    /// [`RenderSurface::configure`] has not been called.
    Unconfigured,
    /// The window has zero area (minimized).
    ZeroSize,
    /// The window is hidden or fully covered.
    Occluded,
    /// Acquiring timed out. The swapchain is rebuilt on the next acquire.
    Timeout,
    /// The swapchain was still out of date after being rebuilt once (the window changed again
    /// mid-frame). The next acquire tries again.
    Outdated,
    /// The surface is gone, usually because its native window was destroyed. Android does
    /// this before the app hears it is going to the background, so it is never fatal.
    ///
    /// The state is sticky: the surface is not touched again, and every acquire reports
    /// `Lost` without calling into the driver, until the app drops it and creates a new one
    /// from a fresh [`SurfaceTarget`] once the platform has a window again. It is also
    /// reported when rebuilding the swapchain fails, which happens for the same reason.
    Lost,
    /// wgpu reported a validation error while acquiring; it has already been logged.
    Validation,
}

/// Result of [`RenderSurface::acquire`].
#[derive(Debug)]
pub enum SurfaceAcquire<'s> {
    Frame(SurfaceFrame<'s>),
    Skip(SkipReason),
}

/// A swapchain image to render into. It borrows the surface mutably, so the surface can't be
/// resized or reconfigured while a frame is outstanding (wgpu would panic).
///
/// Always [`present`](Self::present) it. Dropping it unpresented is logged as a warning: on
/// Vulkan, wgpu cannot hand an acquired image back, so it stays acquired until the swapchain
/// is rebuilt, and after a few such frames acquiring blocks or times out.
#[derive(Debug)]
pub struct SurfaceFrame<'s> {
    /// `None` once presented.
    texture: Option<wgpu::SurfaceTexture>,
    view: wgpu::TextureView,
    size: (u32, u32),
    state: &'s Configured,
}

impl SurfaceFrame<'_> {
    /// The render target, in the surface's sRGB view format.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Queues the image for display. Submit the frame's command buffers first.
    pub fn present(mut self) {
        if let Some(texture) = self.texture.take() {
            self.state.queue.present(texture);
        }
    }
}

impl Drop for SurfaceFrame<'_> {
    fn drop(&mut self) {
        if self.texture.is_some() && !std::thread::panicking() {
            tracing::warn!(
                target: LOG_TARGET,
                "SurfaceFrame dropped without being presented; on Vulkan its image stays acquired"
            );
        }
    }
}

/// A window surface plus its swapchain configuration.
///
/// It owns a guard that keeps its native window alive (see [`Self::new`]). Because that guard
/// is usually tied to the main thread (SDL windows are), `RenderSurface` is `!Send`: create,
/// use and drop it on one thread. That stays so until the engine has a protocol for tearing
/// surfaces down from render threads.
#[derive(Debug)]
pub struct RenderSurface {
    // Field order is drop order: the wgpu surface goes before the window it presents to.
    surface: wgpu::Surface<'static>,
    state: Option<Configured>,
    /// The guard passed to [`Self::new`], held only for its destructor.
    _keep_alive: Box<dyn Any>,
}

#[derive(Debug)]
struct Configured {
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// Format of the views handed out. Differs from `config.format` only when an sRGB view
    /// reinterprets a non-sRGB swapchain.
    view_format: TextureFormat,
    present_modes: Vec<PresentMode>,
    max_dimension: u32,
    /// Requested size, applied on the next acquire so a burst of resize events reconfigures
    /// once.
    size: (u32, u32),
    dirty: bool,
    /// See [`SkipReason::Lost`]. Only a new surface or an explicit `configure` clears it.
    lost: bool,
    /// Shared with the [`GpuContext`]; see [`SkipReason::DeviceLost`].
    device_lost: Arc<AtomicBool>,
    labels: bool,
    frames_since_configure: u32,
}

impl Configured {
    /// Rebuilds the swapchain. Returns `false` and marks the surface lost if wgpu rejects the
    /// configuration, which happens when the native window is already gone. The error is
    /// captured here so it never reaches the device's uncaptured-error handler.
    fn apply(&mut self, surface: &wgpu::Surface<'_>) -> bool {
        self.config.width = self.size.0.min(self.max_dimension);
        self.config.height = self.size.1.min(self.max_dimension);
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(&self.device, &self.config);
        // wgpu-core reports scope errors synchronously, so this never blocks.
        let error = pollster::block_on(scope.pop());
        self.dirty = false;
        self.frames_since_configure = 0;
        if let Some(error) = error {
            tracing::warn!(target: LOG_TARGET, %error, "can't configure the surface; treating it as lost");
            self.lost = true;
        }
        !self.lost
    }

    fn device_lost(&self) -> bool {
        self.device_lost.load(Ordering::Acquire)
    }

    /// Why acquiring failed with a validation error. A destroyed device shows up this way
    /// until wgpu has polled it and run the device-lost callback, so poll once and look again.
    fn validation_or_device_lost(&self) -> SkipReason {
        let _ = self.device.poll(wgpu::PollType::Poll);
        if self.device_lost() { SkipReason::DeviceLost } else { SkipReason::Validation }
    }
}

impl RenderSurface {
    /// Creates an unconfigured surface for `target`. Pass it to [`GpuContext::new`] so the
    /// chosen adapter can present to it, then call [`Self::configure`].
    ///
    /// `keep_alive` is whatever keeps `target`'s native objects alive, such as the platform
    /// layer's surface handle. The surface holds it and drops it right after the wgpu surface,
    /// so the app stores only the surface. Pass `()` if something else guarantees the lifetime.
    ///
    /// # Safety
    ///
    /// - `target` must name a live native object: for [`SurfaceTarget::CoreAnimationLayer`], a
    ///   valid `CAMetalLayer`; for [`SurfaceTarget::Raw`], a window and the display it belongs
    ///   to, such as an Android `ANativeWindow` the platform layer has not yet released.
    /// - That object must stay valid until `keep_alive` is dropped (or longer). On Android,
    ///   drop the surface as soon as the platform reports the native window is being
    ///   destroyed.
    /// - On Apple platforms, call this on the main thread.
    pub unsafe fn new(
        instance: &GpuInstance,
        target: SurfaceTarget,
        keep_alive: impl Any,
    ) -> Result<Self, RhiError> {
        let raw_target = match target {
            // CAMetalLayer only exists on Apple targets; mapping it is part of backend selection (R7).
            #[cfg(target_vendor = "apple")]
            SurfaceTarget::CoreAnimationLayer(layer) => {
                wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer.as_ptr())
            }
            #[cfg(not(target_vendor = "apple"))]
            SurfaceTarget::CoreAnimationLayer(_) => {
                return Err(RhiError::UnsupportedSurfaceTarget("CoreAnimationLayer"));
            }
            SurfaceTarget::Raw { window, display } => wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(display),
                raw_window_handle: window,
            },
        };
        // SAFETY: this function's contract is wgpu's: the target is valid for surface
        // creation and outlives the surface, because `keep_alive` keeps it valid and is dropped
        // after the surface (field order).
        let surface = unsafe { instance.raw().create_surface_unsafe(raw_target)? };
        Ok(Self { surface, state: None, _keep_alive: Box::new(keep_alive) })
    }

    /// Chooses the swapchain format, alpha mode and present mode for `gpu`'s adapter and
    /// configures the swapchain. Call again after recreating the device.
    ///
    /// If the swapchain can't be built because the native window is already gone, this still
    /// returns `Ok` and leaves the surface lost (see [`SkipReason::Lost`]).
    pub fn configure(
        &mut self,
        gpu: &GpuContext,
        width: u32,
        height: u32,
        policy: PresentPolicy,
    ) -> Result<(), RhiError> {
        let caps = self.surface.get_capabilities(gpu.adapter());
        if caps.formats.is_empty() {
            return Err(RhiError::SurfaceIncompatible);
        }
        let can_reinterpret = gpu
            .adapter()
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS);
        let (format, view_format) =
            choose_format(&caps.formats, can_reinterpret).ok_or(RhiError::NoSrgbSurfaceFormat)?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            // SDR output; HDR surfaces come later (spec §5.11).
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: 0,
            height: 0,
            present_mode: policy.resolve(&caps.present_modes),
            // One frame of CPU/GPU overlap, matching the frame pipeline (spec §4.2).
            desired_maximum_frame_latency: 2,
            alpha_mode: choose_alpha(&caps.alpha_modes),
            view_formats: if view_format == format { Vec::new() } else { vec![view_format] },
        };
        tracing::info!(
            target: LOG_TARGET,
            ?format,
            ?view_format,
            present_mode = ?config.present_mode,
            alpha_mode = ?config.alpha_mode,
            "surface configured"
        );
        let state = self.state.insert(Configured {
            device: gpu.device().clone(),
            queue: gpu.queue().clone(),
            config,
            view_format,
            present_modes: caps.present_modes,
            max_dimension: gpu.device().limits().max_texture_dimension_2d,
            size: (width, height),
            dirty: true,
            lost: false,
            device_lost: gpu.lost_flag(),
            labels: gpu.label("").is_some(),
            frames_since_configure: 0,
        });
        if width > 0 && height > 0 {
            // A failure leaves the surface lost, which acquire reports.
            state.apply(&self.surface);
        }
        Ok(())
    }

    /// Records a new window size in pixels. The swapchain is rebuilt on the next acquire.
    pub fn resize(&mut self, width: u32, height: u32) {
        if let Some(state) = &mut self.state
            && state.size != (width, height)
        {
            state.size = (width, height);
            state.dirty = true;
        }
    }

    /// Changes the present policy. Takes effect on the next acquire.
    pub fn set_present_policy(&mut self, policy: PresentPolicy) {
        if let Some(state) = &mut self.state {
            state.config.present_mode = policy.resolve(&state.present_modes);
            state.dirty = true;
        }
    }

    /// The format of frame views (always sRGB), once configured.
    pub fn format(&self) -> Option<TextureFormat> {
        self.state.as_ref().map(|state| state.view_format)
    }

    /// The most recently requested size, once configured.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.state.as_ref().map(|state| state.size)
    }

    /// Gets the next swapchain image, or tells the caller to skip this frame. Nothing here is
    /// fatal; every failure becomes a [`SkipReason`].
    ///
    /// - Device lost: reported first, every time (see [`SkipReason::DeviceLost`]).
    /// - Out of date (resize, rotation): rebuilt and retried once, so no frame is lost.
    /// - Suboptimal: the image is still returned. The swapchain is rebuilt on a later call,
    ///   at most once every 120 frames, because rebuilding waits for the GPU to go idle.
    ///   Size changes don't wait: they come through [`Self::resize`].
    /// - Timeout: the frame is skipped and the swapchain rebuilt on the next call.
    /// - Lost: the frame is skipped and the surface left alone for the app to recreate. It is
    ///   never reconfigured here, because on Android the native window is already destroyed
    ///   by the time the driver reports it.
    pub fn acquire(&mut self) -> SurfaceAcquire<'_> {
        profiling::scope!("RenderSurface::acquire");
        let Self { surface, state, .. } = self;
        let Some(state) = state else {
            return SurfaceAcquire::Skip(SkipReason::Unconfigured);
        };
        if state.device_lost() {
            return SurfaceAcquire::Skip(SkipReason::DeviceLost);
        }
        if state.lost {
            return SurfaceAcquire::Skip(SkipReason::Lost);
        }
        if state.size.0 == 0 || state.size.1 == 0 {
            return SurfaceAcquire::Skip(SkipReason::ZeroSize);
        }
        if state.dirty && !state.apply(surface) {
            return SurfaceAcquire::Skip(SkipReason::Lost);
        }
        state.frames_since_configure = state.frames_since_configure.saturating_add(1);
        for _ in 0..2 {
            let texture = match surface.get_current_texture() {
                CurrentSurfaceTexture::Success(texture) => texture,
                CurrentSurfaceTexture::Suboptimal(texture) => {
                    if state.frames_since_configure >= SUBOPTIMAL_REBUILD_INTERVAL {
                        tracing::debug!(target: LOG_TARGET, "swapchain suboptimal; rebuilding it");
                        state.dirty = true;
                    }
                    texture
                }
                CurrentSurfaceTexture::Outdated => {
                    if !state.apply(surface) {
                        return SurfaceAcquire::Skip(SkipReason::Lost);
                    }
                    continue;
                }
                CurrentSurfaceTexture::Timeout => {
                    // A swapchain that keeps timing out (seen on some Android drivers after
                    // rotation) only recovers by being rebuilt.
                    state.dirty = true;
                    return SurfaceAcquire::Skip(SkipReason::Timeout);
                }
                CurrentSurfaceTexture::Occluded => return SurfaceAcquire::Skip(SkipReason::Occluded),
                CurrentSurfaceTexture::Lost => {
                    tracing::warn!(target: LOG_TARGET, "surface lost; skipping frames until it is recreated");
                    state.lost = true;
                    return SurfaceAcquire::Skip(SkipReason::Lost);
                }
                CurrentSurfaceTexture::Validation => {
                    return SurfaceAcquire::Skip(state.validation_or_device_lost());
                }
            };
            let view = texture.texture.create_view(&wgpu::TextureViewDescriptor {
                label: state.labels.then_some("surface frame"),
                format: Some(state.view_format),
                ..Default::default()
            });
            let size = (texture.texture.width(), texture.texture.height());
            return SurfaceAcquire::Frame(SurfaceFrame { texture: Some(texture), view, size, state });
        }
        SurfaceAcquire::Skip(SkipReason::Outdated)
    }

    pub(crate) fn raw(&self) -> &wgpu::Surface<'static> {
        &self.surface
    }
}

/// Picks `(swapchain format, view format)`. The view format is always sRGB so shading stays
/// linear and the hardware encodes on write.
fn choose_format(formats: &[TextureFormat], can_reinterpret: bool) -> Option<(TextureFormat, TextureFormat)> {
    const PREFERRED: [TextureFormat; 2] = [TextureFormat::Bgra8UnormSrgb, TextureFormat::Rgba8UnormSrgb];
    if let Some(&format) = PREFERRED.iter().find(|f| formats.contains(f)) {
        return Some((format, format));
    }
    if let Some(&format) = formats.iter().find(|f| f.is_srgb()) {
        return Some((format, format));
    }
    // Android's Vulkan backend can't reinterpret swapchain formats, but it always offers an
    // sRGB format, so this path is for desktop compositors that list only UNORM formats.
    if can_reinterpret {
        return formats.iter().find(|f| f.add_srgb_suffix() != **f).map(|&f| (f, f.add_srgb_suffix()));
    }
    None
}

fn choose_alpha(supported: &[CompositeAlphaMode]) -> CompositeAlphaMode {
    if supported.contains(&CompositeAlphaMode::Opaque) {
        CompositeAlphaMode::Opaque
    } else {
        // Some Android compositors only offer Inherit; Auto picks it.
        CompositeAlphaMode::Auto
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TextureFormat as F;

    #[test]
    fn prefers_8bit_srgb_formats() {
        let metal = [F::Bgra8Unorm, F::Bgra8UnormSrgb, F::Rgba16Float, F::Rgb10a2Unorm];
        assert_eq!(choose_format(&metal, true), Some((F::Bgra8UnormSrgb, F::Bgra8UnormSrgb)));
        let android = [F::Rgba8Unorm, F::Rgba8UnormSrgb, F::Rgb10a2Unorm];
        assert_eq!(choose_format(&android, false), Some((F::Rgba8UnormSrgb, F::Rgba8UnormSrgb)));
    }

    #[test]
    fn reinterprets_unorm_swapchains_only_when_allowed() {
        let unorm_only = [F::Rgb10a2Unorm, F::Bgra8Unorm];
        assert_eq!(choose_format(&unorm_only, true), Some((F::Bgra8Unorm, F::Bgra8UnormSrgb)));
        assert_eq!(choose_format(&unorm_only, false), None);
    }

    #[test]
    fn present_policy_falls_back_to_fifo() {
        let fifo_only = [PresentMode::Fifo];
        assert_eq!(PresentPolicy::Mailbox.resolve(&fifo_only), PresentMode::Fifo);
        assert_eq!(PresentPolicy::Immediate.resolve(&fifo_only), PresentMode::Fifo);
        let all = [PresentMode::Fifo, PresentMode::Mailbox, PresentMode::Immediate];
        assert_eq!(PresentPolicy::Mailbox.resolve(&all), PresentMode::Mailbox);
        assert_eq!(PresentPolicy::default().resolve(&all), PresentMode::Fifo);
    }

    #[test]
    fn alpha_is_opaque_when_offered() {
        assert_eq!(
            choose_alpha(&[CompositeAlphaMode::Inherit, CompositeAlphaMode::Opaque]),
            CompositeAlphaMode::Opaque
        );
        assert_eq!(choose_alpha(&[CompositeAlphaMode::Inherit]), CompositeAlphaMode::Auto);
    }

    /// Drives the whole surface lifecycle against a detached `CAMetalLayer`: creation,
    /// adapter compatibility, configure, acquire/present, resize, zero size, drop and
    /// recreate, then device loss. Run with `cargo test -p sapphire_rhi -- --ignored`.
    #[cfg(target_vendor = "apple")]
    #[test]
    #[ignore = "needs a GPU"]
    fn metal_layer_surface_lifecycle() {
        use crate::{GpuContextDesc, GpuInstanceDesc};

        let layer = objc2_quartz_core::CAMetalLayer::new();
        let target = SurfaceTarget::CoreAnimationLayer(NonNull::from(&*layer).cast());
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        // SAFETY: `layer` is a live CAMetalLayer, and the surface holds a reference to it.
        let mut surface = unsafe { RenderSurface::new(&instance, target, layer.clone()) }.expect("surface");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), Some(&surface)).expect("device");

        assert!(matches!(surface.acquire(), SurfaceAcquire::Skip(SkipReason::Unconfigured)));
        surface.configure(&gpu, 64, 32, PresentPolicy::Mailbox).expect("configure");
        assert!(surface.format().expect("configured").is_srgb());

        for size in [(64, 32), (48, 48), (48, 48)] {
            surface.resize(size.0, size.1);
            let SurfaceAcquire::Frame(frame) = surface.acquire() else {
                panic!("no frame at {size:?}");
            };
            assert_eq!(frame.size(), size);
            present_cleared(&gpu, frame);
        }

        surface.resize(0, 48);
        assert!(matches!(surface.acquire(), SurfaceAcquire::Skip(SkipReason::ZeroSize)));

        // What an Android background/foreground cycle does: drop, recreate, reconfigure.
        drop(surface);
        // SAFETY: as above.
        let mut surface =
            unsafe { RenderSurface::new(gpu.instance(), target, layer.clone()) }.expect("recreate");
        surface.configure(&gpu, 16, 16, PresentPolicy::Fifo).expect("reconfigure");
        let SurfaceAcquire::Frame(frame) = surface.acquire() else { panic!("no frame after recreate") };
        present_cleared(&gpu, frame);

        // Device loss is reported before anything else, and stays reported. wgpu notices a
        // destroyed device once its queue is idle; with an idle queue, acquire's own poll does it.
        let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
        gpu.device().destroy();
        for _ in 0..3 {
            assert!(matches!(surface.acquire(), SurfaceAcquire::Skip(SkipReason::DeviceLost)));
        }
        assert!(gpu.is_lost());
    }

    #[cfg(target_vendor = "apple")]
    fn present_cleared(gpu: &GpuContext, frame: SurfaceFrame<'_>) {
        let mut encoder = gpu.device().create_command_encoder(&Default::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        gpu.queue().submit([encoder.finish()]);
        frame.present();
    }
}
