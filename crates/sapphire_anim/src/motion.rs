//! How a character's hair and cloth take a fight, and how much of its IK shows, as a document:
//! `default_character_motion.ron`, compiled in as [`CharacterMotion::shipped`], as the camera's numbers are. A
//! game's own goes through [`CharacterMotion::from_ron`], and `sapphire_stage`'s content hands it to whatever it
//! builds a character with.
//!
//! What reads it is presentation: springs, and the weights the stage's IK (`--ik on`) starts on, so no
//! replay's hash takes it, and a replay plays the same fight on any of it. How the feet and the look solve is
//! the simulation's, since its bodies run on it (M3 S3b): a [`BodyTuning`](crate::BodyTuning) document, which
//! presentation's IK reads too.
//!
//! # A character's own (M5 C4)
//!
//! The shipped document, or a game's, is every character's unless its kind names one of its own: a playable
//! kind's document may name a `<name>.motion.ron`, cooked to `.smotion`
//! ([`sapphire_assets::motion::Motion`], [`CharacterMotion::to_cooked`], [`CharacterMotion::from_cooked`]), which then applies to
//! that kind in full, springs, weights and cloth.
//!
//! # Cloth
//!
//! A `cloth:` section names which of the character's spring chains it wears as sheets ([`ClothSheet`]), in
//! order round or across each: a skirt's panels round the body, closed; a cape's or a coat's tails across,
//! open. How stiff a sheet is, is its two compliances; how many iterations it solves, and out to how far, is
//! the tier's (`SpringQuality::cloth`). A document without the section reads and writes as it always did, and
//! its chains are springs.
//!
//! ```ron
//! cloth: [
//!     (
//!         chains: ["skirt_00", "skirt_01", "skirt_02", "skirt_03"],
//!         closed: true,
//!         lateral_compliance: 0.000002,
//!         bend_compliance: 0.0002,
//!     ),
//! ],
//! ```
//!
//! What the document says by itself is checked when it's read: a sheet of too few chains, a chain named
//! twice in any sheet, a compliance below 0, each refused naming the sheet and the chain. That its chains are
//! the rig's is checked against the rig ([`CharacterMotion::check_rig`]) by whatever puts the two together.

use std::f32::consts::PI;
use std::sync::OnceLock;

use bevy_reflect::TypePath;
use sapphire_assets::motion::{ArchivedMotion, Motion, MotionError, MotionSheet, MotionSprings};
use sapphire_assets::rig::ArchivedCharacterRig;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use sapphire_serialize::cooked::AlignedVec;
use serde::{Deserialize, Serialize};

use crate::springs::{ClothSheet, SpringTuning};

/// The shipped numbers (see [`CharacterMotion::shipped`]).
const SHIPPED: &str = include_str!("../default_character_motion.ron");

/// The numbers a character's secondary motion runs on, how much of its IK shows, and which of its chains it
/// wears as cloth (a `CharacterMotion` document, schema 2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::anim"]
#[serde(deny_unknown_fields)]
pub struct CharacterMotion {
    /// How its chains take a fight's motion.
    pub springs: SpringTuning,
    /// How much of the foot placement shows on the ground, 0 to 1.
    pub feet_weight: f32,
    /// How much of the look shows, 0 to 1.
    pub look_weight: f32,
    /// The sheets of its spring chains it wears as cloth (M5 C4): none, and no section written, for a
    /// character whose chains are all springs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cloth: Vec<ClothSheet>,
}

impl Versioned for CharacterMotion {
    /// 1: M3 S3. 2: M3 S3b, `feet` and `look` gone to the simulation's `BodyTuning`. Version 1 isn't
    /// upgraded, since an upgrade would drop a game's own feet and look with nothing to say so: it's
    /// refused, and its two sections go in a body document. M5 C4's `cloth` is a section a document may
    /// leave out, so it reads and writes a version 2 document without one as it always did.
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
    /// What it says of its cloth, by itself or against a rig, naming the sheet and the chain; or a cooked
    /// motion that doesn't read.
    #[error(transparent)]
    Cooked(#[from] MotionError),
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

    /// Writes it as a document.
    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// Writes it cooked: what a cook makes of a `<name>.motion.ron`. Checks it first.
    pub fn to_cooked(&self) -> Result<AlignedVec, CharacterMotionError> {
        self.check()?;
        Ok(self.cooked().to_cooked()?)
    }

    /// Reads a cooked motion, checking it as a document is checked.
    pub fn from_cooked(motion: &ArchivedMotion) -> Result<Self, CharacterMotionError> {
        let Motion { springs, feet_weight, look_weight, cloth } = motion.to_motion();
        let MotionSprings { root_inertia, anchor_inertia, swing_limit, bend_limit } = springs;
        let motion = Self {
            springs: SpringTuning { root_inertia, anchor_inertia, swing_limit, bend_limit },
            feet_weight,
            look_weight,
            cloth: cloth
                .into_iter()
                .map(|sheet| ClothSheet {
                    chains: sheet.chains,
                    closed: sheet.closed,
                    lateral_compliance: sheet.lateral_compliance,
                    bend_compliance: sheet.bend_compliance,
                })
                .collect(),
        };
        motion.check()?;
        Ok(motion)
    }

    /// Checks that every chain its cloth names is one of `rig`'s spring chains, and only one, naming the
    /// sheet and the chain at fault. A rig with every chain says nothing of how long each is: a sheet links
    /// joints at the same depth, as deep as the shorter of two neighbours goes.
    pub fn check_rig(&self, rig: &ArchivedCharacterRig) -> Result<(), CharacterMotionError> {
        Ok(self.cooked().check_rig(rig)?)
    }

    /// Itself as the cooked format has it.
    fn cooked(&self) -> Motion {
        let SpringTuning { root_inertia, anchor_inertia, swing_limit, bend_limit } = self.springs;
        Motion {
            springs: MotionSprings { root_inertia, anchor_inertia, swing_limit, bend_limit },
            feet_weight: self.feet_weight,
            look_weight: self.look_weight,
            cloth: self
                .cloth
                .iter()
                .map(|sheet| MotionSheet {
                    chains: sheet.chains.clone(),
                    closed: sheet.closed,
                    lateral_compliance: sheet.lateral_compliance,
                    bend_compliance: sheet.bend_compliance,
                })
                .collect(),
        }
    }

    /// Says which field is out of its range, or what's wrong with its cloth.
    fn check(&self) -> Result<(), CharacterMotionError> {
        let Self { springs, feet_weight, look_weight, .. } = self;
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
        Ok(self.cooked().check()?)
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::rig::{CharacterRig, Spring, SpringJoint, SpringTip};

    use super::*;

    /// The shipped document with a skirt of `chains` worn as cloth.
    fn with_cloth(sheets: &str) -> String {
        SHIPPED.replacen("look_weight: 1.0,", &format!("look_weight: 1.0,\n        cloth: [{sheets}],"), 1)
    }

    fn sheet(chains: &[&str], closed: bool) -> String {
        let chains: Vec<String> = chains.iter().map(|chain| format!("{chain:?}")).collect();
        format!(
            "(chains: [{}], closed: {closed}, lateral_compliance: 0.000002, bend_compliance: 0.0002)",
            chains.join(", ")
        )
    }

    #[test]
    fn the_shipped_document_reads_and_keeps_a_strand_near_its_clip() {
        let shipped = CharacterMotion::shipped();
        assert!(shipped.springs.swing_limit < PI && shipped.springs.bend_limit < shipped.springs.swing_limit);
        assert!(shipped.cloth.is_empty(), "nothing shipped wears cloth unless its kind says so");
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

    /// A document with no `cloth:` section reads as it always did and writes back without one, so the
    /// shipped document writes back to its own data, byte for byte; one with a section keeps it through a
    /// write, and through a cook.
    #[test]
    fn a_cloth_section_reads_writes_and_cooks_and_one_left_out_changes_nothing() {
        let shipped = CharacterMotion::shipped();
        let written = shipped.to_ron().expect("writes");
        assert!(!written.contains("cloth"), "{written}");
        assert_eq!(&CharacterMotion::from_ron(&written).expect("reads"), shipped);
        let plain: CharacterMotion = sapphire_serialize::from_ron(SHIPPED).expect("reads");
        assert_eq!(written, sapphire_serialize::to_ron(&plain).expect("writes"));

        let text = with_cloth(&format!(
            "{}, {}",
            sheet(&["skirt_00", "skirt_01", "skirt_02"], true),
            sheet(&["tail_l", "tail_r"], false)
        ));
        let motion = CharacterMotion::from_ron(&text).expect("reads");
        assert_eq!(motion.cloth.len(), 2);
        assert_eq!((motion.cloth[0].closed, motion.cloth[1].chains.len()), (true, 2));
        assert_eq!(motion.springs, shipped.springs, "the rest as it was");
        assert_eq!(CharacterMotion::from_ron(&motion.to_ron().unwrap()).unwrap(), motion);

        let cooked = motion.to_cooked().expect("cooks");
        let read = Motion::read(&cooked).expect("reads cooked");
        assert_eq!(CharacterMotion::from_cooked(read).expect("reads back"), motion);
        assert_eq!(cooked.as_slice(), motion.to_cooked().unwrap().as_slice(), "the same bytes every time");
    }

    /// Each refusal names the sheet, and the chain where one is at fault; a field a sheet doesn't know is
    /// refused by name too.
    #[test]
    fn a_sheet_that_isnt_one_is_refused_naming_the_sheet_and_the_chain() {
        let cases = [
            (sheet(&["a", "b"], true), "cloth[0].chains"),
            (sheet(&["a"], false), "cloth[0].chains"),
            (format!("{}, {}", sheet(&["a", "b"], false), sheet(&["c", "a"], false)), "cloth[1].chains[1]"),
            (sheet(&["a", "b", "a"], true), "cloth[0].chains[2]"),
            (sheet(&["a", "b"], false).replacen("0.0002", "-0.1", 1), "cloth[0].bend_compliance"),
            (sheet(&["a", "b"], false).replacen("closed", "shut", 1), "shut"),
        ];
        for (sheets, named) in cases {
            let refused = CharacterMotion::from_ron(&with_cloth(&sheets)).expect_err(named).to_string();
            assert!(refused.contains(named), "{named}: {refused}");
        }
    }

    /// Against a rig: a chain it lacks, or has twice, is refused, naming the sheet and the chain.
    #[test]
    fn a_chain_the_rig_lacks_is_refused_naming_the_sheet_and_the_chain() {
        let chain = |name: &str, joint: u16| Spring {
            name: name.to_owned(),
            joints: vec![SpringJoint {
                joint,
                hit_radius: 0.02,
                stiffness: 0.5,
                gravity_power: 0.0,
                gravity_dir: [0.0, -1.0, 0.0],
                drag_force: 0.4,
            }],
            tip: SpringTip::Joint,
            collider_groups: Vec::new(),
            center: None,
        };
        let rig = CharacterRig {
            joint_count: 3,
            springs: vec![chain("a", 0), chain("b", 1), chain("c", 2)],
            ..CharacterRig::default()
        };
        let bytes = rig.to_cooked().expect("cooks");
        let rig = CharacterRig::read(&bytes).expect("reads");
        let fits = CharacterMotion::from_ron(&with_cloth(&sheet(&["a", "b", "c"], true))).unwrap();
        assert!(fits.check_rig(rig).is_ok());
        let lacks = CharacterMotion::from_ron(&with_cloth(&sheet(&["a", "skirt_99"], false))).unwrap();
        let refused = lacks.check_rig(rig).expect_err("no such chain").to_string();
        assert!(refused.contains("cloth[0].chains[1]") && refused.contains("skirt_99"), "{refused}");
    }
}
