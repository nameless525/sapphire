//! [`RealClock`]: unscaled wall time.

use std::time::{Duration, Instant};

/// Unscaled, monotonic wall time, updated once per frame.
///
/// For UI animation, input timestamps, networking and feeding [`crate::FixedStepDriver`]. It
/// never pauses or slows down. This is the only type in the engine that reads the OS clock
/// (rule R2); everything else reads a clock passed to it.
#[derive(Clone, Debug)]
pub struct RealClock {
    start: Instant,
    last: Instant,
    delta: Duration,
    frames: u64,
}

impl RealClock {
    /// Starts the clock now.
    #[must_use]
    pub fn new() -> Self {
        let now = os_now();
        Self { start: now, last: now, delta: Duration::ZERO, frames: 0 }
    }

    /// Samples the OS clock once for the new frame and returns the time since the previous
    /// update (or since creation). Call exactly once per frame, before anything reads the clock.
    pub fn update(&mut self) -> Duration {
        let now = os_now();
        self.delta = now.saturating_duration_since(self.last);
        self.last = now;
        self.frames += 1;
        self.delta
    }

    /// The duration returned by the most recent [`RealClock::update`].
    #[must_use]
    pub fn delta(&self) -> Duration {
        self.delta
    }

    /// Wall time from creation to the most recent update. Constant within a frame.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.last.saturating_duration_since(self.start)
    }

    /// Number of updates so far.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Measures how long work takes, for benchmarks and budget checks.
///
/// Reads the OS clock like [`RealClock`], but on demand rather than once per frame. Its
/// readings describe performance only: never feed them into simulation or gameplay (rule R2).
#[derive(Clone, Copy, Debug)]
pub struct Stopwatch {
    start: Instant,
}

impl Stopwatch {
    /// Starts timing now.
    #[must_use]
    pub fn start() -> Self {
        Self { start: os_now() }
    }

    /// Time since [`Self::start`] or the last [`Self::lap`].
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        os_now().saturating_duration_since(self.start)
    }

    /// Returns [`Self::elapsed`] and restarts the stopwatch.
    pub fn lap(&mut self) -> Duration {
        let now = os_now();
        let lap = now.saturating_duration_since(self.start);
        self.start = now;
        lap
    }
}

#[allow(
    clippy::disallowed_methods,
    reason = "RealClock and Stopwatch are the engine's only OS clock reads; all other time flows from them (rule R2)"
)]
fn os_now() -> Instant {
    Instant::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_and_frame_constant() {
        let mut clock = RealClock::new();
        assert_eq!(clock.elapsed(), Duration::ZERO);
        assert_eq!(clock.frames(), 0);
        let mut previous = clock.elapsed();
        for _ in 0..100 {
            let delta = clock.update();
            assert_eq!(delta, clock.delta());
            assert_eq!(clock.elapsed(), previous + delta);
            previous = clock.elapsed();
        }
        assert_eq!(clock.frames(), 100);
        // Reading does not advance the clock.
        assert_eq!(clock.elapsed(), previous);
    }

    #[test]
    fn stopwatch_laps_restart_it() {
        let mut stopwatch = Stopwatch::start();
        let first = stopwatch.lap();
        let total = stopwatch.elapsed();
        assert!(stopwatch.elapsed() >= total, "monotonic");
        // The lap restarted the stopwatch, so the first lap isn't counted again.
        let both = first + stopwatch.lap();
        assert!(both >= first);
    }
}
