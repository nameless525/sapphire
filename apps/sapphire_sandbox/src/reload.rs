//! Hot reload in `dev` builds (M1 A8, `sapphire_hot_reload`): the content root's packs, the
//! renderer's WGSL sources, the day's timeline (the default day's, or with `--look-dev` the
//! look-dev day's, `content/lookdev/lookdev_day.ron`), the character light rig presets and the
//! screen effect presets are watched, and each frame, before loading runs, what changed is
//! applied:
//!
//! - a rewritten pack (`cargo xtask content` after an edit) is remounted: the asset server
//!   reloads the assets that changed behind their handles, and [`Content`](sapphire_stage::content::Content) uploads the new
//!   versions within the frame's upload budget, in place, and the scene poses what a replaced mesh
//!   starts over ([`Scene::follow_content`](sapphire_stage::scene::Scene::follow_content)). A pack live link pushed is read from the push until
//!   another push or a restart, so the file is left alone, and the log and the overlay say its
//!   new version isn't in use;
//! - WGSL files whose text changed on disk since hot reload's last swap replace those files in
//!   the newest sources the renderer was asked for, so live link's pushes of other files keep
//!   their text, and build the renderer's pipelines again off the frame thread, which swaps them
//!   in once they are all built, or keeps the ones it has if they don't compile, and they are
//!   built again with the next save (`shader_edits.rs`);
//! - an edited timeline replaces time of day's base timeline if it is the file of the day the
//!   scene runs on ([`Scene::replace_day`](sapphire_stage::scene::Scene::replace_day));
//! - edited presets replace the scene's (the `LightRigLibrary` resource) if they are the file of
//!   the presets the scene runs on (the shipped ones, or with `--look-dev` the look-dev stage's,
//!   `content/lookdev/lookdev_light_rigs.ron`; [`Scene::replace_light_rigs`](sapphire_stage::scene::Scene::replace_light_rigs)); characters keep
//!   their preset IDs;
//! - edited screen effect presets replace the scene's (the `EffectLibrary` resource, M1 P3);
//!   effects playing keep the layers they were pushed with, and the renderer prewarms the post
//!   variants the new presets draw through, unless they lose a preset a fight's cue table pushes;
//! - an edited fight's document (M2 T2, `tuning.rs`): its moves and fighter, the camera's tuning, a
//!   fighter's graph or the cue table (M2 E3, if the names it holds are all there), each playing on the
//!   next attempt. A new `.smoves` from a pack does the same.
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
use sapphire_render::{Renderer, ShaderFile, ShaderSources};
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::link::cook_not_in_use;
use crate::shader_edits::{Answered, Origin, ShaderEdits};
use sapphire_stage::scene::Day;
use sapphire_stage::tuning::{Document, DocumentFrame, error_chain};

/// The timeline document the scene's day comes from ([`Day`]): the default day's, or the
/// look-dev day's with `--look-dev` (in the repository this build was compiled from, as `sources`
/// finds the engine's own).
pub(crate) fn day_source(options: &crate::options::Options) -> PathBuf {
    if options.in_town() {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/lookdev").join(Day::LookDev.file_name())
    } else {
        sources::default_day()
    }
}

/// The light rig presets' document the scene's characters are lit by: the shipped presets, or the
/// look-dev stage's own with `--look-dev` (M1 C8; in the repository this build was compiled from,
/// as `sources` finds the engine's own).
pub(crate) fn light_rigs_source(options: &crate::options::Options) -> PathBuf {
    if options.in_town() {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/lookdev")
            .join(Day::LookDev.light_rigs_file_name())
    } else {
        sources::default_light_rigs()
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
    /// The shader files whose text changed on disk since one of hot reload's requests was last
    /// swapped in: every request takes all of them from the disk.
    shaders_changed: Vec<ShaderFile>,
    /// Edits of the shaders swapped in.
    shader_rebuilds: u32,
    reloaded_documents: u32,
    /// The errors of the last round that changed something.
    errors: Vec<String>,
}

impl Reload {
    /// Watches the packs in `content_root`, the shaders, the day's timeline at `day`
    /// ([`day_source`]), the light rig presets at `light_rigs` ([`light_rigs_source`]), the
    /// screen effect presets and the fight's documents `tuned` ([`LiveTuning::watched`](sapphire_stage::tuning::LiveTuning::watched)), unless
    /// `enabled` is false. A source that can't be watched is left out, with a warning.
    pub fn start(
        enabled: bool,
        content_root: &Path,
        day: &Path,
        light_rigs: &Path,
        tuned: Vec<(Document, PathBuf)>,
    ) -> Self {
        let mut reload = Self {
            hot: None,
            watching: String::new(),
            documents: Vec::new(),
            shaders_read: ShaderSources::compiled_in(),
            shaders_changed: Vec::new(),
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
        let engine = [
            (Document::Day, day.to_owned(), "the day"),
            (Document::LightRigs, light_rigs.to_owned(), "the light rigs"),
            (Document::ScreenEffects, sources::default_screen_effects(), "the screen effects"),
        ];
        let tuned = tuned.into_iter().map(|(document, path)| (document, path, document.name()));
        for (document, path, what) in engine.into_iter().chain(tuned) {
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
        watched.dedup();
        reload.watching = format!("watching {}", watched.join(", "));
        tracing::info!(target: LOG_TARGET, content = %content_root.display(), "hot reload {}", reload.watching);
        reload.hot = Some(hot);
        reload
    }

    /// Takes in what became of the shader edits it asked for: those swapped in, and the errors
    /// of those that failed, which replace the last change's.
    pub fn shaders_answered(&mut self, answered: &mut Answered) {
        self.shader_rebuilds += answered.reloaded;
        // Swapped in, so they are in use. Requests are made after answers are taken, and an answer
        // covers the requests before it, so none newer can be waiting here with other files.
        if answered.reloaded > 0 {
            self.shaders_changed.clear();
        }
        if !answered.reload_errors.is_empty() {
            self.errors = std::mem::take(&mut answered.reload_errors);
        }
    }

    /// Adds what a pack's new version of something went wrong with, which the overlay lists.
    pub fn add_errors(&mut self, errors: Vec<String>) {
        for error in &errors {
            tracing::warn!(target: LOG_TARGET, error, "hot reload");
        }
        self.errors.extend(errors);
    }

    /// Applies what changed since the last frame; `changes` are the asset server's since then.
    /// New shader sources go to the renderer through `shaders`, which answers them once they are
    /// swapped in. Allocates nothing while nothing changed.
    pub fn update(&mut self, changes: &[AssetChange], frame: DocumentFrame<'_>, shaders: &mut ShaderEdits) {
        let DocumentFrame { content, renderer, gpu, scene, fight, tuning } = frame;
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
        for pack in &packs {
            // Live link's push is what the app reads: a remount would find nothing changed.
            if let Some(warning) = cook_not_in_use(&content.live_packs(), pack) {
                errors.push(warning);
                continue;
            }
            // A pack the app never mounted is no concern of its.
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
            let frame = DocumentFrame {
                content: &mut *content,
                renderer: &mut *renderer,
                gpu,
                scene: &mut *scene,
                fight: &mut *fight,
                tuning: &mut *tuning,
            };
            match document.apply(origin, &text, frame) {
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
    /// hot reload read them from disk) that changed since one of hot reload's requests was last
    /// swapped in, on top of the newest sources asked for (`ShaderEdits::base`). A failed
    /// request's files stay among them, so an edit across files saved a build apart lands whole
    /// with its last save (review DV-1). Taken whole, `read` would put back the disk's text of
    /// files live link pushed, and the push would still be answered as applied.
    pub(crate) fn request_shaders(
        &mut self,
        read: ShaderSources,
        renderer: &mut Renderer,
        shaders: &mut ShaderEdits,
    ) {
        for file in ShaderFile::ALL {
            if read.get(file) != self.shaders_read.get(file) && !self.shaders_changed.contains(&file) {
                self.shaders_changed.push(file);
            }
        }
        self.shaders_read = read;
        // A file saved with the text it had changes nothing, but tries a failed edit again.
        if self.shaders_changed.is_empty() {
            return;
        }
        let mut sources = shaders.base(renderer);
        for &file in &self.shaders_changed {
            sources.set(file, self.shaders_read.get(file).to_owned());
        }
        shaders.request(renderer, sources, Origin::HotReload);
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
