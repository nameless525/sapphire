//! Live link in `dev` builds (M1 A9, `sapphire_livelink`): the desktop pushes packs, documents,
//! shader sources and cvars to the running app, on any platform, and each frame, before loading
//! runs, what came is applied through the paths hot reload takes (`reload.rs`):
//!
//! - a pushed pack is served in place of the app's own and remounted ([`Content::apply_pack`]):
//!   the assets whose bytes changed reload behind their handles, and the push is answered once
//!   their new versions are uploaded, with each one's generation;
//! - a document goes to the owner of the type its envelope names ([`Document`]): the day's
//!   timeline (from the day's own file only, so pushing the default day can't replace the
//!   look-dev stage's), the character light rig presets, the screen effect presets or a running
//!   fight's moves, camera and graph (M2 T2, `tuning.rs`);
//! - shader sources replace those files in the newest sources the renderer was asked for, and
//!   build off the frame thread; the push is answered once the new pipelines are swapped in, the
//!   frame after the first one drawn with them, so the tool's time to the reply is the time until
//!   the edit shows (`shader_edits.rs`);
//! - a cvar is set through the overlay's registry, as the console does;
//! - a capture is answered with the next frame the app draws ([`FrameCapture`]): the windowed
//!   app draws that frame into a texture it reads back instead of its surface, and the offscreen
//!   `--offscreen` mode into one beside its own target. The PNG is compressed on a thread of its
//!   own. A minimized window has no frame to capture, so the capture is refused
//!   ([`CaptureRequest::start`]).
//!
//! Whatever is refused (a document that doesn't parse, shaders that don't compile, a pack the
//! asset server won't mount) keeps what it had; the tool hears why, and the overlay's stats panel
//! lists it.
//!
//! On by default in `dev` builds, listening on the loopback interface (127.0.0.1:7811), which the
//! desktop, the iOS Simulator and USB forwards reach; `--live-link` listens elsewhere, dials a
//! tool (`connect=<addr>`, behind `adb reverse`) or turns it off.

use std::sync::Arc;
use std::time::Duration;

use sapphire_assets::AssetChange;
use sapphire_core::cvar::CvarRegistry;
use sapphire_debug::LiveLinkStatus;
use sapphire_livelink::protocol::{
    Capture, DeviceStatus, MAX_CAPTURE, MoveAttempt, PushedPackStatus, ReplyBody, SourceFile,
};
use sapphire_livelink::{
    LinkMode, LiveLink, LivePacks, Push, ReloadTracker, Responder, capture_refusal, list_cvars, set_cvar,
};
use sapphire_platform::{App as _, DeviceClass, Os, SystemInfo};
use sapphire_render::graph::{FrameGraph, TextureId};
use sapphire_render::{Renderer, ShaderFile};
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::app::SandboxApp;
use crate::capture::{compress, readback_fits};
use crate::options::LiveLinkOption;
use crate::shader_edits::{Origin, ShaderEdits};
use game_combat::combat::Combat;
use game_combat::fight::Fight;
use game_combat::tuning::{Document, DocumentFrame, LiveTuning, attempts, error_chain};
use sapphire_stage::Readback;
use sapphire_stage::content::Content;
use sapphire_stage::scene::Scene;

/// How many refusals the overlay lists.
const SHOWN_ERRORS: usize = 4;
/// The most pushes applied in one frame; the rest wait for the next, so a tool sending many can't
/// hold a frame.
const PUSHES_PER_FRAME: usize = 4;
/// The most captures waiting for a frame; they share it. More are refused, so a tool asking
/// while nothing draws (the app in the background) can't grow the list.
const WAITING_CAPTURES: usize = 8;

/// Live link's state in the app.
pub(crate) struct Link {
    link: Option<LiveLink>,
    /// Where it listens or dials, or why it is off.
    state: String,
    tracker: ReloadTracker,
    /// The app, its platform and its cook, for the status.
    app: String,
    platform: String,
    cook: &'static str,
    /// The last refusals, newest last.
    errors: Vec<String>,
    /// Captures waiting for the next frame drawn.
    captures: Vec<Responder>,
}

/// What applying pushes reads and changes, once a frame.
pub(crate) struct LinkFrame<'a> {
    pub content: &'a mut Content,
    pub renderer: &'a mut Renderer,
    pub gpu: &'a GpuContext,
    pub scene: &'a mut Scene,
    pub fight: &'a mut Fight,
    pub tuning: &'a mut LiveTuning,
    pub cvars: &'a CvarRegistry,
    pub shaders: &'a mut ShaderEdits,
    /// Frames since the app started, and the last one's length.
    pub frame: u64,
    pub frame_time: Duration,
}

impl Link {
    /// Starts taking connections as `option` says (listening on the loopback interface by
    /// default), serving pushed packs through `packs`. If it can't listen, it says why and stays
    /// off: the app runs on.
    pub fn start(option: Option<LiveLinkOption>, packs: Arc<LivePacks>, info: &SystemInfo) -> Self {
        let metadata = SandboxApp::METADATA;
        let platform = match info.os {
            Os::Windows => "Windows",
            Os::MacOs => "macOS",
            Os::Linux => "Linux",
            Os::Ios if info.is_emulator => "iOS (Simulator)",
            Os::Ios => "iOS",
            Os::Android if info.is_emulator => "Android (emulator)",
            Os::Android => "Android",
        };
        let mut link = Self {
            link: None,
            state: String::new(),
            tracker: ReloadTracker::default(),
            app: format!("{} {}", metadata.name, metadata.version),
            platform: platform.to_owned(),
            // The pipeline's cooks: desktop for desktops, mobile for phones and tablets.
            cook: match info.device_class {
                DeviceClass::Desktop => "desktop",
                DeviceClass::Mobile => "mobile",
            },
            errors: Vec::new(),
            captures: Vec::new(),
        };
        let mode = match option {
            Some(LiveLinkOption::Off) => {
                link.state = "off (--live-link off)".to_owned();
                return link;
            }
            Some(LiveLinkOption::Listen(address)) => LinkMode::Listen(address),
            Some(LiveLinkOption::Connect(address)) => LinkMode::Connect(address),
            None => LinkMode::LOOPBACK,
        };
        match LiveLink::start(mode, packs) {
            Ok(started) => {
                link.state = match mode {
                    LinkMode::Listen(_) => format!("listening on {}", started.address()),
                    LinkMode::Connect(address) => format!("dialing {address}"),
                };
                link.link = Some(started);
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, ?mode, %error, "live link off");
                link.state = format!("off: {error}");
            }
        }
        link
    }

    /// Applies what was pushed since the last frame, and answers pushed packs whose reloads
    /// landed; `changes` are the asset server's since the last frame. Allocates nothing while
    /// nothing came.
    pub fn update(&mut self, changes: &[AssetChange], frame: LinkFrame<'_>) {
        self.tracker.record(changes);
        let Some(link) = &self.link else { return };
        let LinkFrame {
            content,
            renderer,
            gpu,
            scene,
            fight,
            tuning,
            cvars,
            shaders: edits,
            frame,
            frame_time,
        } = frame;
        for push in std::iter::from_fn(|| link.poll()).take(PUSHES_PER_FRAME) {
            profiling::scope!("live link");
            match push {
                Push::Status { reply } => {
                    let counters = link.counters();
                    reply.reply(ReplyBody::Status(DeviceStatus {
                        app: self.app.clone(),
                        platform: self.platform.clone(),
                        cook: self.cook.to_owned(),
                        frame,
                        frame_time_us: u32::try_from(frame_time.as_micros()).unwrap_or(u32::MAX),
                        assets: content.stats().into(),
                        generations: self.tracker.generations().to_vec(),
                        applied: counters.applied,
                        refused: counters.refused,
                        received: counters.received,
                        pushed_packs: content
                            .live_packs()
                            .pushed()
                            .into_iter()
                            .map(|(name, len)| PushedPackStatus { name, len })
                            .collect(),
                        documents: tuning.documents(),
                        attempts: fight_attempts(fight.combat.as_ref(), scene),
                    }));
                }
                Push::Cvar { name, value, reply } => match set_cvar(cvars, &name, &value) {
                    ReplyBody::Error(error) => {
                        refuse(&mut self.errors, reply, format!("cvar {name}: {error}"))
                    }
                    set => {
                        tracing::info!(target: LOG_TARGET, ?set, "live link set a cvar");
                        reply.reply(set);
                    }
                },
                Push::Cvars { reply } => reply.reply(list_cvars(cvars)),
                Push::Document { origin, type_name, text, reply } => {
                    let Some(document) = Document::of_type(&type_name) else {
                        let error = format!("{origin}: the app has no document of type `{type_name}`");
                        refuse(&mut self.errors, reply, error);
                        continue;
                    };
                    let frame = DocumentFrame {
                        content: &mut *content,
                        renderer: &mut *renderer,
                        gpu,
                        scene: &mut *scene,
                        fight: &mut *fight,
                        tuning: &mut *tuning,
                    };
                    match document.apply(&origin, &text, frame) {
                        Ok(replaced) => {
                            tracing::info!(target: LOG_TARGET, origin, "live link replaced the {replaced}");
                            reply.reply(ReplyBody::DocumentApplied { owner: replaced });
                        }
                        Err(error) => refuse(&mut self.errors, reply, format!("{origin}: {error}")),
                    }
                }
                Push::Shaders { files, reply } => {
                    if let Err(error) = shaders(&files, renderer, edits, reply) {
                        self.errors.push(error);
                    }
                }
                Push::Pack { pack, reply } => {
                    let (name, received) = (pack.name().to_owned(), pack.received());
                    let stopwatch = Stopwatch::start();
                    match content.apply_pack(pack) {
                        Ok(outcome) => {
                            tracing::info!(
                                target: LOG_TARGET,
                                pack = name,
                                received,
                                mounted = outcome.mounted,
                                changed = outcome.changed,
                                removed = outcome.removed,
                                reloading = outcome.reloading,
                                elapsed = ?stopwatch.elapsed(),
                                "live link pushed a pack"
                            );
                            self.tracker.track(reply, outcome);
                        }
                        Err(error) => {
                            refuse(&mut self.errors, reply, format!("pack {name:?}: {}", error_chain(&error)))
                        }
                    }
                }
                Push::Capture { reply } if self.captures.len() < WAITING_CAPTURES => {
                    self.captures.push(reply)
                }
                Push::Capture { reply } => {
                    reply.refuse(format!("{WAITING_CAPTURES} captures wait for a frame already"))
                }
            }
        }
        if self.tracker.waiting() {
            self.tracker.settle(&content.stats());
        }
        let excess = self.errors.len().saturating_sub(SHOWN_ERRORS);
        self.errors.drain(..excess);
    }

    /// Takes in the errors of shader pushes that failed to build, which their replies refused.
    pub fn shaders_refused(&mut self, errors: &mut Vec<String>) {
        self.errors.append(errors);
        let excess = self.errors.len().saturating_sub(SHOWN_ERRORS);
        self.errors.drain(..excess);
    }

    /// The captures waiting for a frame, if any. Allocates nothing while none wait.
    pub fn take_capture(&mut self) -> Option<CaptureRequest> {
        (!self.captures.is_empty()).then(|| CaptureRequest { replies: std::mem::take(&mut self.captures) })
    }

    /// What the overlay shows.
    pub fn status(&self) -> LiveLinkStatus<'_> {
        let counters = self.link.as_ref().map(LiveLink::counters).unwrap_or_default();
        LiveLinkStatus {
            state: &self.state,
            connections: counters.connections,
            applied: counters.applied,
            refused: counters.refused,
            received: counters.received,
            errors: &self.errors,
        }
    }
}

/// What the fight's fighters are playing, for the status.
fn fight_attempts(combat: Option<&Combat>, scene: &Scene) -> Vec<MoveAttempt> {
    let combat = combat.into_iter().flat_map(|combat| attempts(combat.simulation(), combat.cast()));
    let show = scene.graph_show().into_iter().flat_map(|show| attempts(show.simulation(), show.cast()));
    combat.chain(show).collect()
}

/// Asks `renderer` to build its programs with pushed `files` in place of theirs, all or none, on
/// top of the newest sources asked for; `edits` answers once they are swapped in, or with why
/// they weren't. Returns the refusal of a file the renderer doesn't have, answered at once.
fn shaders(
    files: &[SourceFile],
    renderer: &mut Renderer,
    edits: &mut ShaderEdits,
    reply: Responder,
) -> Result<(), String> {
    let mut sources = edits.base(renderer);
    for file in files {
        let Some(shader) = ShaderFile::from_file_name(&file.name) else {
            let error = format!("the renderer has no shader file {:?}", file.name);
            reply.refuse(error.clone());
            return Err(error);
        };
        sources.set(shader, file.text.clone());
    }
    let files = files.iter().map(|file| file.name.as_str()).collect::<Vec<_>>().join(", ");
    edits.request(renderer, sources, Origin::LiveLink { files, reply });
    Ok(())
}

/// What to say of pack `name`, rewritten on disk, when `packs` serves a push of it in place of the
/// file (review DV-11), or nothing when it serves the file.
pub(crate) fn cook_not_in_use(packs: &LivePacks, name: &str) -> Option<String> {
    packs.pushed().iter().any(|(pushed, _)| pushed == name).then(|| {
        format!(
            "pack {name:?}: its file changed, but that version isn't in use: live link's push is \
             served until another push or a restart"
        )
    })
}

/// Refuses a push with `error`, which the overlay lists.
fn refuse(errors: &mut Vec<String>, reply: Responder, error: String) {
    tracing::warn!(target: LOG_TARGET, error, "live link refused a push");
    errors.push(error.clone());
    reply.refuse(error);
}

/// Captures live link asked for, which the next frame drawn answers ([`FrameCapture`]).
pub(crate) struct CaptureRequest {
    replies: Vec<Responder>,
}

impl CaptureRequest {
    /// Starts drawing the captures into a `size` frame in `format`. A frame with no pixels (a
    /// minimized window's) has nothing to capture, and one past the device's largest buffer can't
    /// be read back: they are all refused, and there is `None`.
    pub fn start(
        self,
        gpu: &GpuContext,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> Option<FrameCapture> {
        let largest = gpu.device().limits().max_buffer_size;
        let refusal = capture_refusal(size).or_else(|| readback_fits(size.0, size.1, largest).err());
        let Some(error) = refusal else {
            return Some(FrameCapture::new(gpu, size, format, self));
        };
        tracing::warn!(target: LOG_TARGET, error, "live link refused a capture");
        for reply in self.replies {
            reply.refuse(error.clone());
        }
        None
    }
}

/// A frame drawn for [`CaptureRequest`]s: into a texture of its own, the size and format of
/// the frame the app would otherwise draw, which is read back once the frame has run.
pub(crate) struct FrameCapture {
    request: CaptureRequest,
    view: wgpu::TextureView,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    readback: Readback,
}

impl FrameCapture {
    /// A `size` frame, with no side of 0, in `format`, an 8-bit RGBA or BGRA format as surfaces
    /// and offscreen targets are.
    pub fn new(
        gpu: &GpuContext,
        size: (u32, u32),
        format: wgpu::TextureFormat,
        request: CaptureRequest,
    ) -> Self {
        let (width, height) = size;
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: gpu.label("live link capture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { request, view, size, format, readback: Readback::new(gpu, width, height) }
    }

    /// What the frame draws into, in place of the surface's image or the offscreen target.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Adds the copy of the frame's `target` (this capture's [`Self::view`]) for reading back,
    /// after every pass that draws it.
    pub fn record<'f>(&'f self, graph: &mut FrameGraph<'f>, target: TextureId) {
        self.readback.record(graph, target);
    }

    /// Once frame number `frame` (as the status counts them) has been submitted: reads it back,
    /// waiting for the GPU, and answers the captures from a thread of its own, which compresses
    /// the PNG.
    pub fn finish(self, gpu: &GpuContext, frame: u64) {
        let Self { request: CaptureRequest { replies }, size: (width, height), format, readback, .. } = self;
        let mut pixels = match readback.read(gpu) {
            Ok(pixels) => pixels,
            Err(error) => {
                for reply in replies {
                    reply.refuse(format!("the frame couldn't be read back: {error}"));
                }
                return;
            }
        };
        if matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        let spawned = std::thread::Builder::new().name("sapphire-capture".into()).spawn(move || {
            let stopwatch = Stopwatch::start();
            match compress(width, height, pixels, MAX_CAPTURE) {
                Ok((halved, png)) => {
                    tracing::info!(target: LOG_TARGET, frame, width, height, halved, bytes = png.len(), elapsed = ?stopwatch.elapsed(), "live link captured a frame");
                    let captured = |png| ReplyBody::Captured(Capture { frame, width, height, halved, png });
                    // The last reply takes the PNG itself, so the usual single capture copies nothing.
                    let mut replies = replies;
                    let last = replies.pop();
                    for reply in replies {
                        reply.reply(captured(png.clone()));
                    }
                    if let Some(reply) = last {
                        reply.reply(captured(png));
                    }
                }
                Err(error) => {
                    for reply in replies {
                        reply.refuse(format!("the frame couldn't be compressed: {error}"));
                    }
                }
            }
        });
        // The captures, dropped unanswered, time out on the tool's side.
        if let Err(error) = spawned {
            tracing::warn!(target: LOG_TARGET, %error, "live link couldn't compress a capture");
        }
    }
}

/// They need a GPU: run them with
/// `cargo test -p sapphire_sandbox --features dev --lib -- --ignored link::`.
#[cfg(test)]
mod gpu_tests {
    use std::net::SocketAddr;

    use sapphire_assets::MemoryPackSource;
    use sapphire_livelink::{Client, LinkError};
    use sapphire_rhi::{GpuContextDesc, GpuInstance, GpuInstanceDesc};

    use super::*;

    /// What a tool hears when the app takes its capture request and starts it at `size`.
    fn capture_at(gpu: &GpuContext, size: (u32, u32)) -> Result<(), LinkError> {
        let packs = Arc::new(LivePacks::new(MemoryPackSource::new()));
        let link =
            LiveLink::start(LinkMode::Listen(SocketAddr::from(([127, 0, 0, 1], 0))), packs).expect("a link");
        let address = link.address();
        let tool = std::thread::spawn(move || Client::connect(address).expect("a device").capture());
        let reply = loop {
            if let Some(Push::Capture { reply }) = link.poll() {
                break reply;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let request = CaptureRequest { replies: vec![reply] };
        assert!(request.start(gpu, size, wgpu::TextureFormat::Rgba8UnormSrgb).is_none());
        tool.join().expect("the tool's thread").map(|_| ())
    }

    /// A capture of a minimized window (review PN-2): a zero-size texture would be a validation
    /// error, which panics in debug builds, rather than the refusal the tool hears.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_capture_with_no_pixels_is_refused() {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        for size in [(0, 0), (0, 720)] {
            let error = capture_at(&gpu, size).expect_err("a refusal").to_string();
            assert!(error.contains("minimized"), "{size:?}: {error}");
        }
    }
}

/// They need no GPU, but compile only in `dev` builds, which CI tests in its GPU job: run them
/// with `cargo test -p sapphire_sandbox --features dev --lib -- --ignored link::`.
#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use sapphire_assets::pack::{Codec, PackAsset, write_pack};
    use sapphire_assets::{AssetBudgets, AssetId, AssetKind, AssetServer, MemoryPackSource};
    use sapphire_livelink::Client;

    use super::*;

    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_pushed_pack_changed_on_disk_is_told_of() {
        let packs = Arc::new(LivePacks::new(MemoryPackSource::new()));
        let mut server = AssetServer::new(packs.clone(), AssetBudgets::DESKTOP);
        assert_eq!(cook_not_in_use(&packs, "shared"), None, "the app reads its own file");

        let mode = LinkMode::Listen(SocketAddr::from(([127, 0, 0, 1], 0)));
        let link = LiveLink::start(mode, packs.clone()).expect("a link");
        let address = link.address();
        let id = AssetId::from_bytes([1; 16]);
        let asset = PackAsset::encode(id, AssetKind::Rig, Codec::Lz4, b"a rig", Vec::new(), 3).unwrap();
        let bytes = write_pack(vec![asset]).unwrap();
        let tool = std::thread::spawn(move || {
            Client::connect(address).expect("a device").push_pack("shared", &bytes)
        });
        let (pack, reply) = loop {
            if let Some(Push::Pack { pack, reply }) = link.poll() {
                break (pack, reply);
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let outcome = pack.apply(&packs, &mut server).expect("served");
        reply.reply(ReplyBody::PackApplied(outcome));
        tool.join().expect("the tool's thread").expect("the push");

        let warning = cook_not_in_use(&packs, "shared").expect("the push is what the app reads");
        assert!(warning.contains("\"shared\"") && warning.contains("live link's push"), "{warning}");
        assert_eq!(cook_not_in_use(&packs, "zone"), None, "another pack is read from its file");
    }
}
