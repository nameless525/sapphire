//! Counting panics on every thread, for the tests that lose a device on purpose: the warm-up's job scope
//! catches a panic and drops the part, so nothing fails, but the `shipping` profile aborts on any panic, so
//! for a shipped build each one is a crash.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts every panic, on any thread, from its making until it's dropped; the default hook still prints each.
/// Panic hooks are the process's: hold one only in a test run alone (`--test-threads=1`, as GPU tests are).
pub(super) struct Panics {
    count: Arc<AtomicUsize>,
}

impl Panics {
    pub(super) fn count() -> Self {
        let count = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&count);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            seen.fetch_add(1, Ordering::SeqCst);
            previous(info);
        }));
        Self { count }
    }

    /// Panics so far.
    pub(super) fn so_far(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

impl Drop for Panics {
    /// Puts the default hook back, unless the test is already failing by a panic, which can't change hooks.
    fn drop(&mut self) {
        if !std::thread::panicking() {
            drop(std::panic::take_hook());
        }
    }
}
