//! Bounding volumes for the CPU's per-object work: per-object light lists (M1 R5), culling
//! shadow casters per cascade (M1 R4) and outline hulls past their cutoff (M1 C4). Each mesh's
//! object-space sphere comes from its cooked bounds and lives in its record in the mesh pool
//! (`mesh_pool::MeshRecord::bounds`).

use glam::{Mat4, Vec3};

/// A bounding sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sphere {
    pub center: Vec3,
    pub radius: f32,
}

impl Sphere {
    /// The sphere around an object-space box (`[min, max]`, as meshes are cooked with).
    pub(crate) fn around([min, max]: [[f32; 3]; 2]) -> Self {
        let (min, max) = (Vec3::from(min), Vec3::from(max));
        Self { center: (min + max) * 0.5, radius: (max - min).length() * 0.5 }
    }

    /// This sphere after `transform`, grown by its largest stretch ([`max_stretch`]) so it still
    /// holds the mesh.
    pub(crate) fn transformed(self, transform: Mat4) -> Self {
        Self { center: transform.transform_point3(self.center), radius: self.radius * max_stretch(transform) }
    }

    /// The smallest sphere holding both this one and `other`.
    pub(crate) fn union(self, other: Self) -> Self {
        let offset = other.center - self.center;
        let distance = offset.length();
        if distance + other.radius <= self.radius {
            return self;
        }
        if distance + self.radius <= other.radius {
            return other;
        }
        let radius = (distance + self.radius + other.radius) * 0.5;
        let center = self.center + offset * ((radius - self.radius) / distance);
        Self { center, radius }
    }
}

/// How far past the longest column's squared length the bound below may run and still be
/// rounding: float rounding leaves the columns of rotations, and of chains of them, up to about
/// 1e-6 off right angles, which puts the bound up to 2e-6 past it. Taking the longest column
/// there falls short of the stretch by at most half this share: 10 µm a meter of radius.
const ROUNDING: f32 = 2e-5;

/// How far `transform` stretches any direction (its largest singular value), or more, never
/// less: what a radius grows by so a sphere or capsule still holds what it held.
///
/// The bound is the square root of the Gershgorin bound on the largest eigenvalue of `MᵀM`: the
/// largest row sum of the columns' absolute dot products. Where the columns are at right angles,
/// as for rotation and scale (the transforms apps send), it is the longest column's length, and
/// that is kept bit for bit as it was before shear was handled, so no culling, cascade, outline
/// or light decision moves. Under shear (a rotated child under a non-uniformly scaled parent,
/// say) the longest column falls short (review RA-003) and the bound doesn't.
pub(crate) fn max_stretch(transform: Mat4) -> f32 {
    let (x, y, z) = (transform.x_axis.truncate(), transform.y_axis.truncate(), transform.z_axis.truncate());
    let (xx, yy, zz) = (x.length_squared(), y.length_squared(), z.length_squared());
    let (xy, xz, yz) = (x.dot(y).abs(), x.dot(z).abs(), y.dot(z).abs());
    let longest = xx.max(yy).max(zz);
    let bound = (xx + xy + xz).max(yy + xy + yz).max(zz + xz + yz);
    // A comparison with NaN is false: a transform that isn't finite keeps the old value too.
    let squared = if bound > longest * (1.0 + ROUNDING) { bound } else { longest };
    // A square root is correctly rounded and never decreases, so the root of the longest
    // squared length is the longest of the lengths, bit for bit.
    squared.sqrt()
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_4;

    use glam::Vec4;

    use super::*;
    use crate::CharacterBody;

    #[test]
    fn spheres_follow_their_transforms() {
        let sphere = Sphere::around([[-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]]);
        assert_eq!(sphere.center, Vec3::Y);
        assert!((sphere.radius - 3f32.sqrt()).abs() < 1e-6);
        let moved = sphere.transformed(Mat4::from_scale_rotation_translation(
            Vec3::new(1.0, 3.0, 2.0),
            glam::Quat::IDENTITY,
            Vec3::X,
        ));
        assert_eq!(moved.center, Vec3::new(1.0, 3.0, 0.0));
        assert!((moved.radius - 3.0 * 3f32.sqrt()).abs() < 1e-5, "grown by the largest scale");
    }

    /// A seeded generator for the transforms below: xorshift32, uniform in `[lo, hi)`.
    struct Random(u32);

    impl Random {
        fn next(&mut self, lo: f32, hi: f32) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            lo + (hi - lo) * (self.0 >> 8) as f32 / (1u32 << 24) as f32
        }

        fn vec3(&mut self, lo: f32, hi: f32) -> Vec3 {
            Vec3::new(self.next(lo, hi), self.next(lo, hi), self.next(lo, hi))
        }

        fn rotation(&mut self) -> glam::Quat {
            glam::Quat::from_xyzw(
                self.next(-1.0, 1.0),
                self.next(-1.0, 1.0),
                self.next(-1.0, 1.0),
                self.next(-1.0, 1.0),
            )
            .normalize()
        }

        /// A positive scale per axis, from a hundredth to a hundred.
        fn scale(&mut self) -> Vec3 {
            Vec3::new(
                10f32.powf(self.next(-2.0, 2.0)),
                10f32.powf(self.next(-2.0, 2.0)),
                10f32.powf(self.next(-2.0, 2.0)),
            )
        }
    }

    /// The box's corners under `transform` all lie in the sphere around the box, transformed.
    fn assert_holds_its_box(bounds: [[f32; 3]; 2], transform: Mat4) {
        let moved = Sphere::around(bounds).transformed(transform);
        let [min, max] = bounds;
        for corner in 0..8 {
            let pick = |axis: usize| if corner & (1 << axis) == 0 { min[axis] } else { max[axis] };
            let point = transform.transform_point3(Vec3::new(pick(0), pick(1), pick(2)));
            // Float rounding in the center and the radius, relative to the transform's size.
            let slack = 1e-5 * (moved.radius + transform.w_axis.truncate().length());
            let distance = point.distance(moved.center);
            assert!(distance <= moved.radius + slack, "{distance} > {} under {transform:?}", moved.radius);
        }
    }

    #[test]
    fn spheres_hold_their_mesh_under_shear() {
        // x' = x + y: the box's corner (1, 1, 0) lands √5 from the center, past the longest
        // column (√2) × the box's half-diagonal (√2).
        let shear = Mat4::from_cols_array(&[
            1.0, 0.0, 0.0, 0.0, //
            1.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ]);
        assert_holds_its_box([[-1.0, -1.0, 0.0], [1.0, 1.0, 0.0]], shear);
        // A rotated child under a non-uniformly scaled parent: its largest stretch (3, which the
        // bound finds exactly here) exceeds every column's length (√5).
        let squashed = Mat4::from_scale(Vec3::new(1.0, 3.0, 1.0)) * Mat4::from_rotation_z(FRAC_PI_4);
        assert_holds_its_box([[-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]], squashed);
        assert!((max_stretch(squashed) - 3.0).abs() < 1e-5, "{}", max_stretch(squashed));
        // Random affine transforms, and random chains of parents and children.
        let mut random = Random(0x5eed);
        for _ in 0..2000 {
            let bounds = [random.vec3(-2.0, 0.0).to_array(), random.vec3(0.0, 2.0).to_array()];
            let affine = Mat4::from_cols(
                random.vec3(-3.0, 3.0).extend(0.0),
                random.vec3(-3.0, 3.0).extend(0.0),
                random.vec3(-3.0, 3.0).extend(0.0),
                random.vec3(-50.0, 50.0).extend(1.0),
            );
            assert_holds_its_box(bounds, affine);
            let mut chain = Mat4::IDENTITY;
            for _ in 0..3 {
                chain *= Mat4::from_scale_rotation_translation(
                    random.scale(),
                    random.rotation(),
                    random.vec3(-5.0, 5.0),
                );
            }
            assert_holds_its_box(bounds, chain);
        }
    }

    #[test]
    fn rotation_and_scale_keep_the_longest_column_exactly() {
        // The radius before shear was handled: culling, cascade, outline and light decisions
        // made on it must not move by a bit for the transforms apps send.
        let longest_column = |transform: Mat4| {
            transform
                .x_axis
                .truncate()
                .length()
                .max(transform.y_axis.truncate().length())
                .max(transform.z_axis.truncate().length())
        };
        let sphere = Sphere { center: Vec3::new(0.25, 1.0, -0.5), radius: 1.7 };
        let mut random = Random(0xb0b);
        let exact = |transform: Mat4| {
            let moved = sphere.transformed(transform);
            assert_eq!(
                moved.radius.to_bits(),
                (sphere.radius * longest_column(transform)).to_bits(),
                "{transform:?}"
            );
            assert_eq!(moved.center, transform.transform_point3(sphere.center));
        };
        for _ in 0..20_000 {
            let (rotation, translation) = (random.rotation(), random.vec3(-1000.0, 1000.0));
            exact(Mat4::from_scale_rotation_translation(random.scale(), rotation, translation));
            exact(Mat4::from_scale_rotation_translation(
                Vec3::splat(random.next(0.01, 100.0)),
                rotation,
                translation,
            ));
            // Uniformly scaled parents and children compose to rotation and scale.
            let parent = Mat4::from_scale_rotation_translation(
                Vec3::splat(random.next(0.1, 10.0)),
                random.rotation(),
                random.vec3(-100.0, 100.0),
            );
            exact(parent * Mat4::from_scale_rotation_translation(random.scale(), rotation, translation));
        }
        // Mirrored (unsupported for drawing, but its bounds hold), flattened and collapsed.
        exact(Mat4::from_scale(Vec3::new(-1.0, 2.0, 1.0)));
        exact(Mat4::from_scale(Vec3::new(1.0, 0.0, 3.0)));
        exact(Mat4::from_scale(Vec3::ZERO));
        assert_eq!(sphere.transformed(Mat4::from_scale(Vec3::ZERO)).radius, 0.0);
        // Transforms that aren't finite keep the old radius too, whatever it was.
        for broken in [
            Mat4::from_scale(Vec3::new(f32::NAN, 1.0, 2.0)),
            Mat4::from_scale(Vec3::new(f32::INFINITY, 1.0, 2.0)),
            Mat4::from_cols(Vec4::splat(f32::NAN), Vec4::Y, Vec4::Z, Vec4::W),
            Mat4::from_cols(Vec4::X, Vec4::new(1.0, f32::INFINITY, 0.0, 0.0), Vec4::Z, Vec4::W),
        ] {
            let radius = sphere.transformed(broken).radius;
            assert_eq!(radius.to_bits(), (sphere.radius * longest_column(broken)).to_bits(), "{broken:?}");
        }
    }

    #[test]
    fn character_bodies_hold_their_capsule_under_shear() {
        let body = CharacterBody::capsule(Vec3::new(0.0, 0.3, 0.0), Vec3::new(0.0, 1.6, 0.0), 0.3);
        // A body's capsule: every point within its radius of the segment from base to top.
        let gap = |body: &CharacterBody, point: Vec3| {
            let axis = body.top - body.base;
            let along = ((point - body.base).dot(axis) / axis.length_squared()).clamp(0.0, 1.0);
            point.distance(body.base + axis * along)
        };
        let squashed = Mat4::from_scale(Vec3::new(1.0, 3.0, 1.0)) * Mat4::from_rotation_z(FRAC_PI_4);
        let sheared = Mat4::from_cols_array(&[
            1.0, 0.0, 0.0, 0.0, //
            0.8, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            2.0, 0.0, -1.0, 1.0,
        ]);
        for transform in [squashed, sheared] {
            let moved = body.transformed(transform);
            // Points on the capsule's surface: around its two ends, and along its side.
            for i in 0..4000 {
                let y = 1.0 - 2.0 * (i as f32 + 0.5) / 4000.0;
                let ring = (1.0 - y * y).sqrt();
                let angle = i as f32 * 2.399_963;
                let direction = Vec3::new(ring * angle.cos(), y, ring * angle.sin());
                let along = body.base.lerp(body.top, (i % 7) as f32 / 6.0);
                let point = transform.transform_point3(along + direction * body.radius);
                assert!(gap(&moved, point) <= moved.radius * (1.0 + 1e-5), "{point} outside {moved:?}");
            }
        }
        // Rotation and scale keep the longest column exactly, as before: Y's, 2.5 long.
        let trs = Mat4::from_scale_rotation_translation(
            Vec3::new(1.0, 2.5, 0.5),
            glam::Quat::from_rotation_y(0.7),
            Vec3::X,
        );
        assert_eq!(body.transformed(trs).radius.to_bits(), (body.radius * 2.5).to_bits());
    }

    #[test]
    fn unions_hold_both_spheres_and_no_more() {
        let a = Sphere { center: Vec3::ZERO, radius: 1.0 };
        let b = Sphere { center: Vec3::new(4.0, 0.0, 0.0), radius: 1.0 };
        let both = a.union(b);
        assert_eq!(both, Sphere { center: Vec3::new(2.0, 0.0, 0.0), radius: 3.0 });
        let inside = Sphere { center: Vec3::new(0.5, 0.0, 0.0), radius: 0.2 };
        assert_eq!(a.union(inside), a, "already held");
        assert_eq!(inside.union(a), a, "holds it");
        assert_eq!(a.union(a), a);
    }
}
