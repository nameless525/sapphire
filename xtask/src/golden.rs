//! `cargo xtask golden [--update] [--scene <name>]`: golden-image tests (spec §9 gate 3).
//!
//! Cooks the desktop content, builds the sandbox and `sapphire_testkit` for the host, then
//! runs `sapphire_testkit golden`, which owns the scenes and the comparison
//! (`sapphire_testkit::scenes`). It runs as a process because nothing may depend on a tool
//! crate (rule R1), as with `sapphire_pipeline`.

use std::process::Command;

use crate::platforms::{self, App, Options, SANDBOX};
use crate::workspace::{self, CargoBuild};

const TESTKIT: &str = "sapphire_testkit";

pub fn run(update: bool, scene: Option<&str>) -> anyhow::Result<()> {
    // Held while the sandbox runs, so no other xtask run recooks the content under it.
    let (content, sandbox) = platforms::desktop_build(&App::load(SANDBOX)?, &Options::default())?;
    let env = platforms::host_build_env()?;
    let testkit = workspace::build_binary(
        &CargoBuild { package: TESTKIT, target: None, release: false, features: &[], env: &env },
        TESTKIT,
    )?;
    let mut command = Command::new(testkit);
    command
        .arg("golden")
        .arg("--sandbox")
        .arg(&sandbox)
        .arg("--content")
        .arg(&content.dir)
        .arg("--goldens")
        .arg(workspace::root().join("tests").join("golden"))
        .arg("--out")
        .arg(workspace::output_dir().join("golden"));
    if update {
        command.arg("--update");
    }
    if let Some(scene) = scene {
        command.args(["--scene", scene]);
    }
    workspace::run(&mut command)
}
