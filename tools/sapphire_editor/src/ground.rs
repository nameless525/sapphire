//! The ground a zone's objects are snapped to (M6 T9b): its colliders, as the editor has their document now
//! (edits not saved included, so what's seen is what's met), cooked as a cook would and queried by the physics'
//! own ray. What the ray sees is every collider of the zone's authored document but those the object itself names:
//! a placement standing in its own collider would otherwise land on its own roof. Never the cooked pack, which can
//! be older than the document.
//!
//! [`Ground::below`] is what [`crate::commands::Command::SnapToGround`] drops an object by; [`Ground::hit`] is a
//! ray's first hit, for a drag from the browser to place at (T10).

//!
//! Only colliders in [`Group::World`] are ground, as [`PhysicsWorld::floor_below`] has it: a camera's or a
//! character's blocker (an arena's unseen walls) is passed through. A snap of several objects cooks the colliders
//! once for each, leaving out that one's own, so each lands on the others where they stood before the snap: two
//! crates stacked and snapped together leave the top one where the lower one's top was.

use sapphire_colliders::ColliderDocument;
use sapphire_math::Vec3;
use sapphire_physics::{Group, GroupMask, PhysicsWorld, QueryFilter};

use crate::gizmo::Ray;

/// How far over an object a ray for the ground under it starts, in metres: one sunk this far still lands on top,
/// and nothing hanging over it further up is met.
pub const LIFT: f32 = 0.05;

/// How far a ray for the ground reaches, in metres.
pub const REACH: f32 = 10_000.0;

/// A zone's colliders, for rays (see the [module docs](self)).
pub struct Ground {
    /// None for a zone with no colliders left to meet.
    world: Option<PhysicsWorld>,
}

impl std::fmt::Debug for Ground {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Ground").field("colliders", &self.world.is_some()).finish()
    }
}

impl Ground {
    /// The colliders of `document` but those named in `leaving_out`, or why they don't cook.
    pub fn of(document: &ColliderDocument, leaving_out: &[String]) -> Result<Self, String> {
        let colliders: Vec<_> =
            (document.colliders.iter()).filter(|entry| !leaving_out.contains(&entry.name)).cloned().collect();
        if colliders.is_empty() {
            return Ok(Self { world: None });
        }
        let set = ColliderDocument { colliders }.cook().map_err(|error| error.to_string())?;
        let bytes = set.to_cooked().map_err(|error| error.to_string())?;
        let archived = sapphire_physics::ColliderSet::read(&bytes).map_err(|error| error.to_string())?;
        let world = PhysicsWorld::from_cooked(archived, 0).map_err(|error| error.to_string())?;
        Ok(Self { world: Some(world) })
    }

    /// Where `ray` first meets the ground, within [`REACH`]; None for a ray that starts inside a collider, which
    /// would meet it where it starts.
    #[must_use]
    pub fn hit(&self, ray: Ray) -> Option<Vec3> {
        let world = self.world.as_ref()?;
        let ground = QueryFilter::new(Group::World.mask()).excluding_groups(GroupMask::ALL);
        let hit = world.cast_ray(ray.origin, ray.direction, REACH, &ground)?;
        (hit.distance > 0.0).then_some(hit.point)
    }

    /// The top of the first collider under `point`, looking down from [`LIFT`] over it. None where that's inside
    /// a collider: what's sunk deeper is the person's to lift, never a snap's.
    #[must_use]
    pub fn below(&self, point: Vec3) -> Option<Vec3> {
        self.hit(Ray { origin: point + Vec3::Y * LIFT, direction: Vec3::NEG_Y })
    }
}

#[cfg(test)]
mod tests {
    use sapphire_colliders::{ColliderEntry, ColliderShape, Group, Rotation};

    use super::*;

    fn slab(name: &str, top: f32) -> ColliderEntry {
        ColliderEntry {
            name: name.to_owned(),
            shape: ColliderShape::Box { half_extents: [4.0, 0.5, 4.0] },
            position: [0.0, top - 0.5, 0.0],
            rotation: Rotation::None,
            groups: vec![Group::World],
        }
    }

    /// The ground under a point is the top of what's below it, leaving out what's left out: never what hangs over
    /// it, nor a blocker, nor one it starts inside.
    #[test]
    fn the_ground_is_the_first_top_below() {
        let blocker = ColliderEntry {
            groups: vec![Group::CameraBlocker, Group::CharacterBlocker],
            ..slab("wall", 2.0)
        };
        let document = ColliderDocument { colliders: vec![slab("floor", 0.25), slab("roof", 5.0), blocker] };
        let ground = Ground::of(&document, &[]).expect("cooks");
        let under = |ground: &Ground, y: f32| ground.below(Vec3::new(1.0, y, 1.0)).map(|hit| hit.y);
        assert!((under(&ground, 5.02).expect("the roof's top, just under") - 5.0).abs() < 1e-5);
        assert!((under(&ground, 3.0).expect("under the roof, through the blocker") - 0.25).abs() < 1e-5);
        assert!((under(&ground, 0.7).expect("the floor") - 0.25).abs() < 1e-5);
        assert!((under(&ground, 0.22).expect("sunk within the lift") - 0.25).abs() < 1e-5);
        assert_eq!(under(&ground, 0.0), None, "sunk deeper: inside the floor, so no hit at 0");
        let roofless = Ground::of(&document, &["roof".to_owned()]).expect("cooks");
        assert!((under(&roofless, 5.5).expect("the floor") - 0.25).abs() < 1e-5);
        let none =
            Ground::of(&document, &["roof".to_owned(), "floor".to_owned()]).expect("cooks the blocker");
        assert_eq!(under(&none, 2.0), None, "a blocker is no ground");
    }
}
