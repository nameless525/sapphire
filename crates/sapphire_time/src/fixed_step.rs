//! [`FixedStepDriver`]: the fixed-timestep accumulator (spec §4.2 step 2, §5.7).

use std::time::Duration;

use crate::TICK_HZ;

/// Accumulator units per tick. The accumulator counts nanoseconds × [`TICK_HZ`], so one tick is
/// exactly 10⁹ units and the 60 Hz rate never drifts against the wall clock (a tick is not a
/// whole number of nanoseconds).
const TICK_UNITS: i128 = 1_000_000_000;

/// Upper limit for [`FixedStepConfig::snap_tolerance`]: an eighth of a tick (about 2.08 ms).
const MAX_SNAP_UNITS: i128 = TICK_UNITS / 8;

/// The largest `f32` below 1, so `alpha` stays in [0, 1) even when rounding would reach 1.
const ALPHA_MAX: f32 = 1.0 - f32::EPSILON / 2.0;

/// Tuning for [`FixedStepDriver`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedStepConfig {
    /// Most steps run in one frame; time beyond that is dropped so a slow frame cannot cause a
    /// spiral of death. At least 1.
    pub max_steps_per_frame: u32,
    /// Frame deltas longer than this (a debugger breakpoint, the app returning from the
    /// background) are clamped before accumulating.
    pub max_frame_delta: Duration,
    /// How early a step may run. When the accumulated frame time is short of the next tick by
    /// no more than this, the step runs this frame and the shortfall is repaid from the next
    /// frame's time.
    ///
    /// This absorbs vsync timestamp jitter: without it a 30 fps frame on the 60 Hz sim can
    /// alternate between 1 and 3 steps instead of a steady 2. Because the shortfall is always
    /// repaid, simulated time never drifts from wall time; it runs ahead by at most this much.
    /// Clamped to an eighth of a tick (about 2.08 ms). Zero disables it.
    pub snap_tolerance: Duration,
}

impl Default for FixedStepConfig {
    /// 4 steps, 250 ms, 200 µs.
    fn default() -> Self {
        Self {
            max_steps_per_frame: 4,
            max_frame_delta: Duration::from_millis(250),
            snap_tolerance: Duration::from_micros(200),
        }
    }
}

/// What one frame should simulate.
#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use = "run `count` fixed steps, then present with `alpha`"]
pub struct FixedSteps {
    /// Fixed steps to run this frame, from 0 to [`FixedStepConfig::max_steps_per_frame`].
    pub count: u32,
    /// How far presentation is between the previous and the latest fixed state, in [0, 1).
    pub alpha: f32,
    /// Real time discarded by the frame-delta clamp and the step cap; nonzero means the
    /// simulation fell behind. Publish it as a stat (rule R9).
    pub dropped: Duration,
}

/// Converts variable frame times into whole 60 Hz steps plus an interpolation factor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedStepDriver {
    config: FixedStepConfig,
    /// [`FixedStepConfig::snap_tolerance`] in accumulator units.
    snap_units: i128,
    /// Real time not yet simulated, in accumulator units: in `[-snap_units, TICK_UNITS)`. A
    /// negative value is time borrowed by a step that ran early, repaid by the next frames.
    accumulator: i128,
    ticks: u64,
}

impl FixedStepDriver {
    /// A driver with an empty accumulator. Out-of-range settings are clamped (see
    /// [`FixedStepConfig`]); [`FixedStepDriver::config`] reports the values in effect.
    #[must_use]
    pub fn new(config: FixedStepConfig) -> Self {
        let snap_units = to_units(config.snap_tolerance).min(MAX_SNAP_UNITS);
        let config = FixedStepConfig {
            max_steps_per_frame: config.max_steps_per_frame.max(1),
            snap_tolerance: from_units(snap_units),
            ..config
        };
        Self { config, snap_units, accumulator: 0, ticks: 0 }
    }

    /// The configuration in effect, after clamping.
    #[must_use]
    pub fn config(&self) -> &FixedStepConfig {
        &self.config
    }

    /// Adds one frame of real time and returns how many fixed steps to run.
    pub fn advance(&mut self, real_delta: Duration) -> FixedSteps {
        let clamped = real_delta.min(self.config.max_frame_delta);
        let mut dropped = real_delta - clamped;

        self.accumulator += to_units(clamped);
        // The invariant `accumulator >= -snap_units` keeps this non-negative.
        let available = (self.accumulator + self.snap_units) / TICK_UNITS;
        let count = u32::try_from(available).unwrap_or(u32::MAX).min(self.config.max_steps_per_frame);
        // Consume every available tick: the ones run, and the ones beyond the cap, which are dropped.
        self.accumulator -= available * TICK_UNITS;
        dropped += from_units((available - i128::from(count)) * TICK_UNITS);
        self.ticks += u64::from(count);

        FixedSteps { count, alpha: self.alpha(), dropped }
    }

    /// The current interpolation factor, in [0, 1). Zero while a step that ran early is being
    /// repaid.
    #[must_use]
    pub fn alpha(&self) -> f32 {
        ((self.accumulator.max(0) as f64 / TICK_UNITS as f64) as f32).min(ALPHA_MAX)
    }

    /// Fixed steps issued since creation.
    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.ticks
    }
}

impl Default for FixedStepDriver {
    fn default() -> Self {
        Self::new(FixedStepConfig::default())
    }
}

/// Accumulator units in `duration`. Even Duration::MAX times TICK_HZ fits i128.
fn to_units(duration: Duration) -> i128 {
    duration.as_nanos() as i128 * i128::from(TICK_HZ)
}

/// The duration of a non-negative number of accumulator units, rounded down to the nanosecond.
fn from_units(units: i128) -> Duration {
    let nanos = (units.max(0) / i128::from(TICK_HZ)) as u128;
    Duration::new((nanos / 1_000_000_000) as u64, (nanos % 1_000_000_000) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_never_reaches_one() {
        let mut driver =
            FixedStepDriver::new(FixedStepConfig { snap_tolerance: Duration::ZERO, ..Default::default() });
        driver.accumulator = TICK_UNITS - 1;
        assert!(driver.alpha() < 1.0);
        assert_eq!(driver.alpha(), ALPHA_MAX);
    }

    #[test]
    fn zero_max_steps_is_raised_to_one() {
        let driver = FixedStepDriver::new(FixedStepConfig { max_steps_per_frame: 0, ..Default::default() });
        assert_eq!(driver.config().max_steps_per_frame, 1);
    }

    #[test]
    fn units_round_trip() {
        assert_eq!(to_units(Duration::from_secs(1)), 60 * TICK_UNITS);
        assert_eq!(from_units(60 * TICK_UNITS), Duration::from_secs(1));
        assert_eq!(from_units(to_units(Duration::MAX)), Duration::MAX);
    }

    #[test]
    fn extreme_deltas_conserve_elapsed_time() {
        for delta in [Duration::from_secs(200_000_000), Duration::MAX] {
            let mut driver = FixedStepDriver::new(FixedStepConfig {
                max_frame_delta: Duration::MAX,
                snap_tolerance: Duration::ZERO,
                ..Default::default()
            });
            let steps = driver.advance(delta);
            // Integer units are an independent exact oracle; Duration floors less than 1 ns.
            let accounted =
                to_units(steps.dropped) + i128::from(steps.count) * TICK_UNITS + driver.accumulator;
            assert!((0..i128::from(TICK_HZ)).contains(&(to_units(delta) - accounted)));
            assert_eq!(steps.count, 4);
            assert!(steps.alpha < 1.0);
        }
    }

    #[test]
    fn snap_tolerance_is_clamped_to_an_eighth_of_a_tick() {
        let driver = FixedStepDriver::new(FixedStepConfig {
            snap_tolerance: Duration::from_millis(5),
            ..Default::default()
        });
        assert_eq!(driver.snap_units, MAX_SNAP_UNITS);
        assert_eq!(driver.config().snap_tolerance, Duration::from_nanos(2_083_333));
        assert_eq!(FixedStepDriver::default().config().snap_tolerance, Duration::from_micros(200));
    }
}
