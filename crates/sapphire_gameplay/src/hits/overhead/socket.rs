// A physical capsule projected into the exact drawn steel mesh coordinates.
use sapphire_colliders::document::MAX_COORDINATE;
use sapphire_math::glam::{DMat3, DVec3, Mat4, Vec3};

// Bound both temporal endpoints' socket-local f32 conversion and f64 witness arithmetic.
// Rounding three coordinates within 64 m costs at most sqrt(3)*2^-19 m locally.
// The admitted scale is at most 4.00004, so both times remain under 30 micrometres in world space.
pub(super) const BRACKET_ROUNDOFF_M: f64 = 30e-6;

#[derive(Clone, Copy)]
pub(super) struct Socket {
    origin: DVec3,
    basis: DMat3,
    inverse: DMat3,
    min_scale: f64,
    max_scale: f64,
}

impl Socket {
    pub(super) fn bind(matrix: Mat4, expected_scale: f32) -> Option<Self> {
        const GRAM_TOLERANCE: f64 = 2e-5;
        let [x, y, z, translation] = matrix.to_cols_array_2d();
        if !matrix.is_finite() || [x[3], y[3], z[3], translation[3]] != [0.0, 0.0, 0.0, 1.0] {
            return None;
        }
        let xyz = |c: [f32; 4]| DVec3::new(f64::from(c[0]), f64::from(c[1]), f64::from(c[2]));
        let origin = xyz(translation);
        let basis = DMat3::from_cols(xyz(x), xyz(y), xyz(z));
        let expected = f64::from(expected_scale);
        if !(0.25..=4.0).contains(&expected)
            || basis.determinant() <= 0.0
            || origin.abs().max_element() > f64::from(MAX_COORDINATE)
        {
            return None;
        }
        let axes = [basis.x_axis / expected, basis.y_axis / expected, basis.z_axis / expected];
        let mut lower = 1.0_f64;
        let mut upper = 1.0_f64;
        for (i, axis) in axes.into_iter().enumerate() {
            let diagonal = axis.dot(axis);
            let off_diagonal = axes
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, &other)| axis.dot(other).abs())
                .sum::<f64>();
            lower = lower.min(diagonal - off_diagonal);
            upper = upper.max(diagonal + off_diagonal);
        }
        if lower < 1.0 - GRAM_TOLERANCE || upper > 1.0 + GRAM_TOLERANCE {
            return None;
        }
        // Gershgorin bounds on the eigenvalues of A^T A give enclosing singular values.
        // Leave margin for the f64 operations used to compute them.
        let min_scale = (lower - 1e-12).sqrt() * expected;
        let max_scale = (upper + 1e-12).sqrt() * expected;
        Some(Self { origin, basis, inverse: basis.inverse(), min_scale, max_scale })
    }

    pub(super) fn capsule(self, world: [Vec3; 2], radius: f64, extra: f64) -> Option<([Vec3; 2], f64, f64)> {
        if world.into_iter().any(|point| !point.is_finite() || point.abs().max_element() > MAX_COORDINATE) {
            return None;
        }
        let points = world.map(|point| self.inverse * (point.as_dvec3() - self.origin));
        if points.into_iter().any(|point| !point.is_finite() || point.abs().max_element() > 64.0)
            || !radius.is_finite()
            || !extra.is_finite()
            || extra < 0.0
        {
            return None;
        }
        let rounded = points.map(DVec3::as_vec3);
        let endpoint_error =
            points[0].distance(rounded[0].as_dvec3()).max(points[1].distance(rounded[1].as_dvec3())) + 1e-9;
        let inner = radius / self.max_scale - endpoint_error;
        let outer = (radius + extra) / self.min_scale + endpoint_error;
        (inner > 0.0).then_some((rounded, inner, outer))
    }

    pub(super) fn relative_chord(
        self,
        after: Self,
        axis: [Vec3; 2],
        after_axis: [Vec3; 2],
        steel_radius: f64,
    ) -> f64 {
        let origin_delta = after.origin - self.origin;
        let axis_delta = axis
            .into_iter()
            .zip(after_axis)
            .map(|(a, b)| (b.as_dvec3() - a.as_dvec3() - origin_delta).length())
            .fold(0.0, f64::max);
        // The Frobenius norm bounds the movement of every point in the steel radius.
        let basis_delta = (after.basis - self.basis)
            .to_cols_array()
            .into_iter()
            .map(|component| component * component)
            .sum::<f64>()
            .sqrt();
        axis_delta + basis_delta * steel_radius
    }

    pub(super) fn relative_box_chord(
        self,
        after: Self,
        axis: [Vec3; 2],
        after_axis: [Vec3; 2],
        [min, max]: [DVec3; 2],
    ) -> f64 {
        let origin_delta = after.origin - self.origin;
        let basis_delta = after.basis - self.basis;
        let axis_delta =
            std::array::from_fn::<_, 2, _>(|i| after_axis[i].as_dvec3() - axis[i].as_dvec3() - origin_delta);
        // The norm is convex in the blade point and the axis parameter. Their
        // product domain is enclosed by the box corners and capsule endpoints.
        let mut longest: f64 = 0.0;
        for x in [min.x, max.x] {
            for y in [min.y, max.y] {
                for z in [min.z, max.z] {
                    let blade_delta = basis_delta * DVec3::new(x, y, z);
                    for delta in axis_delta {
                        longest = longest.max((delta - blade_delta).length_squared());
                    }
                }
            }
        }
        longest.sqrt()
    }

    pub(super) fn world_point(self, local: DVec3) -> DVec3 {
        self.origin + self.basis * local
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_the_actual_sample_and_an_enclosing_radius_under_small_basis_rounding() {
        let matrix = Mat4::from_scale(Vec3::new(0.8, 0.800_002, 0.799_998));
        let socket = Socket::bind(matrix, 0.8).unwrap();
        let axis = [Vec3::new(2.5, 0.25, 0.6), Vec3::new(2.0, 1.0, 0.8)];
        let (converted, inner, outer) = socket.capsule(axis, 0.125, 0.02).unwrap();
        for (converted, original) in converted.into_iter().zip(axis) {
            assert!(socket.world_point(converted.as_dvec3()).distance(original.as_dvec3()) < 1e-6);
        }
        assert!(inner * 0.800_002 < 0.125);
        assert!(outer * 0.799_998 > 0.145);
    }

    #[test]
    fn unsupported_shear_reflection_and_scale_do_not_create_a_socket() {
        assert!(Socket::bind(Mat4::from_scale(Vec3::new(-0.8, 0.8, 0.8)), 0.8).is_none());
        assert!(Socket::bind(Mat4::from_scale(Vec3::splat(0.81)), 0.8).is_none());
        let mut shear = Mat4::from_scale(Vec3::splat(0.8)).to_cols_array_2d();
        shear[1][0] = 0.002;
        assert!(Socket::bind(Mat4::from_cols_array_2d(&shear), 0.8).is_none());
    }

    #[test]
    fn blade_box_bounds_relative_chord_with_common_motion_and_rotating_steel() {
        let before = Socket::bind(Mat4::from_translation(Vec3::splat(8.0)), 1.0).unwrap();
        let after = Socket::bind(
            Mat4::from_translation(Vec3::new(8.6, 7.9, 8.4))
                * Mat4::from_quat(sapphire_math::det::quat_from_rotation_y(0.25)),
            1.0,
        )
        .unwrap();
        let axis = [Vec3::new(8.2, 8.3, 8.4), Vec3::new(8.4, 8.6, 8.3)];
        let after_axis = [Vec3::new(8.8, 8.19, 8.8), Vec3::new(9.01, 8.5, 8.7)];
        let min = DVec3::new(-0.02, 0.05, -0.008);
        let max = DVec3::new(0.02, 0.94, 0.008);
        let bound = before.relative_box_chord(after, axis, after_axis, [min, max]);
        let radius = DVec3::new(0.02, 0.94, 0.008).length();
        assert!(bound < before.relative_chord(after, axis, after_axis, radius) * 0.15);
        for x in 0..=4 {
            for y in 0..=4 {
                for z in 0..=4 {
                    let point = min + (max - min) * DVec3::new(x.into(), y.into(), z.into()) / 4.0;
                    let blade_delta = after.world_point(point) - before.world_point(point);
                    for along in 0..=8 {
                        let t = f64::from(along) / 8.0;
                        let a = axis[0].as_dvec3().lerp(axis[1].as_dvec3(), t);
                        let b = after_axis[0].as_dvec3().lerp(after_axis[1].as_dvec3(), t);
                        assert!((b - a - blade_delta).length() <= bound + 1e-12);
                    }
                }
            }
        }
    }

    #[test]
    fn finite_world_coordinates_outside_the_representable_witness_domain_are_refused() {
        let matrix = Mat4::from_translation(Vec3::X * 1e15) * Mat4::from_scale(Vec3::splat(0.8));
        assert!(matrix.is_finite());
        assert!(Socket::bind(matrix, 0.8).is_none());
        let socket = Socket::bind(Mat4::from_scale(Vec3::splat(0.8)), 0.8).unwrap();
        assert!(socket.capsule([Vec3::X * 1e15; 2], 0.1, 0.0).is_none());
    }
}
