//! Development tools, in `dev` builds only (rule R8): the debug overlay and the cvar registry it
//! edits (spec §5.20), hot reload (spec §5.10, M1 A8; `reload.rs`) and live link (spec §5.29, M1
//! A9; `link.rs`).
//!
//! Everything the windowed app and the offscreen modes do for them is here, so they hold one
//! `dev` field and make a few calls, all compiled away without the feature. Hidden, the overlay
//! costs a frame-time store per frame and a comparison per fixed step; idle, hot reload and live
//! link one empty channel read per frame each.

use std::path::Path;
use std::time::Duration;

use sapphire_assets::AssetStats;
use sapphire_core::alloc::{self, AllocCounts};
use sapphire_core::cvar::CvarRegistry;
use sapphire_debug::{
    CpuStages, DebugOverlay, OverlayConfig, OverlayFrame, OverlayOutput, Panels, WindowMetrics,
};
use sapphire_platform::{
    AppInitError, Event, FramePacing, Platform, TextInputKind, Timestamp, Window, WindowEvent,
};
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_render::quality::Quality;
use sapphire_render::{RenderStats, Renderer};
use sapphire_rhi::{GpuContext, wgpu};

use crate::content::Content;
use crate::input::SandboxInput;
use crate::link::{CaptureRequest, Link, LinkFrame};
use crate::options::Options;
use crate::reload::Reload;
use crate::scene::Scene;
use crate::stats::Stages;

pub(crate) struct DevTools {
    overlay: DebugOverlay,
    cvars: CvarRegistry,
    reload: Reload,
    link: Link,
    /// Frames begun, and the last one's length, for live link's status.
    frames: u64,
    frame_time: Duration,
    /// The frame's pacing, for the stats panel.
    pacing: Option<FramePacing>,
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
    /// Whether the app reads the frame graph's GPU timings this frame (dynamic resolution, the
    /// bench's GPU-timing phase), so they stay on whatever the overlay shows.
    pub gpu_timing: bool,
    pub gpu: &'a GpuContext,
    /// The windowed app's input; offscreen modes have none.
    pub input: Option<&'a SandboxInput>,
    pub scene: &'a Scene,
    pub assets: AssetStats,
}

impl DevTools {
    /// The overlay, drawing into `format` targets, shown from the start if `--debug-overlay`
    /// asked for it (with its panel list); hot reload of the packs in the platform's content
    /// root and the engine's sources, unless `--no-hot-reload` turned it off; and live link,
    /// serving pushed packs through `content`'s source, as `--live-link` says.
    pub fn new(
        gpu: &GpuContext,
        format: wgpu::TextureFormat,
        options: &Options,
        platform: &Platform,
        content: &Content,
    ) -> Result<Self, AppInitError> {
        let show = options.debug_overlay.as_deref();
        let panels = match show {
            None | Some("") => Panels::default(),
            Some(list) => Panels::parse(list).map_err(AppInitError::msg)?,
        };
        let config = OverlayConfig { visible: show.is_some(), panels, ..OverlayConfig::default() };
        let mut cvars = CvarRegistry::new();
        cvars
            .register_all(&crate::cvars::CVARS)
            .and_then(|()| cvars.register_all(&sapphire_render::quality::CVARS))
            .and_then(|()| cvars.register_all(&sapphire_render::debug_view::CVARS))
            .and_then(|()| cvars.register_all(&sapphire_debug::CVARS))
            .map_err(AppInitError::msg)?;
        let content_root = Path::new(platform.content().root());
        Ok(Self {
            overlay: DebugOverlay::new(gpu, format, config),
            cvars,
            reload: Reload::start(!options.no_hot_reload, content_root, &crate::reload::day_source(options)),
            link: Link::start(options.live_link, content.live_packs(), platform.system_info()),
            frames: 0,
            frame_time: Duration::ZERO,
            pacing: None,
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

    /// Applies what hot reload found changed and what live link received, before the frame's
    /// loading: once a frame.
    pub fn reload(&mut self, content: &mut Content, renderer: &mut Renderer, scene: &mut Scene) {
        // Taken once for both: hot reload lists failed reloads, live link answers pushes with them.
        let changes = content.take_changes();
        self.reload.update(&changes, content, renderer, scene);
        let frame = LinkFrame {
            content,
            renderer,
            scene,
            cvars: &self.cvars,
            reload: &mut self.reload,
            frame: self.frames,
            frame_time: self.frame_time,
        };
        self.link.update(&changes, frame);
    }

    /// The captures live link asked for, if any: the frame about to be drawn answers them
    /// ([`crate::link::FrameCapture`]).
    pub fn take_capture(&mut self) -> Option<CaptureRequest> {
        self.link.take_capture()
    }

    /// Frames begun since the app started, as live link's status counts them.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// A new renderer (a tier change, device loss) builds the shaders hot reload gave the last.
    pub fn renderer_created(&mut self, renderer: &mut Renderer) {
        self.reload.renderer_created(renderer);
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

    /// Starts a frame of real length `frame_time` (and the CPU stages measured for it), paced as
    /// `pacing` says where a display paces frames.
    pub fn begin_frame(
        &mut self,
        frame_time: Duration,
        stages: Option<&Stages>,
        pacing: Option<FramePacing>,
    ) {
        self.frames += 1;
        self.frame_time = frame_time;
        self.pacing = pacing;
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

    /// The time controls' hook, before each fixed step: the world clock's, time of day's and the
    /// screen effects'. Allocation-free (rule R6).
    pub fn before_fixed_step(&mut self, scene: &mut Scene) {
        self.overlay.time_controls().before_fixed_step(&mut scene.world_clock_mut().0);
        self.overlay.tod_controls().apply(&mut scene.time_of_day_mut());
        let effects = self.overlay.effect_controls();
        scene.with_screen_effects(|stack, library| effects.apply(stack, library));
    }

    /// Builds the overlay's frame, after `Renderer::prepare` and before the frame graph is
    /// recorded, and sets the executor up for what it shows and for dynamic resolution, which
    /// needs GPU timings whether the overlay shows or not.
    pub fn update(&mut self, frame: FrameState<'_>, executor: &mut GraphExecutor) -> OverlayOutput {
        self.overlay.configure_graph(executor, frame.gpu, frame.gpu_timing);
        self.overlay.update(&OverlayFrame {
            now: self.now,
            window: frame.window,
            allocations: self.last_frame,
            render: frame.render,
            quality: Some(frame.quality.stats()),
            pacing: self.pacing,
            graph: executor,
            gpu: frame.gpu,
            input_device: frame.input.and_then(SandboxInput::last_device),
            gamepads: frame.input.map_or(0, SandboxInput::gamepads),
            cvars: &self.cvars,
            world_clock: frame.scene.world_clock(),
            time_of_day: Some(frame.scene.time_of_day()),
            characters: frame.scene.characters(),
            screen_effects: Some(frame.scene.screen_effects()),
            assets: Some(frame.assets),
            hot_reload: Some(self.reload.status()),
            live_link: Some(self.link.status()),
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
