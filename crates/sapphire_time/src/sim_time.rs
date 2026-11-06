//! [`SimTime`] and [`SimDuration`]: integer sub-tick time.

use std::ops::{Add, AddAssign, Sub};
use std::time::Duration;

use crate::{SUBTICKS_PER_TICK, TICK_HZ};

/// Sub-ticks per second (60,000).
const SUBTICKS_PER_SECOND: u64 = SUBTICKS_PER_TICK as u64 * TICK_HZ as u64;

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A span of simulation time in sub-ticks. Arithmetic saturates rather than wrapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimDuration(u64);

impl SimDuration {
    /// No time.
    pub const ZERO: Self = Self(0);

    /// `subticks` sub-ticks.
    #[must_use]
    pub const fn from_subticks(subticks: u64) -> Self {
        Self(subticks)
    }

    /// `ticks` whole 60 Hz ticks: the unit combat frame data is authored in.
    #[must_use]
    pub const fn from_ticks(ticks: u64) -> Self {
        Self(ticks.saturating_mul(SUBTICKS_PER_TICK as u64))
    }

    /// `millis` milliseconds, exactly (one millisecond is 60 sub-ticks).
    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis.saturating_mul(SUBTICKS_PER_SECOND / 1000))
    }

    /// The nearest whole sub-tick at or below `duration`.
    #[must_use]
    pub fn from_duration(duration: Duration) -> Self {
        let subticks = duration.as_nanos().saturating_mul(u128::from(SUBTICKS_PER_SECOND)) / NANOS_PER_SECOND;
        Self(u64::try_from(subticks).unwrap_or(u64::MAX))
    }

    /// The length in sub-ticks.
    #[must_use]
    pub const fn as_subticks(self) -> u64 {
        self.0
    }

    /// The number of whole ticks.
    #[must_use]
    pub const fn whole_ticks(self) -> u64 {
        self.0 / SUBTICKS_PER_TICK as u64
    }

    /// The length in seconds, for presentation (animation rates, UI).
    #[must_use]
    pub fn as_secs_f32(self) -> f32 {
        (self.0 as f64 / SUBTICKS_PER_SECOND as f64) as f32
    }

    /// The equivalent wall-clock duration, rounded down to the nanosecond.
    #[must_use]
    pub fn to_duration(self) -> Duration {
        let nanos = u128::from(self.0) * NANOS_PER_SECOND / u128::from(SUBTICKS_PER_SECOND);
        // All u64 sub-tick counts fit in Duration's seconds range.
        Duration::new((nanos / NANOS_PER_SECOND) as u64, (nanos % NANOS_PER_SECOND) as u32)
    }
}

impl Add for SimDuration {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl AddAssign for SimDuration {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for SimDuration {
    type Output = Self;

    /// Saturates at zero.
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

/// A point on one clock's timeline, in sub-ticks since that clock started.
///
/// Times from different clocks are not comparable; compare only times read from the same clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimTime(u64);

impl SimTime {
    /// The clock's start.
    pub const ZERO: Self = Self(0);

    /// The time `subticks` after the clock's start.
    #[must_use]
    pub const fn from_subticks(subticks: u64) -> Self {
        Self(subticks)
    }

    /// Sub-ticks since the clock's start.
    #[must_use]
    pub const fn as_subticks(self) -> u64 {
        self.0
    }

    /// Time elapsed since `earlier`, or zero if `earlier` is later.
    #[must_use]
    pub const fn since(self, earlier: Self) -> SimDuration {
        SimDuration(self.0.saturating_sub(earlier.0))
    }

    /// Seconds since the clock's start, for presentation.
    #[must_use]
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / SUBTICKS_PER_SECOND as f64
    }
}

/// A clock's time to draw, in ticks with a fraction: `alpha` of the way from where it was before
/// its latest step (`now - last_delta`) to `now`. It never reads past `now`, and a shrinking step
/// can't take it back past a frame already drawn, as the two ends only move forward.
#[must_use]
pub(crate) fn presented_ticks(now: SimTime, last_delta: SimDuration, alpha: f32) -> f64 {
    // Written so that NaN draws the previous state.
    let alpha = if alpha >= 0.0 { f64::from(alpha.min(1.0)) } else { 0.0 };
    let behind = (1.0 - alpha) * last_delta.as_subticks() as f64;
    (now.as_subticks() as f64 - behind) / f64::from(SUBTICKS_PER_TICK)
}

impl Add<SimDuration> for SimTime {
    type Output = Self;

    fn add(self, rhs: SimDuration) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl AddAssign<SimDuration> for SimTime {
    fn add_assign(&mut self, rhs: SimDuration) {
        *self = *self + rhs;
    }
}

impl Sub for SimTime {
    type Output = SimDuration;

    /// Same as [`SimTime::since`].
    fn sub(self, rhs: Self) -> SimDuration {
        self.since(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(SimDuration::from_ticks(1).as_subticks(), 1000);
        assert_eq!(SimDuration::from_millis(1).as_subticks(), 60);
        assert_eq!(SimDuration::from_millis(1000), SimDuration::from_ticks(60));
        assert_eq!(SimDuration::from_ticks(12).whole_ticks(), 12);
        assert_eq!(SimDuration::from_subticks(1999).whole_ticks(), 1);
        assert_eq!(SimDuration::from_ticks(30).as_secs_f32(), 0.5);
        assert_eq!(SimTime::from_subticks(90_000).as_secs_f64(), 1.5);
    }

    #[test]
    fn duration_conversions() {
        assert_eq!(SimDuration::from_duration(Duration::from_secs(1)), SimDuration::from_ticks(60));
        assert_eq!(SimDuration::from_duration(Duration::from_millis(250)), SimDuration::from_millis(250));
        // One sub-tick is 16.666… µs; anything shorter rounds down to zero.
        assert_eq!(SimDuration::from_duration(Duration::from_micros(16)), SimDuration::ZERO);
        assert_eq!(SimDuration::from_duration(Duration::from_micros(17)).as_subticks(), 1);
        assert_eq!(SimDuration::from_duration(Duration::MAX).as_subticks(), u64::MAX);
        assert_eq!(SimDuration::from_ticks(60).to_duration(), Duration::from_secs(1));
        assert_eq!(SimDuration::from_subticks(1).to_duration(), Duration::from_nanos(16_666));
    }

    #[test]
    fn duration_conversion_preserves_the_full_subtick_range() {
        for subticks in [0, 1, 60_000, u64::MAX / 1_000_000, u64::MAX - 1, u64::MAX] {
            let sim = SimDuration::from_subticks(subticks);
            let duration = sim.to_duration();
            let expected = u128::from(subticks) * 1_000_000_000 / 60_000;
            assert_eq!(duration.as_nanos(), expected);
            assert!(subticks - SimDuration::from_duration(duration).as_subticks() <= 1);
        }
    }

    #[test]
    fn arithmetic_saturates() {
        let a = SimDuration::from_ticks(2);
        let b = SimDuration::from_ticks(5);
        assert_eq!(a - b, SimDuration::ZERO);
        assert_eq!(b - a, SimDuration::from_ticks(3));
        assert_eq!(SimDuration::from_subticks(u64::MAX) + a, SimDuration::from_subticks(u64::MAX));
        assert_eq!(SimDuration::from_ticks(u64::MAX).as_subticks(), u64::MAX);

        let t = SimTime::ZERO + b;
        assert_eq!(t - SimTime::ZERO, b);
        assert_eq!(SimTime::ZERO - t, SimDuration::ZERO);
        let mut u = t;
        u += a;
        assert_eq!(u.since(t), a);
        let mut d = a;
        d += a;
        assert_eq!(d, SimDuration::from_ticks(4));
    }
}
