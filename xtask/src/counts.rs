//! `cargo xtask counts [--update] [--differences <file>] [--walk <name>]... [--walk-timeout <seconds>]`: CI's scale
//! lane (M5 X4).
//!
//! Cooks the desktop content and the benchmark's `ci` district beside it ([`bench::with_content`]), and the
//! mobile content and district the same way for the phone tiers' walks (M5 Z3), builds
//! the sandbox and `sapphire_testkit` as the golden scenes do, then runs `sapphire_testkit counts`, which
//! walks the district's tour at each tier's settings and holds what each walk counted to
//! `tests/counts/district-ci.txt` (`sapphire_testkit::counts`), or with `--differences` to the table plus a GPU
//! family's differences from it (lavapipe's: `tests/counts/district-ci-vulkan-lavapipe.txt`,
//! `sapphire_testkit::counts::differences`). It runs as a process because nothing may depend on a
//! tool crate (rule R1).

use std::process::Command;

use crate::bench::{self, Preset};
use crate::content::{self, Cook};
use crate::platforms::{self, App, Options, SANDBOX};
use crate::workspace::{self, BuildProfile, CargoBuild};

const TESTKIT: &str = "sapphire_testkit";

/// The committed table, relative to the workspace root.
pub const TABLE: &str = "tests/counts/district-ci.txt";

/// What `cargo xtask counts` runs.
pub struct Counts<'a> {
    pub update: bool,
    /// A GPU family's differences from the table, which the walks are held to beside it.
    pub differences: Option<&'a std::path::Path>,
    pub walks: &'a [String],
    /// Seconds a walk may take before the testkit stops it (its default when `None`).
    pub walk_timeout: Option<u64>,
}

pub fn run(counts: &Counts<'_>) -> anyhow::Result<()> {
    let (content, sandbox) = platforms::desktop_build(&App::load(SANDBOX)?, &Options::default())?;
    let content = bench::with_content(content)?;
    // The phone tiers' walks read the mobile cook, as a phone would (M5 Z3).
    let mobile = bench::with_content(content::cook(Cook::Mobile)?)?;
    let env = platforms::host_build_env()?;
    let testkit = workspace::build_binary(
        &CargoBuild { package: TESTKIT, target: None, profile: BuildProfile::Dev, features: &[], env: &env },
        TESTKIT,
    )?;
    let mut command = Command::new(testkit);
    command
        .arg("counts")
        .arg("--sandbox")
        .arg(sandbox.path())
        .arg("--content")
        .arg(&content.dir)
        .arg("--mobile-content")
        .arg(&mobile.dir)
        .arg("--sources")
        .arg(bench::source_dir(Preset::Ci))
        .arg("--mobile-sources")
        .arg(bench::source_dir_for(Preset::Ci, Cook::Mobile))
        .arg("--table")
        .arg(workspace::root().join(TABLE))
        .arg("--out")
        .arg(workspace::output_dir().join("counts"));
    if counts.update {
        command.arg("--update");
    }
    if let Some(differences) = counts.differences {
        command.arg("--differences").arg(differences);
    }
    for walk in counts.walks {
        command.args(["--walk", walk]);
    }
    if let Some(seconds) = counts.walk_timeout {
        command.arg("--walk-timeout").arg(seconds.to_string());
    }
    workspace::run(&mut command)
}
