//! [`Transform`]: translation, rotation and scale.

use std::ops::Mul;

use glam::{Affine3A, Mat4, Quat, Vec3};

use crate::{FORWARD, RIGHT, UP};

/// A translation, rotation and scale, applied in the order scale → rotate → translate.
///
/// TRS is closed under composition and inversion only when scale is uniform. With non-uniform
/// scale, [`Transform::compose`] and [`Transform::inverse`] drop the shear the exact result would
/// need; use [`Affine3A`] (via [`Transform::to_affine3a`]) where that matters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Position, in meters.
    pub translation: Vec3,
    /// Orientation. Expected to be normalized.
    pub rotation: Quat,
    /// Per-axis scale.
    pub scale: Vec3,
}

impl Transform {
    /// No translation, rotation or scaling.
    pub const IDENTITY: Self = Self { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };

    /// A pure translation.
    #[must_use]
    pub const fn from_translation(translation: Vec3) -> Self {
        Self { translation, ..Self::IDENTITY }
    }

    /// A pure rotation.
    #[must_use]
    pub const fn from_rotation(rotation: Quat) -> Self {
        Self { rotation, ..Self::IDENTITY }
    }

    /// A pure scale.
    #[must_use]
    pub const fn from_scale(scale: Vec3) -> Self {
        Self { scale, ..Self::IDENTITY }
    }

    /// A rigid transform (unit scale).
    #[must_use]
    pub const fn from_translation_rotation(translation: Vec3, rotation: Quat) -> Self {
        Self { translation, rotation, scale: Vec3::ONE }
    }

    /// The equivalent 4×4 matrix.
    #[must_use]
    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// The equivalent affine transform, the fast form for transforming many points.
    #[must_use]
    pub fn to_affine3a(&self) -> Affine3A {
        Affine3A::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// `self` applied after `child`: the world transform of `child` when `self` is its parent.
    ///
    /// Equal to the matrix product `self.to_mat4() * child.to_mat4()` whenever `self` has uniform
    /// scale (or `child` has no rotation).
    #[must_use]
    pub fn compose(&self, child: &Self) -> Self {
        Self {
            translation: self.transform_point(child.translation),
            rotation: self.rotation * child.rotation,
            scale: self.scale * child.scale,
        }
    }

    /// The transform that undoes `self`, exact when scale is uniform (or rotation is identity).
    ///
    /// A zero scale component yields infinities.
    #[must_use]
    pub fn inverse(&self) -> Self {
        let rotation = self.rotation.inverse();
        let scale = self.scale.recip();
        Self { translation: -(rotation * (scale * self.translation)), rotation, scale }
    }

    /// Transforms a position: scale, rotate, then translate.
    #[must_use]
    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        self.translation + self.rotation * (self.scale * point)
    }

    /// Transforms a direction or offset: scale and rotate, without translating.
    #[must_use]
    pub fn transform_vector(&self, vector: Vec3) -> Vec3 {
        self.rotation * (self.scale * vector)
    }

    /// Interpolates translation and scale linearly and rotation spherically (shortest arc).
    ///
    /// This is how presentation blends the last two fixed-step states by `alpha` (spec §5.7).
    #[must_use]
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            translation: self.translation.lerp(other.translation, t),
            rotation: self.rotation.slerp(other.rotation, t),
            scale: self.scale.lerp(other.scale, t),
        }
    }

    /// The local forward axis ([`FORWARD`], +Z) in parent space: where the object faces.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        self.rotation * FORWARD
    }

    /// The local right axis ([`RIGHT`], −X) in parent space.
    #[must_use]
    pub fn right(&self) -> Vec3 {
        self.rotation * RIGHT
    }

    /// The local up axis ([`UP`], +Y) in parent space.
    #[must_use]
    pub fn up(&self) -> Vec3 {
        self.rotation * UP
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mul for Transform {
    type Output = Self;

    /// Same as [`Transform::compose`].
    fn mul(self, child: Self) -> Self {
        self.compose(&child)
    }
}

impl From<Transform> for Mat4 {
    fn from(transform: Transform) -> Self {
        transform.to_mat4()
    }
}

impl From<Transform> for Affine3A {
    fn from(transform: Transform) -> Self {
        transform.to_affine3a()
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    use super::*;

    const EPS: f32 = 1e-5;

    fn sample() -> Transform {
        Transform {
            translation: Vec3::new(1.0, -2.0, 3.5),
            rotation: Quat::from_euler(glam::EulerRot::YXZ, 0.7, -0.3, 1.1),
            scale: Vec3::splat(2.5),
        }
    }

    fn assert_transform_near(a: &Transform, b: &Transform) {
        assert!(a.translation.abs_diff_eq(b.translation, EPS), "{a:?} vs {b:?}");
        // q and −q are the same rotation.
        assert!(a.rotation.dot(b.rotation).abs() > 1.0 - EPS, "{a:?} vs {b:?}");
        assert!(a.scale.abs_diff_eq(b.scale, EPS), "{a:?} vs {b:?}");
    }

    #[test]
    fn matrices_agree_with_point_transform() {
        let t = sample();
        let p = Vec3::new(0.3, 4.0, -1.2);
        let expected = t.transform_point(p);
        assert!(t.to_mat4().transform_point3(p).abs_diff_eq(expected, EPS));
        assert!(t.to_affine3a().transform_point3(p).abs_diff_eq(expected, EPS));
        assert!(Mat4::from(t).transform_vector3(p).abs_diff_eq(t.transform_vector(p), EPS));
        assert!(Affine3A::from(t).transform_vector3(p).abs_diff_eq(t.transform_vector(p), EPS));
    }

    #[test]
    fn compose_matches_matrix_product_for_uniform_parent_scale() {
        let parent = sample();
        let child = Transform {
            translation: Vec3::new(-0.5, 0.25, 2.0),
            rotation: Quat::from_rotation_z(0.4),
            scale: Vec3::new(1.0, 2.0, 3.0),
        };
        let composed = parent * child;
        let product = parent.to_mat4() * child.to_mat4();
        assert!(composed.to_mat4().abs_diff_eq(product, 1e-4));
        let p = Vec3::new(1.0, 1.0, 1.0);
        assert!(
            composed.transform_point(p).abs_diff_eq(parent.transform_point(child.transform_point(p)), 1e-4)
        );
    }

    #[test]
    fn inverse_round_trips() {
        let t = sample();
        assert_transform_near(&t.compose(&t.inverse()), &Transform::IDENTITY);
        assert_transform_near(&t.inverse().compose(&t), &Transform::IDENTITY);
        let p = Vec3::new(5.0, -3.0, 0.5);
        assert!(t.inverse().transform_point(t.transform_point(p)).abs_diff_eq(p, 1e-4));

        // Non-uniform scale is exact while rotation is identity.
        let scaled = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            scale: Vec3::new(2.0, 4.0, 0.5),
            ..Transform::IDENTITY
        };
        assert!(scaled.inverse().transform_point(scaled.transform_point(p)).abs_diff_eq(p, EPS));
    }

    #[test]
    fn lerp_endpoints_and_midpoint() {
        let a = Transform::from_translation_rotation(Vec3::ZERO, Quat::IDENTITY);
        let b = Transform {
            translation: Vec3::new(2.0, 0.0, 0.0),
            rotation: Quat::from_rotation_y(FRAC_PI_2),
            scale: Vec3::splat(3.0),
        };
        assert_transform_near(&a.lerp(&b, 0.0), &a);
        assert_transform_near(&a.lerp(&b, 1.0), &b);
        let mid = a.lerp(&b, 0.5);
        assert!(mid.translation.abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), EPS));
        assert!(mid.scale.abs_diff_eq(Vec3::splat(2.0), EPS));
        assert!(mid.rotation.angle_between(Quat::from_rotation_y(FRAC_PI_4)) < EPS);
    }

    #[test]
    fn lerp_takes_the_short_arc() {
        let a = Transform::from_rotation(Quat::from_rotation_y(0.1));
        // The same rotation as `from_rotation_y(-0.1)`, but in the opposite hemisphere.
        let b = Transform::from_rotation(-Quat::from_rotation_y(-0.1));
        let mid = a.lerp(&b, 0.5);
        assert!(mid.rotation.angle_between(Quat::IDENTITY) < 1e-3);
    }

    #[test]
    fn axes_follow_the_convention() {
        let t = Transform::IDENTITY;
        assert_eq!((t.forward(), t.right(), t.up()), (FORWARD, RIGHT, UP));
        // A positive yaw (counter-clockwise seen from above) is a left turn: facing +Z, the left
        // is +X, so forward swings to +X and right to +Z.
        let yawed = Transform::from_rotation(Quat::from_rotation_y(FRAC_PI_2));
        assert!(yawed.forward().abs_diff_eq(Vec3::X, EPS));
        assert!(yawed.right().abs_diff_eq(Vec3::Z, EPS));
        assert!(yawed.up().abs_diff_eq(Vec3::Y, EPS));
        assert_eq!(Transform::default(), Transform::IDENTITY);
    }
}
