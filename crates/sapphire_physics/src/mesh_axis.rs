//! A private-mesh query that returns a conservative separation or a geometric witness.

use glam::{DVec3, Vec3};
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::Pose as ParryPose;
use parry3d::partitioning::BvhNode;
use parry3d::shape::TriMesh;

use crate::stats::Counters;
use crate::world::{ColliderId, PhysicsWorld, StaticGeometry};

const MAX_AXIS_COORDINATE: f32 = 64.0;
const MAX_MESH_COORDINATE: f32 = 2.0;
// This covers the basic f64 support and witness arithmetic in the admitted socket-local domain.
const METRE_SLACK: f64 = 1e-9;

/// A validated identity-space triangle mesh in an immutable physics world.
#[derive(Clone, Copy)]
pub struct MeshAxisBinding {
    id: ColliderId,
    statics_digest: u64,
}

/// A shape-probe result. Only Witness is an actual raw-radius overlap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MeshAxisProbe {
    Separated,
    Possible,
    Witness(MeshAxisWitness),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshAxisWitness {
    pub triangle: u32,
    pub axis: [f64; 3],
    pub mesh: [f64; 3],
    pub distance: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MeshAxisError {
    #[error("mesh-axis probe requires a compatible bounded identity-space triangle mesh")]
    Mesh,
    #[error("mesh-axis probe exceeds its supported coordinate or radius domain")]
    Domain,
}

impl PhysicsWorld {
    /// Bind an immutable private mesh before using it for certified axis probes.
    pub fn bind_mesh_axis(&self, id: ColliderId) -> Result<MeshAxisBinding, MeshAxisError> {
        let mesh = self.axis_mesh(id).ok_or(MeshAxisError::Mesh)?;
        if mesh.vertices().iter().any(|v| v.abs().max_element() > MAX_MESH_COORDINATE) {
            return Err(MeshAxisError::Mesh);
        }
        Ok(MeshAxisBinding { id, statics_digest: self.statics_digest() })
    }

    /// Read an axis against the bound mesh. The larger radius is only for certifying separation.
    /// An actual witness is always within the smaller radius, with numerical slack.
    pub fn probe_mesh_axis(
        &self,
        binding: MeshAxisBinding,
        axis: [Vec3; 2],
        radius: f64,
        separation_radius: f64,
    ) -> Result<MeshAxisProbe, MeshAxisError> {
        if binding.statics_digest != self.statics_digest() {
            return Err(MeshAxisError::Mesh);
        }
        let mesh = self.axis_mesh(binding.id).ok_or(MeshAxisError::Mesh)?;
        if !axis.into_iter().all(|p| p.is_finite() && p.abs().max_element() <= MAX_AXIS_COORDINATE)
            || !radius.is_finite()
            || radius <= 0.0
            || !(radius..=f64::from(MAX_AXIS_COORDINATE)).contains(&separation_radius)
        {
            return Err(MeshAxisError::Domain);
        }
        let axis = axis.map(Vec3::as_dvec3);
        let pad = DVec3::splat(separation_radius + METRE_SLACK);
        let mins = (axis[0].min(axis[1]) - pad).as_vec3().to_array().map(f32::next_down);
        let maxs = (axis[0].max(axis[1]) + pad).as_vec3().to_array().map(f32::next_up);
        let area = Aabb::new(Vec3::from_array(mins), Vec3::from_array(maxs));
        let mut possible = false;
        let mut witness: Option<MeshAxisWitness> = None;
        let mut tested = 0;
        Counters::add(&self.counters.penetration_queries, 1);
        for index in mesh.bvh().leaves(|node: &BvhNode| node.aabb().intersects(&area)) {
            tested += 1;
            let triangle = mesh.triangle(index);
            let triangle = [triangle.a, triangle.b, triangle.c].map(Vec3::as_dvec3);
            let points = closest(axis, triangle);
            let distance = points.distance_squared().sqrt();
            if distance + METRE_SLACK < radius {
                if witness.is_none_or(|hit| (distance, index) < (hit.distance, hit.triangle)) {
                    witness = Some(MeshAxisWitness {
                        triangle: index,
                        axis: points.axis.to_array(),
                        mesh: points.steel.to_array(),
                        distance,
                    });
                }
            } else if support_gap(axis, triangle, points) <= separation_radius + METRE_SLACK {
                possible = true;
            }
        }
        Counters::add(&self.counters.narrow_tests, tested);
        Ok(match witness {
            Some(hit) => MeshAxisProbe::Witness(hit),
            None if possible => MeshAxisProbe::Possible,
            None => MeshAxisProbe::Separated,
        })
    }

    fn axis_mesh(&self, id: ColliderId) -> Option<&TriMesh> {
        let index = self.statics.binary_search_by_key(&id, |entry| entry.id).ok()?;
        let entry = &self.statics[index];
        let StaticGeometry::Mesh(mesh) = &entry.geometry else {
            return None;
        };
        (entry.pose == ParryPose::IDENTITY).then_some(mesh.as_ref())
    }
}

#[derive(Clone, Copy, Debug)]
struct Closest {
    axis: DVec3,
    steel: DVec3,
}

impl Closest {
    fn distance_squared(self) -> f64 {
        self.axis.distance_squared(self.steel)
    }

    fn take(&mut self, axis: DVec3, steel: DVec3) {
        let candidate = Self { axis, steel };
        if candidate.distance_squared() < self.distance_squared() {
            *self = candidate;
        }
    }
}

fn project_to_segment(point: DVec3, [a, b]: [DVec3; 2]) -> DVec3 {
    let direction = b - a;
    let length_squared = direction.length_squared();
    if length_squared == 0.0 {
        a
    } else {
        a + direction * ((point - a).dot(direction) / length_squared).clamp(0.0, 1.0)
    }
}

// The candidate only selects a direction. A support gap in any direction is a lower bound
// on distance, even when the candidate is not the exact nearest pair of features.
fn support_gap(axis: [DVec3; 2], triangle: [DVec3; 3], pair: Closest) -> f64 {
    let direction = pair.axis - pair.steel;
    let length = direction.length();
    if length < 1e-12 {
        return 0.0;
    }
    let axis_support = axis.map(|point| (point - triangle[0]).dot(direction));
    let steel_support = triangle.map(|point| (point - triangle[0]).dot(direction));
    (axis_support[0].min(axis_support[1]) - steel_support[0].max(steel_support[1]).max(steel_support[2]))
        / length
}

fn face_point(point: DVec3, [a, b, c]: [DVec3; 3], normal: DVec3) -> Option<DVec3> {
    let edges = [[b, c], [c, a], [a, b]];
    let weights = edges.map(|[a, b]| (b - a).cross(point - a).dot(normal));
    let sum = weights.iter().sum::<f64>();
    if weights.into_iter().any(|weight| weight < 0.0) || sum <= 0.0 {
        return None;
    }
    // Reconstruct an actual triangle point from nonnegative barycentric weights.
    Some(a + (b - a) * (weights[1] / sum) + (c - a) * (weights[2] / sum))
}

fn closest(axis: [DVec3; 2], triangle @ [a, b, c]: [DVec3; 3]) -> Closest {
    let edges = [[a, b], [b, c], [c, a]];
    let normal = (b - a).cross(c - a);
    let normal_squared = normal.length_squared();
    let [p, q] = axis;
    let direction = q - p;
    let signed = axis.map(|point| (point - a).dot(normal));
    let crossing = signed[0] - signed[1];
    if crossing != 0.0 {
        let fraction = signed[0] / crossing;
        if (0.0..=1.0).contains(&fraction) {
            let point = p + direction * fraction;
            if let Some(steel) = face_point(point, triangle, normal) {
                return Closest { axis: point, steel };
            }
        }
    }

    let mut result = Closest { axis: p, steel: a };
    for (point, signed) in axis.into_iter().zip(signed) {
        let projected = point - normal * (signed / normal_squared);
        if let Some(steel) = face_point(projected, triangle, normal) {
            result.take(point, steel);
        }
    }
    for edge @ [a, b] in edges {
        result.take(p, project_to_segment(p, edge));
        result.take(q, project_to_segment(q, edge));
        result.take(project_to_segment(a, axis), a);
        result.take(project_to_segment(b, axis), b);

        let edge_direction = b - a;
        let cross = direction.cross(edge_direction);
        let cross_squared = cross.length_squared();
        if cross_squared != 0.0 {
            let offset = a - p;
            let on_axis = offset.cross(edge_direction).dot(cross) / cross_squared;
            let on_edge = offset.cross(direction).dot(cross) / cross_squared;
            if (0.0..=1.0).contains(&on_axis) && (0.0..=1.0).contains(&on_edge) {
                result.take(p + direction * on_axis, a + edge_direction * on_edge);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Group, Pose, StaticCollider, StaticShape};

    #[test]
    fn bound_mesh_separates_expanded_only_and_witnessed_probes() {
        let id = ColliderId(3);
        let world = PhysicsWorld::new(
            vec![StaticCollider {
                id,
                shape: StaticShape::Mesh {
                    vertices: vec![Vec3::ZERO, Vec3::X, Vec3::Y],
                    indices: vec![[0, 1, 2]],
                },
                pose: Pose::IDENTITY,
                memberships: Group::World.mask(),
            }],
            0,
        )
        .unwrap();
        let binding = world.bind_mesh_axis(id).unwrap();
        let axis = [Vec3::new(0.2, 0.2, 0.3), Vec3::new(0.4, 0.2, 0.6)];
        assert_eq!(world.probe_mesh_axis(binding, axis, 0.1, 0.2), Ok(MeshAxisProbe::Separated));
        assert_eq!(world.probe_mesh_axis(binding, axis, 0.1, 0.4), Ok(MeshAxisProbe::Possible));
        let MeshAxisProbe::Witness(hit) = world.probe_mesh_axis(binding, axis, 0.4, 0.4).unwrap() else {
            panic!("expected real contact");
        };
        assert!(DVec3::from_array(hit.mesh).abs_diff_eq(v(0.2, 0.2, 0.0), 1e-7));
        assert_eq!(world.probe_mesh_axis(binding, axis, 0.1, f64::INFINITY), Err(MeshAxisError::Domain));
    }

    const TRIANGLE: [DVec3; 3] = [DVec3::ZERO, DVec3::X, DVec3::Y];
    const fn v(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }

    #[test]
    fn through_face_uses_the_axis_intersection_instead_of_projecting_its_midpoint() {
        let axis = [v(-2.0, 0.35, 2.0), v(0.6, 0.35, -0.1)];
        let result = closest(axis, TRIANGLE);
        assert!(result.distance_squared() < 1e-25);
        assert!(result.steel.abs_diff_eq(v(0.476_190_476, 0.35, 0.0), 1e-9));
        assert!((axis[0] + axis[1]).x < 0.0, "the old midpoint projection lies off the steel");
    }

    #[test]
    fn shared_nearest_edge_can_be_interior_to_both_segments() {
        let axis = [v(0.3, -0.3, -0.2), v(0.7, -0.3, 0.2)];
        let hit = closest(axis, TRIANGLE);
        assert!(hit.axis.abs_diff_eq(v(0.5, -0.3, 0.0), 1e-10));
        assert!(hit.steel.abs_diff_eq(v(0.5, 0.0, 0.0), 1e-10));
        assert!((hit.distance_squared() - 0.09).abs() < 1e-10);
    }

    #[test]
    fn the_axis_support_gap_remains_conservative_with_a_bad_candidate() {
        let axis = [v(0.2, 0.2, 0.3), v(0.4, 0.2, 0.6)];
        let nearest = closest(axis, TRIANGLE);
        let exact = nearest.distance_squared().sqrt();
        assert!((exact - 0.3).abs() < 1e-10);
        assert!((support_gap(axis, TRIANGLE, nearest) - exact).abs() < 1e-10);
        let biased = Closest { axis: axis[1], steel: TRIANGLE[2] };
        assert!(support_gap(axis, TRIANGLE, biased) <= exact);
    }

    #[test]
    fn coplanar_crossing_and_degenerate_axis_keep_real_witnesses() {
        let crossing = closest([v(-0.2, 0.2, 0.0), v(1.1, 0.2, 0.0)], TRIANGLE);
        assert!(crossing.distance_squared() < 1e-24);
        let point = closest([v(0.2, 0.2, 0.3); 2], TRIANGLE);
        assert!(point.steel.abs_diff_eq(v(0.2, 0.2, 0.0), 1e-10));
        assert!((point.distance_squared() - 0.09).abs() < 1e-10);
    }
}
