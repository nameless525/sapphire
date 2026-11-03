//! The player's sword in the character's right hand (`--blade on`, M2 O3).
//!
//! The mesh is `sapphire_pipeline::combat::sword`'s, and it draws as a character with no body, as the
//! enemy's blade does ([`crate::scene::Figure::of_item_at`]). Each frame the scene places it by the
//! hand's joint and [`socket`]: the same socket `content/combat/player/player.moves.ron` bakes its
//! `blade` path from, so what's drawn is what the hit volumes sweep.

use sapphire_assets::AssetId;
use sapphire_ecs::prelude::Entity;
use sapphire_math::{Mat4, Quat, Vec3};

/// The sword, by its sidecar's ID (`content/combat/player/sword.glb.meta`).
pub(crate) const SWORD: AssetId = AssetId::from_text("c4a07ad2-6d0e-4f3c-9b21-7e5a1f0c8d34");

/// The `blade` path's socket in `player.moves.ron`, in the right hand's space.
const SOCKET_OFFSET: [f32; 3] = [-0.234, -0.002, 0.417];
const SOCKET_ROTATION: [f32; 4] = [0.5818, -0.5914, -0.3876, 0.4019];

/// The sword's mesh in the right hand's space.
pub(crate) fn socket() -> Mat4 {
    Mat4::from_rotation_translation(
        Quat::from_array(SOCKET_ROTATION).normalize(),
        Vec3::from_array(SOCKET_OFFSET),
    )
}

/// The blade's entity, and where the held pose has the hand that holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeldBlade {
    pub(crate) entity: Entity,
    pub(crate) held_hand: Mat4,
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
        assert_eq!(paths, 6, "one for each attack, and the assist");
        assert_eq!(moves.matches(&offset).count(), paths, "{offset}");
        assert_eq!(moves.matches(&rotation).count(), paths, "{rotation}");
    }

    /// The hand's joint carries the mesh's middle out along the blade.
    #[test]
    fn the_blade_leaves_the_fist_along_its_y() {
        let at = socket().transform_point3(Vec3::new(0.0, -0.45, 0.0));
        assert!(at.length() < 0.1, "the grip's end is in the fist: {at}");
    }
}
