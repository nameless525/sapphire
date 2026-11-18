//! [`WindSample`]: the air's motion a step's [`Force::Wind`](crate::Force::Wind) carries particles by (M6 N2).
//! A plain value of its own, which the caller fills before each step ([`VfxWorld::set_wind`](crate::VfxWorld::set_wind)),
//! so the wind field (M6 Y1) can fill it without this crate taking a dependency on the field's types.

use glam::Vec3;

/// The wind where the effects play, this step: one sample for the whole world, calm until the caller says
/// otherwise. A gust is the caller's to fold in, since it fills this every step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindSample {
    /// Meters per second the air moves, in the world's space.
    pub velocity: Vec3,
}

impl WindSample {
    /// No wind at all: what a world starts with.
    pub const CALM: Self = Self { velocity: Vec3::ZERO };

    /// The sample, or calm where it isn't finite: input from outside never makes a particle's position NaN.
    pub(crate) fn finite(self) -> Self {
        if self.velocity.is_finite() { self } else { Self::CALM }
    }
}
