//! Desktop hot reload for development builds (spec §5.10 and §5.12, M1 A8): watches the files
//! a running app was loaded and built from, and reports what changed, debounced, so the app can
//! swap it in without a restart.
//!
//! A `dev_only` crate (`xtask/layers.toml`): apps reach it through their `dev` feature (rule
//! R8), so shipping builds never link it. Its file watching, `notify` 8.2, comes in through its
//! `notify` feature (the backend, rule R8), which apps turn on for desktop targets only: phones
//! get changes pushed by live link instead (A9), and build none of it. Without the backend,
//! [`HotReload::new`] says so and nothing is watched.
//!
//! # Owns
//!
//! - [`FileWatcher`]: directories watched for changed files, debounced on a thread of its own.
//!   The debounce is trailing-edge: a burst is reported once its files have been quiet for the
//!   debounce time, however long it lasted. `notify-debouncer-full` was the alternative: it
//!   reports each event once that event is older than the timeout, found by a thread waking
//!   four times per timeout, so a burst longer than that splits into several reports, and idle
//!   it wakes 40 times a second at 100 ms. Its rename stitching isn't needed, since only the
//!   names that changed matter, and a rename's event carries both.
//! - [`HotReload`], the engine's dev loop: the content root's packs ([`HotReload::watch_packs`]),
//!   the renderer's WGSL sources ([`HotReload::watch_shaders`]) and documents compiled into the
//!   engine or the app ([`HotReload::watch_document`]), reported as packs to remount, shader
//!   sources to build and documents' new text ([`Changes`]).
//! - Where the engine's own sources are in its repository ([`sources`]).
//!
//! # Does not own
//!
//! - Applying changes: the asset server remounts a pack and swaps what changed behind its
//!   handles (`sapphire_assets::AssetServer::remount`); the renderer rebuilds pipelines from
//!   shader sources (`sapphire_render::Renderer::set_shader_sources`); the app parses documents
//!   and hands them to their owners (time of day's `TimeOfDay::set_timeline`, the light rig
//!   presets' `LightRigLibrary` resource, the screen effect presets' `EffectLibrary` resource).
//!   Live link (A9)
//!   drives the same three from a device's side.
//! - Cooking: `cargo xtask content` writes packs, and hot reload sees them land.
//!
//! # Packs, not loose files
//!
//! A cook writes loose cooked files (`target/content/loose/<cook>/`) and then the packs apps read
//! (`target/content/<cook>/`, the content root). Hot reload watches the packs:
//!
//! - **One consistent version.** A pack is written whole, to a temporary file renamed into place,
//!   after every asset of the cook: an asset and its dependencies always come from the same cook.
//!   Loose files land one by one, so a material could be seen before the texture it now names.
//! - **The same path as loading.** A remount reads the new table of contents and reloads what
//!   its blake3 hashes say changed, through the IO thread, decode jobs, the dependency order and
//!   the upload budget every load takes, by asset ID alone (rule R5). Loose files would need an
//!   index from paths to IDs and kinds that the runtime otherwise never has.
//! - **The same path as live link.** A phone has no cook directory to watch; live link pushes
//!   packs, and the device remounts them the same way.
//!
//! An unchanged pack (a cook that changed nothing rewrites nothing) costs one header read.
//!
//! # Cost
//!
//! Idle, the watcher's threads (the platform's, and the debounce thread) block, and
//! [`HotReload::poll`] costs one empty channel read per frame, allocating nothing. The sandbox's
//! bench measures it (spec §5.10's M1 A8 note).
//!
//! # Later milestones
//!
//! - A pipeline watch mode that cooks on save, so editing a source reaches the running app with
//!   no command in between (today `cargo xtask content` runs after the edit).
//! - The quality tier table (`tiers.ron`) and the sandbox's bindings as watched documents:
//!   `sapphire_render::quality::Quality` must keep its device profile to select a tier from a
//!   new table, and a changed tier makes a new renderer, as an `r.tier` change does.

mod watch;

pub mod sources;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sapphire_assets::pack::PACK_EXTENSION;
use sapphire_render::{ShaderFile, ShaderSources};

pub use watch::{Changed, FileWatcher, WatchError, WatchEvents, WatchId};

/// Log target for this crate.
pub(crate) const LOG_TARGET: &str = "sapphire::hot_reload";

/// How long a file must be quiet before its change is reported: long enough to take an editor's
/// save or a cook's write and rename as one change, short enough to feel immediate.
pub const DEBOUNCE: Duration = Duration::from_millis(100);

/// A document watched with [`HotReload::watch_document`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentId(usize);

/// What changed since the last [`HotReload::poll`].
#[derive(Debug, Default)]
pub struct Changes {
    /// Packs to remount, by name: `<name>.spack` in the content root changed (rewritten, renamed
    /// into place or removed).
    pub packs: Vec<String>,
    /// The renderer's shader sources as they are now, when a WGSL file changed: every file's
    /// text, read again, a file that couldn't be read keeping the text it had.
    pub shaders: Option<ShaderSources>,
    /// Watched documents that changed, with their new text.
    pub documents: Vec<(DocumentId, String)>,
    /// What couldn't be read or watched.
    pub errors: Vec<String>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.packs.is_empty() && self.shaders.is_none() && self.documents.is_empty() && self.errors.is_empty()
    }
}

/// The engine's development hot reload. See the [crate docs](crate).
#[derive(Debug)]
pub struct HotReload {
    watcher: FileWatcher,
    packs: Option<WatchId>,
    shaders: Option<WatchId>,
    /// Each watched document's directory, file name and path, by [`DocumentId`].
    documents: Vec<(WatchId, OsString, PathBuf)>,
    /// The shader sources as last read: the compiled-in ones until a WGSL file changes.
    sources: ShaderSources,
}

impl HotReload {
    /// Hot reload watching nothing yet, reporting a file once it has been quiet for `debounce`
    /// ([`DEBOUNCE`] suits the dev loop). Fails in builds without the file-watching backend.
    pub fn new(debounce: Duration) -> Result<Self, WatchError> {
        Ok(Self {
            watcher: FileWatcher::new(debounce)?,
            packs: None,
            shaders: None,
            documents: Vec::new(),
            sources: ShaderSources::compiled_in(),
        })
    }

    /// Watches the content root's packs, `<name>.spack` files, reporting a changed one as a pack
    /// to remount.
    pub fn watch_packs(&mut self, content_root: &Path) -> Result<(), WatchError> {
        self.packs = Some(self.watcher.watch(content_root)?);
        Ok(())
    }

    /// Watches the renderer's WGSL sources in `dir` ([`sources::shader_dir`] in this
    /// repository), reporting the sources again when one changes.
    pub fn watch_shaders(&mut self, dir: &Path) -> Result<(), WatchError> {
        self.shaders = Some(self.watcher.watch(dir)?);
        Ok(())
    }

    /// Watches the document at `path`, reporting its text when it changes: a document compiled
    /// in, whose source development builds edit (the default day's timeline,
    /// [`sources::default_day`], the character light rig presets,
    /// [`sources::default_light_rigs`], and the screen effect presets,
    /// [`sources::default_screen_effects`]).
    pub fn watch_document(&mut self, path: &Path) -> Result<DocumentId, WatchError> {
        let missing = || WatchError::Io {
            path: path.to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a file in a directory"),
        };
        let (dir, name) = (path.parent().ok_or_else(missing)?, path.file_name().ok_or_else(missing)?);
        let watch = self.watcher.watch(dir)?;
        self.documents.push((watch, name.to_owned(), path.to_owned()));
        Ok(DocumentId(self.documents.len() - 1))
    }

    /// What changed since the last poll. Call it once a frame: it allocates nothing while nothing
    /// changed. Changed shader files and documents are read here.
    pub fn poll(&mut self) -> Changes {
        let mut changes = Changes::default();
        let events = self.watcher.poll();
        changes.errors.extend(events.errors.iter().map(|error| format!("file watching: {error}")));
        let mut shaders = false;
        for Changed { watch, name } in events.changed {
            let name = Path::new(name);
            // `<name>.spack`; the temporary files packs are written to end otherwise.
            let pack = name.extension().is_some_and(|extension| extension == PACK_EXTENSION);
            if Some(*watch) == self.packs
                && pack
                && let Some(pack) = name.file_stem().and_then(|stem| stem.to_str())
                && !changes.packs.iter().any(|known| known == pack)
            {
                changes.packs.push(pack.to_owned());
            }
            if Some(*watch) == self.shaders && name.to_str().and_then(ShaderFile::from_file_name).is_some() {
                shaders = true;
            }
            for (index, (_, _, path)) in self
                .documents
                .iter()
                .enumerate()
                .filter(|(_, (dir, file, _))| dir == watch && file.as_os_str() == name.as_os_str())
            {
                match std::fs::read_to_string(path) {
                    Ok(text) => changes.documents.push((DocumentId(index), text)),
                    Err(error) => changes.errors.push(format!("{}: {error}", path.display())),
                }
            }
        }
        if let Some(watch) = self.shaders.filter(|_| shaders) {
            let dir = self.watcher.dir(watch).to_owned();
            for file in ShaderFile::ALL {
                let path = dir.join(file.file_name());
                match std::fs::read_to_string(&path) {
                    Ok(text) if text != self.sources.get(file) => self.sources.set(file, text),
                    Ok(_) => {}
                    Err(error) => changes.errors.push(format!("{}: {error}", path.display())),
                }
            }
            changes.shaders = Some(self.sources.clone());
        }
        changes
    }
}

#[cfg(all(test, feature = "notify"))]
mod tests;

#[cfg(all(test, not(feature = "notify")))]
#[test]
fn builds_without_the_backend_watch_nothing() {
    assert!(matches!(HotReload::new(DEBOUNCE), Err(WatchError::Unsupported)));
}
