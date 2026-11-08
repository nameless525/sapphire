//! [`Stage`]: a GPU, a renderer, content and a scene, stepped and drawn as one.
//!
//! This is what any app does around [`Scene`] and [`Content`] once it's chosen what to show: make them
//! on one device, run a fixed step, load what's ready, animate, extract, prepare and record. What a frame
//! draws into is the caller's: a texture it owns, or a window's image, since [`Stage::render`] takes a
//! view and nothing else. The frame's order is the one spec §4.5 reads clocks by.
//!
//! [`FrameHooks`] are what a development build adds without this crate knowing it: the sandbox's overlay,
//! hot reload and live link all come through them. `()` adds nothing, and costs nothing.

use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_core::alloc::{AllocCounts, AllocScope};
use sapphire_platform::{AppInitError, Platform};
use sapphire_render::debug_view::DebugView;
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_render::quality::{DeviceProfile, FrameTiming, Quality, QualitySettings, TierTable};
use sapphire_render::{MeshHandle, RenderSettings, Renderer};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, TransientDesc, wgpu};
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::content::{self, Content, UploadWatcher};
use crate::faces::MaterialOverrides;
use crate::scene::Scene;
use crate::show::SceneShow;

/// CPU stage measurements, for a caller that reads and resets them each frame. These are
/// scoped observations, not an end-to-end frame or presentation latency: caller input/events,
/// surface acquisition/presentation and explicit GPU waits lie outside them. Hooks are included
/// in their owning stage, except `FrameHooks::tools` when `FrameHooks::TOOLS` is true.
/// Physics is within simulation; camera/HUD and other presentation work are within packet.
/// Duration fields accumulate until reset; allocation fields describe the latest measured call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameLaps {
    /// The before-fixed-step hook and the scene's fixed-step systems.
    pub scene_step: Duration,
    /// The simulation's steps.
    pub simulation: Duration,
    /// Of those, the character controllers with the physics they ask for: `SimStats::controllers_last`.
    pub physics: Duration,
    /// The arena following the steps, and their cues fired: effects spawned and moved.
    pub cues: Duration,
    /// The before-loading hook, `Content::update` and `Scene::follow_content`.
    pub content: Duration,
    /// `Scene::animate`.
    pub(crate) animation: Duration,
    /// `Scene::frame_packet`: presentation, the camera, extraction and the HUD.
    pub packet: Duration,
    /// `Renderer::prepare`.
    pub prepare: Duration,
    /// Work after prepare through graph submission, including pass callbacks, excluding the
    /// tools hook when `FrameHooks::TOOLS` is true.
    pub record: Duration,
    /// What each drew on the heap, on this thread: the workers that record passes count apart.
    pub animation_allocations: AllocCounts,
    pub packet_allocations: AllocCounts,
    pub prepare_allocations: AllocCounts,
    pub record_allocations: AllocCounts,
}

/// What's been allocated on this thread since `scope` began, and a scope from now on.
fn next_scope(scope: &mut AllocScope) -> AllocCounts {
    let counts = scope.counts();
    *scope = AllocScope::begin();
    counts
}

/// What a caller adds to a stage's frames, whose show is an `S`. Every method defaults to nothing, in the
/// frame's order.
pub trait FrameHooks<S = ()> {
    /// Whether [`Self::tools`] does work of its own, whose time and allocations [`FrameLaps`] leaves out.
    const TOOLS: bool = false;

    /// Before each fixed step, on the scene it'll step.
    fn before_fixed_step(&mut self, _scene: &mut Scene) {}

    /// First in a frame, before loading runs: the place to swap in what changed on disk.
    fn before_loading(&mut self, _frame: LoadingFrame<'_, S>) {}

    /// Whether something draws over the frame, so a capture's warmup can't discard its output.
    fn overlay_visible(&self) -> bool {
        false
    }

    /// Once the frame's fixed steps are over and loading has run.
    fn after_steps(&mut self, _show: &S, _scene: &Scene) {}

    /// Before recording, with the frame prepared: what asks the graph for GPU timing or not has to,
    /// which is all this does by default. A frame graph's GPU timing comes from one owner.
    fn tools(&mut self, frame: ToolFrame<'_, S>, executor: &mut GraphExecutor) {
        executor.set_gpu_timing(frame.gpu, frame.gpu_timing);
    }

    /// After the renderer's passes, on the same target.
    fn record<'f>(&'f mut self, _graph: &mut FrameGraph<'f>, _target: TextureId) {}
}

impl<S> FrameHooks<S> for () {}

/// What [`FrameHooks::before_loading`] can change.
pub struct LoadingFrame<'a, S = ()> {
    pub content: &'a mut Content,
    pub renderer: &'a mut Renderer,
    pub gpu: &'a GpuContext,
    pub scene: &'a mut Scene,
    pub show: &'a mut S,
}

/// How long the last frame's stages took, measured while [`FrameHooks::overlay_visible`] says so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageTimes {
    /// From the end of the frame's steps to its packet.
    pub extract: Duration,
    /// From there to its submit, less what [`FrameHooks::tools`] took.
    pub render: Duration,
}

/// What [`FrameHooks::tools`] can read of a prepared frame.
pub struct ToolFrame<'a, S = ()> {
    pub width: u32,
    pub height: u32,
    /// The last frame's stages, if they were measured.
    pub last_times: Option<StageTimes>,
    pub renderer: &'a Renderer,
    pub quality: &'a Quality,
    pub gpu_timing: bool,
    pub gpu: &'a GpuContext,
    pub scene: &'a Scene,
    pub content: &'a Content,
    pub show: &'a S,
}

/// What [`Stage::new`] makes. [`Self::new`] takes the device's own tier and the mannequin under the
/// shipped sky; change what's different before making the stage.
pub struct StageDesc {
    /// What frames draw into: an offscreen target's format, or a window surface's.
    pub format: wgpu::TextureFormat,
    /// The quality tier the renderer's made for, and each frame's render scale.
    pub quality: Quality,
    /// What the renderer's made with: the tier's, or more than it says.
    pub settings: RenderSettings,
    /// The character, loaded before this returns.
    pub character: AssetId,
    /// The zone's sky material.
    pub sky: AssetId,
    /// Keep a copy of the character's mesh, for meshes derived from it ([`Content::keep_character_mesh`]).
    pub keep_character_mesh: bool,
    /// What hears of uploads as they happen, from before the character's ([`Content::watch_uploads`]).
    pub uploads: Option<Box<dyn UploadWatcher>>,
    /// What every matching material takes as it loads.
    pub material_overrides: MaterialOverrides,
    /// Give the scene its compute-skinning stalk's systems.
    pub skinned: bool,
    /// Build what the packs' meshes draw through before the character's uploaded (M3 K1), as a game does. A
    /// capture doesn't, for the same reason a renderer builds nothing a frame can't run: a software GPU's run would
    /// pay for the whole pack.
    pub prewarm_packs: bool,
    /// The entry of [`Content::pipeline_inventory`] left out of `prewarm_packs`, by its name: what proves a miss
    /// fails.
    pub inventory_without: Option<String>,
}

impl StageDesc {
    /// The shipped tier for this device, with `settings` as the player would have them.
    pub fn new(
        platform: &Platform,
        gpu: &GpuContext,
        format: wgpu::TextureFormat,
        settings: QualitySettings,
    ) -> Self {
        let profile = DeviceProfile::new(
            platform.system_info().device_class,
            gpu.caps(),
            platform.system_info().ram_mib,
        );
        let quality = Quality::new(TierTable::shipped(), &profile, settings);
        let settings = quality.render_settings();
        Self {
            format,
            quality,
            settings,
            character: content::CHARACTER,
            sky: content::SKY,
            keep_character_mesh: false,
            uploads: None,
            material_overrides: MaterialOverrides::default(),
            skinned: false,
            prewarm_packs: false,
            inventory_without: None,
        }
    }
}

/// A device with no window: for a stage that draws into textures, where no one's watching, so an error
/// fails the run ([`gpu_failure`]) in every build rather than logging and going on.
pub fn headless_gpu() -> Result<(GpuInstance, GpuContext), AppInitError> {
    let instance = GpuInstance::new(&GpuInstanceDesc::default())?;
    let desc = GpuContextDesc { hold_errors: true, ..GpuContextDesc::default() };
    let gpu = GpuContext::new(&instance, &desc, None)?;
    Ok((instance, gpu))
}

/// Why a run on `gpu`, made with `GpuContextDesc::hold_errors`, can't stand: the first error it held, or
/// a device lost, whose frames since did nothing. What it drew can't be trusted, whatever its pixels look
/// like, so a capture writes no picture past this and a bench no result.
pub fn gpu_failure(gpu: &GpuContext) -> Result<(), String> {
    if let Some(error) = gpu.first_error() {
        return Err(format!("the GPU reported an error: {error}"));
    }
    if gpu.is_lost() {
        return Err("the GPU device was lost".to_owned());
    }
    Ok(())
}

/// The GPU, renderer, content and scene a frame draws with, and what plays on them.
pub struct Stage<S: SceneShow = (), H: FrameHooks<S> = ()> {
    // Drop order: everything on the device before the context, the context before the instance.
    /// What the caller adds to its frames.
    pub hooks: H,
    pub renderer: Renderer,
    /// What plays on the scene, which is the caller's.
    pub show: S,
    /// The asset server and what it uploaded, driven every frame.
    pub content: Content,
    pub graph: GraphExecutor,
    pub scene: Scene,
    /// The quality tier the renderer was made for, and each frame's render scale.
    pub quality: Quality,
    /// The character's mesh on the renderer.
    pub character: MeshHandle,
    /// Keeps the frame graph's GPU timing on whatever the quality tier wants.
    pub measure_gpu: bool,
    /// Only TAA captures without instrumentation can discard their warmup's display output.
    pub history_only_capture: bool,
    /// Prepare and record the next frames but run none of it, as a frame whose surface was lost (M3 K1). A
    /// load's inventory is checked this way where drawing is what costs: everything but the graph's passes runs.
    pub lose_frames: bool,
    /// Where the frames' CPU time went since it was last reset.
    pub laps: FrameLaps,
    pub gpu: GpuContext,
    /// The last frame's stage times, measured while something draws over the frames.
    last_times: Option<StageTimes>,
    format: wgpu::TextureFormat,
    _instance: GpuInstance,
}

impl Stage {
    /// The renderer, the content and a scene with the character in it, on a device the caller made. The
    /// character and the sky have loaded by the time this returns. The scene holds nothing more: place
    /// what it should show on [`Self::scene`].
    pub fn new(
        platform: &Platform,
        instance: GpuInstance,
        gpu: GpuContext,
        desc: StageDesc,
    ) -> Result<Self, AppInitError> {
        let content = Content::new(platform, desc.character, desc.sky)?;
        Self::new_with_content(instance, gpu, desc, content)
    }

    /// Builds the renderer over already mounted Content, after any CPU-only combat bootstrap.
    pub fn new_with_content(
        instance: GpuInstance,
        gpu: GpuContext,
        desc: StageDesc,
        mut content: Content,
    ) -> Result<Self, AppInitError> {
        if content.selection() != (desc.character, desc.sky) {
            return Err(AppInitError::msg("preloaded content differs from the stage character or sky"));
        }
        let StageDesc {
            format,
            quality,
            settings,
            character: _,
            sky: _,
            keep_character_mesh,
            uploads,
            material_overrides,
            skinned,
            prewarm_packs,
            inventory_without,
        } = desc;
        let mut renderer = Renderer::new(&gpu, format, settings)?;
        if prewarm_packs {
            let mut inventory = content.pipeline_inventory();
            if let Some(entry) = &inventory_without
                && !inventory.remove(entry)
                && entry.starts_with("mesh ")
            {
                let listed = inventory.entries().join(", ");
                return Err(AppInitError::msg(format!("the packs list no {entry:?}, only: {listed}")));
            }
            let stopwatch = Stopwatch::start();
            let built = renderer.prewarm(&inventory);
            tracing::info!(
                target: LOG_TARGET,
                keys = inventory.meshes.len(),
                built,
                elapsed_ms = format_args!("{:.1}", stopwatch.elapsed().as_secs_f64() * 1e3),
                "the packs' pipelines prewarmed"
            );
        }
        content.leave_out_of_zones(inventory_without);
        if keep_character_mesh {
            content.keep_character_mesh();
        }
        if let Some(uploads) = uploads {
            content.watch_uploads(uploads);
        }
        content.override_materials(material_overrides);
        let (character, bounds) = content.character(&mut renderer, &gpu)?;
        let mut scene = Scene::with_character(character, bounds, skinned);
        scene.set_sky_material(content.sky_material(&mut renderer, &gpu)?);
        Ok(Self {
            hooks: (),
            renderer,
            show: (),
            content,
            graph: GraphExecutor::new(),
            scene,
            quality,
            character,
            measure_gpu: false,
            history_only_capture: false,
            lose_frames: false,
            laps: FrameLaps::default(),
            gpu,
            last_times: None,
            format,
            _instance: instance,
        })
    }

    /// The same stage, with `show` playing on it.
    pub fn with_show<S: SceneShow>(self, show: S) -> Stage<S> {
        let Self {
            hooks: (),
            renderer,
            show: (),
            content,
            graph,
            scene,
            quality,
            character,
            measure_gpu,
            history_only_capture,
            lose_frames,
            laps,
            gpu,
            last_times,
            format,
            _instance,
        } = self;
        Stage {
            hooks: (),
            renderer,
            show,
            content,
            graph,
            scene,
            quality,
            character,
            measure_gpu,
            history_only_capture,
            lose_frames,
            laps,
            gpu,
            last_times,
            format,
            _instance,
        }
    }
}

impl<S: SceneShow> Stage<S> {
    /// The same stage, with `hooks` on its frames.
    pub fn with_hooks<H: FrameHooks<S>>(self, hooks: H) -> Stage<S, H> {
        let Self {
            hooks: (),
            renderer,
            show,
            content,
            graph,
            scene,
            quality,
            character,
            measure_gpu,
            history_only_capture,
            lose_frames,
            laps,
            gpu,
            last_times,
            format,
            _instance,
        } = self;
        Stage {
            hooks,
            renderer,
            show,
            content,
            graph,
            scene,
            quality,
            character,
            measure_gpu,
            history_only_capture,
            lose_frames,
            laps,
            gpu,
            last_times,
            format,
            _instance,
        }
    }
}

impl<S: SceneShow, H: FrameHooks<S>> Stage<S, H> {
    /// One fixed step and the end of a frame's: each step here is a frame's worth, as it is on any tier
    /// that draws every tick.
    pub fn step(&mut self) {
        self.fixed_step();
        self.show.end_steps(&mut self.scene, 1.0 / sapphire_time::TICK_HZ as f32);
    }

    /// One fixed step and nothing of the frame's: for a frame that takes several, or asks what the steps
    /// asked of the audio system before the frame's steps end.
    pub fn fixed_step(&mut self) {
        let mut stopwatch = Stopwatch::start();
        self.hooks.before_fixed_step(&mut self.scene);
        self.scene.step();
        self.laps.scene_step += stopwatch.lap();
        self.show.step(&mut self.scene, &mut self.laps);
    }

    /// An error if the GPU has reported one since the stage was made: see [`gpu_failure`].
    pub fn check_gpu(&self) -> Result<(), AppInitError> {
        gpu_failure(&self.gpu).map_err(AppInitError::msg)
    }

    /// Whether something draws over the frames: see [`FrameHooks::overlay_visible`].
    pub fn overlay_visible(&self) -> bool {
        self.hooks.overlay_visible()
    }

    /// A 2D render target in the stage's format with `usage` on top of `RENDER_ATTACHMENT`.
    pub fn target(&self, width: u32, height: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
        self.gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: self.gpu.label("offscreen target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
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

    /// A frame with nothing to draw into, for a window whose surface gave no image: still loaded, animated
    /// and prepared, so the renderer's persistent scene sees every packet's deltas.
    pub fn skip_frame(&mut self, alpha: f32) {
        self.content.update(&mut self.renderer, &self.gpu);
        self.scene.follow_content(&mut self.content, &mut self.show);
        self.scene.animate(&self.content, alpha, &mut self.show);
        let packet = self.scene.frame_packet(alpha, None, &mut self.show);
        self.renderer.prepare(&self.gpu, packet);
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
        let mut laps = Stopwatch::start();
        // Hot reload and loading run every frame, as in the windowed app, so a bench measures them.
        self.hooks.before_loading(LoadingFrame {
            content: &mut self.content,
            renderer: &mut self.renderer,
            gpu: &self.gpu,
            scene: &mut self.scene,
            show: &mut self.show,
        });
        self.content.update(&mut self.renderer, &self.gpu);
        self.scene.follow_content(&mut self.content, &mut self.show);
        self.laps.content += laps.lap();
        // Reload/live link can change debug CVars, so inspect them after applying it.
        let discard_output = discard_output
            && self.history_only_capture
            && !self.hooks.overlay_visible()
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
                    format: self.format,
                    sample_count: 1,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                },
            )
        } else {
            graph.import_texture("offscreen target", view)
        };
        let (width, height) = graph.size(target);
        // While something draws over the frame, it gets the extract and render stages' times (a bench
        // measures its own way, and fixed steps are run outside this).
        let mut stopwatch = (H::TOOLS && self.hooks.overlay_visible()).then(Stopwatch::start);
        self.hooks.after_steps(&self.show, &self.scene);
        laps.lap();
        let mut scope = AllocScope::begin();
        self.scene.animate(&self.content, alpha, &mut self.show);
        self.laps.animation += laps.lap();
        self.laps.animation_allocations = next_scope(&mut scope);
        self.show.begin_interface(self.renderer.ui_page_state());
        let packet = self.scene.frame_packet(alpha, Some((width, height)), &mut self.show);
        self.laps.packet += laps.lap();
        self.laps.packet_allocations = next_scope(&mut scope);
        let extract = stopwatch.as_mut().map(Stopwatch::lap).unwrap_or_default();
        // Before `prepare`, which culls hulls by it, and readies the debug view.
        self.renderer.set_outline_distance(self.quality.outline_distance());
        self.renderer.set_debug_view(sapphire_render::debug_view::DEBUG_VIEW.get());
        self.renderer.prepare(&self.gpu, packet);
        self.laps.prepare += laps.lap();
        self.laps.prepare_allocations = next_scope(&mut scope);
        let timing = FrameTiming::new(&self.graph, FRAME);
        self.renderer.set_render_scale(self.quality.render_scale((width, height), &timing));
        let gpu_timing = self.quality.wants_gpu_timing() || self.measure_gpu;
        if H::TOOLS {
            laps.lap();
        }
        let started = H::TOOLS.then(Stopwatch::start);
        let frame = ToolFrame {
            width,
            height,
            last_times: self.last_times,
            renderer: &self.renderer,
            quality: &self.quality,
            gpu_timing,
            gpu: &self.gpu,
            scene: &self.scene,
            content: &self.content,
            show: &self.show,
        };
        self.hooks.tools(frame, &mut self.graph);
        let tools = started.map(|started| started.elapsed()).unwrap_or_default();
        if H::TOOLS {
            laps.lap();
            scope = AllocScope::begin();
        }
        self.renderer.record(&mut graph, target);
        self.hooks.record(&mut graph, target);
        after(&mut graph, target);
        if self.lose_frames {
            drop(graph);
            // What `prepare` wrote goes on its way, as a frame's submit would take it.
            self.gpu.queue().submit([]);
        } else {
            self.graph.execute(&self.gpu, graph).map_err(AppInitError::msg)?;
        }
        self.laps.record += laps.lap();
        self.laps.record_allocations = scope.counts();
        if let Some(mut stopwatch) = stopwatch {
            let render = stopwatch.lap().saturating_sub(tools);
            self.last_times = Some(StageTimes { extract, render });
        }
        Ok(discard_output)
    }
}

/// The length dynamic resolution and tools are told a frame takes: frames here have no real length, so
/// dynamic resolution follows GPU timings alone.
pub const FRAME: Duration = Duration::from_micros(16_667);
