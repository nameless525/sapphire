//! [`Clock`], [`Timer`] and [`Cooldown`].
//!
//! Timers and cooldowns store deadlines rather than counting down, so they cost nothing per
//! tick, and they follow whichever clock they are read against: on an entity's [`LocalClock`]
//! a cooldown freezes during that entity's hitstop and keeps running for off-field party
//! members (spec §5.22). Always read a timer against the clock it was started on.

use crate::{LocalClock, RealClock, ScaledClock, SimDuration, SimTime};

/// Anything that reports a current [`SimTime`].
pub trait Clock {
    /// The current time on this clock.
    fn now(&self) -> SimTime;
}

impl Clock for ScaledClock {
    fn now(&self) -> SimTime {
        ScaledClock::now(self)
    }
}

impl Clock for LocalClock {
    fn now(&self) -> SimTime {
        LocalClock::now(self)
    }
}

impl Clock for RealClock {
    /// Wall time since the clock started, as of the last update, in sub-ticks.
    fn now(&self) -> SimTime {
        SimTime::ZERO + SimDuration::from_duration(self.elapsed())
    }
}

/// A span of time started at a point on some clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Timer {
    start: SimTime,
    duration: SimDuration,
}

impl Timer {
    /// Starts a timer of `duration` at `clock`'s current time.
    #[must_use]
    pub fn start(clock: &impl Clock, duration: SimDuration) -> Self {
        Self { start: clock.now(), duration }
    }

    /// Restarts with the same duration from `clock`'s current time.
    pub fn restart(&mut self, clock: &impl Clock) {
        self.start = clock.now();
    }

    /// The full length.
    #[must_use]
    pub fn duration(&self) -> SimDuration {
        self.duration
    }

    /// Time since the start, which keeps growing past the duration.
    #[must_use]
    pub fn elapsed(&self, clock: &impl Clock) -> SimDuration {
        clock.now().since(self.start)
    }

    /// Time left, or zero once finished.
    #[must_use]
    pub fn remaining(&self, clock: &impl Clock) -> SimDuration {
        self.duration - self.elapsed(clock)
    }

    /// Whether the full duration has passed.
    #[must_use]
    pub fn is_finished(&self, clock: &impl Clock) -> bool {
        self.elapsed(clock) >= self.duration
    }

    /// Progress in [0, 1]; a zero-length timer is always 1.
    #[must_use]
    pub fn fraction(&self, clock: &impl Clock) -> f32 {
        if self.duration == SimDuration::ZERO {
            return 1.0;
        }
        let ratio = self.elapsed(clock).as_subticks() as f64 / self.duration.as_subticks() as f64;
        ratio.min(1.0) as f32
    }
}

/// Gates an action until a deadline on some clock. Starts ready.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cooldown {
    ready_at: SimTime,
}

impl Cooldown {
    /// A cooldown that is ready immediately.
    pub const READY: Self = Self { ready_at: SimTime::ZERO };

    /// Whether the action may be used now.
    #[must_use]
    pub fn is_ready(&self, clock: &impl Clock) -> bool {
        clock.now() >= self.ready_at
    }

    /// Time until ready, or zero.
    #[must_use]
    pub fn remaining(&self, clock: &impl Clock) -> SimDuration {
        self.ready_at.since(clock.now())
    }

    /// Starts the cooldown from now, whether or not it was ready.
    pub fn trigger(&mut self, clock: &impl Clock, duration: SimDuration) {
        self.ready_at = clock.now() + duration;
    }

    /// If ready, starts the cooldown and returns `true`; otherwise leaves it alone and returns
    /// `false`. The usual check-and-use for abilities.
    pub fn try_trigger(&mut self, clock: &impl Clock, duration: SimDuration) -> bool {
        let ready = self.is_ready(clock);
        if ready {
            self.trigger(clock, duration);
        }
        ready
    }

    /// Makes the cooldown ready immediately.
    pub fn reset(&mut self) {
        *self = Self::READY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScaleSource;

    fn tick_world(world: &mut ScaledClock, ticks: u32) {
        for _ in 0..ticks {
            world.tick();
        }
    }

    #[test]
    fn timer_on_the_world_clock() {
        let mut world = ScaledClock::new();
        tick_world(&mut world, 5);
        let timer = Timer::start(&world, SimDuration::from_ticks(10));
        assert_eq!(timer.duration(), SimDuration::from_ticks(10));
        assert!(!timer.is_finished(&world));
        assert_eq!(timer.fraction(&world), 0.0);
        tick_world(&mut world, 4);
        assert_eq!(timer.elapsed(&world), SimDuration::from_ticks(4));
        assert_eq!(timer.remaining(&world), SimDuration::from_ticks(6));
        assert_eq!(timer.fraction(&world), 0.4);
        tick_world(&mut world, 6);
        assert!(timer.is_finished(&world));
        assert_eq!(timer.remaining(&world), SimDuration::ZERO);
        tick_world(&mut world, 6);
        assert_eq!(timer.fraction(&world), 1.0);
        assert_eq!(timer.elapsed(&world), SimDuration::from_ticks(16));
    }

    #[test]
    fn timer_respects_slow_motion_and_restart() {
        let mut world = ScaledClock::new();
        let mut timer = Timer::start(&world, SimDuration::from_ticks(2));
        world.push_scale(ScaleSource::PerfectDodge, 250).expect("room");
        tick_world(&mut world, 7);
        assert!(!timer.is_finished(&world));
        tick_world(&mut world, 1);
        assert!(timer.is_finished(&world));
        timer.restart(&world);
        assert_eq!(timer.elapsed(&world), SimDuration::ZERO);
        assert_eq!(Timer::start(&world, SimDuration::ZERO).fraction(&world), 1.0);
    }

    #[test]
    fn cooldown_freezes_during_hitstop() {
        let mut world = ScaledClock::new();
        let mut owner = world.new_local_clock();
        let mut cooldown = Cooldown::READY;
        assert!(cooldown.try_trigger(&owner, SimDuration::from_ticks(3)));
        assert!(!cooldown.try_trigger(&owner, SimDuration::from_ticks(3)), "already cooling down");
        owner.tick(&world);
        owner.apply_hitstop(5);
        for _ in 0..5 {
            owner.tick(&world);
            assert_eq!(cooldown.remaining(&owner), SimDuration::from_ticks(2));
        }
        owner.tick(&world);
        owner.tick(&world);
        assert!(cooldown.is_ready(&owner));
        assert!(cooldown.try_trigger(&owner, SimDuration::from_ticks(1)));
        cooldown.reset();
        assert!(cooldown.is_ready(&owner));
        assert_eq!(Cooldown::default(), Cooldown::READY);
    }

    #[test]
    fn real_clock_is_a_clock() {
        let mut real = RealClock::new();
        let timer = Timer::start(&real, SimDuration::from_ticks(1_000_000));
        real.update();
        assert!(!timer.is_finished(&real));
        assert_eq!(Clock::now(&real), SimTime::ZERO + SimDuration::from_duration(real.elapsed()));
    }
}
