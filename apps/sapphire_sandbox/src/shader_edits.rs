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
//! reload's, the files whose text changed on disk since its last swap, so a save that failed is
//! built again with the next. So edits of different files add up, whichever made them and whether
//! the one before is still building or swapped in. A request made while another builds supersedes
//! it, and one answer covers every request it superseded; an edit of a file an earlier request
//! edited too replaces that text, as the later save or push. A new
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
        let mut reload =
            Reload::start(false, Path::new("."), Path::new("day.ron"), Path::new("rigs.ron"), Vec::new());
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

    /// The app as a shader edit meets it, with a sky already pushed over live link, for the traces
    /// of an edit across files. The sky is left out of those edits, so it must stay as pushed.
    struct Editing {
        link: LiveLink,
        reload: Reload,
        edits: ShaderEdits,
        packets: FramePacketRing,
        renderer: Renderer,
        gpu: GpuContext,
        _instance: GpuInstance,
        /// The shader files on disk, as hot reload would read them.
        disk: ShaderSources,
        pushed_sky: String,
    }

    impl Editing {
        fn new() -> Self {
            let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
            let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
            let renderer =
                Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, RenderSettings::default())
                    .expect("renderer");
            let packs = Arc::new(LivePacks::new(MemoryPackSource::new()));
            let link = LiveLink::start(LinkMode::Listen(SocketAddr::from(([127, 0, 0, 1], 0))), packs)
                .expect("a link");
            let mut editing = Self {
                link,
                reload: Reload::start(
                    false,
                    Path::new("."),
                    Path::new("day.ron"),
                    Path::new("rigs.ron"),
                    Vec::new(),
                ),
                edits: ShaderEdits::default(),
                packets: FramePacketRing::new(),
                renderer,
                gpu,
                _instance: instance,
                disk: ShaderSources::compiled_in(),
                pushed_sky: commented(ShaderFile::Sky, "pushed over live link"),
            };

            let address = editing.link.address();
            let name = ShaderFile::Sky.file_name().to_owned();
            let file = SourceFile { name, text: editing.pushed_sky.clone() };
            let tool = std::thread::spawn(move || {
                Client::connect(address).expect("a device").push_shaders(vec![file])
            });
            let (files, reply) = loop {
                if let Some(Push::Shaders { files, reply }) = editing.link.poll() {
                    break (files, reply);
                }
                std::thread::sleep(Duration::from_millis(1));
            };
            let mut sources = editing.edits.base(&editing.renderer);
            sources.set(ShaderFile::Sky, files[0].text.clone());
            let origin = Origin::LiveLink { files: files[0].name.clone(), reply };
            editing.edits.request(&mut editing.renderer, sources, origin);
            editing.settle();
            let answer = tool.join().expect("the tool's thread");
            assert!(answer.is_ok(), "the push is answered as applied: {answer:?}");
            editing
        }

        /// Draws until every request has its answer, and returns how many of hot reload's were
        /// swapped in, each of which logged "shaders reloaded".
        fn settle(&mut self) -> u32 {
            let mut reloaded = 0;
            for _ in 0..60_000 {
                let (gpu, renderer, packets) = (&self.gpu, &mut self.renderer, &mut self.packets);
                reloaded += frame(gpu, renderer, packets, &mut self.edits, &mut self.reload);
                if self.edits.waiting.is_empty() {
                    return reloaded;
                }
            }
            panic!("the shader requests weren't answered");
        }

        /// Saves `file` with `text`: hot reload reads the disk and asks for what changed, which is
        /// answered. Returns how many of hot reload's requests were swapped in.
        fn save(&mut self, file: ShaderFile, text: &str) -> u32 {
            self.disk.set(file, text.to_owned());
            let read = self.disk.clone();
            self.reload.request_shaders(read, &mut self.renderer, &mut self.edits);
            self.settle()
        }

        fn errors(&self) -> &[String] {
            self.reload.status().errors
        }

        fn builds_started(&self) -> u32 {
            self.renderer.stats().shader_builds.started
        }

        /// The disk's text of `files` is what draws, and the sky is still the pushed one.
        fn assert_in_use(&self, files: &[ShaderFile]) {
            let sources = self.renderer.shader_sources();
            for &file in files {
                assert!(
                    sources.get(file) == self.disk.get(file),
                    "the disk's {} is in use",
                    file.file_name()
                );
            }
            assert!(sources.get(ShaderFile::Sky) == self.pushed_sky, "the pushed sky stays");
        }
    }

    /// Rename the exact function token in a real consumer/definition file. The staged-edit
    /// test discovers those files from the compiled source inventory.
    fn apply_fog_renamed(file: ShaderFile) -> String {
        let text = file.compiled_in();
        assert!(text.contains("apply_fog("), "{} calls apply_fog", file.file_name());
        text.replace("apply_fog(", "apply_fog_renamed(")
    }

    /// Review DV-1, trigger A: a rename across its real source consumers, saved more than a
    /// build apart. Partial saves fail; the last save publishes the coherent group.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_rename_saved_a_file_at_a_time_lands_with_the_last_save() {
        let mut editing = Editing::new();

        let files: Vec<_> =
            ShaderFile::ALL.into_iter().filter(|file| file.compiled_in().contains("apply_fog(")).collect();
        let (&last, partial) = files.split_last().expect("the shader defines apply_fog");
        assert!(!partial.is_empty(), "the rename must cross actual shader files");
        let original = editing.renderer.shader_sources().clone();
        for &file in partial {
            let builds = editing.builds_started();
            assert_eq!(editing.save(file, &apply_fog_renamed(file)), 0, "partial save of {file}");
            assert_eq!(editing.builds_started(), builds + 1, "partial save of {file} was built");
            assert!(editing.renderer.shader_sources() == &original, "a partial save changed active shaders");
            assert!(
                editing.errors().last().is_some_and(|error| error.contains("apply_fog")),
                "{:?}",
                editing.errors()
            );
        }
        let reloaded = editing.save(last, &apply_fog_renamed(last));
        assert!(reloaded > 0, "the last save completes the group: {:?}", editing.errors());
        editing.assert_in_use(&files);

        // Swapped in, the edit is done: a save of the same text asks for nothing, and an edit of
        // another file keeps it.
        let builds = editing.builds_started();
        assert_eq!(editing.save(last, &apply_fog_renamed(last)), 0);
        assert_eq!(editing.builds_started(), builds);
        let saved = commented(ShaderFile::Post, "saved on disk");
        assert!(editing.save(ShaderFile::Post, &saved) > 0);
        editing.assert_in_use(&files);
        editing.assert_in_use(&[ShaderFile::Post]);
    }

    /// Review DV-1, trigger B: a typo in `frame.wgsl`, an edit of `fog_card.wgsl` that needs what
    /// the first adds, then the typo fixed. Nothing may report "shaders reloaded" with an edit
    /// left out.
    #[test]
    #[ignore = "needs a GPU"]
    fn nothing_is_reported_reloaded_while_an_edit_is_not_running() {
        let mut editing = Editing::new();
        let frame = ShaderFile::Frame.compiled_in();

        let typo = format!("{frame}\nfn added() -> f32 {{ return 1.0 }}\n");
        assert_eq!(editing.save(ShaderFile::Frame, &typo), 0);
        assert!(editing.errors()[0].contains("frame.wgsl"), "{:?}", editing.errors());

        let card = ShaderFile::FogCard.compiled_in();
        let card = format!("{card}\nfn calls_it() -> f32 {{ return added(); }}\n");
        assert_eq!(editing.save(ShaderFile::FogCard, &card), 0, "frame.wgsl still has its typo");
        assert!(!editing.errors().is_empty());

        let fixed = format!("{frame}\nfn added() -> f32 {{ return 1.0; }}\n");
        let reloaded = editing.save(ShaderFile::Frame, &fixed);
        assert!(reloaded > 0, "the typo is fixed: {:?}", editing.errors());
        editing.assert_in_use(&[ShaderFile::Frame, ShaderFile::FogCard]);
    }

    /// A failed edit is built again when any shader file is saved, its text changed or not, since
    /// what it failed against may have changed (a push), and to show why it fails again.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_failed_edit_is_tried_again_by_a_save_of_the_same_text() {
        let mut editing = Editing::new();
        let text = apply_fog_renamed(ShaderFile::Frame);
        assert_eq!(editing.save(ShaderFile::Frame, &text), 0);
        let builds = editing.builds_started();
        assert_eq!(editing.save(ShaderFile::Frame, &text), 0);
        assert_eq!(editing.builds_started(), builds + 1);
        assert!(editing.errors()[0].contains("apply_fog"), "{:?}", editing.errors());
    }
}
