//! What a game ships in place of the documents Sapphire compiles in.
//!
//! The numbers a simulation or a frame needs before anything's loaded are documents compiled into their
//! crates (`default_targeting.ron`, `default_body.ron`, `default_character_motion.ron`, `default_hud.ron`,
//! `combat_bindings.ron`, `default_effects.ron`). A game names its own in `content/game.documents.ron`, a
//! [`GameDocuments`]: the cook checks what it names and copies it beside the packs, under [`SHIPPED_DIR`], and
//! the game reads them at its start. They aren't assets: nothing streams them, nothing refers to them by ID,
//! and they go into no pack, so a game's own numbers move no pack's bytes.
//!
//! A cook always writes the manifest, empty when `content/` has none, so a game can't mistake old content for
//! having no documents of its own.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
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
    /// An `EffectLibrary`: what a fight's cues and sword trails play.
    #[serde(default)]
    pub effects: Option<String>,
    /// A `FlowDocument`: the game's flow, from its title through a run to a win (M3 G1).
    #[serde(default)]
    pub flow: Option<String>,
    /// A `FlagRegistry`: every flag the game's dialogues and flow may read or set (M3 Q2, G1).
    #[serde(default)]
    pub flags: Option<String>,
}

impl Versioned for GameDocuments {
    /// 1: M3 S3. G1's `flow` and `flags` are fields a version 1 manifest leaves out.
    const VERSION: u32 = 1;
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
}

impl GameDocuments {
    /// Reads a manifest.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    /// What it names.
    pub fn named(&self) -> impl Iterator<Item = NamedDocument<'_>> {
        [
            ("targeting", "sapphire::gameplay::TargetingTuning", &self.targeting),
            ("body", "sapphire::body_pose::BodyTuning", &self.body),
            ("motion", "sapphire::anim::CharacterMotion", &self.motion),
            ("hud", "sapphire::ui::HudDesign", &self.hud),
            ("bindings", "sapphire::input::InputBindings", &self.bindings),
            ("effects", "sapphire::vfx::EffectLibrary", &self.effects),
            ("flow", "sapphire::game::FlowDocument", &self.flow),
            ("flags", "sapphire::dialogue::FlagRegistry", &self.flags),
        ]
        .into_iter()
        .filter_map(|(field, type_name, path)| {
            path.as_deref().map(|path| NamedDocument { field, type_name, path })
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
        format!("(type: \"sapphire::assets::GameDocuments\", version: 1, data: ({body}))")
    }

    #[test]
    fn a_manifest_says_what_it_names_and_nothing_else_is_required() {
        let empty = GameDocuments::from_ron(&manifest("")).expect("nothing is required");
        assert_eq!(empty.named().count(), 0);
        let some = GameDocuments::from_ron(&manifest("hud: Some(\"ui/my_hud.ron\")")).expect("reads");
        let named: Vec<NamedDocument<'_>> = some.named().collect();
        let hud = NamedDocument { field: "hud", type_name: "sapphire::ui::HudDesign", path: "ui/my_hud.ron" };
        assert_eq!(named, [hud]);
        assert_eq!(shipped_path(hud.path), "documents/ui/my_hud.ron");
    }

    /// A misspelled field would otherwise be a document nobody takes, and a game on Sapphire's numbers with
    /// nothing to say so.
    #[test]
    fn a_field_it_doesnt_know_is_refused_by_name() {
        let refused = GameDocuments::from_ron(&manifest("huds: Some(\"my_hud.ron\")")).expect_err("a typo");
        assert!(refused.to_string().contains("huds"), "{refused}");
    }
}
