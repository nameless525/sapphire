//! [`Plane`] and [`Frustum`] for culling.

use glam::{Mat4, Vec3A, Vec4};

use crate::bounds::{Aabb, Sphere};

/// The plane `normal · p + d = 0`, with `normal` of unit length. Points on the side the normal
/// faces have positive signed distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    /// Unit normal.
    pub normal: Vec3A,
    /// Signed offset: the plane's distance from the origin along `-normal`.
    pub d: f32,
}

impl Plane {
    /// The plane through `point` facing `normal` (normalized here).
    #[must_use]
    pub fn from_point_normal(point: impl Into<Vec3A>, normal: impl Into<Vec3A>) -> Self {
        let normal = normal.into().normalize();
        Self { normal, d: -normal.dot(point.into()) }
    }

    /// Normalizes the raw coefficients `(a, b, c, d)` of `a·x + b·y + c·z + d = 0`.
    ///
    /// Returns `None` when the plane is at infinity: its normal is too short relative to `d` for
    /// `f32` to place it, which is how an infinite far plane shows up.
    #[must_use]
    pub fn from_coefficients(coefficients: Vec4) -> Option<Self> {
        let normal = Vec3A::from(coefficients.truncate());
        let length = normal.length();
        // Planes farther than 1/ε (about 8,400 km) from the origin are beyond any zone (spec §5.4).
        if length <= f32::EPSILON * coefficients.w.abs() || length == 0.0 {
            return None;
        }
        Some(Self { normal: normal / length, d: coefficients.w / length })
    }

    /// Signed distance from the plane to `point`, positive on the normal's side.
    #[must_use]
    pub fn signed_distance(&self, point: impl Into<Vec3A>) -> f32 {
        self.normal.dot(point.into()) + self.d
    }
}

/// A convex view volume bounded by inward-facing planes, used for visibility culling.
///
/// Built from a view-projection matrix in the engine's clip space (z in [0, 1]). The planes are
/// left, right, bottom, top, then `z = 0` and `z = w`. Under reverse-Z (the engine default)
/// `z = w` is the near plane and `z = 0` the far plane; with an infinite far plane the latter
/// is at infinity and is omitted, leaving five planes. Standard-Z matrices (z = 0 near) work
/// too, with the roles swapped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    planes: [Plane; 6],
    len: usize,
}

impl Frustum {
    /// Extracts the planes of `view_proj` (Gribb–Hartmann), normalized to unit normals.
    ///
    /// Passing a projection alone gives a view-space frustum; passing `proj * view` gives a
    /// world-space one.
    #[must_use]
    pub fn from_view_proj(view_proj: &Mat4) -> Self {
        let [r0, r1, r2, r3] = [0, 1, 2, 3].map(|i| view_proj.row(i));
        // Each row combination is one clip inequality rewritten as `plane · p >= 0`:
        // −w ≤ x ≤ w, −w ≤ y ≤ w, 0 ≤ z ≤ w.
        let coefficients = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
        let mut planes = [Plane { normal: Vec3A::ZERO, d: 0.0 }; 6];
        let mut len = 0;
        for plane in coefficients.into_iter().filter_map(Plane::from_coefficients) {
            planes[len] = plane;
            len += 1;
        }
        Self { planes, len }
    }

    /// The active planes, normals pointing into the volume.
    #[must_use]
    pub fn planes(&self) -> &[Plane] {
        &self.planes[..self.len]
    }

    /// Whether `point` is inside or on the boundary.
    #[must_use]
    pub fn contains_point(&self, point: impl Into<Vec3A>) -> bool {
        let point = point.into();
        self.planes().iter().all(|plane| plane.signed_distance(point) >= 0.0)
    }

    /// Whether `sphere` may be visible.
    ///
    /// Conservative: a sphere just outside a frustum corner can be reported as intersecting,
    /// which costs a draw but never drops a visible object.
    #[must_use]
    pub fn intersects_sphere(&self, sphere: &Sphere) -> bool {
        self.planes().iter().all(|plane| plane.signed_distance(sphere.center) >= -sphere.radius)
    }

    /// Whether `aabb` may be visible.
    ///
    /// Tests the box corner furthest along each plane normal, so it is conservative in the
    /// same way as [`Frustum::intersects_sphere`].
    #[must_use]
    pub fn intersects_aabb(&self, aabb: &Aabb) -> bool {
        let center = aabb.center();
        let half_extents = aabb.half_extents();
        self.planes().iter().all(|plane| {
            let reach = plane.normal.abs().dot(half_extents);
            plane.signed_distance(center) >= -reach
        })
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use glam::{Quat, Vec3};

    use super::*;
    use crate::projection::{perspective_reverse_z_infinite, view_from_transform};
    use crate::{FORWARD, Transform};

    const NEAR: f32 = 0.1;

    /// 90° vertical FOV, square aspect: at distance d the frustum spans ±d on x and y.
    fn view_space_frustum() -> Frustum {
        Frustum::from_view_proj(&perspective_reverse_z_infinite(FRAC_PI_2, 1.0, NEAR))
    }

    #[test]
    fn plane_basics() {
        let plane = Plane::from_point_normal(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 5.0, 0.0));
        assert_eq!(plane.normal, Vec3A::Y);
        assert!((plane.signed_distance(Vec3::new(3.0, 5.0, -1.0)) - 3.0).abs() < 1e-6);
        assert!((plane.signed_distance(Vec3::ZERO) + 2.0).abs() < 1e-6);
        assert_eq!(Plane::from_coefficients(Vec4::new(0.0, 0.0, 0.0, 0.1)), None);
        assert_eq!(Plane::from_coefficients(Vec4::ZERO), None);
    }

    #[test]
    fn infinite_reverse_z_has_five_inward_planes() {
        let frustum = view_space_frustum();
        assert_eq!(frustum.planes().len(), 5);
        for plane in frustum.planes() {
            assert!((plane.normal.length() - 1.0).abs() < 1e-6);
            // A point straight ahead is inside every plane.
            assert!(plane.signed_distance(Vec3::new(0.0, 0.0, -10.0)) > 0.0, "{plane:?}");
        }
        // The near plane (z = w under reverse-Z) is the last one.
        let near = frustum.planes()[4];
        assert!(near.normal.abs_diff_eq(Vec3A::NEG_Z, 1e-6));
        assert!((near.signed_distance(Vec3::new(0.0, 0.0, -NEAR))).abs() < 1e-6);
    }

    #[test]
    fn spheres_in_view_space() {
        let frustum = view_space_frustum();
        let visible = |center: Vec3, radius: f32| frustum.intersects_sphere(&Sphere::new(center, radius));
        assert!(visible(Vec3::new(0.0, 0.0, -10.0), 1.0));
        assert!(visible(Vec3::new(0.0, 0.0, -1.0e6), 1.0), "no far plane");
        assert!(!visible(Vec3::new(0.0, 0.0, 10.0), 1.0), "behind the camera");
        assert!(!visible(Vec3::new(0.0, 0.0, -0.05), 0.01), "between the eye and the near plane");
        assert!(!visible(Vec3::new(20.0, 0.0, -10.0), 1.0), "off to the right");
        assert!(visible(Vec3::new(10.5, 0.0, -10.0), 1.0), "straddling the right plane");
        assert!(!visible(Vec3::new(0.0, -20.0, -10.0), 1.0), "below");
    }

    #[test]
    fn aabbs_in_view_space() {
        let frustum = view_space_frustum();
        let visible = |center: Vec3, half: f32| {
            frustum.intersects_aabb(&Aabb::from_center_half_extents(center, Vec3::splat(half)))
        };
        assert!(visible(Vec3::new(0.0, 0.0, -10.0), 1.0));
        assert!(visible(Vec3::new(0.0, 0.0, -1.0e5), 1.0));
        assert!(!visible(Vec3::new(0.0, 0.0, 5.0), 1.0));
        assert!(!visible(Vec3::new(-15.0, 0.0, -10.0), 1.0));
        assert!(visible(Vec3::new(-10.5, 0.0, -10.0), 1.0));
        assert!(!visible(Vec3::new(0.0, 15.0, -10.0), 1.0));
        // A box enclosing the camera is always visible.
        assert!(visible(Vec3::ZERO, 100.0));
    }

    #[test]
    fn finite_reverse_z_has_a_far_plane() {
        // Swapping near and far in a standard [0, 1] projection gives finite reverse-Z.
        let proj = glam::camera::rh::proj::directx::perspective(FRAC_PI_2, 1.0, 100.0, NEAR);
        let frustum = Frustum::from_view_proj(&proj);
        assert_eq!(frustum.planes().len(), 6);
        assert!(frustum.contains_point(Vec3::new(0.0, 0.0, -50.0)));
        assert!(!frustum.contains_point(Vec3::new(0.0, 0.0, -150.0)));
        assert!(
            !frustum.intersects_aabb(&Aabb::from_center_half_extents(Vec3::new(0.0, 0.0, -150.0), Vec3::ONE))
        );
        assert!(frustum.intersects_sphere(&Sphere::new(Vec3::new(0.0, 0.0, -100.5), 1.0)));
    }

    #[test]
    fn world_space_frustum_follows_the_camera() {
        // Camera at (100, 0, 0) yawed a quarter turn so its +Z forward looks down world +X.
        let camera = Transform::from_translation_rotation(
            Vec3::new(100.0, 0.0, 0.0),
            Quat::from_rotation_y(FRAC_PI_2),
        );
        let view = view_from_transform(&camera);
        let frustum = Frustum::from_view_proj(&(perspective_reverse_z_infinite(FRAC_PI_2, 1.0, NEAR) * view));
        assert!(frustum.contains_point(Vec3::new(110.0, 0.0, 0.0)));
        assert!(!frustum.contains_point(Vec3::new(90.0, 0.0, 0.0)));
        assert!(!frustum.contains_point(Vec3::new(0.0, 0.0, -10.0)));
    }

    #[test]
    fn frustum_four_kilometres_from_the_origin() {
        let proj = perspective_reverse_z_infinite(1.0, 1.7, NEAR);
        for position in [Vec3::new(3000.0, 10.0, -2500.0), Vec3::new(-4000.0, 300.0, 4000.0)] {
            let camera = Transform::from_translation_rotation(position, Quat::from_rotation_y(0.3));
            let frustum = Frustum::from_view_proj(&(proj * view_from_transform(&camera)));
            assert_eq!(frustum.planes().len(), 5, "the infinite far plane is still dropped at {position}");
            let ahead = camera.transform_point(FORWARD * 50.0);
            let behind = camera.transform_point(FORWARD * -50.0);
            assert!(frustum.contains_point(ahead), "{position}");
            assert!(!frustum.contains_point(behind), "{position}");
            assert!(frustum.intersects_sphere(&Sphere::new(camera.transform_point(FORWARD * 2000.0), 1.0)));
            assert!(!frustum.intersects_aabb(&Aabb::from_center_half_extents(behind, Vec3::ONE)));
        }
    }
}
