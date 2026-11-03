//! Settings and progress on disk (spec §5.25, M3 V1).
//!
//! - **Owns:** [`SettingsFile`], a RON file that always gives the game a value, and
//!   [`ProgressStore`], two rotating slots of `postcard` with a checksum and a generation.
//! - **Does not own:** the formats (`sapphire_serialize`), the schema chain (a game's
//!   [`Versioned`](sapphire_reflect::Versioned) types), where the directory is
//!   (`Platform::user_dir`, which an app passes in), or what a save holds. By m3-plan §3 that's
//!   what a zone start reads, never a fight in progress: the type is the game's, and this crate
//!   is generic over it.
//!
//! # How a file reaches the disk
//!
//! Every write goes through a function the caller gives: an app passes
//! `sapphire_platform::durable_replace`, whose docs say where each OS's guarantee stops. The crate
//! doesn't link the platform itself, because the replay runner may not (`[simulation]` in
//! `xtask/layers.toml`), and a replay can start from a named save (m3-plan §3).
//!
//! # Off the frame's path
//!
//! A write can stall on a phone's flash for far longer than a frame, so serializing and writing
//! are separate calls. Take a [`Snapshot`], plain bytes, on the frame's thread, and write it on a
//! job. Keep one write in flight: two queued jobs could run out of order, and the older save would
//! win. If a save is asked for during a write, hold the newest snapshot until the write reports
//! back. A load reads whole files, and a settings load may repair one, so it belongs at startup or
//! on a job too.
//!
//! ```no_run
//! # use bevy_reflect::TypePath;
//! # use sapphire_reflect::Versioned;
//! # use sapphire_save::ProgressStore;
//! # #[derive(TypePath, serde::Serialize, serde::Deserialize)]
//! # struct ZoneStart;
//! # impl Versioned for ZoneStart { const VERSION: u32 = 1; }
//! # let (user_dir, zone_start) = ("user", ZoneStart);
//! let store = ProgressStore::<ZoneStart>::new(user_dir, "progress", sapphire_platform::durable_replace);
//! let snapshot = store.snapshot(&zone_start)?;
//! let (report, written) = std::sync::mpsc::channel();
//! sapphire_jobs::spawn_async(move || {
//!     let _ = report.send(store.write(&snapshot));
//! });
//! // Later frames poll `written.try_recv()`, and tell the player if a save failed.
//! # Ok::<(), sapphire_serialize::SaveError>(())
//! ```
//!
//! # What's left on disk
//!
//! In the directory given: `<name>.ron` for settings, `<name>.a.sav` and `<name>.b.sav` for
//! progress, `<file>.tmp` if a write died, `<name>.lock` so that a second copy of the game can't
//! write at the same moment, and up to [`KEPT_ASIDE`] copies of damaged files as
//! `<file>.bad.<n>`, the newest kept.

mod aside;
mod lock;
mod progress;
mod salvage;
mod settings;
mod slot;

use std::io;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;

pub use aside::KEPT_ASIDE;
pub use progress::{LoadError, LoadedSave, NewerSave, OtherSlot, ProgressStore, WriteError, Written};
pub use settings::{LoadedSettings, SettingsFile, SettingsOutcome};
pub use slot::{Slot, SlotFault};

const LOG_TARGET: &str = "sapphire::save";

/// How a store puts a file on the disk, whole, from what the caller gave it.
type Replace = Arc<dyn Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync>;

/// A value of `T` serialized into plain bytes, ready to go to a job and be written there. It holds
/// no reference to the game's state, so the frame goes on while it's written.
pub struct Snapshot<T> {
    bytes: Vec<u8>,
    of: PhantomData<fn() -> T>,
}

impl<T> Snapshot<T> {
    fn new(bytes: Vec<u8>) -> Self {
        Self { bytes, of: PhantomData }
    }

    /// The serialized bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl<T> std::fmt::Debug for Snapshot<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot").field("len", &self.bytes.len()).finish()
    }
}

/// Checks that a store's name is a plain file stem, so no game's save can land outside its
/// directory.
fn check_name(name: &str) {
    assert!(
        !name.is_empty() && !name.contains(['/', '\\', ':']) && !name.starts_with('.'),
        "a save's name is a plain file stem, not {name:?}"
    );
}

#[cfg(test)]
mod test_support;
