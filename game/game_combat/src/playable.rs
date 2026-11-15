//! The playable kinds a fight knows (M5 C7, C8), each found by its kind as a kind of enemy is
//! ([`crate::enemy`]).
//!
//! - **What there is:** a playable kind is a document, `<name>.playable.ron` (`sapphire_moves`'s
//!   `PlayableDocument`), cooked beside its moves: its mesh and rig, its graph, the weapon it holds and the
//!   joint that holds it, the joints its body touches by, the face it rests on, and its name in the game's
//!   words. [`look`] reads one by its ID, as `enemy::Look::of` reads an enemy's; nothing here names a mesh,
//!   a rig, a socket or a joint. None of it reaches a step: the stock arena binds its player's body by its
//!   own numbers (`fight::stock_arena`, M5 Z3), and checks only that the kind it draws names that body's
//!   assets.
//! - **The heroine** ([`KIND`], `content/combat/player/heroine.playable.ron`) is the kind a player is when
//!   nothing says otherwise.
//! - **Who plays what:** a scenario's cast member names the kind she draws as (`playable: "hero"`, the
//!   document's file name), which has to play her set; the build compiles the documents in, as it does the
//!   scenarios. A fight's player stands as the kind its scenario's first member is ([`of_scenario`]), and a
//!   party's members each as their own ([`PartyKinds`]). A member who names none is the heroine, so a
//!   scenario that names none plays as it always did.

use sapphire_assets::AssetId;
use sapphire_gameplay::PARTY_SLOTS;
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

/// One of a party's members as a kind: its ID, and the key of its name if its scenario named it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartyMember {
    /// The playable kind it draws as.
    pub kind: AssetId,
    /// The key of its name in the game's words, `character-name-<kind>`, as its document names it, which the
    /// HUD calls it by: none for one its scenario didn't name, whose HUD says what it always has.
    pub name: Option<String>,
}

/// The playable kind each of a party's members draws as, by the party's slots (M5 C8).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartyKinds {
    slots: [Option<PartyMember>; PARTY_SLOTS],
}

impl PartyKinds {
    /// The member in each slot, none past the party's members.
    #[must_use]
    pub fn slots(&self) -> &[Option<PartyMember>; PARTY_SLOTS] {
        &self.slots
    }

    /// The kind in each slot, the heroine's past the party's members, where nobody comes in.
    #[must_use]
    pub fn kinds(&self) -> [AssetId; PARTY_SLOTS] {
        self.slots.each_ref().map(|member| member.as_ref().map_or(KIND, |member| member.kind))
    }

    /// Whether any member draws as another kind than the first's.
    #[must_use]
    pub fn is_mixed(&self) -> bool {
        let mut kinds = self.slots.iter().flatten().map(|member| member.kind);
        let first = kinds.next();
        kinds.any(|kind| Some(kind) != first)
    }
}

/// The kind each of the scenario named `scenario`'s cast draws as, in cast order: the heroine for one that
/// names none, with no name, and none for a scenario of code.
fn cast_kinds(scenario: &str) -> Result<Vec<PartyMember>, String> {
    let Some(document) = sapphire_script::scenarios::document_of(scenario) else { return Ok(Vec::new()) };
    let named = document.playable_kinds().map_err(|error| format!("`{scenario}`: {error}"))?;
    let member = |kind: Option<sapphire_gameplay::replay::KindOfPlayable>| match kind {
        Some(kind) => PartyMember { kind: kind.kind, name: Some(kind.name) },
        None => PartyMember { kind: KIND, name: None },
    };
    Ok(named.into_iter().map(member).collect())
}

/// The kind the player of the scenario named `scenario` stands as, which a stand reads the look of
/// ([`look`]): its first cast member's, the heroine unless that member names another.
///
/// # Errors
///
/// If its cast names a kind this build doesn't hold, or one that doesn't play its member's set.
pub fn of_scenario(scenario: &str) -> Result<AssetId, String> {
    Ok(cast_kinds(scenario)?.first().map_or(KIND, |member| member.kind))
}

/// The kind each of the scenario named `scenario`'s party draws as, by its slots, if it has a party.
///
/// # Errors
///
/// As [`of_scenario`].
pub fn party_of_scenario(scenario: &str) -> Result<Option<PartyKinds>, String> {
    let Some(party) =
        sapphire_script::scenarios::document_of(scenario).and_then(|document| document.party.as_ref())
    else {
        return Ok(None);
    };
    let cast = cast_kinds(scenario)?;
    let mut kinds = PartyKinds::default();
    for (slot, &member) in kinds.slots.iter_mut().zip(&party.members) {
        *slot = Some(cast.get(member).cloned().unwrap_or(PartyMember { kind: KIND, name: None }));
    }
    Ok(Some(kinds))
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

    /// The party of today is the heroine three times, each unnamed: what draws it is what drew it before.
    #[test]
    fn a_party_that_names_no_kind_is_the_heroine_three_times() {
        let kinds = party_of_scenario(crate::arena::PARTY_SCENARIO).expect("reads").expect("a party");
        assert_eq!(kinds.kinds(), [KIND; PARTY_SLOTS]);
        assert!(
            kinds.slots().iter().all(|member| member.as_ref().is_some_and(|member| member.name.is_none()))
        );
        assert!(!kinds.is_mixed());
        assert_eq!(of_scenario(crate::arena::PARTY_SCENARIO), Ok(KIND));
        assert_eq!(party_of_scenario(crate::arena::FIGHT_SCENARIO), Ok(None), "no party");
    }

    /// The two kinds' scenario: the heroine first, then Aster, each named as her document names her.
    #[test]
    fn the_two_kinds_party_is_the_heroine_and_aster() {
        let (aster, document) = shipped("combat/hero/hero.playable.ron");
        let kinds = party_of_scenario(crate::arena::KINDS_SCENARIO).expect("reads").expect("a party");
        let [first, second, third] = kinds.slots().clone();
        assert_eq!(first, Some(PartyMember { kind: KIND, name: Some("character-name-wanderer".to_owned()) }));
        assert_eq!(second, Some(PartyMember { kind: aster, name: Some(document.name) }));
        assert_eq!(third, None, "a party of two");
        assert!(kinds.is_mixed());
        assert_eq!(of_scenario(crate::arena::KINDS_SCENARIO), Ok(KIND));
    }

    /// The shipped playable kind whose file is `<name>.playable.ron`, anywhere under `content/combat/`, as
    /// the gameplay crate's build finds one a cast member names.
    fn shipped_named(name: &str) -> (AssetId, PlayableDocument) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let file = format!("{name}.playable.ron");
        let mut folders = vec![root.join("combat")];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("readable") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.file_name().is_some_and(|found| found.to_string_lossy() == file) {
                    let relative = path.strip_prefix(&root).expect("under content");
                    return shipped(&relative.to_string_lossy());
                }
            }
        }
        panic!("no shipped {file}")
    }

    /// The three kinds' scenario (M5 C12): the heroine, then Aster, then the kind its third member's
    /// document names, each named as its document names it. Nothing here knows the third kind's name: it's
    /// read from the scenario, as a kind added with no Rust is.
    #[test]
    fn the_three_kinds_party_is_the_heroine_aster_and_the_documents_third() {
        let scenario = crate::arena::THREE_KINDS_SCENARIO;
        let document = sapphire_script::scenarios::document_of(scenario).expect("compiled in");
        let party = document.party.as_ref().expect("a party");
        assert_eq!(party.members.len(), 3, "a party of three");
        let named = document.cast[party.members[2]].playable.as_deref().expect("its third names a kind");
        let (third, third_document) = shipped_named(named);
        let (aster, aster_document) = shipped("combat/hero/hero.playable.ron");
        let kinds = party_of_scenario(scenario).expect("reads").expect("a party");
        let [first, second, last] = kinds.slots().clone();
        assert_eq!(first, Some(PartyMember { kind: KIND, name: Some("character-name-wanderer".to_owned()) }));
        assert_eq!(second, Some(PartyMember { kind: aster, name: Some(aster_document.name) }));
        assert_eq!(last, Some(PartyMember { kind: third, name: Some(third_document.name.clone()) }));
        assert!(third != KIND && third != aster, "three kinds");
        assert!(kinds.is_mixed());
        assert_eq!(of_scenario(scenario), Ok(KIND));
        // It draws as itself: its own mesh and graph.
        let look = PlayableLook::of(third, &third_document.compile()).expect("a look");
        let aster_look =
            PlayableLook::of(aster, &shipped("combat/hero/hero.playable.ron").1.compile()).expect("a look");
        assert!(look.mesh != heroine().mesh && look.mesh != aster_look.mesh);
        assert!(look.graph != heroine().graph && look.graph != aster_look.graph);
    }

    /// Aster's look: her own mesh and rig, her own graph, nothing in her hands.
    #[test]
    fn aster_draws_as_herself() {
        let (id, document) = shipped("combat/hero/hero.playable.ron");
        let look = PlayableLook::of(id, &document.compile()).expect("a look");
        assert_ne!(look.mesh, heroine().mesh);
        assert_ne!(look.graph, heroine().graph);
        assert!(look.held.is_none(), "unarmed");
    }
}
