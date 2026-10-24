//! The sandbox [`App`]: window, GPU, renderer, clocks and the frame loop.

use std::time::Duration;

use sapphire_assets::mesh::MeshAsset;
use sapphire_math::{Mat4, Vec3};
use sapphire_platform::raw_window_handle::{HandleError, HasDisplayHandle, HasWindowHandle};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, DeviceClass, Event, GamepadButton, GamepadEvent, KeyEvent,
    LifecycleEvent, Platform, Scancode, SurfaceHandle, Window, WindowDesc, WindowEvent,
};
use sapphire_render::{
    DirectionalLight, FrameView, MeshHandle, MeshInstance, RenderSettings, Renderer, SkyGradient,
};
use sapphire_rhi::{
    GpuContext, GpuContextDesc, GpuDebug, GpuInstance, GpuInstanceDesc, PresentPolicy, RenderSurface,
    SkipReason, SurfaceAcquire, SurfaceTarget,
};
use sapphire_time::{FixedStepConfig, FixedStepDriver, RealClock, ScaleHandle, ScaleSource, ScaledClock};

use crate::LOG_TARGET;
use crate::input::InputState;
use crate::options::Options;
use crate::orbit::OrbitCamera;
use crate::stats::FrameStats;
use crate::turntable::Turntable;

/// The cooked sample character, produced by `cargo xtask content`.
const CHARACTER_MESH: &str = "samples/mannequin.smesh";
/// World slow motion while toggled, in per-mille (perfect-dodge style, spec §4.5).
const SLOW_MOTION: u32 = 200;
/// Fixed steps the `H` key freezes the turntable for.
const HITSTOP_TICKS: u16 = 8;
/// Consecutive lost-surface frames before the surface is rebuilt from a fresh native handle.
const LOST_FRAMES_BEFORE_REBUILD: u32 = 3;
/// Loop period while nothing can be presented (occluded, minimized, backgrounded).
const IDLE_FRAME: Duration = Duration::from_millis(33);

const SUN: DirectionalLight = DirectionalLight {
    direction: Vec3::new(0.55, -0.7, -0.45),
    color: Vec3::new(1.0, 0.97, 0.92),
    intensity: 1.0,
};

pub struct SandboxApp {
    // Fields drop in declaration order: GPU objects go before the window they present to.
    // Crash capture is process-wide (see `crate::startup`) and outlives the app.
    renderer: Renderer,
    surface: Option<RenderSurface>,
    /// Keeps the native window `surface` draws into alive; dropped right after it.
    surface_handle: Option<SurfaceHandle>,
    gpu: GpuContext,
    window: Window,

    character: MeshHandle,
    /// Reused every frame so building the frame view doesn't allocate.
    instances: Vec<MeshInstance>,
    camera: OrbitCamera,
    input: InputState,
    /// Mobile SDL reports touches only; desktop reports the mouse.
    touch_device: bool,

    real: RealClock,
    world: ScaledClock,
    driver: FixedStepDriver,
    turntable: Turntable,
    slow_motion: Option<ScaleHandle>,
    background_pause: Option<ScaleHandle>,
    /// Between `WillEnterBackground` and `DidEnterForeground`: no GPU work allowed (spec §6.5).
    suspended: bool,
    lost_frames: u32,

    stats: FrameStats,
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

        // The Android emulator's Vulkan driver (gfxstream) crashes naming objects.
        let debug = GpuDebug { object_labels: !info.is_emulator, ..GpuDebug::default() };
        let instance = GpuInstance::new(&GpuInstanceDesc { debug, ..GpuInstanceDesc::default() })?;
        let surface_handle = window.surface_handle()?;
        // SAFETY: `surface_handle` keeps the native window alive and is dropped after the
        // surface (field order).
        let mut surface = unsafe { RenderSurface::new(&instance, surface_target(&surface_handle)?)? };
        let gpu = GpuContext::new(
            &instance,
            &GpuContextDesc { pipeline_cache_dir: Some(platform.cache_dir().join("pipeline_cache")) },
            Some(&surface),
        )?;
        tracing::info!(target: LOG_TARGET, caps = ?gpu.caps(), "GPU ready");

        let size = window.pixel_size();
        surface.configure(&gpu, size.width, size.height, PresentPolicy::Fifo)?;
        let format =
            surface.format().ok_or_else(|| AppInitError::msg("configured surface reports no format"))?;
        let mut renderer = Renderer::new(&gpu, format, RenderSettings { msaa: options.msaa })?;
        renderer.resize(size.width, size.height);

        let mesh = load_mesh(platform, CHARACTER_MESH)?;
        let character = renderer.upload_mesh(&mesh)?;
        tracing::info!(
            target: LOG_TARGET,
            path = CHARACTER_MESH,
            vertices = mesh.vertices.len(),
            triangles = mesh.indices.len() / 3,
            "character loaded"
        );

        // Entity clocks are issued by the world clock they run under.
        let mut world = ScaledClock::new();
        let turntable = Turntable::new(&mut world);
        Ok(Self {
            renderer,
            surface: Some(surface),
            surface_handle: Some(surface_handle),
            gpu,
            window,
            character,
            instances: Vec::with_capacity(1),
            camera: OrbitCamera::framing(mesh.bounds),
            input: InputState::default(),
            touch_device: info.device_class == DeviceClass::Mobile,
            real: RealClock::new(),
            world,
            driver: FixedStepDriver::new(FixedStepConfig::default()),
            turntable,
            slow_motion: None,
            background_pause: None,
            suspended: false,
            lost_frames: 0,
            stats: FrameStats::new(),
            frame_limit: options.frame_limit,
        })
    }

    fn event(&mut self, _platform: &mut Platform, event: &Event) -> AppControl {
        match event {
            Event::Quit { .. } => return AppControl::Exit,
            Event::Window { event, .. } => match event {
                WindowEvent::CloseRequested => return AppControl::Exit,
                WindowEvent::PixelSizeChanged { width, height } => self.resize(*width, *height),
                _ => {}
            },
            Event::Lifecycle { event, .. } => self.lifecycle(*event),
            Event::Key(key) => return self.key(key),
            Event::MouseButton(button) if !self.touch_device => {
                self.input.mouse_button(button.button, button.down)
            }
            Event::MouseMotion(motion) if !self.touch_device => {
                self.input.mouse_motion(&mut self.camera, motion.dx, motion.dy);
            }
            Event::MouseWheel(wheel) if !self.touch_device => {
                InputState::mouse_wheel(&mut self.camera, wheel.y)
            }
            Event::Touch(touch) => self.input.touch(&mut self.camera, touch),
            Event::Gamepad { event, .. } => match *event {
                GamepadEvent::Axis { axis, value } => self.input.gamepad_axis(axis, value),
                GamepadEvent::Button { button: GamepadButton::South, down: true } => {
                    self.toggle_slow_motion()
                }
                GamepadEvent::Button { button: GamepadButton::East, down: true } => self.hitstop(),
                _ => {}
            },
            _ => {}
        }
        AppControl::Continue
    }

    fn iterate(&mut self, _platform: &mut Platform) -> AppControl {
        let frame_time = self.real.update();
        if self.suspended {
            std::thread::sleep(IDLE_FRAME);
            return AppControl::Continue;
        }

        let steps = {
            profiling::scope!("simulate");
            let steps = self.driver.advance(frame_time);
            for _ in 0..steps.count {
                self.world.tick();
                self.turntable.step(&self.world);
            }
            steps
        };

        // Presentation runs on the real clock, so the camera stays responsive in slow motion.
        let dt = frame_time.as_secs_f32();
        self.input.update(&mut self.camera, dt);
        self.camera.update(dt);
        let presented = self.render(steps.alpha);
        if !presented {
            // Nothing blocked on vsync this frame (occluded, minimized, no surface), so pace
            // the loop here instead of spinning a core.
            std::thread::sleep(IDLE_FRAME);
        }

        self.stats.record(frame_time, steps.dropped, presented);
        profiling::finish_frame!();

        match self.frame_limit {
            Some(limit) if self.real.frames() >= limit => AppControl::Exit,
            _ => AppControl::Continue,
        }
    }

    fn quit(&mut self, _platform: &mut Platform) {
        self.save_pipeline_cache();
        tracing::info!(target: LOG_TARGET, frames = self.real.frames(), "quit");
    }
}

impl SandboxApp {
    /// Draws and presents one frame. Returns whether a frame was presented.
    fn render(&mut self, alpha: f32) -> bool {
        profiling::scope!("render");
        let Some(surface) = self.surface.as_mut() else { return false };
        let frame = match surface.acquire() {
            SurfaceAcquire::Frame(frame) => frame,
            SurfaceAcquire::Skip(reason) => {
                self.stats.skipped(reason);
                if reason == SkipReason::Lost {
                    self.lost_frames += 1;
                    if self.lost_frames >= LOST_FRAMES_BEFORE_REBUILD {
                        self.rebuild_surface();
                    }
                }
                return false;
            }
        };
        self.lost_frames = 0;

        let (width, height) = frame.size();
        let aspect = width as f32 / height.max(1) as f32;
        self.instances.clear();
        self.instances.push(MeshInstance {
            mesh: self.character,
            transform: Mat4::from_rotation_y(self.turntable.angle(alpha)),
        });
        let view = FrameView {
            camera: self.camera.camera_data(aspect),
            sun: SUN,
            instances: &self.instances,
            sky: SkyGradient::default(),
            time_seconds: self.world.now().as_secs_f64() as f32,
        };
        self.renderer.render(&self.gpu, frame.view(), &view);
        frame.present();
        true
    }

    fn resize(&mut self, width: u32, height: u32) {
        if let Some(surface) = &mut self.surface {
            surface.resize(width, height);
        }
        self.renderer.resize(width, height);
    }

    fn key(&mut self, key: &KeyEvent) -> AppControl {
        if !key.down {
            return AppControl::Continue;
        }
        match key.scancode {
            Scancode::ESCAPE if !key.repeat => return AppControl::Exit,
            Scancode::SPACE if !key.repeat => self.toggle_slow_motion(),
            Scancode::H if !key.repeat => self.hitstop(),
            Scancode::LEFT => InputState::key_orbit(&mut self.camera, 1.0, 0.0),
            Scancode::RIGHT => InputState::key_orbit(&mut self.camera, -1.0, 0.0),
            Scancode::UP => InputState::key_orbit(&mut self.camera, 0.0, 1.0),
            Scancode::DOWN => InputState::key_orbit(&mut self.camera, 0.0, -1.0),
            _ => {}
        }
        AppControl::Continue
    }

    fn toggle_slow_motion(&mut self) {
        match self.slow_motion.take() {
            Some(handle) => {
                self.world.pop_scale(handle);
            }
            None => self.slow_motion = self.world.push_scale(ScaleSource::PerfectDodge, SLOW_MOTION).ok(),
        }
        tracing::info!(target: LOG_TARGET, world_scale = self.world.effective_scale(), "slow motion toggled");
    }

    fn hitstop(&mut self) {
        self.turntable.hitstop(HITSTOP_TICKS);
        tracing::debug!(target: LOG_TARGET, ticks = HITSTOP_TICKS, "hitstop");
    }

    /// Spec §6.5: no GPU work in the background. The surface is dropped going down (Android
    /// destroys its native window) and rebuilt coming back, the same way on every platform.
    fn lifecycle(&mut self, event: LifecycleEvent) {
        tracing::info!(target: LOG_TARGET, ?event, "lifecycle");
        match event {
            LifecycleEvent::WillEnterBackground => {
                self.suspended = true;
                self.surface = None;
                self.surface_handle = None;
                if self.background_pause.is_none() {
                    self.background_pause = self.world.push_scale(ScaleSource::Pause, 0).ok();
                }
                self.save_pipeline_cache();
            }
            LifecycleEvent::DidEnterForeground => {
                self.suspended = false;
                if let Some(handle) = self.background_pause.take() {
                    self.world.pop_scale(handle);
                }
                if self.surface.is_none() {
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
    /// running without a surface and retries on the next foreground or lost frame.
    fn rebuild_surface(&mut self) {
        self.surface = None;
        self.surface_handle = None;
        self.lost_frames = 0;
        let size = self.window.pixel_size();
        let result = self.window.surface_handle().map_err(|error| error.to_string()).and_then(|handle| {
            let target = surface_target(&handle).map_err(|error| error.to_string())?;
            // SAFETY: as in `init`: `handle` is stored next to the surface and outlives it.
            let mut surface = unsafe { RenderSurface::new(self.gpu.instance(), target) }
                .map_err(|error| error.to_string())?;
            surface
                .configure(&self.gpu, size.width, size.height, PresentPolicy::Fifo)
                .map_err(|error| error.to_string())?;
            Ok((surface, handle))
        });
        match result {
            Ok((surface, handle)) => {
                self.surface = Some(surface);
                self.surface_handle = Some(handle);
                self.renderer.resize(size.width, size.height);
                tracing::info!(target: LOG_TARGET, width = size.width, height = size.height, "surface rebuilt");
            }
            Err(error) => tracing::warn!(target: LOG_TARGET, %error, "surface rebuild failed"),
        }
    }

    fn save_pipeline_cache(&self) {
        if let Err(error) = self.gpu.save_pipeline_cache() {
            tracing::warn!(target: LOG_TARGET, %error, "pipeline cache not saved");
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

fn load_mesh(platform: &Platform, path: &str) -> Result<MeshAsset, AppInitError> {
    let bytes = platform.content().read(path).map_err(|error| {
        AppInitError::msg(format!(
            "{error} (content root {:?}; cook it with `cargo xtask content`)",
            platform.content().root()
        ))
    })?;
    Ok(MeshAsset::from_bytes(&bytes)?)
}
