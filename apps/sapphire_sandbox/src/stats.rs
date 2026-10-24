//! Frame statistics, logged periodically (rule R9: every system publishes a stat).

use std::time::Duration;

use sapphire_rhi::SkipReason;

use crate::LOG_TARGET;

/// How often a summary line is logged.
const REPORT_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(crate) struct FrameStats {
    window: Duration,
    /// Loop iterations, presented or not.
    frames: u32,
    /// Frames actually presented.
    presented: u32,
    worst: Duration,
    skipped: u32,
    /// The latest skip reason, so a run of identical skips logs once.
    last_skip: Option<SkipReason>,
    dropped: Duration,
    /// Process allocation total at the start of the window.
    allocations_at_start: u64,
}

impl FrameStats {
    pub fn new() -> Self {
        Self { allocations_at_start: sapphire_core::alloc::stats().total_allocations, ..Self::default() }
    }

    pub fn skipped(&mut self, reason: SkipReason) {
        self.skipped += 1;
        if self.last_skip.replace(reason) != Some(reason) {
            tracing::debug!(target: LOG_TARGET, ?reason, "skipping frames");
        }
    }

    /// Records one frame; logs and resets once per [`REPORT_INTERVAL`].
    pub fn record(&mut self, frame_time: Duration, dropped: Duration, presented: bool) {
        if presented {
            self.presented += 1;
            self.last_skip = None;
        }
        self.window += frame_time;
        self.frames += 1;
        self.worst = self.worst.max(frame_time);
        self.dropped += dropped;
        if self.window < REPORT_INTERVAL {
            return;
        }

        let allocations = sapphire_core::alloc::stats().total_allocations;
        let frames = self.frames.max(1);
        let seconds = self.window.as_secs_f64();
        let average_ms = seconds * 1e3 / f64::from(frames);
        tracing::info!(
            target: LOG_TARGET,
            fps = format_args!("{:.1}", f64::from(self.presented) / seconds),
            loop_hz = format_args!("{:.1}", f64::from(frames) / seconds),
            avg_ms = format_args!("{average_ms:.2}"),
            worst_ms = format_args!("{:.2}", self.worst.as_secs_f64() * 1e3),
            skipped = self.skipped,
            sim_dropped_ms = self.dropped.as_millis(),
            allocs_per_frame = (allocations - self.allocations_at_start) / u64::from(frames),
            "frame stats",
        );
        *self = Self {
            allocations_at_start: sapphire_core::alloc::stats().total_allocations,
            last_skip: self.last_skip,
            ..Self::default()
        };
    }
}
