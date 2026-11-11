//! `cargo xtask store-shots [--scene-timeout <seconds>]`: the stores' screenshots (M4 S5), each golden scene of
//! the game in play drawn again at every store's sizes, in English and Japanese, into `target/store-shots/`,
//! which is never committed.
//!
//! As `golden` does, it cooks the desktop content and builds the sandbox and `sapphire_testkit` for the host,
//! then runs `sapphire_testkit store-shots`, which owns the shots and the sizes
//! (`sapphire_testkit::store_shots`): the same capture road as the goldens, at other sizes.

use std::process::Command;

use crate::platforms::{self, App, Options, SANDBOX};
use crate::workspace::{self, BuildProfile, CargoBuild};

const TESTKIT: &str = "sapphire_testkit";

/// Draws them all; `scene_timeout` is how long one capture may take, in seconds (the testkit's default when
/// `None`).
pub fn run(scene_timeout: Option<u64>) -> anyhow::Result<()> {
    let (content, sandbox) = platforms::desktop_build(&App::load(SANDBOX)?, &Options::default())?;
    let env = platforms::host_build_env()?;
    let testkit = workspace::build_binary(
        &CargoBuild { package: TESTKIT, target: None, profile: BuildProfile::Dev, features: &[], env: &env },
        TESTKIT,
    )?;
    let mut command = Command::new(testkit);
    command
        .arg("store-shots")
        .arg("--sandbox")
        .arg(sandbox.path())
        .arg("--content")
        .arg(&content.dir)
        .arg("--tests")
        .arg(workspace::root().join("tests"))
        .arg("--out")
        .arg(workspace::output_dir().join("store-shots"));
    if let Some(seconds) = scene_timeout {
        command.arg("--scene-timeout").arg(seconds.to_string());
    }
    workspace::run(&mut command)
}
