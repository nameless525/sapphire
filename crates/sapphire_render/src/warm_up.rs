//! The warm-up off the frame (M4 H1, spec §5.12 "Warm-up"): what a load declares
//! ([`PipelineInventory`]) builds on the job system's warm-up pool, which no frame
//! waits on, while frames go on drawing, and the frame's [`Renderer::prepare`](crate::Renderer::prepare) takes in
//! what's done.
//!
//! # Why a pool of its own
//!
//! A cold pipeline takes 0.4 to 0.8 s to compile on an M5's Metal, more on a phone. Built on the frame's thread
//! it stalls the frame; built on the compute pool it still does, because a frame waits on that pool (the graph
//! executor, the ECS, animation's loops) and a thread waiting on a `bevy_tasks` scope runs the pool's queued
//! tasks, builds among them: M4 H0 measured 3 to 7 frames of 0.4 to 1.5 s per cold warm-up that way, against a
//! worst frame of 13 ms with the builds on threads no frame waits on. `sapphire_jobs::spawn_warm_up` runs on such
//! threads, as many as the compute pool has, and a batch opened there stays there.
//!
//! # A warm-up
//!
//! Built as `shader_builds` builds an edit. [`Renderer::warm`](crate::Renderer::warm) asks each owner what of the
//! inventory it lacks, and each that lacks something hands over a step: a closure owning clones of the device,
//! its layouts, the pipeline cache and, where it builds into a set, its set, so the worker never touches the
//! renderer. One task on the warm-up pool runs every step at once (each spreads its own batch, which stays on the
//! warm-up pool), and sends back what they built (`Warmed`). Frames prepare meanwhile, polling it without
//! waiting ([`Renderer::warm_up`](crate::Renderer::warm_up) says how far it has got).
//!
//! # Taking it in
//!
//! At the start of the `prepare` after it finished, each owner takes in what it lacks still: post's, the sky's,
//! the casters', the ambient pass's, effects', skinning's, the interface's and debug draw's. The mesh pipelines are
//! taken whole, since a probe grid's constants build them again: only if nothing was built into them meanwhile
//! (`Pipelines::fingerprint`), or else the warm-up starts again from what they are now. New shader sources swapped
//! in meanwhile start it again too, everything it built being of the old ones. Then the prewarm runs on the
//! frame's thread as it always has, which sets what isn't a pipeline (post's and the sky's variants ready, a
//! probe grid expected, effects' buffers) and finds every pipeline built: whatever it does build there is a hitch,
//! counted ([`PipelineStats::frame_thread_builds`](crate::PipelineStats::frame_thread_builds)) and named.
//!
//! A warm-up asked for while one builds waits, with every request since added to it
//! ([`PipelineInventory::add`](crate::PipelineInventory::add)); requests are answered in order
//! ([`Renderer::is_warm`](crate::Renderer::is_warm)).
//! [`Renderer::prewarm`](crate::Renderer::prewarm) asks and waits: a load that blocks anyway, a capture, a tool or
//! a test. Its wait after the first frame is a hitch too ([`PipelineStats::frame_waits`](crate::PipelineStats)).
//!
//! # Edited sources and failures
//!
//! Compiled-in sources build at once across the warm-up pool, unscoped, as they do on the compute pool
//! (`shaders::BuildOn`). Edited ones build one after another on the warm-up's one task, inside its error scopes;
//! should wgpu refuse one, or a step panic, nothing is taken in and the prewarm runs on the frame's thread, where
//! `checked_build` goes back to the compiled-in shaders as before.
//!
//! # Shutdown
//!
//! Dropping the renderer cancels the warm-up (steps not started don't run, and the mesh step stops between
//! formats) and waits for its task to stop, so no warm-up outlives its renderer.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, mpsc};

use sapphire_assets::mesh::PositionFormat;
use sapphire_rhi::wgpu;

use crate::LOG_TARGET;
use crate::ambient::AmbientPipelines;
use crate::debug_draw::DebugDraw;
use crate::pipeline_builds::PipelineInventory;
use crate::pipelines::Pipelines;
use crate::post::WarmedPost;
use crate::shader_builds::{Cancel, Step, scope_error};
use crate::shaders::ShaderSources;
use crate::skinning::SkinningPass;
use crate::sky::SkyPipelines;
use crate::ui::WarmedUi;
use crate::vfx::WarmedVfx;

/// A request to warm up ([`Renderer::warm`](crate::Renderer::warm)), numbered in the order they were made: one is
/// warm once it and every request before it are taken in ([`Renderer::is_warm`](crate::Renderer::is_warm)). It
/// names the renderer it was made of, so a renderer made since (a tier's) isn't taken for it
/// ([`Renderer::owns_warm_up`](crate::Renderer::owns_warm_up)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WarmUpRequest {
    owner: u32,
    number: u32,
}

/// Which renderer's warm-ups these are: each renderer's are numbered apart.
#[derive(Debug)]
struct Owner(u32);

impl Default for Owner {
    fn default() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// How far the warm-up has got, for a screen that shows it ([`Renderer::warm_up`](crate::Renderer::warm_up)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WarmUpProgress {
    /// Something asked for is building or waiting to.
    pub warming: bool,
    /// The steps of the warm-up building (one per owner that lacked something), and those done. A request
    /// waiting adds its steps when it starts.
    pub steps: u32,
    pub steps_done: u32,
    /// Pipelines the warm-up's threads have built since the renderer was made.
    pub built: u32,
}

impl WarmUpProgress {
    /// The share of the warm-up building that's done, 1 when nothing builds.
    pub fn fraction(self) -> f32 {
        if self.steps == 0 { 1.0 } else { self.steps_done as f32 / self.steps as f32 }
    }
}

/// The warm-up's own counters (rule R9, [`RenderStats::warm_up`](crate::RenderStats::warm_up)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WarmUpStats {
    /// A warm-up builds, or waits to.
    pub warming: bool,
    /// Requests made, and those answered.
    pub requests: u32,
    pub answered: u32,
    /// Warm-ups started on the pool, restarts among them.
    pub started: u32,
    /// Started again: pipelines were built into the mesh set, or new shader sources swapped in, meanwhile.
    pub restarted: u32,
    /// Warm-ups whose build failed (an edit wgpu refused, a panic), the frame's thread building instead.
    pub failed: u32,
}

/// What each owner built, from [`Job::run`].
#[derive(Default)]
pub(crate) struct Warmed {
    pub mesh: Option<Pipelines>,
    pub shadows: Option<Vec<(PositionFormat, wgpu::RenderPipeline)>>,
    pub ambient: Option<AmbientPipelines>,
    pub skinning: Option<SkinningPass>,
    pub post: Option<WarmedPost>,
    pub vfx: Option<WarmedVfx>,
    pub ui: Option<WarmedUi>,
    pub sky: Option<SkyPipelines>,
    pub debug_draw: Option<DebugDraw>,
}

/// One owner's built pipelines, as a step returns them: boxed, as their sizes differ by a kilobyte.
enum Part {
    Mesh(Box<Pipelines>),
    Shadows(Vec<(PositionFormat, wgpu::RenderPipeline)>),
    Ambient(Box<AmbientPipelines>),
    Skinning(Box<SkinningPass>),
    Post(Box<WarmedPost>),
    Vfx(Box<WarmedVfx>),
    Ui(WarmedUi),
    Sky(Box<SkyPipelines>),
    DebugDraw(Box<DebugDraw>),
}

/// A warm-up: each owner's step, for the sources the renderer builds from.
pub(crate) struct Job {
    pub sources: ShaderSources,
    pub device: wgpu::Device,
    /// The sources are edited: build one after another, inside this task's error scopes.
    pub edited: bool,
    pub mesh: Option<Step<Pipelines>>,
    pub shadows: Option<Step<Vec<(PositionFormat, wgpu::RenderPipeline)>>>,
    pub ambient: Option<Step<AmbientPipelines>>,
    pub skinning: Option<Step<SkinningPass>>,
    pub post: Option<Step<WarmedPost>>,
    pub vfx: Option<Step<WarmedVfx>>,
    pub ui: Option<Step<WarmedUi>>,
    pub sky: Option<Step<SkyPipelines>>,
    pub debug_draw: Option<Step<DebugDraw>>,
}

type PartStep = Box<dyn FnOnce(&ShaderSources, &Cancel) -> Part + Send>;

fn part<T: 'static>(step: Option<Step<T>>, wrap: fn(T) -> Part) -> Option<PartStep> {
    step.map(|step| -> PartStep { Box::new(move |sources, cancel| wrap(step(sources, cancel))) })
}

impl Job {
    fn parts(self) -> (ShaderSources, wgpu::Device, bool, Vec<PartStep>) {
        let Self {
            sources,
            device,
            edited,
            mesh,
            shadows,
            ambient,
            skinning,
            post,
            vfx,
            ui,
            sky,
            debug_draw,
        } = self;
        let parts = [
            part(mesh, |built| Part::Mesh(Box::new(built))),
            part(shadows, Part::Shadows),
            part(ambient, |built| Part::Ambient(Box::new(built))),
            part(skinning, |built| Part::Skinning(Box::new(built))),
            part(post, |built| Part::Post(Box::new(built))),
            part(vfx, |built| Part::Vfx(Box::new(built))),
            part(ui, Part::Ui),
            part(sky, |built| Part::Sky(Box::new(built))),
            part(debug_draw, |draw| Part::DebugDraw(Box::new(draw))),
        ];
        (sources, device, edited, parts.into_iter().flatten().collect())
    }

    /// Steps in it: one per owner that lacks something.
    pub(crate) fn steps(&self) -> u32 {
        let has = [
            self.mesh.is_some(),
            self.shadows.is_some(),
            self.ambient.is_some(),
            self.skinning.is_some(),
            self.post.is_some(),
            self.vfx.is_some(),
            self.ui.is_some(),
            self.sky.is_some(),
            self.debug_draw.is_some(),
        ];
        has.into_iter().filter(|&has| has).count() as u32
    }

    /// Runs every step on this thread, one of the warm-up pool's: at once, each spreading its batch on the
    /// warm-up pool, for compiled-in sources; one after another inside this thread's error scopes for edited
    /// ones. `None` once `cancel` is set; an error if wgpu refused one.
    fn run(self, cancel: &Cancel, done: &AtomicU32) -> Option<Result<Warmed, String>> {
        profiling::scope!("warm-up");
        let (sources, device, edited, steps) = self.parts();
        let run = |step: PartStep| {
            let part = (!cancel.is_set()).then(|| step(&sources, cancel));
            done.fetch_add(1, Ordering::Relaxed);
            part
        };
        let parts: Vec<Option<Part>> = if edited {
            let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
            let parts = steps.into_iter().map(run).collect();
            if let Some(error) = scope_error(internal.pop()).or(scope_error(validation.pop())) {
                return Some(Err(error.to_string()));
            }
            parts
        } else {
            let run = &run;
            sapphire_jobs::scope(|scope| {
                for step in steps {
                    scope.spawn(move || run(step));
                }
            })
        };
        if cancel.is_set() {
            return None;
        }
        let mut warmed = Warmed::default();
        for part in parts.into_iter().flatten() {
            match part {
                Part::Mesh(mesh) => warmed.mesh = Some(*mesh),
                Part::Shadows(casters) => warmed.shadows = Some(casters),
                Part::Ambient(ambient) => warmed.ambient = Some(*ambient),
                Part::Skinning(pass) => warmed.skinning = Some(*pass),
                Part::Post(post) => warmed.post = Some(*post),
                Part::Vfx(vfx) => warmed.vfx = Some(*vfx),
                Part::Ui(ui) => warmed.ui = Some(ui),
                Part::Sky(sky) => warmed.sky = Some(*sky),
                Part::DebugDraw(draw) => warmed.debug_draw = Some(*draw),
            }
        }
        Some(Ok(warmed))
    }
}

/// How a warm-up ended: `None`, cancelled; an error, nothing to take in.
pub(crate) type Finished = Option<Result<Warmed, String>>;

/// The warm-up building.
#[derive(Debug)]
struct Running {
    /// The newest request it answers.
    request: WarmUpRequest,
    inventory: PipelineInventory,
    /// The shader sources' generation when its steps were taken
    /// ([`RenderStats::shader_rebuilds`](crate::RenderStats::shader_rebuilds)).
    sources: u32,
    /// The mesh pipelines' fingerprint then.
    mesh: (u32, u32, bool),
    steps: u32,
    done: Arc<AtomicU32>,
    cancel: Cancel,
    finished: mpsc::Receiver<Finished>,
}

/// What a finished warm-up hands the renderer to take in.
pub(crate) struct Done {
    pub request: WarmUpRequest,
    pub inventory: PipelineInventory,
    pub sources: u32,
    pub mesh: (u32, u32, bool),
    pub finished: Finished,
}

/// The renderer's warm-ups: the one building, the requests waiting for it, and what's answered.
#[derive(Debug, Default)]
pub(crate) struct WarmUps {
    owner: Owner,
    running: Option<Running>,
    /// Every request made while one builds, added up, and the newest of them.
    waiting: Option<(WarmUpRequest, PipelineInventory)>,
    next: u32,
    /// The newest request taken in: it and every one before are warm.
    answered: u32,
    stats: WarmUpStats,
    /// Holds each warm-up's task before it builds until the test opens it.
    #[cfg(test)]
    pub(crate) hold: Option<Arc<crate::shader_builds::Gate>>,
}

impl WarmUps {
    /// Numbers a request for `inventory`, and gives the inventory back for the renderer to start if nothing builds
    /// or waits; otherwise it waits, with every request since.
    pub(crate) fn request(
        &mut self,
        inventory: &PipelineInventory,
    ) -> (WarmUpRequest, Option<PipelineInventory>) {
        self.next += 1;
        self.stats.requests += 1;
        let request = WarmUpRequest { owner: self.owner.0, number: self.next };
        if self.running.is_none() && self.waiting.is_none() {
            return (request, Some(inventory.clone()));
        }
        match &mut self.waiting {
            Some((newest, waiting)) => {
                waiting.add(inventory);
                *newest = request;
            }
            None => self.waiting = Some((request, inventory.clone())),
        }
        (request, None)
    }

    /// Runs `job`, for `request` and those before it, on the warm-up pool. `sources` and `mesh` are what the
    /// renderer had when it took the steps: the sources' generation and the mesh pipelines' fingerprint.
    pub(crate) fn spawn(
        &mut self,
        request: WarmUpRequest,
        inventory: PipelineInventory,
        job: Job,
        (sources, mesh): (u32, (u32, u32, bool)),
    ) {
        debug_assert!(self.running.is_none(), "one warm-up at a time");
        let (sender, finished) = mpsc::channel();
        let (cancel, done, steps) = (Cancel::default(), Arc::new(AtomicU32::new(0)), job.steps());
        let (worker_cancel, worker_done) = (cancel.clone(), done.clone());
        #[cfg(test)]
        let hold = self.hold.clone();
        sapphire_jobs::spawn_warm_up(move || {
            #[cfg(test)]
            if let Some(hold) = hold {
                hold.wait();
            }
            // A panic in a driver or in wgpu still answers, so the renderer builds on its own thread rather than
            // wait for ever.
            let run = std::panic::AssertUnwindSafe(|| job.run(&worker_cancel, &worker_done));
            let finished = std::panic::catch_unwind(run)
                .unwrap_or_else(|_| Some(Err("the warm-up panicked".to_owned())));
            // The renderer may be gone, having stopped waiting: then nobody needs it.
            let _ = sender.send(finished);
        });
        self.stats.started += 1;
        self.running = Some(Running { request, inventory, sources, mesh, steps, done, cancel, finished });
    }

    /// Once a frame, never waiting: the warm-up building, if it finished.
    pub(crate) fn poll(&mut self) -> Option<Done> {
        let running = self.running.as_ref()?;
        let finished = match running.finished.try_recv() {
            Ok(finished) => finished,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("the warm-up's task stopped".to_owned())),
        };
        Some(self.finish(finished))
    }

    /// Waits for the warm-up building to finish: `None` if none builds.
    pub(crate) fn wait(&mut self) -> Option<Done> {
        let running = self.running.as_ref()?;
        let finished =
            running.finished.recv().unwrap_or_else(|_| Some(Err("the warm-up's task stopped".to_owned())));
        Some(self.finish(finished))
    }

    fn finish(&mut self, finished: Finished) -> Done {
        let Running { request, inventory, sources, mesh, .. } = self.running.take().expect("one runs");
        if matches!(finished, Some(Err(_))) {
            self.stats.failed += 1;
        }
        Done { request, inventory, sources, mesh, finished }
    }

    /// The requests waiting, added up, once none builds.
    pub(crate) fn take_waiting(&mut self) -> Option<(WarmUpRequest, PipelineInventory)> {
        if self.running.is_some() {
            return None;
        }
        self.waiting.take()
    }

    /// `request`'s warm-up starts again: with the requests waiting, if any, added to it.
    pub(crate) fn restart(
        &mut self,
        request: WarmUpRequest,
        mut inventory: PipelineInventory,
    ) -> (WarmUpRequest, PipelineInventory) {
        self.stats.restarted += 1;
        match self.waiting.take() {
            Some((newest, waiting)) => {
                inventory.add(&waiting);
                (newest, inventory)
            }
            None => (request, inventory),
        }
    }

    /// `request` and every one before it are taken in.
    pub(crate) fn answer(&mut self, request: WarmUpRequest) {
        self.answered = self.answered.max(request.number);
        self.stats.answered = self.answered;
    }

    pub(crate) fn is_warm(&self, request: WarmUpRequest) -> bool {
        self.owns(request) && request.number <= self.answered
    }

    pub(crate) fn owns(&self, request: WarmUpRequest) -> bool {
        request.owner == self.owner.0
    }

    /// How far it has got, with `built` pipelines built by the warm-up's threads so far.
    pub(crate) fn progress(&self, built: u32) -> WarmUpProgress {
        let (steps, steps_done) = self.running.as_ref().map_or((0, 0), |running| {
            (running.steps, running.done.load(Ordering::Relaxed).min(running.steps))
        });
        WarmUpProgress { warming: self.stats().warming, steps, steps_done, built }
    }

    pub(crate) fn stats(&self) -> WarmUpStats {
        WarmUpStats { warming: self.running.is_some() || self.waiting.is_some(), ..self.stats }
    }
}

impl Drop for WarmUps {
    /// Cancels the warm-up building and waits for its task to stop: no warm-up outlives its renderer.
    fn drop(&mut self) {
        let Some(running) = self.running.take() else { return };
        running.cancel.set();
        #[cfg(test)]
        if let Some(hold) = &self.hold {
            hold.open();
        }
        // A task that ran sent its result; one that never will dropped its sender.
        let _ = running.finished.recv();
        tracing::debug!(target: LOG_TARGET, "a warm-up was cancelled with its renderer");
    }
}

#[cfg(test)]
mod gpu_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_made_while_one_builds_wait_together_and_are_answered_in_order() {
        let mut warm_ups = WarmUps::default();
        let ui = PipelineInventory { ui: true, ..PipelineInventory::default() };
        let (first, start) = warm_ups.request(&ui);
        assert_eq!(start, Some(ui.clone()));
        // As if it were building.
        let (_sender, finished) = mpsc::channel();
        warm_ups.running = Some(Running {
            request: first,
            inventory: ui,
            sources: 0,
            mesh: (0, 0, false),
            steps: 2,
            done: Arc::new(AtomicU32::new(1)),
            cancel: Cancel::default(),
            finished,
        });
        let effects = PipelineInventory { effects: true, ..PipelineInventory::default() };
        let fades = PipelineInventory { fades: true, ..PipelineInventory::default() };
        let (second, start) = warm_ups.request(&effects);
        assert!(start.is_none());
        let (third, _) = warm_ups.request(&fades);
        assert!(warm_ups.take_waiting().is_none(), "not while one builds");
        assert_eq!(warm_ups.progress(5), WarmUpProgress { warming: true, steps: 2, steps_done: 1, built: 5 });
        assert!((warm_ups.progress(5).fraction() - 0.5).abs() < 1e-6);
        warm_ups.running = None;
        warm_ups.answer(first);
        assert!(warm_ups.is_warm(first) && !warm_ups.is_warm(second));
        let (newest, waiting) = warm_ups.take_waiting().expect("two waited");
        assert_eq!(newest, third);
        assert!(waiting.effects && waiting.fades && !waiting.ui);
        warm_ups.answer(newest);
        assert!(warm_ups.is_warm(second) && warm_ups.is_warm(third));
        let stats = warm_ups.stats();
        assert_eq!((stats.warming, stats.requests, stats.answered), (false, 3, 3));

        // Another renderer's requests are numbered apart: none of these is its.
        let other = WarmUps::default();
        assert!(!other.owns(first) && !other.is_warm(first) && warm_ups.owns(first));
    }
}
