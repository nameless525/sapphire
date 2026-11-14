//! The playable kinds a fight knows (M5 C7, C8), each found by its kind as a kind of enemy is
//! ([`crate::enemy`]).
//!
//! - **What there is:** a playable kind is a document, `<name>.playable.ron` (`sapphire_moves`'s
//!   `PlayableDocument`), cooked beside its moves: its mesh and rig, its graph, the weapon it holds and the
//!   joint that holds it, the joints its body touches by, the face it rests on, and its name in the game's
//!   words. [`look`] reads one by its ID, as `enemy::Look::of` reads an enemy's; nothing here names a mesh,
//!   a rig, a socket or a joint.
//! - **The heroine** ([`KIND`], `content/combat/player/heroine.playable.ron`) is the kind a player is when
//!   nothing says otherwise.
//! - **Who plays what:** a fight's player stands as the kind its scenario's player is ([`of_scenario`]).

use sapphire_assets::AssetId;
use sapphire_stage::{Content, PlayableLook};

/// The heroine as a playable kind, by her document's sidecar ID: the kind a fight's player is when its
/// scenario names none.
pub const KIND: AssetId = AssetId::from_text("60976269-85cf-4c5a-a29b-4aeeb65f3793");

/// What draws the playable kind `kind`, from its cooked document.
///
/// # Errors
///
/// If the mounted packs hold no playable kind `kind`, or it doesn't read.
pub fn look(content: &mut Content, kind: AssetId) -> Result<PlayableLook, String> {
    content.playable_look(kind)
}

/// The kind the player of the scenario `scenario` stands as, which a stand reads the look of ([`look`]):
/// the heroine for every scenario, since none names one yet.
///
/// # Errors
///
/// None yet: a scenario that names a kind it doesn't hold will be one.
pub fn of_scenario(_scenario: &str) -> Result<AssetId, String> {
    Ok(KIND)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use sapphire_math::Vec3;
    use sapphire_moves::PlayableDocument;

    use super::*;

    /// The shipped document of the playable kind at `path` under `content/`, and its sidecar's ID.
    pub(crate) fn shipped(path: &str) -> (AssetId, PlayableDocument) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content").join(path);
        let text = std::fs::read_to_string(&path).expect("shipped");
        let document = PlayableDocument::parse(&text).expect("reads");
        let meta = std::fs::read_to_string(path.with_extension("ron.meta")).expect("its sidecar");
        let id = meta.split('"').skip_while(|part| !part.ends_with("id: ")).nth(1).expect("an ID");
        (AssetId::from_text(id), document)
    }

    /// The heroine's look, from her shipped document, with no content to cook it.
    pub(crate) fn heroine() -> PlayableLook {
        let (id, document) = shipped("combat/player/heroine.playable.ron");
        PlayableLook::of(id, &document.compile()).expect("a look")
    }

    #[test]
    fn the_heroine_is_her_documents_kind() {
        assert_eq!(heroine().kind, KIND);
    }

    /// What's drawn is what the hit volumes sweep: every path in the moves that follows the hand is the
    /// heroine's socket.
    #[test]
    fn the_socket_is_the_moves_blade_path() {
        let (_, heroine) = shipped("combat/player/heroine.playable.ron");
        let held = heroine.look.held.expect("she holds a sword");
        let moves =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat/player/player.moves.ron");
        let moves = std::fs::read_to_string(moves).expect("the player's moves");
        let [x, y, z] = held.offset;
        let offset = format!("offset: ({x}, {y}, {z})");
        let [x, y, z, w] = held.rotation;
        let rotation = format!("rotation: ({x}, {y}, {z}, {w})");
        let paths = moves.matches("follows: Bone(\"RightHand\")").count();
        assert_eq!(paths, 7, "one for each attack, the assist and the ultimate");
        assert_eq!(moves.matches(&offset).count(), paths, "{offset}");
        assert_eq!(moves.matches(&rotation).count(), paths, "{rotation}");
    }

    /// The hand's joint carries the mesh's middle out along the blade.
    #[test]
    fn the_blade_leaves_the_fist_along_its_y() {
        let socket = heroine().held.expect("she holds a sword").socket;
        let at = socket.transform_point3(Vec3::new(0.0, -0.45, 0.0));
        assert!(at.length() < 0.1, "the grip's end is in the fist: {at}");
    }
}
