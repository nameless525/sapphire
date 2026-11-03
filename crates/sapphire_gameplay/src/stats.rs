//! The simulation's counters against its budget (rule R9, spec §3.2).

use std::time::Duration;

use sapphire_physics::PhysicsStats;

/// §3.2's "Fixed-step sim, per 60 Hz step" on Desktop-Low: combat, AI and the controller.
pub const BUDGET_DESKTOP_LOW: Duration = Duration::from_millis(1);

/// §3.2's "Fixed-step sim, per 60 Hz step" on Mobile-Low, which runs two such steps a frame.
pub const BUDGET_MOBILE_LOW: Duration = Duration::from_micros(1500);

/// §3.2's "Physics, per step" on Desktop-Low, which for now is all the character controllers: hit
/// queries and the camera add to it when they land.
pub const PHYSICS_BUDGET_DESKTOP_LOW: Duration = Duration::from_micros(700);

/// §3.2's "Physics, per step" on Mobile-Low.
pub const PHYSICS_BUDGET_MOBILE_LOW: Duration = Duration::from_micros(1200);

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
    /// Presses of intents from buttons so far. Those a brain makes itself aren't counted.
    pub intents_pressed: u64,
    /// Presses whose buffer ran out with no window taking them so far.
    pub intents_expired: u64,
    /// Presses given up because a full ring needed room, so far.
    pub intents_overwritten: u64,
    /// Moves started so far, all told.
    pub moves_started: u64,
    /// Of those, the ones that cancelled another.
    pub cancels_fired: u64,
    /// Of the moves started, the ones a counter window started, from a move or from none.
    pub counters_fired: u64,
    /// Cues published so far.
    pub cues_published: u64,
    /// Cues dropped so far, for want of room in a step's queue.
    pub cues_dropped: u64,
    /// Hit volumes and hurtbox changes not published so far, for want of room in an actor's step.
    pub volumes_dropped: u64,
    /// Steps on which an actor hit the limit on moves started in one step.
    pub transition_limit_steps: u64,
    /// Wall time of the latest step's character controllers, physics included, to hold against
    /// [`PHYSICS_BUDGET_DESKTOP_LOW`] or [`PHYSICS_BUDGET_MOBILE_LOW`].
    pub controllers_last: Duration,
    /// The longest step's character controllers.
    pub controllers_longest: Duration,
    /// Steps whose character controllers took longer than [`crate::SimConfig::physics_budget`].
    pub controllers_over_budget_steps: u64,
    /// Actors that asked for a controller and couldn't have one: an unknown profile, or a collision
    /// world with no room for the body. They live on without one.
    pub controllers_refused: u64,
    /// What the collision world did in the latest step: moves, sweeps and casts.
    pub physics: PhysicsStats,
    /// Hits that did something so far. `Simulation::hits` has what hit queries found.
    pub hits_applied: u64,
    /// Of those, the ones armour took.
    pub hits_absorbed: u64,
    /// Reactions a hit forced so far.
    pub reactions_forced: u64,
    /// Poise meters emptied so far.
    pub poise_breaks: u64,
    /// Actors defeated so far.
    pub defeats: u64,
    /// Hurtboxes and perfect-dodge volumes the physics world refused so far: non-zero means it's too
    /// small, or a pose went bad.
    pub hurtboxes_refused: u64,
    /// Perfect dodges fired so far. `Simulation::hits` has the swings behind the latest step's.
    pub perfect_dodges: u64,
    /// A party's swaps so far, those a defeat forced among them.
    pub swaps: u32,
    /// Of those, the ones a defeat forced.
    pub forced_swaps: u32,
    /// Timelines started.
    pub timelines_played: u64,
    /// Gameplay events timelines have fired.
    pub timeline_events: u64,
    /// Time scale beats the world clock's full stack turned away: one is a timeline playing slower or
    /// faster than written, on every run alike.
    pub timeline_scales_refused: u64,
}

impl SimStats {
    /// The mean wall time of a step so far.
    #[must_use]
    pub fn mean_step(&self) -> Duration {
        self.total_step_time.checked_div(u32::try_from(self.steps).unwrap_or(u32::MAX)).unwrap_or_default()
    }
}
