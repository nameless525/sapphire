//! The sandbox [`App`]: window, GPU, renderer, clocks and the frame loop.

use std::time::Duration;

use sapphire_platform::raw_window_handle::{HandleError, HasDisplayHandle, HasWindowHandle};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, DeviceClass, Event, LifecycleEvent, Os, Platform,
    SurfaceHandle, Window, WindowDesc, WindowEvent,
};
use sapphire_render::graph::{FrameGraph, GraphError, GraphExecutor};
use sapphire_render::quality::{DeviceProfile, FrameTiming, Quality, TierTable};
use sapphire_render::{MaterialHandle, MeshHandle, RenderSettings, Renderer};
use sapphire_rhi::{
    GpuContext, GpuContextDesc, GpuDebug, GpuInstance, GpuInstanceDesc, PipelineCacheDesc, PresentPolicy,
    RenderSurface, SkipReason, SurfaceAcquire, SurfaceFrame, SurfaceTarget, wgpu,
};
use sapphire_time::{
    FixedStepConfig, FixedStepDriver, FixedSteps, RealClock, ScaleHandle, ScaleSource, Stopwatch,
};

use crate::LOG_TARGET;
use crate::content::Content;
#[cfg(feature = "dev")]
use crate::dev::{DevTools, FrameState};
use crate::environment::FOG_CARDS;
use crate::foliage::FoliageMeshes;
use crate::input::SandboxInput;
use crate::options::Options;
use crate::rims::RimRigMeshes;
use crate::scene::Scene;
use crate::stage::StageMeshes;
use crate::stats::{FrameStats, Stages};
use crate::swatches::SWATCHES;
use crate::{rims, skinned, stage, swatches};

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
    /// The asset server and what it uploaded to the renderer.
    content: Content,
    graph: GraphExecutor,
    surface: Option<RenderSurface>,
    gpu: GpuContext,
    window: Window,
    recovery: SurfaceRecovery,
    /// How the GPU stack is built, kept to rebuild it after device loss.
    gpu_debug: GpuDebug,
    /// The surface's format, which the renderer and the overlay draw into.
    format: wgpu::TextureFormat,
    /// The quality tier, whose settings the renderer is made with, and the render scale.
    quality: Quality,
    /// The command line's overrides of the tier's renderer settings.
    options: Options,

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
    /// The last iteration's real length if it was steady, else zero: dynamic resolution's
    /// signal where the GPU has no timings.
    frame_time: Duration,
    /// This iteration has presented and rebuilt nothing so far, so its length measures
    /// rendering alone.
    steady: bool,
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
        let device = create_device(platform, &window, debug)?;
        // The tier comes from the device the GPU stack runs on; the renderer is made for it.
        let profile = DeviceProfile::new(info.device_class, device.gpu.caps(), info.ram_mib);
        let quality = Quality::new(TierTable::shipped(), &profile, options.quality_settings());
        // Frames are paced to the tier's rate (spec §6.2): on iOS the platform's display link asks
        // the display for it.
        platform.set_frame_rate(quality.frame_rate());
        let mut content = Content::new(platform)?;
        // The stage and the rim rig draw the character without its pedestal.
        if options.stage.is_some() || options.rim_rig.is_some() {
            content.keep_character_mesh();
        }
        let renderer = create_renderer(
            &mut content,
            &device.gpu,
            device.format,
            options.render_settings(quality.render_settings()),
            &options,
        )?;
        // Desktop-only bindings, such as `Esc` quitting, are a context of their own.
        let input = SandboxInput::new(info.device_class == DeviceClass::Desktop, window.size())?;
        let mut scene = Scene::with_character(renderer.character, renderer.character_bounds, options.skinned);
        scene.set_sky_material(renderer.sky);
        if let Some(rig) = &options.character_rig {
            scene.set_character_rig(rig).map_err(AppInitError::msg)?;
        }
        if options.lights {
            scene.spawn_demo_lights();
        }
        if let Some(stalk) = renderer.stalk {
            scene.set_stalk_mesh(stalk);
        }
        if let Some(meshes) = renderer.swatches {
            scene.spawn_swatches(meshes);
        }
        if let Some(head) = renderer.face_head {
            scene.spawn_face_rig(head, options.faces.unwrap_or_default());
        }
        if let (Some(layout), Some(meshes)) = (options.rim_rig, renderer.rim_rig) {
            scene.spawn_rim_rig(layout, meshes);
        }
        if let Some(courtyard) = renderer.courtyard {
            scene.spawn_environment(courtyard);
        }
        if let Some(meshes) = renderer.fog_cards {
            scene.spawn_fog_cards(meshes);
        }
        if let (Some(view), Some(meshes)) = (options.foliage, renderer.foliage) {
            scene.spawn_foliage(view, meshes);
        }
        if options.outline_row {
            scene.spawn_outline_row(renderer.character);
        }
        if let Some(mesh) = renderer.hard_edges {
            scene.spawn_hard_edges(mesh);
        }
        // The day, the sky options, the starting view and the turntable's speed, last so `--view`
        // holds whatever framing the scene chose.
        options.apply_to(&mut scene).map_err(AppInitError::msg)?;
        // The stage after the hour, which places its pillars.
        if let (Some(layout), Some(meshes)) = (options.stage, renderer.stage) {
            scene.spawn_stage(layout, meshes);
        }
        #[cfg(feature = "dev")]
        let dev = DevTools::new(&device.gpu, device.format, &options, platform, &content)?;
        #[cfg(not(feature = "dev"))]
        if options.debug_overlay.is_some() {
            tracing::warn!(target: LOG_TARGET, "--debug-overlay needs a `dev` build (`--features dev`)");
        }
        Ok(Self {
            #[cfg(feature = "dev")]
            dev,
            renderer: renderer.renderer,
            content,
            graph: GraphExecutor::new(),
            surface: Some(device.surface),
            gpu: device.gpu,
            window,
            recovery: SurfaceRecovery::default(),
            gpu_debug: debug,
            format: device.format,
            quality,
            frame_limit: options.frame_limit,
            options,
            scene,
            input,
            real: RealClock::new(),
            driver: FixedStepDriver::new(FixedStepConfig::default()),
            background_pause: None,
            suspended: false,
            stats: FrameStats::new(),
            stages: Stages::default(),
            frame_time: Duration::ZERO,
            steady: false,
            presented_frames: 0,
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
        let measured = self.real.update();
        // Where a display link paces frames (iOS), the frame's time is its interval in whole
        // vsyncs, so steady frames run whole fixed steps whatever the scheduler's jitter.
        let frame_time = platform.frame_delta().unwrap_or(measured);
        let pacing = platform.frame_pacing();
        self.frame_time = if self.steady { frame_time } else { Duration::ZERO };
        self.steady = true;
        // The previous frame's stages go with its length.
        #[cfg(feature = "dev")]
        self.dev.begin_frame(frame_time, Some(&self.stages), Some(pacing));
        let mut stopwatch = Stopwatch::start();
        // On the clock event timestamps use, so every trigger is judged on one timeline.
        #[cfg(feature = "dev")]
        self.dev.poll(platform.now(), &mut self.input);
        if self.input.update(platform.now()) {
            return AppControl::Exit;
        }
        self.stages.input = stopwatch.lap();
        if self.suspended {
            self.steady = false;
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
        if !self.gpu.is_lost() {
            self.follow_tier(platform);
            // Packs, shaders and documents changed on disk, swapped in before loading.
            #[cfg(feature = "dev")]
            self.dev.reload(&mut self.content, &mut self.renderer, &mut self.scene);
            // Finished reads, unloads and this frame's uploads, within the upload budget.
            self.content.update(&mut self.renderer, &self.gpu);
        }
        // Recovery, tier changes and loading are no stage of the frame.
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
            self.steady = false;
            std::thread::sleep(IDLE_FRAME);
        }

        self.scene.end_frame();
        self.stats.record(frame_time, steps.dropped, presented, &self.stages, &pacing);
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
            // Before `prepare`, which culls hulls by it.
            self.renderer.set_outline_distance(self.quality.outline_distance());
            self.renderer.prepare(&self.gpu, packet);
            if let Some(frame) = frame {
                let timing = FrameTiming::new(&self.graph, self.frame_time);
                self.renderer.set_render_scale(self.quality.render_scale(frame.size(), &timing));
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
                        quality: &self.quality,
                        gpu_timing: self.quality.wants_gpu_timing(),
                        gpu: &self.gpu,
                        input: Some(&self.input),
                        scene: &self.scene,
                        assets: self.content.stats(),
                    };
                    // It also keeps GPU timing on while dynamic resolution reads it.
                    let output = self.dev.update(state, &mut self.graph);
                    DevTools::sync_text_input(&self.window, &output);
                    started.elapsed()
                };
                #[cfg(not(feature = "dev"))]
                let overlay = {
                    self.graph.set_gpu_timing(&self.gpu, self.quality.wants_gpu_timing());
                    Duration::ZERO
                };
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
        self.steady = false;
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
    /// renderer) and has the asset server upload every asset to it again, per the recovery contract on
    /// `GpuContext`. On failure the app keeps running without a GPU and [`SurfaceRecovery`]
    /// paces the next attempt.
    fn recover_from_device_loss(&mut self, platform: &Platform) {
        tracing::warn!(target: LOG_TARGET, "GPU device lost; rebuilding the GPU stack");
        // The old surface has to go before a new one is created for the same native window.
        self.surface = None;
        self.steady = false;
        self.recovery.attempted(self.real.elapsed());
        let settings = self.render_settings();
        let (content, options) = (&mut self.content, &self.options);
        let stack = create_device(platform, &self.window, self.gpu_debug).and_then(|device| {
            let renderer = create_renderer(content, &device.gpu, device.format, settings, options)?;
            Ok((device, renderer))
        });
        match stack {
            Ok((device, renderer)) => {
                // Field order matters even here: the renderer goes before the old context.
                #[cfg(feature = "dev")]
                self.dev.recreate_gpu(&device.gpu, device.format);
                self.scene.set_meshes(&renderer);
                self.renderer = renderer.renderer;
                #[cfg(feature = "dev")]
                self.dev.renderer_created(&mut self.renderer);
                self.graph = GraphExecutor::new();
                self.gpu = device.gpu;
                self.format = device.format;
                self.surface = Some(device.surface);
                // The new renderer's scene starts empty.
                self.scene.resend_instances();
                tracing::info!(target: LOG_TARGET, "GPU stack rebuilt");
            }
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, retry_in = ?self.recovery.backoff, "GPU stack rebuild failed");
            }
        }
    }

    /// The renderer settings: the quality tier's, with the command line's overrides.
    fn render_settings(&self) -> RenderSettings {
        self.options.render_settings(self.quality.render_settings())
    }

    /// Follows `r.tier`: a new tier gets a new renderer with its settings, on the same device,
    /// as after device loss, and its frame rate. If that fails the old tier and renderer stay.
    fn follow_tier(&mut self, platform: &mut Platform) {
        let Some(previous) = self.quality.update_tier() else { return };
        self.steady = false;
        let settings = self.render_settings();
        match create_renderer(&mut self.content, &self.gpu, self.format, settings, &self.options) {
            Ok(renderer) => {
                self.scene.set_meshes(&renderer);
                self.renderer = renderer.renderer;
                #[cfg(feature = "dev")]
                self.dev.renderer_created(&mut self.renderer);
                // The new renderer's scene starts empty.
                self.scene.resend_instances();
                tracing::info!(target: LOG_TARGET, tier = %self.quality.active().id, "renderer rebuilt for the quality tier");
            }
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, tier = %self.quality.active().id, "no renderer for the quality tier; keeping the last one");
                self.quality.restore(previous);
            }
        }
        platform.set_frame_rate(self.quality.frame_rate());
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

/// The GPU device and the window's surface: built at startup and again after device loss.
struct GpuDevice {
    surface: RenderSurface,
    gpu: GpuContext,
    /// The surface's format, which the renderer and the overlay draw into.
    format: wgpu::TextureFormat,
}

/// A renderer and the meshes it holds: built with the device, and again for a new quality
/// tier.
struct SceneRenderer {
    renderer: Renderer,
    character: MeshHandle,
    character_bounds: [[f32; 3]; 2],
    /// The skinned stalk, with `--skinned`.
    stalk: Option<MeshHandle>,
    /// The shader swatches, with `--swatches`.
    swatches: Option<[MeshHandle; SWATCHES]>,
    /// The face test head, with `--faces`.
    face_head: Option<MeshHandle>,
    /// The rim rig's figure and backdrop, with `--rim-rig`.
    rim_rig: Option<RimRigMeshes>,
    /// The courtyard, with `--environment`.
    courtyard: Option<MeshHandle>,
    /// The courtyard's fog cards, with `--environment` and fog.
    fog_cards: Option<[MeshHandle; FOG_CARDS.len()]>,
    /// The plants, with `--foliage`.
    foliage: Option<FoliageMeshes>,
    /// The hard-edged outline sample, with `--hard-edges`.
    hard_edges: Option<MeshHandle>,
    /// The stage's floor and pillar, with `--stage`.
    stage: Option<StageMeshes>,
    /// The zone's sky material.
    sky: MaterialHandle,
}

impl Scene {
    /// Draws the meshes of a new `renderer`: after device loss or for a new quality tier.
    fn set_meshes(&mut self, renderer: &SceneRenderer) {
        self.set_character_mesh(renderer.character);
        self.set_sky_material(renderer.sky);
        if let Some(stalk) = renderer.stalk {
            self.set_stalk_mesh(stalk);
        }
        if let Some(meshes) = renderer.swatches {
            self.set_swatch_meshes(meshes);
        }
        if let Some(head) = renderer.face_head {
            self.set_face_mesh(head);
        }
        if let Some(meshes) = renderer.rim_rig {
            self.set_rim_rig_meshes(meshes);
        }
        if let Some(courtyard) = renderer.courtyard {
            self.set_courtyard_mesh(courtyard);
        }
        if let Some(meshes) = renderer.fog_cards {
            self.set_fog_card_meshes(meshes);
        }
        if let Some(meshes) = renderer.foliage {
            self.set_foliage_meshes(meshes);
        }
        self.set_outline_row_mesh(renderer.character);
        if let Some(mesh) = renderer.hard_edges {
            self.set_hard_edges_mesh(mesh);
        }
        if let Some(meshes) = renderer.stage {
            self.set_stage_meshes(meshes);
        }
    }
}

fn create_device(platform: &Platform, window: &Window, debug: GpuDebug) -> Result<GpuDevice, AppInitError> {
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
    Ok(GpuDevice { surface, gpu, format })
}

/// What [`create_renderer`] uploads beside the character, as [`SceneRenderer`] holds it.
struct SceneMeshes {
    stalk: Option<MeshHandle>,
    swatches: Option<[MeshHandle; SWATCHES]>,
    face_head: Option<MeshHandle>,
    rim_rig: Option<RimRigMeshes>,
    courtyard: Option<MeshHandle>,
    fog_cards: Option<[MeshHandle; FOG_CARDS.len()]>,
    foliage: Option<FoliageMeshes>,
    hard_edges: Option<MeshHandle>,
    stage: Option<StageMeshes>,
}

/// A renderer drawing into `format` targets with `settings`, holding the character, which
/// `content` uploads to it from the start, and what `options` add beside it: the skinned stalk,
/// the shader swatches, whose materials `content` uploads too, the face test head, the rim rig,
/// the courtyard and its fog cards, the plants, the hard-edged outline sample and the stage.
fn create_renderer(
    content: &mut Content,
    gpu: &GpuContext,
    format: wgpu::TextureFormat,
    settings: RenderSettings,
    options: &Options,
) -> Result<SceneRenderer, AppInitError> {
    let mut renderer = Renderer::new(gpu, format, settings)?;
    // A new renderer starts empty, so every asset goes to it again. If it fails, the content goes
    // back to the renderer the app keeps.
    let previous = content.renderer_rebuilt();
    let scene = content.character(&mut renderer, gpu).and_then(|(character, character_bounds)| {
        let stalk = options.skinned.then(|| skinned::upload_stalk(&mut renderer)).transpose()?;
        let swatches =
            options.swatches.then(|| swatches::upload_swatches(content, gpu, &mut renderer)).transpose()?;
        let face_head = options.faces.map(|_| content.face_head(&mut renderer, gpu)).transpose()?;
        let rim_rig = match options.rim_rig {
            Some(_) => {
                let character = content
                    .character_mesh()
                    .ok_or_else(|| AppInitError::msg("the character isn't loaded"))?;
                Some(rims::upload_rig(&mut renderer, character)?)
            }
            None => None,
        };
        let courtyard = options.environment.then(|| content.courtyard(&mut renderer, gpu)).transpose()?;
        let fog_cards = (options.environment && !options.fog_off)
            .then(|| content.fog_cards(&mut renderer, gpu))
            .transpose()?;
        let foliage = options.foliage.map(|_| content.foliage(&mut renderer, gpu)).transpose()?;
        let hard_edges = options.hard_edges.then(|| content.hard_edges(&mut renderer, gpu)).transpose()?;
        let stage = match options.stage {
            Some(_) => {
                let figure = content
                    .character_mesh()
                    .ok_or_else(|| AppInitError::msg("the character isn't loaded"))?;
                Some(stage::upload_stage(&mut renderer, figure)?)
            }
            None => None,
        };
        let sky = content.sky_material(&mut renderer, gpu)?;
        let meshes = SceneMeshes {
            stalk,
            swatches,
            face_head,
            rim_rig,
            courtyard,
            fog_cards,
            foliage,
            hard_edges,
            stage,
        };
        Ok((character, character_bounds, meshes, sky))
    });
    match scene {
        Ok((character, character_bounds, meshes, sky)) => {
            let SceneMeshes {
                stalk,
                swatches,
                face_head,
                rim_rig,
                courtyard,
                fog_cards,
                foliage,
                hard_edges,
                stage,
            } = meshes;
            Ok(SceneRenderer {
                renderer,
                character,
                character_bounds,
                stalk,
                swatches,
                face_head,
                rim_rig,
                courtyard,
                fog_cards,
                foliage,
                hard_edges,
                stage,
                sky,
            })
        }
        Err(error) => {
            content.keep_renderer(previous);
            Err(error)
        }
    }
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
