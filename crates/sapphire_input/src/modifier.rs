//! Modifiers: per-binding value transforms, applied in order before triggers see the value.

use bevy_reflect::Reflect;
use sapphire_math::Vec2;
use serde::{Deserialize, Serialize};

/// A transform of a binding's value (Enhanced Input's modifiers). A binding applies its list
/// in order, and its trigger then judges the result.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum Modifier {
    /// Ignores magnitudes up to `lower` and saturates from `upper`, rescaling what lies between
    /// to 0..=1, so the output starts at zero right where the dead zone ends. Defaults: 0.2,
    /// 1.0, radial.
    DeadZone {
        #[serde(default = "default_lower")]
        lower: f32,
        #[serde(default = "default_upper")]
        upper: f32,
        #[serde(default)]
        shape: DeadZoneShape,
    },
    /// Raises the magnitude to `exponent`, keeping the direction: above 1 gives fine control
    /// near the center of a stick. Meant for sources in −1..=1.
    ResponseCurve { exponent: f32 },
    /// Multiplies each axis: sensitivity, units and inversion in one.
    Scale([f32; 2]),
    /// Flips the sign of the given axes.
    Negate(Axes),
    /// Swaps x and y, so a button or a 1D axis can drive the y of a 2D action (W in WASD).
    Swizzle,
    /// Multiplies by the seconds since the last update, turning a held key or stick into a
    /// rate (units per second) in an action that also takes per-update deltas.
    ScaleByDeltaTime,
}

/// How a [`Modifier::DeadZone`] measures the value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum DeadZoneShape {
    /// Each axis on its own: a square dead zone that snaps diagonals to the axes near the
    /// center.
    Axial,
    /// The vector's length: a round dead zone that keeps the direction.
    #[default]
    Radial,
}

/// Axes a modifier acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum Axes {
    X,
    Y,
    XY,
}

fn default_lower() -> f32 {
    0.2
}

fn default_upper() -> f32 {
    1.0
}

impl Modifier {
    /// Why the modifier's parameters are unusable, if they are.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        match *self {
            Self::DeadZone { lower, upper, .. } if !(0.0 <= lower && lower < upper && upper.is_finite()) => {
                Err("a dead zone needs 0 <= lower < upper")
            }
            Self::ResponseCurve { exponent } if !(exponent > 0.0 && exponent.is_finite()) => {
                Err("a response curve needs a positive exponent")
            }
            Self::Scale(factors) if !factors.iter().all(|f| f.is_finite()) => {
                Err("scale factors must be finite")
            }
            _ => Ok(()),
        }
    }

    /// Applies the modifier to `value`. `dt` is the update's length in seconds.
    pub(crate) fn apply(&self, value: Vec2, dt: f32) -> Vec2 {
        match *self {
            Self::DeadZone { lower, upper, shape } => {
                let rescale = |magnitude: f32| ((magnitude - lower) / (upper - lower)).clamp(0.0, 1.0);
                match shape {
                    DeadZoneShape::Axial => Vec2::new(
                        value.x.signum() * rescale(value.x.abs()),
                        value.y.signum() * rescale(value.y.abs()),
                    ),
                    DeadZoneShape::Radial => {
                        let length = value.length();
                        if length <= lower { Vec2::ZERO } else { value * (rescale(length) / length) }
                    }
                }
            }
            Self::ResponseCurve { exponent } => {
                let length = value.length();
                if length > 0.0 { value * (length.powf(exponent) / length) } else { value }
            }
            Self::Scale([x, y]) => value * Vec2::new(x, y),
            Self::Negate(axes) => match axes {
                Axes::X => Vec2::new(-value.x, value.y),
                Axes::Y => Vec2::new(value.x, -value.y),
                Axes::XY => -value,
            },
            Self::Swizzle => Vec2::new(value.y, value.x),
            Self::ScaleByDeltaTime => value * dt,
        }
    }
}

/// Applies `modifiers` in order.
pub(crate) fn apply_all(modifiers: &[Modifier], value: Vec2, dt: f32) -> Vec2 {
    modifiers.iter().fold(value, |value, modifier| modifier.apply(value, dt))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn axial_dead_zone_rescales_each_axis() {
        let zone = Modifier::DeadZone { lower: 0.15, upper: 1.0, shape: DeadZoneShape::Axial };
        assert_eq!(zone.apply(Vec2::new(0.1, -0.1), 0.0), Vec2::ZERO);
        assert!(close(zone.apply(Vec2::new(1.0, -1.0), 0.0), Vec2::new(1.0, -1.0)));
        // Halfway between the thresholds is half; the other axis is inside the zone.
        assert!(close(zone.apply(Vec2::new(0.575, 0.1), 0.0), Vec2::new(0.5, 0.0)));
        // Beyond `upper` saturates.
        let tight = Modifier::DeadZone { lower: 0.2, upper: 0.8, shape: DeadZoneShape::Axial };
        assert!(close(tight.apply(Vec2::new(0.9, 0.0), 0.0), Vec2::new(1.0, 0.0)));
        assert_eq!(zone.apply(Vec2::new(0.0, 0.0), 0.0), Vec2::ZERO, "no sign flip at rest");
    }

    #[test]
    fn radial_dead_zone_keeps_the_direction() {
        let zone = Modifier::DeadZone { lower: 0.2, upper: 1.0, shape: DeadZoneShape::Radial };
        // Axially each component (0.15) is inside 0.2, but the length (0.21) is outside.
        let diagonal = Vec2::new(0.15, 0.15);
        let out = zone.apply(diagonal, 0.0);
        assert!(out.length() > 0.0);
        assert!((out.normalize() - diagonal.normalize()).length() < 1e-5);
        assert_eq!(zone.apply(Vec2::new(0.1, 0.1), 0.0), Vec2::ZERO);
        assert!(close(zone.apply(Vec2::new(0.0, 0.6), 0.0), Vec2::new(0.0, 0.5)));
        assert!(close(zone.apply(Vec2::new(3.0, 4.0), 0.0), Vec2::new(0.6, 0.8)), "saturates at length 1");
    }

    #[test]
    fn response_curve_shapes_the_magnitude() {
        let curve = Modifier::ResponseCurve { exponent: 2.0 };
        assert!(close(curve.apply(Vec2::new(0.5, 0.0), 0.0), Vec2::new(0.25, 0.0)));
        assert!(close(curve.apply(Vec2::new(-0.5, 0.0), 0.0), Vec2::new(-0.25, 0.0)), "sign kept");
        assert!(close(curve.apply(Vec2::new(0.0, 1.0), 0.0), Vec2::new(0.0, 1.0)));
        assert_eq!(curve.apply(Vec2::ZERO, 0.0), Vec2::ZERO);
    }

    #[test]
    fn scale_negate_swizzle_and_delta_time() {
        let v = Vec2::new(2.0, -3.0);
        assert_eq!(Modifier::Scale([0.5, 2.0]).apply(v, 0.0), Vec2::new(1.0, -6.0));
        assert_eq!(Modifier::Negate(Axes::X).apply(v, 0.0), Vec2::new(-2.0, -3.0));
        assert_eq!(Modifier::Negate(Axes::Y).apply(v, 0.0), Vec2::new(2.0, 3.0));
        assert_eq!(Modifier::Negate(Axes::XY).apply(v, 0.0), Vec2::new(-2.0, 3.0));
        assert_eq!(Modifier::Swizzle.apply(v, 0.0), Vec2::new(-3.0, 2.0));
        assert_eq!(Modifier::ScaleByDeltaTime.apply(v, 0.25), Vec2::new(0.5, -0.75));
    }

    #[test]
    fn modifiers_apply_in_order() {
        // W in WASD: a button's (1, 0) becomes (0, speed) per second.
        let w = [Modifier::Swizzle, Modifier::Scale([0.0, 1.5]), Modifier::ScaleByDeltaTime];
        assert!(close(apply_all(&w, Vec2::X, 0.5), Vec2::new(0.0, 0.75)));
        // Order matters: scaling first, then swizzling, moves the factor to the other axis.
        let swapped = [Modifier::Scale([0.0, 1.5]), Modifier::Swizzle];
        assert_eq!(apply_all(&swapped, Vec2::X, 0.0), Vec2::ZERO);
    }

    #[test]
    fn invalid_parameters_are_rejected() {
        let zone = |lower, upper| Modifier::DeadZone { lower, upper, shape: DeadZoneShape::Radial };
        assert!(zone(0.2, 1.0).validate().is_ok());
        assert!(zone(0.5, 0.5).validate().is_err());
        assert!(zone(-0.1, 1.0).validate().is_err());
        assert!(Modifier::ResponseCurve { exponent: 0.0 }.validate().is_err());
        assert!(Modifier::Scale([f32::NAN, 1.0]).validate().is_err());
    }
}
