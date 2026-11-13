//! A tier change in place (M4 T3): the renderer made for another tier's [`RenderSettings`] on the warm-up's
//! threads, with every pipeline the live one has built, and swapped in between frames.
//!
//! Only what the settings shape changes hands. What the game's handles name stays: the mesh pool, the
//! materials, the scene's instances, skinning, the interface, the wind, the characters' entries and the
//! pipeline ledger. A new renderer would start empty, and every mesh, material and instance would go to it
//! again with new handles; this way the run, the fight and the HUD never learn the tier changed, except by the
//! look. The new owners are made with the live renderer's layouts, so what's bound with them binds with the new
//! pipelines, and with its ledger, so their builds count where its do.
//!
//! [`Renderer::build_tier`] starts it, [`TierBuild::is_ready`] says when it's done, and
//! [`Renderer::take_tier`] swaps it in, refusing one built for pipelines or shaders that have changed since, so a
//! frame after it never builds what the live renderer had (`frame_thread_builds` stays 0).

use std::sync::Arc;
use std::sync::mpsc;

use sapphire_assets::probes::ArchivedProbeGrid;
use sapphire_rhi::{GpuContext, wgpu};

use super::{RenderError, RenderSettings, Renderer, Shared};
use crate::pipelines::MeshShape;
use crate::post::PostShape;
use crate::shaders::BuildOn;
use crate::temporal::TemporalStats;

/// What the live renderer had built when a tier's build started: what the build builds, and what it checks
/// again when it's taken.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Held {
    mesh: MeshShape,
    post: PostShape,
    /// A grid held or expected, and the reflection cube's prefilter ([`Ambient::held`](crate::ambient::Ambient)).
    ambient: (bool, bool),
    /// The effects' pipelines, with the mask's or not.
    effects: Option<bool>,
    /// The sky's variants, as bits.
    sky: u8,
    /// The shader sources' generation ([`RenderStats::shader_rebuilds`](super::RenderStats)).
    shader_rebuilds: u32,
}

/// A renderer for another tier, building on the warm-up's threads ([`Renderer::build_tier`]).
#[derive(Debug)]
pub struct TierBuild {
    settings: RenderSettings,
    held: Held,
    receiver: mpsc::Receiver<Result<Box<Renderer>, RenderError>>,
    done: Option<Result<Box<Renderer>, RenderError>>,
}

impl TierBuild {
    /// Whether it has finished, built or failed: [`Renderer::take_tier`] then never waits. Never blocks.
    pub fn is_ready(&mut self) -> bool {
        if self.done.is_none() {
            self.done = match self.receiver.try_recv() {
                Ok(done) => Some(done),
                Err(mpsc::TryRecvError::Empty) => None,
                // The task dropped its end without answering: it panicked.
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(RenderError::InvalidSettings {
                    field: "tier",
                    problem: "the tier's build stopped",
                })),
            };
        }
        self.done.is_some()
    }

    /// The settings it's for.
    pub fn settings(&self) -> &RenderSettings {
        &self.settings
    }
}

/// Why [`Renderer::take_tier`] didn't take a tier's renderer. The live one goes on as it was.
#[derive(Debug, thiserror::Error)]
pub enum TierRefused {
    /// It's still building: ask again when [`TierBuild::is_ready`] says so.
    #[error("the tier's renderer is still building")]
    NotReady,
    /// The live renderer built pipelines, or changed its shaders, since the build started: a frame after the
    /// swap would build them. Build again.
    #[error("the renderer built pipelines or changed shaders since the tier's build started")]
    Stale,
    /// A warm-up is building for the live renderer's owners ([`Renderer::is_warming`]): take it once that's in.
    #[error("a warm-up is building for the live renderer")]
    Warming,
    /// Its build failed, or the probe grid didn't fit it.
    #[error("the tier's renderer failed: {0}")]
    Failed(#[from] RenderError),
}

impl Renderer {
    /// Starts making the renderer `settings` describe on the warm-up's threads, which no frame waits on, with
    /// every pipeline this one has built (M4 T3, the `tier_change` module): for [`Self::take_tier`] at a pause
    /// or a wipe. Fails at once while the shaders are edited (a development build's), whose pipelines are
    /// checked as they build, on the frame's thread.
    pub fn build_tier(
        &self,
        gpu: &Arc<GpuContext>,
        settings: RenderSettings,
    ) -> Result<TierBuild, RenderError> {
        if self.edited_shaders {
            return Err(RenderError::InvalidSettings {
                field: "tier",
                problem: "changes only with the compiled-in shaders",
            });
        }
        settings.validate().map_err(|(field, problem)| RenderError::InvalidSettings { field, problem })?;
        let held = self.held();
        let shared = Shared {
            layouts: self.layouts.clone(),
            pipeline_builds: self.pipeline_builds.clone(),
            shaders: self.shaders.clone(),
            edited_shaders: false,
        };
        let (sender, receiver) = mpsc::channel();
        let (gpu, format, task_settings, task_held) =
            (gpu.clone(), self.color_format, settings, held.clone());
        sapphire_jobs::spawn_warm_up(move || {
            profiling::scope!("Renderer::build_tier");
            let built = build(&gpu, format, task_settings, shared, &task_held).map(Box::new);
            // The receiver may be gone: a build nobody waits for any more.
            let _ = sender.send(built);
        });
        tracing::info!(target: super::LOG_TARGET, "another tier's renderer builds off the frame");
        Ok(TierBuild { settings, held, receiver, done: None })
    }

    /// Swaps in `build`'s renderer between frames, before the next [`Self::prepare`], keeping every mesh,
    /// material and instance; `grid` is the zone's probe grid, if it has one ([`Self::set_probe_grid`]). The next
    /// frame is a camera cut, since temporal history and the shadows' caches start again. Nothing is built: a
    /// build that would need to (the live renderer built pipelines since it started) is [`TierRefused::Stale`].
    /// Refused, this renderer goes on as it was.
    pub fn take_tier(
        &mut self,
        mut build: TierBuild,
        grid: Option<&ArchivedProbeGrid>,
    ) -> Result<(), TierRefused> {
        if !build.is_ready() {
            return Err(TierRefused::NotReady);
        }
        if self.is_warming() {
            return Err(TierRefused::Warming);
        }
        let mut next = build.done.take().expect("ready")?;
        if build.held != self.held() {
            return Err(TierRefused::Stale);
        }
        // The fallible part first, on the new owners alone.
        next.set_probe_grid(grid)?;
        self.settle_last_frame();
        self.swap_owners(&mut next);
        tracing::info!(target: super::LOG_TARGET, temporal = self.settings.temporal, "the tier changed in place");
        Ok(())
    }

    /// What a tier's build builds of what this renderer has.
    fn held(&self) -> Held {
        Held {
            mesh: self.pipelines.shape(),
            post: self.post.held(),
            ambient: self.ambient.held(),
            effects: self.vfx.held(),
            sky: self.sky.ready(),
            shader_rebuilds: self.stats.shader_rebuilds,
        }
    }

    /// Takes `next`'s settings and every owner they shape, keeping what the handles name, and binds them.
    fn swap_owners(&mut self, next: &mut Self) {
        use std::mem::swap;
        swap(&mut self.settings, &mut next.settings);
        swap(&mut self.pipelines, &mut next.pipelines);
        swap(&mut self.frame_uniforms, &mut next.frame_uniforms);
        swap(&mut self.view_uniforms, &mut next.view_uniforms);
        swap(&mut self.view_bind_group, &mut next.view_bind_group);
        swap(&mut self.view_stride, &mut next.view_stride);
        swap(&mut self.first_inset_view, &mut next.first_inset_view);
        swap(&mut self.sky, &mut next.sky);
        swap(&mut self.lights, &mut next.lights);
        swap(&mut self.shadows, &mut next.shadows);
        swap(&mut self.ambient, &mut next.ambient);
        swap(&mut self.fog_cards, &mut next.fog_cards);
        swap(&mut self.vfx, &mut next.vfx);
        swap(&mut self.post, &mut next.post);
        swap(&mut self.insets, &mut next.insets);
        swap(&mut self.temporal, &mut next.temporal);
        // The sky's default material was uploaded to the materials `next` made, which go with it.
        self.sky.share_default_material(&next.sky);
        // Made by the first frame that shows one again: they're a development build's.
        self.debug_views = None;
        self.characters.set_capsule_ao(self.settings.ambient_occlusion.capsules);
        self.shadows.set_character_buffer(&self.device, self.characters.buffer());
        self.ambient.set_character_buffer(self.characters.buffer());
        self.scene_bind_groups = super::create_scene_bind_groups(
            &self.device,
            &self.layouts,
            &self.scene,
            &self.materials,
            &self.fog_cards,
            &self.wind,
        );
        self.rebuild_frame_bind_groups();
        self.insets.redraw();
        // Shed passes are the old owners'; the ladder sheds again with its next step.
        self.shed = super::OptionalPasses::NONE;
        self.prepared = None;
        self.previous_clip_from_world = None;
        self.pending_cut = true;
        self.stats.temporal = TemporalStats { enabled: self.settings.temporal, ..TemporalStats::default() };
        self.update_pool_stats();
    }
}

/// The warm-up task's half: a renderer for `settings`, with every pipeline `held` lists built.
fn build(
    gpu: &GpuContext,
    format: wgpu::TextureFormat,
    settings: RenderSettings,
    shared: Shared,
    held: &Held,
) -> Result<Renderer, RenderError> {
    let mut next = Renderer::create(gpu, format, settings, shared)?;
    let on = BuildOn::Jobs;
    next.ambient.build_held(held.ambient, &next.shaders);
    let constants = next.ambient.constants();
    next.pipelines.set_ambient_constants(&next.device, constants);
    next.pipelines.build_shape(&next.device, &held.mesh, on);
    for format in held.mesh.formats() {
        next.shadows.prepare_format(&next.device, format, &next.shaders);
    }
    next.post.build_held(&held.post, on);
    if let Some(mask) = held.effects {
        next.vfx.build(&next.shaders, mask, on);
    }
    next.sky.build_ready(held.sky, on);
    if gpu.is_lost() {
        return Err(RenderError::DeviceLost);
    }
    Ok(next)
}
