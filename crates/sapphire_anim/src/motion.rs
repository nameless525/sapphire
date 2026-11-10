//! How a character's hair and cloth take a fight, and how much of its IK shows, as a document:
//! `default_character_motion.ron`, compiled in as [`CharacterMotion::shipped`], as the camera's numbers are. A
//! game's own goes through [`CharacterMotion::from_ron`], and `sapphire_stage`'s content hands it to whatever it
//! builds a character with.
//!
//! What reads it is presentation: springs, and the weights the stage's IK (`--ik on`) starts on, so no
//! replay's hash takes it, and a replay plays the same fight on any of it. How the feet and the look solve is
//! the simulation's, since its bodies run on it (M3 S3b): a [`BodyTuning`](crate::BodyTuning) document, which
//! presentation's IK reads too.

use std::f32::consts::PI;
use std::sync::OnceLock;

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::springs::SpringTuning;

/// The shipped numbers (see [`CharacterMotion::shipped`]).
const SHIPPED: &str = include_str!("../default_character_motion.ron");

/// The numbers a character's secondary motion runs on, and how much of its IK shows (a `CharacterMotion`
/// document, schema 2).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::anim"]
#[serde(deny_unknown_fields)]
pub struct CharacterMotion {
    /// How its chains take a fight's motion.
    pub springs: SpringTuning,
    /// How much of the foot placement shows on the ground, 0 to 1.
    pub feet_weight: f32,
    /// How much of the look shows, 0 to 1.
    pub look_weight: f32,
}

impl Versioned for CharacterMotion {
    /// 1: M3 S3. 2: M3 S3b, `feet` and `look` gone to the simulation's `BodyTuning`. Version 1 isn't
    /// upgraded, since an upgrade would drop a game's own feet and look with nothing to say so: it's
    /// refused, and its two sections go in a body document.
    const VERSION: u32 = 2;
}

/// A character motion document that can't be used.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CharacterMotionError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error("{field}: {value} is outside {low} to {high}")]
    OutOfRange { field: &'static str, value: f32, low: f32, high: f32 },
}

impl CharacterMotion {
    /// Sapphire's numbers (`default_character_motion.ron` beside this crate's manifest), compiled in.
    #[must_use]
    pub fn shipped() -> &'static Self {
        static PARSED: OnceLock<CharacterMotion> = OnceLock::new();
        PARSED.get_or_init(|| Self::from_ron(SHIPPED).expect("this module's tests read the shipped numbers"))
    }

    /// Reads a character motion document and checks it.
    pub fn from_ron(text: &str) -> Result<Self, CharacterMotionError> {
        let motion: Self = sapphire_serialize::from_ron(text)?;
        motion.check()?;
        Ok(motion)
    }

    /// Says which field is out of its range.
    fn check(&self) -> Result<(), CharacterMotionError> {
        let Self { springs, feet_weight, look_weight } = self;
        let ranges = [
            ("springs.root_inertia", springs.root_inertia, 0.0, 1.0),
            ("springs.anchor_inertia", springs.anchor_inertia, 0.0, 1.0),
            ("springs.swing_limit", springs.swing_limit, 0.0, PI),
            ("springs.bend_limit", springs.bend_limit, 0.0, PI),
            ("feet_weight", *feet_weight, 0.0, 1.0),
            ("look_weight", *look_weight, 0.0, 1.0),
        ];
        for (field, value, low, high) in ranges {
            // Written so a NaN fails.
            if !(low..=high).contains(&value) {
                return Err(CharacterMotionError::OutOfRange { field, value, low, high });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_document_reads_and_keeps_a_strand_near_its_clip() {
        let shipped = CharacterMotion::shipped();
        assert!(shipped.springs.swing_limit < PI && shipped.springs.bend_limit < shipped.springs.swing_limit);
    }

    /// A typo would otherwise leave a number at whatever it was with nothing to say so.
    #[test]
    fn a_field_it_doesnt_know_or_a_number_out_of_range_is_refused_by_name() {
        let refused =
            CharacterMotion::from_ron(&SHIPPED.replacen("bend_limit:", "bend:", 1)).expect_err("a typo");
        assert!(refused.to_string().contains("bend"), "{refused}");
        for (from, to, named) in [
            ("root_inertia: 0.25", "root_inertia: 1.25", "springs.root_inertia"),
            ("feet_weight: 1.0", "feet_weight: -1.0", "feet_weight"),
            ("look_weight: 1.0", "look_weight: NaN", "look_weight"),
        ] {
            let refused = CharacterMotion::from_ron(&SHIPPED.replacen(from, to, 1)).expect_err(from);
            assert!(refused.to_string().contains(named), "{from}: {refused}");
        }
    }

    /// A version 1 document held the feet and the look, which are the simulation's now: reading it would
    /// leave a game's own numbers behind, so it's refused.
    #[test]
    fn a_document_from_before_the_feet_and_look_moved_is_refused() {
        let old = SHIPPED.replacen("version: 2", "version: 1", 1);
        let refused = CharacterMotion::from_ron(&old).expect_err("version 1");
        assert!(refused.to_string().contains("older"), "{refused}");
    }
}
