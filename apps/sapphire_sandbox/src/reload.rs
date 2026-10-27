//! Hot reload in `dev` builds (M1 A8, `sapphire_hot_reload`): the content root's packs, the
//! renderer's WGSL sources, the day's timeline (the default day's, or with `--look-dev` the
//! look-dev day's, `content/lookdev/lookdev_day.ron`), the character light rig presets and the
//! screen effect presets are watched, and each frame, before loading runs, what changed is
//! applied:
//!
//! - a rewritten pack (`cargo xtask content` after an edit) is remounted: the asset server
//!   reloads the assets that changed behind their handles, and [`Content`] uploads the new
//!   versions within the frame's upload budget, in place;
//! - WGSL files whose text changed on disk replace those files in the newest sources the
//!   renderer was asked for, so live link's pushes of other files keep their text, and build the
//!   renderer's pipelines again off the frame thread, which swaps them in once they are all
//!   built, or keeps the ones it has if they don't compile (`shader_edits.rs`);
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
//! [`Document`]s and [`ShaderEdits`].

use std::path::{Path, PathBuf};

use sapphire_assets::AssetChange;
use sapphire_debug::HotReloadStatus;
use sapphire_hot_reload::{Changes, DEBOUNCE, DocumentId, HotReload, sources};
use sapphire_reflect::stable_name;
use sapphire_render::{Renderer, ShaderFile, ShaderSources};
use sapphire_time::Stopwatch;
use sapphire_world::light_rig::LightRigLibrary;
use sapphire_world::screen_effects::EffectLibrary;
use sapphire_world::tod::TodTimeline;

use crate::LOG_TARGET;
use crate::content::Content;
use crate::scene::{Day, Scene};
use crate::shader_edits::{Answered, Origin, ShaderEdits};

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
    /// The shader files as hot reload last read them: compiled in until one changes on disk, as
    /// `HotReload` starts.
    shaders_read: ShaderSources,
    /// Edits of the shaders swapped in.
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
            shaders_read: ShaderSources::compiled_in(),
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

    /// Takes in what became of the shader edits it asked for: those swapped in, and the errors
    /// of those that failed, which replace the last change's.
    pub fn shaders_answered(&mut self, answered: &mut Answered) {
        self.shader_rebuilds += answered.reloaded;
        if !answered.reload_errors.is_empty() {
            self.errors = std::mem::take(&mut answered.reload_errors);
        }
    }

    /// Applies what changed since the last frame; `changes` are the asset server's since then.
    /// New shader sources go to the renderer through `shaders`, which answers them once they are
    /// swapped in. Allocates nothing while nothing changed.
    pub fn update(
        &mut self,
        changes: &[AssetChange],
        content: &mut Content,
        renderer: &mut Renderer,
        scene: &mut Scene,
        shaders: &mut ShaderEdits,
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
        let Changes { packs, shaders: new_shaders, documents, mut errors } = hot.poll();
        if packs.is_empty() && new_shaders.is_none() && documents.is_empty() && errors.is_empty() {
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
        // Built off the frame thread: `shaders` answers once they show, or with their errors.
        if let Some(sources) = new_shaders {
            self.request_shaders(sources, renderer, shaders);
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

    /// Asks `renderer`, through `shaders`, to build the files of `read` (every shader file, as
    /// hot reload read them from disk) that changed since the last reading, on top of the newest
    /// sources asked for (`ShaderEdits::base`). Taken whole, `read` would put back the disk's
    /// text of files live link pushed, and the push would still be answered as applied.
    pub(crate) fn request_shaders(
        &mut self,
        read: ShaderSources,
        renderer: &mut Renderer,
        shaders: &mut ShaderEdits,
    ) {
        let mut sources = None;
        for file in ShaderFile::ALL {
            if read.get(file) != self.shaders_read.get(file) {
                sources.get_or_insert_with(|| shaders.base(renderer)).set(file, read.get(file).to_owned());
            }
        }
        self.shaders_read = read;
        // A file saved with the text it had changes nothing.
        if let Some(sources) = sources {
            shaders.request(renderer, sources, Origin::HotReload);
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
