//! Hot reload in `dev` builds (M1 A8, `sapphire_hot_reload`): the content root's packs, the
//! renderer's WGSL sources, the default day's timeline and the character light rig presets are
//! watched, and each frame, before loading runs, what changed is applied:
//!
//! - a rewritten pack (`cargo xtask content` after an edit) is remounted: the asset server
//!   reloads the assets that changed behind their handles, and [`Content`] uploads the new
//!   versions within the frame's upload budget, in place;
//! - new shader sources rebuild the renderer's pipelines from the next frame, or keep the ones it
//!   has if they don't compile;
//! - an edited timeline replaces time of day's base timeline;
//! - edited presets replace the scene's (the `LightRigLibrary` resource); characters keep their
//!   preset IDs.
//!
//! On by default in `dev` builds; `--no-hot-reload` turns it off. Phones build no file watching
//! (live link, A9, is their trigger), so it reports itself off there. Errors are logged and
//! shown in the debug overlay's stats panel; whatever failed keeps what it had.

use std::path::Path;

use sapphire_assets::AssetChange;
use sapphire_debug::HotReloadStatus;
use sapphire_hot_reload::{Changes, DEBOUNCE, DocumentId, HotReload, sources};
use sapphire_render::{Renderer, ShaderError, ShaderSources};
use sapphire_time::Stopwatch;
use sapphire_world::light_rig::LightRigLibrary;
use sapphire_world::tod::TodTimeline;

use crate::LOG_TARGET;
use crate::content::Content;
use crate::scene::Scene;

/// Hot reload's state in the app.
pub(crate) struct Reload {
    hot: Option<HotReload>,
    /// What is watched, or why nothing is.
    watching: String,
    day: Option<DocumentId>,
    rigs: Option<DocumentId>,
    /// The shader sources the renderer builds with since an edit replaced the compiled-in ones:
    /// a new renderer (a tier change, device loss) gets them too.
    shaders: Option<ShaderSources>,
    shader_rebuilds: u32,
    documents: u32,
    /// The errors of the last round that changed something.
    errors: Vec<String>,
}

impl Reload {
    /// Watches the packs in `content_root`, the shaders, the default day and the light rig
    /// presets, unless `enabled` is false. A source that can't be watched is left out, with a
    /// warning.
    pub fn start(enabled: bool, content_root: &Path) -> Self {
        let mut reload = Self {
            hot: None,
            watching: String::new(),
            day: None,
            rigs: None,
            shaders: None,
            shader_rebuilds: 0,
            documents: 0,
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
        match hot.watch_document(&sources::default_day()) {
            Ok(day) => {
                reload.day = Some(day);
                watched.push("the default day");
            }
            Err(error) => warn("the default day", &error),
        }
        match hot.watch_document(&sources::default_light_rigs()) {
            Ok(rigs) => {
                reload.rigs = Some(rigs);
                watched.push("the light rigs");
            }
            Err(error) => warn("the light rigs", &error),
        }
        reload.watching = format!("watching {}", watched.join(", "));
        tracing::info!(target: LOG_TARGET, content = %content_root.display(), "hot reload {}", reload.watching);
        reload.hot = Some(hot);
        reload
    }

    /// Applies what changed since the last frame. Allocates nothing while nothing did.
    pub fn update(&mut self, content: &mut Content, renderer: &mut Renderer, scene: &mut Scene) {
        let Some(hot) = &mut self.hot else { return };
        // Reloads land frames after their pack changed, so their failures show as they come.
        for change in content.take_changes() {
            if let AssetChange::ReloadFailed { id, kind, error } = change {
                let error = format!("{kind} {id}: {}", error_chain(&error));
                tracing::warn!(target: LOG_TARGET, error, "hot reload");
                self.errors.push(error);
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
                    let report = match &error {
                        ShaderError::Invalid { report, .. } => report.as_str(),
                        ShaderError::Pipeline { .. } => "",
                    };
                    tracing::error!(target: LOG_TARGET, %error, "shaders not reloaded; keeping the last ones\n{report}");
                    errors.push(format!("shaders: {error}"));
                }
            }
            // What the renderer builds with now, for the next one.
            let current = renderer.shader_sources();
            self.shaders = (!current.is_compiled_in()).then(|| current.clone());
        }
        for (document, text) in documents {
            if Some(document) == self.day {
                match TodTimeline::from_ron(&text).map_err(|error| error_chain(&error)).and_then(|timeline| {
                    scene.time_of_day_mut().set_timeline(timeline).map_err(|error| error.to_string())
                }) {
                    Ok(()) => {
                        self.documents += 1;
                        tracing::info!(target: LOG_TARGET, "default day reloaded");
                    }
                    Err(error) => errors.push(format!("default day: {error}")),
                }
            } else if Some(document) == self.rigs {
                match LightRigLibrary::from_ron(&text) {
                    Ok(rigs) => {
                        scene.set_light_rigs(rigs);
                        self.documents += 1;
                        tracing::info!(target: LOG_TARGET, "light rigs reloaded");
                    }
                    Err(error) => errors.push(format!("light rigs: {}", error_chain(&error))),
                }
            }
        }
        for error in &errors {
            tracing::warn!(target: LOG_TARGET, error, "hot reload");
        }
        self.errors = errors;
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
            documents: self.documents,
            errors: &self.errors,
        }
    }
}

/// An error and its sources, as one line.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
