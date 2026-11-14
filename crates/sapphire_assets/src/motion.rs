//! A character's motion, cooked (`.smotion`, M5 C4): how its hair and cloth take a fight, how much of its IK
//! shows, and which of its spring chains it wears as cloth. The document is `sapphire_anim`'s
//! `CharacterMotion` (`<name>.motion.ron`), which a playable kind names when it moves by numbers of its own;
//! this is what the cook writes from it and the stage reads back, here with every other cooked format so that
//! [`AssetKind::references`](crate::AssetKind::references) can check its bytes.
//!
//! What it says by itself is checked here ([`Motion::check`]): every number finite, and each cloth sheet a
//! sheet, with each chain named once. That its chains are the rig's is checked against a rig
//! ([`Motion::check_rig`]), which only the kind that names both can do.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

use crate::rig::ArchivedCharacterRig;

/// What a cooked character motion's file name ends with.
pub const MOTION_EXTENSION: &str = "smotion";

/// The most sheets one character wears.
pub const MAX_SHEETS: usize = 8;

/// The most chains one sheet joins.
pub const MAX_SHEET_CHAINS: usize = 32;

/// A character's motion, cooked. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::assets"]
pub struct Motion {
    /// How its chains take a fight's motion.
    pub springs: MotionSprings,
    /// How much of the foot placement shows, 0 to 1.
    pub feet_weight: f32,
    /// How much of the look shows, 0 to 1.
    pub look_weight: f32,
    /// The sheets of its chains it wears as cloth: none for a character whose chains are all springs.
    pub cloth: Vec<MotionSheet>,
}

impl Versioned for Motion {
    /// 1: M5 C4.
    const VERSION: u32 = 1;
}

/// `sapphire_anim`'s `SpringTuning`, cooked.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MotionSprings {
    pub root_inertia: f32,
    pub anchor_inertia: f32,
    pub swing_limit: f32,
    pub bend_limit: f32,
}

/// One sheet of neighbouring spring chains worn as cloth: `sapphire_anim`'s `ClothSheet`, cooked.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MotionSheet {
    /// The rig's spring chains by name, in order round or across the sheet.
    pub chains: Vec<String>,
    /// Whether the last chain is linked back to the first, as a skirt's are round the body.
    pub closed: bool,
    pub lateral_compliance: f32,
    pub bend_compliance: f32,
}

impl MotionSheet {
    /// The fewest chains a sheet joins: two across, three round.
    #[must_use]
    pub const fn fewest_chains(closed: bool) -> usize {
        if closed { 3 } else { 2 }
    }
}

/// A character motion that can't be read or used.
#[derive(Debug, thiserror::Error)]
pub enum MotionError {
    #[error(transparent)]
    Cooked(#[from] CookedError),
    /// What's wrong, and where in the document: `cloth[1].chains[3]`.
    #[error("`{field}`: {problem}")]
    Invalid { field: String, problem: String },
}

fn invalid(field: impl Into<String>, problem: impl Into<String>) -> MotionError {
    MotionError::Invalid { field: field.into(), problem: problem.into() }
}

impl Motion {
    /// Writes it: the same bytes for the same motion, every time.
    pub fn to_cooked(&self) -> Result<AlignedVec, MotionError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and what it says by itself.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedMotion, MotionError> {
        let motion = sapphire_serialize::access::<Self>(bytes)?;
        motion.to_motion().check()?;
        Ok(motion)
    }

    /// Checks what it says by itself, naming the field: every number finite and every compliance 0 or more,
    /// and each sheet at least [`MotionSheet::fewest_chains`] chains, none named twice in any sheet or with
    /// an empty name.
    pub fn check(&self) -> Result<(), MotionError> {
        let MotionSprings { root_inertia, anchor_inertia, swing_limit, bend_limit } = self.springs;
        let numbers = [
            ("springs.root_inertia", root_inertia),
            ("springs.anchor_inertia", anchor_inertia),
            ("springs.swing_limit", swing_limit),
            ("springs.bend_limit", bend_limit),
            ("feet_weight", self.feet_weight),
            ("look_weight", self.look_weight),
        ];
        if let Some((field, value)) = numbers.into_iter().find(|(_, value)| !value.is_finite()) {
            return Err(invalid(field, format!("{value} isn't a number")));
        }
        if self.cloth.len() > MAX_SHEETS {
            return Err(invalid("cloth", format!("{} sheets, past {MAX_SHEETS}", self.cloth.len())));
        }
        let mut named: Vec<(&str, usize)> = Vec::new();
        for (index, sheet) in self.cloth.iter().enumerate() {
            for (name, value) in
                [("lateral_compliance", sheet.lateral_compliance), ("bend_compliance", sheet.bend_compliance)]
            {
                // Written so a NaN fails.
                if !(value >= 0.0 && value.is_finite()) {
                    return Err(invalid(
                        format!("cloth[{index}].{name}"),
                        format!("{value} isn't 0 or more"),
                    ));
                }
            }
            let (count, needs) = (sheet.chains.len(), MotionSheet::fewest_chains(sheet.closed));
            if count < needs {
                let shape = if sheet.closed { "a closed sheet" } else { "an open sheet" };
                return Err(invalid(
                    format!("cloth[{index}].chains"),
                    format!("{shape} joins at least {needs} chains, and this one names {count}"),
                ));
            }
            if count > MAX_SHEET_CHAINS {
                return Err(invalid(
                    format!("cloth[{index}].chains"),
                    format!("{count} chains, past {MAX_SHEET_CHAINS}"),
                ));
            }
            for (at, chain) in sheet.chains.iter().enumerate() {
                let field = format!("cloth[{index}].chains[{at}]");
                if chain.is_empty() {
                    return Err(invalid(field, "a chain's name is empty"));
                }
                if let Some(&(_, first)) = named.iter().find(|(name, _)| name == chain) {
                    return Err(invalid(field, format!("{chain:?} is named twice, first by cloth[{first}]")));
                }
                named.push((chain, index));
            }
        }
        Ok(())
    }

    /// Checks that every chain its sheets name is one, and only one, of `rig`'s spring chains, naming the
    /// sheet and the chain at fault. Chains of different lengths are fine: a sheet links joints at the same
    /// depth down each, as deep as the shorter of two neighbours goes.
    pub fn check_rig(&self, rig: &ArchivedCharacterRig) -> Result<(), MotionError> {
        for (index, sheet) in self.cloth.iter().enumerate() {
            for (at, chain) in sheet.chains.iter().enumerate() {
                let found = rig.springs.iter().filter(|spring| spring.name.as_str() == chain).count();
                let field = format!("cloth[{index}].chains[{at}]");
                match found {
                    0 => return Err(invalid(field, format!("the rig has no spring chain named {chain:?}"))),
                    1 => {}
                    _ => {
                        return Err(invalid(
                            field,
                            format!("the rig has {found} spring chains named {chain:?}"),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

impl ArchivedMotion {
    /// It, out of the archive, which was checked when it was read ([`Motion::read`]).
    #[must_use]
    pub fn to_motion(&self) -> Motion {
        rkyv::deserialize::<Motion, rkyv::rancor::Error>(self).expect("a checked motion deserializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::{CharacterRig, Spring, SpringJoint, SpringTip};

    fn skirt(chains: &[&str], closed: bool) -> MotionSheet {
        MotionSheet {
            chains: chains.iter().map(|&chain| chain.to_owned()).collect(),
            closed,
            lateral_compliance: 1e-6,
            bend_compliance: 1e-4,
        }
    }

    fn motion(cloth: Vec<MotionSheet>) -> Motion {
        Motion {
            springs: MotionSprings {
                root_inertia: 0.25,
                anchor_inertia: 0.4,
                swing_limit: 0.87,
                bend_limit: 0.35,
            },
            feet_weight: 1.0,
            look_weight: 1.0,
            cloth,
        }
    }

    #[test]
    fn it_cooks_and_reads_back_the_same() {
        let motion = motion(vec![skirt(&["a", "b", "c"], true), skirt(&["d", "e"], false)]);
        let cooked = motion.to_cooked().expect("cooks");
        assert_eq!(Motion::read(&cooked).expect("reads").to_motion(), motion);
        assert_eq!(cooked.as_slice(), motion.to_cooked().unwrap().as_slice(), "the same bytes every time");
    }

    /// Each refusal names the sheet, and the chain where one is at fault.
    #[test]
    fn a_sheet_that_isnt_one_is_refused_by_name() {
        let cases: [(Vec<MotionSheet>, &str); 6] = [
            (vec![skirt(&["a", "b"], true)], "`cloth[0].chains`"),
            (vec![skirt(&["a"], false)], "`cloth[0].chains`"),
            (vec![skirt(&["a", "b", "a"], true)], "`cloth[0].chains[2]`"),
            (vec![skirt(&["a", "b"], false), skirt(&["c", "b"], false)], "`cloth[1].chains[1]`"),
            (vec![skirt(&["a", ""], false)], "`cloth[0].chains[1]`"),
            (
                vec![MotionSheet { bend_compliance: f32::NAN, ..skirt(&["a", "b"], false) }],
                "`cloth[0].bend_compliance`",
            ),
        ];
        for (cloth, named) in cases {
            let error = motion(cloth).check().expect_err(named).to_string();
            assert!(error.contains(named), "{named}: {error}");
        }
        let error = motion(vec![skirt(&["a", "b"], false), skirt(&["c", "b"], false)]).check().unwrap_err();
        assert!(error.to_string().contains("first by cloth[0]"), "{error}");
        let mut bad = motion(Vec::new());
        bad.feet_weight = f32::INFINITY;
        assert!(bad.check().unwrap_err().to_string().contains("`feet_weight`"));
    }

    /// A chain the rig lacks, or has twice, is refused, naming the sheet and the chain.
    #[test]
    fn a_chain_the_rig_lacks_is_refused_by_name() {
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
        let springs = vec![chain("a", 0), chain("b", 1), chain("c", 2), chain("c", 3)];
        let rig = CharacterRig { joint_count: 4, springs, ..CharacterRig::default() };
        let bytes = rig.to_cooked().expect("cooks");
        let rig = CharacterRig::read(&bytes).expect("reads");
        assert!(motion(vec![skirt(&["a", "b"], false)]).check_rig(rig).is_ok());
        let error =
            motion(vec![skirt(&["a", "b"], false), skirt(&["x", "a2"], false)]).check_rig(rig).unwrap_err();
        assert!(
            error.to_string().contains("`cloth[1].chains[0]`") && error.to_string().contains("\"x\""),
            "{error}"
        );
        let error = motion(vec![skirt(&["a", "c"], false)]).check_rig(rig).unwrap_err();
        assert!(
            error.to_string().contains("`cloth[0].chains[1]`") && error.to_string().contains("2 spring"),
            "{error}"
        );
    }
}
