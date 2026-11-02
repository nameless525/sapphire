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

    /// This clock's time to draw, in ticks with a fraction: `alpha` of the way from its previous
    /// fixed step to its latest, where transforms draw (spec §4.5). A clock that didn't advance on
    /// its latest step (hitstop, a stopped world) gives exactly [`Self::now`], whatever `alpha`.
    #[must_use]
    pub fn presented_ticks(&self, alpha: f32) -> f64 {
        crate::sim_time::presented_ticks(self.now, self.last_delta, alpha)
    }

    /// Feeds this clock's whole state to `sink` as little-endian bytes, the same on every target
    /// whatever the struct's layout: the scales and the fractional carry included, which nothing
    /// else shows. For state checksums (spec §9, gate 2).
    pub fn write_canonical(&self, sink: &mut impl FnMut(&[u8])) {
        self.scales.write_canonical(sink);
        sink(&self.hitstop_ticks.to_le_bytes());
        sink(&self.now.as_subticks().to_le_bytes());
        sink(&self.last_delta.as_subticks().to_le_bytes());
        sink(&self.remainder.to_le_bytes());
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
    fn presented_time_lies_between_the_last_two_steps_and_holds_while_frozen() {
        let mut world = ScaledClock::new();
        let mut local = world.new_local_clock();
        assert_eq!(local.presented_ticks(0.5), 0.0, "before any step");
        world.tick();
        local.tick(&world);
        world.tick();
        local.tick(&world);
        assert_eq!(local.presented_ticks(1.0), 2.0, "a whole tick reads exactly");
        assert_eq!(local.presented_ticks(0.0), 1.0, "the step before");
        assert_eq!(local.presented_ticks(0.25), 1.25);
        assert_eq!(local.presented_ticks(f32::NAN), 1.0);
        assert_eq!(local.presented_ticks(9.0), 2.0, "never past the latest");

        // A frame between the steps, then hitstop: time goes on to where the clock stopped and
        // holds exactly there, whatever the frame's alpha.
        local.apply_hitstop(3);
        world.tick();
        local.tick(&world);
        for alpha in [0.0, 0.25, 0.999, 1.0] {
            assert_eq!(local.presented_ticks(alpha), 2.0, "frozen at alpha {alpha}");
        }
    }

    /// Entering slow motion shrinks the step, but shown time still only goes forward.
    #[test]
    fn presented_time_never_steps_back_when_the_step_shrinks() {
        let mut world = ScaledClock::new();
        let mut local = world.new_local_clock();
        let mut shown = 0.0_f64;
        let mut show = |clock: &LocalClock, alpha: f32| {
            let now = clock.presented_ticks(alpha);
            assert!(now >= shown, "{now} after {shown}");
            shown = now;
        };
        for tick in 0..30_u32 {
            if tick == 10 {
                world.push_scale(ScaleSource::Cinematic, 100).expect("room");
            }
            if tick == 20 {
                world.push_scale(ScaleSource::Cinematic, 0).expect("room");
            }
            world.tick();
            local.tick(&world);
            for alpha in [0.1, 0.5, 0.9] {
                show(&local, alpha);
            }
        }
    }

    /// The bytes are the contract a recorded checksum depends on, so they're pinned here.
    #[test]
    fn canonical_state_is_pinned_and_complete() {
        let bytes = |clock: &LocalClock| {
            let mut bytes = Vec::new();
            clock.write_canonical(&mut |chunk| bytes.extend_from_slice(chunk));
            bytes
        };
        let mut world = ScaledClock::new();
        let mut local = world.new_local_clock();
        local.push_scale(ScaleSource::Gameplay, 333).expect("room");
        local.tick(&world);
        local.apply_hitstop(2);
        let mut expected = Vec::new();
        expected.extend_from_slice(&1u32.to_le_bytes()); // owner
        expected.extend_from_slice(&1u32.to_le_bytes()); // next serial
        expected.extend_from_slice(&1u32.to_le_bytes()); // scales
        expected.extend_from_slice(&0u32.to_le_bytes()); // the scale's serial
        expected.push(3); // Gameplay
        expected.extend_from_slice(&333u32.to_le_bytes());
        expected.extend_from_slice(&2u16.to_le_bytes()); // hitstop
        expected.extend_from_slice(&333u64.to_le_bytes()); // now
        expected.extend_from_slice(&333u64.to_le_bytes()); // last delta
        expected.extend_from_slice(&0u64.to_le_bytes()); // carry
        assert_eq!(bytes(&local), expected);

        // Two clocks that read alike but carry a different fraction aren't in the same state.
        let mut a = world.new_local_clock();
        let mut b = world.new_local_clock();
        let (id_a, id_b) = (bytes(&a), bytes(&b));
        assert_ne!(id_a, id_b, "the id counts");
        a.push_scale(ScaleSource::Debug, 100).expect("room");
        b.push_scale(ScaleSource::Debug, 100).expect("room");
        let scale = a.scales.factor();
        assert_eq!(scale, b.scales.factor());
        a.remainder = 1;
        assert_ne!(bytes(&a)[4..], bytes(&b)[4..], "the carry counts");
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
