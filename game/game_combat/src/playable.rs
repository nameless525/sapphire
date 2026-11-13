//! The playable kinds a fight knows (M5 C7), each found by its kind as a kind of enemy is
//! ([`crate::enemy`]).
//!
//! - **What there is:** one kind, the heroine ([`KIND`]): the character (`content/lookdev/character/`), its
//!   graph, and the sword in its right hand (`content/combat/player/`). No document holds a playable kind
//!   yet, so its look is written here ([`look`]), the one place a fight names it.
//! - **Who plays what:** a fight's player stands as the kind its scenario's player is ([`of_scenario`]).
//!   No scenario names one yet, so every player is the heroine.
//! - **C8 plugs in here:** a playable kind's document cooks its look, [`look`] reads it by its ID as
//!   `enemy::Look::of` reads an enemy's, and [`of_scenario`] reads each party member's kind from the
//!   scenario. Nothing else in the stage or the fight changes: each finds a character by its kind.

use sapphire_assets::AssetId;
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_stage::{HeldWeapon, PlayableLook};

/// The heroine as a playable kind: the kind a fight's player always is until a scenario names another. Its
/// playable document (C8) takes this ID.
pub const KIND: AssetId = AssetId::from_text("60976269-85cf-4c5a-a29b-4aeeb65f3793");

/// Its mesh, by its sidecar's ID (`content/lookdev/character/character.vrm.meta`).
const MESH: AssetId = AssetId::from_text("6f8a81b6-29c4-485f-b161-847b001d561f");
/// Its rig: the VRM's `rig` sub-asset, whose ID the pipeline derives from the VRM's
/// (`sapphire_pipeline::meta::sub_asset_id`; its look-dev tests pin this value).
const RIG: AssetId = AssetId::from_text("4b4073fd-44c5-8437-a712-1cccad486da3");
/// How its clips play (`--graph`), by its sidecar's ID
/// (`content/lookdev/character/character.animgraph.ron.meta`).
const GRAPH: AssetId = AssetId::from_text("b7f2c9a4-6e13-4d8a-a5c0-1e94d7b23f68");
/// What sets its blinks apart from another's. Any number: a golden pins this one's.
const SEED: u32 = 1;
/// The sword, by its sidecar's ID (`content/combat/player/sword.glb.meta`).
const SWORD: AssetId = AssetId::from_text("c4a07ad2-6d0e-4f3c-9b21-7e5a1f0c8d34");
/// The `blade` path's socket in `player.moves.ron`, in the right hand's space: so what's drawn is what the
/// hit volumes sweep.
const SOCKET_OFFSET: [f32; 3] = [-0.234, -0.002, 0.417];
const SOCKET_ROTATION: [f32; 4] = [0.5818, -0.5914, -0.3876, 0.4019];
/// The joints where its body meets what it touches, by the VRM's names.
const BODY_CONTACTS: [&str; 19] = [
    "J_Bip_C_Hips",
    "J_Bip_C_Spine",
    "J_Bip_C_UpperChest",
    "J_Bip_C_Neck",
    "J_Bip_C_Head",
    "J_Bip_L_UpperLeg",
    "J_Bip_L_LowerLeg",
    "J_Bip_L_Foot",
    "J_Bip_L_ToeBase",
    "J_Bip_R_UpperLeg",
    "J_Bip_R_LowerLeg",
    "J_Bip_R_Foot",
    "J_Bip_R_ToeBase",
    "J_Bip_L_UpperArm",
    "J_Bip_L_LowerArm",
    "J_Bip_L_Hand",
    "J_Bip_R_UpperArm",
    "J_Bip_R_LowerArm",
    "J_Bip_R_Hand",
];

/// What draws the playable kind `kind`.
///
/// # Errors
///
/// If no kind is `kind`.
pub fn look(kind: AssetId) -> Result<PlayableLook, String> {
    if kind != KIND {
        return Err(format!("no playable kind is {kind}: the heroine ({KIND}) is the one there is"));
    }
    Ok(PlayableLook {
        kind,
        mesh: MESH,
        rig: RIG,
        graph: GRAPH,
        seed: SEED,
        held: Some(HeldWeapon { mesh: SWORD, socket: socket() }),
        body_contacts: BODY_CONTACTS.map(str::to_owned).to_vec(),
    })
}

/// The heroine's look: [`look`] of [`KIND`].
#[must_use]
pub fn heroine() -> PlayableLook {
    look(KIND).expect("the heroine is a playable kind")
}

/// The kind the player of the scenario `scenario` stands as: the heroine's for every scenario, since none
/// names one yet.
///
/// # Errors
///
/// As [`look`].
pub fn of_scenario(_scenario: &str) -> Result<PlayableLook, String> {
    look(KIND)
}

/// The sword's mesh in the right hand's space.
fn socket() -> Mat4 {
    Mat4::from_rotation_translation(
        Quat::from_array(SOCKET_ROTATION).normalize(),
        Vec3::from_array(SOCKET_OFFSET),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What's drawn is what the hit volumes sweep: every path in the moves is this socket.
    #[test]
    fn the_socket_is_the_moves_blade_path() {
        let moves = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/combat/player/player.moves.ron");
        let moves = std::fs::read_to_string(moves).expect("the player's moves");
        let [x, y, z] = SOCKET_OFFSET;
        let offset = format!("offset: ({x}, {y}, {z})");
        let [x, y, z, w] = SOCKET_ROTATION;
        let rotation = format!("rotation: ({x}, {y}, {z}, {w})");
        let paths = moves.matches("follows: Bone(\"RightHand\")").count();
        assert_eq!(paths, 7, "one for each attack, the assist and the ultimate");
        assert_eq!(moves.matches(&offset).count(), paths, "{offset}");
        assert_eq!(moves.matches(&rotation).count(), paths, "{rotation}");
    }

    /// The hand's joint carries the mesh's middle out along the blade.
    #[test]
    fn the_blade_leaves_the_fist_along_its_y() {
        let at = socket().transform_point3(Vec3::new(0.0, -0.45, 0.0));
        assert!(at.length() < 0.1, "the grip's end is in the fist: {at}");
    }

    /// The rig is the VRM's own sub-asset, as the cook derives it, so the look can't name another's.
    #[test]
    fn the_rig_is_the_meshs() {
        assert_eq!(heroine().rig, MESH.sub_asset("rig"));
    }

    /// A kind that isn't one says which is.
    #[test]
    fn only_the_heroine_is_a_kind() {
        let other = AssetId::from_bytes([7; 16]);
        let error = look(other).expect_err("nobody plays it");
        assert!(error.contains(&KIND.to_string()), "{error}");
        assert_eq!(of_scenario("arena-fight"), Ok(heroine()));
    }
}
