//! Offscreen modes: `--capture`, `--bench` and, in `dev` builds, `--offscreen`.
//!
//! They render the sandbox scene into an offscreen target instead of a window. `main` runs them
//! through `sapphire_platform::run_with` with `RunOptions { background: true }`, so the process
//! never takes focus either. That makes the real app checkable on a desktop someone is using:
//! [`capture`](crate::capture) writes a PNG (the hook for golden-image tests, spec §9) and
//! [`bench`](crate::bench) measures the per-frame CPU path, then each exits; `--offscreen`
//! (`serve.rs`) keeps drawing for hot reload and live link until stopped.

use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, DeviceClass, Event, Platform};
use sapphire_render::debug_view::DebugView;
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_render::quality::{DeviceProfile, FrameTiming, Quality, TierTable};
use sapphire_render::{RenderSettings, Renderer};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, TransientDesc, wgpu};

use crate::LOG_TARGET;
use crate::app::SandboxApp;
use crate::content::Content;
use crate::options::{OffscreenMode, Options};
use crate::scene::{LookDev, Scene};
#[cfg(feature = "dev")]
use crate::stats::Stages;

/// Offscreen targets are sRGB like the swapchain, so they match what the window shows.
pub(crate) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The offscreen "app". A capture or bench does all its work in `init`, and the first frame
/// exits; `--offscreen` draws a frame each iteration until the process is told to quit.
pub struct OffscreenApp {
    #[cfg(feature = "dev")]
    serving: Option<crate::serve::Serving>,
}

impl App for OffscreenApp {
    const METADATA: AppMetadata = SandboxApp::METADATA;

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let (options, unknown) = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        if !unknown.is_empty() {
            tracing::warn!(target: LOG_TARGET, ?unknown, "ignoring unknown arguments");
        }
        let offscreen = options.offscreen.as_ref().ok_or_else(|| {
            AppInitError::msg("OffscreenApp needs --capture <png>, --bench <frames> or --offscreen")
        })?;
        let (width, height) = (offscreen.width, offscreen.height);
        match &offscreen.mode {
            OffscreenMode::Capture { path, ticks, frames } => {
                let mut stage = Stage::new(platform, &options)?;
                crate::capture::run(&mut stage, path, *ticks, *frames, width, height)?;
            }
            OffscreenMode::Bench { frames } => crate::bench::run(platform, &options, *frames, width, height)?,
            #[cfg(feature = "dev")]
            OffscreenMode::Serve => {
                let stage = Stage::new(platform, &options)?;
                let serving = crate::serve::Serving::new(stage, width, height, options.frame_limit);
                return Ok(Self { serving: Some(serving) });
            }
            #[cfg(not(feature = "dev"))]
            OffscreenMode::Serve => {
                return Err(AppInitError::msg(
                    "--offscreen serves hot reload and live link, which only `dev` builds have (`--features dev`)",
                ));
            }
        }
        Ok(Self {
            #[cfg(feature = "dev")]
            serving: None,
        })
    }

    fn event(&mut self, _platform: &mut Platform, event: &Event) -> AppControl {
        match event {
            // Ctrl-C or `kill`, which SDL reports as a quit.
            Event::Quit { .. } => AppControl::Exit,
            _ => AppControl::Continue,
        }
    }

    fn iterate(&mut self, _platform: &mut Platform) -> AppControl {
        #[cfg(feature = "dev")]
        if let Some(serving) = &mut self.serving {
            return serving.frame();
        }
        AppControl::Exit
    }
}

/// The GPU, renderer and scene an offscreen mode draws with.
pub(crate) struct Stage {
    // Drop order: everything on the device before the context, the context before the instance.
    /// The debug overlay, drawn over the frame with `--debug-overlay`, hidden otherwise.
    #[cfg(feature = "dev")]
    pub dev: crate::dev::DevTools,
    /// The last frame's stage times, measured while the overlay shows.
    #[cfg(feature = "dev")]
    stages: Option<Stages>,
    pub renderer: Renderer,
    /// The asset server and what it uploaded, driven every frame like the windowed app's.
    pub content: Content,
    pub graph: GraphExecutor,
    pub scene: Scene,
    /// The quality tier the renderer was made for, and each frame's render scale.
    pub quality: Quality,
    /// Keeps the frame graph's GPU timing on whatever the quality tier wants: the bench's
    /// GPU-timing phase.
    pub measure_gpu: bool,
    /// Only TAA captures without instrumentation can discard their warmup's display output.
    history_only_capture: bool,
    pub gpu: GpuContext,
    _instance: GpuInstance,
}

impl Stage {
    /// The scene and renderer `options` ask for. Offscreen modes run on desktops only.
    pub fn new(platform: &Platform, options: &Options) -> Result<Self, AppInitError> {
        let instance = GpuInstance::new(&GpuInstanceDesc::default())?;
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None)?;
        let profile = DeviceProfile::new(DeviceClass::Desktop, gpu.caps(), platform.system_info().ram_mib);
        let quality = Quality::new(TierTable::shipped(), &profile, options.quality_settings());
        let settings = options.render_settings(quality.render_settings());
        let history_only_capture = history_only_capture_allowed(options, settings);
        let mut renderer = Renderer::new(&gpu, FORMAT, settings)?;
        let mut content = Content::new(platform, options.character(), options.sky())?;
        if options.keeps_character_mesh() {
            content.keep_character_mesh();
        }
        content.override_materials(options.material_overrides());
        let (character, bounds) = content.character(&mut renderer, &gpu)?;
        let mut scene = Scene::with_character(character, bounds, options.skinned);
        scene.set_sky_material(content.sky_material(&mut renderer, &gpu)?);
        if let Some(rig) = &options.character_rig {
            scene.set_character_rig(rig).map_err(AppInitError::msg)?;
        }
        if options.lights {
            scene.spawn_demo_lights();
        }
        if options.skinned {
            scene.set_stalk_mesh(crate::skinned::upload_stalk(&mut renderer)?);
        }
        if options.swatches {
            scene.spawn_swatches(crate::swatches::upload_swatches(&mut content, &gpu, &mut renderer)?);
        }
        if options.materials {
            scene.spawn_layer_samples(crate::layers::upload_samples(&mut content, &gpu, &mut renderer)?);
        }
        if options.two_sided {
            scene.spawn_garments(crate::garments::upload_samples(&mut renderer)?);
        }
        if let Some(layout) = options.faces {
            scene.spawn_face_rig(content.face_head(&mut renderer, &gpu)?, layout);
        }
        if let Some(layout) = options.rim_rig {
            let figure =
                content.character_mesh().ok_or_else(|| AppInitError::msg("the character isn't loaded"))?;
            scene.spawn_rim_rig(layout, crate::rims::upload_rig(&mut renderer, figure)?);
        }
        if options.environment {
            scene.spawn_environment(content.courtyard(&mut renderer, &gpu)?);
            if !options.fog_off {
                scene.spawn_fog_cards(content.fog_cards(&mut renderer, &gpu)?);
            }
        }
        if let Some(view) = options.foliage {
            scene.spawn_foliage(view, content.foliage(&mut renderer, &gpu)?);
        }
        if options.probes {
            let arcade = content.probes(&mut renderer, &gpu)?;
            scene.spawn_arcade(arcade, content.probe_hours());
        }
        if options.outline_row {
            scene.spawn_outline_row(character);
        }
        if options.hard_edges {
            scene.spawn_hard_edges(content.hard_edges(&mut renderer, &gpu)?);
        }
        if let Some(shot) = options.look_dev {
            let town = content.town(&mut renderer, &gpu)?;
            let fog_cards = (!options.fog_off).then(|| content.fog_cards(&mut renderer, &gpu)).transpose()?;
            let (pose, figure) = crate::lookdev::hold(&mut content, &mut renderer, &gpu)?;
            scene.spawn_look_dev(shot, LookDev { town, fog_cards, pose, figure });
        }
        // The day, the sky options, the starting view and the turntable's speed, last so `--view`
        // holds whatever framing the scene chose.
        options.apply_to(&mut scene).map_err(AppInitError::msg)?;
        // The screen effects it may push, `--post-effect`'s among them, show on their first frame.
        renderer.prewarm_post(scene.effect_variants());
        // The stage after the hour, which places its pillars.
        if let Some(layout) = options.stage {
            let character =
                content.character_mesh().ok_or_else(|| AppInitError::msg("the character isn't loaded"))?;
            scene.spawn_stage(layout, crate::stage::upload_stage(&mut renderer, character)?);
        }
        #[cfg(feature = "dev")]
        let dev = crate::dev::DevTools::new(&gpu, FORMAT, options, platform, &content)?;
        #[cfg(not(feature = "dev"))]
        if options.debug_overlay.is_some() {
            tracing::warn!(target: LOG_TARGET, "--debug-overlay needs a `dev` build (`--features dev`)");
        }
        Ok(Self {
            #[cfg(feature = "dev")]
            dev,
            #[cfg(feature = "dev")]
            stages: None,
            renderer,
            content,
            graph: {
                let mut graph = GraphExecutor::new();
                graph.set_serial(options.serial_passes);
                graph
            },
            scene,
            quality,
            measure_gpu: false,
            history_only_capture,
            gpu,
            _instance: instance,
        })
    }

    /// One fixed step, under the overlay's time controls in `dev` builds.
    pub fn step(&mut self) {
        #[cfg(feature = "dev")]
        self.dev.before_fixed_step(&mut self.scene);
        self.scene.step();
    }

    /// Whether the overlay draws over the frames (`--debug-overlay` in a `dev` build).
    #[cfg(feature = "dev")]
    pub fn overlay_visible(&self) -> bool {
        self.dev.overlay_visible()
    }

    #[cfg(not(feature = "dev"))]
    pub fn overlay_visible(&self) -> bool {
        false
    }

    /// A 2D render target in [`FORMAT`] with `usage` on top of `RENDER_ATTACHMENT`.
    pub fn target(&self, width: u32, height: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
        self.gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: self.gpu.label("offscreen target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
            view_formats: &[],
        })
    }

    /// Renders the scene as it stands into `view` and submits it. `after` adds passes that
    /// run after the renderer's, given the target (a readback, say). `alpha` interpolates
    /// between the last two fixed steps, as in the windowed app.
    pub fn render<'f>(
        &'f mut self,
        view: &'f wgpu::TextureView,
        alpha: f32,
        after: impl FnOnce(&mut FrameGraph<'f>, TextureId),
    ) -> Result<(), AppInitError> {
        self.render_frame(view, false, alpha, after).map(|_| ())
    }

    /// Draws every persistent history update but can discard an unused display result. The
    /// graph keeps imported TAA history/depth, shadows and other persistent writes alive;
    /// only unconsumed transient passes (bloom and the final composite) may be culled.
    /// Instrumented frames use the ordinary path so their timings/reports stay representative.
    pub fn render_capture_warmup(
        &mut self,
        view: &wgpu::TextureView,
        requested: bool,
    ) -> Result<bool, AppInitError> {
        self.render_frame(view, requested, 1.0, |_, _| {})
    }

    fn render_frame<'f>(
        &'f mut self,
        view: &'f wgpu::TextureView,
        discard_output: bool,
        alpha: f32,
        after: impl FnOnce(&mut FrameGraph<'f>, TextureId),
    ) -> Result<bool, AppInitError> {
        // Hot reload and loading run every frame, as in the windowed app, so the bench measures
        // them.
        #[cfg(feature = "dev")]
        self.dev.reload(&mut self.content, &mut self.renderer, &mut self.scene);
        self.content.update(&mut self.renderer, &self.gpu);
        self.scene.set_probe_hours(self.content.probe_hours());
        // Reload/live link can change debug CVars, so inspect them after applying it.
        let discard_output = discard_output
            && self.history_only_capture
            && !self.overlay_visible()
            && !self.measure_gpu
            && !self.quality.wants_gpu_timing()
            && sapphire_render::debug_view::DEBUG_VIEW.get() == DebugView::Off;
        let mut graph = FrameGraph::new();
        let target = if discard_output {
            let texture = view.texture();
            graph.create_texture(
                "discarded capture output",
                TransientDesc {
                    width: texture.width(),
                    height: texture.height(),
                    format: FORMAT,
                    sample_count: 1,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                },
            )
        } else {
            graph.import_texture("offscreen target", view)
        };
        let (width, height) = graph.size(target);
        // While the overlay shows, it gets the extract and render stages' times (the bench
        // measures its own way, and fixed steps are run outside this).
        #[cfg(feature = "dev")]
        let mut stopwatch = self.dev.overlay_visible().then(sapphire_time::Stopwatch::start);
        let packet = self.scene.frame_packet(alpha, Some((width, height)));
        #[cfg(feature = "dev")]
        let extract = stopwatch.as_mut().map(sapphire_time::Stopwatch::lap).unwrap_or_default();
        // Before `prepare`, which culls hulls by it, and readies the debug view.
        self.renderer.set_outline_distance(self.quality.outline_distance());
        self.renderer.set_debug_view(sapphire_render::debug_view::DEBUG_VIEW.get());
        self.renderer.prepare(&self.gpu, packet);
        let timing = FrameTiming::new(&self.graph, OFFSCREEN_FRAME);
        self.renderer.set_render_scale(self.quality.render_scale((width, height), &timing));
        let gpu_timing = self.quality.wants_gpu_timing() || self.measure_gpu;
        #[cfg(not(feature = "dev"))]
        self.graph.set_gpu_timing(&self.gpu, gpu_timing);
        #[cfg(feature = "dev")]
        let overlay = {
            let started = sapphire_time::Stopwatch::start();
            // Offscreen frames have no real length; the overlay is told each is a 60 Hz frame.
            // No display paces offscreen frames.
            self.dev.begin_frame(OFFSCREEN_FRAME, self.stages.as_ref(), None);
            self.dev.advance_offscreen(OFFSCREEN_FRAME);
            // Drawn as on a display 720 points high, so text stays legible at any size.
            let scale = (height as f32 / 720.0).max(1.0);
            let state = crate::dev::FrameState {
                window: sapphire_debug::WindowMetrics::offscreen(width, height, scale),
                render: self.renderer.stats(),
                quality: &self.quality,
                gpu_timing,
                gpu: &self.gpu,
                input: None,
                scene: &self.scene,
                assets: self.content.stats(),
            };
            self.dev.update(state, &mut self.graph);
            started.elapsed()
        };
        self.renderer.record(&mut graph, target);
        #[cfg(feature = "dev")]
        self.dev.record(&mut graph, target);
        after(&mut graph, target);
        self.graph.execute(&self.gpu, graph).map_err(AppInitError::msg)?;
        #[cfg(feature = "dev")]
        if let Some(mut stopwatch) = stopwatch {
            let render = stopwatch.lap().saturating_sub(overlay);
            self.stages = Some(Stages { extract, render, ..Stages::default() });
        }
        Ok(discard_output)
    }
}

/// TAA's persistent history gives the frame graph a root independent of the display target.
/// Dynamic resolution and debug modes observe work/timings beyond the final image, so retain
/// their complete frames. Use effective settings: `--aa` can override the tier.
fn history_only_capture_allowed(options: &Options, settings: RenderSettings) -> bool {
    settings.temporal
        && !options.dynamic_resolution
        && !options.serial_passes
        && options.debug_overlay.is_none()
        && options.debug_view.is_none_or(|view| view == DebugView::Off)
}

/// The length the overlay and dynamic resolution are told an offscreen frame takes: offscreen
/// frames have no real length, so dynamic resolution follows GPU timings alone.
const OFFSCREEN_FRAME: std::time::Duration = std::time::Duration::from_micros(16_667);

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Options {
        let args =
            std::iter::once("sandbox").chain(args.iter().copied()).map(str::to_owned).collect::<Vec<_>>();
        let (options, unknown) = Options::parse(&args).unwrap();
        assert!(unknown.is_empty(), "unexpected arguments: {unknown:?}");
        options
    }

    #[test]
    fn history_only_requires_effective_taa_without_instrumentation() {
        let allowed = |args: &[&str], tier| {
            let options = options(args);
            history_only_capture_allowed(&options, options.render_settings(tier))
        };
        let taa = RenderSettings { temporal: true, ..RenderSettings::default() };
        let no_taa = RenderSettings { temporal: false, ..taa };
        assert!(allowed(&[], taa));
        assert!(!allowed(&[], no_taa));
        assert!(allowed(&["--aa", "taa"], no_taa), "explicit AA overrides the tier");
        assert!(!allowed(&["--aa", "msaa"], taa));
        assert!(!allowed(&["--aa", "off"], taa));
        for args in [
            &["--dynamic-resolution"][..],
            &["--serial-passes"],
            &["--debug-overlay"],
            &["--debug-view", "wireframe"],
        ] {
            assert!(!allowed(args, taa), "{args:?} must observe full frames");
        }
        assert!(allowed(&["--debug-view", "off"], taa));
    }
}
