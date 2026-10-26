//! Bounding volumes for the CPU's per-object work: per-object light lists (M1 R5) and culling
//! shadow casters per cascade (M1 R4). Each mesh's object-space sphere comes from its cooked
//! bounds and lives in its record in the mesh pool (`mesh_pool::MeshRecord::bounds`).

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

    /// This sphere after `transform`, grown by its largest scale so it still holds the mesh.
    pub(crate) fn transformed(self, transform: Mat4) -> Self {
        let scale = transform
            .x_axis
            .truncate()
            .length()
            .max(transform.y_axis.truncate().length())
            .max(transform.z_axis.truncate().length());
        Self { center: transform.transform_point3(self.center), radius: self.radius * scale }
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

#[cfg(test)]
mod tests {
    use super::*;

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
