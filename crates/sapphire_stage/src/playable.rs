//! Playable kinds (M5 C7, C8): what draws a character a player plays, found by its kind, as an enemy's look
//! is found by its kind of enemy. The stage names no character of its own: a kind's document
//! (`.playable.ron`, cooked to a [`Playable`]) says what draws it, [`Content::playable_look`] reads it by the
//! kind's ID, and everything after that (its held pose, its clips, its graph and the driver over it, its IK,
//! its clearance, the body the simulation binds) looks the kind up by its ID.
//!
//! - **Where a look comes from** is the kind's cooked document ([`PlayableLook::of`]). A test may make one
//!   by hand.
//! - **The scene's character** is one entity, which draws as the kind [`Content::character_kind`] names:
//!   the one [`crate::lookdev::hold`] last stood it as.
//! - **A kind's graph** is keyed by the kind ([`GraphKey::playable`]), so two kinds never share one by
//!   accident.
//!
//! [`Content`]: crate::content::Content
//! [`Content::playable_look`]: crate::content::Content::playable_look
//! [`Content::character_kind`]: crate::content::Content::character_kind
//! [`GraphKey::playable`]: crate::content::GraphKey::playable

use sapphire_assets::AssetId;
use sapphire_assets::rig::ExpressionPreset;
use sapphire_assets::skeleton::{ArchivedSkeleton, SKELETON_SUB_ASSET};
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_moves::Playable;

/// What draws a playable kind, by its assets' IDs (rule R5).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayableLook {
    /// The kind, which everything that stands it looks it up by.
    pub kind: AssetId,
    /// Its skinned mesh, whose skin names its skeleton ([`Self::skeleton`]).
    pub mesh: AssetId,
    /// Its character rig: humanoid bones, constraints, springs and expressions.
    pub rig: AssetId,
    /// The graph it plays.
    pub graph: AssetId,
    /// What sets its graph's draws (its blinks) apart from another's. A golden pins each kind's.
    pub seed: u32,
    /// The weapon it holds, if it holds one that isn't skinned in.
    pub held: Option<HeldWeapon>,
    /// The expression its face rests on, and how much of it, if any.
    pub expression: Option<(ExpressionPreset, f32)>,
    /// Its own character motion (M5 C4): its springs' tuning, IK weights and cloth. None moves by the
    /// content's ([`Content::character_motion`]).
    ///
    /// [`Content::character_motion`]: crate::content::Content::character_motion
    pub motion: Option<AssetId>,
}

impl PlayableLook {
    /// The look of the playable kind `kind`, from its cooked document.
    ///
    /// # Errors
    ///
    /// If its face rests on an expression no rig knows.
    pub fn of(kind: AssetId, playable: &Playable) -> Result<Self, String> {
        let expression = playable
            .expression
            .as_ref()
            .map(|rest| {
                ExpressionPreset::from_name(&rest.preset).map(|preset| (preset, rest.weight)).ok_or_else(
                    || format!("playable kind {kind} rests on {:?}, which no rig knows", rest.preset),
                )
            })
            .transpose()?;
        Ok(Self {
            kind,
            mesh: playable.mesh,
            rig: playable.rig,
            graph: playable.graph,
            seed: playable.seed,
            held: playable.held.as_ref().map(|held| HeldWeapon {
                mesh: held.mesh,
                joint: held.joint.clone(),
                socket: Mat4::from_rotation_translation(
                    Quat::from_array(held.rotation).normalize(),
                    Vec3::from_array(held.offset),
                ),
            }),
            expression,
            motion: playable.motion,
        })
    }

    /// Its skeleton: the mesh's sub-asset, as the cook derives it.
    #[must_use]
    pub fn skeleton(&self) -> AssetId {
        self.mesh.sub_asset(SKELETON_SUB_ASSET)
    }

    /// The joint of `skeleton`, its own, that holds its weapon, if it holds one.
    ///
    /// # Errors
    ///
    /// If the skeleton has no joint by that name: a socket on a joint the kind lacks.
    pub fn weapon_joint(&self, skeleton: &ArchivedSkeleton) -> Result<Option<usize>, String> {
        let Some(held) = &self.held else { return Ok(None) };
        let joint = skeleton.names.iter().position(|name| name.as_str() == held.joint);
        joint.map(Some).ok_or_else(|| {
            format!(
                "playable kind {}'s weapon is held by {:?}, which its skeleton lacks",
                self.kind, held.joint
            )
        })
    }
}

/// A weapon a playable kind holds, drawn on its own and placed by the joint that grips it.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldWeapon {
    /// Its mesh.
    pub mesh: AssetId,
    /// The joint that holds it, by the skeleton's name.
    pub joint: String,
    /// Its mesh in that joint's space: where a moves document's paths that follow the joint are.
    pub socket: Mat4,
}

#[cfg(test)]
mod tests {
    use sapphire_assets::skeleton::{Joint, JointTransform, Skeleton};
    use sapphire_moves::{HeldWeapon as HeldDocument, RestPose};

    use super::*;

    fn kind() -> Playable {
        let mesh = AssetId::from_bytes([1; 16]);
        Playable {
            name: "character-name-test".to_owned(),
            mesh,
            rig: mesh.sub_asset("rig"),
            graph: AssetId::from_bytes([2; 16]),
            seed: 4,
            held: Some(HeldDocument {
                mesh: AssetId::from_bytes([3; 16]),
                joint: "hand.r".to_owned(),
                offset: [0.0, 0.1, 0.0],
                rotation: [0.0, 0.0, 0.0, 2.0],
            }),
            expression: Some(RestPose { preset: "relaxed".to_owned(), weight: 0.35 }),
            motion: Some(AssetId::from_bytes([6; 16])),
        }
    }

    fn skeleton(names: &[&str]) -> sapphire_assets::material::AlignedVec {
        let joints = names
            .iter()
            .enumerate()
            .map(|(index, name)| Joint {
                name: (*name).to_owned(),
                parent: index.checked_sub(1).map(|parent| u16::try_from(parent).expect("a few")),
                rest: JointTransform::IDENTITY,
            })
            .collect::<Vec<_>>();
        Skeleton::new(&joints).to_cooked().expect("cooks")
    }

    #[test]
    fn a_look_is_its_documents() {
        let id = AssetId::from_bytes([5; 16]);
        let look = PlayableLook::of(id, &kind()).expect("a look");
        assert_eq!((look.kind, look.seed), (id, 4));
        assert_eq!(look.expression, Some((ExpressionPreset::Relaxed, 0.35)));
        assert_eq!(look.motion, Some(AssetId::from_bytes([6; 16])));
        let held = look.held.expect("a weapon");
        let at = held.socket.transform_point3(Vec3::ZERO);
        assert!(at.abs_diff_eq(Vec3::new(0.0, 0.1, 0.0), 1e-6), "{at}");
        assert!(held.socket.transform_vector3(Vec3::X).abs_diff_eq(Vec3::X, 1e-6), "normalized");
    }

    #[test]
    fn a_face_resting_on_no_preset_is_refused() {
        let mut playable = kind();
        playable.expression = Some(RestPose { preset: "smug".to_owned(), weight: 0.5 });
        let error = PlayableLook::of(AssetId::from_bytes([5; 16]), &playable).expect_err("no preset");
        assert!(error.contains("smug"), "{error}");
    }

    /// Its socket is on one of its own joints, or it isn't held.
    #[test]
    fn a_weapon_on_a_joint_the_skeleton_lacks_is_refused() {
        let look = PlayableLook::of(AssetId::from_bytes([5; 16]), &kind()).expect("a look");
        let has = skeleton(&["root", "hips", "hand.r"]);
        let has = Skeleton::read(&has).expect("reads");
        assert_eq!(look.weapon_joint(has), Ok(Some(2)));
        let lacks = skeleton(&["root", "hips", "J_Bip_R_Hand"]);
        let lacks = Skeleton::read(&lacks).expect("reads");
        let error = look.weapon_joint(lacks).expect_err("no such joint");
        assert!(error.contains("hand.r"), "{error}");
        let unarmed = PlayableLook { held: None, ..look };
        assert_eq!(unarmed.weapon_joint(lacks), Ok(None));
    }
}
