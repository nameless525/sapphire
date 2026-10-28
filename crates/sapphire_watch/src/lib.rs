//! File watching and repository source paths for development tools.
//!
//! The watcher owns per-file trailing-edge debounce and bounded delivery during continuous
//! writes. It knows paths and file events, not renderers, shader contents, asset formats or
//! document types. Hot reload interprets those events for an app; live link uses them to push
//! edited files. Keeping this boundary independent lets the live-link tool stay built while
//! the shaders and engine code it sends are edited.
//!
//! This is a dev-only crate (rule R8). Desktop apps enable the `notify` backend; without it,
//! [`FileWatcher::new`] returns [`WatchError::Unsupported`] and no watcher thread is created.

mod watch;

pub mod sources;

use std::time::Duration;

pub use watch::{Changed, FileWatcher, WatchError, WatchEvents, WatchId};

// Keep existing log filters effective after moving the watcher out of hot reload.
pub(crate) const LOG_TARGET: &str = "sapphire::hot_reload";

/// How long a file must be quiet before its change is reported: long enough to coalesce an
/// editor's save or a cook's write and rename, short enough to feel immediate. A file written
/// without pause waits two seconds at most ([`FileWatcher`]).
pub const DEBOUNCE: Duration = Duration::from_millis(100);

#[cfg(all(test, feature = "notify"))]
mod tests;

#[cfg(all(test, not(feature = "notify")))]
#[test]
fn builds_without_the_backend_watch_nothing() {
    assert!(matches!(FileWatcher::new(DEBOUNCE), Err(WatchError::Unsupported)));
}
