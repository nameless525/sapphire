//! [`ScaledClock`]: the world clock.

use crate::scale::{self, ScaleStack};
use crate::{ClockId, LocalClock, ScaleHandle, ScaleSource, ScaleStackFull, SimDuration, SimTime, TimeScale};

/// Scale-stack capacity of the world clock.
const WORLD_SCALE_CAPACITY: usize = 8;

/// The world clock: advanced once per fixed step, scaled by the product of its [`TimeScale`]s.
///
/// Pausing pushes a [`ScaleSource::Pause`] scale of 0 rather than skipping fixed steps, so a
/// paused world still ticks (at zero speed) and entity hitstop stays frozen with it.
///
/// It also issues the entity clocks of its world ([`ScaledClock::new_local_clock`]), so every
/// clock has a [`ClockId`] that replays identically and scale handles can be checked against
/// the clock that issued them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScaledClock {
    scales: ScaleStack<WORLD_SCALE_CAPACITY>,
    now: SimTime,
    last_delta: SimDuration,
    /// Fraction of a sub-tick carried between ticks.
    remainder: u64,
    ticks: u64,
    next_local_id: u32,
}

impl ScaledClock {
    /// Maximum number of scales pushed at once.
    pub const SCALE_CAPACITY: usize = WORLD_SCALE_CAPACITY;

    /// A clock at time zero, running at normal speed.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            scales: ScaleStack::new(ClockId::WORLD),
            now: SimTime::ZERO,
            last_delta: SimDuration::ZERO,
            remainder: 0,
            ticks: 0,
            next_local_id: 1,
        }
    }

    /// Creates an entity clock for this world, at time zero with no scales or hitstop.
    ///
    /// Ids are issued in creation order, so a replay that creates clocks in the same order gets
    /// the same ids. They are never reused (until 2³² clocks), so a handle kept from a despawned
    /// entity cannot touch the clock of whatever spawns next.
    pub fn new_local_clock(&mut self) -> LocalClock {
        let id = ClockId(self.next_local_id);
        // Skip the world's id if the counter ever wraps.
        self.next_local_id = self.next_local_id.wrapping_add(1).max(1);
        LocalClock::new(id)
    }

    /// This clock's id, [`ClockId::WORLD`].
    #[must_use]
    pub fn id(&self) -> ClockId {
        self.scales.owner()
    }

    /// Pushes a scale; the clock runs at the product of all pushed scales from the next tick.
    /// Products are evaluated in insertion order, rounding down and capping at 10⁹× after
    /// each factor. A later slowdown scales the capped intermediate, not its lost excess.
    pub fn push_scale(&mut self, source: ScaleSource, per_mille: u32) -> Result<ScaleHandle, ScaleStackFull> {
        self.scales.push(source, per_mille)
    }

    /// Removes the scale `handle` refers to, wherever it sits in the stack. Returns `None` if it
    /// was already removed, or (failing a debug assertion) if another clock issued `handle`.
    pub fn pop_scale(&mut self, handle: ScaleHandle) -> Option<TimeScale> {
        self.scales.pop(handle)
    }

    /// Removes every scale pushed by `source` (for example all cinematic scales when a cutscene
    /// is skipped) and returns how many were removed.
    pub fn remove_source(&mut self, source: ScaleSource) -> usize {
        self.scales.remove_source(source)
    }

    /// The pushed scales, oldest first.
    pub fn scales(&self) -> impl ExactSizeIterator<Item = TimeScale> + '_ {
        self.scales.iter()
    }

    /// The combined speed as a float (1.0 is normal), for presentation such as audio pitch.
    #[must_use]
    pub fn effective_scale(&self) -> f32 {
        scale::factor_to_f32(self.scales.factor())
    }

    /// Whether the combined scale is exactly zero (paused or frozen).
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.scales.factor() == 0
    }

    /// Advances one fixed step and returns how much world time passed.
    pub fn tick(&mut self) -> SimDuration {
        let delta = SimDuration::from_subticks(scale::advance(self.scales.factor(), &mut self.remainder));
        let before = self.now;
        self.now += delta;
        // At the saturating timeline horizon only the representable advance occurred.
        // Presentation must not repeatedly interpolate a step that the clock cannot take.
        let delta = self.now - before;
        self.last_delta = delta;
        self.ticks += 1;
        delta
    }

    /// World time since the clock started.
    #[must_use]
    pub fn now(&self) -> SimTime {
        self.now
    }

    /// How much world time the most recent [`ScaledClock::tick`] advanced.
    #[must_use]
    pub fn last_delta(&self) -> SimDuration {
        self.last_delta
    }

    /// Fixed steps ticked so far, regardless of scale.
    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// This clock's time to draw, in ticks with a fraction: `alpha` of the way from its previous
    /// fixed step to its latest, where transforms draw (spec §4.5). A clock that didn't advance on
    /// its latest step (a pause, a stopped world) gives exactly [`Self::now`], whatever `alpha`.
    #[must_use]
    pub fn presented_ticks(&self, alpha: f32) -> f64 {
        crate::sim_time::presented_ticks(self.now, self.last_delta, alpha)
    }

    /// Feeds this clock's whole state to `sink` as little-endian bytes, as
    /// [`LocalClock::write_canonical`] does: the next entity clock's id included.
    pub fn write_canonical(&self, sink: &mut impl FnMut(&[u8])) {
        self.scales.write_canonical(sink);
        sink(&self.now.as_subticks().to_le_bytes());
        sink(&self.last_delta.as_subticks().to_le_bytes());
        sink(&self.remainder.to_le_bytes());
        sink(&self.ticks.to_le_bytes());
        sink(&self.next_local_id.to_le_bytes());
    }

    pub(crate) fn factor(&self) -> u128 {
        self.scales.factor()
    }
}

impl Default for ScaledClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_speed_is_one_tick_per_tick() {
        let mut clock = ScaledClock::new();
        for _ in 0..60 {
            assert_eq!(clock.tick(), SimDuration::from_ticks(1));
        }
        assert_eq!(clock.now(), SimTime::ZERO + SimDuration::from_ticks(60));
        assert_eq!(clock.ticks(), 60);
        assert_eq!(clock.effective_scale(), 1.0);
    }

    #[test]
    fn pause_stops_time_but_counts_ticks() {
        let mut clock = ScaledClock::new();
        let pause = clock.push_scale(ScaleSource::Pause, 0).expect("room");
        assert!(clock.is_stopped());
        for _ in 0..10 {
            assert_eq!(clock.tick(), SimDuration::ZERO);
        }
        assert_eq!(clock.now(), SimTime::ZERO);
        assert_eq!(clock.ticks(), 10);
        clock.pop_scale(pause);
        assert!(!clock.is_stopped());
        assert_eq!(clock.tick(), SimDuration::from_ticks(1));
    }

    #[test]
    fn canonical_state_is_pinned() {
        let mut clock = ScaledClock::new();
        clock.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
        clock.tick();
        let _ = clock.new_local_clock();
        let mut bytes = Vec::new();
        clock.write_canonical(&mut |chunk| bytes.extend_from_slice(chunk));
        let mut expected = Vec::new();
        expected.extend_from_slice(&0u32.to_le_bytes()); // owner: the world
        expected.extend_from_slice(&1u32.to_le_bytes()); // next serial
        expected.extend_from_slice(&1u32.to_le_bytes()); // scales
        expected.extend_from_slice(&0u32.to_le_bytes()); // the scale's serial
        expected.push(1); // PerfectDodge
        expected.extend_from_slice(&200u32.to_le_bytes());
        expected.extend_from_slice(&200u64.to_le_bytes()); // now
        expected.extend_from_slice(&200u64.to_le_bytes()); // last delta
        expected.extend_from_slice(&0u64.to_le_bytes()); // carry
        expected.extend_from_slice(&1u64.to_le_bytes()); // ticks
        expected.extend_from_slice(&2u32.to_le_bytes()); // next entity clock's id
        assert_eq!(bytes, expected);
    }

    #[test]
    fn presentation_stops_at_the_saturating_timeline_horizon() {
        let mut clock = ScaledClock::new();
        clock.now = SimTime::from_subticks(u64::MAX - 500);
        assert_eq!(clock.tick().as_subticks(), 500);
        assert_eq!(clock.now().as_subticks(), u64::MAX);
        assert_eq!(clock.tick(), SimDuration::ZERO);
        let end = clock.now().as_secs_f64() * 60.0;
        assert!((clock.presented_ticks(0.0) - end).abs() <= 4.0);
        assert_eq!(clock.presented_ticks(0.0), clock.presented_ticks(1.0));
    }

    #[test]
    fn capacity_is_enforced() {
        let mut clock = ScaledClock::new();
        for _ in 0..ScaledClock::SCALE_CAPACITY {
            clock.push_scale(ScaleSource::Debug, 1000).expect("room");
        }
        assert_eq!(
            clock.push_scale(ScaleSource::Debug, 1000),
            Err(ScaleStackFull { capacity: ScaledClock::SCALE_CAPACITY })
        );
        assert_eq!(clock.remove_source(ScaleSource::Debug), ScaledClock::SCALE_CAPACITY);
        assert_eq!(clock.scales().len(), 0);
    }
}
