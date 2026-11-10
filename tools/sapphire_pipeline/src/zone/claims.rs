//! A zone's colliders held to what stands for them (M4 X1).
//!
//! A zone's colliders are a document of their own (`.colliders.ron`), written apart from what's drawn: a
//! box per building, wall or post, cheaper to walk on than the meshes and free to leave out what a body
//! should pass through. A placement or an instance names the colliders that stand for what it draws
//! (`colliders`), and a cook holds each of those to it: every side of the collider a body meets, along X
//! and Z in its own space, has something that names it drawn within [`TOLERANCE`] of it, inside the box or
//! just past it. So moving, turning or shrinking a placement without its colliders, or a collider without
//! what it stands for, is refused, naming both, where it used to leave a post drawn in one place and solid
//! in another.
//!
//! What it can't see: something drawn past its colliders' sides, as a placement scaled up leaves, since a
//! side only has to have something at it; a collider nothing names, such as the ground, a ramp or a plant's
//! trunk; and what isn't an assembly, which has no triangles here. `tests/zone_ward.rs` holds the Lantern
//! Ward both ways, through its navmesh.

use std::collections::BTreeMap;

use glam::{Mat4, Quat, Vec3};
use sapphire_colliders::{ColliderSet, CookedShape};

use super::{ZoneCookError, ZonePlan, placement_transform};

/// How far a collider's side may stand from what's drawn there: a little under half a character's radius,
/// past which an invisible wall starts to show, as the arena's and the ward's tests hold.
pub const TOLERANCE: f32 = 0.12;

/// A collider as a box in its own space.
struct Volume {
    center: Vec3,
    rotation: Quat,
    half: Vec3,
}

impl Volume {
    fn local(&self, point: Vec3) -> Vec3 {
        self.rotation.inverse() * (point - self.center)
    }
}

/// What names a collider: a placement or an instance, by its field and its transform.
struct Claimant {
    field: String,
    prefab: sapphire_assets::AssetId,
    transform: Mat4,
}

/// Holds every collider a placement or an instance names to what those draw, from the zone's cooked
/// collider set (see the [module docs](self)). Fails on the first collider that disagrees, or that the set
/// doesn't have, by the field that names it.
pub fn check(plan: &ZonePlan, colliders: &[u8]) -> Result<(), ZoneCookError> {
    let document = plan.document();
    let set = ColliderSet::from_cooked(colliders)
        .map_err(|error| ZoneCookError::invalid("colliders", format!("it doesn't read: {error}")))?;
    let placements = document.placements.iter().enumerate().map(|(index, placement)| {
        let transform = placement_transform(placement.at, placement.turn, placement.scale);
        (format!("placements[{index}]"), placement.prefab, transform, &placement.colliders)
    });
    let instances = document.instances.iter().enumerate().map(|(index, instance)| {
        let transform = placement_transform(instance.at, instance.turn, instance.scale);
        (format!("instances[{index}]"), instance.mesh, transform, &instance.colliders)
    });
    // By collider, in name order, so the first refused is the same every cook.
    let mut claimed: BTreeMap<&str, Vec<Claimant>> = BTreeMap::new();
    for (field, prefab, transform, claims) in placements.chain(instances) {
        for (index, claim) in claims.iter().enumerate() {
            let Some(collider) = set.colliders.iter().find(|collider| collider.name == *claim) else {
                let problem = format!("`{claim}` isn't one of the zone's colliders");
                return Err(ZoneCookError::invalid(format!("{field}.colliders[{index}]"), problem));
            };
            let entry = claimed.entry(collider.name.as_str()).or_default();
            entry.push(Claimant { field: field.clone(), prefab, transform });
        }
    }
    for (name, claimants) in &claimed {
        let collider = set.colliders.iter().find(|collider| collider.name == *name).expect("found above");
        let names = claimants.iter().map(|claimant| claimant.field.as_str()).collect::<Vec<_>>().join(", ");
        let field = format!("{}.colliders", claimants[0].field);
        let half = match collider.shape {
            CookedShape::Box { half_extents } => Vec3::from(half_extents),
            CookedShape::Capsule { half_height, radius } => Vec3::new(radius, half_height + radius, radius),
            CookedShape::Sphere { radius } => Vec3::splat(radius),
            CookedShape::Mesh { .. } => {
                let problem = format!("`{name}` is a mesh, which has no sides to hold to what's drawn");
                return Err(ZoneCookError::invalid(field, problem));
            }
        };
        let volume = Volume {
            center: Vec3::from(collider.position),
            rotation: Quat::from_array(collider.rotation),
            half,
        };
        let reach = volume.half + Vec3::splat(TOLERANCE);
        let (mut least, mut most) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for claimant in claimants {
            let triangles = plan.triangles(claimant.prefab, &claimant.field)?;
            for triangle in triangles.iter() {
                let mut polygon: Vec<Vec3> = triangle
                    .iter()
                    .map(|&corner| volume.local(claimant.transform.transform_point3(corner)))
                    .collect();
                for axis in 0..3 {
                    polygon = clip_plane(&polygon, |point| point[axis] + reach[axis]);
                    polygon = clip_plane(&polygon, |point| reach[axis] - point[axis]);
                }
                for point in polygon {
                    least = least.min(point);
                    most = most.max(point);
                }
            }
        }
        for (axis, letter) in [(0, "x"), (2, "z")] {
            for (side, gap) in [("-", least[axis] + volume.half[axis]), ("+", volume.half[axis] - most[axis])]
            {
                // Nothing within reach at all leaves both infinite.
                if gap > TOLERANCE {
                    let at = volume.center;
                    let gap = if gap.is_finite() { format!("{gap:.3} m") } else { "far".to_owned() };
                    let problem = format!(
                        "collider `{name}` at ({:.2}, {:.2}, {:.2}) has its {side}{letter} side {gap} from \
                         anything {names} draws, past the {TOLERANCE} m it may be: move the two together",
                        at.x, at.y, at.z
                    );
                    return Err(ZoneCookError::invalid(field, problem));
                }
            }
        }
    }
    Ok(())
}

/// Sutherland–Hodgman against one plane: what of `polygon` has `distance(point) >= 0`.
fn clip_plane(polygon: &[Vec3], distance: impl Fn(Vec3) -> f32) -> Vec<Vec3> {
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
