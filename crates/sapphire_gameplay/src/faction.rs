//! Who fights whom (spec §5.22, M3 B1): a number on a fighter, and one rule. Two actors of the same
//! faction are on one side, so neither's swing touches the other, a soft lock passes over it and a brain
//! looks past it. Everyone else is a foe.
//!
//! [`Faction::NONE`], where an actor starts, is on nobody's side, not even its own kind's: a fight that
//! names no factions is everyone against everyone, as every fight was before there were any.
//!
//! It's data on the actor rather than in its move set, because one warrior's document has to serve on
//! either side: whoever spawns it says which ([`crate::ActorSpawn::faction`]), and it holds for the
//! actor's life. Who's friends is a matter of equal numbers, with no table of relations, since nothing
//! yet asks for an ally who isn't the same side.
//!
//! A hit honours it in the world rather than after it: a faction's hurtboxes carry its mark and a swing's
//! casts leave out its own, so a friend standing in front of a foe takes no test and no room in a result
//! list.

use sapphire_physics::GroupMask;

/// A side in a fight. See the [module docs](self).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Faction(u8);

impl Faction {
    /// On nobody's side: a foe to everyone, its own kind included.
    pub const NONE: Self = Self(0);

    /// How many factions a fight can name, each a [`GroupMask::mark`] of its own.
    pub const COUNT: u8 = 16;

    /// The faction with this number, from 1 to [`Self::COUNT`], or `None` for any other.
    #[must_use]
    pub const fn new(number: u8) -> Option<Self> {
        if number >= 1 && number <= Self::COUNT { Some(Self(number)) } else { None }
    }

    /// Its number: 0 for [`Self::NONE`].
    #[must_use]
    pub const fn number(self) -> u8 {
        self.0
    }

    /// Whether a member of this faction fights a member of `other`. It reads the same both ways.
    #[must_use]
    pub const fn is_hostile_to(self, other: Self) -> bool {
        self.0 == 0 || self.0 != other.0
    }

    /// What its members' hurtboxes carry, beside `Group::Hurtbox`, for a swing to pass over. No faction
    /// has none, so nothing passes over it.
    #[must_use]
    pub(crate) const fn mark(self) -> GroupMask {
        if self.0 == 0 { GroupMask::NONE } else { GroupMask::mark(self.0 as u32 - 1) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: Faction = Faction::new(1).unwrap();
    const TWO: Faction = Faction::new(2).unwrap();

    #[test]
    fn a_side_is_a_friend_only_to_itself() {
        assert!(!ONE.is_hostile_to(ONE));
        assert!(ONE.is_hostile_to(TWO) && TWO.is_hostile_to(ONE));
        assert!(ONE.is_hostile_to(Faction::NONE) && Faction::NONE.is_hostile_to(ONE));
        assert!(Faction::NONE.is_hostile_to(Faction::NONE), "no faction is everyone against everyone");
    }

    #[test]
    fn numbers_stop_where_the_marks_do() {
        assert_eq!(Faction::new(0), None);
        assert_eq!(Faction::new(Faction::COUNT + 1), None);
        assert!(u32::from(Faction::COUNT) <= GroupMask::MARKS);
        let last = Faction::new(Faction::COUNT).unwrap();
        assert!(!last.mark().intersects(ONE.mark()));
        assert_eq!(Faction::NONE.mark(), GroupMask::NONE);
    }
}
