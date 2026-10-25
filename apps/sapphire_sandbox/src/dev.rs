//! Development tools, in `dev` builds only (rule R8): the debug overlay and the cvar registry it
//! edits (spec §5.20).
//!
//! Everything the windowed app and the offscreen modes do for the overlay is here, so they hold
//! one `dev` field and make a few calls, all compiled away without the feature. Hidden, the
//! overlay costs a frame-time store per frame and a comparison per fixed step.

use std::time::Duration;

use sapphire_core::alloc::{self, AllocCounts};
use sapphire_core::cvar::CvarRegistry;
use sapphire_debug::{
    CpuStages, DebugOverlay, OverlayConfig, OverlayFrame, OverlayOutput, Panels, WindowMetrics,
};
use sapphire_platform::{AppInitError, Event, TextInputKind, Timestamp, Window, WindowEvent};
use sapphire_render::RenderStats;
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_render::quality::Quality;
use sapphire_rhi::{GpuContext, wgpu};

use crate::input::SandboxInput;
use crate::scene::Scene;
use crate::stats::Stages;

pub(crate) struct DevTools {
    overlay: DebugOverlay,
    cvars: CvarRegistry,
    /// Process-wide allocation totals when the current frame began.
    allocations_at: AllocCounts,
    /// The previous frame's allocations.
    last_frame: AllocCounts,
    /// The frame's time on the platform clock, from [`Self::poll`].
    now: Timestamp,
    focused: bool,
}

/// What the overlay reads from the app in one frame.
pub(crate) struct FrameState<'a> {
    pub window: WindowMetrics,
    pub render: RenderStats,
    /// The quality tier and render scale, after this frame's [`Quality::render_scale`].
    pub quality: &'a Quality,
    pub gpu: &'a GpuContext,
    /// The windowed app's input; offscreen modes have none.
    pub input: Option<&'a SandboxInput>,
    pub scene: &'a Scene,
}

impl DevTools {
    /// The overlay, drawing into `format` targets, shown from the start if `--debug-overlay`
    /// asked for it (with its panel list).
    pub fn new(
        gpu: &GpuContext,
        format: wgpu::TextureFormat,
        show: Option<&str>,
    ) -> Result<Self, AppInitError> {
        let panels = match show {
            None | Some("") => Panels::default(),
            Some(list) => Panels::parse(list).map_err(AppInitError::msg)?,
        };
        let config = OverlayConfig { visible: show.is_some(), panels, ..OverlayConfig::default() };
        let mut cvars = CvarRegistry::new();
        cvars
            .register_all(&crate::cvars::CVARS)
            .and_then(|()| cvars.register_all(&sapphire_render::quality::CVARS))
            .and_then(|()| cvars.register_all(&sapphire_debug::CVARS))
            .map_err(AppInitError::msg)?;
        Ok(Self {
            overlay: DebugOverlay::new(gpu, format, config),
            cvars,
            allocations_at: process_allocations(),
            last_frame: AllocCounts::default(),
            now: Timestamp::default(),
            focused: true,
        })
    }

    /// After GPU device loss: the overlay's GPU half on the new device.
    pub fn recreate_gpu(&mut self, gpu: &GpuContext, format: wgpu::TextureFormat) {
        self.overlay.recreate_gpu(gpu, format);
    }

    /// The overlay sees each event first; the game's input gets what it doesn't use.
    pub fn handle_event(&mut self, event: &Event, input: &mut SandboxInput) {
        match event {
            Event::Window { event: WindowEvent::FocusGained, .. } => self.focused = true,
            Event::Window { event: WindowEvent::FocusLost, .. } => self.focused = false,
            _ => {}
        }
        self.overlay.handle_event(event, &mut |event| input.handle_event(event));
    }

    /// Starts the frame at `now` on the platform clock: the overlay's time-driven toggle
    /// gesture, before the input update.
    pub fn poll(&mut self, now: Timestamp, input: &mut SandboxInput) {
        self.now = now;
        self.overlay.poll_gestures(now, &mut |event| input.handle_event(event));
    }

    /// Advances the overlay's clock without a platform: offscreen frames are `frame` long.
    pub fn advance_offscreen(&mut self, frame: Duration) {
        self.now = Timestamp::from_nanos(self.now.as_nanos() + frame.as_nanos() as u64);
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    pub fn overlay_visible(&self) -> bool {
        self.overlay.is_visible()
    }

    /// Starts a frame of real length `frame_time` (and the CPU stages measured for it).
    pub fn begin_frame(&mut self, frame_time: Duration, stages: Option<&Stages>) {
        let now = process_allocations();
        self.last_frame = AllocCounts {
            allocations: now.allocations - self.allocations_at.allocations,
            bytes: now.bytes - self.allocations_at.bytes,
        };
        self.allocations_at = now;
        let stages = stages.map(|stages| CpuStages {
            input: stages.input,
            simulation: stages.simulation,
            fixed_steps: stages.fixed_steps,
            extract: stages.extract,
            render: stages.render,
        });
        self.overlay.record_frame(frame_time, stages.as_ref());
    }

    /// The time controls' hook, before each fixed step. Allocation-free (rule R6).
    pub fn before_fixed_step(&mut self, scene: &mut Scene) {
        self.overlay.time_controls().before_fixed_step(&mut scene.world_clock_mut().0);
    }

    /// Builds the overlay's frame, after `Renderer::prepare` and before the frame graph is
    /// recorded, and sets the executor up for what it shows and for dynamic resolution, which
    /// needs GPU timings whether the overlay shows or not.
    pub fn update(&mut self, frame: FrameState<'_>, executor: &mut GraphExecutor) -> OverlayOutput {
        self.overlay.configure_graph(executor, frame.gpu, frame.quality.wants_gpu_timing());
        self.overlay.update(&OverlayFrame {
            now: self.now,
            window: frame.window,
            allocations: self.last_frame,
            render: frame.render,
            quality: Some(frame.quality.stats()),
            graph: executor,
            gpu: frame.gpu,
            input_device: frame.input.and_then(SandboxInput::last_device),
            gamepads: frame.input.map_or(0, SandboxInput::gamepads),
            cvars: &self.cvars,
            world_clock: frame.scene.world_clock(),
        })
    }

    /// Adds the overlay's pass, after the renderer's.
    pub fn record<'f>(&'f mut self, graph: &mut FrameGraph<'f>, target: TextureId) {
        self.overlay.record(graph, target);
    }

    /// Takes text input on `window` while an overlay text field has focus.
    pub fn sync_text_input(window: &Window, output: &OverlayOutput) {
        let active = window.is_text_input_active();
        if output.text_input && !active {
            // The console takes commands and cvar names: no automatic capitals or corrections.
            window.start_text_input(TextInputKind::Verbatim);
        } else if !output.text_input && active {
            window.stop_text_input();
        }
        if let Some(area) = output.text_area {
            window.set_text_input_area(area.area, area.cursor_x);
        }
    }
}

fn process_allocations() -> AllocCounts {
    let stats = alloc::stats();
    AllocCounts { allocations: stats.total_allocations, bytes: stats.total_bytes }
}
