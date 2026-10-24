//! The sandbox's simulation: a turntable that spins the model on the fixed 60 Hz step.
//!
//! It reads time only from its own [`LocalClock`], so world slow motion and local hitstop
//! compose exactly as they will in combat (spec §4.5).

use std::f32::consts::TAU;

use sapphire_time::{LocalClock, ScaledClock};

/// Radians per second of turntable time.
const SPEED: f32 = 0.35;

pub(crate) struct Turntable {
    clock: LocalClock,
    /// Angle after the latest fixed step, kept in [0, TAU).
    angle: f32,
    /// Angle after the step before, possibly negative after wrapping, so interpolation never
    /// sweeps the long way round.
    previous: f32,
}

impl Turntable {
    pub fn new(world: &mut ScaledClock) -> Self {
        Self { clock: world.new_local_clock(), angle: 0.0, previous: 0.0 }
    }

    /// Advances one fixed step. Call after the world clock has ticked.
    pub fn step(&mut self, world: &ScaledClock) {
        let delta = self.clock.tick(world).as_secs_f32();
        self.previous = self.angle;
        self.angle += SPEED * delta;
        if self.angle >= TAU {
            self.angle -= TAU;
            self.previous -= TAU;
        }
    }

    /// Freezes the turntable for `ticks` fixed steps.
    pub fn hitstop(&mut self, ticks: u16) {
        self.clock.apply_hitstop(ticks);
    }

    /// The angle to draw, `alpha` of the way from the previous step to the latest.
    pub fn angle(&self, alpha: f32) -> f32 {
        self.previous + (self.angle - self.previous) * alpha
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sapphire_time::{ScaleSource, TICK_HZ};

    fn run(table: &mut Turntable, world: &mut ScaledClock, ticks: u32) {
        for _ in 0..ticks {
            world.tick();
            table.step(world);
        }
    }

    #[test]
    fn spins_at_speed_and_wraps() {
        let mut world = ScaledClock::new();
        let mut table = Turntable::new(&mut world);
        run(&mut table, &mut world, TICK_HZ);
        assert!((table.angle(1.0) - SPEED).abs() < 1e-4);

        let ticks_per_turn = (TAU / SPEED * TICK_HZ as f32).ceil() as u32;
        run(&mut table, &mut world, ticks_per_turn);
        assert!((0.0..TAU).contains(&table.angle(1.0)));
        assert!(table.angle(0.0) < table.angle(1.0), "interpolation stays on the short arc");
    }

    #[test]
    fn hitstop_and_slow_motion_scale_the_spin() {
        let mut world = ScaledClock::new();
        let mut table = Turntable::new(&mut world);
        table.hitstop(8);
        run(&mut table, &mut world, 8);
        assert_eq!(table.angle(1.0), 0.0);

        world.push_scale(ScaleSource::PerfectDodge, 200).unwrap();
        run(&mut table, &mut world, TICK_HZ);
        assert!((table.angle(1.0) - SPEED * 0.2).abs() < 1e-4);
    }
}
