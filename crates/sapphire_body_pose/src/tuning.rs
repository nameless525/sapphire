//! How a body's feet and look solve, as a document: `default_body.ron`, compiled in as
//! [`BodyTuning::shipped`]. A game's own goes through [`BodyTuning::from_ron`], named `body` in its
//! `content/game.documents.ron`.
//!
//! These are the simulation's numbers (M3 S3b): a simulation holds one table, binds its bodies on it, and its
//! `tables_hash` takes [`BodyTuning::write_canonical`], so a replay recorded on other numbers refuses to play
//! rather than drifting. Presentation's own IK (a stage's `--ik on`, a show's graph) runs on the same table,
//! so a character looks the same with or without a simulation behind it. What only presentation reads, its
//! hair and cloth and how much of each IK it shows, is `sapphire_anim`'s `CharacterMotion`, which no hash
//! takes.

use std::f32::consts::PI;
use std::sync::OnceLock;

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::{FootSettings, LookSettings};

/// The shipped numbers (see [`BodyTuning::shipped`]).
const SHIPPED: &str = include_str!("../default_body.ron");

/// How many numbers a [`BodyTuning`] holds.
const NUMBERS: usize = 21;

/// The numbers a body's feet and look solve by (a `BodyTuning` document, schema 1).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::body_pose"]
#[serde(deny_unknown_fields)]
pub struct BodyTuning {
    pub feet: FootSettings,
    pub look: LookSettings,
}

impl Versioned for BodyTuning {
    /// 1: M3 S3b, from `CharacterMotion` version 1's `feet` and `look`.
    const VERSION: u32 = 1;
}

/// A body document that can't be used.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BodyTuningError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error("{field}: {value} is outside {low} to {high}")]
    OutOfRange { field: &'static str, value: f32, low: f32, high: f32 },
}

impl BodyTuning {
    /// Sapphire's numbers (`default_body.ron` beside this crate's manifest), compiled in, because a replay
    /// needs no assets.
    #[must_use]
    pub fn shipped() -> &'static Self {
        static PARSED: OnceLock<BodyTuning> = OnceLock::new();
        PARSED.get_or_init(|| Self::from_ron(SHIPPED).expect("this module's tests read the shipped numbers"))
    }

    /// Reads a body document and checks it.
    pub fn from_ron(text: &str) -> Result<Self, BodyTuningError> {
        let tuning: Self = sapphire_serialize::from_ron(text)?;
        tuning.check()?;
        Ok(tuning)
    }

    /// Says which field is out of its range. A time of 0 would divide by zero, so times stay above it.
    fn check(&self) -> Result<(), BodyTuningError> {
        for (field, value, low, high) in self.numbers() {
            // Written so a NaN fails.
            if !(low..=high).contains(&value) {
                return Err(BodyTuningError::OutOfRange { field, value, low, high });
            }
        }
        Ok(())
    }

    /// Every number, by name, with the range it has to stay in: what [`Self::check`] holds and
    /// [`Self::write_canonical`] writes, in the document's order. Taken apart whole, so a new field doesn't
    /// build until it's here.
    fn numbers(&self) -> [(&'static str, f32, f32, f32); NUMBERS] {
        const LONG: f32 = 1000.0;
        const SOON: f32 = 1e-4;
        let Self {
            feet:
                FootSettings {
                    reach_up,
                    reach_down,
                    min_normal_y,
                    max_tilt,
                    plant_height,
                    max_drop,
                    max_raise,
                    ground_half_life,
                    hips_half_life,
                    fade: feet_fade,
                    pop_speed,
                    soften,
                },
            look:
                LookSettings {
                    yaw_limit,
                    up_limit,
                    down_limit,
                    hold,
                    rate,
                    half_life,
                    eye_rate,
                    eye_half_life,
                    fade: look_fade,
                },
        } = *self;
        [
            ("feet.reach_up", reach_up, 0.0, LONG),
            ("feet.reach_down", reach_down, 0.0, LONG),
            ("feet.min_normal_y", min_normal_y, 0.0, 1.0),
            ("feet.max_tilt", max_tilt, 0.0, PI / 2.0),
            ("feet.plant_height", plant_height, SOON, LONG),
            ("feet.max_drop", max_drop, 0.0, LONG),
            ("feet.max_raise", max_raise, 0.0, LONG),
            ("feet.ground_half_life", ground_half_life, SOON, LONG),
            ("feet.hips_half_life", hips_half_life, SOON, LONG),
            ("feet.fade", feet_fade, SOON, LONG),
            ("feet.pop_speed", pop_speed, 0.0, LONG),
            ("feet.soften", soften, SOON, 1.0),
            ("look.yaw_limit", yaw_limit, 0.0, PI),
            ("look.up_limit", up_limit, 0.0, PI / 2.0),
            ("look.down_limit", down_limit, 0.0, PI / 2.0),
            ("look.hold", hold, 0.0, PI),
            ("look.rate", rate, SOON, LONG),
            ("look.half_life", half_life, SOON, LONG),
            ("look.eye_rate", eye_rate, SOON, LONG),
            ("look.eye_half_life", eye_half_life, SOON, LONG),
            ("look.fade", look_fade, SOON, LONG),
        ]
    }

    /// Its canonical bytes, for a replay's `tables_hash`: a tag, then every number by its bits, little-endian,
    /// in the document's order, the same on every architecture. [`Self::from_ron`] refuses a NaN, so no NaN
    /// needs making canonical.
    pub fn write_canonical(&self, sink: &mut impl FnMut(&[u8])) {
        sink(b"sapphire.body-tuning.v1");
        for (_, value, _, _) in self.numbers() {
            sink(&value.to_bits().to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(tuning: &BodyTuning) -> Vec<u8> {
        let mut bytes = Vec::new();
        tuning.write_canonical(&mut |chunk| bytes.extend_from_slice(chunk));
        bytes
    }

    /// `tuning` with its numbers replaced by `values`, in [`BodyTuning::numbers`]' order.
    fn with_numbers(tuning: &BodyTuning, values: [f32; NUMBERS]) -> BodyTuning {
        let mut changed = *tuning;
        let BodyTuning { feet, look } = &mut changed;
        let slots = [
            &mut feet.reach_up,
            &mut feet.reach_down,
            &mut feet.min_normal_y,
            &mut feet.max_tilt,
            &mut feet.plant_height,
            &mut feet.max_drop,
            &mut feet.max_raise,
            &mut feet.ground_half_life,
            &mut feet.hips_half_life,
            &mut feet.fade,
            &mut feet.pop_speed,
            &mut feet.soften,
            &mut look.yaw_limit,
            &mut look.up_limit,
            &mut look.down_limit,
            &mut look.hold,
            &mut look.rate,
            &mut look.half_life,
            &mut look.eye_rate,
            &mut look.eye_half_life,
            &mut look.fade,
        ];
        for (slot, value) in slots.into_iter().zip(values) {
            *slot = value;
        }
        changed
    }

    #[test]
    fn the_shipped_document_reads_and_its_eyes_are_quicker_than_its_head() {
        let shipped = BodyTuning::shipped();
        assert!(shipped.look.eye_rate > shipped.look.rate);
        assert!(shipped.feet.reach_up > 0.0 && shipped.feet.reach_down > 0.0, "the feet look for ground");
    }

    /// A typo would otherwise leave a number at whatever it was with nothing to say so.
    #[test]
    fn a_field_it_doesnt_know_or_a_number_out_of_range_is_refused_by_name() {
        let refused = BodyTuning::from_ron(&SHIPPED.replacen("soften:", "soft:", 1)).expect_err("a typo");
        assert!(refused.to_string().contains("soft"), "{refused}");
        for (from, to, named) in [
            ("hips_half_life: 0.06", "hips_half_life: 0.0", "feet.hips_half_life"),
            ("min_normal_y: 0.5", "min_normal_y: 1.5", "feet.min_normal_y"),
            ("eye_rate: 14.0", "eye_rate: NaN", "look.eye_rate"),
        ] {
            let refused = BodyTuning::from_ron(&SHIPPED.replacen(from, to, 1)).expect_err(from);
            assert!(refused.to_string().contains(named), "{from}: {refused}");
        }
    }

    /// Whichever number moved, by one unit in the last place, the bytes say so: this is what stands between
    /// other numbers and a replay.
    #[test]
    fn the_canonical_bytes_take_every_number() {
        let shipped = BodyTuning::shipped();
        let values = shipped.numbers().map(|(_, value, _, _)| value);
        assert_eq!(with_numbers(shipped, values), *shipped, "this test's order is the document's");
        let base = bytes(shipped);
        for (index, (name, value, _, _)) in shipped.numbers().into_iter().enumerate() {
            let mut nudged = values;
            nudged[index] = f32::from_bits(value.to_bits() + 1);
            assert_ne!(bytes(&with_numbers(shipped, nudged)), base, "{name}");
        }
    }
}
