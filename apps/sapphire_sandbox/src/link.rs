//! Live link in `dev` builds (M1 A9, `sapphire_livelink`): the desktop pushes packs, documents,
//! shader sources and cvars to the running app, on any platform, and each frame, before loading
//! runs, what came is applied through the paths hot reload takes (`reload.rs`):
//!
//! - a pushed pack is served in place of the app's own and remounted ([`Content::apply_pack`]):
//!   the assets whose bytes changed reload behind their handles, and the push is answered once
//!   their new versions are uploaded, with each one's generation;
//! - a document goes to the owner of the type its envelope names ([`Document`]): the day's
//!   timeline (from the day's own file only, so pushing the default day can't replace the
//!   look-dev stage's), the character light rig presets or the screen effect presets;
//! - shader sources replace those files in the newest sources the renderer was asked for, and
//!   build off the frame thread; the push is answered once the new pipelines are swapped in, the
//!   frame after the first one drawn with them, so the tool's time to the reply is the time until
//!   the edit shows (`shader_edits.rs`);
//! - a cvar is set through the overlay's registry, as the console does;
//! - a capture is answered with the next frame the app draws ([`FrameCapture`]): the windowed
//!   app draws that frame into a texture it reads back instead of its surface, and the offscreen
//!   `--offscreen` mode into one beside its own target. The PNG is compressed on a thread of its
//!   own.
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
use sapphire_livelink::protocol::{Capture, DeviceStatus, MAX_CAPTURE, ReplyBody, SourceFile};
use sapphire_livelink::{LinkMode, LiveLink, LivePacks, Push, ReloadTracker, Responder, set_cvar};
use sapphire_platform::{App as _, DeviceClass, Os, SystemInfo};
use sapphire_render::graph::{FrameGraph, TextureId};
use sapphire_render::{Renderer, ShaderFile};
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::app::SandboxApp;
use crate::capture::{Readback, compress};
use crate::content::Content;
use crate::options::LiveLinkOption;
use crate::reload::{Document, error_chain};
use crate::scene::Scene;
use crate::shader_edits::{Origin, ShaderEdits};

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
    pub scene: &'a mut Scene,
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
        let LinkFrame { content, renderer, scene, cvars, shaders: edits, frame, frame_time } = frame;
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
                Push::Document { origin, type_name, text, reply } => {
                    let Some(document) = Document::of_type(&type_name) else {
                        let error = format!("{origin}: the app has no document of type `{type_name}`");
                        refuse(&mut self.errors, reply, error);
                        continue;
                    };
                    match document.apply(&origin, &text, scene, renderer) {
                        Ok(replaced) => {
                            tracing::info!(target: LOG_TARGET, origin, "live link replaced the {replaced}");
                            reply.reply(ReplyBody::DocumentApplied { owner: replaced.to_owned() });
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
    /// A `size` frame in `format`, an 8-bit RGBA or BGRA format as surfaces and offscreen
    /// targets are.
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
                    for reply in replies {
                        let capture = Capture { frame, width, height, halved, png: png.clone() };
                        reply.reply(ReplyBody::Captured(capture));
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
