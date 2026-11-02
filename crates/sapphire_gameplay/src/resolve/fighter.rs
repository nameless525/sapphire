//! [`Fighter`]: an actor's health, poise and push. See the [module docs](super).

use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_moves::{Attack, FighterDef, Knockback};
use sapphire_time::{SimDuration, SimTime};

/// What a landed hit came to: the `resolve` module's docs say how.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HitOutcome {
    /// It reacts by the hit's weight.
    Flinched,
    /// Its armour took it: no reaction and no push, and its clock runs on.
    Absorbed,
    /// It took the last of its poise: it plays its break, whatever armour says, and the meter fills again.
    PoiseBroken,
    /// It took the last of its health.
    Defeated,
}

/// A push under way, on the target's own clock, so a freeze delays it and costs it nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Slide {
    /// Unit length, across the ground.
    pub(crate) direction: Vec3,
    pub(crate) distance: f32,
    /// How long it lasts on the target's clock. Zero is no slide.
    pub(crate) length: SimDuration,
    pub(crate) elapsed: SimDuration,
    /// How far it has gone, along the direction.
    pub(crate) moved: f32,
}

impl Slide {
    /// Moves it on by `delta`, and returns how far that takes the target. It eases out, as
    /// `distance * t * (2 - t)` of `t` from 0 to 1, so it gets there at any clock speed.
    fn advance(&mut self, delta: SimDuration) -> Vec3 {
        self.elapsed = (self.elapsed + delta).min(self.length);
        let t = self.elapsed.as_subticks() as f32 / self.length.as_subticks() as f32;
        let reached = self.distance * t * (2.0 - t);
        let step = self.direction * (reached - self.moved);
        self.moved = reached;
        if self.elapsed == self.length {
            *self = Self::default();
        }
        step
    }
}

/// What makes an actor something a hit can land on: its numbers now. Every pooled actor has one, and
/// one whose set has no [`FighterDef`] is left empty ([`Self::is_fighter`]). Its numbers are copied at
/// spawn, so replacing a set changes the next spawn's.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct Fighter {
    pub(crate) health: u32,
    pub(crate) max_health: u32,
    pub(crate) poise: u32,
    pub(crate) max_poise: u32,
    pub(crate) poise_refill: SimDuration,
    /// When a poise meter that's been hit is next full, on this actor's clock.
    pub(crate) poise_refills_at: Option<SimTime>,
    pub(crate) defeated: bool,
    pub(crate) slide: Slide,
}

impl Fighter {
    /// A new actor of this kind, whole.
    pub(crate) fn new(def: &FighterDef) -> Self {
        Self {
            health: def.health,
            max_health: def.health,
            poise: def.poise,
            max_poise: def.poise,
            poise_refill: def.refill(),
            ..Self::default()
        }
    }

    /// Whether hits can land on it at all: its set has a [`FighterDef`].
    #[must_use]
    pub const fn is_fighter(&self) -> bool {
        self.max_health > 0
    }

    /// Health left.
    #[must_use]
    pub const fn health(&self) -> u32 {
        self.health
    }

    /// Health at its spawn.
    #[must_use]
    pub const fn max_health(&self) -> u32 {
        self.max_health
    }

    /// The poise meter now.
    #[must_use]
    pub const fn poise(&self) -> u32 {
        self.poise
    }

    /// The poise meter when full. Zero is no meter.
    #[must_use]
    pub const fn max_poise(&self) -> u32 {
        self.max_poise
    }

    /// Whether a hit has taken the last of its health: it starts nothing more, hits nothing more and
    /// can't be hit.
    #[must_use]
    pub const fn is_defeated(&self) -> bool {
        self.defeated
    }

    /// Whether a push is moving it.
    #[must_use]
    pub fn is_sliding(&self) -> bool {
        self.slide.length != SimDuration::ZERO
    }

    pub(super) fn advance_slide(&mut self, delta: SimDuration) -> Vec3 {
        self.slide.advance(delta)
    }

    pub(super) fn start_slide(&mut self, knockback: Knockback, direction: Vec3) {
        if knockback.distance > 0.0 {
            self.slide = Slide {
                direction,
                distance: knockback.distance,
                length: SimDuration::from_ticks(u64::from(knockback.ticks)),
                elapsed: SimDuration::ZERO,
                moved: 0.0,
            };
        }
    }

    pub(super) fn refill_is_due(&self, now: SimTime) -> bool {
        self.poise_refills_at.is_some_and(|at| at <= now)
    }

    pub(super) fn refill_poise(&mut self) {
        self.poise = self.max_poise;
        self.poise_refills_at = None;
    }

    /// Applies a hit's numbers, and says what it came to: steps 1 to 5 of the
    /// [module docs](super#what-a-hit-does). `armour` is the threshold open on its move now.
    pub(super) fn take_hit(&mut self, attack: &Attack, armour: u32, now: SimTime) -> HitOutcome {
        self.health = self.health.saturating_sub(attack.damage);
        if attack.poise > 0 {
            self.poise = self.poise.saturating_sub(attack.poise);
            self.poise_refills_at = (self.poise_refill != SimDuration::ZERO).then(|| now + self.poise_refill);
        }
        if self.health == 0 {
            self.defeated = true;
            HitOutcome::Defeated
        } else if self.max_poise > 0 && self.poise == 0 {
            self.refill_poise();
            HitOutcome::PoiseBroken
        } else if attack.poise < armour {
            HitOutcome::Absorbed
        } else {
            HitOutcome::Flinched
        }
    }
}
