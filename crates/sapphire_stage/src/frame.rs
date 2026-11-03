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
use crate::combat::Combat;
use crate::content::{self, Content};
use crate::faces::MaterialOverrides;
use crate::scene::Scene;

/// Where a frame's CPU time went, as [`Stage`] measures it, for whoever reads and resets it each frame.
/// Two clock reads a stage cost nothing a frame can see.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameLaps {
    /// The scene's fixed-step systems.
    pub scene_step: Duration,
    /// The simulation's steps.
    pub simulation: Duration,
    /// Of those, the character controllers with the physics they ask for: `SimStats::controllers_last`.
    pub physics: Duration,
    /// The arena following the steps, and their cues fired: effects spawned and moved.
    pub cues: Duration,
    /// `Content::update` and `Scene::follow_content`.
    pub content: Duration,
    /// `Scene::animate`.
    pub animation: Duration,
    /// `Scene::frame_packet`: presentation, the camera, extraction and the HUD.
    pub packet: Duration,
    /// `Renderer::prepare`.
    pub prepare: Duration,
    /// `Renderer::record` and the frame graph's execution, its submit included.
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

/// What a caller adds to a stage's frames. Every method defaults to nothing, in the frame's order.
pub trait FrameHooks {
    /// Whether [`Self::tools`] does work of its own, whose time and allocations [`FrameLaps`] leaves out.
    const TOOLS: bool = false;

    /// Before each fixed step, on the scene it'll step.
    fn before_fixed_step(&mut self, _scene: &mut Scene) {}

    /// First in a frame, before loading runs: the place to swap in what changed on disk.
    fn before_loading(&mut self, _frame: LoadingFrame<'_>) {}

    /// Whether something draws over the frame, so a capture's warmup can't discard its output.
    fn overlay_visible(&self) -> bool {
        false
    }

    /// Once the frame's fixed steps are over and loading has run.
    fn after_steps(&mut self, _combat: Option<&Combat>, _scene: &Scene) {}

    /// Before recording, with the frame prepared: what asks the graph for GPU timing or not has to,
    /// which is all this does by default. A frame graph's GPU timing comes from one owner.
    fn tools(&mut self, frame: ToolFrame<'_>, executor: &mut GraphExecutor) {
        executor.set_gpu_timing(frame.gpu, frame.gpu_timing);
    }

    /// After the renderer's passes, on the same target.
    fn record<'f>(&'f mut self, _graph: &mut FrameGraph<'f>, _target: TextureId) {}
}

impl FrameHooks for () {}

/// What [`FrameHooks::before_loading`] can change.
pub struct LoadingFrame<'a> {
    pub content: &'a mut Content,
    pub renderer: &'a mut Renderer,
    pub gpu: &'a GpuContext,
    pub scene: &'a mut Scene,
    pub combat: Option<&'a mut Combat>,
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
pub struct ToolFrame<'a> {
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
    pub combat: Option<&'a Combat>,
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
    /// Make the party's recolors of the character ([`Content::keep_party_looks`]).
    pub party_looks: bool,
    /// What every matching material takes as it loads.
    pub material_overrides: MaterialOverrides,
    /// Give the scene its compute-skinning stalk's systems.
    pub skinned: bool,
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
            party_looks: false,
            material_overrides: MaterialOverrides::default(),
            skinned: false,
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

/// The GPU, renderer, content and scene a frame draws with, and the fight beside them if there is one.
pub struct Stage<H: FrameHooks = ()> {
    // Drop order: everything on the device before the context, the context before the instance.
    /// What the caller adds to its frames.
    pub hooks: H,
    pub renderer: Renderer,
    /// A simulation that takes a tick with each fixed step.
    pub combat: Option<Combat>,
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
        let StageDesc {
            format,
            quality,
            settings,
            character,
            sky,
            keep_character_mesh,
            party_looks,
            material_overrides,
            skinned,
        } = desc;
        let mut renderer = Renderer::new(&gpu, format, settings)?;
        let mut content = Content::new(platform, character, sky)?;
        if keep_character_mesh {
            content.keep_character_mesh();
        }
        if party_looks {
            content.keep_party_looks();
        }
        content.override_materials(material_overrides);
        let (character, bounds) = content.character(&mut renderer, &gpu)?;
        let mut scene = Scene::with_character(character, bounds, skinned);
        scene.set_sky_material(content.sky_material(&mut renderer, &gpu)?);
        Ok(Self {
            hooks: (),
            renderer,
            combat: None,
            content,
            graph: GraphExecutor::new(),
            scene,
            quality,
            character,
            measure_gpu: false,
            history_only_capture: false,
            laps: FrameLaps::default(),
            gpu,
            last_times: None,
            format,
            _instance: instance,
        })
    }

    /// The same stage, with `hooks` on its frames.
    pub fn with_hooks<H: FrameHooks>(self, hooks: H) -> Stage<H> {
        let Self {
            hooks: (),
            renderer,
            combat,
            content,
            graph,
            scene,
            quality,
            character,
            measure_gpu,
            history_only_capture,
            laps,
            gpu,
            last_times,
            format,
            _instance,
        } = self;
        Stage {
            hooks,
            renderer,
            combat,
            content,
            graph,
            scene,
            quality,
            character,
            measure_gpu,
            history_only_capture,
            laps,
            gpu,
            last_times,
            format,
            _instance,
        }
    }
}

impl<H: FrameHooks> Stage<H> {
    /// One fixed step and the end of a cue frame: each step here is a frame's worth, as it is on any tier
    /// that draws every tick.
    pub fn step(&mut self) {
        self.fixed_step();
        // No sound and no pad here: a frame's end is all that's left of them. Frames here draw the latest step.
        let frame = 1.0 / sapphire_time::TICK_HZ as f32;
        self.scene.end_cue_frame(frame, frame, 1.0, &mut ());
    }

    /// One fixed step and nothing of the frame's: for a frame that takes several, or asks what the steps
    /// asked of the audio system before the cue frame ends.
    pub fn fixed_step(&mut self) {
        let mut stopwatch = Stopwatch::start();
        self.hooks.before_fixed_step(&mut self.scene);
        self.scene.step();
        // No one's watching a clock here: a step of time is what a frame shows.
        self.scene.drive_arena_camera(1.0 / sapphire_time::TICK_HZ as f32, sapphire_math::Vec2::ZERO, 0.0);
        self.laps.scene_step += stopwatch.lap();
        if let Some(combat) = &mut self.combat {
            combat.step(&mut crate::combat::no_bindings);
            self.laps.simulation += stopwatch.lap();
            self.laps.physics += combat.simulation().stats().controllers_last;
            self.scene.follow_simulation(combat.simulation(), combat.cast());
            self.scene.show_cues(combat.simulation());
            self.laps.cues += stopwatch.lap();
        }
    }

    /// The latest step's hits as the HUD's numbers, for a fight an agent runs with no golden behind it:
    /// captures leave them out, so `arena-fight-hit` keeps its frame.
    #[cfg(feature = "dev")]
    pub fn show_damage(&mut self) {
        if let Some(combat) = &self.combat {
            self.scene.show_damage(combat.simulation());
        }
    }

    /// An error if a replay has left its recorded checksums so far.
    pub fn check_combat(&self) -> Result<(), AppInitError> {
        self.combat.as_ref().map_or(Ok(()), Combat::check).map_err(AppInitError::msg)
    }

    /// An error if the GPU has reported one since the stage was made: see [`gpu_failure`].
    pub fn check_gpu(&self) -> Result<(), AppInitError> {
        gpu_failure(&self.gpu).map_err(AppInitError::msg)
    }

    /// Ends the replay or recording, if there is one: see [`Combat::finish`].
    pub fn finish_combat(&mut self) -> Result<(), AppInitError> {
        if let Some(cues) = self.scene.cue_show() {
            tracing::info!(target: LOG_TARGET, stats = ?cues.stats(), "fired a fight's cues");
        }
        self.combat.take().map_or(Ok(()), Combat::finish).map_err(AppInitError::msg)
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
            combat: self.combat.as_mut(),
        });
        self.content.update(&mut self.renderer, &self.gpu);
        self.scene.follow_content(&mut self.content);
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
        self.hooks.after_steps(self.combat.as_ref(), &self.scene);
        laps.lap();
        let mut scope = AllocScope::begin();
        self.scene.animate(&self.content, alpha);
        self.laps.animation += laps.lap();
        self.laps.animation_allocations = next_scope(&mut scope);
        if let Some(combat) = &self.combat {
            combat.draw_views();
        }
        let packet = self.scene.frame_packet(alpha, Some((width, height)));
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
            combat: self.combat.as_ref(),
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
        self.graph.execute(&self.gpu, graph).map_err(AppInitError::msg)?;
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
