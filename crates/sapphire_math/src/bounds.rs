//! Bounding volumes: [`Aabb`] and [`Sphere`].
//!
//! Every containment and intersection test treats the boundary as inside, so touching volumes
//! intersect.

use glam::{Affine3A, Vec3A};

/// An axis-aligned bounding box. Valid boxes have `min <= max` on every axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    /// Minimum corner.
    pub min: Vec3A,
    /// Maximum corner.
    pub max: Vec3A,
}

impl Aabb {
    /// A box from its corners.
    #[must_use]
    pub fn new(min: impl Into<Vec3A>, max: impl Into<Vec3A>) -> Self {
        let (min, max) = (min.into(), max.into());
        debug_assert!(min.cmple(max).all(), "Aabb min {min} exceeds max {max}");
        Self { min, max }
    }

    /// A box from its center and half extents.
    #[must_use]
    pub fn from_center_half_extents(center: impl Into<Vec3A>, half_extents: impl Into<Vec3A>) -> Self {
        let (center, half_extents) = (center.into(), half_extents.into());
        Self::new(center - half_extents, center + half_extents)
    }

    /// The tightest box around `points`, or `None` if there are none.
    #[must_use]
    pub fn from_points<P: Into<Vec3A>>(points: impl IntoIterator<Item = P>) -> Option<Self> {
        let mut points = points.into_iter().map(Into::into);
        let first = points.next()?;
        let (min, max) = points.fold((first, first), |(min, max), p| (min.min(p), max.max(p)));
        Some(Self { min, max })
    }

    /// The center point.
    #[must_use]
    pub fn center(&self) -> Vec3A {
        (self.min + self.max) * 0.5
    }

    /// Half the size on each axis.
    #[must_use]
    pub fn half_extents(&self) -> Vec3A {
        (self.max - self.min) * 0.5
    }

    /// The smallest box containing both `self` and `other`.
    #[must_use]
    pub fn merge(&self, other: &Self) -> Self {
        Self { min: self.min.min(other.min), max: self.max.max(other.max) }
    }

    /// The exact axis-aligned bounds of this box after `transform`.
    ///
    /// Transforms the center and takes the absolute value of the linear part for the extents
    /// (Arvo's method), which equals the bounds of all eight transformed corners at a fraction
    /// of the cost.
    #[must_use]
    pub fn transformed(&self, transform: &Affine3A) -> Self {
        let center = transform.transform_point3a(self.center());
        let half = self.half_extents();
        let m = transform.matrix3;
        let half_extents = m.x_axis.abs() * half.x + m.y_axis.abs() * half.y + m.z_axis.abs() * half.z;
        Self { min: center - half_extents, max: center + half_extents }
    }

    /// Whether `point` is inside or on the boundary.
    #[must_use]
    pub fn contains_point(&self, point: impl Into<Vec3A>) -> bool {
        let point = point.into();
        point.cmpge(self.min).all() && point.cmple(self.max).all()
    }

    /// Whether `other` lies entirely inside `self`.
    #[must_use]
    pub fn contains_aabb(&self, other: &Self) -> bool {
        other.min.cmpge(self.min).all() && other.max.cmple(self.max).all()
    }

    /// Whether the boxes overlap or touch.
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }
}

/// A bounding sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sphere {
    /// Center point.
    pub center: Vec3A,
    /// Radius; never negative.
    pub radius: f32,
}

impl Sphere {
    /// A sphere from its center and radius.
    #[must_use]
    pub fn new(center: impl Into<Vec3A>, radius: f32) -> Self {
        debug_assert!(radius >= 0.0, "negative sphere radius {radius}");
        Self { center: center.into(), radius }
    }

    /// The sphere through the corners of `aabb` (it encloses the box, not the reverse).
    #[must_use]
    pub fn from_aabb(aabb: &Aabb) -> Self {
        Self { center: aabb.center(), radius: aabb.half_extents().length() }
    }

    /// A sphere enclosing this one after `transform`, including non-uniform scale and shear.
    ///
    /// The radius grows by an upper bound on the transform's largest stretch (its largest
    /// singular value): the square root of a Gershgorin bound on the largest eigenvalue of
    /// `MᵀM`. It is exact for rotation with any per-axis scale, where the columns are
    /// orthogonal, and stays conservative under shear (for example a non-uniformly scaled parent
    /// over a rotated child).
    #[must_use]
    pub fn transformed(&self, transform: &Affine3A) -> Self {
        let m = transform.matrix3;
        let (x, y, z) = (m.x_axis, m.y_axis, m.z_axis);
        let (xy, xz, yz) = (x.dot(y).abs(), x.dot(z).abs(), y.dot(z).abs());
        let max_stretch_sq = (x.length_squared() + xy + xz)
            .max(y.length_squared() + xy + yz)
            .max(z.length_squared() + xz + yz);
        Self { center: transform.transform_point3a(self.center), radius: self.radius * max_stretch_sq.sqrt() }
    }

    /// Whether `point` is inside or on the surface.
    #[must_use]
    pub fn contains_point(&self, point: impl Into<Vec3A>) -> bool {
        self.center.distance_squared(point.into()) <= self.radius * self.radius
    }

    /// Whether the spheres overlap or touch.
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        let reach = self.radius + other.radius;
        self.center.distance_squared(other.center) <= reach * reach
    }

    /// Whether the sphere overlaps or touches `aabb`.
    #[must_use]
    pub fn intersects_aabb(&self, aabb: &Aabb) -> bool {
        let closest = self.center.clamp(aabb.min, aabb.max);
        self.center.distance_squared(closest) <= self.radius * self.radius
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_4;

    use glam::{Quat, Vec3};

    use super::*;

    fn unit_box() -> Aabb {
        Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0))
    }

    #[test]
    fn from_points() {
        assert_eq!(Aabb::from_points(std::iter::empty::<Vec3>()), None);
        let aabb = Aabb::from_points([
            Vec3::new(1.0, -2.0, 3.0),
            Vec3::new(-1.0, 5.0, 0.0),
            Vec3::new(0.0, 0.0, -4.0),
        ])
        .expect("non-empty");
        assert_eq!(aabb, Aabb::new(Vec3::new(-1.0, -2.0, -4.0), Vec3::new(1.0, 5.0, 3.0)));
        assert_eq!(Aabb::from_points([[2.0, 2.0, 2.0]]), Some(Aabb::new(Vec3::splat(2.0), Vec3::splat(2.0))));
    }

    #[test]
    fn center_and_half_extents() {
        let aabb = Aabb::new(Vec3::new(0.0, 2.0, -4.0), Vec3::new(2.0, 6.0, 0.0));
        assert_eq!(aabb.center(), Vec3A::new(1.0, 4.0, -2.0));
        assert_eq!(aabb.half_extents(), Vec3A::new(1.0, 2.0, 2.0));
        assert_eq!(Aabb::from_center_half_extents(aabb.center(), aabb.half_extents()), aabb);
    }

    #[test]
    fn merge() {
        let a = Aabb::new(Vec3::ZERO, Vec3::ONE);
        let b = Aabb::new(Vec3::new(-1.0, 0.5, 2.0), Vec3::new(0.0, 0.7, 3.0));
        assert_eq!(a.merge(&b), Aabb::new(Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 3.0)));
        assert_eq!(a.merge(&a), a);
    }

    #[test]
    fn transformed_equals_bounds_of_transformed_corners() {
        let aabb = Aabb::new(Vec3::new(-1.0, 0.0, 2.0), Vec3::new(3.0, 1.0, 2.5));
        let transform = Affine3A::from_scale_rotation_translation(
            Vec3::new(2.0, 0.5, 1.5),
            Quat::from_euler(glam::EulerRot::XYZ, 0.3, -1.2, 0.8),
            Vec3::new(10.0, -3.0, 0.25),
        );
        let corners = (0..8).map(|i| {
            let pick = |bit: u32, lo: f32, hi: f32| if i & bit == 0 { lo } else { hi };
            let corner = Vec3A::new(
                pick(1, aabb.min.x, aabb.max.x),
                pick(2, aabb.min.y, aabb.max.y),
                pick(4, aabb.min.z, aabb.max.z),
            );
            transform.transform_point3a(corner)
        });
        let expected = Aabb::from_points(corners).expect("eight corners");
        let actual = aabb.transformed(&transform);
        assert!(actual.min.abs_diff_eq(expected.min, 1e-4), "{actual:?} vs {expected:?}");
        assert!(actual.max.abs_diff_eq(expected.max, 1e-4), "{actual:?} vs {expected:?}");
    }

    #[test]
    fn containment_and_intersection_include_the_boundary() {
        let aabb = unit_box();
        assert!(aabb.contains_point(Vec3::ZERO));
        assert!(aabb.contains_point(Vec3::new(1.0, -1.0, 1.0)));
        assert!(!aabb.contains_point(Vec3::new(1.001, 0.0, 0.0)));
        assert!(aabb.contains_aabb(&Aabb::new(Vec3::splat(-0.5), Vec3::splat(1.0))));
        assert!(!aabb.contains_aabb(&Aabb::new(Vec3::splat(-0.5), Vec3::splat(1.5))));

        let touching = Aabb::new(Vec3::new(1.0, -1.0, -1.0), Vec3::new(2.0, 1.0, 1.0));
        let apart = Aabb::new(Vec3::new(1.1, -1.0, -1.0), Vec3::new(2.0, 1.0, 1.0));
        assert!(aabb.intersects(&touching) && touching.intersects(&aabb));
        assert!(!aabb.intersects(&apart) && !apart.intersects(&aabb));
    }

    #[test]
    fn spheres() {
        let sphere = Sphere::new(Vec3::ZERO, 1.0);
        assert!(sphere.contains_point(Vec3::new(0.0, 1.0, 0.0)));
        assert!(!sphere.contains_point(Vec3::new(0.0, 1.01, 0.0)));
        assert!(sphere.intersects(&Sphere::new(Vec3::new(2.0, 0.0, 0.0), 1.0)));
        assert!(!sphere.intersects(&Sphere::new(Vec3::new(2.1, 0.0, 0.0), 1.0)));

        // Near a box corner the sphere test is exact, unlike a box-vs-box approximation.
        let aabb = Aabb::new(Vec3::splat(1.0), Vec3::splat(2.0));
        assert!(!sphere.intersects_aabb(&aabb));
        assert!(Sphere::new(Vec3::ZERO, 3f32.sqrt()).intersects_aabb(&aabb));
        assert!(Sphere::new(Vec3::splat(1.5), 0.1).intersects_aabb(&aabb));

        let enclosing = Sphere::from_aabb(&unit_box());
        assert_eq!(enclosing.center, Vec3A::ZERO);
        assert!((enclosing.radius - 3f32.sqrt()).abs() < 1e-6);
    }

    /// Points spread over the unit sphere (a Fibonacci lattice).
    fn unit_sphere_points() -> impl Iterator<Item = Vec3A> {
        let count = 2000;
        (0..count).map(move |i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / count as f32;
            let ring = (1.0 - y * y).sqrt();
            let angle = i as f32 * 2.399_963;
            Vec3A::new(ring * angle.cos(), y, ring * angle.sin())
        })
    }

    fn assert_encloses(sphere: &Sphere, transform: &Affine3A) {
        let moved = sphere.transformed(transform);
        for direction in unit_sphere_points() {
            let surface = transform.transform_point3a(sphere.center + direction * sphere.radius);
            let distance = surface.distance(moved.center);
            assert!(distance <= moved.radius * (1.0 + 1e-5), "{distance} > {}", moved.radius);
        }
    }

    #[test]
    fn sphere_transform_is_exact_without_shear() {
        let sphere = Sphere::new(Vec3::new(1.0, 0.0, 0.0), 1.0);
        let transform = Affine3A::from_scale_rotation_translation(
            Vec3::new(1.0, 3.0, 2.0),
            Quat::from_rotation_x(0.5),
            Vec3::new(0.0, 5.0, 0.0),
        );
        let moved = sphere.transformed(&transform);
        assert!(moved.center.abs_diff_eq(transform.transform_point3a(sphere.center), 1e-6));
        assert!((moved.radius - 3.0).abs() < 1e-5);
        assert_encloses(&sphere, &transform);
    }

    #[test]
    fn sphere_transform_is_conservative_under_shear() {
        // Squash-and-stretch parent over a rotated child: the product shears, and its largest
        // stretch (3) exceeds every column length (√5 ≈ 2.24).
        let world = Affine3A::from_scale(Vec3::new(1.0, 3.0, 1.0)) * Affine3A::from_rotation_z(FRAC_PI_4);
        let sphere = Sphere::new(Vec3::ZERO, 1.0);
        let moved = sphere.transformed(&world);
        assert!(moved.radius >= 3.0 - 1e-5, "{}", moved.radius);
        assert_encloses(&sphere, &world);

        // A generic sheared affine, and a nested chain of non-uniform scales and rotations.
        let shear =
            Affine3A::from_cols_array(&[1.0, 0.4, -0.2, 0.9, 1.5, 0.3, -0.7, 0.1, 0.6, 4.0, -1.0, 2.0]);
        assert_encloses(&Sphere::new(Vec3::new(0.5, -1.0, 2.0), 2.5), &shear);
        let chain = Affine3A::from_scale_rotation_translation(
            Vec3::new(0.3, 2.0, 1.1),
            Quat::from_rotation_y(1.1),
            Vec3::X,
        ) * Affine3A::from_scale_rotation_translation(
            Vec3::new(4.0, 0.5, 1.0),
            Quat::from_rotation_x(-0.6),
            Vec3::Y,
        );
        assert_encloses(&Sphere::new(Vec3::ONE, 0.75), &chain);
    }
}
