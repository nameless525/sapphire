//! Hurtboxes: bodies in the physics world whose IDs say who owns them.

use sapphire_moves::PathSample;
use sapphire_physics::{
    BodyDesc, BodyId, CollisionGroups, Group, GroupMask, PhysicsWorld, Shape, WorldError,
};

use super::volume::volume_pose;
use crate::ActorTransform;
use crate::actors::ActorId;

/// The most hurtboxes one actor can have: as many as a move set's fighter may list.
pub const MAX_HURTBOXES: u8 = {
    assert!(sapphire_moves::MAX_HURTBOXES <= (INDEX_MASK + 1) as usize);
    sapphire_moves::MAX_HURTBOXES as u8
};

/// Marks a [`BodyId`] as a hurtbox's, above any ID a character's capsule uses.
const HURTBOX_BIT: u32 = 1 << 31;
const INDEX_BITS: u32 = 4;
const INDEX_MASK: u32 = (1 << INDEX_BITS) - 1;
const LARGEST_ACTOR: u32 = (HURTBOX_BIT >> INDEX_BITS) - 1;

/// One hurtbox of one actor, by its place in that actor's list. Its [`BodyId`] holds both, so a
/// query's result names its owner with no table to keep, an actor's hurtboxes are a run of IDs a
/// filter can skip, and they sort by actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HurtboxId {
    actor: ActorId,
    index: u8,
}

impl HurtboxId {
    /// The hurtbox at `index` of `actor`, or `None` if the index is [`MAX_HURTBOXES`] or more, or the
    /// run has spawned 2²⁷ actors, which an ID has no room for.
    #[must_use]
    pub const fn new(actor: ActorId, index: u8) -> Option<Self> {
        if index >= MAX_HURTBOXES || actor.raw() > LARGEST_ACTOR {
            return None;
        }
        Some(Self { actor, index })
    }

    /// Whose it is.
    #[must_use]
    pub const fn actor(self) -> ActorId {
        self.actor
    }

    /// Which of its owner's.
    #[must_use]
    pub const fn index(self) -> u8 {
        self.index
    }

    /// The body it is in the world.
    #[must_use]
    pub const fn body(self) -> BodyId {
        BodyId(HURTBOX_BIT | self.actor.raw() << INDEX_BITS | self.index as u32)
    }

    /// The hurtbox behind a body, or `None` for a body that isn't one.
    #[must_use]
    pub const fn from_body(body: BodyId) -> Option<Self> {
        if body.0 & HURTBOX_BIT == 0 {
            return None;
        }
        let Some(actor) = ActorId::from_raw((body.0 & !HURTBOX_BIT) >> INDEX_BITS) else {
            return None;
        };
        Some(Self { actor, index: (body.0 & INDEX_MASK) as u8 })
    }

    /// The first and last bodies an actor's hurtboxes can be, for
    /// [`QueryFilter::excluding_bodies`](sapphire_physics::QueryFilter::excluding_bodies).
    #[must_use]
    pub const fn bodies_of(actor: ActorId) -> Option<(BodyId, BodyId)> {
        let (Some(first), Some(last)) = (Self::new(actor, 0), Self::new(actor, MAX_HURTBOXES - 1)) else {
            return None;
        };
        Some((first.body(), last.body()))
    }

    /// Makes this hurtbox what it is this tick. `Some` puts it in the world in that shape, where its
    /// root and `local`, its transform in the root's space, say, in [`Group::Hurtbox`], which only hit
    /// queries read. `None` takes it out, so nothing can touch it.
    ///
    /// Call it for each of an actor's hurtboxes in
    /// [`CombatSet::Movement`](crate::CombatSet::Movement), after the actor's own movement, so
    /// [`CombatSet::Hits`](crate::CombatSet::Hits) reads this tick's. [`published_hurtbox`] gives
    /// `shape` from what the actor's moves published. Allocates nothing within the world's body
    /// capacity, which has to allow for these besides the capsules. A shape or pose the world refuses
    /// leaves it as it was.
    ///
    /// [`published_hurtbox`]: super::published_hurtbox
    pub fn set(
        self,
        world: &mut PhysicsWorld,
        shape: Option<Shape>,
        root: ActorTransform,
        local: PathSample,
    ) -> Result<(), WorldError> {
        let id = self.body();
        let Some(shape) = shape else {
            world.remove_body(id);
            return Ok(());
        };
        if !shape.is_valid() {
            return Err(WorldError::InvalidBody(id));
        }
        let pose = volume_pose(root, local);
        let groups = CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE);
        match world.body_shape(id) {
            Some(current) if current == shape => world.set_body_pose(id, pose),
            Some(_) => {
                // Bodies keep their shapes, so a new one is a new body. Moving the old one first is what
                // checks the pose while there's still something to leave as it was.
                world.set_body_pose(id, pose)?;
                world.remove_body(id);
                world.add_body(BodyDesc { id, shape, pose, groups })
            }
            None => world.add_body(BodyDesc { id, shape, pose, groups }),
        }
    }
}

/// Takes all of an actor's hurtboxes out of the world, for a despawn, and returns how many there
/// were.
pub fn remove_hurtboxes(world: &mut PhysicsWorld, actor: ActorId) -> u32 {
    let mut removed = 0;
    for index in 0..MAX_HURTBOXES {
        if let Some(id) = HurtboxId::new(actor, index) {
            removed += u32::from(world.remove_body(id.body()));
        }
    }
    removed
}
