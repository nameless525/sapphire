//! Scripts (`.luau`) cook to `.sscript` (M3 L2, spec §5.24): one chunk's bytecode, compiled with the
//! pinned options `sapphire_bytecode::compile` holds, and refused here if it breaks a rule, so a person
//! hears of it at a cook and not in a fight. A `.d.luau` beside them is what an editor reads, and isn't a
//! source.

use std::path::Path;

use sapphire_bytecode::{CookError, ScriptChunk, ScriptError, cook as compile};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reading the source, as cache keys know it. Bump the version whenever its output changes for the same
/// file.
pub const IMPORTER: Stage = Stage { name: "luau source", version: 1 };

/// Compiling and scanning. Bump the version with Luau, or with the compile options: either can change
/// the bytes, and so every replay's `tables_hash` that holds them.
pub const COOKER: Stage = Stage { name: "sscript", version: 1 };

/// What a script's file name ends with.
pub const SOURCE_SUFFIX: &str = ".luau";

/// What an editor's type definitions end with: `.luau`, and not a script.
const DEFINITIONS_SUFFIX: &str = ".d.luau";

/// Import settings for scripts: none yet. A struct, so settings can join without changing existing
/// sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptSettings {}

/// A script that didn't cook.
#[derive(Debug, thiserror::Error)]
pub enum ScriptCookError {
    #[error("the source isn't UTF-8")]
    NotUtf8(#[source] std::str::Utf8Error),
    #[error(transparent)]
    Refused(#[from] CookError),
    #[error(transparent)]
    Written(#[from] ScriptError),
}

/// Whether `path` names a script.
pub fn is_source(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        name.ends_with(SOURCE_SUFFIX) && !name.ends_with(DEFINITIONS_SUFFIX)
    })
}

/// The bytes of a script's cooked file, read back through the runtime's check.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, ScriptCookError> {
    let source = std::str::from_utf8(bytes).map_err(ScriptCookError::NotUtf8)?;
    let cooked = ScriptChunk { bytecode: compile(source)?.into_bytes() }.to_cooked()?;
    ScriptChunk::read(&cooked)?;
    Ok(cooked.into_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_arent_a_source() {
        assert!(is_source(Path::new("content/boss/phases.luau")));
        assert!(!is_source(Path::new("content/sapphire.d.luau")));
        assert!(!is_source(Path::new("content/boss/phases.lua")));
    }

    #[test]
    fn a_cook_is_one_set_of_bytes_a_runtime_reads() {
        let source = b"return { on_beat = function(tick) sapphire.mark(tick) end }";
        let cooked = cook(source).expect("cooks");
        assert_eq!(cook(source).expect("cooks again"), cooked);
        // `to_cooked` aligned it; a read needs the same, so it's read from where a decode would put it.
        let mut aligned: sapphire_serialize::cooked::AlignedVec =
            sapphire_serialize::cooked::AlignedVec::new();
        aligned.extend_from_slice(&cooked);
        assert!(!ScriptChunk::read(&aligned).expect("reads").bytecode.is_empty());
    }

    #[test]
    fn a_broken_rule_stops_the_cook() {
        let error = cook(b"return { hook = function(x) return x ^ 2.5 end }").unwrap_err();
        assert!(matches!(error, ScriptCookError::Refused(CookError::Refused(_))), "{error}");
        let error = cook(b"return {").unwrap_err();
        assert!(matches!(error, ScriptCookError::Refused(_)), "{error}");
        assert!(matches!(cook(&[0xff]), Err(ScriptCookError::NotUtf8(_))));
    }
}
