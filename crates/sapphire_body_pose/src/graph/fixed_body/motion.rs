//! Travel bounds for regions attached to the actual published body pair.

use sapphire_math::{Quat, Transform, Vec3, det};

use super::BodyFrame;
use crate::NO_PARENT;

/// A body query's supported interval and a conservative bound on region travel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BodyMotionBound {
    /// A cut or the first endpoint. There is no interval to sweep.
    EndpointOnly,
    /// Bounds on speed and acceleration per simulation-step fraction, with numerical slack for the sampled FK.
    Continuous { metres_per_step: f64, metres_per_step_squared: f64, numerical_margin: f64 },
}

/// Why a region cannot use the current body interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyMotionError {
    Unavailable,
    Unsupported,
}

pub(super) fn region_bound(
    before: &BodyFrame,
    after: &BodyFrame,
    parents: &[u16],
    mut joint: usize,
    radius: f32,
) -> Result<BodyMotionBound, BodyMotionError> {
    // Leave room for f32 interpolation differences and the sums in affine composition.
    const FK_EXTENT_LIMIT: f64 = f32::MAX as f64 / 64.0;
    let mut radius = f64::from(radius);
    let mut fk_extent = radius.max(1.0);
    let mut rate = 0.0;
    let mut acceleration = 0.0;
    let mut depth = 0_u32;
    loop {
        let a = before.locals[joint];
        let b = after.locals[joint];
        let scale = scale_extent(a.scale).max(scale_extent(b.scale));
        let translation = length(a.translation).max(length(b.translation));
        let scale_rate = a
            .scale
            .to_array()
            .into_iter()
            .zip(b.scale.to_array())
            .map(|(a, b)| (f64::from(b) - f64::from(a)).abs())
            .fold(0.0, f64::max);
        let rotation = rotation_motion(a.rotation, b.rotation)?;
        if !(a.translation.is_finite()
            && b.translation.is_finite()
            && a.scale.is_finite()
            && b.scale.is_finite())
        {
            return Err(BodyMotionError::Unsupported);
        }
        // A later small scale cannot protect an earlier FK multiplication from overflow.
        // Including a unit basis and clamping each scale from below bounds every prefix too.
        fk_extent = translation + scale.max(1.0) * fk_extent;
        if fk_extent > FK_EXTENT_LIMIT {
            return Err(BodyMotionError::Unsupported);
        }
        let basis_rate = scale_rate + rotation.rate * scale;
        let basis_acceleration = (rotation.acceleration + rotation.rate * rotation.rate) * scale
            + 2.0 * rotation.rate * scale_rate;
        acceleration = scale * acceleration + 2.0 * basis_rate * rate + basis_acceleration * radius;
        rate = distance(a.translation, b.translation) + scale * rate + basis_rate * radius;
        radius = translation + scale * radius;
        depth += 1;
        let parent = parents[joint];
        if parent == NO_PARENT {
            break;
        }
        joint = usize::from(parent);
    }

    let a = before.root;
    let b = after.root;
    let scale = f64::from(a.scale.max(b.scale));
    // A controller yaw is unwrapped. Equal endpoint orientations can enclose a full turn.
    let yaw_rate = (f64::from(b.yaw) - f64::from(a.yaw)).abs();
    let yaw_extent = f64::from(a.yaw.abs().max(b.yaw.abs()));
    let root_translation = length(a.translation).max(length(b.translation));
    if yaw_extent > FK_EXTENT_LIMIT || root_translation + scale.max(1.0) * fk_extent > FK_EXTENT_LIMIT {
        return Err(BodyMotionError::Unsupported);
    }
    let scale_rate = (f64::from(b.scale) - f64::from(a.scale)).abs();
    let basis_rate = scale_rate + yaw_rate * scale;
    let basis_acceleration = yaw_rate * yaw_rate * scale + 2.0 * yaw_rate * scale_rate;
    let acceleration = scale * acceleration + 2.0 * basis_rate * rate + basis_acceleration * radius;
    let rate = distance(a.translation, b.translation) + scale * rate + basis_rate * radius;
    let world_radius = scale * radius;
    let numerical_margin = 64.0
        * f64::from(f32::EPSILON)
        * ((f64::from(depth + 1) + yaw_extent) * world_radius + root_translation + 1.0);
    if rate.is_finite() && acceleration.is_finite() && numerical_margin.is_finite() {
        Ok(BodyMotionBound::Continuous {
            metres_per_step: rate,
            metres_per_step_squared: acceleration,
            numerical_margin,
        })
    } else {
        Err(BodyMotionError::Unsupported)
    }
}

fn scale_extent(scale: Vec3) -> f64 {
    f64::from(scale.abs().max_element())
}

fn length(vector: Vec3) -> f64 {
    distance(Vec3::ZERO, vector)
}

fn distance(a: Vec3, b: Vec3) -> f64 {
    a.to_array()
        .into_iter()
        .zip(b.to_array())
        .map(|(a, b)| {
            let delta = f64::from(b) - f64::from(a);
            delta * delta
        })
        .sum::<f64>()
        .sqrt()
}

struct AngularMotion {
    rate: f64,
    acceleration: f64,
}

fn rotation_motion(a: Quat, b: Quat) -> Result<AngularMotion, BodyMotionError> {
    let a = a.to_array();
    let b = b.to_array();
    let dot = |a: [f32; 4], b: [f32; 4]| {
        a.into_iter().zip(b).map(|(a, b)| f64::from(a) * f64::from(b)).sum::<f64>()
    };
    let a2 = dot(a, a);
    let b2 = dot(b, b);
    let dot = dot(a, b);
    let unit_tolerance = 16.0 * f64::from(f32::EPSILON);
    let unit = (1.0 - unit_tolerance)..=(1.0 + unit_tolerance);
    if !unit.contains(&a2) || !unit.contains(&b2) {
        return Err(BodyMotionError::Unsupported);
    }
    if a == b {
        return Ok(AngularMotion { rate: 0.0, acceleration: 0.0 });
    }
    let dot = dot.abs();
    let alignment = (dot / (a2 * b2).sqrt()).min(1.0);
    let tangent = ((1.0 - alignment) / (1.0 + alignment)).sqrt();
    let norm_ratio = (a2.max(b2) / a2.min(b2)).sqrt();
    // Both blends normalize a positive weighted sum in the shortest hemisphere. The nlerp
    // angular-rate bound is 4*tan(theta/4). Sine weights add the derivative factor angle/sin(angle).
    // Bound the raw dot used to choose the actual f32 blend branch too.
    let lower_dot = (dot - 8.0 * f64::from(f32::EPSILON)).max(0.0);
    let sine_weight_rate = if lower_dot > 0.9995 {
        1.0
    } else {
        let angle = det::acos(lower_dot as f32);
        f64::from(angle) / f64::from(det::sin(angle))
    };
    let rate = 4.0 * tangent * norm_ratio * sine_weight_rate;
    // The unnormalized interpolation u stays in the endpoint plane. Both linear and sine
    // weights give u'' parallel to u, so the angular acceleration is bounded by
    // 2 * rate * |u'| / |u|. The sine-weight sum is at least one on the shortest hemisphere.
    let norm_floor = a2.min(b2).sqrt() * ((1.0 + alignment) * 0.5).sqrt();
    let chord = (a2 + b2 - 2.0 * dot).max(0.0).sqrt();
    let slope = if lower_dot > 0.9995 {
        chord
    } else {
        sine_weight_rate * (chord + a2.max(b2).sqrt() * (1.0 - lower_dot))
    };
    Ok(AngularMotion { rate, acceleration: 2.0 * rate * slope / norm_floor })
}

pub(super) fn interpolate_local(from: Transform, to: Transform, fraction: f32) -> Transform {
    Transform {
        translation: from.translation.lerp(to.translation, fraction),
        rotation: det::quat_slerp(from.rotation, to.rotation, fraction),
        scale: from.scale.lerp(to.scale, fraction),
    }
}

#[cfg(test)]
mod tests {
    use sapphire_math::{Affine3A, Vec3};

    use super::*;
    use crate::model;

    fn point(before: &BodyFrame, after: &BodyFrame, parents: &[u16], offset: Vec3, t: f32) -> Vec3 {
        let locals: Vec<_> =
            before.locals.iter().zip(&after.locals).map(|(&a, &b)| interpolate_local(a, b, t)).collect();
        let mut model = vec![Affine3A::IDENTITY; parents.len()];
        model::walk(parents.iter().copied(), &locals, &mut model);
        before
            .root
            .lerp(after.root, t)
            .affine()
            .transform_point3(model[model.len() - 1].transform_point3(offset))
    }

    #[test]
    fn rejects_invalid_quaternions_instead_of_returning_a_finite_bound() {
        let before = BodyFrame::blank(1, 0);
        let mut after = BodyFrame::blank(1, 0);
        after.locals[0].rotation = Quat::from_xyzw(0.0, 0.0, 0.0, 0.0);
        assert_eq!(region_bound(&before, &after, &[NO_PARENT], 0, 1.0), Err(BodyMotionError::Unsupported));
    }

    #[test]
    fn motion_bound_rejects_a_nonrepresentable_intermediate_yaw() {
        let mut before = BodyFrame::blank(1, 0);
        let mut after = BodyFrame::blank(1, 0);
        before.root.yaw = f32::MAX;
        after.root.yaw = -f32::MAX;
        assert!(before.root.valid() && after.root.valid());
        assert!(!before.root.lerp(after.root, 0.5).yaw.is_finite());
        assert_eq!(region_bound(&before, &after, &[NO_PARENT], 0, 1.0), Err(BodyMotionError::Unsupported));
    }

    #[test]
    fn motion_bound_rejects_an_overflowing_ancestor_even_with_a_tiny_leaf() {
        let mut frame = BodyFrame::blank(3, 0);
        frame.locals[0].scale = Vec3::splat(1e20);
        frame.locals[1].scale = Vec3::splat(1e20);
        frame.locals[2].scale = Vec3::splat(1e-20);
        let parents = [NO_PARENT, 0, 1];
        assert!(!point(&frame, &frame, &parents, Vec3::X, 0.5).is_finite());
        assert_eq!(region_bound(&frame, &frame, &parents, 2, 0.0), Err(BodyMotionError::Unsupported));
    }

    #[test]
    fn a_full_root_turn_cannot_use_the_endpoint_chord() {
        let before = BodyFrame::blank(1, 0);
        let mut after = BodyFrame::blank(1, 0);
        after.root.yaw = std::f32::consts::TAU;
        let parents = [NO_PARENT];
        let region = region_bound(&before, &after, &parents, 0, 1.0).unwrap();
        let BodyMotionBound::Continuous { metres_per_step, numerical_margin, .. } = region else {
            panic!("expected a continuous interval");
        };
        let start = point(&before, &after, &parents, Vec3::X, 0.0);
        let end = point(&before, &after, &parents, Vec3::X, 1.0);
        let mid = point(&before, &after, &parents, Vec3::X, 0.5);
        assert!(length(start - end) < 1e-5);
        assert!(length(start - mid) > 1.99);
        assert!(length(start - mid) <= metres_per_step * 0.5 + numerical_margin);
    }

    #[test]
    fn chord_bound_covers_rotation_branch_threshold_norm_drift_and_negated_endpoints() {
        let mut before = BodyFrame::blank(1, 0);
        before.locals[0].rotation = Quat::from_xyzw(0.0, 0.0, 0.0, 1.0 + 2.0 * f32::EPSILON);
        let parents = [NO_PARENT];
        for cos in [0.999_498_f32, 0.999_502, 0.01] {
            for sign in [-1.0, 1.0] {
                let mut after = BodyFrame::blank(1, 0);
                let scale = sign * (1.0 - 3.0 * f32::EPSILON);
                after.locals[0].rotation =
                    Quat::from_xyzw(0.0, (1.0 - cos * cos).sqrt() * scale, 0.0, cos * scale);
                let raw_dot = (before.locals[0].rotation.w * after.locals[0].rotation.w).abs();
                assert_eq!(raw_dot > 0.9995, cos > 0.9995);
                let BodyMotionBound::Continuous { metres_per_step_squared, numerical_margin, .. } =
                    region_bound(&before, &after, &parents, 0, 1.0).unwrap()
                else {
                    panic!("expected a continuous interval");
                };
                for tick in 0..64 {
                    let a = point(&before, &after, &parents, Vec3::X, tick as f32 / 64.0);
                    let b = point(&before, &after, &parents, Vec3::X, (tick + 1) as f32 / 64.0);
                    let mid = point(&before, &after, &parents, Vec3::X, (tick as f32 + 0.5) / 64.0);
                    let sag = metres_per_step_squared / (8.0 * 64.0 * 64.0);
                    assert!(distance(mid, a.lerp(b, 0.5)) <= sag + numerical_margin);
                }
            }
        }
    }

    #[test]
    fn a_nested_rotating_and_scaling_chain_bounds_the_drawn_points() {
        let before = BodyFrame::blank(3, 0);
        let mut after = BodyFrame::blank(3, 0);
        after.root.translation = Vec3::new(0.15, -0.02, -0.18);
        after.root.yaw = 0.32;
        after.root.scale = 0.8;
        after.locals[0].translation = Vec3::Y * 0.13;
        after.locals[0].rotation = det::quat_from_rotation_y(0.004);
        after.locals[1].translation = Vec3::new(0.3, 0.65, 0.06);
        after.locals[1].rotation = det::quat_from_rotation_x(1.14);
        after.locals[1].scale = Vec3::new(1.0, 0.99, 1.02);
        after.locals[2].translation = Vec3::new(-0.12, 0.1, 0.3);
        after.locals[2].rotation = det::quat_from_rotation_z(-2.9);
        let parents = [NO_PARENT, 0, 1];
        let region = region_bound(&before, &after, &parents, 2, 1.2).unwrap();
        let BodyMotionBound::Continuous { metres_per_step, metres_per_step_squared, numerical_margin } =
            region
        else {
            panic!("expected a continuous interval");
        };
        for offset in [Vec3::X, Vec3::Y, -Vec3::Z] {
            let offset = offset * 1.2;
            for interval in [1, 4, 16, 64] {
                let step = 1.0 / interval as f32;
                for start in 0..interval {
                    let at = start as f32 * step;
                    let a = point(&before, &after, &parents, offset, at);
                    let b = point(&before, &after, &parents, offset, at + step);
                    for mix in [0.125, 0.5, 0.875] {
                        let actual = point(&before, &after, &parents, offset, at + step * mix);
                        let deviation = distance(actual, a.lerp(b, mix));
                        let sag = metres_per_step_squared * f64::from(step) * f64::from(step) / 8.0;
                        assert!(deviation <= sag + numerical_margin, "{deviation} exceeds {sag}");
                    }
                }
            }
            for tick in 0..256 {
                let from = point(&before, &after, &parents, offset, tick as f32 / 256.0);
                let to = point(&before, &after, &parents, offset, (tick + 1) as f32 / 256.0);
                assert!(length(to - from) <= metres_per_step / 256.0 + numerical_margin);
            }
        }
    }
}
