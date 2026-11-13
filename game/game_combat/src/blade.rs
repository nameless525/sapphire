//! The weapon in the character's right hand (`--blade on`, M2 O3): the one its playable kind holds
//! ([`sapphire_stage::HeldWeapon`], [`crate::playable`]).
//!
//! It draws as a character with no body, as the enemy's blade does
//! ([`sapphire_stage::scene::Figure::of_item_at`]). Each frame the scene places it by the hand's joint and
//! its kind's socket: for the heroine, the same socket `content/combat/player/player.moves.ron` bakes its
//! `blade` path from, so what's drawn is what the hit volumes sweep.

use sapphire_ecs::prelude::Entity;
use sapphire_math::Mat4;

/// The blade's entity, where the held pose has the hand that holds it, and where it sits in that hand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeldBlade {
    pub(crate) entity: Entity,
    pub(crate) held_hand: Mat4,
    /// Its mesh in the right hand's space.
    pub(crate) socket: Mat4,
}
