//! What a bake reads, as triangles: a collider set's solids.

use sapphire_colliders::{ColliderSet, CookedCollider, CookedShape, GroupMask};
use sapphire_math::det;

/// Three corners, in metres. A triangle faces up when its corners run so that `(b − a) × (c − a)`
/// has a positive y.
pub type Triangle = [[f32; 3]; 3];

/// How many sides a round collider's prism has.
const ROUND_SIDES: u32 = 12;

/// Whether a triangle's slope is walkable, for `slope_cos`, the cosine of the steepest walkable
/// slope. In `f64` scalars with no root and no reciprocal, so every architecture reads it alike.
pub fn is_walkable(triangle: &Triangle, slope_cos: f32) -> bool {
    let [a, b, c] = triangle.map(|corner| corner.map(f64::from));
    let (ux, uy, uz) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
    let (vx, vy, vz) = (c[0] - a[0], c[1] - a[1], c[2] - a[2]);
    let (nx, ny, nz) = (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx);
    let cos = f64::from(slope_cos);
    ny > 0.0 && ny * ny > cos * cos * (nx * nx + ny * ny + nz * nz)
}

/// The solids of a collider set in the groups `solid` names, as triangles. Round shapes become
/// prisms of their full height, which is all a bake can tell apart at a character's scale.
pub fn colliders_to_triangles(set: &ColliderSet, solid: GroupMask) -> Vec<Triangle> {
    let mut triangles = Vec::new();
    for collider in &set.colliders {
        if collider.memberships & solid.bits() == 0 {
            continue;
        }
        let place = |local: [f32; 3]| placed(collider, local);
        match collider.shape {
            CookedShape::Box { half_extents: [x, y, z] } => {
                let corners = [
                    [-x, -y, -z],
                    [-x, -y, z],
                    [x, -y, z],
                    [x, -y, -z],
                    [-x, y, -z],
                    [-x, y, z],
                    [x, y, z],
                    [x, y, -z],
                ]
                .map(place);
                // Each face from outside, as two triangles.
                for [a, b, c, d] in
                    [[4, 5, 6, 7], [0, 3, 2, 1], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]]
                {
                    triangles.push([corners[a], corners[b], corners[c]]);
                    triangles.push([corners[a], corners[c], corners[d]]);
                }
            }
            CookedShape::Sphere { radius } => prism(&mut triangles, radius, radius, &place),
            CookedShape::Capsule { half_height, radius } => {
                prism(&mut triangles, radius, half_height + radius, &place);
            }
            CookedShape::Mesh { mesh } => {
                let mesh = &set.meshes[mesh as usize];
                for corners in &mesh.indices {
                    let [a, b, c] = corners.map(|index| place(mesh.vertices[index as usize]));
                    // Two-sided, so whichever way up it was written, its top is walkable.
                    if is_walkable(&[a, b, c], 0.0) {
                        triangles.push([a, b, c]);
                    } else {
                        triangles.push([a, c, b]);
                    }
                }
            }
        }
    }
    triangles
}

fn prism(triangles: &mut Vec<Triangle>, radius: f32, half: f32, place: &dyn Fn([f32; 3]) -> [f32; 3]) {
    let rim = |side: u32, y: f32| {
        let turn = side as f32 / ROUND_SIDES as f32 * std::f32::consts::TAU;
        let (sin, cos) = det::sin_cos(turn);
        place([radius * cos, y, radius * sin])
    };
    let (top, bottom) = (place([0.0, half, 0.0]), place([0.0, -half, 0.0]));
    for side in 0..ROUND_SIDES {
        let (a_up, b_up) = (rim(side, half), rim(side + 1, half));
        let (a_down, b_down) = (rim(side, -half), rim(side + 1, -half));
        triangles.push([top, b_up, a_up]);
        triangles.push([bottom, a_down, b_down]);
        triangles.push([a_up, b_up, b_down]);
        triangles.push([a_up, b_down, a_down]);
    }
}

/// A local point of a collider, in world space: scalars, since this is what the bake's bytes rest on.
fn placed(collider: &CookedCollider, local: [f32; 3]) -> [f32; 3] {
    let [qx, qy, qz, qw] = collider.rotation;
    let [x, y, z] = local;
    // v + w t + q × t, where t is 2 (q × v).
    let (tx, ty, tz) = (2.0 * (qy * z - qz * y), 2.0 * (qz * x - qx * z), 2.0 * (qx * y - qy * x));
    let at = collider.position;
    [
        at[0] + x + qw * tx + (qy * tz - qz * ty),
        at[1] + y + qw * ty + (qz * tx - qx * tz),
        at[2] + z + qw * tz + (qx * ty - qy * tx),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sapphire_colliders::{ColliderDocument, ColliderEntry, ColliderShape, Group, Rotation};

    #[test]
    fn a_floor_is_walkable_only_from_above_and_up_to_the_slope() {
        // 45° is a cosine of 0.7071, so a 40° limit refuses it and a 50° limit takes it.
        let ramp: Triangle = [[0.0, 0.0, 0.0], [0.0, 1.0, 1.0], [1.0, 0.0, 0.0]];
        assert!(is_walkable(&ramp, 0.6428));
        assert!(!is_walkable(&ramp, 0.766));
        let [a, b, c] = ramp;
        assert!(!is_walkable(&[a, c, b], 0.0), "its underside");
        assert!(!is_walkable(&[a, a, a], 0.0), "no area, no normal");
    }

    fn entry(shape: ColliderShape, rotation: Rotation, groups: Vec<Group>) -> ColliderEntry {
        ColliderEntry { name: "solid".to_owned(), shape, position: [1.0, 2.0, 3.0], rotation, groups }
    }

    #[test]
    fn a_box_has_one_walkable_face_and_its_turn_keeps_it_level() {
        let shape = ColliderShape::Box { half_extents: [1.0, 0.5, 2.0] };
        let rotation = Rotation::AxisAngle { axis: [0.0, 1.0, 0.0], degrees: 30.0 };
        let document = ColliderDocument { colliders: vec![entry(shape, rotation, vec![Group::World])] };
        let triangles = colliders_to_triangles(&document.cook().expect("cooks"), Group::World.mask());
        assert_eq!(triangles.len(), 12);
        let walkable: Vec<_> = triangles.iter().filter(|t| is_walkable(t, 0.64)).collect();
        assert_eq!(walkable.len(), 2);
        for corner in walkable.iter().flat_map(|t| t.iter()) {
            assert!((corner[1] - 2.5).abs() < 1e-5, "{corner:?}");
        }
    }

    #[test]
    fn a_capsule_stands_as_tall_as_it_is() {
        let shape = ColliderShape::Capsule { half_height: 1.0, radius: 0.25 };
        let document = ColliderDocument { colliders: vec![entry(shape, Rotation::None, vec![Group::World])] };
        let triangles = colliders_to_triangles(&document.cook().expect("cooks"), Group::World.mask());
        let top = triangles.iter().flat_map(|t| t.iter()).map(|c| c[1]).fold(f32::MIN, f32::max);
        assert_eq!(top, 3.25);
        assert_eq!(triangles.iter().filter(|t| is_walkable(t, 0.64)).count(), ROUND_SIDES as usize);
    }

    #[test]
    fn a_group_nobody_asked_for_isnt_solid() {
        let shape = ColliderShape::Box { half_extents: [1.0; 3] };
        let document =
            ColliderDocument { colliders: vec![entry(shape, Rotation::None, vec![Group::CameraBlocker])] };
        assert!(colliders_to_triangles(&document.cook().expect("cooks"), Group::World.mask()).is_empty());
    }
}
