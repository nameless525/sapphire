//! Collision groups: which things can touch which.
//!
//! Every collider and body belongs to one or more [`Group`]s. A query names the groups it wants
//! to hit ([`GroupMask`]); two bodies touch when each is in a group the other's filter lets
//! through ([`CollisionGroups::interacts_with`]).
//!
//! Testing a group is an integer test, the cheapest thing a query does, so a set that only one
//! kind of query reads, such as hurtboxes (M2 D4), is effectively a broadphase of its own: every
//! other query rejects each member before looking at its geometry.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use std::ops::{BitOr, BitOrAssign};

use serde::{Deserialize, Serialize};

/// A named collision group. Each is a bit of a [`GroupMask`], and documents name them, so a new
/// group takes the next free bit and no existing one ever moves.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[repr(u8)]
pub enum Group {
    /// Scenery: blocks characters and casts alike.
    World = 0,
    /// Characters' capsules.
    Character = 1,
    /// Hurtboxes (M2 D4), which only hit queries read.
    Hurtbox = 2,
    /// Scenery only characters meet, such as an arena's invisible walls.
    CharacterBlocker = 3,
    /// Scenery only the camera meets (M2 G5).
    CameraBlocker = 4,
}

impl Group {
    /// The mask of this group alone.
    pub const fn mask(self) -> GroupMask {
        GroupMask(1 << self as u32)
    }
}

/// A set of [`Group`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GroupMask(u32);

impl GroupMask {
    /// No group: matches nothing.
    pub const NONE: Self = Self(0);
    /// Every group, those added later included.
    pub const ALL: Self = Self(u32::MAX);

    /// The mask with these bits, as cooked data stores it.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Both sets together.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether the sets share a group.
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl From<Group> for GroupMask {
    fn from(group: Group) -> Self {
        group.mask()
    }
}

impl BitOr for GroupMask {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl BitOr<Group> for GroupMask {
    type Output = Self;

    fn bitor(self, other: Group) -> Self {
        self.union(other.mask())
    }
}

impl BitOr for Group {
    type Output = GroupMask;

    fn bitor(self, other: Self) -> GroupMask {
        self.mask().union(other.mask())
    }
}

impl BitOrAssign for GroupMask {
    fn bitor_assign(&mut self, other: Self) {
        *self = self.union(other);
    }
}

/// What a body belongs to and what it may touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CollisionGroups {
    /// The groups this body is in.
    pub memberships: GroupMask,
    /// The groups this body may touch.
    pub filter: GroupMask,
}

impl CollisionGroups {
    pub const fn new(memberships: GroupMask, filter: GroupMask) -> Self {
        Self { memberships, filter }
    }

    /// Whether two bodies touch: each must let the other through, so a body can't be pushed
    /// around by one it has filtered out.
    pub const fn interacts_with(self, other: Self) -> bool {
        self.memberships.intersects(other.filter) && other.memberships.intersects(self.filter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_bits_never_move() {
        // Cooked data stores these bits, so changing one recooks everything silently wrong.
        assert_eq!(Group::World.mask().bits(), 1);
        assert_eq!(Group::Character.mask().bits(), 2);
        assert_eq!(Group::Hurtbox.mask().bits(), 4);
        assert_eq!(Group::CharacterBlocker.mask().bits(), 8);
        assert_eq!(Group::CameraBlocker.mask().bits(), 16);
    }

    #[test]
    fn interaction_needs_both_sides() {
        let character = CollisionGroups::new(
            Group::Character.mask(),
            Group::World | Group::Character | Group::CharacterBlocker,
        );
        let hurtbox = CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE);
        let wall = CollisionGroups::new(Group::World.mask(), GroupMask::ALL);
        assert!(character.interacts_with(character));
        assert!(character.interacts_with(wall));
        assert!(!character.interacts_with(hurtbox));
        assert!(!hurtbox.interacts_with(hurtbox));
        // A body filtering a group out isn't touched by it, even one that would touch it.
        let ghost = CollisionGroups::new(Group::Character.mask(), GroupMask::NONE);
        assert!(!character.interacts_with(ghost));
    }

    #[test]
    fn masks_combine() {
        let mask = Group::World | Group::Hurtbox;
        assert!(mask.intersects(Group::World.mask()));
        assert!(mask.intersects(Group::Hurtbox.mask()));
        assert!(!mask.intersects(Group::Character.mask()));
        assert_eq!((mask | Group::Character).bits(), 0b111);
        assert!(!GroupMask::NONE.intersects(GroupMask::ALL));
    }
}
