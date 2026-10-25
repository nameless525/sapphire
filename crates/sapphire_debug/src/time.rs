//! [`TimeControls`]: pause, single-step and speed for the world clock (spec §5.20), through its
//! time-scale stack.

use sapphire_time::{NORMAL_SPEED, ScaleHandle, ScaleSource, ScaledClock};

/// The speeds the time panel offers, in per-mille.
pub const SPEEDS: [u32; 7] = [50, 100, 250, 500, 1000, 2000, 4000];

/// Developer control of the world clock: one [`ScaleSource::Debug`] entry on its scale stack,
/// pushed and popped as the controls change, so it composes with every other scale (pause,
/// perfect dodge, cinematics) instead of overriding them.
///
/// The app calls [`Self::before_fixed_step`] before every fixed step. Changes apply from the
/// next step, and a single step while paused advances the world exactly one tick at normal
/// speed, then stops it again. It never allocates, so the hook may run inside the fixed step's
/// allocation check (rule R6).
#[derive(Clone, Debug)]
pub struct TimeControls {
    paused: bool,
    /// While running, per-mille.
    speed: u32,
    /// Ticks still to run while paused.
    steps: u32,
    /// The scale this pushed, and its factor.
    applied: Option<(ScaleHandle, u32)>,
    /// A push failed because the clock's stack was full (a bug elsewhere: a scale never popped).
    stack_full: bool,
}

impl Default for TimeControls {
    fn default() -> Self {
        Self { paused: false, speed: NORMAL_SPEED, steps: 0, applied: None, stack_full: false }
    }
}

impl TimeControls {
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Pausing drops any steps still pending.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.steps = 0;
    }

    /// Runs `ticks` more fixed steps of world time, then stays paused. Pauses if running.
    pub fn step(&mut self, ticks: u32) {
        if !self.paused {
            self.set_paused(true);
        }
        self.steps = self.steps.saturating_add(ticks);
    }

    /// The world speed while running, per-mille ([`NORMAL_SPEED`] is 1×).
    pub fn speed(&self) -> u32 {
        self.speed
    }

    pub fn set_speed(&mut self, per_mille: u32) {
        self.speed = per_mille;
    }

    /// Whether these controls currently change the world's speed at all.
    pub fn is_active(&self) -> bool {
        self.paused || self.speed != NORMAL_SPEED
    }

    /// Whether a scale couldn't be pushed because the clock's stack was full.
    pub fn stack_full(&self) -> bool {
        self.stack_full
    }

    /// Brings the clock's debug scale in line with the controls for the step about to run.
    pub fn before_fixed_step(&mut self, clock: &mut ScaledClock) {
        let wanted = match (self.paused, self.steps) {
            (false, _) => self.speed,
            (true, 0) => 0,
            (true, _) => {
                self.steps -= 1;
                NORMAL_SPEED
            }
        };
        self.apply(clock, wanted);
    }

    /// Takes the controls' scale off the clock (when the controls go away). The controls keep
    /// their settings and re-apply them at the next [`Self::before_fixed_step`].
    pub fn release(&mut self, clock: &mut ScaledClock) {
        self.apply(clock, NORMAL_SPEED);
    }

    fn apply(&mut self, clock: &mut ScaledClock, per_mille: u32) {
        let current = self.applied.map_or(NORMAL_SPEED, |(_, factor)| factor);
        if per_mille == current {
            return;
        }
        if let Some((handle, _)) = self.applied.take() {
            clock.pop_scale(handle);
        }
        if per_mille != NORMAL_SPEED {
            match clock.push_scale(ScaleSource::Debug, per_mille) {
                Ok(handle) => self.applied = Some((handle, per_mille)),
                Err(_) => self.stack_full = true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sapphire_time::{SimDuration, SimTime, TimeScale};

    use super::*;

    fn tick(controls: &mut TimeControls, clock: &mut ScaledClock, steps: u32) -> SimDuration {
        let start = clock.now();
        for _ in 0..steps {
            controls.before_fixed_step(clock);
            clock.tick();
        }
        clock.now() - start
    }

    #[test]
    fn pause_stops_the_world_and_steps_run_one_tick_each() {
        let (mut controls, mut clock) = (TimeControls::default(), ScaledClock::new());
        assert_eq!(tick(&mut controls, &mut clock, 3), SimDuration::from_ticks(3));
        assert_eq!(clock.scales().len(), 0, "nothing pushed at normal speed");

        controls.set_paused(true);
        assert_eq!(tick(&mut controls, &mut clock, 10), SimDuration::ZERO);
        assert!(clock.is_stopped());
        controls.step(1);
        assert_eq!(tick(&mut controls, &mut clock, 5), SimDuration::from_ticks(1), "one tick, then paused");
        controls.step(2);
        assert_eq!(tick(&mut controls, &mut clock, 5), SimDuration::from_ticks(2));
        assert!(controls.is_paused());

        controls.set_paused(false);
        assert_eq!(tick(&mut controls, &mut clock, 2), SimDuration::from_ticks(2));
        assert_eq!(clock.scales().len(), 0, "the pause was popped");
        assert_eq!(clock.now(), SimTime::ZERO + SimDuration::from_ticks(8));
    }

    #[test]
    fn speed_composes_with_other_scales() {
        let (mut controls, mut clock) = (TimeControls::default(), ScaledClock::new());
        let dodge = clock.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
        controls.set_speed(500);
        assert_eq!(tick(&mut controls, &mut clock, 10), SimDuration::from_ticks(1), "0.2 × 0.5 = 0.1×");
        let sources: Vec<TimeScale> = clock.scales().collect();
        assert_eq!(sources[1], TimeScale { source: ScaleSource::Debug, per_mille: 500 });

        // Stepping while paused runs at normal speed, under the other scales.
        controls.step(5);
        tick(&mut controls, &mut clock, 5);
        assert_eq!(clock.last_delta(), SimDuration::from_subticks(200), "a fifth of a tick");
        clock.pop_scale(dodge);
        controls.set_paused(false);
        controls.release(&mut clock);
        assert_eq!(clock.scales().len(), 0);
        assert!(controls.is_active(), "the setting stays for the next step");
        assert_eq!(tick(&mut controls, &mut clock, 2), SimDuration::from_ticks(1));
    }

    #[test]
    fn a_full_stack_is_reported_not_fatal() {
        let (mut controls, mut clock) = (TimeControls::default(), ScaledClock::new());
        for _ in 0..ScaledClock::SCALE_CAPACITY {
            clock.push_scale(ScaleSource::Gameplay, NORMAL_SPEED).expect("room");
        }
        controls.set_paused(true);
        tick(&mut controls, &mut clock, 1);
        assert!(controls.stack_full() && !clock.is_stopped());
    }
}
