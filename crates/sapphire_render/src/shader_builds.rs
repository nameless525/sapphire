//! Shader builds off the frame thread (M1 A8 follow-up 2): new shader sources from hot reload or
//! live link build their pipelines on a worker while frames go on drawing with the ones the
//! renderer has, and the whole new set replaces them at a frame boundary once it is complete. A
//! cold edit of the anime character shader or `post.wgsl` takes seconds to compile (X2's
//! rehearsal measured 1.7 to 3.5 s on the desktop, the iOS Simulator and the Android emulator),
//! which a build on the frame thread held the frame for.
//!
//! # A build
//!
//! [`Renderer::request_shader_sources`](crate::Renderer::request_shader_sources) finds the programs
//! whose files differ from the sources in use and takes a snapshot of what their pipelines are
//! built from: each owner's `Step`, a closure that owns clones of the device, its layouts, the
//! pipeline cache and the set it has built so far (wgpu's handles are reference counted, `Send`
//! and `Sync`), so the worker never touches the renderer. The worker checks each changed program
//! with naga, then builds the new modules and every pipeline built from their old versions, each
//! on the layout it had, inside error scopes of its own, and sends back what it built (`Job`,
//! `Rebuilt`). It runs on the job system's async compute pool (`sapphire_jobs::spawn_async`), the
//! engine's one place for work that spans frames.
//!
//! **Thread safety.** wgpu's `Device` is `Send + Sync`, and so are the modules, layouts, caches and
//! pipelines the steps hold. wgpu-core 30 creates shader modules and pipelines without a lock
//! held across the compile (its registries' locks are taken briefly to hand out an ID), so the
//! frame thread's own resource creation and submits don't wait on a build. Below it, Metal's
//! device is thread-safe and wgpu-hal takes no lock around it; Vulkan's pipeline creation is
//! thread-safe and its pipeline cache internally synchronized; DX12's shader cache is locked only
//! to look an entry up and to store one, not while compiling. naga's parse and validation are
//! pure functions of the text. Error scopes are per thread (wgpu's `std` feature), so the
//! worker's catch exactly what it built, and the frame thread's never see them.
//!
//! # The swap
//!
//! At the start of each [`Renderer::prepare`](crate::Renderer::prepare), before anything reads a
//! pipeline, the renderer checks, without waiting, whether the build finished. If it did, every
//! rebuilt owner takes its new pipelines at once, the sources in use become the request's, and
//! the request is answered ([`ShaderOutcome`], through
//! [`Renderer::shader_outcomes`](crate::Renderer::shader_outcomes)). A frame draws entirely with
//! the old set or entirely with the new. The mesh pipelines' generation changes as a
//! synchronous rebuild's does, so the debug views copy the new ones for their replays. A build
//! that failed (naga's error at its file and line, or what wgpu refused) is answered with its
//! error and changes nothing.
//!
//! # Coalescing
//!
//! One build runs at a time. A request made while one runs supersedes it: the running build is
//! cancelled (its worker stops at the next pipeline), the new request waits until that worker has
//! stopped, and then builds; a third request replaces the waiting one. A build that was
//! superseded is never swapped in, even if it finished first. A request for the sources a build
//! is already building joins it instead of starting again. An answer covers its request and every
//! earlier one still waiting, whose sources a later request is built on when the app makes it from
//! [`Renderer::requested_shader_sources`](crate::Renderer::requested_shader_sources), as live link's
//! pushes are.
//!
//! # Lazy builds while a build runs
//!
//! Owners keep building pipelines on first use (a new surface variant, f32 positions, a sky
//! material with a nebula, a post variant nothing prewarmed, a debug view), from the sources in
//! use, into the set in use, as before. A build in flight then no longer covers everything built,
//! so the renderer compares what was built when the snapshot was taken with what is built now
//! (`Built`: the sources' generation and each owner's count of pipelines, and the mesh
//! pipelines' generation, which a probe grid's ambient constants change) and, if anything was
//! built since, starts the request again from a new snapshot rather than swap in a set that
//! lacks it. The rebuild of what was already built again costs little where the backend caches
//! compiled shaders (Metal, Vulkan's pipeline cache, DX12's shader cache), and lazy builds are
//! few and finite, so this ends. Sources set on the frame thread meanwhile
//! ([`Renderer::set_shader_sources`](crate::Renderer::set_shader_sources), or the fall back to the
//! compiled-in shaders when a pipeline built later fails under edited ones) start it again the
//! same way, on top of them.
//!
//! # Shutdown and device loss
//!
//! Dropping the renderer cancels the build in flight and waits for its worker to stop, which it
//! does at its next pipeline, so no build outlives its renderer. A device lost while a build runs
//! fails the build's remaining calls at once (wgpu checks the device first), and its answer
//! belongs to the renderer of the lost device: an app makes a new renderer for the new device,
//! dropping the old one, and requests the sources again there, so nothing built on the old
//! device can reach the new one.
//!
//! # Later: M4's PSO warm-up
//!
//! The warm-up (spec §5.12) compiles a pack's listed pipelines on workers while the game draws
//! and lets an object show once its pipelines are ready. It can take from here the snapshot of
//! what a pipeline is built from as an owned, `Send` step, the build inside per-thread error
//! scopes, the cancellation that lets an owner be dropped mid-build, and handing results back
//! at a frame boundary. What it adds: building many pipelines in parallel (a build here runs on
//! one worker), and a cache of pipelines by descriptor that the lazy builds and these rebuilds
//! share, so a set grown during a build is topped up rather than built again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};

use sapphire_rhi::wgpu;

use crate::LOG_TARGET;
use crate::ambient::AmbientPipelines;
use crate::debug_view::RebuiltViews;
use crate::lights::clusters::ClusterPipelines;
use crate::pipelines::Pipelines;
use crate::post::RebuiltPipelines;
use crate::renderer::ShaderUpdate;
use crate::shaders::{self, Program, ShaderError, ShaderSources};
use crate::shadows::ShadowPipelines;
use crate::sky::SkyPipelines;
use crate::temporal::TemporalPipelines;

/// One owner's pipelines built again from new sources: a closure that owns what it builds them
/// from, so a worker can run it while the frame thread draws with the old ones. It checks
/// `Cancel` between pipelines where it builds many.
pub(crate) type Step<T> = Box<dyn FnOnce(&ShaderSources, &Cancel) -> T + Send>;

/// Asks a build to stop early: set when a newer request supersedes it or its renderer is dropped.
/// What a cancelled build returns is thrown away.
#[derive(Clone, Debug, Default)]
pub(crate) struct Cancel(Arc<AtomicBool>);

impl Cancel {
    fn set(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// A request for new shader sources
/// ([`Renderer::request_shader_sources`](crate::Renderer::request_shader_sources)), numbered in
/// the order they were made: an answer to one answers every earlier one still waiting too
/// ([`ShaderOutcome::request`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShaderRequest(u32);

/// How a request for new shader sources ended
/// ([`Renderer::shader_outcomes`](crate::Renderer::shader_outcomes)).
#[derive(Debug)]
pub struct ShaderOutcome {
    /// The newest request this answers. Earlier requests still waiting are answered by it too:
    /// it superseded them.
    pub request: ShaderRequest,
    /// What was built and swapped in, from the frame that prepared it on; or why nothing changed,
    /// the pipelines in use staying.
    pub result: Result<ShaderUpdate, ShaderError>,
    /// Frames prepared between the request and its answer, drawn with the shaders in use while
    /// it built.
    pub frames: u32,
}

/// Shader builds off the frame thread, for the stats overlay and Tracy (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShaderBuildStats {
    /// A build runs on a worker, or a request waits for the one running to stop.
    pub building: bool,
    /// Builds started since the renderer was created, restarts among them.
    pub started: u32,
    /// Builds started again because the renderer built pipelines while they ran.
    pub restarted: u32,
    /// Builds a newer request superseded before they were swapped in.
    pub superseded: u32,
    /// Requests answered with an error.
    pub failed: u32,
}

/// What the renderer had built when a build's snapshot was taken (`Renderer::built`): the
/// sources' generation (`RenderStats::shader_rebuilds`), each owner's count of pipelines, and
/// the mesh pipelines' generation. A build is swapped in only if it is still the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Built {
    pub sources: u32,
    /// The mesh pipelines' generation and count.
    pub mesh: (u32, u32),
    pub sky: u32,
    pub post: u32,
    pub skinning: bool,
    pub shadows: u32,
    pub ambient: u32,
    /// The debug views' own pipelines, once a view was shown.
    pub debug_views: Option<u32>,
}

/// A build: the sources, the programs that changed, and each changed program's owners' steps
/// (none for an owner that built nothing from it). Temporal AA's and the light clusters' are
/// built with the renderer, so they are fixed.
pub(crate) struct Job {
    pub sources: ShaderSources,
    pub changed: Vec<Program>,
    pub device: wgpu::Device,
    /// The mesh pipelines and how many it built.
    pub mesh: Option<Step<(Pipelines, u32)>>,
    pub sky: Option<Step<SkyPipelines>>,
    pub post: Option<Step<RebuiltPipelines>>,
    pub temporal: Option<Step<TemporalPipelines>>,
    pub clusters: Option<Step<ClusterPipelines>>,
    pub skinning: Option<Step<wgpu::ComputePipeline>>,
    pub shadows: Option<Step<ShadowPipelines>>,
    pub ambient: Option<Step<AmbientPipelines>>,
    pub debug_views: Option<Step<RebuiltViews>>,
}

/// What a build built, for `Renderer::swap_pipelines`.
pub(crate) struct Rebuilt {
    pub mesh: Option<Pipelines>,
    pub sky: Option<SkyPipelines>,
    pub post: Option<RebuiltPipelines>,
    pub temporal: Option<TemporalPipelines>,
    pub clusters: Option<ClusterPipelines>,
    pub skinning: Option<wgpu::ComputePipeline>,
    pub shadows: Option<ShadowPipelines>,
    pub ambient: Option<AmbientPipelines>,
    pub debug_views: Option<RebuiltViews>,
    pub update: ShaderUpdate,
}

/// How a build ended. `None` from [`Job::run`]: it was cancelled.
pub(crate) type Finished = Option<Result<Rebuilt, ShaderError>>;

impl Job {
    /// The changed programs' names, for errors.
    fn programs(&self) -> String {
        self.changed.iter().map(|program| program.name()).collect::<Vec<_>>().join(", ")
    }

    /// Checks every changed program with naga, then builds every step, on this thread, inside
    /// validation and internal error scopes of this thread's: wgpu checks what naga accepted
    /// against the layouts and vertex buffers the steps build with, and what it refuses would
    /// otherwise be an uncaptured (and in debug builds fatal) error. Stops early, returning
    /// `None`, once `cancel` is set.
    pub(crate) fn run(self, cancel: &Cancel) -> Finished {
        profiling::scope!("shader build");
        let programs = self.programs();
        let Self {
            sources,
            changed,
            device,
            mesh,
            sky,
            post,
            temporal,
            clusters,
            skinning,
            shadows,
            ambient,
            debug_views,
        } = self;
        for &program in &changed {
            if cancel.is_set() {
                return None;
            }
            if let Err(error) = shaders::validate(program, &sources) {
                return Some(Err(error));
            }
        }
        // Popped in reverse order when they drop, if a cancelled build returns early.
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let mesh = run(mesh, &sources, cancel);
        let sky = run(sky, &sources, cancel);
        let post = run(post, &sources, cancel);
        let temporal = run(temporal, &sources, cancel);
        let clusters = run(clusters, &sources, cancel);
        let skinning = run(skinning, &sources, cancel);
        let shadows = run(shadows, &sources, cancel);
        let ambient = run(ambient, &sources, cancel);
        let debug_views = run(debug_views, &sources, cancel);
        if cancel.is_set() {
            return None;
        }
        if let Some(error) = scope_error(internal.pop()).or(scope_error(validation.pop())) {
            return Some(Err(ShaderError::Pipeline { programs, message: error.to_string() }));
        }
        let pipelines = mesh.as_ref().map_or(0, |(_, built)| *built)
            + sky.as_ref().map_or(0, SkyPipelines::count)
            + post.as_ref().map_or(0, RebuiltPipelines::count)
            + if temporal.is_some() { TemporalPipelines::COUNT } else { 0 }
            + if clusters.is_some() { ClusterPipelines::COUNT } else { 0 }
            + u32::from(skinning.is_some())
            + shadows.as_ref().map_or(0, ShadowPipelines::count)
            + ambient.as_ref().map_or(0, AmbientPipelines::count)
            + debug_views.as_ref().map_or(0, RebuiltViews::count);
        Some(Ok(Rebuilt {
            mesh: mesh.map(|(mesh, _)| mesh),
            sky,
            post,
            temporal,
            clusters,
            skinning,
            shadows,
            ambient,
            debug_views,
            update: ShaderUpdate { programs: changed.len() as u32, pipelines },
        }))
    }
}

/// Runs `step`, if there is one and the build isn't cancelled.
fn run<T>(step: Option<Step<T>>, sources: &ShaderSources, cancel: &Cancel) -> Option<T> {
    step.filter(|_| !cancel.is_set()).map(|step| step(sources, cancel))
}

/// The error an error scope caught: `pop`'s future, which native backends resolve at once. Only
/// WebGPU resolves it later, and the engine has no web build (spec §1.3).
pub(crate) fn scope_error(pop: impl Future<Output = Option<wgpu::Error>>) -> Option<wgpu::Error> {
    let mut pop = std::pin::pin!(pop);
    match pop.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(error) => error,
        Poll::Pending => None,
    }
}

/// A request, as it waits or builds.
#[derive(Debug)]
pub(crate) struct Request {
    pub id: ShaderRequest,
    pub sources: ShaderSources,
    /// Frames prepared since it was made.
    frames: u32,
}

/// The build in flight.
#[derive(Debug)]
struct Running {
    request: Request,
    /// What the renderer had built when the snapshot was taken.
    built: Built,
    cancel: Cancel,
    finished: mpsc::Receiver<Finished>,
}

/// The renderer's builds off the frame thread: the one running, the request waiting for it to
/// stop, and the answers not yet taken.
#[derive(Debug, Default)]
pub(crate) struct ShaderBuilds {
    running: Option<Running>,
    /// The newest request made while a build ran: it superseded that build, and builds once its
    /// worker has stopped.
    waiting: Option<Request>,
    next: u32,
    outcomes: Vec<ShaderOutcome>,
    stats: ShaderBuildStats,
    /// Holds each worker before it builds until the test opens it.
    #[cfg(test)]
    pub(crate) hold: Option<Arc<Gate>>,
}

impl ShaderBuilds {
    /// A new request for `sources`, numbered after every earlier one.
    pub(crate) fn request(&mut self, sources: ShaderSources) -> Request {
        self.next += 1;
        Request { id: ShaderRequest(self.next), sources, frames: 0 }
    }

    /// Takes `request` in, given back if nothing runs, for the renderer to start. Otherwise it
    /// joins the build in flight if it asks for the same sources, or supersedes it: the build is
    /// cancelled and the request waits for its worker to stop, replacing any request waiting.
    pub(crate) fn supersede(&mut self, request: Request) -> Option<Request> {
        let Some(running) = &mut self.running else { return Some(request) };
        if self.waiting.is_none() && running.request.sources == request.sources {
            running.request.id = request.id;
            return None;
        }
        if !running.cancel.is_set() {
            running.cancel.set();
            self.stats.superseded += 1;
        }
        self.waiting = Some(request);
        None
    }

    /// Runs `job`, built for `request` from what the renderer had built (`built`), on a worker.
    pub(crate) fn spawn(&mut self, request: Request, job: Job, built: Built) {
        debug_assert!(self.running.is_none(), "one build at a time");
        let (sender, finished) = mpsc::channel();
        let cancel = Cancel::default();
        #[cfg(test)]
        let hold = self.hold.clone();
        let worker_cancel = cancel.clone();
        sapphire_jobs::spawn_async(move || {
            #[cfg(test)]
            if let Some(hold) = hold {
                hold.wait();
            }
            let programs = job.programs();
            // A panic in a driver or in wgpu still answers the request, so the renderer goes on
            // to the next one rather than wait for this one forever.
            let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.run(&worker_cancel)))
                .unwrap_or_else(|_| {
                    let message = "the build panicked".to_owned();
                    Some(Err(ShaderError::Pipeline { programs, message }))
                });
            // The renderer may be gone, having stopped waiting: then nobody needs it.
            let _ = sender.send(finished);
        });
        self.stats.started += 1;
        self.running = Some(Running { request, built, cancel, finished });
    }

    /// Once a frame: counts the frame for the requests waiting, and returns the build in flight
    /// if it finished, with its request and what the renderer had built when it started. Never
    /// waits.
    pub(crate) fn poll(&mut self) -> Option<(Request, Built, Finished)> {
        if let Some(running) = &mut self.running {
            running.request.frames += 1;
        }
        if let Some(waiting) = &mut self.waiting {
            waiting.frames += 1;
        }
        let running = self.running.as_ref()?;
        let finished = match running.finished.try_recv() {
            Ok(finished) => finished,
            Err(mpsc::TryRecvError::Empty) => return None,
            // The worker dropped its task unrun: a pool shutting down.
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(ShaderError::Pipeline {
                programs: String::new(),
                message: "the build's worker stopped before it finished".to_owned(),
            })),
        };
        let Running { request, built, .. } = self.running.take().expect("checked above");
        Some((request, built, finished))
    }

    /// Whether a newer request superseded the build that just finished.
    pub(crate) fn superseded(&self) -> bool {
        self.waiting.is_some()
    }

    /// The request waiting for the build in flight to stop, once none runs.
    pub(crate) fn take_waiting(&mut self) -> Option<Request> {
        if self.running.is_some() {
            return None;
        }
        self.waiting.take()
    }

    /// Counts a build started again from a new snapshot.
    pub(crate) fn restarted(&mut self) {
        self.stats.restarted += 1;
    }

    /// Answers `request` with `result`.
    pub(crate) fn answer(&mut self, request: Request, result: Result<ShaderUpdate, ShaderError>) {
        if let Err(error) = &result {
            self.stats.failed += 1;
            tracing::warn!(target: LOG_TARGET, %error, "shaders not built; keeping the ones in use");
        }
        self.outcomes.push(ShaderOutcome { request: request.id, result, frames: request.frames });
    }

    /// The sources of the newest request not answered yet, if any.
    pub(crate) fn requested(&self) -> Option<&ShaderSources> {
        let waiting = self.waiting.as_ref().map(|request| &request.sources);
        waiting.or(self.running.as_ref().map(|running| &running.request.sources))
    }

    /// The answers not taken yet, oldest first.
    pub(crate) fn outcomes(&mut self) -> std::vec::Drain<'_, ShaderOutcome> {
        self.outcomes.drain(..)
    }

    pub(crate) fn stats(&self) -> ShaderBuildStats {
        ShaderBuildStats { building: self.running.is_some() || self.waiting.is_some(), ..self.stats }
    }
}

impl Drop for ShaderBuilds {
    /// Cancels the build in flight and waits for its worker to stop: no build outlives its
    /// renderer.
    fn drop(&mut self) {
        let Some(running) = self.running.take() else { return };
        running.cancel.set();
        #[cfg(test)]
        if let Some(hold) = &self.hold {
            hold.open();
        }
        // A worker that ran sent its result; one that never will dropped its sender.
        let _ = running.finished.recv();
    }
}

/// Holds workers before they build until opened, so a test can draw frames while a build is
/// known to be in flight.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct Gate {
    open: std::sync::Mutex<bool>,
    opened: std::sync::Condvar,
}

#[cfg(test)]
impl Gate {
    /// How long a held worker waits at most: a frame that waited on the build would otherwise
    /// hang the test; it fails it instead, the build landing late.
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

    pub(crate) fn open(&self) {
        *self.open.lock().expect("no test panics holding it") = true;
        self.opened.notify_all();
    }

    fn wait(&self) {
        let open = self.open.lock().expect("no test panics holding it");
        let _open = self.opened.wait_timeout_while(open, Self::TIMEOUT, |open| !*open);
    }
}

#[cfg(test)]
mod gpu_tests;
