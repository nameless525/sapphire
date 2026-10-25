//! Offscreen modes: `--capture` and `--bench`.
//!
//! Both render the sandbox scene into an offscreen target instead of a window, then exit.
//! `main` runs them through `sapphire_platform::run_with` with `RunOptions { background:
//! true }`, so the process never takes focus either. That makes the real app checkable on a
//! desktop someone is using: [`capture`](crate::capture) writes a PNG (the hook for
//! golden-image tests, spec §9) and [`bench`](crate::bench) measures the per-frame CPU path.

use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform};
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_render::{Msaa, RenderSettings, Renderer};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use crate::LOG_TARGET;
use crate::app::SandboxApp;
use crate::options::{OffscreenMode, Options};
use crate::scene::{self, Scene};

/// Offscreen targets are sRGB like the swapchain, so they match what the window shows.
pub(crate) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The offscreen "app": all work happens in `init`; the first frame exits.
pub struct OffscreenApp;

impl App for OffscreenApp {
    const METADATA: AppMetadata = SandboxApp::METADATA;

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let (options, unknown) = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        if !unknown.is_empty() {
            tracing::warn!(target: LOG_TARGET, ?unknown, "ignoring unknown arguments");
        }
        let offscreen = options
            .offscreen
            .ok_or_else(|| AppInitError::msg("OffscreenApp needs --capture <png> or --bench <frames>"))?;
        let (width, height) = (offscreen.width, offscreen.height);
        match offscreen.mode {
            OffscreenMode::Capture { path, ticks } => {
                let mut stage = Stage::new(platform, options.msaa)?;
                crate::capture::run(&mut stage, &path, ticks, width, height)?;
            }
            OffscreenMode::Bench { frames } => {
                crate::bench::run(platform, options.msaa, frames, width, height)?
            }
        }
        Ok(Self)
    }

    fn event(&mut self, _platform: &mut Platform, _event: &Event) -> AppControl {
        AppControl::Continue
    }

    fn iterate(&mut self, _platform: &mut Platform) -> AppControl {
        AppControl::Exit
    }
}

/// The GPU, renderer and scene an offscreen mode draws with.
pub(crate) struct Stage {
    // Drop order: everything on the device before the context, the context before the instance.
    pub renderer: Renderer,
    pub graph: GraphExecutor,
    pub scene: Scene,
    pub gpu: GpuContext,
    _instance: GpuInstance,
}

impl Stage {
    pub fn new(platform: &Platform, msaa: Msaa) -> Result<Self, AppInitError> {
        let instance = GpuInstance::new(&GpuInstanceDesc::default())?;
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None)?;
        let mut renderer = Renderer::new(&gpu, FORMAT, RenderSettings { msaa })?;
        let mesh = scene::load_mesh(platform, scene::CHARACTER_MESH)?;
        let scene = Scene::with_character(renderer.upload_mesh(&mesh)?, mesh.bounds);
        Ok(Self { renderer, graph: GraphExecutor::new(), scene, gpu, _instance: instance })
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
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("offscreen target", view);
        self.renderer.prepare(&self.gpu, self.scene.frame_packet(alpha, Some(graph.size(target))));
        self.renderer.record(&mut graph, target);
        after(&mut graph, target);
        self.graph.execute(&self.gpu, graph).map_err(AppInitError::msg)
    }
}
