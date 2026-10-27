//! Hot reload in `dev` builds (M1 A8, `sapphire_hot_reload`): the content root's packs, the
//! renderer's WGSL sources, the day's timeline (the default day's, or with `--look-dev` the
//! look-dev day's, `content/lookdev/lookdev_day.ron`), the character light rig presets and the
//! screen effect presets are watched, and each frame, before loading runs, what changed is
//! applied:
//!
//! - a rewritten pack (`cargo xtask content` after an edit) is remounted: the asset server
//!   reloads the assets that changed behind their handles, and [`Content`] uploads the new
//!   versions within the frame's upload budget, in place;
//! - new shader sources rebuild the renderer's pipelines from the next frame, or keep the ones it
//!   has if they don't compile;
//! - an edited timeline replaces time of day's base timeline if it is the file of the day the
//!   scene runs on ([`Scene::replace_day`]);
//! - edited presets replace the scene's (the `LightRigLibrary` resource) if they are the file of
//!   the presets the scene runs on (the shipped ones, or with `--look-dev` the look-dev stage's,
//!   `content/lookdev/lookdev_light_rigs.ron`; [`Scene::replace_light_rigs`]); characters keep
//!   their preset IDs;
//! - edited screen effect presets replace the scene's (the `EffectLibrary` resource, M1 P3);
//!   effects playing keep the layers they were pushed with, and the renderer prewarms the post
//!   variants the new presets draw through.
//!
//! On by default in `dev` builds; `--no-hot-reload` turns it off. Phones build no file watching
//! (live link, A9, is their trigger), so it reports itself off there. Errors are logged and
//! shown in the debug overlay's stats panel; whatever failed keeps what it had.
//!
//! Live link (`link.rs`) applies pushed documents and shader sources through the same
//! [`Document`]s and [`Reload::remember_shaders`].

use std::path::{Path, PathBuf};

use sapphire_assets::AssetChange;
use sapphire_debug::HotReloadStatus;
use sapphire_hot_reload::{Changes, DEBOUNCE, DocumentId, HotReload, sources};
use sapphire_reflect::stable_name;
use sapphire_render::{Renderer, ShaderError, ShaderSources};
use sapphire_time::Stopwatch;
use sapphire_world::light_rig::LightRigLibrary;
use sapphire_world::screen_effects::EffectLibrary;
use sapphire_world::tod::TodTimeline;

use crate::LOG_TARGET;
use crate::content::Content;
use crate::scene::{Day, Scene};

/// A document the sandbox swaps in while it runs, from hot reload or live link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Document {
    /// The day's timeline (`default_day.ron`, or the look-dev day with `--look-dev`): it replaces
    /// time of day's base timeline, if it comes from the file of the day the scene runs on
    /// ([`Scene::replace_day`]), parses and is complete.
    Day,
    /// The character light rig presets (`default_light_rigs.ron`, or the look-dev stage's with
    /// `--look-dev`): they replace the scene's library, if they come from the file of the presets
    /// the scene runs on ([`Scene::replace_light_rigs`]), parse and are valid; characters keep
    /// their preset IDs.
    LightRigs,
    /// The screen effect presets (`default_screen_effects.ron`, M1 P3): they replace the scene's
    /// library, if they parse and are valid.
    ScreenEffects,
}

impl Document {
    const ALL: [Self; 3] = [Self::Day, Self::LightRigs, Self::ScreenEffects];

    /// The document whose envelope names `type_name`, if the sandbox has one.
    pub fn of_type(type_name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|document| document.type_name() == type_name)
    }

    fn type_name(self) -> &'static str {
        match self {
            Self::Day => stable_name::<TodTimeline>(),
            Self::LightRigs => stable_name::<LightRigLibrary>(),
            Self::ScreenEffects => stable_name::<EffectLibrary>(),
        }
    }

    /// What it is, in messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::LightRigs => "light rigs",
            Self::ScreenEffects => "screen effects",
        }
    }

    /// Parses `text`, read from `origin` (a path or a file name), and swaps it into `scene`,
    /// returning what it replaced, or says why it keeps what it has. A timeline replaces the day
    /// only from the file of the day the scene runs on, so the default day can't replace the
    /// look-dev stage's ([`Scene::replace_day`]). New screen effect presets have `renderer`
    /// prewarm their post variants, which the overlay may fire.
    pub fn apply(
        self,
        origin: &str,
        text: &str,
        scene: &mut Scene,
        renderer: &mut Renderer,
    ) -> Result<&'static str, String> {
        match self {
            Self::Day => {
                let timeline = TodTimeline::from_ron(text).map_err(|error| error_chain(&error))?;
                Ok(scene.replace_day(origin, timeline)?.name())
            }
            Self::LightRigs => {
                let rigs = LightRigLibrary::from_ron(text).map_err(|error| error_chain(&error))?;
                scene.replace_light_rigs(origin, rigs)?;
                Ok(self.name())
            }
            Self::ScreenEffects => {
                let effects = EffectLibrary::from_ron(text).map_err(|error| error_chain(&error))?;
                scene.set_effect_library(effects);
                renderer.prewarm_post(scene.effect_variants());
                Ok(self.name())
            }
        }
    }
}

/// The timeline document the scene's day comes from ([`Day`]): the default day's, or the
/// look-dev day's with `--look-dev` (in the repository this build was compiled from, as `sources`
/// finds the engine's own).
pub(crate) fn day_source(options: &crate::options::Options) -> PathBuf {
    match options.look_dev {
        Some(_) => {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/lookdev").join(Day::LookDev.file_name())
        }
        None => sources::default_day(),
    }
}

/// The light rig presets' document the scene's characters are lit by: the shipped presets, or the
/// look-dev stage's own with `--look-dev` (M1 C8; in the repository this build was compiled from,
/// as `sources` finds the engine's own).
pub(crate) fn light_rigs_source(options: &crate::options::Options) -> PathBuf {
    match options.look_dev {
        Some(_) => Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/lookdev")
            .join(Day::LookDev.light_rigs_file_name()),
        None => sources::default_light_rigs(),
    }
}

/// Hot reload's state in the app.
pub(crate) struct Reload {
    hot: Option<HotReload>,
    /// What is watched, or why nothing is.
    watching: String,
    /// The watched documents, with their files' names.
    documents: Vec<(DocumentId, Document, String)>,
    /// The shader sources the renderer builds with since an edit replaced the compiled-in ones:
    /// a new renderer (a tier change, device loss) gets them too.
    shaders: Option<ShaderSources>,
    shader_rebuilds: u32,
    reloaded_documents: u32,
    /// The errors of the last round that changed something.
    errors: Vec<String>,
}

impl Reload {
    /// Watches the packs in `content_root`, the shaders, the day's timeline at `day`
    /// ([`day_source`]), the light rig presets at `light_rigs` ([`light_rigs_source`]) and the
    /// screen effect presets, unless `enabled` is false. A source that can't be watched is left
    /// out, with a warning.
    pub fn start(enabled: bool, content_root: &Path, day: &Path, light_rigs: &Path) -> Self {
        let mut reload = Self {
            hot: None,
            watching: String::new(),
            documents: Vec::new(),
            shaders: None,
            shader_rebuilds: 0,
            reloaded_documents: 0,
            errors: Vec::new(),
        };
        if !enabled {
            reload.watching = "off (--no-hot-reload)".to_owned();
            return reload;
        }
        let mut hot = match HotReload::new(DEBOUNCE) {
            Ok(hot) => hot,
            Err(error) => {
                tracing::info!(target: LOG_TARGET, %error, "hot reload off");
                reload.watching = format!("off: {error}");
                return reload;
            }
        };
        let mut watched = Vec::new();
        let warn = |what: &str, error: &dyn std::fmt::Display| {
            tracing::warn!(target: LOG_TARGET, %error, "hot reload can't watch {what}");
        };
        match hot.watch_packs(content_root) {
            Ok(()) => watched.push("packs"),
            Err(error) => warn("the content root", &error),
        }
        match hot.watch_shaders(&sources::shader_dir()) {
            Ok(()) => watched.push("shaders"),
            Err(error) => warn("the shaders", &error),
        }
        for (document, path, what) in [
            (Document::Day, day.to_owned(), "the day"),
            (Document::LightRigs, light_rigs.to_owned(), "the light rigs"),
            (Document::ScreenEffects, sources::default_screen_effects(), "the screen effects"),
        ] {
            match hot.watch_document(&path) {
                Ok(id) => {
                    // Its file's name, which says which day a timeline or the presets are for.
                    let origin =
                        path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                    reload.documents.push((id, document, origin));
                    watched.push(what);
                }
                Err(error) => warn(what, &error),
            }
        }
        reload.watching = format!("watching {}", watched.join(", "));
        tracing::info!(target: LOG_TARGET, content = %content_root.display(), "hot reload {}", reload.watching);
        reload.hot = Some(hot);
        reload
    }

    /// Applies what changed since the last frame; `changes` are the asset server's since then.
    /// Allocates nothing while nothing did.
    pub fn update(
        &mut self,
        changes: &[AssetChange],
        content: &mut Content,
        renderer: &mut Renderer,
        scene: &mut Scene,
    ) {
        let Some(hot) = &mut self.hot else { return };
        // Reloads land frames after their pack changed, so they are told of as they come: the
        // moment an edit shows.
        for change in changes {
            match change {
                AssetChange::Reloaded { id, kind, generation, .. } => {
                    tracing::info!(target: LOG_TARGET, %id, %kind, generation, "asset swapped in");
                }
                AssetChange::ReloadFailed { id, kind, error } => {
                    let error = format!("{kind} {id}: {}", error_chain(error));
                    tracing::warn!(target: LOG_TARGET, error, "hot reload");
                    self.errors.push(error);
                }
                AssetChange::Removed { .. } => {}
            }
        }
        let Changes { packs, shaders, documents, mut errors } = hot.poll();
        if packs.is_empty() && shaders.is_none() && documents.is_empty() && errors.is_empty() {
            return;
        }
        profiling::scope!("hot reload");
        // A pack the app never mounted is no concern of its.
        for pack in &packs {
            if !content.is_mounted(pack) {
                continue;
            }
            let stopwatch = Stopwatch::start();
            match content.remount(pack) {
                Ok(report) => {
                    tracing::info!(target: LOG_TARGET, pack, ?report, elapsed = ?stopwatch.elapsed(), "pack changed");
                }
                Err(error) => errors.push(format!("pack {pack:?}: {}", error_chain(&error))),
            }
        }
        if let Some(sources) = shaders {
            // On this frame's thread: the frame that applies an edit is longer by this much.
            let stopwatch = Stopwatch::start();
            match renderer.set_shader_sources(&sources) {
                Ok(update) => {
                    self.shader_rebuilds += 1;
                    tracing::info!(target: LOG_TARGET, ?update, elapsed = ?stopwatch.elapsed(), "shaders reloaded");
                }
                Err(error) => {
                    tracing::error!(target: LOG_TARGET, %error, "shaders not reloaded; keeping the last ones\n{}", shader_report(&error));
                    errors.push(format!("shaders: {error}"));
                }
            }
            self.remember_shaders(renderer);
        }
        for (id, text) in documents {
            let Some((_, document, origin)) = self.documents.iter().find(|(watched, ..)| *watched == id)
            else {
                continue;
            };
            match document.apply(origin, &text, scene, renderer) {
                Ok(replaced) => {
                    self.reloaded_documents += 1;
                    tracing::info!(target: LOG_TARGET, "{replaced} reloaded");
                }
                Err(error) => errors.push(format!("{origin}: {error}")),
            }
        }
        for error in &errors {
            tracing::warn!(target: LOG_TARGET, error, "hot reload");
        }
        self.errors = errors;
    }

    /// Keeps the shader sources `renderer` builds with now, after hot reload or live link gave it
    /// new ones, for a renderer made later.
    pub fn remember_shaders(&mut self, renderer: &Renderer) {
        let current = renderer.shader_sources();
        self.shaders = (!current.is_compiled_in()).then(|| current.clone());
    }

    /// Builds `renderer`'s shaders from the sources an edit gave the last one: for a renderer
    /// made after a tier change or device loss.
    pub fn renderer_created(&mut self, renderer: &mut Renderer) {
        if let Some(sources) = &self.shaders
            && let Err(error) = renderer.set_shader_sources(sources)
        {
            tracing::error!(target: LOG_TARGET, %error, "the new renderer keeps the compiled-in shaders");
        }
    }

    /// What the overlay shows.
    pub fn status(&self) -> HotReloadStatus<'_> {
        HotReloadStatus {
            watching: &self.watching,
            shader_rebuilds: self.shader_rebuilds,
            documents: self.reloaded_documents,
            errors: &self.errors,
        }
    }
}

/// naga's report of a shader error, with the source lines it points at: empty for the
/// pipelines wgpu refused.
pub(crate) fn shader_report(error: &ShaderError) -> &str {
    match error {
        ShaderError::Invalid { report, .. } => report,
        ShaderError::Pipeline { .. } => "",
    }
}

/// An error and its sources, as one line.
pub(crate) fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
