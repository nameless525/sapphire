//! The sandbox [`App`]: window, GPU, renderer, clocks and the frame loop.

use std::time::Duration;

use sapphire_platform::raw_window_handle::{HandleError, HasDisplayHandle, HasWindowHandle};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, DeviceClass, Event, LifecycleEvent, Os, Platform,
    SurfaceHandle, Window, WindowDesc, WindowEvent,
};
use sapphire_render::graph::{FrameGraph, GraphError, GraphExecutor};
use sapphire_render::{MeshHandle, RenderSettings, Renderer};
use sapphire_rhi::{
    GpuContext, GpuContextDesc, GpuDebug, GpuInstance, GpuInstanceDesc, PipelineCacheDesc, PresentPolicy,
    RenderSurface, SkipReason, SurfaceAcquire, SurfaceFrame, SurfaceTarget, wgpu,
};
use sapphire_time::{
    FixedStepConfig, FixedStepDriver, FixedSteps, RealClock, ScaleHandle, ScaleSource, Stopwatch,
};

use crate::LOG_TARGET;
#[cfg(feature = "dev")]
use crate::dev::{DevTools, FrameState};
use crate::input::SandboxInput;
use crate::options::Options;
use crate::scene::{self, Scene};
use crate::stats::{FrameStats, Stages};

/// Loop period while nothing can be presented (occluded, minimized, backgrounded).
const IDLE_FRAME: Duration = Duration::from_millis(33);
/// Longest real-time step presentation (camera springs, held input) takes in one frame, so a
/// hitch can't fling the camera.
const MAX_PRESENTATION_DT: Duration = Duration::from_millis(100);
/// `--frames N` runs fail unless N frames were presented within this long (or 50 ms per frame,
/// whichever is longer), so an unattended run that never drew anything can't pass.
const FRAME_LIMIT_MIN_DEADLINE: Duration = Duration::from_secs(10);

pub struct SandboxApp {
    // Fields drop in declaration order: GPU objects go before the window they present to.
    // Crash capture is process-wide (see `crate::startup`) and outlives the app.
    /// The debug overlay and its cvars.
    #[cfg(feature = "dev")]
    dev: DevTools,
    renderer: Renderer,
    graph: GraphExecutor,
    surface: Option<RenderSurface>,
    gpu: GpuContext,
    window: Window,
    recovery: SurfaceRecovery,
    /// How the GPU stack is built, kept to rebuild it after device loss.
    gpu_debug: GpuDebug,
    settings: RenderSettings,

    scene: Scene,
    input: SandboxInput,

    real: RealClock,
    driver: FixedStepDriver,
    background_pause: Option<ScaleHandle>,
    /// Between `WillEnterBackground` and `DidEnterForeground`: no GPU work allowed (spec §6.5).
    suspended: bool,

    stats: FrameStats,
    /// The latest frame's CPU time per stage.
    stages: Stages,
    presented_frames: u64,
    frame_limit: Option<u64>,
}

impl App for SandboxApp {
    const METADATA: AppMetadata = AppMetadata {
        name: "Sapphire Sandbox",
        organization: "Sapphire",
        identifier: "dev.sapphire.sandbox",
        version: env!("CARGO_PKG_VERSION"),
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let (options, unknown) = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        if !unknown.is_empty() {
            tracing::warn!(target: LOG_TARGET, ?unknown, "ignoring unknown arguments");
        }
        let info = *platform.system_info();
        tracing::info!(target: LOG_TARGET, version = Self::METADATA.version, ?info, ?options, "starting");

        let window = platform
            .create_window(&WindowDesc { title: Self::METADATA.name.to_owned(), ..WindowDesc::default() })?;

        // Narrow the build's defaults, never widen them: the Android emulator's Vulkan driver
        // (gfxstream) crashes naming objects, so labels go off there and only there.
        let base = GpuDebug::default();
        let debug = GpuDebug {
            object_labels: base.object_labels && !(info.is_emulator && info.os == Os::Android),
            ..base
        };
        let settings = options.render_settings(info.device_class);
        let stack = create_gpu_stack(platform, &window, debug, settings)?;
        // Desktop-only bindings, such as `Esc` quitting, are a context of their own.
        let input = SandboxInput::new(info.device_class == DeviceClass::Desktop, window.size())?;
        let mut scene = Scene::with_character(stack.character, stack.character_bounds);
        if options.lights {
            scene.spawn_demo_lights();
        }
        #[cfg(feature = "dev")]
        let dev = DevTools::new(&stack.gpu, stack.format, options.debug_overlay.as_deref())?;
        #[cfg(not(feature = "dev"))]
        if options.debug_overlay.is_some() {
            tracing::warn!(target: LOG_TARGET, "--debug-overlay needs a `dev` build (`--features dev`)");
        }
        Ok(Self {
            #[cfg(feature = "dev")]
            dev,
            renderer: stack.renderer,
            graph: stack.graph,
            surface: Some(stack.surface),
            gpu: stack.gpu,
            window,
            recovery: SurfaceRecovery::default(),
            gpu_debug: debug,
            settings,
            scene,
            input,
            real: RealClock::new(),
            driver: FixedStepDriver::new(FixedStepConfig::default()),
            background_pause: None,
            suspended: false,
            stats: FrameStats::new(),
            stages: Stages::default(),
            presented_frames: 0,
            frame_limit: options.frame_limit,
        })
    }

    fn event(&mut self, _platform: &mut Platform, event: &Event) -> AppControl {
        // Input queues for the next frame. Losing focus or the foreground releases everything
        // held, since the releases may never arrive. With the debug overlay, it sees each event
        // first and passes on what it doesn't use.
        #[cfg(feature = "dev")]
        self.dev.handle_event(event, &mut self.input);
        #[cfg(not(feature = "dev"))]
        self.input.handle_event(event);
        match event {
            Event::Quit { .. } => return AppControl::Exit,
            Event::Window { event, .. } => match *event {
                WindowEvent::CloseRequested => return AppControl::Exit,
                WindowEvent::PixelSizeChanged { width, height } => self.resize(width, height),
                // SDL reports no 0×0 size on minimize; park the surface so nothing is acquired.
                WindowEvent::Minimized => self.resize(0, 0),
                WindowEvent::Restored => {
                    let size = self.window.pixel_size();
                    self.resize(size.width, size.height);
                }
                _ => {}
            },
            Event::Lifecycle { event, .. } => self.lifecycle(*event),
            _ => {}
        }
        AppControl::Continue
    }

    fn iterate(&mut self, platform: &mut Platform) -> AppControl {
        let frame_time = self.real.update();
        // The previous frame's stages go with its length.
        #[cfg(feature = "dev")]
        self.dev.begin_frame(frame_time, Some(&self.stages));
        let mut stopwatch = Stopwatch::start();
        // On the clock event timestamps use, so every trigger is judged on one timeline.
        #[cfg(feature = "dev")]
        self.dev.poll(platform.now(), &mut self.input);
        if self.input.update(platform.now()) {
            return AppControl::Exit;
        }
        self.stages.input = stopwatch.lap();
        if self.suspended {
            std::thread::sleep(IDLE_FRAME);
            return self.frame_limit_control().unwrap_or(AppControl::Continue);
        }
        if self.gpu.is_lost() {
            if self.recovery.due(self.real.elapsed()) {
                self.recover_from_device_loss(platform);
            }
        } else if self.surface.is_none() && self.recovery.due(self.real.elapsed()) {
            self.rebuild_surface();
        }
        // Recovery is no stage of the frame.
        stopwatch.lap();

        let steps = self.simulate(frame_time);
        self.stages.simulation = stopwatch.lap();
        self.stages.fixed_steps = steps.count;

        // Presentation runs on the real clock, so the camera stays responsive in slow motion.
        let dt = frame_time.min(MAX_PRESENTATION_DT).as_secs_f32();
        let mut camera = self.scene.camera_mut();
        self.input.drive_camera(&mut camera);
        camera.update(dt);
        let presentation = stopwatch.lap();
        let presented = match self.render(steps.alpha, &mut stopwatch, presentation) {
            Ok(presented) => presented,
            Err(error) => {
                // The graph is built in code, so it would fail the same way every frame.
                tracing::error!(target: LOG_TARGET, %error, "invalid frame graph");
                return AppControl::Fail;
            }
        };
        if presented {
            self.presented_frames += 1;
            self.recovery.reset();
            // Rendering works with this pipeline cache, so the next boot may trust it.
            self.gpu.confirm_pipeline_cache();
        } else {
            // Nothing blocked on vsync this frame (occluded, minimized, no surface), so pace
            // the loop here instead of spinning a core.
            std::thread::sleep(IDLE_FRAME);
        }

        self.scene.end_frame();
        self.stats.record(frame_time, steps.dropped, presented, &self.stages);
        profiling::finish_frame!();
        self.frame_limit_control().unwrap_or(AppControl::Continue)
    }

    fn quit(&mut self, _platform: &mut Platform) {
        self.save_pipeline_cache();
        tracing::info!(target: LOG_TARGET, presented = self.presented_frames, "quit");
    }
}

impl SandboxApp {
    /// Runs this frame's fixed steps. Queued input lands on the first of them (rule R3), and
    /// the steps themselves must not allocate (rule R6).
    fn simulate(&mut self, frame_time: Duration) -> FixedSteps {
        profiling::scope!("simulate");
        let steps = self.driver.advance(frame_time);
        if steps.count == 0 {
            return steps;
        }
        self.scene.apply(self.input.take_actions());

        let scene = &mut self.scene;
        #[cfg(feature = "dev")]
        let dev = &mut self.dev;
        let ((), allocations) = sapphire_core::alloc::count_in_scope(|| {
            for _ in 0..steps.count {
                // The overlay's pause, step and speed, applied to the step about to run.
                #[cfg(feature = "dev")]
                dev.before_fixed_step(scene);
                scene.step();
            }
        });
        // Only exact where counts are per thread (spec R6; `PER_THREAD_COUNTS`).
        debug_assert!(
            !sapphire_core::alloc::PER_THREAD_COUNTS || allocations.allocations == 0,
            "rule R6: the fixed step allocated {} times",
            allocations.allocations
        );
        steps
    }

    /// Extracts the frame, then draws and presents it if the surface gave an image. Returns
    /// whether a frame was presented. `stopwatch` times the extract and render stages, which
    /// `presentation` (already measured) joins; acquiring and presenting wait for vsync and
    /// count for neither.
    fn render(
        &mut self,
        alpha: f32,
        stopwatch: &mut Stopwatch,
        presentation: Duration,
    ) -> Result<bool, GraphError> {
        profiling::scope!("render");
        // The frame borrows the surface until the end of this block.
        let skipped = {
            let (frame, skipped) = match self.surface.as_mut().map(RenderSurface::acquire) {
                Some(SurfaceAcquire::Frame(frame)) => (Some(frame), None),
                Some(SurfaceAcquire::Skip(reason)) => (None, Some(reason)),
                None => (None, None),
            };
            stopwatch.lap();
            // Every frame is extracted and prepared, drawn or not, so the renderer's persistent
            // scene sees every packet's deltas. On a lost device the GPU calls do nothing (wgpu
            // drops device-lost errors), and the rebuilt renderer is sent everything.
            let packet = self.scene.frame_packet(alpha, frame.as_ref().map(SurfaceFrame::size));
            self.stages.extract = presentation + stopwatch.lap();
            self.renderer.prepare(&self.gpu, packet);
            if let Some(frame) = frame {
                // The overlay's own frame is no stage of the game's; its panel shows its cost.
                #[cfg(feature = "dev")]
                let overlay = {
                    let started = Stopwatch::start();
                    let (width, height) = frame.size();
                    let window = sapphire_debug::WindowMetrics {
                        pixel_size: sapphire_platform::Size { width, height },
                        ..sapphire_debug::WindowMetrics::of(&self.window, self.dev.focused())
                    };
                    let state = FrameState {
                        window,
                        render: self.renderer.stats(),
                        gpu: &self.gpu,
                        input: Some(&self.input),
                        scene: &self.scene,
                    };
                    let output = self.dev.update(state, &mut self.graph);
                    DevTools::sync_text_input(&self.window, &output);
                    started.elapsed()
                };
                #[cfg(not(feature = "dev"))]
                let overlay = Duration::ZERO;
                let mut graph = FrameGraph::new();
                // The renderer sizes its attachments from the frame itself.
                let target = graph.import_texture("swapchain", frame.view());
                self.renderer.record(&mut graph, target);
                #[cfg(feature = "dev")]
                self.dev.record(&mut graph, target);
                self.graph.execute(&self.gpu, graph)?;
                self.stages.render = stopwatch.lap().saturating_sub(overlay);
                frame.present();
                return Ok(true);
            }
            self.stages.render = stopwatch.lap();
            skipped
        };
        let Some(reason) = skipped else { return Ok(false) };
        self.stats.skipped(reason);
        if reason == SkipReason::Lost {
            // `Lost` is sticky until the surface is rebuilt from a fresh native handle;
            // `recovery` paces the attempts.
            self.surface = None;
        }
        // `DeviceLost` is handled in `iterate`, which rebuilds the whole GPU stack.
        Ok(false)
    }

    fn resize(&mut self, width: u32, height: u32) {
        if let Some(surface) = &mut self.surface {
            surface.resize(width, height);
        }
    }

    /// Spec §6.5: no GPU work in the background. Android also destroys the native window, so
    /// the surface is dropped going down and rebuilt coming back; elsewhere it is kept.
    fn lifecycle(&mut self, event: LifecycleEvent) {
        tracing::info!(target: LOG_TARGET, ?event, "lifecycle");
        match event {
            LifecycleEvent::WillEnterBackground => {
                self.suspended = true;
                if !Os::CURRENT.keeps_surface_in_background() {
                    self.surface = None;
                }
                if self.background_pause.is_none() {
                    self.background_pause =
                        self.scene.world_clock_mut().0.push_scale(ScaleSource::Pause, 0).ok();
                }
                self.save_pipeline_cache();
            }
            LifecycleEvent::DidEnterForeground => {
                // `iterate` doesn't run while suspended on Android (or after a real iOS
                // background), so the next frame would otherwise see the whole pause as one
                // frame: drop that gap before anything steps.
                self.real.update();
                self.suspended = false;
                if let Some(handle) = self.background_pause.take() {
                    self.scene.world_clock_mut().0.pop_scale(handle);
                }
                if self.surface.is_none() {
                    self.recovery.reset();
                    self.rebuild_surface();
                }
            }
            LifecycleEvent::Terminating => self.save_pipeline_cache(),
            LifecycleEvent::DidEnterBackground
            | LifecycleEvent::WillEnterForeground
            | LifecycleEvent::LowMemory => {}
        }
    }

    /// Recreates the surface from a freshly fetched native handle. On failure the app keeps
    /// running without one, and [`SurfaceRecovery`] schedules the next attempt.
    fn rebuild_surface(&mut self) {
        self.surface = None;
        self.recovery.attempted(self.real.elapsed());
        let size = self.window.pixel_size();
        let result = self.window.surface_handle().map_err(|error| error.to_string()).and_then(|handle| {
            let target = surface_target(&handle).map_err(|error| error.to_string())?;
            // SAFETY: the surface owns `handle`, which keeps the native window alive, and drops
            // it right after itself.
            let mut surface = unsafe { RenderSurface::new(self.gpu.instance(), target, handle) }
                .map_err(|error| error.to_string())?;
            surface
                .configure(&self.gpu, size.width, size.height, PresentPolicy::Fifo)
                .map_err(|error| error.to_string())?;
            Ok(surface)
        });
        match result {
            Ok(surface) => {
                self.surface = Some(surface);
                tracing::info!(target: LOG_TARGET, width = size.width, height = size.height, "surface rebuilt");
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, retry_in = ?self.recovery.backoff, "surface rebuild failed");
            }
        }
    }

    /// Rebuilds everything that lived on the lost device (instance, context, surface,
    /// renderer) and re-uploads the character from content, per the recovery contract on
    /// `GpuContext`. On failure the app keeps running without a GPU and [`SurfaceRecovery`]
    /// paces the next attempt.
    fn recover_from_device_loss(&mut self, platform: &Platform) {
        tracing::warn!(target: LOG_TARGET, "GPU device lost; rebuilding the GPU stack");
        // The old surface has to go before a new one is created for the same native window.
        self.surface = None;
        self.recovery.attempted(self.real.elapsed());
        match create_gpu_stack(platform, &self.window, self.gpu_debug, self.settings) {
            Ok(stack) => {
                // Field order matters even here: the renderer goes before the old context.
                #[cfg(feature = "dev")]
                self.dev.recreate_gpu(&stack.gpu, stack.format);
                self.renderer = stack.renderer;
                self.graph = stack.graph;
                self.gpu = stack.gpu;
                self.surface = Some(stack.surface);
                self.scene.set_character_mesh(stack.character);
                // The new renderer's scene starts empty.
                self.scene.resend_instances();
                tracing::info!(target: LOG_TARGET, "GPU stack rebuilt");
            }
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, retry_in = ?self.recovery.backoff, "GPU stack rebuild failed");
            }
        }
    }

    /// `--frames N`: exit successfully once N frames were presented, or fail once the run's
    /// deadline passes without getting there.
    fn frame_limit_control(&self) -> Option<AppControl> {
        let limit = self.frame_limit?;
        if self.presented_frames >= limit {
            return Some(AppControl::Exit);
        }
        let deadline = FRAME_LIMIT_MIN_DEADLINE.max(Duration::from_millis(50).saturating_mul(limit as u32));
        if self.real.elapsed() > deadline {
            tracing::error!(
                target: LOG_TARGET,
                presented = self.presented_frames,
                limit,
                ?deadline,
                "frame limit not reached in time; is the window visible?"
            );
            return Some(AppControl::Fail);
        }
        None
    }

    fn save_pipeline_cache(&self) {
        if let Err(error) = self.gpu.save_pipeline_cache() {
            tracing::warn!(target: LOG_TARGET, %error, "pipeline cache not saved");
        }
    }
}

/// When to try rebuilding a missing surface: at once after it was lost or the app returned to
/// the foreground, then with exponential backoff while attempts keep failing.
#[derive(Debug)]
struct SurfaceRecovery {
    next_attempt: Duration,
    backoff: Duration,
}

impl SurfaceRecovery {
    const INITIAL_BACKOFF: Duration = Duration::from_millis(250);
    const MAX_BACKOFF: Duration = Duration::from_secs(4);

    /// Whether an attempt may run at real time `now`.
    fn due(&self, now: Duration) -> bool {
        now >= self.next_attempt
    }

    /// Records an attempt at `now` and backs off the next one.
    fn attempted(&mut self, now: Duration) {
        self.next_attempt = now + self.backoff;
        self.backoff = (self.backoff * 2).min(Self::MAX_BACKOFF);
    }

    /// A frame was presented, or the app returned to the foreground: the next loss retries at once.
    fn reset(&mut self) {
        *self = Self::default();
    }
}

impl Default for SurfaceRecovery {
    fn default() -> Self {
        Self { next_attempt: Duration::ZERO, backoff: Self::INITIAL_BACKOFF }
    }
}

/// Everything that lives on the GPU device: built at startup and again after device loss.
struct GpuStack {
    renderer: Renderer,
    graph: GraphExecutor,
    surface: RenderSurface,
    gpu: GpuContext,
    /// The surface's format, which the renderer and the overlay draw into.
    #[cfg_attr(not(feature = "dev"), allow(dead_code))]
    format: wgpu::TextureFormat,
    character: MeshHandle,
    character_bounds: [[f32; 3]; 2],
}

fn create_gpu_stack(
    platform: &Platform,
    window: &Window,
    debug: GpuDebug,
    settings: RenderSettings,
) -> Result<GpuStack, AppInitError> {
    let instance = GpuInstance::new(&GpuInstanceDesc { debug, ..GpuInstanceDesc::default() })?;
    let handle = window.surface_handle()?;
    let target = surface_target(&handle)?;
    // SAFETY: the surface owns `handle`, which keeps the native window alive, and drops it
    // right after itself.
    let mut surface = unsafe { RenderSurface::new(&instance, target, handle)? };
    let pipeline_cache = PipelineCacheDesc {
        dir: platform.cache_dir().join("pipeline_cache"),
        build_id: SandboxApp::METADATA.version.to_owned(),
    };
    let gpu =
        GpuContext::new(&instance, &GpuContextDesc { pipeline_cache: Some(pipeline_cache) }, Some(&surface))?;
    tracing::info!(target: LOG_TARGET, caps = ?gpu.caps(), "GPU ready");

    let size = window.pixel_size();
    surface.configure(&gpu, size.width, size.height, PresentPolicy::Fifo)?;
    let format = surface.format().ok_or_else(|| AppInitError::msg("configured surface reports no format"))?;
    let mut renderer = Renderer::new(&gpu, format, settings)?;

    let (character, character_bounds) = scene::load_mesh(platform, scene::CHARACTER_MESH, |mesh| {
        renderer.upload_mesh(mesh).map(|handle| (handle, scene::mesh_bounds(mesh)))
    })?;
    Ok(GpuStack { renderer, graph: GraphExecutor::new(), surface, gpu, format, character, character_bounds })
}

/// The renderer's surface target for a window's surface handle: its `CAMetalLayer` on
/// Apple platforms, raw native handles elsewhere.
fn surface_target(handle: &SurfaceHandle) -> Result<SurfaceTarget, HandleError> {
    if let Some(layer) = handle.metal_layer() {
        return Ok(SurfaceTarget::CoreAnimationLayer(layer));
    }
    Ok(SurfaceTarget::Raw {
        window: handle.window_handle()?.as_raw(),
        display: handle.display_handle()?.as_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_matches_the_package_manifest() {
        // xtask packages the app from `[package.metadata.sapphire]`; the identity SDL sees at
        // runtime (user directories, iOS bundle checks) must be the same one.
        let manifest = include_str!("../Cargo.toml");
        let field = |key: &str| {
            manifest
                .lines()
                .find_map(|line| {
                    let value = line.strip_prefix(key)?.trim_start().strip_prefix('=')?.trim();
                    value.strip_prefix('"')?.strip_suffix('"')
                })
                .unwrap_or_else(|| panic!("`{key}` missing from [package.metadata.sapphire]"))
        };
        assert_eq!(SandboxApp::METADATA.identifier, field("bundle-id"));
        assert_eq!(SandboxApp::METADATA.name, field("display-name"));
    }

    #[test]
    fn surface_recovery_backs_off_until_a_frame_is_presented() {
        let mut recovery = SurfaceRecovery::default();
        let at = Duration::from_millis;
        assert!(recovery.due(at(0)));

        let mut now = at(1000);
        let mut gaps = Vec::new();
        for _ in 0..7 {
            assert!(recovery.due(now));
            recovery.attempted(now);
            gaps.push(recovery.next_attempt - now);
            assert!(!recovery.due(now + gaps.last().unwrap().saturating_sub(at(1))));
            now = recovery.next_attempt;
        }
        assert_eq!(gaps, [250, 500, 1000, 2000, 4000, 4000, 4000].map(at));

        recovery.reset();
        assert!(recovery.due(now));
        recovery.attempted(now);
        assert_eq!(recovery.next_attempt - now, at(250));
    }
}
