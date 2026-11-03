//! Timeline documents (M3 Q1): `.timeline.ron` cooked to `.stimeline`, a cutscene or a cinematic.
//!
//! The document, its checks and the cooked format all live in `sapphire_timeline`, a plain-data crate the
//! simulation and the sequencer read too. This module only places them in the cook: a source with a sidecar
//! like any asset (`importer: Timeline(())`) and no inputs besides itself, since what a timeline plays it
//! names, for the stage to have loaded.

use std::path::{Path, PathBuf};

use sapphire_timeline::{TimelineDocument, TimelineError};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reads and checks the document. Bump the version whenever that changes output for the same document.
pub const IMPORTER: Stage = Stage { name: "timeline document", version: 1 };

/// The `.stimeline` writer. Bump the version whenever it changes output for the same document.
pub const COOKER: Stage = Stage { name: "stimeline", version: 1 };

/// What a timeline source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".timeline.ron";

/// Import settings for timeline documents: none yet. A struct, so settings can join without changing
/// existing sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimelineSettings {}

/// A timeline document that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum TimelineCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Timeline(#[from] TimelineError),
}

/// Whether `path` names a timeline source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a timeline source at `relative` writes: `intro.timeline.ron` → `intro.stimeline`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Cooks a document's bytes.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, TimelineCookError> {
    let document = TimelineDocument::from_ron(std::str::from_utf8(bytes)?)?;
    Ok(document.cook()?.to_cooked()?.into_vec())
}

#[cfg(test)]
mod tests {
    use sapphire_assets::AssetKind;
    use sapphire_timeline::{TIMELINE_EXTENSION, Timeline};

    use super::*;

    const CUTSCENE: &[u8] =
        include_bytes!("../../../crates/sapphire_timeline/test_content/cutscene.timeline.ron");

    #[test]
    fn a_source_is_known_by_its_whole_suffix() {
        assert!(is_source(Path::new("sequences/intro.timeline.ron")));
        assert!(is_source(Path::new("INTRO.TIMELINE.RON")));
        assert!(!is_source(Path::new("sequences/intro.ron")));
        assert!(!is_source(Path::new("sequences/timeline.ron")));
    }

    #[test]
    fn a_source_cooks_beside_itself() {
        let output = output_path(Path::new("sequences/intro.timeline.ron"), TIMELINE_EXTENSION);
        assert_eq!(output, Path::new("sequences/intro.stimeline"));
    }

    #[test]
    fn what_cooks_reads_back_as_its_kind_and_a_cook_is_repeatable() {
        let (one, two) = (cook(CUTSCENE).unwrap(), cook(CUTSCENE).unwrap());
        assert_eq!(one, two);
        let mut aligned = sapphire_serialize::cooked::AlignedVec::<16>::new();
        aligned.extend_from_slice(&one);
        assert_eq!(Timeline::read(&aligned).unwrap().simulation.ticks, 0);
        assert_eq!(AssetKind::Timeline.references(&aligned).unwrap(), []);
        assert_eq!(crate::pack::kind(Path::new("intro.stimeline")), Some(AssetKind::Timeline));
    }

    #[test]
    fn a_bad_document_is_refused_with_its_field() {
        let text = std::str::from_utf8(CUTSCENE).unwrap().replacen("length: 6.0", "length: -1.0", 1);
        let message = cook(text.as_bytes()).unwrap_err().to_string();
        assert!(message.contains("`length`"), "{message}");
    }
}
