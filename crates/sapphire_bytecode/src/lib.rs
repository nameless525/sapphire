//! Luau bytecode as cooked data (spec §5.24, M3 L2), beneath the asset server and the cooker, neither of
//! which may link what runs it: `sapphire_script` stands on gameplay, and so on physics.
//!
//! Owns:
//!
//! - [`ScriptChunk`]: the `.sscript` format, one chunk's bytes in the cooked envelope.
//! - [`scan`]: what a chunk's bytes do that a replay can't survive, and [`check`], which refuses them.
//! - `compile`, with the `compile` feature: the one pinned way to make those bytes from source, at one
//!   level with libm's builtins off, and the handle a VM loads. Luau's implicit arithmetic text coercion
//!   and compiler folding through constant locals remain documented portability limits; the pinned
//!   cross-platform corpus checks samples rather than proving every source equivalent.
//!
//! Doesn't own: running them. That's `sapphire_script`'s, and what's here links no VM unless asked to
//! compile.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::CookedError;
use sapphire_serialize::cooked::AlignedVec;

#[cfg(feature = "compile")]
pub mod compile;
pub mod scan;

#[cfg(feature = "compile")]
pub use compile::cook;
pub use scan::{Findings, MAX_STRING, ScanError};

/// Conventional file extension for a cooked script.
pub const SCRIPT_EXTENSION: &str = "sscript";

/// What a script can get wrong before it runs.
#[derive(Debug, thiserror::Error)]
pub enum CookError {
    #[error(transparent)]
    Unreadable(#[from] ScanError),
    #[error("{0}")]
    Refused(String),
}

/// Reads a chunk and refuses it if it breaks a deterministic instruction/library rule: what a VM does to
/// everything it loads, and an asset's check. This is not a complete bytecode verifier or proof of
/// compiler provenance; success cannot admit bytes to execution.
pub fn check(bytes: &[u8]) -> Result<Findings, CookError> {
    let findings = scan::scan(bytes)?;
    match findings.violation() {
        Some(rule) => Err(CookError::Refused(rule)),
        None => Ok(findings),
    }
}

/// One chunk's bytes, cooked once with a pinned compiler, so a device never runs the compiler or its
/// folding. They're the same on every architecture, which is what lets a replay hold them in its one
/// `tables_hash`.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::bytecode"]
pub struct ScriptChunk {
    /// What `compile::cook` returned. Reading these back as something a VM runs takes the trust
    /// `compile::CookedScript::from_trusted_bytes` asks for: the cooker made them.
    pub bytecode: Vec<u8>,
}

impl Versioned for ScriptChunk {
    /// 1: M3 L2.
    const VERSION: u32 = 1;
}

/// A cooked script that can't be read or run.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error(transparent)]
    Refused(#[from] CookError),
}

impl ScriptChunk {
    /// The cooked file of `bytecode`.
    pub fn to_cooked(&self) -> Result<AlignedVec, ScriptError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and the chunk itself ([`check`]).
    pub fn read(bytes: &[u8]) -> Result<&ArchivedScriptChunk, ScriptError> {
        let script = sapphire_serialize::access::<Self>(bytes)?;
        check(script.bytecode.as_slice())?;
        Ok(script)
    }
}

#[cfg(all(test, feature = "compile"))]
mod tests {
    use super::*;

    #[test]
    fn a_cooked_script_reads_back_in_place() {
        let bytecode = cook("return { hook = function() return 1 end }").expect("cooks").into_bytes();
        let cooked = ScriptChunk { bytecode: bytecode.clone() }.to_cooked().expect("writes");
        assert_eq!(ScriptChunk::read(&cooked).expect("reads").bytecode.as_slice(), bytecode.as_slice());
    }

    /// Bytes from somewhere else get the same read a VM would give them: the scan isn't the cook's alone.
    #[test]
    fn a_chunk_the_scan_refuses_is_refused_as_an_asset() {
        let bytecode =
            compile::compile_unchecked("x = 1 return {}", compile::SHIPPING_LEVEL).expect("compiles");
        let cooked = ScriptChunk { bytecode }.to_cooked().expect("writes");
        let error = ScriptChunk::read(&cooked).unwrap_err();
        assert!(matches!(error, ScriptError::Refused(CookError::Refused(_))), "{error}");
    }

    #[test]
    fn bytes_that_arent_a_chunk_are_refused() {
        let cooked = ScriptChunk { bytecode: vec![1, 2, 3] }.to_cooked().expect("writes");
        assert!(matches!(ScriptChunk::read(&cooked), Err(ScriptError::Refused(CookError::Unreadable(_)))));
    }
}
