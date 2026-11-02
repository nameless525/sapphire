//! Where actors are, and the one place a move's root motion reaches them: [`apply_root_motion`] hands
//! it to the actor's character controller, which collides it, or moves an actor with none directly.

use sapphire_ecs::prelude::*;
use sapphire_math::{Vec3, det};

use crate::actors::ActorTable;
use crate::controller::{CharacterController, ControllerDrive};
use crate::moves::{MoveOutput, MoveRunner};

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

/// [`crate::CombatSet::Movement`], just before `controller::move_characters`: applies each actor's
/// root motion, warp included. It runs inside the combat step because hits read positions in the same
/// tick, and what [`MoveOutput::previous`] holds is the tick's start, whatever the controller does.
///
/// A running move takes the stick away from its first tick, startup's still frames included. A move's
/// last stretch of motion arrives on the tick it ends, when it isn't running any more.
pub(crate) fn apply_root_motion(
    table: Res<'_, ActorTable>,
    mut actors: Query<'_, '_, (&mut ActorTransform, &mut MoveOutput, &MoveRunner, &mut CharacterController)>,
) {
    for (_, entity) in table.iter() {
        let Ok((mut transform, mut output, runner, mut controller)) = actors.get_mut(entity) else {
            continue;
        };
        output.previous = *transform;
        transform.yaw += output.root_delta.yaw;
        let delta = output.root_delta.translation;
        if !controller.is_active() {
            transform.position += delta;
        } else if runner.is_running() || delta != Vec3::ZERO {
            controller.drive = ControllerDrive::strike(delta);
        }
    }
}
