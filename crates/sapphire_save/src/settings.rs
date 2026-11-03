//! Settings: one RON file that always gives the game a value.

use std::fs;
use std::io;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sapphire_reflect::{TypePath, Versioned, stable_name};
use sapphire_serialize::{DocumentError, from_ron, to_ron};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::aside::keep_aside;
use crate::lock::WriteLock;
use crate::salvage::{self, Entry};
use crate::{LOG_TARGET, Replace, Snapshot, check_name};

/// A game's settings: `<name>.ron` in a `sapphire_serialize` document, written whole.
///
/// A player edits this file, a build changes under it, and neither may cost them all their
/// settings or stop the game. So [`load`](Self::load) can't fail:
///
/// | The file | What the game gets |
/// |---|---|
/// | Is missing | `T::default()` |
/// | Parses, at this version or an older one | Its value, upgraded by the type's [`Versioned`] chain |
/// | Has a field this version doesn't know | Its value, with that field ignored, and gone at the next write. Settings types don't deny unknown fields, unlike authored documents: a field a newer build wrote mustn't cost a downgrade the rest |
/// | Lacks a field | The field's default, if the type says `#[serde(default)]`; otherwise as for a line that won't parse |
/// | Won't parse: a bad line, a wrong type, a cut-off file, garbage, or a version newer than this build's | Every field that can still be read, and defaults for the rest. The file is copied to `<name>.ron.bad.<n>` with a log line, and replaced by what was read, so the next launch starts clean |
///
/// Reading fields by name out of a damaged file can't run an older version's migration, so a
/// field that was renamed since keeps its default. A value that parses but makes no sense, a volume
/// of 900 say, is the game's to clamp.
///
/// A settings type should be a struct with named fields: nothing else has fields to read one at a
/// time, so a damaged file then gives the default.
pub struct SettingsFile<T> {
    path: PathBuf,
    lock_path: PathBuf,
    replace: Replace,
    of: PhantomData<fn() -> T>,
}

impl<T> Clone for SettingsFile<T> {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            lock_path: self.lock_path.clone(),
            replace: self.replace.clone(),
            of: PhantomData,
        }
    }
}

/// The settings a load gave, and how it found them.
#[derive(Debug)]
pub struct LoadedSettings<T> {
    pub value: T,
    pub outcome: SettingsOutcome,
}

/// How a load found the settings file.
#[derive(Debug)]
pub enum SettingsOutcome {
    /// There was no file: the value is the default.
    Missing,
    /// The file was whole.
    Read,
    /// The file wouldn't parse, and has been replaced by the fields that could be read.
    Repaired {
        /// Where the damaged file went, unless it couldn't be copied.
        kept_aside: Option<PathBuf>,
        /// What the parser said of the whole file.
        error: String,
        /// The entries that were found and couldn't be read, by name.
        dropped: Vec<String>,
    },
    /// The file couldn't be read at all, so it's left alone and the value is the default. A write
    /// will still replace it.
    Unreadable(io::Error),
}

impl<T: Versioned + Serialize + DeserializeOwned + TypePath + Default> SettingsFile<T> {
    /// The settings named `name` in `dir`. `replace` writes a file whole: an app passes
    /// `sapphire_platform::durable_replace`.
    ///
    /// # Panics
    ///
    /// If `name` isn't a plain file stem.
    pub fn new(
        dir: impl AsRef<Path>,
        name: &str,
        replace: impl Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        check_name(name);
        let dir = dir.as_ref();
        Self {
            path: dir.join(format!("{name}.ron")),
            lock_path: dir.join(format!("{name}.lock")),
            replace: Arc::new(replace),
            of: PhantomData,
        }
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the settings, repairing the file if it needs it. See the type's docs for every case.
    pub fn load(&self) -> LoadedSettings<T> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return LoadedSettings { value: T::default(), outcome: SettingsOutcome::Missing };
            }
            Err(error) => {
                tracing::error!(
                    target: LOG_TARGET,
                    file = %self.path.display(),
                    %error,
                    "couldn't read the settings; using the defaults"
                );
                return LoadedSettings { value: T::default(), outcome: SettingsOutcome::Unreadable(error) };
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        match from_ron::<T>(&text) {
            Ok(value) => LoadedSettings { value, outcome: SettingsOutcome::Read },
            Err(error) => self.repair(&text, &error.to_string()),
        }
    }

    /// Serializes `value` into plain bytes, for [`write`](Self::write) to put on disk later and
    /// elsewhere.
    pub fn snapshot(&self, value: &T) -> Result<Snapshot<T>, DocumentError> {
        to_ron(value).map(|text| Snapshot::new(text.into_bytes()))
    }

    /// Replaces the file. Whatever goes wrong, the old file or the new one is there whole. It can
    /// stall for as long as the storage does: see the crate docs.
    pub fn write(&self, snapshot: &Snapshot<T>) -> io::Result<()> {
        let _lock = WriteLock::take(&self.lock_path);
        (self.replace)(&self.path, &snapshot.bytes)
    }

    /// Takes every field `text` still holds, keeps the file aside and replaces it with them.
    fn repair(&self, text: &str, error: &str) -> LoadedSettings<T> {
        let (value, dropped) = salvage_fields::<T>(text);
        tracing::warn!(
            target: LOG_TARGET,
            file = %self.path.display(),
            error,
            ?dropped,
            "the settings wouldn't parse; kept what could be read"
        );
        let kept_aside = keep_aside(&self.path, error);
        // A failure here leaves the damaged file for the next launch to repair again, which only
        // costs another copy aside: this launch has its value either way.
        match self.snapshot(&value) {
            Ok(snapshot) => {
                if let Err(error) = self.write(&snapshot) {
                    tracing::warn!(target: LOG_TARGET, file = %self.path.display(), %error, "couldn't write the repaired settings");
                }
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, file = %self.path.display(), %error, "couldn't serialize the repaired settings");
            }
        }
        LoadedSettings {
            value,
            outcome: SettingsOutcome::Repaired { kept_aside, error: error.to_owned(), dropped },
        }
    }
}

/// The defaults with every entry of `text` laid over them that leaves a readable `T`, and the names
/// of those that didn't.
fn salvage_fields<T: Versioned + Serialize + DeserializeOwned + TypePath + Default>(
    text: &str,
) -> (T, Vec<String>) {
    let Ok(defaults_text) = to_ron(&T::default()) else {
        return (T::default(), Vec::new());
    };
    let mut merged: Vec<Entry<'_>> = salvage::data_entries(&defaults_text);
    let mut dropped = Vec::new();
    for entry in salvage::data_entries(text) {
        let previous = merged.iter().position(|kept| kept.name == entry.name);
        let mut trial = merged.clone();
        match previous {
            Some(at) => trial[at] = entry,
            None => trial.push(entry),
        }
        if from_ron::<T>(&document::<T>(&trial)).is_ok() {
            merged = trial;
        } else {
            dropped.push(entry.name.to_owned());
        }
    }
    (from_ron::<T>(&document::<T>(&merged)).unwrap_or_default(), dropped)
}

/// A whole document at this build's version, holding `entries`.
fn document<T: Versioned + TypePath>(entries: &[Entry<'_>]) -> String {
    use std::fmt::Write as _;

    let mut text = format!("(type: \"{}\", version: {}, data: (\n", stable_name::<T>(), T::VERSION);
    for entry in entries {
        // A line of its own, so that a comment at the value's end can't hide the comma.
        let _ = writeln!(text, "{}: {}\n,", entry.name, entry.value);
    }
    text.push_str("))");
    text
}

#[cfg(test)]
mod tests;
