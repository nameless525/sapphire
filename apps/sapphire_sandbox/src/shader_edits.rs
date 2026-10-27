//! Shader edits in `dev` builds (M1 A8 follow-up 2): new shader sources from hot reload
//! (`reload.rs`) and live link (`link.rs`) go to the renderer, which builds them off the frame
//! thread while frames go on drawing with the shaders in use, and swaps the new pipelines in at a
//! frame boundary once they are all built (`Renderer::request_shader_sources`). Each request is
//! answered then, the frame after the first one drawn with them: hot reload's with a log line,
//! live link's push with its reply, so the tool's time from push to reply is the time until the
//! edit shows, as it was when the frame built them itself.
//!
//! A request made while another builds supersedes it and is built on its sources
//! (`Renderer::requested_shader_sources`), so pushes of different files add up; one answer covers
//! every request it superseded. A new renderer (a tier change, device loss) builds the newest
//! sources asked for, or else the last edited ones swapped in, and the requests still waiting are
//! answered when it has.

use sapphire_livelink::Responder;
use sapphire_livelink::protocol::ReplyBody;
use sapphire_render::{Renderer, ShaderError, ShaderOutcome, ShaderRequest, ShaderSources};
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;

/// Who asked for new shader sources, which its answer goes to.
pub(crate) enum Origin {
    HotReload,
    /// A push of `files` (their names), answered through `reply`.
    LiveLink {
        files: String,
        reply: Responder,
    },
    /// A new renderer building the sources the last one had.
    NewRenderer,
}

/// A request not answered yet.
struct Waiting {
    request: ShaderRequest,
    origin: Origin,
    /// Since the request: until the edit shows, once answered.
    stopwatch: Stopwatch,
}

/// The requests made since the last frame's that were answered: what hot reload and live link
/// show of them.
#[derive(Default)]
pub(crate) struct Answered {
    /// Hot reload's requests swapped in.
    pub reloaded: u32,
    /// Why hot reload's requests that failed did.
    pub reload_errors: Vec<String>,
    /// Why live link's pushes that failed did, as refused.
    pub link_errors: Vec<String>,
}

/// The shader sources asked of the renderer, and who waits for them.
#[derive(Default)]
pub(crate) struct ShaderEdits {
    /// Oldest first.
    waiting: Vec<Waiting>,
    /// The newest sources asked for while a request waits: a new renderer builds them.
    newest: Option<ShaderSources>,
    /// The edited sources swapped in last, if they aren't the compiled-in ones: a new renderer
    /// builds them when nothing waits.
    applied: Option<ShaderSources>,
}

impl ShaderEdits {
    /// The sources a new edit replaces files of: the newest asked for, so it keeps what the edits
    /// before it changed while they build.
    pub fn base(&self, renderer: &Renderer) -> ShaderSources {
        renderer.requested_shader_sources().clone()
    }

    /// Asks `renderer` to build `sources`, for `origin`.
    pub fn request(&mut self, renderer: &mut Renderer, sources: ShaderSources, origin: Origin) {
        let stopwatch = Stopwatch::start();
        self.newest = Some(sources.clone());
        let request = renderer.request_shader_sources(sources);
        tracing::debug!(target: LOG_TARGET, ?request, "shaders building off the frame thread");
        self.waiting.push(Waiting { request, origin, stopwatch });
    }

    /// Answers the requests `renderer` answered since the last frame, once a frame. Allocates
    /// nothing while none were.
    pub fn answer(&mut self, renderer: &mut Renderer) -> Answered {
        let mut answered = Answered::default();
        let outcomes: Vec<ShaderOutcome> = renderer.shader_outcomes().collect();
        if outcomes.is_empty() {
            return answered;
        }
        for outcome in &outcomes {
            // An answer covers its request and every earlier one: it superseded them.
            let covered =
                self.waiting.iter().take_while(|waiting| waiting.request <= outcome.request).count();
            for waiting in self.waiting.drain(..covered) {
                answer(waiting, outcome, &mut answered);
            }
        }
        if outcomes.iter().any(|outcome| outcome.result.is_ok()) {
            let current = renderer.shader_sources();
            self.applied = (!current.is_compiled_in()).then(|| current.clone());
        }
        if self.waiting.is_empty() {
            self.newest = None;
        }
        answered
    }

    /// A new renderer (a tier change, device loss) builds the newest sources asked for, which
    /// answers the requests the last one left waiting, or else the edited sources the last one
    /// swapped in.
    pub fn renderer_created(&mut self, renderer: &mut Renderer) {
        if let Some(sources) = self.newest.clone().filter(|_| !self.waiting.is_empty()) {
            let request = renderer.request_shader_sources(sources);
            for waiting in &mut self.waiting {
                waiting.request = request;
            }
        } else if let Some(sources) = self.applied.clone() {
            self.request(renderer, sources, Origin::NewRenderer);
        }
    }
}

/// Answers `waiting` with `outcome`: logs it, replies to a push, and counts it in `answered`.
fn answer(waiting: Waiting, outcome: &ShaderOutcome, answered: &mut Answered) {
    let Waiting { origin, stopwatch, .. } = waiting;
    let (visible_after, frames) = (stopwatch.elapsed(), outcome.frames);
    match (origin, &outcome.result) {
        (Origin::HotReload, Ok(update)) => {
            answered.reloaded += 1;
            tracing::info!(target: LOG_TARGET, ?update, ?visible_after, frames, "shaders reloaded");
        }
        (Origin::HotReload, Err(error)) => {
            tracing::error!(target: LOG_TARGET, %error, "shaders not reloaded; keeping the last ones\n{}", shader_report(error));
            answered.reload_errors.push(format!("shaders: {error}"));
        }
        (Origin::LiveLink { files, reply }, Ok(update)) => {
            tracing::info!(target: LOG_TARGET, files, ?update, ?visible_after, frames, "live link rebuilt shaders");
            reply.reply(ReplyBody::ShadersApplied {
                programs: update.programs,
                pipelines: update.pipelines,
                frames,
            });
        }
        (Origin::LiveLink { files, reply }, Err(error)) => {
            let report = shader_report(error);
            tracing::error!(target: LOG_TARGET, files, %error, "pushed shaders refused; keeping the last ones\n{report}");
            reply.refuse(if report.is_empty() { error.to_string() } else { format!("{error}\n{report}") });
            answered.link_errors.push(format!("shaders: {error}"));
        }
        (Origin::NewRenderer, Ok(update)) => {
            tracing::info!(target: LOG_TARGET, ?update, ?visible_after, "the new renderer built the edited shaders");
        }
        (Origin::NewRenderer, Err(error)) => {
            tracing::error!(target: LOG_TARGET, %error, "the new renderer keeps the compiled-in shaders");
        }
    }
}

/// naga's report of a shader error, with the source lines it points at: empty for the
/// pipelines wgpu refused.
fn shader_report(error: &ShaderError) -> &str {
    match error {
        ShaderError::Invalid { report, .. } => report,
        ShaderError::Pipeline { .. } => "",
    }
}
