//! Playable kinds (M5 C7): what draws a character a player plays, found by its kind, as an enemy's look is
//! found by its kind of enemy. The stage names no character of its own: whoever stands one hands [`Content`]
//! the kind's [`PlayableLook`], and everything after that (its held pose, its clips, its graph and the driver
//! over it, its IK, its clearance, the body the simulation binds) looks the kind up by its ID.
//!
//! - **Where a look comes from** is the caller's. A game reads it from what it knows of its kinds
//!   (`game_combat`'s `playable`); nothing here reads a document yet.
//! - **The scene's character** is one entity, which draws as the kind [`Content::character_kind`] names:
//!   the one [`crate::lookdev::hold`] last stood it as.
//! - **A kind's graph** is keyed by the kind ([`GraphKey::playable`]), so two kinds never share one by
//!   accident.
//!
//! [`Content`]: crate::content::Content
//! [`Content::character_kind`]: crate::content::Content::character_kind
//! [`GraphKey::playable`]: crate::content::GraphKey::playable

use sapphire_assets::AssetId;
use sapphire_assets::skeleton::SKELETON_SUB_ASSET;
use sapphire_math::Mat4;

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
    /// The weapon it holds in its right hand, if it holds one that isn't skinned in.
    pub held: Option<HeldWeapon>,
    /// The joints where its body meets what it touches, by its skeleton's names, for the body a fight's
    /// simulation binds to it.
    pub body_contacts: Vec<String>,
}

impl PlayableLook {
    /// Its skeleton: the mesh's sub-asset, as the cook derives it.
    #[must_use]
    pub fn skeleton(&self) -> AssetId {
        self.mesh.sub_asset(SKELETON_SUB_ASSET)
    }
}

/// A weapon a playable kind holds, drawn on its own and placed by the hand that grips it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeldWeapon {
    /// Its mesh.
    pub mesh: AssetId,
    /// Its mesh in the right hand's space: where a moves document's paths that follow the hand are.
    pub socket: Mat4,
}
