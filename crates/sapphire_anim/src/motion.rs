//! How a character moves on top of what its clips and its rig say, as a document: `default_character_motion.ron`,
//! compiled in as [`CharacterMotion::shipped`], as the camera's numbers are. A game's own goes through
//! [`CharacterMotion::from_ron`], and `sapphire_stage`'s content hands it to whatever it builds a character
//! with.
//!
//! What reads it is presentation: springs, and the stage's IK (`--ik on`), so no replay's hash takes it, and
//! a replay plays the same fight on any of it. The simulation's own bodies (`sapphire_body_pose`, M3 #213)
//! run their IK on [`FootSettings::default`] and [`LookSettings::default`], which the shipped `feet` and
//! `look` equal (a test holds that): a game's own document doesn't reach them yet.

use std::f32::consts::PI;
use std::sync::OnceLock;

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::springs::SpringTuning;
use crate::{FootSettings, LookSettings};

/// The shipped numbers (see [`CharacterMotion::shipped`]).
const SHIPPED: &str = include_str!("../default_character_motion.ron");

/// The numbers a character's secondary motion and IK run on (a `CharacterMotion` document, schema 1).
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
    pub feet: FootSettings,
    pub look: LookSettings,
}

impl Versioned for CharacterMotion {
    /// 1: M3 S3.
    const VERSION: u32 = 1;
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

    /// Says which field is out of its range. A time of 0 would divide by zero, so times stay above it.
    fn check(&self) -> Result<(), CharacterMotionError> {
        let Self { springs, feet_weight, look_weight, feet, look } = self;
        const LONG: f32 = 1000.0;
        const SOON: f32 = 1e-4;
        let ranges = [
            ("springs.root_inertia", springs.root_inertia, 0.0, 1.0),
            ("springs.anchor_inertia", springs.anchor_inertia, 0.0, 1.0),
            ("springs.swing_limit", springs.swing_limit, 0.0, PI),
            ("springs.bend_limit", springs.bend_limit, 0.0, PI),
            ("feet_weight", *feet_weight, 0.0, 1.0),
            ("look_weight", *look_weight, 0.0, 1.0),
            ("feet.reach_up", feet.reach_up, 0.0, LONG),
            ("feet.reach_down", feet.reach_down, 0.0, LONG),
            ("feet.min_normal_y", feet.min_normal_y, 0.0, 1.0),
            ("feet.max_tilt", feet.max_tilt, 0.0, PI / 2.0),
            ("feet.plant_height", feet.plant_height, SOON, LONG),
            ("feet.max_drop", feet.max_drop, 0.0, LONG),
            ("feet.max_raise", feet.max_raise, 0.0, LONG),
            ("feet.ground_half_life", feet.ground_half_life, SOON, LONG),
            ("feet.hips_half_life", feet.hips_half_life, SOON, LONG),
            ("feet.fade", feet.fade, SOON, LONG),
            ("feet.pop_speed", feet.pop_speed, 0.0, LONG),
            ("feet.soften", feet.soften, SOON, 1.0),
            ("look.yaw_limit", look.yaw_limit, 0.0, PI),
            ("look.up_limit", look.up_limit, 0.0, PI / 2.0),
            ("look.down_limit", look.down_limit, 0.0, PI / 2.0),
            ("look.hold", look.hold, 0.0, PI),
            ("look.rate", look.rate, SOON, LONG),
            ("look.half_life", look.half_life, SOON, LONG),
            ("look.eye_rate", look.eye_rate, SOON, LONG),
            ("look.eye_half_life", look.eye_half_life, SOON, LONG),
            ("look.fade", look.fade, SOON, LONG),
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
        assert!(shipped.look.eye_rate > shipped.look.rate, "the eyes are much quicker than the head");
    }

    /// The simulation's bodies run on the code's settings, and a fight's presentation on these: until the
    /// simulation takes the document, they can't differ.
    #[test]
    fn the_shipped_feet_and_look_are_what_the_simulations_bodies_run_on() {
        let shipped = CharacterMotion::shipped();
        assert_eq!(shipped.feet, FootSettings::default());
        assert_eq!(shipped.look, LookSettings::default());
    }

    /// A typo would otherwise leave a number at whatever it was with nothing to say so.
    #[test]
    fn a_field_it_doesnt_know_or_a_number_out_of_range_is_refused_by_name() {
        let refused =
            CharacterMotion::from_ron(&SHIPPED.replacen("soften:", "soft:", 1)).expect_err("a typo");
        assert!(refused.to_string().contains("soft"), "{refused}");
        for (from, to, named) in [
            ("root_inertia: 0.25", "root_inertia: 1.25", "springs.root_inertia"),
            ("feet_weight: 1.0", "feet_weight: -1.0", "feet_weight"),
            ("hips_half_life: 0.06", "hips_half_life: 0.0", "feet.hips_half_life"),
            ("eye_rate: 14.0", "eye_rate: NaN", "look.eye_rate"),
        ] {
            let refused = CharacterMotion::from_ron(&SHIPPED.replacen(from, to, 1)).expect_err(from);
            assert!(refused.to_string().contains(named), "{from}: {refused}");
        }
    }
}
