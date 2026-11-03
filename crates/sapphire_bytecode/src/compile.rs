//! Compiling a script: one pinned set of options, and the scan its bytes must pass.

use mlua::chunk::Compiler;

use crate::scan;
pub use crate::{CookError, check};

/// The optimization level a chunk ships at. Level 1 is the first with `FORGPREP_NEXT` and `POWK`, without
/// which a `pairs` can't be told from a bare table, nor `x ^ 2` from `x ^ y`. It's also the first that
/// folds, so what it leaves can hold the host's arithmetic: [`cook`] reads level 0 for that.
pub const SHIPPING_LEVEL: u8 = 1;

/// Library functions the compiler mustn't turn into `FASTCALL`, which would reach the C library or a
/// removed function whatever the global now holds. They're the replaced ones, plus everything removed
/// that a script might reasonably name, so that it fails with a nil global and not at the scan.
pub const DISABLED_BUILTINS: &[&str] = &[
    "math.acos",
    "math.asin",
    "math.atan",
    "math.atan2",
    "math.cos",
    "math.cosh",
    "math.exp",
    "math.fmod",
    "math.frexp",
    "math.ldexp",
    "math.log",
    "math.log10",
    "math.modf",
    "math.pow",
    "math.sin",
    "math.sinh",
    "math.tan",
    "math.tanh",
    "getmetatable",
    "setmetatable",
    "tonumber",
    "tostring",
];

/// The compiler for every chunk the VM runs.
#[must_use]
pub fn compiler(level: u8) -> Compiler {
    Compiler::new()
        .set_optimization_level(level)
        .set_debug_level(1)
        .set_type_info_level(0)
        .set_coverage_level(0)
        .set_disabled_builtins(DISABLED_BUILTINS.iter().map(|name| (*name).to_owned()))
}

/// Compiles without the checks, for tests that show what they catch.
pub fn compile_unchecked(source: &str, level: u8) -> Result<Vec<u8>, CookError> {
    compiler(level).compile(source).map_err(|error| CookError::Refused(error.to_string()))
}

/// Immutable bytecode produced by the pinned compiler. Safe execution (`sapphire_script::ScriptVm::load`)
/// accepts this provenance-bearing type rather than arbitrary bytes: Luau's binary loader assumes
/// structurally valid bytecode. Deterministic instruction/library rules are still checked whenever the
/// script is loaded. A cooked file holds the bytes as a [`ScriptChunk`](crate::ScriptChunk).
///
/// ```compile_fail
/// let script = sapphire_bytecode::cook("return {}").unwrap();
/// script.as_ref()[0] = 0;
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookedScript {
    bytes: Vec<u8>,
}

impl CookedScript {
    /// Restores bytecode whose compiler provenance the caller has independently established.
    /// Instruction/library policy is checked by `sapphire_script::ScriptVm::load`, not by this constructor.
    ///
    /// # Safety
    ///
    /// `bytes` must be complete, unmodified, structurally valid output of the exact pinned Luau
    /// compiler linked into this crate. A version tag or a successful [`check`] alone does not prove
    /// this: that scan enforces deterministic operations, not every bytecode reference/register
    /// invariant. Untrusted, corrupted or edited bytes must not be admitted this way.
    #[must_use]
    pub unsafe fn from_trusted_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// The bytes, for a cooked file.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl AsRef<[u8]> for CookedScript {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// What a cook does: refuses a script that breaks a rule, and returns the bytes to run.
///
/// Powers are read at level 0, where folding cannot hide them. Luau's compile directives and native
/// attributes can override that level. The cook reserves `--!optimize`, `--!native`, `@native` and
/// `@[` anywhere in source, including inside comments and strings. Listed attributes are reserved
/// as a group because they can contain a native attribute separated by whitespace or comments.
///
/// Baked scalar and vector constants must be finite. Scripts can still compute non-finite values
/// at runtime or receive them as hook arguments.
pub fn cook(source: &str) -> Result<CookedScript, CookError> {
    if ["--!optimize", "--!native", "@native", "@["].into_iter().any(|marker| source.contains(marker)) {
        return Err(CookError::Refused("a script doesn't choose its own compile options".to_owned()));
    }
    let unfolded = scan::scan(&compile_unchecked(source, 0)?)?;
    if let Some(rule) = unfolded.violation_unfolded() {
        return Err(CookError::Refused(rule));
    }
    let bytes = compile_unchecked(source, SHIPPING_LEVEL)?;
    check(&bytes)?;
    Ok(CookedScript { bytes })
}
