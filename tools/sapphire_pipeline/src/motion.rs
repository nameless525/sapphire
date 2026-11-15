//! Character motions (M5 C4): `.motion.ron` cooked to `.smotion`, a playable kind's own springs' tuning, IK
//! weights and cloth, which its document names (`look.motion`).
//!
//! The document and its checks are `sapphire_anim`'s (`CharacterMotion`), the cooked format
//! `sapphire_assets`' ([`sapphire_assets::motion`]). This module places them in the cook: a source with a
//! sidecar like any asset (`importer: Motion(())`) that reads nothing besides itself. That the chains its
//! cloth names are the rig's is the stage's to check when it puts the kind's rig and motion together, since
//! one motion may serve any kind whose rig has those chains.

use std::path::{Path, PathBuf};

use sapphire_anim::{CharacterMotion, CharacterMotionError};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reads and checks the document. Bump the version whenever that changes output for the same document.
pub const IMPORTER: Stage = Stage { name: "character motion document", version: 1 };

/// The `.smotion` writer. Bump the version whenever it changes output for the same document.
pub const COOKER: Stage = Stage { name: "smotion", version: 1 };

/// What a character motion's source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".motion.ron";

/// Import settings for character motions: none yet. A struct, so settings can join without changing existing
/// sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionSettings {}

/// A character motion that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum MotionCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Motion(#[from] CharacterMotionError),
}

/// Whether `path` names a character motion's source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a source at `relative` writes: `hero.motion.ron` → `hero.smotion`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Reads and checks a document's bytes.
pub fn read(bytes: &[u8]) -> Result<CharacterMotion, MotionCookError> {
    Ok(CharacterMotion::from_ron(std::str::from_utf8(bytes)?)?)
}

/// Cooks a document's bytes.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, MotionCookError> {
    Ok(read(bytes)?.to_cooked()?.into_vec())
}

#[cfg(test)]
mod tests {
    use sapphire_assets::motion::{MOTION_EXTENSION, Motion};

    use super::*;

    #[test]
    fn a_source_is_known_by_its_whole_suffix() {
        assert!(is_source(Path::new("combat/hero/hero.motion.ron")));
        assert!(is_source(Path::new("HERO.MOTION.RON")));
        assert!(!is_source(Path::new("combat/motion.ron")));
        assert!(!is_source(Path::new("default_character_motion.ron")));
    }

    #[test]
    fn a_source_cooks_beside_itself() {
        let output = output_path(Path::new("combat/hero/hero.motion.ron"), MOTION_EXTENSION);
        assert_eq!(output, Path::new("combat/hero/hero.smotion"));
    }

    /// The shipped motions cook, and what they cook reads back as the document said.
    #[test]
    fn the_shipped_motions_cook() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat");
        let mut motions = Vec::new();
        for kind in std::fs::read_dir(&root).expect("content/combat") {
            let kind = kind.expect("an entry").path();
            if kind.is_dir() {
                for file in std::fs::read_dir(&kind).expect("a kind's folder") {
                    let file = file.expect("an entry").path();
                    if file.to_string_lossy().ends_with(".motion.ron") {
                        motions.push(file);
                    }
                }
            }
        }
        motions.sort();
        assert!(!motions.is_empty(), "Aster's motion is shipped");
        for path in &motions {
            let motion = path.display();
            let bytes = std::fs::read(path).expect("shipped");
            let cooked = cook(&bytes).unwrap_or_else(|error| panic!("{motion}: {error}"));
            let mut aligned = sapphire_serialize::cooked::AlignedVec::<16>::new();
            aligned.extend_from_slice(&cooked);
            let read = Motion::read(&aligned).expect("reads");
            assert_eq!(CharacterMotion::from_cooked(read).expect("reads back"), super::read(&bytes).unwrap());
        }
    }

    /// Every shipped playable kind that names a motion of its own names a shipped motion whose cloth's chains
    /// are its rig's, as the stage checks when it loads the two together: Aster's skirt is her VRM's twelve
    /// panels.
    #[test]
    fn the_shipped_kinds_motions_fit_their_rigs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat");
        // Every source under `root` by its sidecar's ID.
        let mut sources = Vec::new();
        let mut folders = vec![root];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("readable") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.extension().is_some_and(|extension| extension == "meta") {
                    let meta = std::fs::read_to_string(&path).expect("a sidecar");
                    let id =
                        meta.split('"').skip_while(|part| !part.ends_with("id: ")).nth(1).expect("an ID");
                    sources.push((sapphire_assets::AssetId::from_text(id), path.with_extension("")));
                }
            }
        }
        let source = |id| sources.iter().find(|(of, _)| *of == id).map(|(_, path)| path.clone());
        let mut fitted = Vec::new();
        for (_, path) in sources.iter().filter(|(_, path)| path.to_string_lossy().ends_with(".playable.ron"))
        {
            let text = std::fs::read_to_string(path).expect("a kind");
            let look = sapphire_moves::PlayableDocument::parse(&text).expect("it reads").look;
            let Some(motion) = look.motion else { continue };
            let motion =
                source(motion).unwrap_or_else(|| panic!("{}: its motion is shipped", path.display()));
            let motion = read(&std::fs::read(&motion).expect("its motion")).expect("it reads");
            let model =
                source(look.mesh).unwrap_or_else(|| panic!("{}: its mesh is shipped", path.display()));
            let rig = crate::import::vrm::import_rig(&std::fs::read(&model).expect("its model"), None)
                .expect("its rig");
            let rig = rig.to_cooked().expect("cooked");
            let rig = sapphire_assets::rig::CharacterRig::read(&rig).expect("a valid rig");
            motion.check_rig(rig).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            fitted.push(path.file_name().expect("a name").to_string_lossy().into_owned());
        }
        // Whatever the documents say (M5 C12): a new kind with a motion of its own needs no change here.
        assert!(!fitted.is_empty(), "no shipped kind names a motion of its own");
    }

    #[test]
    fn a_sheet_of_one_chain_is_refused_naming_it() {
        let text = include_str!("../../../crates/sapphire_anim/default_character_motion.ron").replacen(
            "look_weight: 1.0,",
            "look_weight: 1.0, cloth: [(chains: [\"cape\"], closed: false, lateral_compliance: 0.0, \
             bend_compliance: 0.0)],",
            1,
        );
        let error = read(text.as_bytes()).expect_err("one chain").to_string();
        assert!(error.contains("cloth[0].chains"), "{error}");
    }
}
