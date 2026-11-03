//! The lock around a write, so a second copy of the game, or a second job in this one, waits its
//! turn rather than sharing a temp file.

use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::LOG_TARGET;

/// Held for one write. The OS lets go of it when the file closes, a killed process's included, so
/// the file that stays behind means nothing.
pub(crate) struct WriteLock {
    _file: Option<File>,
}

impl WriteLock {
    /// Waits for `path`'s lock. A filesystem that can't lock isn't a reason to lose a save: the
    /// failure is logged, and a checksum still catches a write torn by two at once.
    pub(crate) fn take(path: &Path) -> Self {
        let locked = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .and_then(|file| file.lock().map(|()| file));
        match locked {
            Ok(file) => Self { _file: Some(file) },
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, path = %path.display(), %error, "writing without a lock");
                Self { _file: None }
            }
        }
    }
}
