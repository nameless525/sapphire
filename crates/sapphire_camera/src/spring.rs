//! The camera's smoothing: critically damped springs whose update is exact for a target moving in a
//! straight line over the step.
//!
//! `sapphire_math::spring::damp` is exact for a target that holds still over a step. A target moving
//! every frame jumps at each step's start instead, which shortens the spring's lag by about half a
//! step's travel: at 6 m/s and a half-life of 0.1 s, it trails 9 cm less at 30 Hz than at 240, and
//! uneven frames make that lag wander by a centimetre from frame to frame. [`Follow`] solves the same
//! equation with the target ramping from where it was to where it is, so a target at a constant speed
//! is followed at the same lag (`2 v / rate`) at every frame rate and through uneven frames, and a
//! target at rest gives exactly what `damp` gives. Fighters' interpolated positions are straight lines
//! between ticks, so this is the right model for them, not only a closer one.

use sapphire_math::spring::SpringValue;

/// `u` with `(1 + u)·e^(−u) = ½`, as in `sapphire_math::spring`: a spring at rest closes half the gap
/// to a fixed target in one half-life.
const HALF_LIFE_RATE: f64 = 1.678_347;

/// A value following a target. See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Follow<T> {
    pub(crate) value: T,
    /// Units per second.
    pub(crate) velocity: T,
    /// The target at the end of the last step, where the next step's ramp starts.
    target: T,
}

impl<T: SpringValue> Follow<T> {
    /// At rest at `value`.
    pub(crate) fn new(value: T) -> Self {
        Self { value, velocity: T::ZERO, target: value }
    }

    /// Jumps to `value` and stops, for cuts and teleports.
    pub(crate) fn reset(&mut self, value: T) {
        *self = Self::new(value);
    }

    /// Advances by `dt` seconds toward `target`, which moved in a straight line from the last step's
    /// target, and returns the new value.
    ///
    /// A `half_life` of zero or less lands on the target. A `dt` of zero or less changes nothing, and
    /// the next step's ramp covers whatever the target did meanwhile.
    pub(crate) fn update(&mut self, target: T, half_life: f32, dt: f32) -> T {
        if half_life <= 0.0 {
            self.reset(target);
            return target;
        }
        if dt <= 0.0 {
            return self.value;
        }
        // In f64: the ramp's coefficient is a small difference of large terms when the step is short.
        let t = f64::from(dt);
        let rate = HALF_LIFE_RATE / f64::from(half_life);
        let u = rate * t;
        let decay = (-u).exp();
        let held = decay * (1.0 + u);
        let ramp_gain = t - 2.0 * (1.0 - decay) / rate + t * decay;

        let ramp = (target - self.target) * (1.0 / dt);
        let offset = self.value - self.target;
        self.value =
            self.target + offset * held as f32 + self.velocity * (t * decay) as f32 + ramp * ramp_gain as f32;
        self.velocity = self.velocity * (decay * (1.0 - u)) as f32 - offset * (rate * u * decay) as f32
            + ramp * (1.0 - held) as f32;
        self.target = target;
        self.value
    }

    /// Moves the value and its target by `by`: a yaw rewrapped by whole turns.
    pub(crate) fn shift(&mut self, by: T) {
        self.value = self.value + by;
        self.target = self.target + by;
    }
}

/// A first-order approach to a goal that moved in a straight line from `from` to `to` over `dt`
/// seconds, with `rate` in 1/s: exact, so the lag behind a steadily turning goal doesn't depend on the
/// frame rate. It has no velocity of its own: the [`Follow`] downstream smooths its starts and stops.
pub(crate) fn chase(value: f32, from: f32, to: f32, rate: f32, dt: f32) -> f32 {
    let u = f64::from(rate) * f64::from(dt);
    if u <= 0.0 {
        return value;
    }
    let decay = (-u).exp();
    let ramp_gain = f64::from(dt) - (1.0 - decay) / f64::from(rate);
    let ramp = f64::from(to - from) / f64::from(dt);
    (f64::from(from) + f64::from(value - from) * decay + ramp * ramp_gain) as f32
}

/// The first-order rate, in 1/s, that closes half a gap in `half_life` seconds.
pub(crate) fn rate_of(half_life: f32) -> f32 {
    std::f32::consts::LN_2 / half_life
}

#[cfg(test)]
mod tests {
    use sapphire_math::spring::damp;
    use sapphire_math::{Vec2, Vec3};

    use super::*;

    #[test]
    fn a_target_at_rest_matches_sapphire_maths_damp() {
        let target = Vec3::new(1.0, -2.0, 3.0);
        let velocity = Vec3::new(4.0, 0.5, -1.0);
        let mut follow = Follow { value: Vec3::ZERO, velocity, target };
        let mut plain = (Vec3::ZERO, velocity);
        for _ in 0..200 {
            let value = follow.update(target, 0.2, 1.0 / 60.0);
            plain.0 = damp(plain.0, target, &mut plain.1, 0.2, 1.0 / 60.0);
            assert!(value.abs_diff_eq(plain.0, 1e-4), "{value} vs {}", plain.0);
            assert!(follow.velocity.abs_diff_eq(plain.1, 1e-3));
        }
    }

    /// Why this exists: at a steady speed the lag is the same at any frame rate and through uneven
    /// frames, where `damp`'s changes with the frame rate.
    #[test]
    fn a_steady_target_is_followed_at_one_lag_whatever_the_steps() {
        const SPEED: f32 = 6.0;
        const HALF_LIFE: f32 = 0.1;
        let lag = 2.0 * SPEED * HALF_LIFE / HALF_LIFE_RATE as f32;
        let patterns: [&[f32]; 4] = [
            &[1.0 / 30.0],
            &[1.0 / 60.0],
            &[1.0 / 240.0],
            &[1.0 / 240.0, 1.0 / 30.0, 1.0 / 60.0, 1.0 / 20.0, 1.0 / 120.0],
        ];
        let mut plain_lags = (f32::MAX, f32::MIN);
        for pattern in patterns {
            let mut follow = Follow::new(0.0_f32);
            let (mut now, mut plain, mut plain_velocity) = (0.0_f32, 0.0_f32, 0.0_f32);
            while now < 3.0 {
                for &dt in pattern {
                    now += dt;
                    let target = now * SPEED;
                    follow.update(target, HALF_LIFE, dt);
                    plain = damp(plain, target, &mut plain_velocity, HALF_LIFE, dt);
                    if now > 2.0 {
                        let gap = target - follow.value;
                        assert!((gap - lag).abs() < 1e-3, "at {now}: lag {gap}, not {lag}");
                        plain_lags = (plain_lags.0.min(target - plain), plain_lags.1.max(target - plain));
                    }
                }
            }
        }
        assert!(plain_lags.1 - plain_lags.0 > 0.08, "damp's lag changes with the rate: {plain_lags:?}");
    }

    #[test]
    fn a_target_that_stops_is_reached_without_overshoot() {
        let mut follow = Follow::new(0.0_f32);
        let dt = 1.0 / 60.0;
        for frame in 1..=60 {
            follow.update(frame as f32 * dt * 8.0, 0.15, dt);
        }
        let end = 8.0;
        let mut last = follow.value;
        for _ in 0..600 {
            let value = follow.update(end, 0.15, dt);
            assert!(value <= end + 1e-4, "{value} overshot {end}");
            assert!(value >= last - 1e-4, "{value} fell back from {last}");
            last = value;
        }
        assert!((last - end).abs() < 1e-3);
    }

    #[test]
    fn steps_of_any_size_stay_finite_and_land() {
        for dt in [1e-7, 1e-3, 1.0, 1e6, f32::MAX] {
            let mut follow = Follow::new(Vec2::new(5.0, -5.0));
            follow.velocity = Vec2::splat(100.0);
            let value = follow.update(Vec2::new(-3.0, 9.0), 0.1, dt);
            assert!(value.is_finite() && follow.velocity.is_finite(), "{dt}: {value}");
            assert!(value.length() < 40.0, "{dt}: {value}");
        }
        let mut follow = Follow::new(0.0_f32);
        assert!((follow.update(3.0, 0.1, 1e6) - 3.0).abs() < 1e-3);
    }

    #[test]
    fn nothing_moves_without_time_and_a_zero_half_life_lands() {
        let mut follow = Follow::new(2.0_f32);
        assert_eq!(follow.update(9.0, 0.2, 0.0), 2.0);
        assert_eq!(follow.update(9.0, 0.2, -1.0), 2.0);
        assert_eq!(follow.update(9.0, 0.0, 0.0), 9.0);
        assert_eq!(follow.velocity, 0.0);
    }

    #[test]
    fn chase_is_exact_for_a_steady_goal_at_any_step() {
        const RATE: f32 = 7.0;
        const TURN: f32 = 1.5;
        for dt in [1.0_f32 / 30.0, 1.0 / 60.0, 1.0 / 144.0] {
            let (mut value, mut now) = (0.0_f32, 0.0_f32);
            for _ in 0..((3.0 / dt) as u32) {
                value = chase(value, now * TURN, (now + dt) * TURN, RATE, dt);
                now += dt;
            }
            let lag = now * TURN - value;
            assert!((lag - TURN / RATE).abs() < 1e-3, "{dt}: lag {lag}");
        }
        assert_eq!(chase(4.0, 1.0, 1.0, 0.0, 0.1), 4.0);
    }
}
