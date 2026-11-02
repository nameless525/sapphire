//! Where actors are, and the one place a move's root motion reaches them.
//!
//! **M2 G1 takes over here:** [`apply_root_motion`] is everything between what a move wants and where
//! an actor stands. The controller reads the same [`crate::MoveOutput::root_delta`] and moves a
//! capsule instead.

use sapphire_ecs::prelude::*;
use sapphire_math::{Vec3, det};

use crate::actors::ActorTable;
use crate::moves::MoveOutput;

/// Where an actor is, as the fixed step left it. Scalar on purpose: `Vec3` adds the same way on every
/// architecture, which `Vec3A` doesn't promise (spec §5.4).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct ActorTransform {
    pub position: Vec3,
    /// Radians about +Y, unwrapped; 0 faces +Z.
    pub yaw: f32,
}

impl ActorTransform {
    /// A point in this actor's root space, in the world: what a hit path's sample means, so a path
    /// rides wherever warp and root motion took the root.
    #[must_use]
    pub fn point(self, local: Vec3) -> Vec3 {
        self.position + det::rotate_y(local, self.yaw)
    }
}

/// What a step did to an actor's root, in world space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RootDelta {
    pub translation: Vec3,
    pub yaw: f32,
}

/// [`crate::CombatSet::Movement`]: applies each actor's root motion, warp included. It runs inside
/// the combat step because hits read positions in the same tick.
pub(crate) fn apply_root_motion(
    table: Res<'_, ActorTable>,
    mut actors: Query<'_, '_, (&mut ActorTransform, &mut MoveOutput)>,
) {
    for (_, entity) in table.iter() {
        let Ok((mut transform, mut output)) = actors.get_mut(entity) else { continue };
        output.previous = *transform;
        transform.position += output.root_delta.translation;
        transform.yaw += output.root_delta.yaw;
    }
}
