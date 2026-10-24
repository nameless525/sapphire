//! [`LocalClock`]: per-entity time with its own scales and hitstop.

use crate::scale::{self, ScaleStack};
use crate::{
    ClockId, ScaleHandle, ScaleSource, ScaleStackFull, ScaledClock, SimDuration, SimTime, TimeScale,
};

/// Scale-stack capacity of an entity clock (spec §4.5).
const LOCAL_SCALE_CAPACITY: usize = 4;

/// One entity's clock. A plain struct for now; it becomes an ECS component in M1.
///
/// Created by [`ScaledClock::new_local_clock`], which gives it a [`ClockId`] in that world.
///
/// Each tick it advances by the world clock's scale times its own scales, or not at all while
/// hitstop is active. Everything the entity owns reads it: move timelines, cooldowns, animation
/// playback, VFX and audio pitch (spec §4.5), so hitstop and perfect dodge affect them together.
///
/// # Hitstop
///
/// [`LocalClock::apply_hitstop`] freezes the clock for a number of *real* fixed ticks, so a
/// hit lands with the same punch whatever the world's slow motion. Hitstop applied during a
/// step freezes exactly the next N ticks. It does not count down while the world clock is
/// stopped, so pausing mid-hitstop resumes it intact.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalClock {
    scales: ScaleStack<LOCAL_SCALE_CAPACITY>,
    hitstop_ticks: u16,
    now: SimTime,
    last_delta: SimDuration,
    /// Fraction of a sub-tick carried between ticks.
    remainder: u64,
}

impl LocalClock {
    /// Maximum number of scales pushed at once.
    pub const SCALE_CAPACITY: usize = LOCAL_SCALE_CAPACITY;

    pub(crate) const fn new(id: ClockId) -> Self {
        Self {
            scales: ScaleStack::new(id),
            hitstop_ticks: 0,
            now: SimTime::ZERO,
            last_delta: SimDuration::ZERO,
            remainder: 0,
        }
    }

    /// This clock's id within its world.
    #[must_use]
    pub fn id(&self) -> ClockId {
        self.scales.owner()
    }

    /// Pushes a local scale, multiplied with the world scale from the next tick.
    ///
    /// For perfect dodge, the player's counter-scale is the inverse of the world slowdown: with
    /// the world at 200‰, a local 5000‰ keeps the player at 1×.
    pub fn push_scale(&mut self, source: ScaleSource, per_mille: u32) -> Result<ScaleHandle, ScaleStackFull> {
        self.scales.push(source, per_mille)
    }

    /// Removes the scale `handle` refers to. Returns `None` if it was already removed, or
    /// (failing a debug assertion) if another clock issued `handle`.
    pub fn pop_scale(&mut self, handle: ScaleHandle) -> Option<TimeScale> {
        self.scales.pop(handle)
    }

    /// Removes every local scale pushed by `source` and returns how many were removed.
    pub fn remove_source(&mut self, source: ScaleSource) -> usize {
        self.scales.remove_source(source)
    }

    /// The local scales, oldest first.
    pub fn scales(&self) -> impl ExactSizeIterator<Item = TimeScale> + '_ {
        self.scales.iter()
    }

    /// Freezes the clock for the next `ticks` fixed ticks. Overlapping hits keep the longer
    /// freeze rather than adding up.
    pub fn apply_hitstop(&mut self, ticks: u16) {
        self.hitstop_ticks = self.hitstop_ticks.max(ticks);
    }

    /// Frozen ticks still to come.
    #[must_use]
    pub fn hitstop_ticks(&self) -> u16 {
        self.hitstop_ticks
    }

    /// The speed this clock will run at on the next tick, as a float (1.0 is normal): the world
    /// scale times the local scales, or 0 during hitstop. For presentation such as animation rate.
    #[must_use]
    pub fn effective_scale(&self, world: &ScaledClock) -> f32 {
        if self.hitstop_ticks > 0 { 0.0 } else { scale::factor_to_f32(self.factor(world)) }
    }

    /// Advances one fixed step and returns how much of this entity's time passed. Call after
    /// [`ScaledClock::tick`] in the same step.
    pub fn tick(&mut self, world: &ScaledClock) -> SimDuration {
        let delta = if self.hitstop_ticks > 0 {
            if !world.is_stopped() {
                self.hitstop_ticks -= 1;
            }
            SimDuration::ZERO
        } else {
            SimDuration::from_subticks(scale::advance(self.factor(world), &mut self.remainder))
        };
        self.now += delta;
        self.last_delta = delta;
        delta
    }

    /// This entity's time since its clock started.
    #[must_use]
    pub fn now(&self) -> SimTime {
        self.now
    }

    /// How much time the most recent [`LocalClock::tick`] advanced.
    #[must_use]
    pub fn last_delta(&self) -> SimDuration {
        self.last_delta
    }

    fn factor(&self, world: &ScaledClock) -> u128 {
        scale::combine(world.factor(), self.scales.factor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_world_scale() {
        let mut world = ScaledClock::new();
        let mut local = world.new_local_clock();
        world.push_scale(ScaleSource::Cinematic, 500).expect("room");
        world.tick();
        assert_eq!(local.tick(&world), SimDuration::from_subticks(500));
        assert_eq!(local.effective_scale(&world), 0.5);
    }

    #[test]
    fn local_scales_multiply() {
        let mut world = ScaledClock::new();
        let mut local = world.new_local_clock();
        local.push_scale(ScaleSource::Gameplay, 1500).expect("room");
        local.push_scale(ScaleSource::Gameplay, 2000).expect("room");
        assert_eq!(local.tick(&world), SimDuration::from_subticks(3000));
        assert_eq!(local.scales().len(), 2);
    }

    #[test]
    fn hitstop_reports_zero_scale() {
        let mut world = ScaledClock::new();
        let mut local = world.new_local_clock();
        local.apply_hitstop(2);
        assert_eq!(local.effective_scale(&world), 0.0);
        assert_eq!(local.hitstop_ticks(), 2);
        local.tick(&world);
        local.tick(&world);
        assert_eq!(local.effective_scale(&world), 1.0);
    }

    #[test]
    fn capacity_is_enforced() {
        let mut local = ScaledClock::new().new_local_clock();
        for _ in 0..LocalClock::SCALE_CAPACITY {
            local.push_scale(ScaleSource::Gameplay, 900).expect("room");
        }
        assert!(local.push_scale(ScaleSource::Gameplay, 900).is_err());
        assert_eq!(local.remove_source(ScaleSource::Gameplay), LocalClock::SCALE_CAPACITY);
    }
}
