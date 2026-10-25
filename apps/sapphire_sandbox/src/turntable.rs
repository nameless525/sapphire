//! The sandbox's simulation: a turntable that spins the model on the fixed 60 Hz step.
//!
//! It reads time only from its entity's [`EntityClock`], so world slow motion and local hitstop
//! compose exactly as they will in combat (spec §4.5).

use std::f32::consts::TAU;

use sapphire_ecs::prelude::*;

/// Radians per second of turntable time.
const SPEED: f32 = 0.35;

#[derive(Component, Debug, Default)]
pub(crate) struct Turntable {
    /// Angle after the latest fixed step, kept in [0, TAU).
    angle: f32,
    /// Angle after the step before, possibly negative after wrapping, so interpolation never
    /// sweeps the long way round.
    previous: f32,
}

impl Turntable {
    /// Advances by `seconds` of turntable time.
    fn advance(&mut self, seconds: f32) {
        self.previous = self.angle;
        self.angle += SPEED * seconds;
        if self.angle >= TAU {
            self.angle -= TAU;
            self.previous -= TAU;
        }
    }

    /// The angle to draw, `alpha` of the way from the previous step to the latest.
    pub fn angle(&self, alpha: f32) -> f32 {
        self.previous + (self.angle - self.previous) * alpha
    }
}

/// Spins every turntable by its clock's advance this step. Runs in `FixedCombat`, after the
/// clocks ticked.
pub(crate) fn spin(mut tables: Query<'_, '_, (&mut Turntable, &EntityClock)>) {
    for (mut table, clock) in &mut tables {
        table.advance(clock.0.last_delta().as_secs_f32());
    }
}

#[cfg(test)]
mod tests {
    use sapphire_time::{SimDuration, TICK_HZ};

    use super::*;

    #[test]
    fn spins_at_speed_and_wraps() {
        let tick = SimDuration::from_ticks(1).as_secs_f32();
        let mut table = Turntable::default();
        for _ in 0..TICK_HZ {
            table.advance(tick);
        }
        assert!((table.angle(1.0) - SPEED).abs() < 1e-4);

        let ticks_per_turn = (TAU / SPEED * TICK_HZ as f32).ceil() as u32;
        for _ in 0..ticks_per_turn {
            table.advance(tick);
        }
        assert!((0.0..TAU).contains(&table.angle(1.0)));
        assert!(table.angle(0.0) < table.angle(1.0), "interpolation stays on the short arc");
    }
}
