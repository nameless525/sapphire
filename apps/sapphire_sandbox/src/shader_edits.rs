//! Shader edits in `dev` builds (M1 A8 follow-up 2): new shader sources from hot reload
//! (`reload.rs`) and live link (`link.rs`) go to the renderer, which builds them off the frame
//! thread while frames go on drawing with the shaders in use, and swaps the new pipelines in at a
//! frame boundary once they are all built (`Renderer::request_shader_sources`). Each request is
//! answered then, the frame after the first one drawn with them: hot reload's with a log line,
//! live link's push with its reply, so the tool's time from push to reply is the time until the
//! edit shows, as it was when the frame built them itself.
//!
//! Every request is built on the newest sources asked for (`Renderer::requested_shader_sources`,
//! [`ShaderEdits::base`]), replacing only the files it edits: live link's, the files pushed; hot
//! reload's, the files whose text changed on disk. So edits of different files add up, whichever
//! made them and whether the one before is still building or swapped in. A request made while
//! another builds supersedes it, and one answer covers every request it superseded; an edit of a
//! file an earlier request edited too replaces that text, as the later save or push. A new
//! renderer (a tier change, device loss) builds the newest sources asked for, or else the last
//! edited ones swapped in, and the requests still waiting are answered when it has.

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

/// Shader edits from live link and hot reload together, through a renderer and a live link of
/// their own. They need a GPU: run them with
/// `cargo test -p sapphire_sandbox --features dev --lib -- --ignored shader_edits::`.
#[cfg(test)]
mod gpu_tests {
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use sapphire_assets::MemoryPackSource;
    use sapphire_livelink::protocol::SourceFile;
    use sapphire_livelink::{Client, LinkMode, LiveLink, LivePacks, Push};
    use sapphire_math::Vec3;
    use sapphire_render::{CameraData, FramePacketRing, RenderSettings, ShaderFile, ViewData};
    use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

    use super::*;
    use crate::reload::Reload;

    /// `file`'s compiled-in text with a comment line added: an edit that builds its programs
    /// again and draws as before.
    fn commented(file: ShaderFile, comment: &str) -> String {
        format!("{}\n// {comment}\n", file.compiled_in())
    }

    /// Hot reload's reading of the shader files: each compiled in, but `file`'s `text`.
    fn read_from_disk(file: ShaderFile, text: &str) -> ShaderSources {
        let mut read = ShaderSources::compiled_in();
        read.set(file, text.to_owned());
        read
    }

    /// Prepares a frame, as the app does before drawing one, which swaps in a finished build;
    /// then answers the shader requests the renderer answered. Returns how many of hot reload's
    /// were swapped in.
    fn frame(
        gpu: &GpuContext,
        renderer: &mut Renderer,
        packets: &mut FramePacketRing,
        edits: &mut ShaderEdits,
        reload: &mut Reload,
    ) -> u32 {
        let packet = packets.begin();
        let camera = CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 1.2, 1.0, 0.05);
        packet.views.push(ViewData::new(camera));
        renderer.prepare(gpu, packets.current());
        let mut answered = edits.answer(renderer);
        reload.shaders_answered(&mut answered);
        std::thread::sleep(Duration::from_millis(1));
        answered.reloaded
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn a_push_survives_a_hot_reload_of_another_file() {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let mut renderer =
            Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, RenderSettings::default())
                .expect("renderer");
        let mut edits = ShaderEdits::default();
        let mut reload = Reload::start(false, Path::new("."), Path::new("day.ron"), Path::new("rigs.ron"));
        let packs = Arc::new(LivePacks::new(MemoryPackSource::new()));
        let link =
            LiveLink::start(LinkMode::Listen(SocketAddr::from(([127, 0, 0, 1], 0))), packs).expect("a link");

        // The tool pushes an edited sky.
        let pushed = commented(ShaderFile::Sky, "pushed over live link");
        let address = link.address();
        let file = SourceFile { name: ShaderFile::Sky.file_name().to_owned(), text: pushed.clone() };
        let tool =
            std::thread::spawn(move || Client::connect(address).expect("a device").push_shaders(vec![file]));
        let (files, reply) = loop {
            if let Some(Push::Shaders { files, reply }) = link.poll() {
                break (files, reply);
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        // Applied as the sandbox applies it (`link.rs`), on the newest sources asked for.
        let mut sources = edits.base(&renderer);
        sources.set(ShaderFile::Sky, files[0].text.clone());
        edits.request(&mut renderer, sources, Origin::LiveLink { files: files[0].name.clone(), reply });

        // While it builds, hot reload reads an edited post.wgsl from disk, and every other file
        // as it was: the sky as compiled in, which the push replaced. Its request supersedes the
        // push's build (the push's is still compiling, in practice), and its answer covers both.
        let saved = commented(ShaderFile::Post, "saved on disk");
        reload.request_shaders(read_from_disk(ShaderFile::Post, &saved), &mut renderer, &mut edits);
        let mut packets = FramePacketRing::new();
        for _ in 0..60_000 {
            if tool.is_finished() {
                break;
            }
            frame(&gpu, &mut renderer, &mut packets, &mut edits, &mut reload);
        }
        let answer = tool.join().expect("the tool's thread");
        assert!(answer.is_ok(), "the push is answered as applied: {answer:?}");
        // And it is: the frames draw with both edits.
        let sources = renderer.shader_sources();
        assert!(sources.get(ShaderFile::Post) == saved, "hot reload's edit is in use");
        assert!(sources.get(ShaderFile::Sky) == pushed, "the pushed sky is in use, not the disk's");

        // Swapped in, the push stays through a later save of another file.
        let saved_again = commented(ShaderFile::Post, "saved on disk again");
        reload.request_shaders(read_from_disk(ShaderFile::Post, &saved_again), &mut renderer, &mut edits);
        let reloaded =
            (0..60_000).any(|_| frame(&gpu, &mut renderer, &mut packets, &mut edits, &mut reload) > 0);
        assert!(reloaded, "hot reload's edit is answered");
        let sources = renderer.shader_sources();
        assert!(sources.get(ShaderFile::Post) == saved_again);
        assert!(sources.get(ShaderFile::Sky) == pushed, "the pushed sky stays");

        // A reading in which no file changed (a save of the same text) asks for nothing.
        let builds = renderer.stats().shader_builds.started;
        reload.request_shaders(read_from_disk(ShaderFile::Post, &saved_again), &mut renderer, &mut edits);
        assert_eq!(frame(&gpu, &mut renderer, &mut packets, &mut edits, &mut reload), 0);
        assert_eq!(renderer.stats().shader_builds.started, builds);
    }
}
