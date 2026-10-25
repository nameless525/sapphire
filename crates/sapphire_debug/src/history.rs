//! [`FrameTimes`]: the last few seconds of frame times, for the frame-time graph.

use std::time::Duration;

/// Frames kept: four seconds at 60 fps.
pub const HISTORY: usize = 240;

/// A fixed ring of recent frame times. Recording is a store into an array, so the overlay keeps
/// history even while hidden, at no measurable cost.
#[derive(Clone, Debug)]
pub struct FrameTimes {
    /// Seconds, oldest first from `next` (once full).
    samples: [f32; HISTORY],
    next: usize,
    len: usize,
}

/// Statistics over the recorded frames.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameSummary {
    pub frames: usize,
    pub min: f32,
    pub mean: f32,
    /// The 99th percentile: the frame time 1 frame in 100 exceeds.
    pub p99: f32,
    pub max: f32,
}

impl FrameSummary {
    /// Frames per second at the mean frame time.
    pub fn fps(&self) -> f32 {
        if self.mean > 0.0 { 1.0 / self.mean } else { 0.0 }
    }
}

impl Default for FrameTimes {
    fn default() -> Self {
        Self { samples: [0.0; HISTORY], next: 0, len: 0 }
    }
}

impl FrameTimes {
    pub fn push(&mut self, frame: Duration) {
        self.samples[self.next] = frame.as_secs_f32();
        self.next = (self.next + 1) % HISTORY;
        self.len = (self.len + 1).min(HISTORY);
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The recorded frame times in seconds, oldest first.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = f32> + '_ {
        let start = (self.next + HISTORY - self.len) % HISTORY;
        (0..self.len).map(move |i| self.samples[(start + i) % HISTORY])
    }

    /// The latest frame time in seconds.
    pub fn last(&self) -> Option<f32> {
        (self.len > 0).then(|| self.samples[(self.next + HISTORY - 1) % HISTORY])
    }

    /// Statistics over everything recorded; all zero while empty. Sorts a copy on the stack.
    pub fn summary(&self) -> FrameSummary {
        if self.len == 0 {
            return FrameSummary::default();
        }
        let mut sorted = [0.0_f32; HISTORY];
        for (slot, sample) in sorted.iter_mut().zip(self.iter()) {
            *slot = sample;
        }
        let sorted = &mut sorted[..self.len];
        sorted.sort_unstable_by(f32::total_cmp);
        // Nearest rank: the smallest sample with at least 99% of samples at or below it.
        let rank = (self.len * 99).div_ceil(100).max(1);
        FrameSummary {
            frames: self.len,
            min: sorted[0],
            mean: sorted.iter().sum::<f32>() / self.len as f32,
            p99: sorted[rank - 1],
            max: sorted[self.len - 1],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn keeps_the_latest_frames_in_order() {
        let mut times = FrameTimes::default();
        assert!(times.is_empty() && times.last().is_none());
        assert_eq!(times.summary(), FrameSummary::default());
        for frame in 1..=(HISTORY as u64 + 10) {
            times.push(ms(frame));
        }
        assert_eq!(times.len(), HISTORY);
        let first = times.iter().next().expect("full");
        assert!((first - 0.011).abs() < 1e-6, "the 10 oldest were overwritten: {first}");
        assert!((times.last().expect("full") - (HISTORY as f32 + 10.0) / 1000.0).abs() < 1e-6);
        assert!(times.iter().zip(times.iter().skip(1)).all(|(a, b)| a < b), "oldest first");
    }

    #[test]
    fn summarizes_with_a_nearest_rank_percentile() {
        let mut times = FrameTimes::default();
        // 99 frames of 16 ms and one hitch of 50 ms.
        for _ in 0..99 {
            times.push(ms(16));
        }
        times.push(ms(50));
        let summary = times.summary();
        assert_eq!(summary.frames, 100);
        assert!((summary.min - 0.016).abs() < 1e-6 && (summary.max - 0.050).abs() < 1e-6);
        assert!((summary.p99 - 0.016).abs() < 1e-6, "one hitch in 100 is above the 99th percentile");
        assert!((summary.mean - 0.01634).abs() < 1e-5);
        assert!((summary.fps() - 61.2).abs() < 0.1);
        times.push(ms(50));
        assert!((times.summary().p99 - 0.050).abs() < 1e-6, "two in 101 is not");

        let mut one = FrameTimes::default();
        one.push(ms(20));
        let summary = one.summary();
        assert_eq!((summary.min, summary.p99, summary.max), (0.02, 0.02, 0.02));
    }
}
