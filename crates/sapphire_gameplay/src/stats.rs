//! The simulation's counters against its budget (rule R9, spec §3.2).

use std::time::Duration;

/// §3.2's "Fixed-step sim, per 60 Hz step" on Desktop-Low: combat, AI and the controller.
pub const BUDGET_DESKTOP_LOW: Duration = Duration::from_millis(1);

/// §3.2's "Fixed-step sim, per 60 Hz step" on Mobile-Low, which runs two such steps a frame.
pub const BUDGET_MOBILE_LOW: Duration = Duration::from_micros(1500);

/// What the simulation has done as of its latest step, for the overlay and the replay gates.
///
/// Times describe performance only. They never reach simulated state (rule R2), so they're the one
/// thing here that differs from run to run, and none of this is in the checksum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SimStats {
    /// Fixed steps run.
    pub steps: u64,
    /// Wall time of the latest step.
    pub last_step: Duration,
    /// Wall time of the longest step so far.
    pub longest_step: Duration,
    /// Wall time of every step together.
    pub total_step_time: Duration,
    /// The budget the steps are held to: [`crate::SimConfig::step_budget`].
    pub budget: Duration,
    /// Steps that took longer than [`Self::budget`].
    pub over_budget_steps: u64,
    /// Heap allocations on the simulation thread during steps, which rule R6 holds to zero. Where
    /// `sapphire_core::alloc::PER_THREAD_COUNTS` is false, Android's case, it counts the whole
    /// process. Stays 0 with no counting allocator installed; see `sapphire_core::alloc::is_installed`.
    pub allocations: u64,
    /// Actors alive after the latest step.
    pub actors: u32,
    /// The most actors alive after any step: how close a fight comes to the pool's size.
    pub peak_actors: u32,
    /// Spawns refused so far, because the pool was full.
    pub spawns_refused: u64,
}

impl SimStats {
    /// The mean wall time of a step so far.
    #[must_use]
    pub fn mean_step(&self) -> Duration {
        self.total_step_time.checked_div(u32::try_from(self.steps).unwrap_or(u32::MAX)).unwrap_or_default()
    }
}
