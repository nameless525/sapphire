//! One playable kind (M5 C8): `<name>.playable.ron`, cooked to `.splayable`, as a kind of enemy is
//! ([`crate::EnemyDocument`]).
//!
//! ```ron
//! (
//!     type: "sapphire::moves::PlayableDocument",
//!     version: 1,
//!     data: (
//!         // The key of its name in the game's words: what the party's slots on the HUD call it.
//!         name: "character-name-wanderer",
//!         // Its `.moves.ron`, by the ID in its sidecar: the simulation's half.
//!         moves: "c1d2e3f4-...",
//!         look: (
//!             mesh: "...", rig: "...", graph: "...", seed: 1,
//!             held: Some((mesh: "...", joint: "J_Bip_R_Hand", offset: (0.0, 0.0, 0.4), rotation: (0.0, 0.0, 0.0, 1.0))),
//!             body_contacts: ["J_Bip_C_Hips", "J_Bip_L_Foot", "J_Bip_R_Foot"],
//!             expression: Some((preset: "relaxed", weight: 0.35)),
//!         ),
//!     ),
//! )
//! ```
//!
//! **Two halves, as an enemy's.** [`PlayableDocument::moves`] names the simulation's half, which stays
//! compiled in as every kind's does (m5-plan §3, "The simulation's data"): a scenario names the set its
//! members play and the kind each draws as, and checks that the two agree. The cooked kind holds only the
//! look, which the stage reads by the kind's ID, so nothing cooked here reaches a step or a checksum.
//!
//! **What it points at rather than copies:**
//!
//! - its **springs** are its rig's: the model's own spring chains and colliders (a VRM's
//!   `VRMC_springBone`), cooked into the rig sub-asset [`PlayableLookDocument::rig`] names. How stiff
//!   every character's springs are, and how much of its IK shows, is the game's one motion document
//!   (`CharacterMotion`), which every kind shares.
//! - **cloth** (M5 C4) is chains of those springs worn as a sheet, named by the kind's own joints, so it is
//!   the kind's: its `cloth:` section goes in [`PlayableLookDocument`], beside `rig`, naming the rig's
//!   chains, and nothing else here changes.
//!
//! **What's checked where.** [`PlayableDocument::check`] holds what the document says by itself: a name, a
//! rig that is its mesh's own sub-asset (a VRM's rig is derived from the model's ID), a weapon's joint and
//! numbers. A cook holds what it names to the right kinds of asset and its expression to a preset the rigs
//! know. That the weapon's joint is one of the mesh's skeleton is the stage's to check when it loads the
//! kind, since only the cooked skeleton says.

use bevy_reflect::{Reflect, TypePath};
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{self, AlignedVec};
use sapphire_serialize::{AssetId, CookedError, DocumentError};
use serde::{Deserialize, Serialize};

/// What a cooked playable kind's file name ends with.
pub const PLAYABLE_EXTENSION: &str = "splayable";

/// The sub-asset of a character model that holds its rig, as a cook names it: a kind's rig is its mesh's.
pub const RIG_SUB_ASSET: &str = "rig";

/// The prefix of every playable kind's name: the HUD reads a character's name under it (`hud.ui.ron`).
pub const NAME_PREFIX: &str = "character-name-";

/// Most joints a kind's body binds by.
pub const MAX_BODY_CONTACTS: usize = 32;

/// A playable kind's document, `.playable.ron`, which compiles to a [`Playable`].
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[type_path = "sapphire::moves"]
#[serde(deny_unknown_fields)]
pub struct PlayableDocument {
    /// The key of its name in the game's words, a string table's message under [`NAME_PREFIX`]: what the
    /// party's slots call it.
    pub name: String,
    /// Its set of moves: the `.moves.ron`'s ID, which a scenario holding the kind plays. The cooked kind
    /// doesn't hold it: the simulation's half is compiled in, and a cook only checks it's a set of moves.
    pub moves: AssetId,
    pub look: PlayableLookDocument,
}

impl Versioned for PlayableDocument {
    /// 1: M5 C8.
    const VERSION: u32 = 1;
}

/// What draws one.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayableLookDocument {
    /// A skinned character model, whose skin names its skeleton.
    pub mesh: AssetId,
    /// Its rig: humanoid bones, constraints, springs and expressions. Always the mesh's own
    /// ([`RIG_SUB_ASSET`]).
    pub rig: AssetId,
    /// Its animation graph: the clips it plays.
    pub graph: AssetId,
    /// What sets its graph's draws (its blinks) apart from another kind's. A golden pins each kind's.
    pub seed: u32,
    /// The weapon it holds that isn't skinned in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<HeldWeaponDocument>,
    /// The joints where its body meets what it touches, by its skeleton's names: what a simulation binds
    /// a body to it by, where a fight binds one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_contacts: Vec<String>,
    /// The expression its face rests on, if any: a preset its rig knows, by name, and how much of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<RestExpression>,
}

/// A weapon a kind holds, drawn on its own and placed at a joint by a socket.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeldWeaponDocument {
    /// Its mesh.
    pub mesh: AssetId,
    /// The joint that holds it, by the skeleton's name.
    pub joint: String,
    /// The socket: its mesh's place in the joint's space, a translation in metres and a rotation as a
    /// quaternion (x, y, z, w). A moves document's paths that follow that joint use the same numbers, so
    /// what's drawn is what the hit volumes sweep.
    pub offset: [f32; 3],
    pub rotation: [f32; 4],
}

/// The expression a face rests on.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestExpression {
    /// A VRM preset's name, which its rig binds.
    pub preset: String,
    /// 0 to 1.
    pub weight: f32,
}

/// A playable kind that can't be read or cooked.
#[derive(Debug, thiserror::Error)]
pub enum PlayableError {
    /// Boxed: RON's errors are large.
    #[error(transparent)]
    Document(Box<DocumentError>),
    #[error("`{field}`: {problem}")]
    Invalid { field: String, problem: String },
    #[error(transparent)]
    Cooked(#[from] CookedError),
    /// Cooked data that isn't what any document compiles to.
    #[error("cooked playable kind is damaged: {0}")]
    Damaged(String),
}

fn invalid(field: impl Into<String>, problem: impl Into<String>) -> PlayableError {
    PlayableError::Invalid { field: field.into(), problem: problem.into() }
}

impl PlayableDocument {
    /// Reads a document and checks what it can say by itself.
    pub fn parse(text: &str) -> Result<Self, PlayableError> {
        let document: Self =
            sapphire_serialize::from_ron(text).map_err(|error| PlayableError::Document(Box::new(error)))?;
        document.check()?;
        Ok(document)
    }

    /// Checks everything that needs nothing outside the document, naming the field.
    pub fn check(&self) -> Result<(), PlayableError> {
        if self.name.strip_prefix(NAME_PREFIX).is_none_or(str::is_empty) {
            return Err(invalid("name", format!("a playable kind's name's key is `{NAME_PREFIX}<kind>`")));
        }
        self.compile().check()
    }

    /// The kind it compiles to.
    pub fn compile(&self) -> Playable {
        let look = &self.look;
        Playable {
            name: self.name.clone(),
            mesh: look.mesh,
            rig: look.rig,
            graph: look.graph,
            seed: look.seed,
            held: look.held.as_ref().map(|held| HeldWeapon {
                mesh: held.mesh,
                joint: held.joint.clone(),
                offset: held.offset,
                rotation: held.rotation,
            }),
            body_contacts: look.body_contacts.clone(),
            expression: look
                .expression
                .as_ref()
                .map(|rest| RestPose { preset: rest.preset.clone(), weight: rest.weight }),
        }
    }
}

/// A weapon a kind holds, cooked. See [`HeldWeaponDocument`].
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct HeldWeapon {
    pub mesh: AssetId,
    pub joint: String,
    pub offset: [f32; 3],
    pub rotation: [f32; 4],
}

/// The expression a face rests on, cooked. See [`RestExpression`].
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RestPose {
    pub preset: String,
    pub weight: f32,
}

/// One playable kind, cooked (`.splayable`): what the stage draws it by. See [`PlayableLookDocument`] for
/// each.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::moves"]
pub struct Playable {
    /// Its name's key in the game's words.
    pub name: String,
    pub mesh: AssetId,
    pub rig: AssetId,
    pub graph: AssetId,
    pub seed: u32,
    pub held: Option<HeldWeapon>,
    pub body_contacts: Vec<String>,
    pub expression: Option<RestPose>,
}

impl Versioned for Playable {
    /// 1: M5 C8.
    const VERSION: u32 = 1;
}

impl Playable {
    /// Writes it: the same bytes for the same kind, every time.
    pub fn to_cooked(&self) -> Result<AlignedVec, PlayableError> {
        Ok(cooked::to_cooked(self)?)
    }

    /// Reads a cooked kind, whose bytes must start 16-byte aligned, and checks what it relies on of itself:
    /// bytes from anywhere are refused as a document would be.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, PlayableError> {
        let archived = cooked::access::<Self>(bytes)?;
        let kind = rkyv::deserialize::<Self, rkyv::rancor::Error>(archived).map_err(CookedError::from)?;
        kind.check().map_err(|error| PlayableError::Damaged(error.to_string()))?;
        Ok(kind)
    }

    /// The assets it names, which loading it doesn't load but which must ship with it: its mesh, rig and
    /// graph, and its weapon's mesh.
    pub fn named(&self) -> impl Iterator<Item = AssetId> + '_ {
        [self.mesh, self.rig, self.graph].into_iter().chain(self.held.as_ref().map(|held| held.mesh))
    }

    /// What it says by itself, naming the document's field.
    fn check(&self) -> Result<(), PlayableError> {
        if self.name.is_empty() {
            return Err(invalid("name", "a playable kind needs its name's key"));
        }
        if self.rig != self.mesh.sub_asset(RIG_SUB_ASSET) {
            let rig = self.mesh.sub_asset(RIG_SUB_ASSET);
            return Err(invalid("look.rig", format!("{} isn't its mesh's rig, which is {rig}", self.rig)));
        }
        if let Some(held) = &self.held {
            if held.joint.is_empty() {
                return Err(invalid("look.held.joint", "a name is empty"));
            }
            if !held.offset.iter().chain(&held.rotation).all(|value| value.is_finite()) {
                return Err(invalid("look.held", "its socket's numbers aren't all finite"));
            }
            let length = held.rotation.iter().map(|value| value * value).sum::<f32>();
            if !(0.98..=1.02).contains(&length) {
                return Err(invalid("look.held.rotation", "a rotation is a unit quaternion"));
            }
        }
        if self.body_contacts.len() > MAX_BODY_CONTACTS {
            let count = self.body_contacts.len();
            return Err(invalid("look.body_contacts", format!("{count} joints, past {MAX_BODY_CONTACTS}")));
        }
        if self.body_contacts.iter().any(String::is_empty) {
            return Err(invalid("look.body_contacts", "a name is empty"));
        }
        if let Some(rest) = &self.expression
            && (rest.preset.is_empty() || !(0.0..=1.0).contains(&rest.weight))
        {
            return Err(invalid("look.expression", "a preset's name and a weight from 0 to 1"));
        }
        Ok(())
    }
}

impl ArchivedPlayable {
    /// It, out of the archive: what a stage keeps while it draws one. The archive was checked when it was
    /// read ([`Playable::from_cooked`]).
    #[must_use]
    pub fn to_playable(&self) -> Playable {
        rkyv::deserialize::<Playable, rkyv::rancor::Error>(self)
            .expect("a checked kind deserializes: nothing in it can fail")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIND: &str = include_str!("../test_content/slice.playable.ron");

    fn document() -> PlayableDocument {
        PlayableDocument::parse(KIND).expect("parses")
    }

    #[test]
    fn it_reads_writes_and_cooks_the_same_bytes() {
        let kind = document().compile();
        let cooked = kind.to_cooked().expect("cooks");
        assert_eq!(Playable::from_cooked(&cooked).expect("reads"), kind);
        assert_eq!(cooked.as_slice(), document().compile().to_cooked().unwrap().as_slice());
        let text = sapphire_serialize::to_ron(&document()).expect("writes");
        let again: PlayableDocument = sapphire_serialize::from_ron(&text).expect("reads what it wrote");
        assert_eq!(again, document());
        assert_eq!(kind.named().count(), 4, "mesh, rig, graph and the weapon's mesh");
    }

    #[test]
    fn a_field_nobody_knows_is_refused() {
        let text = KIND.replacen("seed: 3,", "seed: 3, springs: 2,", 1);
        assert_ne!(text, KIND);
        let error = PlayableDocument::parse(&text).expect_err("no such field").to_string();
        assert!(error.contains("springs"), "{error}");
        let text = KIND.replacen("joint: \"hand.r\",", "joint: \"hand.r\", scale: 2.0,", 1);
        assert_ne!(text, KIND);
        assert!(PlayableDocument::parse(&text).is_err(), "nor in the weapon");
    }

    /// A VRM's rig is its model's sub-asset, so a rig that isn't the mesh's draws one model on another's
    /// bones.
    #[test]
    fn a_rig_that_isnt_the_meshs_is_refused() {
        let mut document = document();
        document.look.rig = AssetId::from_bytes([9; 16]);
        let error = document.check().expect_err("another's rig").to_string();
        assert!(error.contains("`look.rig`"), "{error}");
    }

    /// One bad document per check: the message names the field.
    #[test]
    fn a_bad_document_names_the_field() {
        type Spoil = fn(&mut PlayableDocument);
        let cases: &[(&str, Spoil)] = &[
            ("`name`", |d| d.name = "heroine".to_owned()),
            ("`name`", |d| d.name = NAME_PREFIX.to_owned()),
            ("`look.held.joint`", |d| d.look.held.as_mut().unwrap().joint.clear()),
            ("`look.held`", |d| d.look.held.as_mut().unwrap().offset[1] = f32::NAN),
            ("`look.held.rotation`", |d| d.look.held.as_mut().unwrap().rotation = [0.0; 4]),
            ("`look.body_contacts`", |d| d.look.body_contacts.push(String::new())),
            ("`look.expression`", |d| d.look.expression.as_mut().unwrap().weight = 2.0),
        ];
        for (field, spoil) in cases {
            let mut document = document();
            spoil(&mut document);
            let error = document.check().expect_err(field).to_string();
            assert!(error.contains(field), "{field}: {error}");
        }
    }

    /// Bytes from anywhere: what no document makes is refused.
    #[test]
    fn a_damaged_kind_is_refused() {
        let mut kind = document().compile();
        kind.rig = kind.mesh;
        let cooked = kind.to_cooked().expect("cooks");
        assert!(matches!(Playable::from_cooked(&cooked), Err(PlayableError::Damaged(_))));
    }
}
