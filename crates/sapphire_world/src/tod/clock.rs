//! [`TodClock`]: time of day's own clock domain (spec §4.5).

use sapphire_time::{
    LocalClock, SUBTICKS_PER_TICK, ScaleHandle, ScaleSource, ScaledClock, SimDuration, SimTime, TICK_HZ,
};

use super::timeline::DAY_HOURS;

const SUBTICKS_PER_SECOND: f64 = (SUBTICKS_PER_TICK as u64 * TICK_HZ as u64) as f64;

/// The time of day, as a clock domain of its own (spec §4.5): a [`LocalClock`] issued by the
/// world clock, so pause, slow motion and cinematic scales on the world reach it, plus scales of
/// its own (a developer pausing the cycle), and a cycle rate that turns its seconds into hours.
///
/// The hour is computed from integer clock time every time it is read, never accumulated, so
/// it doesn't drift however long the session: `anchor hours + elapsed seconds × rate`, wrapped
/// into the day. With no cycle (the rate 0) it is exactly the hour it was set to.
#[derive(Clone, Debug)]
pub struct TodClock {
    clock: LocalClock,
    /// The hour at `anchor` on `clock`.
    anchor_hours: f64,
    anchor: SimTime,
    /// Hours per second of this clock's time: 24 / the day's length, or 0 without a cycle.
    hours_per_second: f64,
    /// The developer pause's scale on `clock`.
    pause: Option<ScaleHandle>,
}

impl TodClock {
    /// A clock showing `hours` (wrapped into the day), with no cycle: time stands still until
    /// [`Self::set_day_length`] starts one.
    pub fn new(clock: LocalClock, hours: f32) -> Self {
        let anchor = clock.now();
        Self { clock, anchor_hours: wrap(f64::from(hours)), anchor, hours_per_second: 0.0, pause: None }
    }

    /// Advances one fixed step: call after the world clock ticks. Allocates nothing.
    pub fn tick(&mut self, world: &ScaledClock) {
        self.clock.tick(world);
    }

    /// The hour, `0 ≤ hours < 24`, `alpha` of the way from the last fixed step to the next (as
    /// presentation interpolates everything it draws).
    pub fn hours(&self, alpha: f32) -> f32 {
        let elapsed = seconds(self.clock.now() - self.anchor) + f64::from(alpha) * self.step_seconds();
        let hours = wrap(self.anchor_hours + elapsed * self.hours_per_second) as f32;
        // Rounding to f32 can land on 24 itself, which is the next midnight.
        if hours >= DAY_HOURS { 0.0 } else { hours }
    }

    /// Jumps to `hours` (wrapped into the day). The cycle, if any, carries on from there.
    pub fn set_hours(&mut self, hours: f32) {
        if hours.is_finite() {
            self.anchor = self.clock.now();
            self.anchor_hours = wrap(f64::from(hours));
        }
    }

    /// The length of a day in seconds of this clock's time, or `None` while time stands still.
    pub fn day_length(&self) -> Option<f32> {
        (self.hours_per_second > 0.0).then(|| (f64::from(DAY_HOURS) / self.hours_per_second) as f32)
    }

    /// Runs a day in `seconds` of this clock's time from the current hour, or stops the cycle
    /// with `None` (or a length that isn't positive and finite).
    pub fn set_day_length(&mut self, seconds: Option<f32>) {
        let now = self.hours(0.0);
        self.anchor = self.clock.now();
        self.anchor_hours = f64::from(now);
        self.hours_per_second = match seconds {
            Some(seconds) if seconds.is_finite() && seconds > 0.0 => {
                f64::from(DAY_HOURS) / f64::from(seconds)
            }
            _ => 0.0,
        };
    }

    /// Whether the developer pause holds the clock (the world's own pause is separate).
    pub fn is_paused(&self) -> bool {
        self.pause.is_some()
    }

    /// Holds or releases the clock with a [`ScaleSource::Debug`] scale of 0 on its own stack,
    /// leaving the world clock alone. Returns false if the stack had no room. Allocates nothing.
    pub fn set_paused(&mut self, paused: bool) -> bool {
        match (paused, self.pause) {
            (true, None) => match self.clock.push_scale(ScaleSource::Debug, 0) {
                Ok(handle) => self.pause = Some(handle),
                Err(_) => return false,
            },
            (false, Some(handle)) => {
                self.clock.pop_scale(handle);
                self.pause = None;
            }
            _ => {}
        }
        true
    }

    /// How fast the clock runs against real time: the world's scales times its own.
    pub fn speed(&self, world: &ScaledClock) -> f32 {
        self.clock.effective_scale(world)
    }

    /// The underlying clock.
    pub fn clock(&self) -> &LocalClock {
        &self.clock
    }

    /// Seconds the last fixed step advanced this clock.
    pub(crate) fn step_seconds(&self) -> f64 {
        seconds(self.clock.last_delta())
    }
}

fn seconds(duration: SimDuration) -> f64 {
    duration.as_subticks() as f64 / SUBTICKS_PER_SECOND
}

fn wrap(hours: f64) -> f64 {
    if hours.is_finite() { hours.rem_euclid(f64::from(DAY_HOURS)) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(hours: f32) -> (ScaledClock, TodClock) {
        let mut world = ScaledClock::new();
        let local = world.new_local_clock();
        (world, TodClock::new(local, hours))
    }

    fn run(world: &mut ScaledClock, tod: &mut TodClock, ticks: u32) {
        for _ in 0..ticks {
            world.tick();
            tod.tick(world);
        }
    }

    #[test]
    fn without_a_cycle_the_hour_is_exactly_what_was_set() {
        let (mut world, mut tod) = clock(10.0);
        run(&mut world, &mut tod, 600);
        assert_eq!(tod.hours(0.0), 10.0);
        assert_eq!(tod.hours(0.7), 10.0);
        assert_eq!(tod.day_length(), None);
        assert_eq!(clock(-1.0).1.hours(0.0), 23.0, "wrapped into the day");
        assert_eq!(clock(48.5).1.hours(0.0), 0.5);
    }

    #[test]
    fn a_cycle_runs_the_day_and_wraps_at_midnight() {
        let (mut world, mut tod) = clock(23.0);
        // A day in 24 seconds: an hour a second.
        tod.set_day_length(Some(24.0));
        assert_eq!(tod.day_length(), Some(24.0));
        run(&mut world, &mut tod, 30);
        assert!((tod.hours(0.0) - 23.5).abs() < 1e-5);
        // Interpolated between steps: one tick is 1/60 hour here.
        assert!((tod.hours(0.5) - (23.5 + 0.5 / 60.0)).abs() < 1e-5);
        run(&mut world, &mut tod, 60);
        assert!((tod.hours(0.0) - 0.5).abs() < 1e-5, "past midnight: {}", tod.hours(0.0));
        // Changing the length keeps the hour.
        tod.set_day_length(Some(48.0));
        assert!((tod.hours(0.0) - 0.5).abs() < 1e-5);
        run(&mut world, &mut tod, 60);
        assert!((tod.hours(0.0) - 1.0).abs() < 1e-5);
        tod.set_day_length(None);
        run(&mut world, &mut tod, 60);
        assert!((tod.hours(0.0) - 1.0).abs() < 1e-5, "stopped");
    }

    #[test]
    fn the_world_and_the_developer_pause_both_hold_it() {
        let (mut world, mut tod) = clock(6.0);
        tod.set_day_length(Some(24.0));
        let pause = world.push_scale(ScaleSource::Pause, 0).expect("room");
        run(&mut world, &mut tod, 60);
        assert_eq!(tod.hours(1.0), 6.0, "the world is paused");
        world.pop_scale(pause);
        assert!(tod.set_paused(true) && tod.is_paused());
        run(&mut world, &mut tod, 60);
        assert_eq!(tod.hours(1.0), 6.0, "the cycle is paused");
        assert_eq!(world.ticks(), 120);
        tod.set_paused(false);
        world.push_scale(ScaleSource::PerfectDodge, 500).expect("room");
        run(&mut world, &mut tod, 60);
        assert!((tod.hours(0.0) - 6.5).abs() < 1e-5, "slow motion slows it: {}", tod.hours(0.0));
        tod.set_hours(18.0);
        assert_eq!(tod.hours(0.0), 18.0);
    }

    #[test]
    fn hours_never_drift() {
        // Two hours of a 24-minute day at 60 Hz: 432,000 steps, read from integer time.
        let (mut world, mut tod) = clock(0.0);
        tod.set_day_length(Some(24.0 * 60.0));
        run(&mut world, &mut tod, 432_000);
        assert!(
            (tod.hours(0.0) - 0.0).abs() < 1e-4 || (tod.hours(0.0) - 24.0).abs() < 1e-4,
            "{}",
            tod.hours(0.0)
        );
    }
}
