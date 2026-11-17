//! What a game ships in place of the documents Sapphire compiles in.
//!
//! The numbers a simulation or a frame needs before anything's loaded are documents compiled into their
//! crates (`default_targeting.ron`, `default_body.ron`, `default_character_motion.ron`, `default_hud.ron`,
//! `combat_bindings.ron`). A game names its own in `content/game.documents.ron`, a
//! [`GameDocuments`]: the cook checks what it names and copies it beside the packs, under [`SHIPPED_DIR`], and
//! the game reads them at its start. They aren't assets: nothing streams them, nothing refers to them by ID,
//! and they go into no pack, so a game's own numbers move no pack's bytes.
//!
//! A cook always writes the manifest, empty when `content/` has none, so a game can't mistake old content for
//! having no documents of its own.
//!
//! **What a patch may change (M4 P1):** a document a replay's run reads ([`NamedDocument::simulation`]: the
//! targeting and body tuning, the flow and the flags) changes only with an app build, as everything the
//! simulation reads does (m4-plan §3 "What a patch may carry"), and so does this manifest, which says which
//! files those are. The rest are presentation (hair and cloth, the HUD, the bindings, which turn input into
//! the actions a replay records, and the screens), which a patch may change. The release manifest
//! (`crate::manifest`) records which is which, file by file.
//!
//! **Effects aren't here (M6 N1):** an effects library is an asset ([`crate::effects`]), cooked from a
//! `.effects.ron` into a pack and named by the cue table, so a version 1 manifest that names one is refused,
//! saying so.

use bevy_reflect::TypePath;
use sapphire_reflect::{Decoder, VersionError, Versioned};
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

/// The manifest's file: in `content/` where a game writes it, and under [`SHIPPED_DIR`] where it ships.
pub const MANIFEST_FILE: &str = "game.documents.ron";

/// Where a cook puts a game's documents, in the directory that holds its packs.
pub const SHIPPED_DIR: &str = "documents";

/// The documents a game has in place of Sapphire's: each a path under `content/`, none of them required.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::assets"]
#[serde(deny_unknown_fields)]
pub struct GameDocuments {
    /// A `TargetingTuning`: who a soft lock picks.
    #[serde(default)]
    pub targeting: Option<String>,
    /// A `BodyTuning`: how bodies place their feet and turn their heads, in the simulation (M3 S3b).
    #[serde(default)]
    pub body: Option<String>,
    /// A `CharacterMotion`: hair, cloth and how much of the IK shows.
    #[serde(default)]
    pub motion: Option<String>,
    /// A `HudDesign`: where the combat HUD's pieces go.
    #[serde(default)]
    pub hud: Option<String>,
    /// An `InputBindings`: combat's bindings.
    #[serde(default)]
    pub bindings: Option<String>,
    /// A `FlowDocument`: the game's flow, from its title through a run to a win (M3 G1).
    #[serde(default)]
    pub flow: Option<String>,
    /// A `FlagRegistry`: every flag the game's dialogues and flow may read or set (M3 Q2, G1).
    #[serde(default)]
    pub flags: Option<String>,
    /// A `MenuDocument`: the game's screens, and what each of their rows does (M3 U9).
    #[serde(default)]
    pub menus: Option<String>,
}

impl Versioned for GameDocuments {
    /// 1: M3 S3. G1's `flow` and `flags` and U9's `menus` are fields a version 1 manifest leaves out. 2: M6 N1,
    /// `effects` gone, since an effects library is a cooked asset: a version 1 manifest reads as it is unless it
    /// names one.
    const VERSION: u32 = 2;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        match version {
            1 => data.decode::<v1::GameDocuments>().map(Into::into).map_err(VersionError::Decode),
            found => Err(VersionError::TooOld { found, oldest: 1 }),
        }
    }
}

/// A version 1 manifest, which could name an effects library as a document.
mod v1 {
    use serde::Deserialize;

    /// What a version 1 manifest says, refused if it names an effects library.
    #[derive(Deserialize)]
    #[serde(try_from = "Unchecked")]
    pub(super) struct GameDocuments(super::GameDocuments);

    /// As written: version 2's fields, and `effects`.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Unchecked {
        #[serde(default)]
        targeting: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        motion: Option<String>,
        #[serde(default)]
        hud: Option<String>,
        #[serde(default)]
        bindings: Option<String>,
        #[serde(default)]
        effects: Option<String>,
        #[serde(default)]
        flow: Option<String>,
        #[serde(default)]
        flags: Option<String>,
        #[serde(default)]
        menus: Option<String>,
    }

    impl TryFrom<Unchecked> for GameDocuments {
        type Error = String;

        fn try_from(unchecked: Unchecked) -> Result<Self, String> {
            let Unchecked { targeting, body, motion, hud, bindings, effects, flow, flags, menus } = unchecked;
            if let Some(path) = effects {
                return Err(format!(
                    "`effects` names {path:?}, but an effects library is an asset now (M6 N1): give it a name \
                     ending `.effects.ron`, so a cook packs it, and name it by its sidecar's ID in the cue \
                     table's `effect_library`"
                ));
            }
            Ok(Self(super::GameDocuments { targeting, body, motion, hud, bindings, flow, flags, menus }))
        }
    }

    impl From<GameDocuments> for super::GameDocuments {
        fn from(documents: GameDocuments) -> Self {
            documents.0
        }
    }
}

/// One document a manifest names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedDocument<'a> {
    /// The manifest's field, for messages.
    pub field: &'static str,
    /// What its envelope has to say it is.
    pub type_name: &'static str,
    /// Its path under `content/`.
    pub path: &'a str,
    /// Whether a replay's run reads it, so that only an app build may change it (see the module docs).
    pub simulation: bool,
}

impl GameDocuments {
    /// Reads a manifest.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    /// What it names.
    pub fn named(&self) -> impl Iterator<Item = NamedDocument<'_>> {
        [
            ("targeting", "sapphire::gameplay::TargetingTuning", true, &self.targeting),
            ("body", "sapphire::body_pose::BodyTuning", true, &self.body),
            ("motion", "sapphire::anim::CharacterMotion", false, &self.motion),
            ("hud", "sapphire::ui::HudDesign", false, &self.hud),
            ("bindings", "sapphire::input::InputBindings", false, &self.bindings),
            ("flow", "sapphire::game::FlowDocument", true, &self.flow),
            ("flags", "sapphire::dialogue::FlagRegistry", true, &self.flags),
            ("menus", "sapphire::game::MenuDocument", false, &self.menus),
        ]
        .into_iter()
        .filter_map(|(field, type_name, simulation, path)| {
            path.as_deref().map(|path| NamedDocument { field, type_name, path, simulation })
        })
    }
}

/// Where a document named by `path` ships, from the root of an app's content.
#[must_use]
pub fn shipped_path(path: &str) -> String {
    format!("{SHIPPED_DIR}/{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(body: &str) -> String {
        format!("(type: \"sapphire::assets::GameDocuments\", version: 2, data: ({body}))")
    }

    #[test]
    fn a_manifest_says_what_it_names_and_nothing_else_is_required() {
        let empty = GameDocuments::from_ron(&manifest("")).expect("nothing is required");
        assert_eq!(empty.named().count(), 0);
        let some = GameDocuments::from_ron(&manifest("hud: Some(\"ui/my_hud.ron\")")).expect("reads");
        let named: Vec<NamedDocument<'_>> = some.named().collect();
        let hud = NamedDocument {
            field: "hud",
            type_name: "sapphire::ui::HudDesign",
            path: "ui/my_hud.ron",
            simulation: false,
        };
        assert_eq!(named, [hud]);
        assert_eq!(shipped_path(hud.path), "documents/ui/my_hud.ron");
    }

    /// A version 1 manifest reads as it was, but one naming an effects library is refused, saying where the
    /// library goes now; version 2 has no such field.
    #[test]
    fn a_version_1_manifest_reads_unless_it_names_effects() {
        let one =
            |body: &str| format!("(type: \"sapphire::assets::GameDocuments\", version: 1, data: ({body}))");
        let read = GameDocuments::from_ron(&one("hud: Some(\"my_hud.ron\")")).expect("reads");
        assert_eq!(read, GameDocuments { hud: Some("my_hud.ron".to_owned()), ..GameDocuments::default() });
        let refused =
            GameDocuments::from_ron(&one("effects: Some(\"my_effects.ron\")")).expect_err("effects");
        let refused = refused.to_string();
        assert!(refused.contains("my_effects.ron") && refused.contains("effect_library"), "{refused}");
        assert!(GameDocuments::from_ron(&one("huds: Some(\"my_hud.ron\")")).is_err(), "a typo still is one");
        let effects = GameDocuments::from_ron(&manifest("effects: Some(\"my_effects.ron\")"));
        assert!(effects.expect_err("not a field of version 2").to_string().contains("effects"));
    }

    /// A misspelled field would otherwise be a document nobody takes, and a game on Sapphire's numbers with
    /// nothing to say so.
    #[test]
    fn a_field_it_doesnt_know_is_refused_by_name() {
        let refused = GameDocuments::from_ron(&manifest("huds: Some(\"my_hud.ron\")")).expect_err("a typo");
        assert!(refused.to_string().contains("huds"), "{refused}");
    }
}
