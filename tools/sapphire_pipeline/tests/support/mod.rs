//! Colliders against what's drawn, which `arena.rs` and `zone_ward.rs` both hold.

#![allow(dead_code, reason = "each test uses its own part")]

use glam::{Quat, Vec3};
use sapphire_colliders::{ColliderSet, CookedShape, Group};

pub type Triangle = [Vec3; 3];

/// A collider as a box in its own space: a capsule's box, for this test's purposes.
pub struct Volume {
    pub name: String,
    pub center: Vec3,
    pub rotation: Quat,
    pub half: Vec3,
    /// In the world group, rather than one of the blockers nothing draws.
    pub world: bool,
}

impl Volume {
    pub fn local(&self, point: Vec3) -> Vec3 {
        self.rotation.inverse() * (point - self.center)
    }

    /// How far `point` is outside the box, along its worst axis: 0 or less is inside.
    pub fn outside(&self, point: Vec3) -> f32 {
        (self.local(point).abs() - self.half).max_element()
    }
}

pub fn volumes(set: &ColliderSet) -> Vec<Volume> {
    set.colliders
        .iter()
        .map(|collider| {
            let half = match collider.shape {
                CookedShape::Box { half_extents } => Vec3::from(half_extents),
                CookedShape::Capsule { half_height, radius } => {
                    Vec3::new(radius, half_height + radius, radius)
                }
                CookedShape::Sphere { radius } => Vec3::splat(radius),
                CookedShape::Mesh { .. } => panic!("{}: a mesh has no box here", collider.name),
            };
            Volume {
                name: collider.name.clone(),
                center: Vec3::from(collider.position),
                rotation: Quat::from_array(collider.rotation),
                half,
                world: collider.memberships & Group::World.mask().bits() != 0,
            }
        })
        .collect()
}

/// Sutherland–Hodgman against one plane: what of `polygon` has `distance(point) >= 0`.
pub fn clip_plane(polygon: &[Vec3], distance: impl Fn(Vec3) -> f32) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(polygon.len() + 1);
    for k in 0..polygon.len() {
        let (a, b) = (polygon[k], polygon[(k + 1) % polygon.len()]);
        let (da, db) = (distance(a), distance(b));
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

/// What of a triangle lies in the band of heights.
pub fn in_band(triangle: Triangle, band: [f32; 2]) -> Vec<Vec3> {
    let above = clip_plane(&triangle, |p| p.y - band[0]);
    clip_plane(&above, |p| band[1] - p.y)
}
