//! The join with the move system: what a step's [`MoveOutput`] means to hit queries. The rest of this
//! module takes plain inputs, and this is the one place that knows where they come from.

use sapphire_core::Name;
use sapphire_math::Vec3;
use sapphire_moves::HurtboxState;
use sapphire_physics::{PhysicsWorld, Shape};
use sapphire_time::SimDuration;

use super::HitQuery;
use super::volume::{OwnerStep, VolumeSweep};
use crate::actors::ActorId;
use crate::movement::ActorTransform;
use crate::moves::{ActiveVolume, MoveLibrary, MoveOutput, MoveRunner};

/// A move's shape as a query takes it: the same volume, on the same local +Y. A capsule too short for
/// the physics world to call a capsule is the sphere it nearly is.
#[must_use]
pub fn hit_shape(shape: sapphire_moves::Shape) -> Shape {
    match shape {
        sapphire_moves::Shape::Sphere { radius } => Shape::Sphere { radius },
        sapphire_moves::Shape::Capsule { radius, length } => {
            let capsule = Shape::Capsule { half_height: length * 0.5, radius };
            if capsule.is_valid() { capsule } else { Shape::Sphere { radius } }
        }
        sapphire_moves::Shape::Box { half_extents } => {
            Shape::Box { half_extents: Vec3::from_array(half_extents) }
        }
    }
}

/// What a hurtbox is this tick, by what its actor's moves published: nothing while the actor is
/// invulnerable or a window says `HurtboxState::Off`, a window's shape while it says
/// `HurtboxState::Shape`, and otherwise `own`. The first window published for this name wins. Give
/// the result to [`HurtboxId::set`](super::HurtboxId::set).
///
/// Invulnerability takes the hurtbox out of the world rather than dropping the hit afterwards,
/// because a hit that lands writes its target's record. A dropped one would spend the group's one
/// hit on nothing, where this way a swing still live when the window closes lands.
#[must_use]
pub fn published_hurtbox(name: Name, own: Shape, output: &MoveOutput) -> Option<Shape> {
    if output.windows.invulnerable {
        return None;
    }
    match output.hurtboxes.iter().find(|change| change.name == name).map(|change| change.state) {
        Some(HurtboxState::Off) => None,
        Some(HurtboxState::Shape(shape)) => Some(hit_shape(shape)),
        None => Some(own),
    }
}

/// One actor's moves after a step, for [`HitQuery::sweep_published`].
#[derive(Clone, Copy, Debug)]
pub struct PublishedMoves<'a> {
    pub attacker: ActorId,
    /// How far its clock moved this step (`LocalClock::last_delta`).
    pub delta: SimDuration,
    pub runner: &'a MoveRunner,
    pub output: &'a MoveOutput,
    /// Where it is now, after this tick's movement.
    pub root: ActorTransform,
}

impl HitQuery {
    /// Sweeps everything one actor's moves published this step, in the order published: step 1 of
    /// [a step](Self#a-step) for that actor, with each volume's [`VolumeSweep::rearm`] and
    /// [`VolumeSweep::blocked_by`] as its move authors them.
    pub fn sweep_published(
        &mut self,
        world: &PhysicsWorld,
        library: &MoveLibrary,
        moves: &PublishedMoves<'_>,
    ) {
        for (index, volume) in moves.output.volumes.iter().enumerate() {
            let Some(mv) = library.get(volume.at) else { continue };
            let (Some(hitbox), Some(path)) =
                (mv.hitboxes.get(volume.hitbox as usize), mv.tables.path(volume.path))
            else {
                continue;
            };
            self.sweep(
                world,
                &VolumeSweep {
                    attacker: moves.attacker,
                    instance: volume.serial,
                    volume: index as u16,
                    group: volume.name,
                    rearm: &hitbox.rearm,
                    shape: hit_shape(volume.shape),
                    path,
                    blocked_by: hitbox.blocked_by(),
                    owner: OwnerStep {
                        delta: moves.delta,
                        reading: move_reading(moves.runner, &volume),
                        root_before: moves.output.previous,
                        root_after: moves.root,
                    },
                    from: volume.from,
                    to: volume.to,
                },
            );
        }
    }
}

/// What the actor's clock now reads of a volume's move, or the volume's own end for a move something
/// cut off this step, which has no reading: see [`OwnerStep::reading`].
pub(super) fn move_reading(runner: &MoveRunner, volume: &ActiveVolume) -> SimDuration {
    match runner.running {
        Some(running) if running.serial == volume.serial => runner.cursor.since(running.started_at),
        _ => volume.to,
    }
}
