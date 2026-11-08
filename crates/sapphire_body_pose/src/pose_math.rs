//! Numeric policy at the graph's host-math boundary. The fixed and legacy owners share evaluation.

use sapphire_math::spring::{damp, damp_deterministic};
use sapphire_math::{Quat, Transform, Vec2, Vec3, Vec3A, det};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PoseMath {
    #[default]
    Legacy,
    Pinned,
}

// Host-math calls belong only to the explicit legacy arms. The staged audit applies the
// simulation Clippy rule to the evaluator and keeps its remaining IK/FK debt visible.
#[allow(clippy::disallowed_methods)]
impl PoseMath {
    pub(crate) fn cos(self, angle: f32) -> f32 {
        match self {
            Self::Legacy => angle.cos(),
            Self::Pinned => det::cos(angle),
        }
    }

    pub(crate) fn sin_cos(self, angle: f32) -> (f32, f32) {
        match self {
            Self::Legacy => angle.sin_cos(),
            Self::Pinned => det::sin_cos(angle),
        }
    }

    pub(crate) fn atan2(self, y: f32, x: f32) -> f32 {
        match self {
            Self::Legacy => y.atan2(x),
            Self::Pinned => det::atan2(y, x),
        }
    }

    pub(crate) fn exp(self, value: f32) -> f32 {
        match self {
            Self::Legacy => value.exp(),
            Self::Pinned => det::exp(value),
        }
    }

    pub(crate) fn axis_angle(self, axis: Vec3, angle: f32) -> Quat {
        match self {
            Self::Legacy => Quat::from_axis_angle(axis, angle),
            Self::Pinned => det::quat_from_axis_angle(axis, angle),
        }
    }

    pub(crate) fn normalize(self, rotation: Quat) -> Quat {
        match self {
            Self::Legacy => rotation.normalize(),
            Self::Pinned => det::quat_normalize(rotation),
        }
    }

    pub(crate) fn dot(self, a: Quat, b: Quat) -> f32 {
        match self {
            Self::Legacy => a.dot(b),
            Self::Pinned => (a.x * b.x + a.y * b.y) + (a.z * b.z + a.w * b.w),
        }
    }

    pub(crate) fn multiply(self, a: Quat, b: Quat) -> Quat {
        match self {
            Self::Legacy => a * b,
            Self::Pinned => det::quat_mul(a, b),
        }
    }

    pub(crate) fn rotate(self, rotation: Quat, point: Vec3) -> Vec3 {
        match self {
            Self::Legacy => rotation * point,
            Self::Pinned => det::quat_rotate(rotation, point),
        }
    }

    pub(crate) fn sin(self, angle: f32) -> f32 {
        match self {
            Self::Legacy => angle.sin(),
            Self::Pinned => det::sin(angle),
        }
    }

    pub(crate) fn acos(self, value: f32) -> f32 {
        match self {
            Self::Legacy => value.acos(),
            Self::Pinned => det::acos(value),
        }
    }

    pub(crate) fn powf(self, base: f32, power: f32) -> f32 {
        match self {
            Self::Legacy => base.powf(power),
            Self::Pinned => det::pow(base, power),
        }
    }

    pub(crate) fn hypot(self, x: f32, y: f32) -> f32 {
        match self {
            Self::Legacy => x.hypot(y),
            Self::Pinned => det::hypot(x, y),
        }
    }

    pub(crate) fn length_3a(self, value: Vec3A) -> f32 {
        match self {
            Self::Legacy => value.length(),
            Self::Pinned => Vec3::from(value).length(),
        }
    }

    pub(crate) fn normalize_3a(self, value: Vec3A) -> Vec3A {
        match self {
            Self::Legacy => value.normalize(),
            Self::Pinned => Vec3::from(value).normalize().into(),
        }
    }

    pub(crate) fn rotation_x(self, angle: f32) -> Quat {
        match self {
            Self::Legacy => Quat::from_rotation_x(angle),
            Self::Pinned => det::quat_from_rotation_x(angle),
        }
    }

    pub(crate) fn rotation_y(self, angle: f32) -> Quat {
        match self {
            Self::Legacy => Quat::from_rotation_y(angle),
            Self::Pinned => det::quat_from_rotation_y(angle),
        }
    }

    pub(crate) fn rotation_arc(self, from: Vec3, to: Vec3) -> Quat {
        match self {
            Self::Legacy => Quat::from_rotation_arc(from, to),
            Self::Pinned => det::quat_from_rotation_arc(from, to),
        }
    }

    pub(crate) fn scaled_axis(self, axis: Vec3) -> Quat {
        match self {
            Self::Legacy => Quat::from_scaled_axis(axis),
            Self::Pinned => {
                let angle = axis.length();
                if angle == 0.0 { Quat::IDENTITY } else { det::quat_from_axis_angle(axis / angle, angle) }
            }
        }
    }

    pub(crate) fn to_axis_angle(self, rotation: Quat) -> (Vec3, f32) {
        match self {
            Self::Legacy => rotation.to_axis_angle(),
            Self::Pinned => {
                // These solvers use the input quaternion's hemisphere. Preserve the full
                // axis-angle range when limiting rotation rather than wrapping at pi.
                let axis = rotation.xyz();
                let length = axis.length();
                if length >= 1e-8 {
                    (axis / length, 2.0 * det::atan2(length, rotation.w))
                } else {
                    (Vec3::X, 0.0)
                }
            }
        }
    }

    pub(crate) fn slerp(self, from: Quat, to: Quat, fraction: f32) -> Quat {
        match self {
            Self::Legacy => from.slerp(to, fraction),
            Self::Pinned => det::quat_slerp(from, to, fraction),
        }
    }

    pub(crate) fn rotation_length_squared(self, rotation: Quat) -> f32 {
        match self {
            Self::Legacy => rotation.length_squared(),
            Self::Pinned => self.dot(rotation, rotation),
        }
    }

    pub(crate) fn transform_point(self, frame: &Transform, point: Vec3) -> Vec3 {
        frame.translation + self.rotate(frame.rotation, frame.scale * point)
    }

    pub(crate) fn nlerp(self, from: Quat, to: Quat, fraction: f32) -> Quat {
        match self {
            Self::Legacy => from.lerp(to, fraction),
            Self::Pinned => det::quat_nlerp(from, to, fraction),
        }
    }

    pub(crate) fn square(self, value: f32) -> f32 {
        match self {
            Self::Legacy => value.powi(2),
            Self::Pinned => value * value,
        }
    }

    pub(crate) fn damp_f32(
        self,
        current: f32,
        target: f32,
        velocity: &mut f32,
        half_life: f32,
        dt: f32,
    ) -> f32 {
        match self {
            Self::Legacy => damp(current, target, velocity, half_life, dt),
            Self::Pinned => damp_deterministic(current, target, velocity, half_life, dt),
        }
    }

    pub(crate) fn damp_vec2(
        self,
        current: Vec2,
        target: Vec2,
        velocity: &mut Vec2,
        half_life: f32,
        dt: f32,
    ) -> Vec2 {
        match self {
            Self::Legacy => damp(current, target, velocity, half_life, dt),
            Self::Pinned => Vec2::new(
                damp_deterministic(current.x, target.x, &mut velocity.x, half_life, dt),
                damp_deterministic(current.y, target.y, &mut velocity.y, half_life, dt),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_vector_spring_matches_the_shared_deterministic_scalar_kernel() {
        let mut velocity = Vec2::new(0.3, -0.6);
        let mut scalar_velocity = velocity;
        let current = Vec2::new(-1.4, 0.02);
        let target = Vec2::new(3.2, -0.71);
        let actual = PoseMath::Pinned.damp_vec2(current, target, &mut velocity, 4.0, 0.25);
        let expected = Vec2::new(
            damp_deterministic(current.x, target.x, &mut scalar_velocity.x, 4.0, 0.25),
            damp_deterministic(current.y, target.y, &mut scalar_velocity.y, 4.0, 0.25),
        );
        assert_eq!(actual.to_array().map(f32::to_bits), expected.to_array().map(f32::to_bits));
        assert_eq!(velocity.to_array().map(f32::to_bits), scalar_velocity.to_array().map(f32::to_bits));
    }
}
