//! `cargo xtask content`: makes sure the sample sources exist, then cooks `content/` with
//! the pipeline binary (spec §5.27, §7.4).
//!
//! Every run recooks everything into a fresh directory; the content-hash cache and
//! `--only` arrive in M1.

use std::fmt;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;

use anyhow::anyhow;

use crate::workspace::{self, CargoBuild};

/// Source assets live here, relative to the workspace root.
pub const SOURCE_DIR: &str = "content";

/// Generated sample sources, relative to [`SOURCE_DIR`]. `gen-samples` writes all of them.
const SAMPLE_SOURCES: &[&str] = &["samples/mannequin.glb"];

/// A target cook (spec §2): `desktop` for macOS, Windows and Linux, `mobile` for iOS and
/// Android. Mirrors `sapphire_pipeline::cook::Cook`; xtask runs the pipeline as a
/// process rather than linking it, since nothing may depend on a tool crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cook {
    Desktop,
    Mobile,
}

impl Cook {
    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Mobile => "mobile",
        }
    }
}

impl fmt::Display for Cook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Cook {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        [Self::Desktop, Self::Mobile]
            .into_iter()
            .find(|cook| cook.name() == s)
            .ok_or_else(|| anyhow!("unknown cook `{s}`; expected `desktop` or `mobile`"))
    }
}

/// Where a cook's output goes: `target/content/<cook>/`.
pub fn cooked_dir(cook: Cook) -> PathBuf {
    workspace::output_dir().join("content").join(cook.name())
}

/// Generates missing sample sources, then cooks all of `content/` for `cook`. Returns
/// the cooked directory.
pub fn cook(cook: Cook) -> anyhow::Result<PathBuf> {
    // Release: cooking is the slow part, and the pipeline itself rarely changes.
    let pipeline = workspace::build_binary(
        &CargoBuild { package: "sapphire_pipeline", target: None, release: true, env: &[] },
        "sapphire_pipeline",
    )?;

    let source = workspace::root().join(SOURCE_DIR);
    if SAMPLE_SOURCES.iter().any(|sample| !source.join(sample).exists()) {
        workspace::run(Command::new(&pipeline).arg("gen-samples").arg("--out").arg(source.join("samples")))?;
    }

    // Start clean so assets deleted from `content/` don't linger in builds.
    let out = cooked_dir(cook);
    workspace::remove_dir_if_exists(&out)?;
    workspace::run(
        Command::new(&pipeline)
            .arg("cook")
            .arg("--source")
            .arg(&source)
            .arg("--out")
            .arg(&out)
            .args(["--cook", cook.name()]),
    )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cooks() {
        assert_eq!("desktop".parse::<Cook>().unwrap(), Cook::Desktop);
        assert_eq!("mobile".parse::<Cook>().unwrap(), Cook::Mobile);
        assert!("console".parse::<Cook>().is_err());
        assert!(cooked_dir(Cook::Mobile).ends_with("target/content/mobile"));
    }
}
