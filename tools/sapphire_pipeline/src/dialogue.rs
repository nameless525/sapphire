//! Dialogue documents (M3 Q2): `.dialogue.ron` cooked to `.sdialogue`, a conversation.
//!
//! The document, its checks and the cooked format live in `sapphire_dialogue`, a plain-data crate a game
//! and the simulation read too. This module places them in the cook:
//!
//! - A dialogue reads one file besides itself, the content root's flag registry ([`REGISTRY`]), which is
//!   in its key: a flag added there recooks every dialogue. A registry past `MAX_FLAGS` stops the cook
//!   whether or not any dialogue names a flag, naming the file.
//! - A dialogue's conditions compile to one chunk with the pinned compiler scripts use, so its scan
//!   refuses here what a replay couldn't survive.
//! - Every key a dialogue shows is checked against every locale's table after the tree cooks
//!   ([`check_keys`]), as screens' are, since neither key holds the other's sources.

use std::path::{Path, PathBuf};

use sapphire_assets::rig::ExpressionPreset;
use sapphire_dialogue::{Dialogue, DialogueDocument, DialogueError, FlagRegistry};
use sapphire_strings::{StringTable, key_shows};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reads and checks the document. Bump the version whenever that changes output for the same document.
pub const IMPORTER: Stage = Stage { name: "dialogue document", version: 1 };

/// The `.sdialogue` writer. Bump the version whenever it changes output for the same document. 2: M5 C5's
/// faces on a line (`Dialogue` version 3).
pub const COOKER: Stage = Stage { name: "sdialogue", version: 2 };

/// What a dialogue source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".dialogue.ron";

/// The flag registry, at the content root.
pub const REGISTRY: &str = "flags.ron";

/// Import settings for dialogue documents: none yet. A struct, so settings can join without changing
/// existing sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DialogueSettings {}

/// A dialogue document that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum DialogueCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Dialogue(#[from] DialogueError),
    #[error("`{field}`: {name:?} is no VRM 1.0 expression preset")]
    Expression { field: String, name: String },
}

/// A flag registry that can't be read, or holds more than the simulation can.
#[derive(Debug, thiserror::Error)]
#[error("the flag registry {} can't be used", path.display())]
pub struct RegistryError {
    pub path: PathBuf,
    #[source]
    pub source: Box<dyn std::error::Error + Send + Sync>,
}

/// A key a dialogue shows that a locale can't.
#[derive(Debug, thiserror::Error)]
#[error("{} shows `{key}` (`{field}`), which locale `{locale}` has no string for", dialogue.display())]
pub struct MissingKey {
    pub dialogue: PathBuf,
    pub field: String,
    pub key: String,
    pub locale: String,
}

/// The content root's registry and its bytes, as a tree's dialogues read it: empty, with no bytes, if the
/// tree has none.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    pub flags: FlagRegistry,
    pub bytes: Option<Vec<u8>>,
}

impl Registry {
    /// Reads `source_root`'s registry, if it has one, and checks it.
    pub fn read(source_root: &Path) -> Result<Self, RegistryError> {
        let path = source_root.join(REGISTRY);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(RegistryError { path, source: Box::new(error) }),
        };
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| RegistryError { path: path.clone(), source: Box::new(error) })?;
        let flags =
            FlagRegistry::from_ron(text).map_err(|error| RegistryError { path, source: Box::new(error) })?;
        Ok(Self { flags, bytes: Some(bytes) })
    }
}

/// Whether `path` names a dialogue source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a dialogue source at `relative` writes: `guide.dialogue.ron` → `guide.sdialogue`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Cooks a document's bytes against the registry, read back through the runtime's check.
pub fn cook(bytes: &[u8], registry: &FlagRegistry) -> Result<Vec<u8>, DialogueCookError> {
    let document = DialogueDocument::from_ron(std::str::from_utf8(bytes)?)?;
    // A face is a preset by its VRM name, which `sapphire_dialogue` is beneath knowing (M5 C5).
    for (index, node) in document.nodes.iter().enumerate() {
        for (at, face) in node.say.iter().flat_map(|say| say.faces.iter()).enumerate() {
            if ExpressionPreset::from_name(&face.expression).is_none() {
                let field = format!("nodes[{index}].say.faces[{at}].expression");
                return Err(DialogueCookError::Expression { field, name: face.expression.clone() });
            }
        }
    }
    let dialogue = document.cook(registry, |source| {
        sapphire_bytecode::cook(source).map(drop).map_err(|error| error.to_string())
    })?;
    let cooked = dialogue.to_cooked()?;
    Dialogue::read(&cooked)?;
    Ok(cooked.into_vec())
}

/// Every key `dialogue` shows, with the field it's in.
fn keys(dialogue: &Dialogue) -> impl Iterator<Item = (String, &str)> {
    dialogue.presentation.nodes.iter().enumerate().flat_map(|(index, node)| {
        let say = node.say.iter().flat_map(move |say| {
            [
                (format!("nodes[{index}].say.speaker"), say.speaker.as_str()),
                (format!("nodes[{index}].say.line"), say.line.as_str()),
            ]
        });
        let choices =
            node.choices.iter().enumerate().map(move |(choice, at)| {
                (format!("nodes[{index}].choices[{choice}].line"), at.line.as_str())
            });
        say.chain(choices)
    })
}

/// Refuses a key a dialogue shows that a locale, with its fallbacks, has no string for. `dialogues` holds
/// each cooked dialogue by its source's path.
pub fn check_keys(dialogues: &[(PathBuf, Dialogue)], tables: &[StringTable]) -> Result<(), Box<MissingKey>> {
    for table in tables {
        let mut chain = vec![table];
        chain.extend(
            table.fallback.iter().filter_map(|locale| tables.iter().find(|other| other.locale == *locale)),
        );
        for (path, dialogue) in dialogues {
            if let Some((field, key)) = keys(dialogue).find(|(_, key)| key_shows(&chain, key).is_none()) {
                return Err(Box::new(MissingKey {
                    dialogue: path.clone(),
                    field,
                    key: key.to_owned(),
                    locale: table.locale.clone(),
                }));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sapphire_assets::AssetKind;
    use sapphire_dialogue::DIALOGUE_EXTENSION;

    use super::*;

    const GUIDE: &[u8] = include_bytes!("../../../crates/sapphire_dialogue/test_content/guide.dialogue.ron");

    fn registry() -> FlagRegistry {
        FlagRegistry { flags: vec!["met_guide".to_owned(), "ward_lit".to_owned()] }
    }

    #[test]
    fn a_source_is_known_by_its_whole_suffix_and_cooks_beside_itself() {
        assert!(is_source(Path::new("dialogue/guide.dialogue.ron")));
        assert!(is_source(Path::new("GUIDE.DIALOGUE.RON")));
        assert!(!is_source(Path::new("dialogue/guide.ron")));
        let output = output_path(Path::new("dialogue/guide.dialogue.ron"), DIALOGUE_EXTENSION);
        assert_eq!(output, Path::new("dialogue/guide.sdialogue"));
    }

    #[test]
    fn what_cooks_reads_back_as_its_kind_and_a_cook_is_repeatable() {
        let (one, two) = (cook(GUIDE, &registry()).unwrap(), cook(GUIDE, &registry()).unwrap());
        assert_eq!(one, two);
        let mut aligned = sapphire_serialize::cooked::AlignedVec::<16>::new();
        aligned.extend_from_slice(&one);
        assert!(Dialogue::read(&aligned).unwrap().needs_scripts());
        assert_eq!(AssetKind::Dialogue.references(&aligned).unwrap(), []);
        assert_eq!(crate::pack::kind(Path::new("guide.sdialogue")), Some(AssetKind::Dialogue));
    }

    #[test]
    fn a_face_that_is_no_preset_stops_the_cook_naming_the_field() {
        let text = std::str::from_utf8(GUIDE).unwrap().replacen("\"happy\"", "\"grin\"", 1);
        let error = cook(text.as_bytes(), &registry()).unwrap_err().to_string();
        assert!(error.contains("`nodes[2].say.faces[0].expression`"), "{error}");
    }

    #[test]
    fn a_flag_the_registry_lacks_stops_the_cook_naming_the_field() {
        let message =
            cook(GUIDE, &FlagRegistry { flags: vec!["ward_lit".to_owned()] }).unwrap_err().to_string();
        assert!(message.contains("`nodes[2].set[0]`") && message.contains("flags.ron"), "{message}");
    }

    #[test]
    fn a_registry_past_the_simulations_table_is_refused_naming_the_file() {
        let scratch = std::env::temp_dir().join(format!("sapphire-q2-registry-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        assert!(Registry::read(&scratch).unwrap().bytes.is_none(), "no registry is an empty one");
        let flags: Vec<String> =
            (0..=sapphire_dialogue::MAX_FLAGS).map(|index| format!("\"flag_{index}\"")).collect();
        let text = format!(
            r#"(type: "sapphire::dialogue::FlagRegistry", version: 1, data: (flags: [{}]))"#,
            flags.join(", ")
        );
        std::fs::write(scratch.join(REGISTRY), text).unwrap();
        let error = Registry::read(&scratch).unwrap_err();
        let shown = format!("{error}: {}", error.source);
        assert!(shown.contains(REGISTRY) && shown.contains("257 flags"), "{shown}");
        std::fs::remove_dir_all(&scratch).unwrap();
    }
}
