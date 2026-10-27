//! Live link in `dev` builds (M1 A9, `sapphire_livelink`): the desktop pushes packs, documents,
//! shader sources and cvars to the running app, on any platform, and each frame, before loading
//! runs, what came is applied through the paths hot reload takes (`reload.rs`):
//!
//! - a pushed pack is served in place of the app's own and remounted ([`Content::apply_pack`]):
//!   the assets whose bytes changed reload behind their handles, and the push is answered once
//!   their new versions are uploaded, with each one's generation;
//! - a document goes to the owner of the type its envelope names ([`Document`]): the default
//!   day's timeline, the character light rig presets or the screen effect presets;
//! - shader sources replace those files in what the renderer builds with;
//! - a cvar is set through the overlay's registry, as the console does.
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
use sapphire_livelink::protocol::{DeviceStatus, ReplyBody, SourceFile};
use sapphire_livelink::{LinkMode, LiveLink, LivePacks, Push, ReloadTracker, Responder, set_cvar};
use sapphire_platform::{App as _, DeviceClass, Os, SystemInfo};
use sapphire_render::{Renderer, ShaderFile};
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::app::SandboxApp;
use crate::content::Content;
use crate::options::LiveLinkOption;
use crate::reload::{Document, Reload, error_chain, shader_report};
use crate::scene::Scene;

/// How many refusals the overlay lists.
const SHOWN_ERRORS: usize = 4;
/// The most pushes applied in one frame; the rest wait for the next, so a tool sending many can't
/// hold a frame.
const PUSHES_PER_FRAME: usize = 4;

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
}

/// What applying pushes reads and changes, once a frame.
pub(crate) struct LinkFrame<'a> {
    pub content: &'a mut Content,
    pub renderer: &'a mut Renderer,
    pub scene: &'a mut Scene,
    pub cvars: &'a CvarRegistry,
    pub reload: &'a mut Reload,
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
        let LinkFrame { content, renderer, scene, cvars, reload, frame, frame_time } = frame;
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
                    match document.apply(&text, scene, renderer) {
                        Ok(()) => {
                            tracing::info!(target: LOG_TARGET, origin, "live link replaced the {}", document.name());
                            reply.reply(ReplyBody::DocumentApplied { owner: document.name().to_owned() });
                        }
                        Err(error) => refuse(&mut self.errors, reply, format!("{origin}: {error}")),
                    }
                }
                Push::Shaders { files, reply } => {
                    if let Err(error) = shaders(&files, renderer, reload, reply) {
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
            }
        }
        if self.tracker.waiting() {
            self.tracker.settle(&content.stats());
        }
        let excess = self.errors.len().saturating_sub(SHOWN_ERRORS);
        self.errors.drain(..excess);
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

/// Builds `renderer`'s programs with pushed `files` in place of theirs, all or none, and
/// answers. Returns the refusal, if any.
fn shaders(
    files: &[SourceFile],
    renderer: &mut Renderer,
    reload: &mut Reload,
    reply: Responder,
) -> Result<(), String> {
    let mut sources = renderer.shader_sources().clone();
    for file in files {
        let Some(shader) = ShaderFile::from_file_name(&file.name) else {
            let error = format!("the renderer has no shader file {:?}", file.name);
            reply.refuse(error.clone());
            return Err(error);
        };
        sources.set(shader, file.text.clone());
    }
    // On this frame's thread, as hot reload's rebuilds are.
    let stopwatch = Stopwatch::start();
    match renderer.set_shader_sources(&sources) {
        Ok(update) => {
            reload.remember_shaders(renderer);
            tracing::info!(target: LOG_TARGET, ?update, elapsed = ?stopwatch.elapsed(), "live link rebuilt shaders");
            reply.reply(ReplyBody::ShadersApplied { programs: update.programs, pipelines: update.pipelines });
            Ok(())
        }
        Err(error) => {
            let report = shader_report(&error);
            tracing::error!(target: LOG_TARGET, %error, "pushed shaders refused; keeping the last ones\n{report}");
            reply.refuse(if report.is_empty() { error.to_string() } else { format!("{error}\n{report}") });
            Err(format!("shaders: {error}"))
        }
    }
}

/// Refuses a push with `error`, which the overlay lists.
fn refuse(errors: &mut Vec<String>, reply: Responder, error: String) {
    tracing::warn!(target: LOG_TARGET, error, "live link refused a push");
    errors.push(error.clone());
    reply.refuse(error);
}
