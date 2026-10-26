//! [`DebugOverlay`]: the egui context, input routing, panels and drawing, put together.

use std::time::Duration;

use sapphire_assets::AssetStats;
use sapphire_core::alloc::{self, AllocCounts};
use sapphire_core::cvar::CvarRegistry;
use sapphire_input::DeviceKind;
use sapphire_platform::{Event, Scancode, Timestamp, TouchEvent, TouchPhase};
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_render::quality::QualityStats;
use sapphire_render::{CharacterInstance, RenderStats};
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_time::{ScaledClock, Stopwatch};
use sapphire_world::screen_effects::{EffectLibrary, EffectStack};
use sapphire_world::tod::TimeOfDay;

use crate::budget::CpuStages;
use crate::effects::EffectControls;
use crate::history::FrameTimes;
use crate::input::{EguiInput, WindowMetrics};
use crate::render::OverlayRenderer;
use crate::route::{Owner, Router, UiState};
use crate::time::TimeControls;
use crate::time_of_day::TodControls;
use crate::ui::{self, Panels, UiMemory, View};
use crate::{GPU_TIMING, UI_SCALE};

/// How much of each frame's stage times the panel's averages take.
const STAGE_SMOOTHING: f32 = 0.1;

/// How the overlay starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayConfig {
    /// The key that shows and hides it (by position): ` (grave) by default, the key below Esc.
    pub toggle_key: Scancode,
    /// Shown from the first frame.
    pub visible: bool,
    /// The panels open when it is first shown.
    pub panels: Panels,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self { toggle_key: Scancode::GRAVE, visible: false, panels: Panels::default() }
    }
}

/// What the overlay shows for one frame, gathered by the app. Everything is read-only: the
/// overlay changes the world only through cvars and [`TimeControls`].
#[derive(Clone, Copy)]
pub struct OverlayFrame<'a> {
    /// Now, on the platform's event clock: egui's time.
    pub now: Timestamp,
    pub window: WindowMetrics,
    /// The previous frame's process-wide allocations, overlay included.
    pub allocations: AllocCounts,
    pub render: RenderStats,
    /// The quality tier, render scale and dynamic resolution, if the app runs tiers.
    pub quality: Option<QualityStats>,
    /// The executor that ran the previous frame: its stats, report and GPU timings.
    pub graph: &'a GraphExecutor,
    pub gpu: &'a GpuContext,
    /// The device the player used last, and how many gamepads are connected.
    pub input_device: Option<DeviceKind>,
    pub gamepads: usize,
    pub cvars: &'a CvarRegistry,
    pub world_clock: &'a ScaledClock,
    /// The world's time of day, if the app runs one: the time-of-day panel's.
    pub time_of_day: Option<&'a TimeOfDay>,
    /// This frame's characters with their resolved light rigs (M1 C2), as the frame packet
    /// carries them: the time-of-day panel shows them and previews the first.
    pub characters: &'a [CharacterInstance],
    /// The world's screen effects and their presets, if the app runs them (M1 P3): the effects
    /// panel's.
    pub screen_effects: Option<(&'a EffectStack, &'a EffectLibrary)>,
    /// The asset server's counters, if the app runs one.
    pub assets: Option<AssetStats>,
    /// Hot reload, if the app runs it.
    pub hot_reload: Option<HotReloadStatus<'a>>,
    /// Live link, if the app runs it.
    pub live_link: Option<LiveLinkStatus<'a>>,
}

/// What hot reload (M1 A8) did, for the stats panel. Reloaded assets are counted in
/// [`AssetStats`], shader rebuilds in [`RenderStats`].
#[derive(Clone, Copy, Debug)]
pub struct HotReloadStatus<'a> {
    /// What is watched, or why nothing is.
    pub watching: &'a str,
    /// Times new shader sources were built.
    pub shader_rebuilds: u32,
    /// Documents reloaded (the default day, the light rig presets).
    pub documents: u32,
    /// What failed in the last change: shaders that don't compile, assets whose new version
    /// didn't load. Each keeps what it had.
    pub errors: &'a [String],
}

/// Live link's state (M1 A9), for the stats panel.
#[derive(Clone, Copy, Debug)]
pub struct LiveLinkStatus<'a> {
    /// Where it listens or dials, or why it is off.
    pub state: &'a str,
    /// Tools connected now.
    pub connections: u32,
    /// Pushes applied and refused, and pack bytes received.
    pub applied: u32,
    pub refused: u32,
    pub received: u64,
    /// The last refusals; each push refused kept what it would have replaced.
    pub errors: &'a [String],
}

/// What the app should do for the overlay after [`DebugOverlay::update`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OverlayOutput {
    /// A text field has focus: take text input
    /// ([`Window::start_text_input`](sapphire_platform::Window::start_text_input)) while this
    /// holds, and stop when it no longer does.
    pub text_input: bool,
    /// Where the focused text field is, for the input method, when egui placed it this frame.
    pub text_area: Option<TextInputArea>,
}

/// Where the focused text field is, in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextInputArea {
    pub area: sapphire_platform::Rect,
    /// The cursor's offset from the area's left edge.
    pub cursor_x: i32,
}

/// The overlay's own cost in its latest frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayCost {
    /// Building and tessellating the UI.
    pub cpu: Duration,
    pub allocations: u64,
    pub vertices: usize,
}

/// The development overlay. See the [crate docs](crate) for how an app drives it.
pub struct DebugOverlay {
    ctx: egui::Context,
    input: EguiInput,
    router: Router,
    renderer: OverlayRenderer,
    visible: bool,
    /// A text field had focus at the end of the last frame.
    wants_keyboard: bool,
    frame_times: FrameTimes,
    /// Smoothed stage times, once the app has reported any.
    stages: Option<CpuStages>,
    memory: UiMemory,
    time: TimeControls,
    day: TodControls,
    effects: EffectControls,
    cost: OverlayCost,
    gpu_timing: bool,
    max_texture_side: usize,
}

impl std::fmt::Debug for DebugOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugOverlay").field("visible", &self.visible).finish_non_exhaustive()
    }
}

impl DebugOverlay {
    /// An overlay drawing into targets of `format`, the frame's output format.
    pub fn new(gpu: &GpuContext, format: wgpu::TextureFormat, config: OverlayConfig) -> Self {
        let ctx = egui::Context::default();
        ui::style(&ctx);
        Self {
            ctx,
            input: EguiInput::default(),
            router: Router::new(config.toggle_key),
            renderer: OverlayRenderer::new(gpu, format),
            visible: config.visible,
            wants_keyboard: false,
            frame_times: FrameTimes::default(),
            stages: None,
            memory: UiMemory::new(config.panels),
            time: TimeControls::default(),
            day: TodControls::default(),
            effects: EffectControls::default(),
            cost: OverlayCost::default(),
            gpu_timing: false,
            max_texture_side: gpu.caps().limits.max_texture_dimension_2d as usize,
        }
    }

    /// Rebuilds the GPU half on a new device (after device loss). The egui context starts over,
    /// so its textures are sent again; panels, the console and time controls carry over.
    pub fn recreate_gpu(&mut self, gpu: &GpuContext, format: wgpu::TextureFormat) {
        self.renderer = OverlayRenderer::new(gpu, format);
        self.ctx = egui::Context::default();
        ui::style(&self.ctx);
        self.max_texture_side = gpu.caps().limits.max_texture_dimension_2d as usize;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_visible(&mut self, visible: bool) {
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        // Hidden, the overlay runs no frames: drop what it had queued, and start its next
        // frame with nothing held or hovered.
        drop(self.input.take_events());
        self.input.release_all();
        self.renderer.clear();
        self.wants_keyboard = false;
        tracing::debug!(target: crate::LOG_TARGET, visible, "debug overlay toggled");
    }

    /// Routes a platform event: the overlay takes what it uses and passes the rest to `game`
    /// (see [`crate::route`] for the rules). Call it for every event, before the game's input.
    pub fn handle_event(&mut self, event: &Event, game: &mut dyn FnMut(&Event)) {
        let (ctx, input) = (&self.ctx, &self.input);
        let covers = |x: f32, y: f32| {
            let point = input.to_points(x, y);
            ctx.layer_id_at(point).is_some_and(|layer| layer.order != egui::Order::Background)
        };
        let state = UiState { visible: self.visible, wants_keyboard: self.wants_keyboard, covers: &covers };
        let route = self.router.route(event, &state);
        if route.toggle {
            self.set_visible(!self.visible);
        }
        if route.overlay {
            self.input.push(event, route.finger);
        }
        if route.game {
            game(event);
        }
    }

    /// Checks the time-driven toggle gesture (a two-finger hold) at `now`. Call it once per
    /// frame before the game's input update, with the same `game` as [`Self::handle_event`]: it
    /// sends the game a cancel for each finger the gesture takes.
    pub fn poll_gestures(&mut self, now: Timestamp, game: &mut dyn FnMut(&Event)) {
        if !self.router.poll(now) {
            return;
        }
        for claimed in self.router.take_claimed() {
            match claimed.owner {
                Owner::Game => {
                    let cancel = TouchEvent { timestamp: now, phase: TouchPhase::Canceled, ..claimed.touch };
                    game(&Event::Touch(cancel));
                }
                Owner::Overlay if self.visible => self.input.cancel_touch(&claimed.touch, claimed.slot),
                Owner::Overlay | Owner::Nobody => {}
            }
        }
        self.set_visible(!self.visible);
    }

    /// Records one frame's real time and, if the app measured them, its CPU stages. Cheap
    /// enough to call every frame, shown or not, so the frame-time graph has history.
    pub fn record_frame(&mut self, frame_time: Duration, stages: Option<&CpuStages>) {
        self.frame_times.push(frame_time);
        if let Some(stages) = stages {
            self.stages = Some(match &self.stages {
                Some(smoothed) => smoothed.smoothed(stages, STAGE_SMOOTHING),
                None => *stages,
            });
        }
    }

    /// The world clock's developer controls. The app calls
    /// [`TimeControls::before_fixed_step`] before every fixed step, shown or not.
    pub fn time_controls(&mut self) -> &mut TimeControls {
        &mut self.time
    }

    /// Time of day's developer controls. The app calls [`TodControls::apply`] before every
    /// fixed step, shown or not.
    pub fn tod_controls(&mut self) -> &mut TodControls {
        &mut self.day
    }

    /// The screen effects' developer controls. The app calls [`EffectControls::apply`] before
    /// every fixed step, shown or not.
    pub fn effect_controls(&mut self) -> &mut EffectControls {
        &mut self.effects
    }

    /// Turns the executor's graph report and GPU timing on while the overlay shows them, and GPU
    /// timing also while `app_timing` says the app reads it (dynamic resolution,
    /// `Quality::wants_gpu_timing`), so the two never switch it off under each other. Call it
    /// once per frame before executing the frame graph, instead of setting GPU timing yourself.
    pub fn configure_graph(&mut self, executor: &mut GraphExecutor, gpu: &GpuContext, app_timing: bool) {
        executor.set_report(self.visible && self.memory.panels.graph);
        self.gpu_timing = executor.set_gpu_timing(gpu, app_timing || (self.visible && GPU_TIMING.get()));
    }

    /// Builds this frame's UI, if shown. Its draw is added by [`Self::record`].
    pub fn update(&mut self, frame: &OverlayFrame<'_>) -> OverlayOutput {
        if !self.visible {
            return OverlayOutput::default();
        }
        profiling::scope!("DebugOverlay::update");
        let stopwatch = Stopwatch::start();
        let scope = alloc::AllocScope::begin();

        let zoom = UI_SCALE.get();
        let metrics = frame.window;
        let pixels_per_point = metrics.display_scale.max(0.25) * zoom;
        self.input.set_metrics(&metrics, pixels_per_point);
        self.router.set_window_size(metrics.size);
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(metrics.pixel_size.width as f32, metrics.pixel_size.height as f32) / pixels_per_point,
        );
        let safe = metrics.safe_area;
        let safe = egui::Rect::from_min_max(
            self.input.to_points(safe.x as f32, safe.y as f32),
            self.input.to_points((safe.x + safe.width as i32) as f32, (safe.y + safe.height as i32) as f32),
        )
        .intersect(screen);
        let mut raw = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(Duration::from_nanos(frame.now.as_nanos()).as_secs_f64()),
            focused: metrics.focused,
            max_texture_side: Some(self.max_texture_side),
            events: self.input.take_events(),
            ..egui::RawInput::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point =
            Some(metrics.display_scale);
        self.ctx.set_zoom_factor(zoom);

        let view = View {
            frame,
            history: &self.frame_times,
            stages: self.stages.as_ref(),
            cost: self.cost,
            safe,
            gpu_timing: self.gpu_timing,
        };
        let ctx = self.ctx.clone();
        let (memory, time, day, effects) =
            (&mut self.memory, &mut self.time, &mut self.day, &mut self.effects);
        let mut hide = false;
        let output = ctx.run_pass(raw, |ctx| hide = ui::show(ctx, memory, time, day, effects, &view));

        self.wants_keyboard = ctx.text_edit_focused();
        // Text input follows focus alone: egui reports the field's area only on frames it lays
        // the field out with a cursor, and stopping text input in between would hide the
        // on-screen keyboard.
        let text_area = output.platform_output.ime.as_ref().filter(|_| self.wants_keyboard).map(|ime| {
            let to_window = |point: egui::Pos2| self.input.to_window(point);
            let (min, max) = (to_window(ime.rect.min), to_window(ime.rect.max));
            let cursor = to_window(ime.cursor_rect.min);
            TextInputArea {
                area: sapphire_platform::Rect {
                    x: min.0 as i32,
                    y: min.1 as i32,
                    width: (max.0 - min.0).max(0.0) as u32,
                    height: (max.1 - min.1).max(0.0) as u32,
                },
                cursor_x: (cursor.0 - min.0) as i32,
            }
        });
        self.renderer.prepare(&ctx, output);
        self.cost = OverlayCost {
            cpu: stopwatch.elapsed(),
            allocations: scope.counts().allocations,
            vertices: self.renderer.vertices(),
        };
        if hide {
            self.set_visible(false);
            return OverlayOutput::default();
        }
        OverlayOutput { text_input: self.wants_keyboard, text_area }
    }

    /// Adds the overlay's pass to `graph`, drawing the last [`Self::update`] over `target`. Add
    /// it after the renderer's passes; it draws nothing while hidden.
    pub fn record<'f>(&'f mut self, graph: &mut FrameGraph<'f>, target: TextureId) {
        if self.visible {
            self.renderer.record(graph, target);
        }
    }
}
