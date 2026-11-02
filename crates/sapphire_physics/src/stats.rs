//! What the world has done since it last reported (rule R9).

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// §3.2's physics budget for one fixed step on Desktop-Low, in microseconds.
pub const STEP_BUDGET_DESKTOP_LOW_US: f32 = 700.0;
/// §3.2's physics budget for one fixed step on Mobile-Low, in microseconds.
pub const STEP_BUDGET_MOBILE_LOW_US: f32 = 1200.0;

/// The world's size and the work it has done since [`take_stats`] was last called. Counts, not
/// time: this crate reads no clock (rule R2), so whoever runs the fixed step times it against
/// [`STEP_BUDGET_DESKTOP_LOW_US`] or [`STEP_BUDGET_MOBILE_LOW_US`].
///
/// [`take_stats`]: crate::PhysicsWorld::take_stats
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicsStats {
    /// Static colliders in the world.
    pub statics: u32,
    /// Kinematic bodies in the world.
    pub bodies: u32,
    /// Calls to `move_and_collide`.
    pub moves: u32,
    /// Sweeps those moves made.
    pub move_sweeps: u32,
    /// Surfaces those moves hit.
    pub move_hits: u32,
    /// Moves that began by pushing a body out of something.
    pub overlaps_resolved: u32,
    /// Ray casts.
    pub ray_casts: u32,
    /// Shape casts, single and all-hits.
    pub shape_casts: u32,
    /// Overlap queries.
    pub overlap_queries: u32,
    /// Penetration queries.
    pub penetration_queries: u32,
    /// Shape pairs (triangles included) tested past the broadphase: the closest thing to a cost.
    pub narrow_tests: u32,
    /// Results that didn't fit the buffers callers gave. Non-zero means a query lost hits.
    pub results_dropped: u32,
}

#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub(crate) moves: AtomicU32,
    pub(crate) move_sweeps: AtomicU32,
    pub(crate) move_hits: AtomicU32,
    pub(crate) overlaps_resolved: AtomicU32,
    pub(crate) ray_casts: AtomicU32,
    pub(crate) shape_casts: AtomicU32,
    pub(crate) overlap_queries: AtomicU32,
    pub(crate) penetration_queries: AtomicU32,
    pub(crate) narrow_tests: AtomicU32,
    pub(crate) results_dropped: AtomicU32,
}

impl Counters {
    pub(crate) fn add(counter: &AtomicU32, amount: u32) {
        counter.fetch_add(amount, Relaxed);
    }

    pub(crate) fn take(&self, statics: u32, bodies: u32) -> PhysicsStats {
        let take = |counter: &AtomicU32| counter.swap(0, Relaxed);
        PhysicsStats {
            statics,
            bodies,
            moves: take(&self.moves),
            move_sweeps: take(&self.move_sweeps),
            move_hits: take(&self.move_hits),
            overlaps_resolved: take(&self.overlaps_resolved),
            ray_casts: take(&self.ray_casts),
            shape_casts: take(&self.shape_casts),
            overlap_queries: take(&self.overlap_queries),
            penetration_queries: take(&self.penetration_queries),
            narrow_tests: take(&self.narrow_tests),
            results_dropped: take(&self.results_dropped),
        }
    }
}
