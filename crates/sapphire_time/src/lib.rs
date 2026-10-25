//! Clock domains and the fixed-step driver (spec §4.5, §5.7).
//!
//! Owns:
//! - [`FixedStepDriver`]: turns variable frame times into 0–4 fixed 60 Hz steps plus an
//!   interpolation `alpha`.
//! - [`RealClock`]: unscaled wall time for UI, input timestamps and networking. It and
//!   [`Stopwatch`] (for benchmarks and budgets) are the only code in the engine that reads the
//!   OS monotonic clock (rule R2).
//! - [`ScaledClock`]: the world clock, slowed or stopped by a stack of [`TimeScale`]s (pause,
//!   perfect dodge, cinematics, debug).
//! - [`LocalClock`]: one entity's clock, with its own scales and hitstop on top of the world's.
//! - [`Timer`] and [`Cooldown`]: deadlines read against any [`Clock`].
//!
//! Does not own: the frame loop (the app drives [`FixedStepDriver::advance`] and runs the steps)
//! or ECS integration (`LocalClock` becomes a component in M1).
//!
//! # Time model
//!
//! Simulation time is integer sub-ticks: [`SUBTICKS_PER_TICK`] per 60 Hz tick, so 1 ms is
//! exactly 60 sub-ticks. Scales are integer per-mille factors and their products are exact
//! fixed-point values with the fractional sub-tick carried from tick to tick, so scaled time
//! never drifts and identical inputs give bit-identical clocks on every platform.
//!
//! Each fixed step, in order:
//! 1. [`ScaledClock::tick`] on the world clock;
//! 2. [`LocalClock::tick`] on every entity clock, passing the world clock;
//! 3. gameplay, which may push or pop scales and call [`LocalClock::apply_hitstop`]. Those
//!    changes take effect from the next step's ticks.

mod fixed_step;
mod local;
mod real;
mod scale;
mod scaled;
mod sim_time;
mod timer;

pub use fixed_step::{FixedStepConfig, FixedStepDriver, FixedSteps};
pub use local::LocalClock;
pub use real::{RealClock, Stopwatch};
pub use scale::{ClockId, NORMAL_SPEED, ScaleHandle, ScaleSource, ScaleStackFull, TimeScale};
pub use scaled::ScaledClock;
pub use sim_time::{SimDuration, SimTime};
pub use timer::{Clock, Cooldown, Timer};

/// Fixed simulation rate on every platform; combat frame data is authored at 60 (spec §5.7).
pub const TICK_HZ: u32 = 60;

/// Sub-ticks per tick: the resolution of scaled time.
pub const SUBTICKS_PER_TICK: u32 = 1000;

/// Length of one fixed step in seconds, for integrators that need a float `dt`.
pub const TICK_SECONDS: f32 = 1.0 / TICK_HZ as f32;
